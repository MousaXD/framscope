//! Streaming orchestration for perceptually gated adjacent-frame grouping.
//!
//! This crate sits above the low-level similarity and perceptual crates so dependency direction
//! stays acyclic. Groups remain a derived view over authoritative Phase 3 frame identities and
//! timestamps; no frame is deleted or renumbered.

use framescope_cache::{FrameIndexEntry, OwnedRgbaFrame};
use framescope_core::MediaTimestamp;
use framescope_perceptual::{
    HybridDecision, HybridSimilarityEngine, HybridSimilarityError, HybridSimilarityPolicy,
};
use framescope_similarity::{FrameGroup, SIMILARITY_SCALE, SimilarityError, SimilarityScore};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HybridGroupingError {
    #[error(transparent)]
    Similarity(#[from] SimilarityError),
    #[error(transparent)]
    Hybrid(#[from] HybridSimilarityError),
}

#[derive(Debug, Clone)]
struct ActiveGroup {
    metadata: FrameGroup,
    representative: OwnedRgbaFrame,
    previous: OwnedRgbaFrame,
}

/// Bounded streaming grouper using dHash as a rejection prefilter and full color confirmation.
///
/// A candidate joins only when it is accepted against both the immediately previous frame and the
/// canonical representative. This preserves the anti-chain-drift invariant while retaining only
/// two owned full-resolution frames plus metadata, regardless of video length.
#[derive(Debug, Clone)]
pub struct HybridFrameGrouper {
    engine: HybridSimilarityEngine,
    active: Option<ActiveGroup>,
}

impl HybridFrameGrouper {
    pub fn new(policy: HybridSimilarityPolicy) -> Result<Self, HybridGroupingError> {
        Ok(Self {
            engine: HybridSimilarityEngine::new(policy)?,
            active: None,
        })
    }

    pub fn policy(&self) -> HybridSimilarityPolicy {
        self.engine.policy()
    }

    pub fn push(
        &mut self,
        entry: &FrameIndexEntry,
        pixels: OwnedRgbaFrame,
    ) -> Result<Option<FrameGroup>, HybridGroupingError> {
        let timestamp = entry
            .presentation_timestamp
            .ok_or(SimilarityError::MissingTimestamp(entry.frame_id))?;

        let Some(active) = self.active.as_mut() else {
            self.active = Some(active_group(entry, timestamp, pixels));
            return Ok(None);
        };

        let expected = active.metadata.last_frame.0.checked_add(1);
        if expected != Some(entry.frame_id.0) {
            return Err(SimilarityError::NonContiguousFrameIds {
                previous: active.metadata.last_frame,
                current: entry.frame_id,
            }
            .into());
        }

        let previous = self.engine.compare(&active.previous, &pixels)?;
        if accepted_confirmation(previous).is_none() {
            let completed = active.metadata.clone();
            self.active = Some(active_group(entry, timestamp, pixels));
            return Ok(Some(completed));
        }

        let representative = self.engine.compare(&active.representative, &pixels)?;
        let Some(representative_confirmation) = accepted_confirmation(representative) else {
            let completed = active.metadata.clone();
            self.active = Some(active_group(entry, timestamp, pixels));
            return Ok(Some(completed));
        };

        active.metadata.last_frame = entry.frame_id;
        active.metadata.frame_count += 1;
        active.metadata.end_timestamp = timestamp;
        active.metadata.end_duration = entry.duration;
        active.metadata.representative_similarity_floor = active
            .metadata
            .representative_similarity_floor
            .min(representative_confirmation.basis_points);
        active.previous = pixels;
        Ok(None)
    }

    pub fn finish(&mut self) -> Option<FrameGroup> {
        self.active.take().map(|active| active.metadata)
    }
}

fn accepted_confirmation(decision: HybridDecision) -> Option<SimilarityScore> {
    match decision {
        HybridDecision::Accepted { confirmation, .. } => Some(confirmation),
        HybridDecision::RejectedByHash { .. } | HybridDecision::RejectedByConfirmation { .. } => {
            None
        }
    }
}

fn active_group(
    entry: &FrameIndexEntry,
    timestamp: MediaTimestamp,
    pixels: OwnedRgbaFrame,
) -> ActiveGroup {
    ActiveGroup {
        metadata: FrameGroup {
            representative_frame: entry.frame_id,
            first_frame: entry.frame_id,
            last_frame: entry.frame_id,
            frame_count: 1,
            start_timestamp: timestamp,
            end_timestamp: timestamp,
            start_duration: entry.duration,
            end_duration: entry.duration,
            representative_similarity_floor: SIMILARITY_SCALE,
        },
        representative: pixels.clone(),
        previous: pixels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{FrameId, KeyframeAnchor};
    use framescope_core::{MediaDuration, TimeBase};

    fn time_base() -> TimeBase {
        TimeBase::new(1, 1000).unwrap()
    }

    fn solid(value: u8) -> OwnedRgbaFrame {
        let mut pixels = Vec::with_capacity(9 * 8 * 4);
        for _ in 0..(9 * 8) {
            pixels.extend_from_slice(&[value, value, value, 255]);
        }
        OwnedRgbaFrame::new(9, 8, 36, pixels).unwrap()
    }

    fn gradient(reverse: bool) -> OwnedRgbaFrame {
        let mut pixels = vec![0_u8; 9 * 8 * 4];
        for y in 0..8 {
            for x in 0..9 {
                let column = if reverse { 8 - x } else { x };
                let value = (column * 20) as u8;
                let offset = (y * 9 + x) * 4;
                pixels[offset..offset + 4].copy_from_slice(&[value, value, value, 255]);
            }
        }
        OwnedRgbaFrame::new(9, 8, 36, pixels).unwrap()
    }

    fn entry(id: u64, ticks: i64, duration: i64) -> FrameIndexEntry {
        FrameIndexEntry {
            frame_id: FrameId(id),
            presentation_timestamp: Some(MediaTimestamp {
                ticks,
                time_base: time_base(),
            }),
            duration: Some(MediaDuration {
                ticks: duration,
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

    fn policy() -> HybridSimilarityPolicy {
        HybridSimilarityPolicy {
            max_hash_distance: 8,
            minimum_luma_similarity: 9_700,
        }
    }

    #[test]
    fn preserves_vfr_timing_and_authoritative_frame_ids() {
        let mut grouper = HybridFrameGrouper::new(policy()).unwrap();
        assert!(grouper.push(&entry(0, 0, 40), solid(20)).unwrap().is_none());
        assert!(
            grouper
                .push(&entry(1, 40, 85), solid(22))
                .unwrap()
                .is_none()
        );
        let first = grouper
            .push(&entry(2, 125, 33), solid(120))
            .unwrap()
            .unwrap();

        assert_eq!(first.first_frame, FrameId(0));
        assert_eq!(first.last_frame, FrameId(1));
        assert_eq!(first.frame_count, 2);
        assert_eq!(first.start_timestamp.ticks, 0);
        assert_eq!(first.end_timestamp.ticks, 40);
        assert_eq!(first.start_duration.unwrap().ticks, 40);
        assert_eq!(first.end_duration.unwrap().ticks, 85);

        let second = grouper.finish().unwrap();
        assert_eq!(second.representative_frame, FrameId(2));
        assert_eq!(second.start_timestamp.ticks, 125);
        assert_eq!(second.end_duration.unwrap().ticks, 33);
    }

    #[test]
    fn representative_confirmation_prevents_transitive_chain_drift() {
        let mut grouper = HybridFrameGrouper::new(policy()).unwrap();
        assert!(grouper.push(&entry(0, 0, 40), solid(0)).unwrap().is_none());
        assert!(grouper.push(&entry(1, 40, 40), solid(5)).unwrap().is_none());
        let completed = grouper.push(&entry(2, 80, 40), solid(10)).unwrap().unwrap();

        assert_eq!(completed.first_frame, FrameId(0));
        assert_eq!(completed.last_frame, FrameId(1));
        assert_eq!(completed.frame_count, 2);
        assert_eq!(grouper.finish().unwrap().first_frame, FrameId(2));
    }

    fn solid_rgb(r: u8, g: u8, b: u8) -> OwnedRgbaFrame {
        let mut pixels = Vec::with_capacity(9 * 8 * 4);
        for _ in 0..(9 * 8) {
            pixels.extend_from_slice(&[r, g, b, 255]);
        }
        OwnedRgbaFrame::new(9, 8, 36, pixels).unwrap()
    }

    #[test]
    fn equal_luma_different_hue_frames_do_not_group() {
        let mut grouper = HybridFrameGrouper::new(policy()).unwrap();
        assert!(
            grouper
                .push(&entry(0, 0, 40), solid_rgb(255, 0, 0))
                .unwrap()
                .is_none()
        );
        let completed = grouper
            .push(&entry(1, 40, 40), solid_rgb(0, 131, 0))
            .unwrap()
            .unwrap();
        assert_eq!(completed.first_frame, FrameId(0));
        assert_eq!(completed.last_frame, FrameId(0));
        assert_eq!(grouper.finish().unwrap().first_frame, FrameId(1));
    }

    #[test]
    fn a_b_a_remains_three_consecutive_runs_not_a_global_cluster() {
        let mut grouper = HybridFrameGrouper::new(policy()).unwrap();
        assert!(grouper.push(&entry(0, 0, 40), solid(10)).unwrap().is_none());
        let first = grouper
            .push(&entry(1, 40, 40), solid(220))
            .unwrap()
            .unwrap();
        let second = grouper.push(&entry(2, 80, 40), solid(10)).unwrap().unwrap();
        let third = grouper.finish().unwrap();
        assert_eq!((first.first_frame.0, first.last_frame.0), (0, 0));
        assert_eq!((second.first_frame.0, second.last_frame.0), (1, 1));
        assert_eq!((third.first_frame.0, third.last_frame.0), (2, 2));
    }

    #[test]
    fn perceptual_rejection_creates_boundary_without_deleting_frames() {
        let mut grouper = HybridFrameGrouper::new(HybridSimilarityPolicy {
            max_hash_distance: 0,
            minimum_luma_similarity: 9_000,
        })
        .unwrap();
        assert!(
            grouper
                .push(&entry(0, 0, 40), gradient(false))
                .unwrap()
                .is_none()
        );
        let completed = grouper
            .push(&entry(1, 40, 40), gradient(true))
            .unwrap()
            .unwrap();

        assert_eq!(completed.first_frame, FrameId(0));
        assert_eq!(completed.last_frame, FrameId(0));
        let remaining = grouper.finish().unwrap();
        assert_eq!(remaining.first_frame, FrameId(1));
        assert_eq!(remaining.last_frame, FrameId(1));
    }

    #[test]
    fn non_contiguous_authoritative_ids_are_rejected() {
        let mut grouper = HybridFrameGrouper::new(policy()).unwrap();
        assert!(grouper.push(&entry(0, 0, 40), solid(10)).unwrap().is_none());
        assert!(matches!(
            grouper.push(&entry(2, 80, 40), solid(10)),
            Err(HybridGroupingError::Similarity(
                SimilarityError::NonContiguousFrameIds { .. }
            ))
        ));
    }
}
