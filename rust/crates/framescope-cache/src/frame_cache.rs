use crate::{FrameId, SourceIdentity};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

/// Stable key for a decoded frame cached in memory.
///
/// The key is derived from the authoritative Phase 3 source identity rather than a path, URI, or
/// display name, so two different videos with the same filename cannot collide.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FrameCacheKey {
    source_key: String,
    pub stream_index: u32,
    pub frame_id: FrameId,
}

impl FrameCacheKey {
    pub fn new(
        source: &SourceIdentity,
        stream_index: u32,
        frame_id: FrameId,
    ) -> Result<Self, FrameCacheError> {
        if !source.is_reuse_safe() {
            return Err(FrameCacheError::UnsafeSourceIdentity);
        }
        Ok(Self {
            source_key: source.stable_key(),
            stream_index,
            frame_id,
        })
    }

    pub fn source_key(&self) -> &str {
        &self.source_key
    }
}

/// Owned canonical full-resolution frame payload used by the RAM hot cache.
///
/// The byte storage is reference counted so cache hits can hand ownership to a caller without
/// copying the full frame. It never borrows FFmpeg `AVFrame` storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedRgbaFrame {
    pub width: u32,
    pub height: u32,
    pub stride_bytes: usize,
    pixels: Arc<[u8]>,
}

impl OwnedRgbaFrame {
    pub fn new(
        width: u32,
        height: u32,
        stride_bytes: usize,
        pixels: Vec<u8>,
    ) -> Result<Self, FrameCacheError> {
        if width == 0 || height == 0 {
            return Err(FrameCacheError::InvalidFrame(
                "frame dimensions must be positive".into(),
            ));
        }
        let minimum_stride = usize::try_from(width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or_else(|| {
                FrameCacheError::InvalidFrame("frame width overflows byte size".into())
            })?;
        if stride_bytes < minimum_stride {
            return Err(FrameCacheError::InvalidFrame(format!(
                "RGBA stride {stride_bytes} is smaller than minimum {minimum_stride}"
            )));
        }
        let expected = stride_bytes
            .checked_mul(usize::try_from(height).map_err(|_| {
                FrameCacheError::InvalidFrame("frame height does not fit memory size".into())
            })?)
            .ok_or_else(|| FrameCacheError::InvalidFrame("frame byte size overflows".into()))?;
        if pixels.len() != expected {
            return Err(FrameCacheError::InvalidFrame(format!(
                "RGBA payload has {} bytes, expected {expected}",
                pixels.len()
            )));
        }
        Ok(Self {
            width,
            height,
            stride_bytes,
            pixels: Arc::from(pixels),
        })
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn byte_len(&self) -> usize {
        self.pixels.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedFrame {
    pub key: FrameCacheKey,
    pub pixels: OwnedRgbaFrame,
}

impl CachedFrame {
    pub fn byte_len(&self) -> usize {
        self.pixels.byte_len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RamCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub insertions: u64,
    pub evictions: u64,
    pub resident_bytes: usize,
    pub resident_frames: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RamInsertResult {
    Inserted,
    TooLarge,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FrameCacheError {
    #[error(
        "source identity lacks content-derived evidence and is unsafe for reusable frame cache keys"
    )]
    UnsafeSourceIdentity,
    #[error("invalid decoded frame: {0}")]
    InvalidFrame(String),
}

#[derive(Debug)]
struct RamEntry {
    frame: CachedFrame,
    last_access: u64,
}

/// Byte-bounded hot-frame cache.
///
/// The cache is intentionally weighted by the actual owned RGBA payload size, not frame count.
/// Therefore a 4K frame naturally consumes more of the budget than a small preview. Cache hits
/// clone only `Arc` ownership and do not duplicate the pixel buffer.
#[derive(Debug)]
pub struct RamFrameCache {
    budget_bytes: usize,
    resident_bytes: usize,
    access_clock: u64,
    entries: HashMap<FrameCacheKey, RamEntry>,
    stats: RamCacheStats,
}

impl RamFrameCache {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            budget_bytes,
            resident_bytes: 0,
            access_clock: 0,
            entries: HashMap::new(),
            stats: RamCacheStats::default(),
        }
    }

    pub fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }

    pub fn resident_bytes(&self) -> usize {
        self.resident_bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&mut self, key: &FrameCacheKey) -> Option<CachedFrame> {
        self.access_clock = self.access_clock.wrapping_add(1);
        let now = self.access_clock;
        match self.entries.get_mut(key) {
            Some(entry) => {
                entry.last_access = now;
                self.stats.hits = self.stats.hits.saturating_add(1);
                Some(entry.frame.clone())
            }
            None => {
                self.stats.misses = self.stats.misses.saturating_add(1);
                None
            }
        }
    }

    pub fn insert(&mut self, frame: CachedFrame) -> RamInsertResult {
        let frame_bytes = frame.byte_len();
        if frame_bytes > self.budget_bytes {
            return RamInsertResult::TooLarge;
        }

        self.access_clock = self.access_clock.wrapping_add(1);
        let now = self.access_clock;
        if let Some(previous) = self.entries.remove(&frame.key) {
            self.resident_bytes = self
                .resident_bytes
                .saturating_sub(previous.frame.byte_len());
        }

        self.resident_bytes = self.resident_bytes.saturating_add(frame_bytes);
        self.entries.insert(
            frame.key.clone(),
            RamEntry {
                frame,
                last_access: now,
            },
        );
        self.stats.insertions = self.stats.insertions.saturating_add(1);
        self.evict_to_budget();
        RamInsertResult::Inserted
    }

    pub fn invalidate_source(&mut self, source: &SourceIdentity) {
        let source_key = source.stable_key();
        let keys = self
            .entries
            .keys()
            .filter(|key| key.source_key == source_key)
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(entry) = self.entries.remove(&key) {
                self.resident_bytes = self
                    .resident_bytes
                    .saturating_sub(entry.frame.byte_len());
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.resident_bytes = 0;
    }

    pub fn stats(&self) -> RamCacheStats {
        RamCacheStats {
            resident_bytes: self.resident_bytes,
            resident_frames: self.entries.len(),
            ..self.stats
        }
    }

    fn evict_to_budget(&mut self) {
        while self.resident_bytes > self.budget_bytes {
            let Some(key) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(entry) = self.entries.remove(&key) {
                self.resident_bytes = self
                    .resident_bytes
                    .saturating_sub(entry.frame.byte_len());
                self.stats.evictions = self.stats.evictions.saturating_add(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(tag: &str) -> SourceIdentity {
        SourceIdentity::new(1234, Some(55), Some(tag.into()))
    }

    fn frame(source: &SourceIdentity, id: u64, width: u32, height: u32) -> CachedFrame {
        let stride = usize::try_from(width).unwrap() * 4;
        let bytes = stride * usize::try_from(height).unwrap();
        CachedFrame {
            key: FrameCacheKey::new(source, 0, FrameId(id)).unwrap(),
            pixels: OwnedRgbaFrame::new(width, height, stride, vec![id as u8; bytes]).unwrap(),
        }
    }

    #[test]
    fn rejects_unsafe_source_identity() {
        let weak = SourceIdentity::metadata_only(Some(1234), None, Some("same-name".into()));
        let error = FrameCacheKey::new(&weak, 0, FrameId(0)).unwrap_err();
        assert_eq!(error, FrameCacheError::UnsafeSourceIdentity);
    }

    #[test]
    fn validates_rgba_payload_size() {
        let error = OwnedRgbaFrame::new(2, 2, 8, vec![0; 15]).unwrap_err();
        assert!(matches!(error, FrameCacheError::InvalidFrame(_)));
    }

    #[test]
    fn second_access_is_a_ram_hit_without_copying_pixels() {
        let source = source("video-a");
        let cached = frame(&source, 7, 2, 2);
        let original_pixels = cached.pixels.pixels.clone();
        let key = cached.key.clone();
        let mut cache = RamFrameCache::new(64);
        assert_eq!(cache.insert(cached), RamInsertResult::Inserted);

        let first = cache.get(&key).unwrap();
        let second = cache.get(&key).unwrap();
        assert!(Arc::ptr_eq(&original_pixels, &first.pixels.pixels));
        assert!(Arc::ptr_eq(&first.pixels.pixels, &second.pixels.pixels));
        assert_eq!(cache.stats().hits, 2);
    }

    #[test]
    fn evicts_by_bytes_and_recency() {
        let source = source("video-a");
        let first = frame(&source, 1, 2, 2); // 16 bytes
        let second = frame(&source, 2, 2, 2); // 16 bytes
        let third = frame(&source, 3, 2, 2); // 16 bytes
        let first_key = first.key.clone();
        let second_key = second.key.clone();
        let third_key = third.key.clone();
        let mut cache = RamFrameCache::new(32);

        cache.insert(first);
        cache.insert(second);
        assert!(cache.get(&first_key).is_some()); // make frame 1 newer than frame 2
        cache.insert(third);

        assert!(cache.get(&first_key).is_some());
        assert!(cache.get(&second_key).is_none());
        assert!(cache.get(&third_key).is_some());
        assert_eq!(cache.resident_bytes(), 32);
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn oversized_frame_is_not_cached() {
        let source = source("video-a");
        let mut cache = RamFrameCache::new(8);
        assert_eq!(
            cache.insert(frame(&source, 1, 2, 2)),
            RamInsertResult::TooLarge
        );
        assert!(cache.is_empty());
    }

    #[test]
    fn same_filename_equivalent_metadata_different_content_cannot_collide() {
        let source_a = source("content-a");
        let source_b = source("content-b");
        let key_a = FrameCacheKey::new(&source_a, 0, FrameId(9)).unwrap();
        let key_b = FrameCacheKey::new(&source_b, 0, FrameId(9)).unwrap();
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn source_invalidation_removes_only_that_source() {
        let source_a = source("content-a");
        let source_b = source("content-b");
        let key_b = FrameCacheKey::new(&source_b, 0, FrameId(1)).unwrap();
        let mut cache = RamFrameCache::new(64);
        cache.insert(frame(&source_a, 1, 2, 2));
        cache.insert(frame(&source_b, 1, 2, 2));

        cache.invalidate_source(&source_a);
        assert_eq!(cache.len(), 1);
        assert!(cache.get(&key_b).is_some());
    }
}
