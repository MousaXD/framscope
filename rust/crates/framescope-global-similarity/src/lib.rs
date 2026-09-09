//! Versioned, source-derived descriptors for non-contiguous similar-frame retrieval.
//!
//! This crate is deliberately separate from the adjacent-frame grouper. The authoritative frame
//! index remains the source of frame identity and presentation timing; this store contains only
//! disposable descriptors keyed to that contract.

use framescope_cache::{FrameId, OwnedRgbaFrame, SourceIdentity, FRAME_TIMELINE_CONTRACT_GENERATION};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const GLOBAL_SIMILARITY_ALGORITHM_VERSION: u32 = 1;
pub const GLOBAL_SIMILARITY_STORE_SCHEMA_VERSION: u32 = 1;
pub const NORMALIZED_SIDE: usize = 12;
pub const NORMALIZED_RGB_BYTES: usize = NORMALIZED_SIDE * NORMALIZED_SIDE * 3;
pub const HASH_BANDS_PER_KIND: usize = 4;
pub const DEFAULT_MINIMUM_SIMILARITY: u16 = 9_300;
pub const DEFAULT_MAX_RESULTS: usize = 64;
const SIMILARITY_SCALE: u32 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalSimilarityStoreKey {
    pub source: SourceIdentity,
    pub stream_index: u32,
    pub frame_timeline_contract_generation: u32,
    pub descriptor_algorithm_version: u32,
}

impl GlobalSimilarityStoreKey {
    pub fn new(source: SourceIdentity, stream_index: u32) -> Result<Self, GlobalSimilarityError> {
        if !source.is_reuse_safe() {
            return Err(GlobalSimilarityError::UnsafeSourceIdentity);
        }
        Ok(Self {
            source,
            stream_index,
            frame_timeline_contract_generation: FRAME_TIMELINE_CONTRACT_GENERATION,
            descriptor_algorithm_version: GLOBAL_SIMILARITY_ALGORITHM_VERSION,
        })
    }
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
        if self.minimum_similarity > SIMILARITY_SCALE as u16 {
            return Err(GlobalSimilarityError::InvalidThreshold(self.minimum_similarity));
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

#[derive(Debug, Error)]
pub enum GlobalSimilarityError {
    #[error("source identity is not safe for reusable global-similarity data")]
    UnsafeSourceIdentity,
    #[error("global similarity threshold {0} is outside 0..=10000")]
    InvalidThreshold(u16),
    #[error("global similarity max_results must be greater than zero")]
    InvalidMaxResults,
    #[error("normalized descriptor length is invalid: {0}")]
    InvalidDescriptorLength(usize),
    #[error("target frame {0:?} is not present in the similarity store")]
    MissingTarget(FrameId),
    #[error("stored similarity metadata does not match this source/index generation")]
    StoreKeyMismatch,
    #[error("stored similarity database is incomplete or corrupt: {0}")]
    InvalidStore(String),
    #[error("frame layout overflow")]
    FrameLayoutOverflow,
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("metadata serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn descriptor_for_frame(
    frame_id: FrameId,
    frame: &OwnedRgbaFrame,
) -> Result<GlobalDescriptor, GlobalSimilarityError> {
    let normalized_rgb = resample_rgb(frame, NORMALIZED_SIDE, NORMALIZED_SIDE)?;
    let hash_rgb = resample_rgb(frame, 8, 8)?;
    let mut lumas = [0u16; 64];
    let mut sum = 0u32;
    for (index, px) in hash_rgb.chunks_exact(3).enumerate() {
        let luma = integer_luma(px[0], px[1], px[2]);
        lumas[index] = luma;
        sum += u32::from(luma);
    }
    let average = ((sum + 32) / 64) as u16;
    let mut average_hash = 0u64;
    for (index, value) in lumas.iter().enumerate() {
        if *value >= average {
            average_hash |= 1u64 << index;
        }
    }

    let dhash_rgb = resample_rgb(frame, 9, 8)?;
    let mut difference_hash = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            let left = (y * 9 + x) * 3;
            let right = left + 3;
            let left_luma = integer_luma(dhash_rgb[left], dhash_rgb[left + 1], dhash_rgb[left + 2]);
            let right_luma = integer_luma(dhash_rgb[right], dhash_rgb[right + 1], dhash_rgb[right + 2]);
            if left_luma > right_luma {
                difference_hash |= 1u64 << (y * 8 + x);
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

/// Deterministic color-aware confirmation score. A one-cell normalized translation search absorbs
/// tiny crop/scale/decoder resampling differences without making color information disappear.
pub fn confirmation_similarity(
    left: &[u8],
    right: &[u8],
) -> Result<u16, GlobalSimilarityError> {
    if left.len() != NORMALIZED_RGB_BYTES {
        return Err(GlobalSimilarityError::InvalidDescriptorLength(left.len()));
    }
    if right.len() != NORMALIZED_RGB_BYTES {
        return Err(GlobalSimilarityError::InvalidDescriptorLength(right.len()));
    }
    let mut best = 0u16;
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            let mut difference = 0u64;
            for y in 0..NORMALIZED_SIDE {
                for x in 0..NORMALIZED_SIDE {
                    let rx = (x as i32 + dx).clamp(0, NORMALIZED_SIDE as i32 - 1) as usize;
                    let ry = (y as i32 + dy).clamp(0, NORMALIZED_SIDE as i32 - 1) as usize;
                    let li = (y * NORMALIZED_SIDE + x) * 3;
                    let ri = (ry * NORMALIZED_SIDE + rx) * 3;
                    difference += u64::from(left[li].abs_diff(right[ri]));
                    difference += u64::from(left[li + 1].abs_diff(right[ri + 1]));
                    difference += u64::from(left[li + 2].abs_diff(right[ri + 2]));
                }
            }
            let maximum = (NORMALIZED_RGB_BYTES as u64) * 255;
            let difference_bps = ((difference * u64::from(SIMILARITY_SCALE) + maximum / 2) / maximum)
                .min(u64::from(SIMILARITY_SCALE));
            best = best.max((u64::from(SIMILARITY_SCALE) - difference_bps) as u16);
        }
    }
    Ok(best)
}

#[derive(Debug)]
pub struct GlobalSimilarityStore {
    path: PathBuf,
    key: GlobalSimilarityStoreKey,
    descriptor_count: u64,
}

impl GlobalSimilarityStore {
    pub fn create(
        path: impl AsRef<Path>,
        key: GlobalSimilarityStoreKey,
        descriptors: &[GlobalDescriptor],
    ) -> Result<Self, GlobalSimilarityError> {
        if !key.source.is_reuse_safe() {
            return Err(GlobalSimilarityError::UnsafeSourceIdentity);
        }
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let building = path.with_extension("building");
        let _ = fs::remove_file(&building);
        let mut conn = Connection::open(&building)?;
        conn.execute_batch(
            "PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;
             CREATE TABLE metadata (id INTEGER PRIMARY KEY CHECK(id=1), schema_version INTEGER NOT NULL, state TEXT NOT NULL, key_json TEXT NOT NULL, descriptor_count INTEGER NOT NULL);
             CREATE TABLE descriptors (frame_id INTEGER PRIMARY KEY, average_hash INTEGER NOT NULL, difference_hash INTEGER NOT NULL, normalized_rgb BLOB NOT NULL);
             CREATE TABLE hash_bands (kind INTEGER NOT NULL, band INTEGER NOT NULL, band_value INTEGER NOT NULL, frame_id INTEGER NOT NULL, PRIMARY KEY(kind, band, band_value, frame_id));
             CREATE INDEX hash_band_lookup ON hash_bands(kind, band, band_value, frame_id);"
        )?;
        let key_json = serde_json::to_string(&key)?;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO metadata(id, schema_version, state, key_json, descriptor_count) VALUES(1, ?1, 'building', ?2, ?3)",
            params![GLOBAL_SIMILARITY_STORE_SCHEMA_VERSION, key_json, descriptors.len() as i64],
        )?;
        {
            let mut descriptor_stmt = tx.prepare("INSERT INTO descriptors(frame_id, average_hash, difference_hash, normalized_rgb) VALUES(?1, ?2, ?3, ?4)")?;
            let mut band_stmt = tx.prepare("INSERT INTO hash_bands(kind, band, band_value, frame_id) VALUES(?1, ?2, ?3, ?4)")?;
            for descriptor in descriptors {
                if descriptor.normalized_rgb.len() != NORMALIZED_RGB_BYTES {
                    return Err(GlobalSimilarityError::InvalidDescriptorLength(descriptor.normalized_rgb.len()));
                }
                descriptor_stmt.execute(params![descriptor.frame_id.0 as i64, descriptor.average_hash as i64, descriptor.difference_hash as i64, descriptor.normalized_rgb])?;
                for (kind, hash) in [(0i64, descriptor.average_hash), (1i64, descriptor.difference_hash)] {
                    for band in 0..HASH_BANDS_PER_KIND {
                        let value = ((hash >> (band * 16)) & 0xffff) as i64;
                        band_stmt.execute(params![kind, band as i64, value, descriptor.frame_id.0 as i64])?;
                    }
                }
            }
        }
        tx.execute("UPDATE metadata SET state='complete' WHERE id=1", [])?;
        tx.commit()?;
        conn.execute_batch("PRAGMA wal_checkpoint(FULL);")?;
        drop(conn);
        if path.exists() {
            fs::remove_file(path)?;
        }
        fs::rename(&building, path)?;
        Self::open(path, key)
    }

    pub fn open(
        path: impl AsRef<Path>,
        key: GlobalSimilarityStoreKey,
    ) -> Result<Self, GlobalSimilarityError> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path)?;
        let quick: String = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if quick != "ok" {
            return Err(GlobalSimilarityError::InvalidStore(quick));
        }
        let (schema, state, stored_key, descriptor_count): (u32, String, String, i64) = conn.query_row(
            "SELECT schema_version, state, key_json, descriptor_count FROM metadata WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        if schema != GLOBAL_SIMILARITY_STORE_SCHEMA_VERSION || state != "complete" {
            return Err(GlobalSimilarityError::InvalidStore(format!("schema={schema} state={state}")));
        }
        let decoded: GlobalSimilarityStoreKey = serde_json::from_str(&stored_key)?;
        if decoded != key {
            return Err(GlobalSimilarityError::StoreKeyMismatch);
        }
        let actual_count: i64 = conn.query_row("SELECT COUNT(*) FROM descriptors", [], |row| row.get(0))?;
        let band_count: i64 = conn.query_row("SELECT COUNT(*) FROM hash_bands", [], |row| row.get(0))?;
        if descriptor_count < 0 || actual_count != descriptor_count || band_count != descriptor_count * 8 {
            return Err(GlobalSimilarityError::InvalidStore(format!("descriptor_count={descriptor_count} actual={actual_count} bands={band_count}")));
        }
        Ok(Self { path, key, descriptor_count: descriptor_count as u64 })
    }

    pub fn descriptor_count(&self) -> u64 {
        self.descriptor_count
    }

    pub fn key(&self) -> &GlobalSimilarityStoreKey {
        &self.key
    }

    pub fn query(
        &self,
        target_frame: FrameId,
        policy: GlobalSimilarityPolicy,
    ) -> Result<GlobalSimilarityQuery, GlobalSimilarityError> {
        let policy = policy.validate()?;
        let conn = Connection::open(&self.path)?;
        let target = read_descriptor(&conn, target_frame)?.ok_or(GlobalSimilarityError::MissingTarget(target_frame))?;
        let mut candidate_ids = BTreeSet::new();
        let mut stmt = conn.prepare("SELECT frame_id FROM hash_bands WHERE kind=?1 AND band=?2 AND band_value=?3")?;
        for (kind, hash) in [(0i64, target.average_hash), (1i64, target.difference_hash)] {
            for band in 0..HASH_BANDS_PER_KIND {
                let value = ((hash >> (band * 16)) & 0xffff) as i64;
                let rows = stmt.query_map(params![kind, band as i64, value], |row| row.get::<_, i64>(0))?;
                for row in rows {
                    let id = row? as u64;
                    if id != target_frame.0 {
                        candidate_ids.insert(FrameId(id));
                    }
                }
            }
        }
        let candidate_count = candidate_ids.len() as u64;
        let mut matches = Vec::new();
        for frame_id in candidate_ids {
            let Some(candidate) = read_descriptor(&conn, frame_id)? else { continue; };
            let similarity = confirmation_similarity(&target.normalized_rgb, &candidate.normalized_rgb)?;
            if similarity >= policy.minimum_similarity {
                matches.push(GlobalSimilarityMatch { frame_id, similarity });
            }
        }
        matches.sort_by(|left, right| right.similarity.cmp(&left.similarity).then_with(|| left.frame_id.0.cmp(&right.frame_id.0)));
        let matched_count = matches.len() as u64;
        let truncated = matches.len() > policy.max_results;
        matches.truncate(policy.max_results);
        Ok(GlobalSimilarityQuery { target_frame, descriptor_count: self.descriptor_count, candidate_count, matched_count, truncated, matches })
    }
}

fn read_descriptor(conn: &Connection, frame_id: FrameId) -> Result<Option<GlobalDescriptor>, GlobalSimilarityError> {
    let mut stmt = conn.prepare("SELECT average_hash, difference_hash, normalized_rgb FROM descriptors WHERE frame_id=?1")?;
    let mut rows = stmt.query(params![frame_id.0 as i64])?;
    let Some(row) = rows.next()? else { return Ok(None); };
    let normalized_rgb: Vec<u8> = row.get(2)?;
    if normalized_rgb.len() != NORMALIZED_RGB_BYTES {
        return Err(GlobalSimilarityError::InvalidDescriptorLength(normalized_rgb.len()));
    }
    Ok(Some(GlobalDescriptor { frame_id, average_hash: row.get::<_, i64>(0)? as u64, difference_hash: row.get::<_, i64>(1)? as u64, normalized_rgb }))
}

fn integer_luma(r: u8, g: u8, b: u8) -> u16 {
    ((u32::from(r) * 77 + u32::from(g) * 150 + u32::from(b) * 29 + 128) >> 8) as u16
}

fn resample_rgb(frame: &OwnedRgbaFrame, out_width: usize, out_height: usize) -> Result<Vec<u8>, GlobalSimilarityError> {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let required = frame.stride_bytes.checked_mul(height).ok_or(GlobalSimilarityError::FrameLayoutOverflow)?;
    if frame.pixels().len() < required {
        return Err(GlobalSimilarityError::FrameLayoutOverflow);
    }
    let mut output = vec![0u8; out_width * out_height * 3];
    for oy in 0..out_height {
        let sy_fp = if out_height == 1 || height == 1 { 0 } else { (oy as u64 * (height - 1) as u64 * 65536) / (out_height - 1) as u64 };
        let y0 = (sy_fp >> 16) as usize;
        let y1 = (y0 + 1).min(height - 1);
        let fy = (sy_fp & 0xffff) as u32;
        for ox in 0..out_width {
            let sx_fp = if out_width == 1 || width == 1 { 0 } else { (ox as u64 * (width - 1) as u64 * 65536) / (out_width - 1) as u64 };
            let x0 = (sx_fp >> 16) as usize;
            let x1 = (x0 + 1).min(width - 1);
            let fx = (sx_fp & 0xffff) as u32;
            for channel in 0..3 {
                let p00 = u64::from(frame.pixels()[y0 * frame.stride_bytes + x0 * 4 + channel]);
                let p10 = u64::from(frame.pixels()[y0 * frame.stride_bytes + x1 * 4 + channel]);
                let p01 = u64::from(frame.pixels()[y1 * frame.stride_bytes + x0 * 4 + channel]);
                let p11 = u64::from(frame.pixels()[y1 * frame.stride_bytes + x1 * 4 + channel]);
                let top = p00 * u64::from(65536 - fx) + p10 * u64::from(fx);
                let bottom = p01 * u64::from(65536 - fx) + p11 * u64::from(fx);
                let value = (top * u64::from(65536 - fy) + bottom * u64::from(fy) + (1u64 << 31)) >> 32;
                output[(oy * out_width + ox) * 3 + channel] = value.min(255) as u8;
            }
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn frame(width: u32, height: u32, mut pixel: impl FnMut(u32, u32) -> [u8; 3]) -> OwnedRgbaFrame {
        let mut bytes = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = pixel(x, y);
                bytes.extend_from_slice(&[r, g, b, 255]);
            }
        }
        OwnedRgbaFrame::new(width, height, width as usize * 4, bytes).unwrap()
    }

    fn base(width: u32, height: u32) -> OwnedRgbaFrame {
        frame(width, height, |x, y| {
            let checker = if ((x / 7) + (y / 5)) % 2 == 0 { 36 } else { 180 };
            [checker + ((x * 3 + y) % 40) as u8, 30 + ((x + y * 2) % 110) as u8, 210u8.saturating_sub(((x * 2 + y * 3) % 120) as u8)]
        })
    }

    fn resized(source: &OwnedRgbaFrame, width: u32, height: u32) -> OwnedRgbaFrame {
        frame(width, height, |x, y| {
            let sx = if width <= 1 { 0 } else { x * (source.width - 1) / (width - 1) };
            let sy = if height <= 1 { 0 } else { y * (source.height - 1) / (height - 1) };
            let i = sy as usize * source.stride_bytes + sx as usize * 4;
            [source.pixels()[i], source.pixels()[i + 1], source.pixels()[i + 2]]
        })
    }

    fn key(tag: &str) -> GlobalSimilarityStoreKey {
        GlobalSimilarityStoreKey::new(SourceIdentity::new(123, Some(9), Some(format!("blake3-full-v2:{tag}"))), 0).unwrap()
    }

    #[test]
    fn deterministic_fixture_matrix_separates_requested_near_duplicates() {
        let original = base(64, 48);
        let same = original.clone();
        let scale = resized(&original, 40, 30);
        let recompressed = frame(64, 48, |x, y| {
            let i = y as usize * original.stride_bytes + x as usize * 4;
            [(original.pixels()[i] / 8) * 8, (original.pixels()[i + 1] / 8) * 8, (original.pixels()[i + 2] / 8) * 8]
        });
        let crop = frame(58, 44, |x, y| {
            let sx = x + 3;
            let sy = y + 2;
            let i = sy as usize * original.stride_bytes + sx as usize * 4;
            [original.pixels()[i], original.pixels()[i + 1], original.pixels()[i + 2]]
        });
        let color_shift = frame(64, 48, |x, y| {
            let i = y as usize * original.stride_bytes + x as usize * 4;
            [original.pixels()[i].saturating_add(5), original.pixels()[i + 1].saturating_sub(3), original.pixels()[i + 2].saturating_add(4)]
        });
        let adjacent = frame(64, 48, |x, y| {
            let i = y as usize * original.stride_bytes + x as usize * 4;
            let delta = if (x + y) % 17 == 0 { 2 } else { 0 };
            [original.pixels()[i].saturating_add(delta), original.pixels()[i + 1], original.pixels()[i + 2]]
        });
        let unrelated = frame(64, 48, |x, y| [((x * 11 + y * 17) % 256) as u8, ((x * 19 + y * 5 + 90) % 256) as u8, ((x * 2 + y * 23) % 256) as u8]);

        let originals = descriptor_for_frame(FrameId(0), &original).unwrap();
        let positives = [same, scale, recompressed, crop, color_shift, adjacent];
        for (offset, candidate) in positives.iter().enumerate() {
            let descriptor = descriptor_for_frame(FrameId(offset as u64 + 1), candidate).unwrap();
            let score = confirmation_similarity(&originals.normalized_rgb, &descriptor.normalized_rgb).unwrap();
            assert!(score >= DEFAULT_MINIMUM_SIMILARITY, "positive fixture {offset} scored {score}");
            let shared_average_band = (0..4).any(|band| ((originals.average_hash >> (band * 16)) & 0xffff) == ((descriptor.average_hash >> (band * 16)) & 0xffff));
            let shared_difference_band = (0..4).any(|band| ((originals.difference_hash >> (band * 16)) & 0xffff) == ((descriptor.difference_hash >> (band * 16)) & 0xffff));
            assert!(shared_average_band || shared_difference_band, "positive fixture {offset} missed candidate generation");
        }
        let unrelated = descriptor_for_frame(FrameId(99), &unrelated).unwrap();
        let unrelated_score = confirmation_similarity(&originals.normalized_rgb, &unrelated.normalized_rgb).unwrap();
        assert!(unrelated_score < DEFAULT_MINIMUM_SIMILARITY, "unrelated fixture scored {unrelated_score}");
    }

    #[test]
    fn store_is_versioned_persistent_and_returns_non_contiguous_matches_in_score_order() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("global.sqlite3");
        let original = base(64, 48);
        let close = frame(64, 48, |x, y| {
            let i = y as usize * original.stride_bytes + x as usize * 4;
            [original.pixels()[i].saturating_add(2), original.pixels()[i + 1], original.pixels()[i + 2]]
        });
        let unrelated = frame(64, 48, |x, y| [((x * 13 + y * 29) % 256) as u8, ((x * 3 + y * 31) % 256) as u8, ((x * 7 + y * 11) % 256) as u8]);
        let descriptors = vec![
            descriptor_for_frame(FrameId(0), &original).unwrap(),
            descriptor_for_frame(FrameId(1), &unrelated).unwrap(),
            descriptor_for_frame(FrameId(2), &close).unwrap(),
            descriptor_for_frame(FrameId(7), &original).unwrap(),
        ];
        let store = GlobalSimilarityStore::create(&path, key("abc"), &descriptors).unwrap();
        drop(store);
        let reopened = GlobalSimilarityStore::open(&path, key("abc")).unwrap();
        let result = reopened.query(FrameId(0), GlobalSimilarityPolicy::default()).unwrap();
        assert_eq!(result.descriptor_count, 4);
        assert_eq!(result.matches.iter().map(|m| m.frame_id.0).collect::<Vec<_>>(), vec![7, 2]);
        assert_eq!(result.matches[0].similarity, 10_000);
        assert!(GlobalSimilarityStore::open(&path, key("different")).is_err());
    }

    #[test]
    fn incomplete_store_is_never_reused() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("bad.sqlite3");
        fs::write(&path, b"not sqlite").unwrap();
        assert!(GlobalSimilarityStore::open(&path, key("abc")).is_err());
    }
}
