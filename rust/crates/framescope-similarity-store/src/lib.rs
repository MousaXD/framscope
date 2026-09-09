//! Versioned, bounded, disposable persistence for derived frame-similarity groups.
//!
//! The source video and authoritative Phase 3 index remain truth. Similarity results are derived
//! cache state: incomplete, stale, or corrupt databases are never reused.

use framescope_cache::{FrameId, FrameIndexStreamIdentity, SourceIdentity};
use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};
use framescope_perceptual::{
    HYBRID_SIMILARITY_ALGORITHM_VERSION, HybridSimilarityEngine, HybridSimilarityPolicy,
};
use framescope_similarity::{FrameGroup, SIMILARITY_SCALE, SimilarityMode};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

pub const SIMILARITY_STORE_SCHEMA_VERSION: u32 = 4;
const META_ROW_ID: i64 = 1;
const STATE_BUILDING: i64 = 1;
const STATE_COMPLETE: i64 = 2;
const WRITE_BATCH_GROUPS: usize = 128;
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimilarityStoreConfig {
    Basic(SimilarityMode),
    Hybrid(HybridSimilarityPolicy),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimilarityStoreKey {
    pub source: SourceIdentity,
    pub stream: FrameIndexStreamIdentity,
    pub config: SimilarityStoreConfig,
}

impl SimilarityStoreKey {
    pub fn new(
        source: SourceIdentity,
        stream: FrameIndexStreamIdentity,
        mode: SimilarityMode,
    ) -> Result<Self, SimilarityStoreError> {
        mode.validate()
            .map_err(|error| SimilarityStoreError::InvalidConfig(error.to_string()))?;
        Self::for_config(source, stream, SimilarityStoreConfig::Basic(mode))
    }

    pub fn new_hybrid(
        source: SourceIdentity,
        stream: FrameIndexStreamIdentity,
        policy: HybridSimilarityPolicy,
    ) -> Result<Self, SimilarityStoreError> {
        HybridSimilarityEngine::new(policy)
            .map_err(|error| SimilarityStoreError::InvalidConfig(error.to_string()))?;
        Self::for_config(source, stream, SimilarityStoreConfig::Hybrid(policy))
    }

    fn for_config(
        source: SourceIdentity,
        stream: FrameIndexStreamIdentity,
        config: SimilarityStoreConfig,
    ) -> Result<Self, SimilarityStoreError> {
        if !source.is_reuse_safe() {
            return Err(SimilarityStoreError::UnsafeSourceIdentity);
        }
        Ok(Self {
            source,
            stream,
            config,
        })
    }

    fn file_name(&self) -> String {
        let suffix = match self.config {
            SimilarityStoreConfig::Basic(SimilarityMode::Exact) => "exact".to_owned(),
            SimilarityStoreConfig::Basic(SimilarityMode::LumaMeanAbsolute {
                minimum_similarity,
            }) => format!("luma-{minimum_similarity}"),
            SimilarityStoreConfig::Hybrid(HybridSimilarityPolicy {
                max_hash_distance,
                minimum_luma_similarity,
            }) => format!(
                "hybrid-a{HYBRID_SIMILARITY_ALGORITHM_VERSION}-h{max_hash_distance}-l{minimum_luma_similarity}"
            ),
        };
        format!("stream-{}-{suffix}.sqlite3", self.stream.stream_index)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimilarityStoreLoad {
    Missing,
    Reused { group_count: u64 },
    InvalidatedStale,
    InvalidatedIncomplete,
    InvalidatedCorrupt,
}

#[derive(Debug, Error)]
pub enum SimilarityStoreError {
    #[error("source identity is unsafe for persisted similarity reuse")]
    UnsafeSourceIdentity,
    #[error("invalid similarity store configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid frame group: {0}")]
    InvalidGroup(String),
    #[error("similarity store numeric value is outside the supported SQLite range: {0}")]
    NumericRange(&'static str),
    #[error("similarity store I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("similarity store SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("similarity store key serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedKey {
    source: SourceIdentity,
    stream: FrameIndexStreamIdentity,
    config: PersistedConfig,
}

impl From<&SimilarityStoreKey> for PersistedKey {
    fn from(value: &SimilarityStoreKey) -> Self {
        Self {
            source: value.source.clone(),
            stream: value.stream.clone(),
            config: value.config.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PersistedConfig {
    Exact,
    LumaMeanAbsolute {
        minimum_similarity: u16,
    },
    Hybrid {
        algorithm_version: u32,
        max_hash_distance: u8,
        minimum_luma_similarity: u16,
    },
}

impl From<SimilarityStoreConfig> for PersistedConfig {
    fn from(value: SimilarityStoreConfig) -> Self {
        match value {
            SimilarityStoreConfig::Basic(SimilarityMode::Exact) => Self::Exact,
            SimilarityStoreConfig::Basic(SimilarityMode::LumaMeanAbsolute {
                minimum_similarity,
            }) => Self::LumaMeanAbsolute { minimum_similarity },
            SimilarityStoreConfig::Hybrid(HybridSimilarityPolicy {
                max_hash_distance,
                minimum_luma_similarity,
            }) => Self::Hybrid {
                algorithm_version: HYBRID_SIMILARITY_ALGORITHM_VERSION,
                max_hash_distance,
                minimum_luma_similarity,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct SimilarityStore {
    root: PathBuf,
}

impl SimilarityStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path_for(&self, key: &SimilarityStoreKey) -> PathBuf {
        self.root
            .join(format!("v{SIMILARITY_STORE_SCHEMA_VERSION}"))
            .join(key.source.stable_key())
            .join(key.file_name())
    }

    pub fn begin(
        &self,
        key: &SimilarityStoreKey,
    ) -> Result<SimilarityStoreWriter, SimilarityStoreError> {
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
        let key_json = serde_json::to_string(&PersistedKey::from(key))?;
        connection.execute(
            "INSERT INTO similarity_meta (id, state, key_json, group_count)
             VALUES (?1, ?2, ?3, 0)",
            params![META_ROW_ID, STATE_BUILDING, key_json],
        )?;

        Ok(SimilarityStoreWriter {
            connection,
            final_path,
            building_path,
            pending: Vec::with_capacity(WRITE_BATCH_GROUPS),
            next_ordinal: 0,
        })
    }

    pub fn visit_groups<F>(
        &self,
        expected: &SimilarityStoreKey,
        visitor: F,
    ) -> Result<SimilarityStoreLoad, SimilarityStoreError>
    where
        F: FnMut(FrameGroup),
    {
        self.visit_groups_with_frame_count(expected, None, visitor)
    }

    /// Validate a reusable contiguous grouping result against the authoritative completed frame
    /// count. Derived state with a missing first frame, a gap, an overlap, or a wrong final frame is
    /// invalidated rather than repaired optimistically.
    pub fn visit_groups_for_frame_count<F>(
        &self,
        expected: &SimilarityStoreKey,
        expected_frame_count: u64,
        visitor: F,
    ) -> Result<SimilarityStoreLoad, SimilarityStoreError>
    where
        F: FnMut(FrameGroup),
    {
        self.visit_groups_with_frame_count(expected, Some(expected_frame_count), visitor)
    }

    fn visit_groups_with_frame_count<F>(
        &self,
        expected: &SimilarityStoreKey,
        expected_frame_count: Option<u64>,
        mut visitor: F,
    ) -> Result<SimilarityStoreLoad, SimilarityStoreError>
    where
        F: FnMut(FrameGroup),
    {
        let path = self.path_for(expected);
        if !path.exists() {
            return Ok(SimilarityStoreLoad::Missing);
        }
        if !has_sqlite_header(&path)? {
            purge_database_files(&path)?;
            return Ok(SimilarityStoreLoad::InvalidatedCorrupt);
        }

        let connection = Connection::open(&path)?;
        configure_connection(&connection)?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != i64::from(SIMILARITY_STORE_SCHEMA_VERSION) {
            drop(connection);
            purge_database_files(&path)?;
            return Ok(SimilarityStoreLoad::InvalidatedStale);
        }

        let meta: Option<(i64, String, i64)> = connection
            .query_row(
                "SELECT state, key_json, group_count FROM similarity_meta WHERE id = ?1",
                params![META_ROW_ID],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((state, key_json, stored_count)) = meta else {
            drop(connection);
            purge_database_files(&path)?;
            return Ok(SimilarityStoreLoad::InvalidatedCorrupt);
        };
        if state != STATE_COMPLETE {
            drop(connection);
            purge_database_files(&path)?;
            return Ok(SimilarityStoreLoad::InvalidatedIncomplete);
        }

        let persisted_key: PersistedKey = match serde_json::from_str(&key_json) {
            Ok(value) => value,
            Err(_) => {
                drop(connection);
                purge_database_files(&path)?;
                return Ok(SimilarityStoreLoad::InvalidatedCorrupt);
            }
        };
        if persisted_key != PersistedKey::from(expected) {
            drop(connection);
            purge_database_files(&path)?;
            return Ok(SimilarityStoreLoad::InvalidatedStale);
        }
        let group_count = from_sql_u64(stored_count, "stored group count")?;

        if !quick_check_ok(&connection)?
            || !validate_rows(&connection, group_count, expected_frame_count)?
        {
            drop(connection);
            purge_database_files(&path)?;
            return Ok(SimilarityStoreLoad::InvalidatedCorrupt);
        }

        let mut statement = connection.prepare_cached(
            "SELECT representative_frame, first_frame, last_frame, frame_count,
                    start_ticks, start_tb_num, start_tb_den,
                    end_ticks, end_tb_num, end_tb_den,
                    start_duration_ticks, start_duration_tb_num, start_duration_tb_den,
                    end_duration_ticks, end_duration_tb_num, end_duration_tb_den,
                    representative_similarity_floor
             FROM similarity_groups ORDER BY ordinal",
        )?;
        let rows = statement.query_map([], decode_group)?;
        for row in rows {
            visitor(row?);
        }
        Ok(SimilarityStoreLoad::Reused { group_count })
    }

    pub fn invalidate_source(&self, source: &SourceIdentity) -> Result<(), SimilarityStoreError> {
        let source_key = source.stable_key();
        for schema_version in 1..=SIMILARITY_STORE_SCHEMA_VERSION {
            let path = self
                .root
                .join(format!("v{schema_version}"))
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

pub struct SimilarityStoreWriter {
    connection: Connection,
    final_path: PathBuf,
    building_path: PathBuf,
    pending: Vec<FrameGroup>,
    next_ordinal: u64,
}

impl SimilarityStoreWriter {
    pub fn append(&mut self, group: &FrameGroup) -> Result<(), SimilarityStoreError> {
        validate_group(group)?;
        self.pending.push(group.clone());
        if self.pending.len() >= WRITE_BATCH_GROUPS {
            self.flush_batch()?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<u64, SimilarityStoreError> {
        self.flush_batch()?;
        let count = self.next_ordinal;
        self.connection.execute(
            "UPDATE similarity_meta SET state = ?1, group_count = ?2 WHERE id = ?3",
            params![
                STATE_COMPLETE,
                to_sql_u64(count, "group count")?,
                META_ROW_ID
            ],
        )?;
        self.connection.execute_batch("PRAGMA optimize;")?;
        drop(self.connection);

        fs::rename(&self.building_path, &self.final_path)?;
        sync_parent(&self.final_path)?;
        Ok(count)
    }

    fn flush_batch(&mut self) -> Result<(), SimilarityStoreError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut statement = transaction.prepare_cached(
                "INSERT INTO similarity_groups (
                    ordinal, representative_frame, first_frame, last_frame, frame_count,
                    start_ticks, start_tb_num, start_tb_den,
                    end_ticks, end_tb_num, end_tb_den,
                    start_duration_ticks, start_duration_tb_num, start_duration_tb_den,
                    end_duration_ticks, end_duration_tb_num, end_duration_tb_den,
                    representative_similarity_floor
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                    ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18
                 )",
            )?;
            for group in &self.pending {
                let ordinal = to_sql_u64(self.next_ordinal, "group ordinal")?;
                let start_duration = group.start_duration;
                let end_duration = group.end_duration;
                statement.execute(params![
                    ordinal,
                    to_sql_u64(group.representative_frame.0, "representative frame")?,
                    to_sql_u64(group.first_frame.0, "first frame")?,
                    to_sql_u64(group.last_frame.0, "last frame")?,
                    to_sql_u64(group.frame_count, "represented frame count")?,
                    group.start_timestamp.ticks,
                    group.start_timestamp.time_base.numerator,
                    group.start_timestamp.time_base.denominator,
                    group.end_timestamp.ticks,
                    group.end_timestamp.time_base.numerator,
                    group.end_timestamp.time_base.denominator,
                    start_duration.map(|value| value.ticks),
                    start_duration.map(|value| value.time_base.numerator),
                    start_duration.map(|value| value.time_base.denominator),
                    end_duration.map(|value| value.ticks),
                    end_duration.map(|value| value.time_base.numerator),
                    end_duration.map(|value| value.time_base.denominator),
                    i64::from(group.representative_similarity_floor),
                ])?;
                self.next_ordinal = self
                    .next_ordinal
                    .checked_add(1)
                    .ok_or(SimilarityStoreError::NumericRange("group ordinal"))?;
            }
        }
        transaction.execute(
            "UPDATE similarity_meta SET group_count = ?1 WHERE id = ?2",
            params![to_sql_u64(self.next_ordinal, "group count")?, META_ROW_ID],
        )?;
        transaction.commit()?;
        self.pending.clear();
        Ok(())
    }
}

fn configure_connection(connection: &Connection) -> Result<(), SimilarityStoreError> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "journal_mode", "DELETE")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    Ok(())
}

fn create_schema(connection: &Connection) -> Result<(), SimilarityStoreError> {
    connection.execute_batch(
        "CREATE TABLE similarity_meta (
            id INTEGER PRIMARY KEY CHECK(id = 1),
            state INTEGER NOT NULL CHECK(state IN (1, 2)),
            key_json TEXT NOT NULL,
            group_count INTEGER NOT NULL CHECK(group_count >= 0)
         );
         CREATE TABLE similarity_groups (
            ordinal INTEGER PRIMARY KEY CHECK(ordinal >= 0),
            representative_frame INTEGER NOT NULL CHECK(representative_frame >= 0),
            first_frame INTEGER NOT NULL CHECK(first_frame >= 0),
            last_frame INTEGER NOT NULL CHECK(last_frame >= 0),
            frame_count INTEGER NOT NULL CHECK(frame_count > 0),
            start_ticks INTEGER NOT NULL,
            start_tb_num INTEGER NOT NULL CHECK(start_tb_num > 0),
            start_tb_den INTEGER NOT NULL CHECK(start_tb_den > 0),
            end_ticks INTEGER NOT NULL,
            end_tb_num INTEGER NOT NULL CHECK(end_tb_num > 0),
            end_tb_den INTEGER NOT NULL CHECK(end_tb_den > 0),
            start_duration_ticks INTEGER,
            start_duration_tb_num INTEGER,
            start_duration_tb_den INTEGER,
            end_duration_ticks INTEGER,
            end_duration_tb_num INTEGER,
            end_duration_tb_den INTEGER,
            representative_similarity_floor INTEGER NOT NULL
                CHECK(representative_similarity_floor BETWEEN 0 AND 10000)
         );",
    )?;
    connection.pragma_update(None, "user_version", SIMILARITY_STORE_SCHEMA_VERSION)?;
    Ok(())
}

fn quick_check_ok(connection: &Connection) -> Result<bool, SimilarityStoreError> {
    let result: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    Ok(result == "ok")
}

fn validate_rows(
    connection: &Connection,
    expected_count: u64,
    expected_frame_count: Option<u64>,
) -> Result<bool, SimilarityStoreError> {
    let (count, min_ordinal, max_ordinal): (i64, Option<i64>, Option<i64>) = connection.query_row(
        "SELECT COUNT(*), MIN(ordinal), MAX(ordinal) FROM similarity_groups",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if from_sql_u64(count, "database group count")? != expected_count {
        return Ok(false);
    }
    if expected_count > 0
        && (min_ordinal != Some(0)
            || max_ordinal != Some(to_sql_u64(expected_count - 1, "maximum group ordinal")?))
    {
        return Ok(false);
    }

    let mut statement = connection.prepare_cached(
        "SELECT representative_frame, first_frame, last_frame, frame_count,
                start_ticks, start_tb_num, start_tb_den,
                end_ticks, end_tb_num, end_tb_den,
                start_duration_ticks, start_duration_tb_num, start_duration_tb_den,
                end_duration_ticks, end_duration_tb_num, end_duration_tb_den,
                representative_similarity_floor
         FROM similarity_groups ORDER BY ordinal",
    )?;
    let rows = statement.query_map([], decode_group)?;
    let mut previous_last: Option<u64> = None;
    let mut observed_groups = 0_u64;
    for row in rows {
        let group = row?;
        if invalid_group(&group) {
            return Ok(false);
        }
        if expected_frame_count.is_some() {
            if observed_groups == 0 && group.first_frame != FrameId(0) {
                return Ok(false);
            }
            if let Some(previous_last) = previous_last {
                if previous_last.checked_add(1) != Some(group.first_frame.0) {
                    return Ok(false);
                }
            }
        }
        previous_last = Some(group.last_frame.0);
        observed_groups = observed_groups.saturating_add(1);
    }

    if let Some(expected_frame_count) = expected_frame_count {
        if expected_frame_count == 0 {
            return Ok(expected_count == 0 && observed_groups == 0);
        }
        if expected_count == 0
            || observed_groups != expected_count
            || previous_last != Some(expected_frame_count - 1)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn decode_group(row: &rusqlite::Row<'_>) -> rusqlite::Result<FrameGroup> {
    let start_time_base = decode_time_base(row.get(5)?, row.get(6)?, 5)?;
    let end_time_base = decode_time_base(row.get(8)?, row.get(9)?, 8)?;
    let start_duration = decode_duration(row.get(10)?, row.get(11)?, row.get(12)?, 10)?;
    let end_duration = decode_duration(row.get(13)?, row.get(14)?, row.get(15)?, 13)?;
    let floor: i64 = row.get(16)?;
    let representative_similarity_floor =
        u16::try_from(floor).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(16, floor))?;
    Ok(FrameGroup {
        representative_frame: FrameId(sql_i64_to_u64(row.get(0)?, 0)?),
        first_frame: FrameId(sql_i64_to_u64(row.get(1)?, 1)?),
        last_frame: FrameId(sql_i64_to_u64(row.get(2)?, 2)?),
        frame_count: sql_i64_to_u64(row.get(3)?, 3)?,
        start_timestamp: MediaTimestamp {
            ticks: row.get(4)?,
            time_base: start_time_base,
        },
        end_timestamp: MediaTimestamp {
            ticks: row.get(7)?,
            time_base: end_time_base,
        },
        start_duration,
        end_duration,
        representative_similarity_floor,
    })
}

fn decode_time_base(numerator: i32, denominator: i32, column: usize) -> rusqlite::Result<TimeBase> {
    TimeBase::new(numerator, denominator).ok_or(rusqlite::Error::IntegralValueOutOfRange(
        column,
        i64::from(numerator),
    ))
}

fn decode_duration(
    ticks: Option<i64>,
    numerator: Option<i32>,
    denominator: Option<i32>,
    column: usize,
) -> rusqlite::Result<Option<MediaDuration>> {
    match (ticks, numerator, denominator) {
        (None, None, None) => Ok(None),
        (Some(ticks), Some(numerator), Some(denominator)) if ticks > 0 => Ok(Some(MediaDuration {
            ticks,
            time_base: decode_time_base(numerator, denominator, column)?,
        })),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn validate_group(group: &FrameGroup) -> Result<(), SimilarityStoreError> {
    if invalid_group(group) {
        return Err(SimilarityStoreError::InvalidGroup(
            "group ranges must be ordered, non-empty, contiguous, and use valid timing/score data"
                .into(),
        ));
    }
    to_sql_u64(group.representative_frame.0, "representative frame")?;
    to_sql_u64(group.first_frame.0, "first frame")?;
    to_sql_u64(group.last_frame.0, "last frame")?;
    to_sql_u64(group.frame_count, "represented frame count")?;
    Ok(())
}

fn invalid_group(group: &FrameGroup) -> bool {
    let expected = group
        .last_frame
        .0
        .checked_sub(group.first_frame.0)
        .and_then(|delta| delta.checked_add(1));
    let invalid_duration = |duration: Option<MediaDuration>| {
        duration.is_some_and(|value| {
            value.ticks <= 0 || value.time_base.numerator <= 0 || value.time_base.denominator <= 0
        })
    };
    group.first_frame.0 > group.last_frame.0
        || group.frame_count == 0
        || expected != Some(group.frame_count)
        || group.representative_frame.0 < group.first_frame.0
        || group.representative_frame.0 > group.last_frame.0
        || group.start_timestamp.time_base.numerator <= 0
        || group.start_timestamp.time_base.denominator <= 0
        || group.end_timestamp.time_base.numerator <= 0
        || group.end_timestamp.time_base.denominator <= 0
        || group.representative_similarity_floor > SIMILARITY_SCALE
        || invalid_duration(group.start_duration)
        || invalid_duration(group.end_duration)
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
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".building");
    path.with_file_name(name)
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

fn to_sql_u64(value: u64, label: &'static str) -> Result<i64, SimilarityStoreError> {
    i64::try_from(value).map_err(|_| SimilarityStoreError::NumericRange(label))
}

fn from_sql_u64(value: i64, label: &'static str) -> Result<u64, SimilarityStoreError> {
    u64::try_from(value).map_err(|_| SimilarityStoreError::NumericRange(label))
}

fn sql_i64_to_u64(value: i64, column: usize) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_core::{CodecInfo, MediaKind, StreamInfo};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn root(tag: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("framescope-similarity-store-{tag}-{nonce}"))
    }

    fn source(tag: &str) -> SourceIdentity {
        SourceIdentity::new(100, Some(10), Some(tag.into()))
    }

    fn stream() -> FrameIndexStreamIdentity {
        FrameIndexStreamIdentity::from_stream(&StreamInfo {
            index: 2,
            media_kind: MediaKind::Video,
            codec: CodecInfo {
                id: 27,
                name: "h264".into(),
                decoder_available: true,
            },
            is_default: true,
            time_base: Some(TimeBase::new(1, 1000).unwrap()),
            duration: None,
            frame_count: None,
            width: Some(1920),
            height: Some(1080),
            pixel_format: Some("yuv420p".into()),
            average_frame_rate: None,
            nominal_frame_rate: None,
            rotation_degrees: None,
        })
        .unwrap()
    }

    fn group(first: u64, last: u64, start_ticks: i64) -> FrameGroup {
        let time_base = TimeBase::new(1, 1000).unwrap();
        FrameGroup {
            representative_frame: FrameId(first),
            first_frame: FrameId(first),
            last_frame: FrameId(last),
            frame_count: last - first + 1,
            start_timestamp: MediaTimestamp {
                ticks: start_ticks,
                time_base,
            },
            end_timestamp: MediaTimestamp {
                ticks: start_ticks + 40,
                time_base,
            },
            start_duration: Some(MediaDuration {
                ticks: 40,
                time_base,
            }),
            end_duration: Some(MediaDuration {
                ticks: 85,
                time_base,
            }),
            representative_similarity_floor: 9_800,
        }
    }

    fn hybrid(max_hash_distance: u8, minimum_luma_similarity: u16) -> HybridSimilarityPolicy {
        HybridSimilarityPolicy {
            max_hash_distance,
            minimum_luma_similarity,
        }
    }

    #[test]
    fn streaming_round_trip_preserves_exact_timing() {
        let root = root("roundtrip");
        let store = SimilarityStore::new(&root);
        let key = SimilarityStoreKey::new(source("a"), stream(), SimilarityMode::Exact).unwrap();
        let expected = group(10, 11, -25);
        let mut writer = store.begin(&key).unwrap();
        writer.append(&expected).unwrap();
        assert_eq!(writer.finish().unwrap(), 1);

        let mut loaded = None;
        assert_eq!(
            store
                .visit_groups(&key, |value| loaded = Some(value))
                .unwrap(),
            SimilarityStoreLoad::Reused { group_count: 1 }
        );
        assert_eq!(loaded, Some(expected));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn interrupted_build_never_replaces_last_complete_result() {
        let root = root("interrupt");
        let store = SimilarityStore::new(&root);
        let key = SimilarityStoreKey::new(source("a"), stream(), SimilarityMode::Exact).unwrap();
        let original = group(0, 0, 0);
        let mut writer = store.begin(&key).unwrap();
        writer.append(&original).unwrap();
        writer.finish().unwrap();

        let mut interrupted = store.begin(&key).unwrap();
        interrupted.append(&group(1, 1, 40)).unwrap();
        drop(interrupted);

        let mut loaded = None;
        assert_eq!(
            store
                .visit_groups(&key, |value| loaded = Some(value))
                .unwrap(),
            SimilarityStoreLoad::Reused { group_count: 1 }
        );
        assert_eq!(loaded, Some(original));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn large_group_set_is_written_and_visited_incrementally() {
        let root = root("large");
        let store = SimilarityStore::new(&root);
        let key = SimilarityStoreKey::new_hybrid(source("a"), stream(), hybrid(8, 9_700)).unwrap();
        let mut writer = store.begin(&key).unwrap();
        for id in 0..20_000_u64 {
            writer
                .append(&group(id, id, i64::try_from(id).unwrap()))
                .unwrap();
        }
        assert_eq!(writer.finish().unwrap(), 20_000);

        let mut count = 0_u64;
        assert_eq!(
            store
                .visit_groups(&key, |value| {
                    assert_eq!(value.first_frame, FrameId(count));
                    count += 1;
                })
                .unwrap(),
            SimilarityStoreLoad::Reused {
                group_count: 20_000
            }
        );
        assert_eq!(count, 20_000);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn config_changes_namespace() {
        let store = SimilarityStore::new(root("config"));
        let exact = SimilarityStoreKey::new(source("a"), stream(), SimilarityMode::Exact).unwrap();
        let hybrid_a =
            SimilarityStoreKey::new_hybrid(source("a"), stream(), hybrid(8, 9_700)).unwrap();
        let hybrid_b =
            SimilarityStoreKey::new_hybrid(source("a"), stream(), hybrid(9, 9_700)).unwrap();
        assert_ne!(store.path_for(&exact), store.path_for(&hybrid_a));
        assert_ne!(store.path_for(&hybrid_a), store.path_for(&hybrid_b));
    }

    #[test]
    fn corrupt_file_is_disposable() {
        let root = root("corrupt");
        let store = SimilarityStore::new(&root);
        let key = SimilarityStoreKey::new(source("a"), stream(), SimilarityMode::Exact).unwrap();
        let path = store.path_for(&key);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not-sqlite").unwrap();
        assert_eq!(
            store.visit_groups(&key, |_| {}).unwrap(),
            SimilarityStoreLoad::InvalidatedCorrupt
        );
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn source_invalidation_is_scoped_and_removes_old_schema_namespaces() {
        let root = root("invalidate");
        let store = SimilarityStore::new(&root);
        let a = source("a");
        let b = source("b");
        let ka = SimilarityStoreKey::new(a.clone(), stream(), SimilarityMode::Exact).unwrap();
        let kb = SimilarityStoreKey::new(b.clone(), stream(), SimilarityMode::Exact).unwrap();
        for key in [&ka, &kb] {
            let mut writer = store.begin(key).unwrap();
            writer.append(&group(0, 0, 0)).unwrap();
            writer.finish().unwrap();
        }
        let legacy = root.join("v2").join(a.stable_key());
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("legacy.json"), b"old").unwrap();

        store.invalidate_source(&a).unwrap();
        assert_eq!(
            store.visit_groups(&ka, |_| {}).unwrap(),
            SimilarityStoreLoad::Missing
        );
        assert!(matches!(
            store.visit_groups(&kb, |_| {}).unwrap(),
            SimilarityStoreLoad::Reused { group_count: 1 }
        ));
        assert!(!legacy.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unsafe_source_identity_is_rejected() {
        let source = SourceIdentity::metadata_only(Some(100), Some(10), Some("doc".into()));
        assert!(matches!(
            SimilarityStoreKey::new(source, stream(), SimilarityMode::Exact),
            Err(SimilarityStoreError::UnsafeSourceIdentity)
        ));
    }

    #[test]
    fn invalid_group_is_rejected_before_persistence() {
        let root = root("invalid");
        let store = SimilarityStore::new(&root);
        let key = SimilarityStoreKey::new(source("a"), stream(), SimilarityMode::Exact).unwrap();
        let mut invalid = group(2, 2, 0);
        invalid.frame_count = 99;
        let mut writer = store.begin(&key).unwrap();
        assert!(matches!(
            writer.append(&invalid),
            Err(SimilarityStoreError::InvalidGroup(_))
        ));
        let _ = fs::remove_dir_all(root);
    }

    fn assert_strict_coverage_invalid(tag: &str, groups: &[FrameGroup], frame_count: u64) {
        let root = root(tag);
        let store = SimilarityStore::new(&root);
        let key =
            SimilarityStoreKey::new_hybrid(source("coverage"), stream(), hybrid(8, 9_700)).unwrap();
        let mut writer = store.begin(&key).unwrap();
        for group in groups {
            writer.append(group).unwrap();
        }
        writer.finish().unwrap();
        assert_eq!(
            store
                .visit_groups_for_frame_count(&key, frame_count, |_| {})
                .unwrap(),
            SimilarityStoreLoad::InvalidatedCorrupt
        );
        assert!(!store.path_for(&key).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn strict_coverage_accepts_complete_contiguous_groups() {
        let root = root("strict-complete");
        let store = SimilarityStore::new(&root);
        let key = SimilarityStoreKey::new_hybrid(source("a"), stream(), hybrid(8, 9_700)).unwrap();
        let mut writer = store.begin(&key).unwrap();
        writer.append(&group(0, 1, 0)).unwrap();
        writer.append(&group(2, 2, 80)).unwrap();
        writer.finish().unwrap();
        assert_eq!(
            store.visit_groups_for_frame_count(&key, 3, |_| {}).unwrap(),
            SimilarityStoreLoad::Reused { group_count: 2 }
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn strict_coverage_invalidates_gap() {
        assert_strict_coverage_invalid("strict-gap", &[group(0, 0, 0), group(2, 2, 80)], 3);
    }

    #[test]
    fn strict_coverage_invalidates_overlap() {
        assert_strict_coverage_invalid("strict-overlap", &[group(0, 1, 0), group(1, 2, 40)], 3);
    }

    #[test]
    fn strict_coverage_invalidates_first_group_not_at_zero() {
        assert_strict_coverage_invalid("strict-first", &[group(1, 2, 40)], 3);
    }

    #[test]
    fn strict_coverage_invalidates_final_group_ending_early() {
        assert_strict_coverage_invalid("strict-final", &[group(0, 1, 0)], 3);
    }

    #[test]
    fn hybrid_store_namespace_fences_metric_algorithm_generation() {
        let store = SimilarityStore::new(root("metric-version"));
        let key = SimilarityStoreKey::new_hybrid(source("a"), stream(), hybrid(8, 9_700)).unwrap();
        let path = store.path_for(&key).to_string_lossy().into_owned();
        assert!(path.contains("/v4/"));
        assert!(path.contains(&format!(
            "hybrid-a{HYBRID_SIMILARITY_ALGORITHM_VERSION}-h8-l9700"
        )));
    }
}
