//! Versioned persistence and deterministic fingerprints for non-contiguous similar-frame retrieval.
//!
//! This is intentionally separate from consecutive near-duplicate grouping. The authoritative
//! frame index remains the source of frame identity and timing. This module stores only disposable
//! descriptors keyed to strong source identity, exact stream identity, and the frame-timeline
//! contract generation.

use framescope_cache::{
    FRAME_TIMELINE_CONTRACT_GENERATION, FrameId, FrameIndexStreamIdentity, OwnedRgbaFrame,
    SourceIdentity,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

pub const GLOBAL_SIMILARITY_ALGORITHM_VERSION: u32 = 1;
pub const GLOBAL_SIMILARITY_STORE_SCHEMA_VERSION: u32 = 1;
pub const NORMALIZED_SIDE: usize = 12;
pub const NORMALIZED_RGB_BYTES: usize = NORMALIZED_SIDE * NORMALIZED_SIDE * 3;
pub const HASH_BANDS_PER_KIND: usize = 4;
pub const DEFAULT_MINIMUM_SIMILARITY: u16 = 9_300;
pub const DEFAULT_MAX_RESULTS: usize = 64;
const SIMILARITY_SCALE: u16 = 10_000;
const META_ROW_ID: i64 = 1;
const STATE_BUILDING: i64 = 1;
const STATE_COMPLETE: i64 = 2;
const WRITE_BATCH_DESCRIPTORS: usize = 128;
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalSimilarityStoreKey {
    pub source: SourceIdentity,
    pub stream: FrameIndexStreamIdentity,
}

impl GlobalSimilarityStoreKey {
    pub fn new(
        source: SourceIdentity,
        stream: FrameIndexStreamIdentity,
    ) -> Result<Self, GlobalSimilarityError> {
        if !source.is_reuse_safe() {
            return Err(GlobalSimilarityError::UnsafeSourceIdentity);
        }
        Ok(Self { source, stream })
    }

    fn persisted(&self) -> PersistedGlobalSimilarityKey {
        PersistedGlobalSimilarityKey {
            source: self.source.clone(),
            stream: self.stream.clone(),
            frame_timeline_contract_generation: FRAME_TIMELINE_CONTRACT_GENERATION,
            descriptor_algorithm_version: GLOBAL_SIMILARITY_ALGORITHM_VERSION,
        }
    }

    fn file_name(&self) -> String {
        format!(
            "stream-{}-a{}.sqlite3",
            self.stream.stream_index, GLOBAL_SIMILARITY_ALGORITHM_VERSION
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedGlobalSimilarityKey {
    source: SourceIdentity,
    stream: FrameIndexStreamIdentity,
    frame_timeline_contract_generation: u32,
    descriptor_algorithm_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalDescriptor {
    pub frame_id: FrameId,
    pub average_hash: u64,
    pub difference_hash: u64,
    pub normalized_rgb: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalSimilarityPolicy {
    pub minimum_similarity: u16,
    pub max_results: usize,
}

impl Default for GlobalSimilarityPolicy {
    fn default() -> Self {
        Self {
            minimum_similarity: DEFAULT_MINIMUM_SIMILARITY,
            max_results: DEFAULT_MAX_RESULTS,
        }
    }
}

impl GlobalSimilarityPolicy {
    pub fn validate(self) -> Result<Self, GlobalSimilarityError> {
        if self.minimum_similarity > SIMILARITY_SCALE {
            return Err(GlobalSimilarityError::InvalidThreshold(
                self.minimum_similarity,
            ));
        }
        if self.max_results == 0 {
            return Err(GlobalSimilarityError::InvalidMaxResults);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalSimilarityMatch {
    pub frame_id: FrameId,
    pub similarity: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalSimilarityQuery {
    pub target_frame: FrameId,
    pub descriptor_count: u64,
    pub candidate_count: u64,
    pub matched_count: u64,
    pub truncated: bool,
    pub matches: Vec<GlobalSimilarityMatch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalSimilarityStoreLoad {
    Missing,
    Reused { descriptor_count: u64 },
    InvalidatedStale,
    InvalidatedIncomplete,
    InvalidatedCorrupt,
}

#[derive(Debug, Error)]
pub enum GlobalSimilarityError {
    #[error("source identity is unsafe for persisted global-similarity reuse")]
    UnsafeSourceIdentity,
    #[error("global similarity threshold {0} is outside 0..=10000")]
    InvalidThreshold(u16),
    #[error("global similarity max_results must be greater than zero")]
    InvalidMaxResults,
    #[error("normalized descriptor length is invalid: {0}")]
    InvalidDescriptorLength(usize),
    #[error("target frame {0:?} is not present in the global-similarity store")]
    MissingTarget(FrameId),
    #[error(
        "global-similarity descriptor frame IDs are not contiguous: expected {expected:?}, got {actual:?}"
    )]
    NonContiguousFrameIds { expected: FrameId, actual: FrameId },
    #[error("global-similarity numeric value is outside the supported SQLite range: {0}")]
    NumericRange(&'static str),
    #[error("global-similarity store is not reusable: {0}")]
    NotReusable(&'static str),
    #[error("global-similarity store is incomplete or corrupt: {0}")]
    InvalidStore(String),
    #[error("RGBA frame layout overflows addressable memory")]
    FrameLayoutOverflow,
    #[error("global-similarity store I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("global-similarity SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("global-similarity key serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Build the deterministic descriptor used by candidate generation and final confirmation.
///
/// The descriptor is generated from source-quality RGBA pixels. Resolution normalization makes the
/// candidate robust to resize/re-encode pipelines while preserving RGB information for final
/// confirmation instead of reducing the decision to luma only.
pub fn descriptor_for_frame(
    frame_id: FrameId,
    frame: &OwnedRgbaFrame,
) -> Result<GlobalDescriptor, GlobalSimilarityError> {
    let normalized_rgb = resample_rgb(frame, NORMALIZED_SIDE, NORMALIZED_SIDE)?;
    let hash_rgb = resample_rgb(frame, 8, 8)?;
    let mut lumas = [0_u16; 64];
    let mut sum = 0_u32;
    for (index, pixel) in hash_rgb.chunks_exact(3).enumerate() {
        let luma = integer_luma(pixel[0], pixel[1], pixel[2]);
        lumas[index] = luma;
        sum += u32::from(luma);
    }
    let average = ((sum + 32) / 64) as u16;
    let mut average_hash = 0_u64;
    for (index, value) in lumas.iter().enumerate() {
        if *value >= average {
            average_hash |= 1_u64 << index;
        }
    }

    let difference_rgb = resample_rgb(frame, 9, 8)?;
    let mut difference_hash = 0_u64;
    for y in 0..8 {
        for x in 0..8 {
            let left = (y * 9 + x) * 3;
            let right = left + 3;
            let left_luma = integer_luma(
                difference_rgb[left],
                difference_rgb[left + 1],
                difference_rgb[left + 2],
            );
            let right_luma = integer_luma(
                difference_rgb[right],
                difference_rgb[right + 1],
                difference_rgb[right + 2],
            );
            if left_luma > right_luma {
                difference_hash |= 1_u64 << (y * 8 + x);
            }
        }
    }

    Ok(GlobalDescriptor {
        frame_id,
        average_hash,
        difference_hash,
        normalized_rgb,
    })
}

/// Deterministic color-aware confirmation score on the documented 0..=10_000 scale.
///
/// A one-cell normalized translation search absorbs small crop/resampling shifts. Only overlapping
/// cells are compared for a shifted candidate, avoiding artificial border duplication.
pub fn confirmation_similarity(left: &[u8], right: &[u8]) -> Result<u16, GlobalSimilarityError> {
    validate_descriptor_bytes(left)?;
    validate_descriptor_bytes(right)?;

    let mut best = 0_u16;
    for dy in -1_i32..=1 {
        for dx in -1_i32..=1 {
            let mut difference = 0_u64;
            let mut compared_channels = 0_u64;
            for y in 0..NORMALIZED_SIDE {
                let right_y = y as i32 + dy;
                if !(0..NORMALIZED_SIDE as i32).contains(&right_y) {
                    continue;
                }
                for x in 0..NORMALIZED_SIDE {
                    let right_x = x as i32 + dx;
                    if !(0..NORMALIZED_SIDE as i32).contains(&right_x) {
                        continue;
                    }
                    let left_index = (y * NORMALIZED_SIDE + x) * 3;
                    let right_index = (right_y as usize * NORMALIZED_SIDE + right_x as usize) * 3;
                    for channel in 0..3 {
                        difference += u64::from(
                            left[left_index + channel].abs_diff(right[right_index + channel]),
                        );
                        compared_channels += 1;
                    }
                }
            }
            if compared_channels == 0 {
                continue;
            }
            let maximum = compared_channels * 255;
            let difference_bps = ((difference * u64::from(SIMILARITY_SCALE) + maximum / 2)
                / maximum)
                .min(u64::from(SIMILARITY_SCALE));
            best = best.max((u64::from(SIMILARITY_SCALE) - difference_bps) as u16);
        }
    }
    Ok(best)
}

#[derive(Debug, Clone)]
pub struct GlobalSimilarityStore {
    root: PathBuf,
}

impl GlobalSimilarityStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path_for(&self, key: &GlobalSimilarityStoreKey) -> PathBuf {
        self.root
            .join(format!("global-v{GLOBAL_SIMILARITY_STORE_SCHEMA_VERSION}"))
            .join(key.source.stable_key())
            .join(key.file_name())
    }

    pub fn begin(
        &self,
        key: &GlobalSimilarityStoreKey,
    ) -> Result<GlobalSimilarityStoreWriter, GlobalSimilarityError> {
        if !key.source.is_reuse_safe() {
            return Err(GlobalSimilarityError::UnsafeSourceIdentity);
        }
        let final_path = self.path_for(key);
        let parent = final_path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "store path has no parent")
        })?;
        fs::create_dir_all(parent)?;
        let building_path = building_path(&final_path);
        purge_database_files(&building_path)?;

        let connection = Connection::open(&building_path)?;
        configure_connection(&connection)?;
        create_schema(&connection)?;
        connection.execute(
            "INSERT INTO global_similarity_meta (
                id, state, key_json, descriptor_count
             ) VALUES (?1, ?2, ?3, 0)",
            params![
                META_ROW_ID,
                STATE_BUILDING,
                serde_json::to_string(&key.persisted())?
            ],
        )?;

        Ok(GlobalSimilarityStoreWriter {
            connection,
            final_path,
            building_path,
            next_frame_id: 0,
            pending: Vec::with_capacity(WRITE_BATCH_DESCRIPTORS),
        })
    }

    pub fn load(
        &self,
        expected: &GlobalSimilarityStoreKey,
        expected_frame_count: u64,
    ) -> Result<GlobalSimilarityStoreLoad, GlobalSimilarityError> {
        let path = self.path_for(expected);
        if !path.exists() {
            return Ok(GlobalSimilarityStoreLoad::Missing);
        }
        if !has_sqlite_header(&path)? {
            purge_database_files(&path)?;
            return Ok(GlobalSimilarityStoreLoad::InvalidatedCorrupt);
        }

        let connection = Connection::open(&path)?;
        configure_connection(&connection)?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != i64::from(GLOBAL_SIMILARITY_STORE_SCHEMA_VERSION) {
            drop(connection);
            purge_database_files(&path)?;
            return Ok(GlobalSimilarityStoreLoad::InvalidatedStale);
        }

        let metadata: Option<(i64, String, i64)> = connection
            .query_row(
                "SELECT state, key_json, descriptor_count
                 FROM global_similarity_meta WHERE id = ?1",
                params![META_ROW_ID],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((state, key_json, descriptor_count)) = metadata else {
            drop(connection);
            purge_database_files(&path)?;
            return Ok(GlobalSimilarityStoreLoad::InvalidatedCorrupt);
        };
        if state != STATE_COMPLETE {
            drop(connection);
            purge_database_files(&path)?;
            return Ok(GlobalSimilarityStoreLoad::InvalidatedIncomplete);
        }

        let persisted: PersistedGlobalSimilarityKey = match serde_json::from_str(&key_json) {
            Ok(value) => value,
            Err(_) => {
                drop(connection);
                purge_database_files(&path)?;
                return Ok(GlobalSimilarityStoreLoad::InvalidatedCorrupt);
            }
        };
        if persisted != expected.persisted() {
            drop(connection);
            purge_database_files(&path)?;
            return Ok(GlobalSimilarityStoreLoad::InvalidatedStale);
        }
        let descriptor_count = from_sql_u64(descriptor_count, "descriptor count")?;
        if descriptor_count != expected_frame_count
            || !quick_check_ok(&connection)?
            || !validate_rows(&connection, descriptor_count)?
        {
            drop(connection);
            purge_database_files(&path)?;
            return Ok(GlobalSimilarityStoreLoad::InvalidatedCorrupt);
        }

        Ok(GlobalSimilarityStoreLoad::Reused { descriptor_count })
    }

    pub fn query(
        &self,
        expected: &GlobalSimilarityStoreKey,
        expected_frame_count: u64,
        target_frame: FrameId,
        policy: GlobalSimilarityPolicy,
    ) -> Result<GlobalSimilarityQuery, GlobalSimilarityError> {
        let policy = policy.validate()?;
        match self.load(expected, expected_frame_count)? {
            GlobalSimilarityStoreLoad::Reused { .. } => {}
            GlobalSimilarityStoreLoad::Missing => {
                return Err(GlobalSimilarityError::NotReusable("store is missing"));
            }
            GlobalSimilarityStoreLoad::InvalidatedStale => {
                return Err(GlobalSimilarityError::NotReusable("store was stale"));
            }
            GlobalSimilarityStoreLoad::InvalidatedIncomplete => {
                return Err(GlobalSimilarityError::NotReusable("store was incomplete"));
            }
            GlobalSimilarityStoreLoad::InvalidatedCorrupt => {
                return Err(GlobalSimilarityError::NotReusable("store was corrupt"));
            }
        }

        let path = self.path_for(expected);
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let target = read_descriptor(&connection, target_frame)?
            .ok_or(GlobalSimilarityError::MissingTarget(target_frame))?;

        let mut candidate_ids = BTreeSet::new();
        let mut statement = connection.prepare_cached(
            "SELECT frame_id FROM global_hash_bands
             WHERE kind = ?1 AND band = ?2 AND band_value = ?3",
        )?;
        for (kind, hash) in [
            (0_i64, target.average_hash),
            (1_i64, target.difference_hash),
        ] {
            for band in 0..HASH_BANDS_PER_KIND {
                let band_value = ((hash >> (band * 16)) & 0xffff) as i64;
                let rows = statement.query_map(params![kind, band as i64, band_value], |row| {
                    row.get::<_, i64>(0)
                })?;
                for row in rows {
                    let raw = row?;
                    let id = u64::try_from(raw).map_err(|_| {
                        GlobalSimilarityError::InvalidStore(format!(
                            "negative candidate frame id {raw}"
                        ))
                    })?;
                    if id >= expected_frame_count {
                        return Err(GlobalSimilarityError::InvalidStore(format!(
                            "candidate frame id {id} exceeds authoritative frame count {expected_frame_count}"
                        )));
                    }
                    if id != target_frame.0 {
                        candidate_ids.insert(FrameId(id));
                    }
                }
            }
        }

        let candidate_count = candidate_ids.len() as u64;
        let mut matches = Vec::new();
        for frame_id in candidate_ids {
            let candidate = read_descriptor(&connection, frame_id)?.ok_or_else(|| {
                GlobalSimilarityError::InvalidStore(format!(
                    "candidate frame {} has no descriptor row",
                    frame_id.0
                ))
            })?;
            let similarity =
                confirmation_similarity(&target.normalized_rgb, &candidate.normalized_rgb)?;
            if similarity >= policy.minimum_similarity {
                matches.push(GlobalSimilarityMatch {
                    frame_id,
                    similarity,
                });
            }
        }
        matches.sort_by(|left, right| {
            right
                .similarity
                .cmp(&left.similarity)
                .then_with(|| left.frame_id.0.cmp(&right.frame_id.0))
        });
        let matched_count = matches.len() as u64;
        let truncated = matches.len() > policy.max_results;
        matches.truncate(policy.max_results);
        Ok(GlobalSimilarityQuery {
            target_frame,
            descriptor_count: expected_frame_count,
            candidate_count,
            matched_count,
            truncated,
            matches,
        })
    }

    pub fn invalidate_source(&self, source: &SourceIdentity) -> Result<(), GlobalSimilarityError> {
        let source_key = source.stable_key();
        for schema_version in 1..=GLOBAL_SIMILARITY_STORE_SCHEMA_VERSION {
            let path = self
                .root
                .join(format!("global-v{schema_version}"))
                .join(&source_key);
            match fs::remove_dir_all(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

pub struct GlobalSimilarityStoreWriter {
    connection: Connection,
    final_path: PathBuf,
    building_path: PathBuf,
    next_frame_id: u64,
    pending: Vec<GlobalDescriptor>,
}

impl GlobalSimilarityStoreWriter {
    pub fn append(&mut self, descriptor: GlobalDescriptor) -> Result<(), GlobalSimilarityError> {
        let expected = FrameId(self.next_frame_id);
        if descriptor.frame_id != expected {
            return Err(GlobalSimilarityError::NonContiguousFrameIds {
                expected,
                actual: descriptor.frame_id,
            });
        }
        validate_descriptor_bytes(&descriptor.normalized_rgb)?;
        self.pending.push(descriptor);
        self.next_frame_id = self
            .next_frame_id
            .checked_add(1)
            .ok_or(GlobalSimilarityError::NumericRange("frame id"))?;
        if self.pending.len() >= WRITE_BATCH_DESCRIPTORS {
            self.flush_batch()?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<u64, GlobalSimilarityError> {
        self.flush_batch()?;
        let count = self.next_frame_id;
        self.connection.execute(
            "UPDATE global_similarity_meta
             SET state = ?1, descriptor_count = ?2 WHERE id = ?3",
            params![
                STATE_COMPLETE,
                to_sql_u64(count, "descriptor count")?,
                META_ROW_ID
            ],
        )?;
        self.connection.execute_batch("PRAGMA optimize;")?;
        drop(self.connection);

        purge_database_files(&self.final_path)?;
        fs::rename(&self.building_path, &self.final_path)?;
        sync_parent(&self.final_path)?;
        Ok(count)
    }

    fn flush_batch(&mut self) -> Result<(), GlobalSimilarityError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut descriptor_statement = transaction.prepare_cached(
                "INSERT INTO global_descriptors (
                    frame_id, average_hash, difference_hash, normalized_rgb
                 ) VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut band_statement = transaction.prepare_cached(
                "INSERT INTO global_hash_bands (
                    kind, band, band_value, frame_id
                 ) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for descriptor in &self.pending {
                descriptor_statement.execute(params![
                    to_sql_u64(descriptor.frame_id.0, "frame id")?,
                    descriptor.average_hash as i64,
                    descriptor.difference_hash as i64,
                    descriptor.normalized_rgb,
                ])?;
                for (kind, hash) in [
                    (0_i64, descriptor.average_hash),
                    (1_i64, descriptor.difference_hash),
                ] {
                    for band in 0..HASH_BANDS_PER_KIND {
                        let band_value = ((hash >> (band * 16)) & 0xffff) as i64;
                        band_statement.execute(params![
                            kind,
                            band as i64,
                            band_value,
                            to_sql_u64(descriptor.frame_id.0, "frame id")?,
                        ])?;
                    }
                }
            }
        }
        transaction.commit()?;
        self.pending.clear();
        Ok(())
    }
}

fn create_schema(connection: &Connection) -> Result<(), GlobalSimilarityError> {
    connection.execute_batch(
        "CREATE TABLE global_similarity_meta (
            id INTEGER PRIMARY KEY CHECK(id = 1),
            state INTEGER NOT NULL CHECK(state IN (1, 2)),
            key_json TEXT NOT NULL,
            descriptor_count INTEGER NOT NULL CHECK(descriptor_count >= 0)
         );
         CREATE TABLE global_descriptors (
            frame_id INTEGER PRIMARY KEY CHECK(frame_id >= 0),
            average_hash INTEGER NOT NULL,
            difference_hash INTEGER NOT NULL,
            normalized_rgb BLOB NOT NULL CHECK(length(normalized_rgb) = 432)
         );
         CREATE TABLE global_hash_bands (
            kind INTEGER NOT NULL CHECK(kind IN (0, 1)),
            band INTEGER NOT NULL CHECK(band BETWEEN 0 AND 3),
            band_value INTEGER NOT NULL CHECK(band_value BETWEEN 0 AND 65535),
            frame_id INTEGER NOT NULL CHECK(frame_id >= 0),
            PRIMARY KEY(kind, band, band_value, frame_id),
            FOREIGN KEY(frame_id) REFERENCES global_descriptors(frame_id) ON DELETE CASCADE
         );
         CREATE INDEX global_hash_band_lookup
            ON global_hash_bands(kind, band, band_value, frame_id);",
    )?;
    connection.pragma_update(None, "user_version", GLOBAL_SIMILARITY_STORE_SCHEMA_VERSION)?;
    Ok(())
}

fn configure_connection(connection: &Connection) -> Result<(), GlobalSimilarityError> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "journal_mode", "DELETE")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    Ok(())
}

fn validate_rows(
    connection: &Connection,
    expected_count: u64,
) -> Result<bool, GlobalSimilarityError> {
    let (count, minimum, maximum): (i64, Option<i64>, Option<i64>) = connection.query_row(
        "SELECT COUNT(*), MIN(frame_id), MAX(frame_id) FROM global_descriptors",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if from_sql_u64(count, "database descriptor count")? != expected_count {
        return Ok(false);
    }
    if expected_count == 0 {
        if minimum.is_some() || maximum.is_some() {
            return Ok(false);
        }
    } else if minimum != Some(0)
        || maximum != Some(to_sql_u64(expected_count - 1, "maximum frame id")?)
    {
        return Ok(false);
    }

    let invalid_descriptors: i64 = connection.query_row(
        "SELECT COUNT(*) FROM global_descriptors WHERE length(normalized_rgb) != ?1",
        params![NORMALIZED_RGB_BYTES as i64],
        |row| row.get(0),
    )?;
    if invalid_descriptors != 0 {
        return Ok(false);
    }

    let band_count: i64 =
        connection.query_row("SELECT COUNT(*) FROM global_hash_bands", [], |row| {
            row.get(0)
        })?;
    let expected_bands = expected_count
        .checked_mul((HASH_BANDS_PER_KIND * 2) as u64)
        .ok_or(GlobalSimilarityError::NumericRange("hash band count"))?;
    if from_sql_u64(band_count, "hash band count")? != expected_bands {
        return Ok(false);
    }

    let frames_with_wrong_band_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM (
            SELECT frame_id, COUNT(*) AS bands
            FROM global_hash_bands
            GROUP BY frame_id
            HAVING bands != ?1
         )",
        params![(HASH_BANDS_PER_KIND * 2) as i64],
        |row| row.get(0),
    )?;
    Ok(frames_with_wrong_band_count == 0)
}

fn read_descriptor(
    connection: &Connection,
    frame_id: FrameId,
) -> Result<Option<GlobalDescriptor>, GlobalSimilarityError> {
    let row: Option<(i64, i64, Vec<u8>)> = connection
        .query_row(
            "SELECT average_hash, difference_hash, normalized_rgb
             FROM global_descriptors WHERE frame_id = ?1",
            params![to_sql_u64(frame_id.0, "frame id")?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((average_hash, difference_hash, normalized_rgb)) = row else {
        return Ok(None);
    };
    validate_descriptor_bytes(&normalized_rgb)?;
    Ok(Some(GlobalDescriptor {
        frame_id,
        average_hash: average_hash as u64,
        difference_hash: difference_hash as u64,
        normalized_rgb,
    }))
}

fn validate_descriptor_bytes(bytes: &[u8]) -> Result<(), GlobalSimilarityError> {
    if bytes.len() != NORMALIZED_RGB_BYTES {
        return Err(GlobalSimilarityError::InvalidDescriptorLength(bytes.len()));
    }
    Ok(())
}

fn resample_rgb(
    frame: &OwnedRgbaFrame,
    output_width: usize,
    output_height: usize,
) -> Result<Vec<u8>, GlobalSimilarityError> {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let required = frame
        .stride_bytes
        .checked_mul(height)
        .ok_or(GlobalSimilarityError::FrameLayoutOverflow)?;
    if frame.pixels().len() < required {
        return Err(GlobalSimilarityError::FrameLayoutOverflow);
    }
    let output_len = output_width
        .checked_mul(output_height)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or(GlobalSimilarityError::FrameLayoutOverflow)?;
    let mut output = vec![0_u8; output_len];

    for output_y in 0..output_height {
        let source_y = fixed_point_coordinate(output_y, output_height, height);
        let y0 = (source_y >> 16) as usize;
        let y1 = (y0 + 1).min(height - 1);
        let fy = (source_y & 0xffff) as u32;
        for output_x in 0..output_width {
            let source_x = fixed_point_coordinate(output_x, output_width, width);
            let x0 = (source_x >> 16) as usize;
            let x1 = (x0 + 1).min(width - 1);
            let fx = (source_x & 0xffff) as u32;
            for channel in 0..3 {
                let p00 = u64::from(frame.pixels()[y0 * frame.stride_bytes + x0 * 4 + channel]);
                let p10 = u64::from(frame.pixels()[y0 * frame.stride_bytes + x1 * 4 + channel]);
                let p01 = u64::from(frame.pixels()[y1 * frame.stride_bytes + x0 * 4 + channel]);
                let p11 = u64::from(frame.pixels()[y1 * frame.stride_bytes + x1 * 4 + channel]);
                let top = p00 * u64::from(65_536 - fx) + p10 * u64::from(fx);
                let bottom = p01 * u64::from(65_536 - fx) + p11 * u64::from(fx);
                let value =
                    (top * u64::from(65_536 - fy) + bottom * u64::from(fy) + (1_u64 << 31)) >> 32;
                output[(output_y * output_width + output_x) * 3 + channel] = value.min(255) as u8;
            }
        }
    }
    Ok(output)
}

fn fixed_point_coordinate(output: usize, output_size: usize, input_size: usize) -> u64 {
    if output_size <= 1 || input_size <= 1 {
        return 0;
    }
    (output as u64 * (input_size - 1) as u64 * 65_536) / (output_size - 1) as u64
}

fn integer_luma(red: u8, green: u8, blue: u8) -> u16 {
    ((u32::from(red) * 77 + u32::from(green) * 150 + u32::from(blue) * 29 + 128) >> 8) as u16
}

fn quick_check_ok(connection: &Connection) -> Result<bool, GlobalSimilarityError> {
    let result: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    Ok(result == "ok")
}

fn has_sqlite_header(path: &Path) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let mut header = [0_u8; 16];
    match file.read_exact(&mut header) {
        Ok(()) => Ok(&header == SQLITE_HEADER),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error),
    }
}

fn building_path(path: &Path) -> PathBuf {
    let mut file_name = path.file_name().unwrap_or_default().to_os_string();
    file_name.push(".building");
    path.with_file_name(file_name)
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn purge_database_files(path: &Path) -> io::Result<()> {
    remove_if_present(path)?;
    remove_if_present(&sidecar_path(path, "-journal"))?;
    remove_if_present(&sidecar_path(path, "-wal"))?;
    remove_if_present(&sidecar_path(path, "-shm"))?;
    Ok(())
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "store path has no parent"))?;
    File::open(parent)?.sync_all()
}

fn to_sql_u64(value: u64, label: &'static str) -> Result<i64, GlobalSimilarityError> {
    i64::try_from(value).map_err(|_| GlobalSimilarityError::NumericRange(label))
}

fn from_sql_u64(value: i64, label: &'static str) -> Result<u64, GlobalSimilarityError> {
    u64::try_from(value).map_err(|_| GlobalSimilarityError::NumericRange(label))
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_core::TimeBase;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(1);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let id = TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "framescope-global-similarity-{label}-{}-{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn frame(
        width: u32,
        height: u32,
        mut pixel: impl FnMut(u32, u32) -> [u8; 3],
    ) -> OwnedRgbaFrame {
        let mut bytes = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                let [red, green, blue] = pixel(x, y);
                bytes.extend_from_slice(&[red, green, blue, 255]);
            }
        }
        OwnedRgbaFrame::new(width, height, width as usize * 4, bytes).unwrap()
    }

    fn base(width: u32, height: u32) -> OwnedRgbaFrame {
        frame(width, height, |x, y| {
            let checker = if ((x / 7) + (y / 5)) % 2 == 0 {
                36
            } else {
                180
            };
            [
                checker + ((x * 3 + y) % 40) as u8,
                30 + ((x + y * 2) % 110) as u8,
                210_u8.saturating_sub(((x * 2 + y * 3) % 120) as u8),
            ]
        })
    }

    fn resized(source: &OwnedRgbaFrame, width: u32, height: u32) -> OwnedRgbaFrame {
        frame(width, height, |x, y| {
            let source_x = if width <= 1 {
                0
            } else {
                x * (source.width - 1) / (width - 1)
            };
            let source_y = if height <= 1 {
                0
            } else {
                y * (source.height - 1) / (height - 1)
            };
            let index = source_y as usize * source.stride_bytes + source_x as usize * 4;
            [
                source.pixels()[index],
                source.pixels()[index + 1],
                source.pixels()[index + 2],
            ]
        })
    }

    fn unrelated(seed: u32) -> OwnedRgbaFrame {
        frame(64, 48, |x, y| {
            [
                ((x * (11 + seed) + y * 17 + seed * 7) % 256) as u8,
                ((x * 19 + y * (5 + seed) + 90 + seed * 13) % 256) as u8,
                ((x * (2 + seed) + y * 23 + seed * 29) % 256) as u8,
            ]
        })
    }

    fn source(tag: &str) -> SourceIdentity {
        SourceIdentity::new(12_345, Some(99), Some(format!("blake3-full-v2:{tag}")))
    }

    fn stream() -> FrameIndexStreamIdentity {
        FrameIndexStreamIdentity {
            stream_index: 0,
            codec_id: 27,
            codec_name: "h264".into(),
            time_base: TimeBase::new(1, 90_000).unwrap(),
            width: Some(64),
            height: Some(48),
        }
    }

    fn key(tag: &str) -> GlobalSimilarityStoreKey {
        GlobalSimilarityStoreKey::new(source(tag), stream()).unwrap()
    }

    fn candidate_shares_hash_band(left: &GlobalDescriptor, right: &GlobalDescriptor) -> bool {
        [
            (left.average_hash, right.average_hash),
            (left.difference_hash, right.difference_hash),
        ]
        .into_iter()
        .any(|(left_hash, right_hash)| {
            (0..HASH_BANDS_PER_KIND).any(|band| {
                ((left_hash >> (band * 16)) & 0xffff) == ((right_hash >> (band * 16)) & 0xffff)
            })
        })
    }

    #[test]
    fn deterministic_fixture_matrix_separates_requested_near_duplicates() {
        let original = base(64, 48);
        let identical = original.clone();
        let scaled = resized(&original, 40, 30);
        let recompressed = frame(64, 48, |x, y| {
            let index = y as usize * original.stride_bytes + x as usize * 4;
            [
                (original.pixels()[index] / 8) * 8,
                (original.pixels()[index + 1] / 8) * 8,
                (original.pixels()[index + 2] / 8) * 8,
            ]
        });
        let cropped = frame(58, 44, |x, y| {
            let source_x = x + 3;
            let source_y = y + 2;
            let index = source_y as usize * original.stride_bytes + source_x as usize * 4;
            [
                original.pixels()[index],
                original.pixels()[index + 1],
                original.pixels()[index + 2],
            ]
        });
        let color_shifted = frame(64, 48, |x, y| {
            let index = y as usize * original.stride_bytes + x as usize * 4;
            [
                original.pixels()[index].saturating_add(5),
                original.pixels()[index + 1].saturating_sub(3),
                original.pixels()[index + 2].saturating_add(4),
            ]
        });
        let adjacent = frame(64, 48, |x, y| {
            let index = y as usize * original.stride_bytes + x as usize * 4;
            let delta = if (x + y) % 17 == 0 { 2 } else { 0 };
            [
                original.pixels()[index].saturating_add(delta),
                original.pixels()[index + 1],
                original.pixels()[index + 2],
            ]
        });
        let unrelated = unrelated(3);

        let reference = descriptor_for_frame(FrameId(0), &original).unwrap();
        let positives = [
            ("identical", identical),
            ("scaled", scaled),
            ("recompressed", recompressed),
            ("cropped", cropped),
            ("color_shifted", color_shifted),
            ("adjacent", adjacent),
        ];
        for (offset, (name, candidate)) in positives.iter().enumerate() {
            let descriptor = descriptor_for_frame(FrameId(offset as u64 + 1), candidate).unwrap();
            let score =
                confirmation_similarity(&reference.normalized_rgb, &descriptor.normalized_rgb)
                    .unwrap();
            assert!(
                score >= DEFAULT_MINIMUM_SIMILARITY,
                "positive fixture {name} scored {score}"
            );
            assert!(
                candidate_shares_hash_band(&reference, &descriptor),
                "positive fixture {name} missed indexed candidate generation"
            );
        }

        let unrelated = descriptor_for_frame(FrameId(99), &unrelated).unwrap();
        let unrelated_score =
            confirmation_similarity(&reference.normalized_rgb, &unrelated.normalized_rgb).unwrap();
        assert!(
            unrelated_score < DEFAULT_MINIMUM_SIMILARITY,
            "unrelated fixture scored {unrelated_score}"
        );
    }

    #[test]
    fn persistent_query_finds_non_contiguous_matches_and_orders_deterministically() {
        let directory = TestDirectory::new("query");
        let store = GlobalSimilarityStore::new(directory.path());
        let key = key("query");
        let original = base(64, 48);
        let close = frame(64, 48, |x, y| {
            let index = y as usize * original.stride_bytes + x as usize * 4;
            [
                original.pixels()[index].saturating_add(2),
                original.pixels()[index + 1],
                original.pixels()[index + 2],
            ]
        });

        let mut writer = store.begin(&key).unwrap();
        let frames = [
            original.clone(),
            unrelated(1),
            close,
            unrelated(2),
            unrelated(4),
            unrelated(5),
            unrelated(6),
            original,
        ];
        for (frame_id, pixels) in frames.iter().enumerate() {
            writer
                .append(descriptor_for_frame(FrameId(frame_id as u64), pixels).unwrap())
                .unwrap();
        }
        assert_eq!(writer.finish().unwrap(), 8);
        assert_eq!(
            store.load(&key, 8).unwrap(),
            GlobalSimilarityStoreLoad::Reused {
                descriptor_count: 8
            }
        );

        let result = store
            .query(&key, 8, FrameId(0), GlobalSimilarityPolicy::default())
            .unwrap();
        assert_eq!(result.descriptor_count, 8);
        assert!(result.candidate_count >= 2);
        assert_eq!(result.matches[0].frame_id, FrameId(7));
        assert_eq!(result.matches[0].similarity, SIMILARITY_SCALE);
        assert!(
            result
                .matches
                .iter()
                .any(|item| item.frame_id == FrameId(2))
        );
        assert!(
            !result
                .matches
                .iter()
                .any(|item| item.frame_id == FrameId(0))
        );
    }

    #[test]
    fn persisted_store_is_invalidated_when_source_or_stream_contract_changes() {
        let directory = TestDirectory::new("stale");
        let store = GlobalSimilarityStore::new(directory.path());
        let original_key = key("source-a");
        let mut writer = store.begin(&original_key).unwrap();
        writer
            .append(descriptor_for_frame(FrameId(0), &base(64, 48)).unwrap())
            .unwrap();
        writer.finish().unwrap();

        let different_source = key("source-b");
        assert_eq!(
            store.load(&different_source, 1).unwrap(),
            GlobalSimilarityStoreLoad::Missing
        );

        let mut changed_stream = stream();
        changed_stream.codec_id = 173;
        changed_stream.codec_name = "hevc".into();
        let changed_stream_key =
            GlobalSimilarityStoreKey::new(source("source-a"), changed_stream).unwrap();
        assert_eq!(
            store.load(&changed_stream_key, 1).unwrap(),
            GlobalSimilarityStoreLoad::InvalidatedStale
        );
    }

    #[test]
    fn incomplete_or_corrupt_store_is_never_reused() {
        let directory = TestDirectory::new("corrupt");
        let store = GlobalSimilarityStore::new(directory.path());
        let key = key("corrupt");
        let path = store.path_for(&key);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not sqlite").unwrap();
        assert_eq!(
            store.load(&key, 1).unwrap(),
            GlobalSimilarityStoreLoad::InvalidatedCorrupt
        );
        assert!(!path.exists());
    }

    #[test]
    fn writer_rejects_frame_identity_gaps() {
        let directory = TestDirectory::new("gap");
        let store = GlobalSimilarityStore::new(directory.path());
        let key = key("gap");
        let mut writer = store.begin(&key).unwrap();
        let error = writer
            .append(descriptor_for_frame(FrameId(1), &base(64, 48)).unwrap())
            .unwrap_err();
        assert!(matches!(
            error,
            GlobalSimilarityError::NonContiguousFrameIds {
                expected: FrameId(0),
                actual: FrameId(1)
            }
        ));
    }
}
