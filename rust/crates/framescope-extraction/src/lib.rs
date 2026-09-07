//! Bounded extraction selection and progress planning for FrameScope.
//!
//! This crate intentionally does not encode images or own Android storage. It resolves user-facing
//! frame/time selections against the authoritative persistent frame index and produces a constant-
//! memory stream of persistent `FrameId`s for later source-quality decode/export stages.

use framescope_cache::{FrameId, FrameIndex, FrameIndexError, FrameIndexLifecycle};
use std::iter::FusedIterator;
use std::num::NonZeroU64;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionSelection {
    CurrentFrame(FrameId),
    FrameRangeInclusive {
        start: FrameId,
        end: FrameId,
    },
    TimestampRangeUsInclusive {
        start_us: i64,
        end_us: i64,
    },
    AllFrames,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionSampling {
    EveryFrame,
    EveryNthFrame(NonZeroU64),
}

impl ExtractionSampling {
    pub fn stride_frames(self) -> NonZeroU64 {
        match self {
            Self::EveryFrame => NonZeroU64::MIN,
            Self::EveryNthFrame(stride) => stride,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractionRequest {
    pub selection: ExtractionSelection,
    pub sampling: ExtractionSampling,
}

impl ExtractionRequest {
    pub fn current_frame(frame_id: FrameId) -> Self {
        Self {
            selection: ExtractionSelection::CurrentFrame(frame_id),
            sampling: ExtractionSampling::EveryFrame,
        }
    }

    pub fn all_frames() -> Self {
        Self {
            selection: ExtractionSelection::AllFrames,
            sampling: ExtractionSampling::EveryFrame,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractionPlan {
    pub first_frame: FrameId,
    pub last_frame: FrameId,
    pub stride_frames: NonZeroU64,
    pub selected_count: u64,
}

impl ExtractionPlan {
    pub fn frame_ids(self) -> PlannedFrameIds {
        PlannedFrameIds {
            next: Some(self.first_frame.0),
            last: self.last_frame.0,
            stride: self.stride_frames.get(),
        }
    }

    pub fn contains(self, frame_id: FrameId) -> bool {
        if frame_id.0 < self.first_frame.0 || frame_id.0 > self.last_frame.0 {
            return false;
        }
        (frame_id.0 - self.first_frame.0).is_multiple_of(self.stride_frames.get())
    }

    /// Visit planned frame identities without allocating a video-wide selection vector.
    ///
    /// Cancellation is checked immediately before each visitor call. `ordinal` is one-based and
    /// `total` is exact for the resolved plan, allowing stable progress reporting without FPS math.
    pub fn visit_frame_ids<E>(
        self,
        mut is_cancelled: impl FnMut() -> bool,
        mut visitor: impl FnMut(ExtractionProgress) -> Result<(), E>,
    ) -> Result<(), ExtractionVisitError<E>> {
        let mut ordinal = 0_u64;
        for frame_id in self.frame_ids() {
            if is_cancelled() {
                return Err(ExtractionVisitError::Cancelled);
            }
            ordinal = ordinal.saturating_add(1);
            visitor(ExtractionProgress {
                frame_id,
                ordinal,
                total: self.selected_count,
            })
            .map_err(ExtractionVisitError::Visitor)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractionProgress {
    pub frame_id: FrameId,
    /// One-based selected-frame ordinal.
    pub ordinal: u64,
    pub total: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ExtractionVisitError<E> {
    Cancelled,
    Visitor(E),
}

#[derive(Debug, Clone)]
pub struct PlannedFrameIds {
    next: Option<u64>,
    last: u64,
    stride: u64,
}

impl Iterator for PlannedFrameIds {
    type Item = FrameId;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.next?;
        if current > self.last {
            self.next = None;
            return None;
        }

        self.next = current
            .checked_add(self.stride)
            .filter(|next| *next <= self.last);
        Some(FrameId(current))
    }
}

impl FusedIterator for PlannedFrameIds {}

#[derive(Debug, Error)]
pub enum ExtractionPlanError {
    #[error("frame index is not complete")]
    IncompleteIndex,
    #[error("complete frame index is missing its frame count")]
    MissingFrameCount,
    #[error("frame index contains no extractable frames")]
    EmptyIndex,
    #[error("requested frame {requested} is outside the indexed range of {frame_count} frames")]
    FrameOutOfRange { requested: u64, frame_count: u64 },
    #[error("frame range start {start} follows end {end}")]
    InvalidFrameRange { start: u64, end: u64 },
    #[error("timestamp range start {start_us}us follows end {end_us}us")]
    InvalidTimestampRange { start_us: i64, end_us: i64 },
    #[error("no indexed frame has a presentation timestamp inside the requested range")]
    NoFramesInTimestampRange,
    #[error("extraction plan numeric range overflow: {0}")]
    NumericRange(&'static str),
    #[error(transparent)]
    Index(#[from] FrameIndexError),
}

pub fn plan_extraction(
    index: &FrameIndex,
    request: ExtractionRequest,
) -> Result<ExtractionPlan, ExtractionPlanError> {
    let status = index.status()?;
    if status.lifecycle != FrameIndexLifecycle::Complete {
        return Err(ExtractionPlanError::IncompleteIndex);
    }
    let frame_count = status
        .frame_count
        .ok_or(ExtractionPlanError::MissingFrameCount)?;
    if frame_count == 0 {
        return Err(ExtractionPlanError::EmptyIndex);
    }

    let (first_frame, last_frame) = match request.selection {
        ExtractionSelection::CurrentFrame(frame_id) => {
            ensure_frame_in_range(frame_id, frame_count)?;
            (frame_id, frame_id)
        }
        ExtractionSelection::FrameRangeInclusive { start, end } => {
            if start.0 > end.0 {
                return Err(ExtractionPlanError::InvalidFrameRange {
                    start: start.0,
                    end: end.0,
                });
            }
            ensure_frame_in_range(start, frame_count)?;
            ensure_frame_in_range(end, frame_count)?;
            (start, end)
        }
        ExtractionSelection::TimestampRangeUsInclusive { start_us, end_us } => {
            if start_us > end_us {
                return Err(ExtractionPlanError::InvalidTimestampRange { start_us, end_us });
            }
            let first = index
                .frame_at_or_after_us(start_us)?
                .ok_or(ExtractionPlanError::NoFramesInTimestampRange)?;
            let last = index
                .frame_at_or_before_us(end_us)?
                .ok_or(ExtractionPlanError::NoFramesInTimestampRange)?;
            let first_us = first
                .timestamp_us()
                .ok_or(ExtractionPlanError::NoFramesInTimestampRange)?;
            let last_us = last
                .timestamp_us()
                .ok_or(ExtractionPlanError::NoFramesInTimestampRange)?;
            if first.frame_id.0 > last.frame_id.0 || first_us > end_us || last_us < start_us {
                return Err(ExtractionPlanError::NoFramesInTimestampRange);
            }
            (first.frame_id, last.frame_id)
        }
        ExtractionSelection::AllFrames => (FrameId::ZERO, FrameId(frame_count - 1)),
    };

    let stride_frames = request.sampling.stride_frames();
    let span = last_frame
        .0
        .checked_sub(first_frame.0)
        .ok_or(ExtractionPlanError::NumericRange("frame span"))?;
    let selected_count = span
        .checked_div(stride_frames.get())
        .and_then(|count| count.checked_add(1))
        .ok_or(ExtractionPlanError::NumericRange("selected frame count"))?;

    Ok(ExtractionPlan {
        first_frame,
        last_frame,
        stride_frames,
        selected_count,
    })
}

fn ensure_frame_in_range(frame_id: FrameId, frame_count: u64) -> Result<(), ExtractionPlanError> {
    if frame_id.0 < frame_count {
        Ok(())
    } else {
        Err(ExtractionPlanError::FrameOutOfRange {
            requested: frame_id.0,
            frame_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{
        FrameIndexEntry, FrameIndexStreamIdentity, KeyframeAnchor, SourceIdentity,
    };
    use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

    fn temp_root(label: &str) -> PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "framescope-extraction-{label}-{}-{id}",
            std::process::id()
        ))
    }

    fn stream_identity() -> FrameIndexStreamIdentity {
        FrameIndexStreamIdentity {
            stream_index: 0,
            codec_id: 1,
            codec_name: "test".into(),
            time_base: TimeBase::new(1, 1_000).unwrap(),
            width: Some(16),
            height: Some(9),
        }
    }

    fn entry(frame_id: u64, timestamp_ticks: i64) -> FrameIndexEntry {
        let timestamp = MediaTimestamp {
            ticks: timestamp_ticks,
            time_base: TimeBase::new(1, 1_000).unwrap(),
        };
        FrameIndexEntry {
            frame_id: FrameId(frame_id),
            presentation_timestamp: Some(timestamp),
            duration: Some(MediaDuration {
                ticks: 1,
                time_base: timestamp.time_base,
            }),
            keyframe: true,
            corrupt: false,
            anchor: KeyframeAnchor::Keyframe {
                frame_id: FrameId(frame_id),
                presentation_timestamp: Some(timestamp),
            },
        }
    }

    fn open_index(root: &Path) -> FrameIndex {
        let content_id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        FrameIndex::open_or_create(
            root.join("index.sqlite3"),
            SourceIdentity::new(
                123_456,
                None,
                Some(format!("extraction-test-{content_id}")),
            ),
            stream_identity(),
        )
        .unwrap()
        .0
    }

    fn complete_index(root: &Path, ticks: &[i64]) -> FrameIndex {
        let mut index = open_index(root);
        let entries: Vec<_> = ticks
            .iter()
            .enumerate()
            .map(|(frame_id, ticks)| entry(frame_id as u64, *ticks))
            .collect();
        index.append_batch(&entries).unwrap();
        index.mark_complete().unwrap();
        index
    }

    #[test]
    fn all_frames_are_planned_without_materializing_a_selection_vector() {
        let root = temp_root("all");
        let index = complete_index(&root, &[0, 40, 80, 120]);
        let plan = plan_extraction(&index, ExtractionRequest::all_frames()).unwrap();

        assert_eq!(plan.first_frame, FrameId(0));
        assert_eq!(plan.last_frame, FrameId(3));
        assert_eq!(plan.selected_count, 4);
        assert_eq!(
            plan.frame_ids().collect::<Vec<_>>(),
            vec![FrameId(0), FrameId(1), FrameId(2), FrameId(3)]
        );

        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn frame_range_sampling_uses_persistent_frame_ids() {
        let root = temp_root("stride");
        let index = complete_index(&root, &[0, 10, 30, 60, 100, 150, 210]);
        let request = ExtractionRequest {
            selection: ExtractionSelection::FrameRangeInclusive {
                start: FrameId(1),
                end: FrameId(6),
            },
            sampling: ExtractionSampling::EveryNthFrame(NonZeroU64::new(2).unwrap()),
        };
        let plan = plan_extraction(&index, request).unwrap();

        assert_eq!(plan.selected_count, 3);
        assert_eq!(
            plan.frame_ids().collect::<Vec<_>>(),
            vec![FrameId(1), FrameId(3), FrameId(5)]
        );
        assert!(plan.contains(FrameId(3)));
        assert!(!plan.contains(FrameId(4)));

        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn timestamp_range_resolves_via_indexed_pts_not_fps_math() {
        let root = temp_root("timestamps");
        let index = complete_index(&root, &[0, 40, 95, 160, 250]);
        let request = ExtractionRequest {
            selection: ExtractionSelection::TimestampRangeUsInclusive {
                start_us: 50_000,
                end_us: 170_000,
            },
            sampling: ExtractionSampling::EveryFrame,
        };
        let plan = plan_extraction(&index, request).unwrap();

        assert_eq!(plan.first_frame, FrameId(2));
        assert_eq!(plan.last_frame, FrameId(3));
        assert_eq!(plan.frame_ids().collect::<Vec<_>>(), vec![FrameId(2), FrameId(3)]);

        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cancellation_stops_before_the_next_planned_frame() {
        let root = temp_root("cancel");
        let index = complete_index(&root, &[0, 40, 80, 120, 160]);
        let plan = plan_extraction(&index, ExtractionRequest::all_frames()).unwrap();
        let visited = Cell::new(0_u64);

        let result = plan.visit_frame_ids(
            || visited.get() >= 2,
            |progress| {
                assert_eq!(progress.ordinal, visited.get() + 1);
                assert_eq!(progress.total, 5);
                visited.set(progress.ordinal);
                Ok::<(), ()>(())
            },
        );

        assert_eq!(result, Err(ExtractionVisitError::Cancelled));
        assert_eq!(visited.get(), 2);

        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn incomplete_and_empty_indexes_are_not_extractable() {
        let incomplete_root = temp_root("incomplete");
        let incomplete = open_index(&incomplete_root);
        assert!(matches!(
            plan_extraction(&incomplete, ExtractionRequest::all_frames()),
            Err(ExtractionPlanError::IncompleteIndex)
        ));
        drop(incomplete);
        let _ = std::fs::remove_dir_all(incomplete_root);

        let empty_root = temp_root("empty");
        let empty = complete_index(&empty_root, &[]);
        assert!(matches!(
            plan_extraction(&empty, ExtractionRequest::all_frames()),
            Err(ExtractionPlanError::EmptyIndex)
        ));
        drop(empty);
        let _ = std::fs::remove_dir_all(empty_root);
    }

    #[test]
    fn invalid_ranges_and_out_of_bounds_frames_are_rejected() {
        let root = temp_root("invalid");
        let index = complete_index(&root, &[0, 40, 80]);

        let reversed_frames = ExtractionRequest {
            selection: ExtractionSelection::FrameRangeInclusive {
                start: FrameId(2),
                end: FrameId(1),
            },
            sampling: ExtractionSampling::EveryFrame,
        };
        assert!(matches!(
            plan_extraction(&index, reversed_frames),
            Err(ExtractionPlanError::InvalidFrameRange { .. })
        ));

        assert!(matches!(
            plan_extraction(&index, ExtractionRequest::current_frame(FrameId(3))),
            Err(ExtractionPlanError::FrameOutOfRange { .. })
        ));

        let reversed_time = ExtractionRequest {
            selection: ExtractionSelection::TimestampRangeUsInclusive {
                start_us: 100_000,
                end_us: 50_000,
            },
            sampling: ExtractionSampling::EveryFrame,
        };
        assert!(matches!(
            plan_extraction(&index, reversed_time),
            Err(ExtractionPlanError::InvalidTimestampRange { .. })
        ));

        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }
}
