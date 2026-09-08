use framescope_cache::{FrameCacheError, FrameId, OwnedRgbaFrame};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ScrubPreviewError {
    #[error("preview max edge must be positive")]
    InvalidMaxEdge,
    #[error("preview dimensions or byte layout overflow the supported numeric range")]
    NumericRange,
    #[error("preview RGBA frame is invalid: {0}")]
    Frame(#[from] FrameCacheError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrubPreviewCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub insertions: u64,
    pub evictions: u64,
    pub resident_bytes: usize,
    pub resident_frames: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ScrubPreviewKey {
    frame_id: FrameId,
    max_edge: u32,
}

#[derive(Debug)]
struct ScrubPreviewEntry {
    pixels: OwnedRgbaFrame,
    last_access: u64,
}

/// A byte- and count-bounded cache for downscaled live-scrub RGBA frames.
///
/// This cache is intentionally separate from the source-quality frame cache. Every entry is keyed
/// by the exact persistent [FrameId] and the requested preview profile (`max_edge`). A smaller
/// preview is therefore never allowed to satisfy a later larger-preview request. Payloads remain
/// disposable UI previews and must never feed extraction or authoritative microscope presentation.
#[derive(Debug)]
pub struct ScrubPreviewCache {
    budget_bytes: usize,
    max_frames: usize,
    resident_bytes: usize,
    access_clock: u64,
    entries: HashMap<ScrubPreviewKey, ScrubPreviewEntry>,
    hits: u64,
    misses: u64,
    insertions: u64,
    evictions: u64,
}

impl ScrubPreviewCache {
    pub fn new(budget_bytes: usize, max_frames: usize) -> Self {
        Self {
            budget_bytes,
            max_frames,
            resident_bytes: 0,
            access_clock: 0,
            entries: HashMap::new(),
            hits: 0,
            misses: 0,
            insertions: 0,
            evictions: 0,
        }
    }

    pub fn get(&mut self, frame_id: FrameId, max_edge: u32) -> Option<OwnedRgbaFrame> {
        self.access_clock = self.access_clock.saturating_add(1);
        let key = ScrubPreviewKey { frame_id, max_edge };
        let Some(entry) = self.entries.get_mut(&key) else {
            self.misses = self.misses.saturating_add(1);
            return None;
        };
        entry.last_access = self.access_clock;
        self.hits = self.hits.saturating_add(1);
        Some(entry.pixels.clone())
    }

    pub fn insert(&mut self, frame_id: FrameId, max_edge: u32, pixels: OwnedRgbaFrame) {
        if self.budget_bytes == 0 || self.max_frames == 0 || pixels.byte_len() > self.budget_bytes {
            return;
        }

        let key = ScrubPreviewKey { frame_id, max_edge };
        if let Some(previous) = self.entries.remove(&key) {
            self.resident_bytes = self
                .resident_bytes
                .saturating_sub(previous.pixels.byte_len());
        }

        self.evict_until_fits(pixels.byte_len(), true);

        self.access_clock = self.access_clock.saturating_add(1);
        self.resident_bytes = self.resident_bytes.saturating_add(pixels.byte_len());
        self.entries.insert(
            key,
            ScrubPreviewEntry {
                pixels,
                last_access: self.access_clock,
            },
        );
        self.insertions = self.insertions.saturating_add(1);
    }

    /// Changes the byte ceiling and immediately evicts LRU entries until the new ceiling is met.
    pub fn set_budget_bytes(&mut self, budget_bytes: usize) {
        self.budget_bytes = budget_bytes;
        self.evict_until_fits(0, false);
    }

    /// Changes the frame-count ceiling and immediately evicts LRU entries to comply.
    pub fn set_max_frames(&mut self, max_frames: usize) {
        self.max_frames = max_frames;
        self.evict_until_fits(0, false);
    }

    pub fn clear(&mut self) {
        self.evictions = self
            .evictions
            .saturating_add(u64::try_from(self.entries.len()).unwrap_or(u64::MAX));
        self.entries.clear();
        self.resident_bytes = 0;
    }

    pub fn stats(&self) -> ScrubPreviewCacheStats {
        ScrubPreviewCacheStats {
            hits: self.hits,
            misses: self.misses,
            insertions: self.insertions,
            evictions: self.evictions,
            resident_bytes: self.resident_bytes,
            resident_frames: self.entries.len(),
        }
    }

    fn evict_until_fits(&mut self, additional_bytes: usize, inserting: bool) {
        while !self.entries.is_empty()
            && (self.resident_bytes.saturating_add(additional_bytes) > self.budget_bytes
                || if inserting {
                    self.entries.len() >= self.max_frames
                } else {
                    self.entries.len() > self.max_frames
                })
        {
            let Some(lru) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(key, _)| *key)
            else {
                break;
            };
            if let Some(evicted) = self.entries.remove(&lru) {
                self.resident_bytes = self
                    .resident_bytes
                    .saturating_sub(evicted.pixels.byte_len());
                self.evictions = self.evictions.saturating_add(1);
            }
        }
    }
}

/// Downscale RGBA pixels to fit inside [max_edge] while preserving aspect ratio and never upscaling.
///
/// Live scrub prioritizes bounded latency over source-quality resampling. Nearest-neighbour sampling
/// keeps this operation deterministic and cheap; the authoritative source-quality frame is decoded
/// and displayed after the gesture is released.
pub fn downscale_scrub_preview(
    source: &OwnedRgbaFrame,
    max_edge: u32,
) -> Result<OwnedRgbaFrame, ScrubPreviewError> {
    if max_edge == 0 {
        return Err(ScrubPreviewError::InvalidMaxEdge);
    }
    let (target_width, target_height) = preview_dimensions(source.width, source.height, max_edge)?;
    let target_width_usize =
        usize::try_from(target_width).map_err(|_| ScrubPreviewError::NumericRange)?;
    let target_height_usize =
        usize::try_from(target_height).map_err(|_| ScrubPreviewError::NumericRange)?;
    let target_stride = target_width_usize
        .checked_mul(4)
        .ok_or(ScrubPreviewError::NumericRange)?;
    let target_len = target_stride
        .checked_mul(target_height_usize)
        .ok_or(ScrubPreviewError::NumericRange)?;

    if target_width == source.width
        && target_height == source.height
        && source.stride_bytes == target_stride
    {
        return Ok(source.clone());
    }

    let source_width = u64::from(source.width);
    let source_height = u64::from(source.height);
    let target_width_u64 = u64::from(target_width);
    let target_height_u64 = u64::from(target_height);
    let mut output = vec![0_u8; target_len];

    for target_y in 0..target_height_usize {
        let source_y = ((target_y as u64) * source_height / target_height_u64)
            .min(source_height.saturating_sub(1));
        let source_row = usize::try_from(source_y)
            .map_err(|_| ScrubPreviewError::NumericRange)?
            .checked_mul(source.stride_bytes)
            .ok_or(ScrubPreviewError::NumericRange)?;
        let target_row = target_y
            .checked_mul(target_stride)
            .ok_or(ScrubPreviewError::NumericRange)?;

        for target_x in 0..target_width_usize {
            let source_x = ((target_x as u64) * source_width / target_width_u64)
                .min(source_width.saturating_sub(1));
            let source_offset = source_row
                .checked_add(
                    usize::try_from(source_x)
                        .map_err(|_| ScrubPreviewError::NumericRange)?
                        .checked_mul(4)
                        .ok_or(ScrubPreviewError::NumericRange)?,
                )
                .ok_or(ScrubPreviewError::NumericRange)?;
            let target_offset = target_row
                .checked_add(
                    target_x
                        .checked_mul(4)
                        .ok_or(ScrubPreviewError::NumericRange)?,
                )
                .ok_or(ScrubPreviewError::NumericRange)?;
            output[target_offset..target_offset + 4]
                .copy_from_slice(&source.pixels()[source_offset..source_offset + 4]);
        }
    }

    OwnedRgbaFrame::new(target_width, target_height, target_stride, output)
        .map_err(ScrubPreviewError::from)
}

fn preview_dimensions(
    width: u32,
    height: u32,
    max_edge: u32,
) -> Result<(u32, u32), ScrubPreviewError> {
    if width == 0 || height == 0 || max_edge == 0 {
        return Err(ScrubPreviewError::InvalidMaxEdge);
    }
    let longest = width.max(height);
    if longest <= max_edge {
        return Ok((width, height));
    }

    let width_u64 = u64::from(width);
    let height_u64 = u64::from(height);
    let longest_u64 = u64::from(longest);
    let max_edge_u64 = u64::from(max_edge);
    let scaled_width = width_u64
        .checked_mul(max_edge_u64)
        .ok_or(ScrubPreviewError::NumericRange)?
        .checked_add(longest_u64 / 2)
        .ok_or(ScrubPreviewError::NumericRange)?
        / longest_u64;
    let scaled_height = height_u64
        .checked_mul(max_edge_u64)
        .ok_or(ScrubPreviewError::NumericRange)?
        .checked_add(longest_u64 / 2)
        .ok_or(ScrubPreviewError::NumericRange)?
        / longest_u64;

    Ok((
        u32::try_from(scaled_width.max(1)).map_err(|_| ScrubPreviewError::NumericRange)?,
        u32::try_from(scaled_height.max(1)).map_err(|_| ScrubPreviewError::NumericRange)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(width: u32, height: u32, marker: u8) -> OwnedRgbaFrame {
        let stride = width as usize * 4;
        OwnedRgbaFrame::new(
            width,
            height,
            stride,
            vec![marker; stride * height as usize],
        )
        .unwrap()
    }

    #[test]
    fn downscale_preserves_aspect_and_bounds_long_edge() {
        let source = rgba(1_920, 1_080, 7);
        let preview = downscale_scrub_preview(&source, 640).unwrap();
        assert_eq!((preview.width, preview.height), (640, 360));
        assert_eq!(preview.stride_bytes, 640 * 4);
        assert_eq!(preview.byte_len(), 640 * 360 * 4);
        assert!(preview.pixels().iter().all(|value| *value == 7));
    }

    #[test]
    fn small_frames_are_not_upscaled() {
        let source = rgba(320, 180, 9);
        let preview = downscale_scrub_preview(&source, 640).unwrap();
        assert_eq!(preview, source);
    }

    #[test]
    fn preview_cache_is_bounded_by_frame_count_and_bytes() {
        let mut cache = ScrubPreviewCache::new(2 * 16, 2);
        cache.insert(FrameId(1), 640, rgba(2, 2, 1));
        cache.insert(FrameId(2), 640, rgba(2, 2, 2));
        assert_eq!(cache.stats().resident_frames, 2);
        assert!(cache.get(FrameId(1), 640).is_some());

        cache.insert(FrameId(3), 640, rgba(2, 2, 3));
        let stats = cache.stats();
        assert_eq!(stats.resident_frames, 2);
        assert_eq!(stats.resident_bytes, 32);
        assert_eq!(stats.evictions, 1);
        assert!(cache.get(FrameId(1), 640).is_some());
        assert!(cache.get(FrameId(2), 640).is_none());
        assert!(cache.get(FrameId(3), 640).is_some());
    }

    #[test]
    fn preview_profile_is_part_of_cache_identity() {
        let mut cache = ScrubPreviewCache::new(1024, 4);
        cache.insert(FrameId(9), 320, rgba(2, 2, 7));

        assert!(cache.get(FrameId(9), 320).is_some());
        assert!(cache.get(FrameId(9), 640).is_none());
    }

    #[test]
    fn shrinking_budget_evicts_immediately() {
        let mut cache = ScrubPreviewCache::new(64, 4);
        cache.insert(FrameId(1), 320, rgba(2, 2, 1));
        cache.insert(FrameId(2), 320, rgba(2, 2, 2));
        assert_eq!(cache.stats().resident_bytes, 32);

        cache.set_budget_bytes(16);
        assert_eq!(cache.stats().resident_bytes, 16);
        assert_eq!(cache.stats().resident_frames, 1);

        cache.set_budget_bytes(0);
        assert_eq!(cache.stats().resident_bytes, 0);
        assert_eq!(cache.stats().resident_frames, 0);
    }

    #[test]
    fn shrinking_frame_limit_evicts_immediately() {
        let mut cache = ScrubPreviewCache::new(64, 4);
        cache.insert(FrameId(1), 320, rgba(2, 2, 1));
        cache.insert(FrameId(2), 320, rgba(2, 2, 2));

        cache.set_max_frames(1);
        assert_eq!(cache.stats().resident_frames, 1);
        assert_eq!(cache.stats().resident_bytes, 16);

        cache.set_max_frames(0);
        assert_eq!(cache.stats().resident_frames, 0);
    }

    #[test]
    fn oversized_preview_is_not_cached() {
        let mut cache = ScrubPreviewCache::new(15, 4);
        cache.insert(FrameId(1), 320, rgba(2, 2, 1));
        assert_eq!(cache.stats().resident_frames, 0);
        assert!(cache.get(FrameId(1), 320).is_none());
    }
}
