use framescope_cache::{FrameIndexEntry, OwnedRgbaFrame};
use framescope_grouping::{HybridFrameGrouper, HybridGroupingError};
use framescope_perceptual::HybridSimilarityPolicy;
use framescope_similarity::FrameGroup;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use thiserror::Error;

/// Cheap cooperative cancellation shared between a controller and one grouping session.
#[derive(Debug, Clone, Default)]
pub struct GroupingCancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl GroupingCancellationToken {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GroupingSessionStats {
    pub frames_processed: u64,
    pub groups_emitted: u64,
    pub cancellation_checks: u64,
}

#[derive(Debug, Error)]
pub enum GroupingSessionError {
    #[error("frame grouping cancelled")]
    Cancelled,
    #[error(transparent)]
    Grouping(#[from] HybridGroupingError),
}

/// Cancellation-aware orchestration around the bounded streaming hybrid grouper.
///
/// The session never accumulates source frames or completed groups. A cancelled push is rejected
/// before the underlying grouper is mutated, so callers can discard the incomplete derived result
/// without affecting the authoritative Phase 3 timeline/index.
#[derive(Debug, Clone)]
pub struct HybridGroupingSession {
    grouper: HybridFrameGrouper,
    cancellation: GroupingCancellationToken,
    stats: GroupingSessionStats,
}

impl HybridGroupingSession {
    pub fn new(policy: HybridSimilarityPolicy) -> Result<Self, GroupingSessionError> {
        Self::with_cancellation(policy, GroupingCancellationToken::default())
    }

    pub fn with_cancellation(
        policy: HybridSimilarityPolicy,
        cancellation: GroupingCancellationToken,
    ) -> Result<Self, GroupingSessionError> {
        Ok(Self {
            grouper: HybridFrameGrouper::new(policy)?,
            cancellation,
            stats: GroupingSessionStats::default(),
        })
    }

    pub fn cancellation_token(&self) -> GroupingCancellationToken {
        self.cancellation.clone()
    }

    pub fn stats(&self) -> GroupingSessionStats {
        self.stats
    }

    pub fn push(
        &mut self,
        entry: &FrameIndexEntry,
        pixels: OwnedRgbaFrame,
    ) -> Result<Option<FrameGroup>, GroupingSessionError> {
        self.stats.cancellation_checks = self.stats.cancellation_checks.saturating_add(1);
        if self.cancellation.is_cancelled() {
            return Err(GroupingSessionError::Cancelled);
        }

        let completed = self.grouper.push(entry, pixels)?;
        self.stats.frames_processed = self.stats.frames_processed.saturating_add(1);
        if completed.is_some() {
            self.stats.groups_emitted = self.stats.groups_emitted.saturating_add(1);
        }
        Ok(completed)
    }

    pub fn finish(&mut self) -> Result<Option<FrameGroup>, GroupingSessionError> {
        self.stats.cancellation_checks = self.stats.cancellation_checks.saturating_add(1);
        if self.cancellation.is_cancelled() {
            return Err(GroupingSessionError::Cancelled);
        }

        let completed = self.grouper.finish();
        if completed.is_some() {
            self.stats.groups_emitted = self.stats.groups_emitted.saturating_add(1);
        }
        Ok(completed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{FrameId, KeyframeAnchor};
    use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};

    fn time_base() -> TimeBase {
        TimeBase::new(1, 1000).unwrap()
    }

    fn policy() -> HybridSimilarityPolicy {
        HybridSimilarityPolicy {
            max_hash_distance: 8,
            minimum_luma_similarity: 9_700,
        }
    }

    fn entry(id: u64) -> FrameIndexEntry {
        FrameIndexEntry {
            frame_id: FrameId(id),
            presentation_timestamp: Some(MediaTimestamp {
                ticks: i64::try_from(id).unwrap() * 40,
                time_base: time_base(),
            }),
            duration: Some(MediaDuration {
                ticks: 40,
                time_base: time_base(),
            }),
            keyframe: id == 0,
            corrupt: false,
            anchor: KeyframeAnchor::Keyframe {
                frame_id: FrameId(0),
                presentation_timestamp: Some(MediaTimestamp {
                    ticks: 0,
                    time_base: time_base(),
                }),
            },
        }
    }

    fn solid(value: u8) -> OwnedRgbaFrame {
        let mut pixels = Vec::with_capacity(9 * 8 * 4);
        for _ in 0..(9 * 8) {
            pixels.extend_from_slice(&[value, value, value, 255]);
        }
        OwnedRgbaFrame::new(9, 8, 36, pixels).unwrap()
    }

    #[test]
    fn cancellation_before_push_does_not_mutate_session_progress() {
        let token = GroupingCancellationToken::default();
        let mut session = HybridGroupingSession::with_cancellation(policy(), token.clone()).unwrap();
        token.cancel();

        assert!(matches!(
            session.push(&entry(0), solid(20)),
            Err(GroupingSessionError::Cancelled)
        ));
        assert_eq!(session.stats().frames_processed, 0);
        assert_eq!(session.stats().groups_emitted, 0);
        assert_eq!(session.stats().cancellation_checks, 1);
    }

    #[test]
    fn cancellation_after_progress_stops_before_next_frame() {
        let token = GroupingCancellationToken::default();
        let mut session = HybridGroupingSession::with_cancellation(policy(), token.clone()).unwrap();
        assert!(session.push(&entry(0), solid(20)).unwrap().is_none());
        token.cancel();

        assert!(matches!(
            session.push(&entry(1), solid(20)),
            Err(GroupingSessionError::Cancelled)
        ));
        assert!(matches!(session.finish(), Err(GroupingSessionError::Cancelled)));
        assert_eq!(session.stats().frames_processed, 1);
    }

    #[test]
    fn stats_are_instance_bound() {
        let mut first = HybridGroupingSession::new(policy()).unwrap();
        let second = HybridGroupingSession::new(policy()).unwrap();
        assert!(first.push(&entry(0), solid(10)).unwrap().is_none());

        assert_eq!(first.stats().frames_processed, 1);
        assert_eq!(second.stats(), GroupingSessionStats::default());
    }

    #[test]
    fn long_stream_is_consumed_incrementally_without_collecting_inputs() {
        let mut session = HybridGroupingSession::new(policy()).unwrap();
        let mut emitted = 0_u64;

        for id in 0..20_000_u64 {
            if session.push(&entry(id), solid((id % 2) as u8)).unwrap().is_some() {
                emitted += 1;
            }
        }
        if session.finish().unwrap().is_some() {
            emitted += 1;
        }

        assert_eq!(session.stats().frames_processed, 20_000);
        assert_eq!(session.stats().groups_emitted, emitted);
        assert!(emitted >= 1);
    }
}
