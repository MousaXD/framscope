//! Phase 3 verification helpers.
//!
//! This crate contains test oracles and instrumentation contracts only. It does
//! not implement the production index, seek engine, or cache.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampSelection {
    AtOrBefore,
    AtOrAfter,
    Nearest,
}

/// Returns the expected presentation-frame index for a sorted timestamp slice.
///
/// `AtOrBefore` chooses the last equal timestamp, `AtOrAfter` chooses the first
/// equal timestamp, and `Nearest` chooses the first exact match or, between two
/// timestamps, the earlier frame on an equal-distance tie.
pub fn select_timestamp_index(
    timestamps: &[i64],
    target: i64,
    selection: TimestampSelection,
) -> Option<usize> {
    if timestamps.is_empty() {
        return None;
    }

    match selection {
        TimestampSelection::AtOrBefore => {
            let end = timestamps.partition_point(|timestamp| *timestamp <= target);
            end.checked_sub(1)
        }
        TimestampSelection::AtOrAfter => {
            let start = timestamps.partition_point(|timestamp| *timestamp < target);
            (start < timestamps.len()).then_some(start)
        }
        TimestampSelection::Nearest => {
            let after = timestamps.partition_point(|timestamp| *timestamp < target);
            if after == 0 {
                return Some(0);
            }
            if after == timestamps.len() {
                return Some(timestamps.len() - 1);
            }
            if timestamps[after] == target {
                return Some(after);
            }

            let before = after - 1;
            let before_distance = target.saturating_sub(timestamps[before]);
            let after_distance = timestamps[after].saturating_sub(target);
            Some(if before_distance <= after_distance {
                before
            } else {
                after
            })
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamingCounters {
    pub decoded_frames: u64,
    pub max_live_decoded_frames: usize,
    pub persisted_batches: u64,
    pub peak_buffered_metadata_entries: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AccessCounters {
    pub decoded_frames: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub disk_proxy_reads: u64,
    pub evictions: u64,
    pub source_invalidations: u64,
}

impl AccessCounters {
    #[must_use]
    pub fn delta_since(self, earlier: Self) -> Self {
        Self {
            decoded_frames: self.decoded_frames.saturating_sub(earlier.decoded_frames),
            cache_hits: self.cache_hits.saturating_sub(earlier.cache_hits),
            cache_misses: self.cache_misses.saturating_sub(earlier.cache_misses),
            disk_proxy_reads: self
                .disk_proxy_reads
                .saturating_sub(earlier.disk_proxy_reads),
            evictions: self.evictions.saturating_sub(earlier.evictions),
            source_invalidations: self
                .source_invalidations
                .saturating_sub(earlier.source_invalidations),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractViolation {
    DecodedFrameCount { expected: u64, actual: u64 },
    LiveFrameBound { limit: usize, actual: usize },
    MetadataBufferBound { limit: usize, actual: usize },
    PersistBatchCount { minimum: u64, actual: u64 },
    ExpectedCacheHit,
    UnexpectedDecodeOnWarmHit { decoded_frames: u64 },
    ExpectedCacheMiss,
    ExpectedDecodeFallback,
    ExpectedSourceInvalidation,
    StaleCacheHit { cache_hits: u64 },
}

pub fn verify_bounded_streaming(
    counters: StreamingCounters,
    expected_decoded_frames: u64,
    live_frame_limit: usize,
    metadata_batch_limit: usize,
    minimum_persist_batches: u64,
) -> Result<(), ContractViolation> {
    if counters.decoded_frames != expected_decoded_frames {
        return Err(ContractViolation::DecodedFrameCount {
            expected: expected_decoded_frames,
            actual: counters.decoded_frames,
        });
    }
    if counters.max_live_decoded_frames > live_frame_limit {
        return Err(ContractViolation::LiveFrameBound {
            limit: live_frame_limit,
            actual: counters.max_live_decoded_frames,
        });
    }
    if counters.peak_buffered_metadata_entries > metadata_batch_limit {
        return Err(ContractViolation::MetadataBufferBound {
            limit: metadata_batch_limit,
            actual: counters.peak_buffered_metadata_entries,
        });
    }
    if counters.persisted_batches < minimum_persist_batches {
        return Err(ContractViolation::PersistBatchCount {
            minimum: minimum_persist_batches,
            actual: counters.persisted_batches,
        });
    }
    Ok(())
}

pub fn verify_cold_access(counters: AccessCounters) -> Result<(), ContractViolation> {
    if counters.cache_misses == 0 {
        return Err(ContractViolation::ExpectedCacheMiss);
    }
    if counters.decoded_frames == 0 && counters.disk_proxy_reads == 0 {
        return Err(ContractViolation::ExpectedDecodeFallback);
    }
    Ok(())
}

pub fn verify_warm_access(counters: AccessCounters) -> Result<(), ContractViolation> {
    if counters.cache_hits == 0 {
        return Err(ContractViolation::ExpectedCacheHit);
    }
    if counters.decoded_frames != 0 {
        return Err(ContractViolation::UnexpectedDecodeOnWarmHit {
            decoded_frames: counters.decoded_frames,
        });
    }
    Ok(())
}

pub fn verify_evicted_access(counters: AccessCounters) -> Result<(), ContractViolation> {
    if counters.cache_misses == 0 {
        return Err(ContractViolation::ExpectedCacheMiss);
    }
    if counters.decoded_frames == 0 && counters.disk_proxy_reads == 0 {
        return Err(ContractViolation::ExpectedDecodeFallback);
    }
    Ok(())
}

pub fn verify_stale_source_access(counters: AccessCounters) -> Result<(), ContractViolation> {
    if counters.source_invalidations == 0 {
        return Err(ContractViolation::ExpectedSourceInvalidation);
    }
    if counters.cache_hits != 0 {
        return Err(ContractViolation::StaleCacheHit {
            cache_hits: counters.cache_hits,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_selection_defines_between_and_eof_behavior() {
        let pts = [0, 10, 25, 40];
        assert_eq!(
            select_timestamp_index(&pts, 20, TimestampSelection::AtOrBefore),
            Some(1)
        );
        assert_eq!(
            select_timestamp_index(&pts, 20, TimestampSelection::AtOrAfter),
            Some(2)
        );
        assert_eq!(
            select_timestamp_index(&pts, 20, TimestampSelection::Nearest),
            Some(2)
        );
        assert_eq!(
            select_timestamp_index(&pts, 5, TimestampSelection::Nearest),
            Some(0)
        );
        assert_eq!(
            select_timestamp_index(&pts, -1, TimestampSelection::AtOrBefore),
            None
        );
        assert_eq!(
            select_timestamp_index(&pts, 41, TimestampSelection::AtOrAfter),
            None
        );
    }

    #[test]
    fn duplicate_timestamp_semantics_are_directional_and_stable() {
        let pts = [0, 10, 10, 20];
        assert_eq!(
            select_timestamp_index(&pts, 10, TimestampSelection::AtOrBefore),
            Some(2)
        );
        assert_eq!(
            select_timestamp_index(&pts, 10, TimestampSelection::AtOrAfter),
            Some(1)
        );
        assert_eq!(
            select_timestamp_index(&pts, 10, TimestampSelection::Nearest),
            Some(1)
        );
    }

    #[test]
    fn bounded_streaming_is_structural_not_absolute_rss() {
        let counters = StreamingCounters {
            decoded_frames: 10_000,
            max_live_decoded_frames: 3,
            persisted_batches: 40,
            peak_buffered_metadata_entries: 256,
        };
        assert_eq!(
            verify_bounded_streaming(counters, 10_000, 4, 256, 2),
            Ok(())
        );
    }

    #[test]
    fn warm_access_rejects_redundant_decode() {
        let counters = AccessCounters {
            cache_hits: 1,
            decoded_frames: 1,
            ..AccessCounters::default()
        };
        assert_eq!(
            verify_warm_access(counters),
            Err(ContractViolation::UnexpectedDecodeOnWarmHit { decoded_frames: 1 })
        );
    }
}
