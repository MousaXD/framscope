//! Streaming frame-similarity and adjacent-frame grouping primitives.
//!
//! This crate deliberately keeps the authoritative Phase 3 frame index intact. Groups are a
//! derived view over adjacent presentation frames; they never delete or renumber timeline entries.

use framescope_cache::{FrameId, FrameIndexEntry, OwnedRgbaFrame};

pub const SIMILARITY_SCALE: u16 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimilarityMode {
    /// Only byte-identical RGBA frames are grouped.
    Exact,
    /// Compare mean absolute luma difference after an exact-byte fast path.
    ///
    /// `minimum_similarity` is expressed on a 0..=10_000 scale where 10_000 means identical.
    LumaMeanAbsolute { minimum_similarity: u16 },
}

impl SimilarityMode {
    pub fn validate(self) -> Result<Self, SimilarityError> {
        match self {
            Self::Exact => Ok(self),
            Self::LumaMeanAbsolute { minimum_similarity }
                if minimum_similarity <= SIMILARITY_SCALE =>
            {
                Ok(self)
            }
            Self::LumaMeanAbsolute { minimum_similarity } => {
                Err(SimilarityError::InvalidThreshold(minimum_similarity))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimilarityScore {
    /// 0..=10_000, with 10_000 meaning identical under the selected metric.
    pub basis_points: u16,
    pub exact: bool,
}

impl SimilarityScore {
    pub const IDENTICAL: Self = Self {
        basis_points: SIMILARITY_SCALE,
        exact: true,
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimilarityError {
    InvalidThreshold(u16),
    DimensionMismatch { left: (u32, u32), right: (u32, u32) },
    MissingTimestamp(FrameId),
    NonContiguousFrameIds { previous: FrameId, current: FrameId },
}

impl std::fmt::Display for SimilarityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidThreshold(value) => write!(
                f,
                "similarity threshold {value} is outside 0..={SIMILARITY_SCALE}"
            ),
            Self::DimensionMismatch { left, right } => {
                write!(f, "frame dimensions differ: {left:?} vs {right:?}")
            }
            Self::MissingTimestamp(frame_id) => {
                write!(f, "frame {} has no presentation timestamp", frame_id.0)
            }
            Self::NonContiguousFrameIds { previous, current } => write!(
                f,
                "frame IDs are not contiguous: {} followed by {}",
                previous.0, current.0
            ),
        }
    }
}

impl std::error::Error for SimilarityError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimilarityEngine {
    mode: SimilarityMode,
}

impl SimilarityEngine {
    pub fn new(mode: SimilarityMode) -> Result<Self, SimilarityError> {
        Ok(Self {
            mode: mode.validate()?,
        })
    }

    pub fn mode(&self) -> SimilarityMode {
        self.mode
    }

    pub fn compare(
        &self,
        left: &OwnedRgbaFrame,
        right: &OwnedRgbaFrame,
    ) -> Result<SimilarityScore, SimilarityError> {
        if (left.width, left.height) != (right.width, right.height) {
            return Err(SimilarityError::DimensionMismatch {
                left: (left.width, left.height),
                right: (right.width, right.height),
            });
        }

        if left.stride_bytes == right.stride_bytes && left.pixels() == right.pixels() {
            return Ok(SimilarityScore::IDENTICAL);
        }

        match self.mode {
            SimilarityMode::Exact => Ok(SimilarityScore {
                basis_points: 0,
                exact: false,
            }),
            SimilarityMode::LumaMeanAbsolute { .. } => {
                let width = usize::try_from(left.width).expect("u32 width always fits usize");
                let height = usize::try_from(left.height).expect("u32 height always fits usize");
                let mut total_difference: u128 = 0;
                let pixel_count = u128::from(left.width) * u128::from(left.height);

                for y in 0..height {
                    let left_row =
                        &left.pixels()[y * left.stride_bytes..y * left.stride_bytes + width * 4];
                    let right_row =
                        &right.pixels()[y * right.stride_bytes..y * right.stride_bytes + width * 4];
                    for (left_px, right_px) in
                        left_row.chunks_exact(4).zip(right_row.chunks_exact(4))
                    {
                        let left_luma = integer_luma(left_px[0], left_px[1], left_px[2]);
                        let right_luma = integer_luma(right_px[0], right_px[1], right_px[2]);
                        total_difference += u128::from(left_luma.abs_diff(right_luma));
                    }
                }

                let max_difference = pixel_count * 255;
                let difference_bps = if max_difference == 0 {
                    0
                } else {
                    ((total_difference * u128::from(SIMILARITY_SCALE) + max_difference / 2)
                        / max_difference)
                        .min(u128::from(SIMILARITY_SCALE)) as u16
                };
                Ok(SimilarityScore {
                    basis_points: SIMILARITY_SCALE - difference_bps,
                    exact: false,
                })
            }
        }
    }

    pub fn accepts(&self, score: SimilarityScore) -> bool {
        match self.mode {
            SimilarityMode::Exact => score.exact,
            SimilarityMode::LumaMeanAbsolute { minimum_similarity } => {
                score.basis_points >= minimum_similarity
            }
        }
    }
}

fn integer_luma(r: u8, g: u8, b: u8) -> u16 {
    // BT.601-style integer luma approximation. The exact weights are part of this metric's
    // semantics and intentionally avoid floating-point platform drift.
    ((u32::from(r) * 77 + u32::from(g) * 150 + u32::from(b) * 29 + 128) >> 8) as u16
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameGroup {
    pub representative_frame: FrameId,
    pub first_frame: FrameId,
    pub last_frame: FrameId,
    pub frame_count: u64,
    pub start_timestamp_ticks: i64,
    pub end_timestamp_ticks: i64,
    /// Lowest candidate-to-representative similarity observed within this group.
    pub representative_similarity_floor: u16,
}

#[derive(Debug, Clone)]
struct ActiveGroup {
    metadata: FrameGroup,
    representative: OwnedRgbaFrame,
    previous: OwnedRgbaFrame,
}

/// Bounded streaming adjacent-frame grouper.
///
/// To prevent transitive chain drift, a candidate joins the current group only when it satisfies
/// the selected threshold against both the immediately previous frame and the canonical group
/// representative. Memory usage is therefore bounded to two owned frames plus group metadata.
#[derive(Debug, Clone)]
pub struct FrameGrouper {
    engine: SimilarityEngine,
    active: Option<ActiveGroup>,
}

impl FrameGrouper {
    pub fn new(mode: SimilarityMode) -> Result<Self, SimilarityError> {
        Ok(Self {
            engine: SimilarityEngine::new(mode)?,
            active: None,
        })
    }

    pub fn push(
        &mut self,
        entry: &FrameIndexEntry,
        pixels: OwnedRgbaFrame,
    ) -> Result<Option<FrameGroup>, SimilarityError> {
        let timestamp = entry
            .presentation_timestamp
            .ok_or(SimilarityError::MissingTimestamp(entry.frame_id))?;

        let Some(active) = self.active.as_mut() else {
            self.active = Some(ActiveGroup {
                metadata: FrameGroup {
                    representative_frame: entry.frame_id,
                    first_frame: entry.frame_id,
                    last_frame: entry.frame_id,
                    frame_count: 1,
                    start_timestamp_ticks: timestamp.ticks,
                    end_timestamp_ticks: timestamp.ticks,
                    representative_similarity_floor: SIMILARITY_SCALE,
                },
                representative: pixels.clone(),
                previous: pixels,
            });
            return Ok(None);
        };

        let expected = active.metadata.last_frame.0.checked_add(1);
        if expected != Some(entry.frame_id.0) {
            return Err(SimilarityError::NonContiguousFrameIds {
                previous: active.metadata.last_frame,
                current: entry.frame_id,
            });
        }

        let previous_score = self.engine.compare(&active.previous, &pixels)?;
        let representative_score = self.engine.compare(&active.representative, &pixels)?;
        if self.engine.accepts(previous_score) && self.engine.accepts(representative_score) {
            active.metadata.last_frame = entry.frame_id;
            active.metadata.frame_count += 1;
            active.metadata.end_timestamp_ticks = timestamp.ticks;
            active.metadata.representative_similarity_floor = active
                .metadata
                .representative_similarity_floor
                .min(representative_score.basis_points);
            active.previous = pixels;
            return Ok(None);
        }

        let completed = active.metadata.clone();
        self.active = Some(ActiveGroup {
            metadata: FrameGroup {
                representative_frame: entry.frame_id,
                first_frame: entry.frame_id,
                last_frame: entry.frame_id,
                frame_count: 1,
                start_timestamp_ticks: timestamp.ticks,
                end_timestamp_ticks: timestamp.ticks,
                representative_similarity_floor: SIMILARITY_SCALE,
            },
            representative: pixels.clone(),
            previous: pixels,
        });
        Ok(Some(completed))
    }

    pub fn finish(&mut self) -> Option<FrameGroup> {
        self.active.take().map(|active| active.metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::KeyframeAnchor;
    use framescope_core::{MediaTimestamp, TimeBase};

    fn rgba(value: u8) -> OwnedRgbaFrame {
        OwnedRgbaFrame::new(
            2,
            1,
            8,
            vec![value, value, value, 255, value, value, value, 255],
        )
        .unwrap()
    }

    fn entry(id: u64, ticks: i64) -> FrameIndexEntry {
        FrameIndexEntry {
            frame_id: FrameId(id),
            presentation_timestamp: Some(MediaTimestamp {
                ticks,
                time_base: TimeBase::new(1, 1000).unwrap(),
            }),
            duration: None,
            keyframe: id == 0,
            corrupt: false,
            anchor: if id == 0 {
                KeyframeAnchor::Keyframe {
                    frame_id: FrameId(0),
                    presentation_timestamp: Some(MediaTimestamp {
                        ticks,
                        time_base: TimeBase::new(1, 1000).unwrap(),
                    }),
                }
            } else {
                KeyframeAnchor::Keyframe {
                    frame_id: FrameId(0),
                    presentation_timestamp: Some(MediaTimestamp {
                        ticks: 0,
                        time_base: TimeBase::new(1, 1000).unwrap(),
                    }),
                }
            },
        }
    }

    #[test]
    fn exact_mode_only_accepts_identical_pixels() {
        let engine = SimilarityEngine::new(SimilarityMode::Exact).unwrap();
        assert!(engine.accepts(engine.compare(&rgba(50), &rgba(50)).unwrap()));
        assert!(!engine.accepts(engine.compare(&rgba(50), &rgba(51)).unwrap()));
    }

    #[test]
    fn luma_score_has_documented_scale() {
        let engine = SimilarityEngine::new(SimilarityMode::LumaMeanAbsolute {
            minimum_similarity: 9_000,
        })
        .unwrap();
        let identical = engine.compare(&rgba(100), &rgba(100)).unwrap();
        let opposite = engine.compare(&rgba(0), &rgba(255)).unwrap();
        assert_eq!(identical.basis_points, 10_000);
        assert_eq!(opposite.basis_points, 0);
    }

    #[test]
    fn grouping_preserves_frame_and_timestamp_ranges() {
        let mut grouper = FrameGrouper::new(SimilarityMode::Exact).unwrap();
        assert!(grouper.push(&entry(0, 0), rgba(10)).unwrap().is_none());
        assert!(grouper.push(&entry(1, 40), rgba(10)).unwrap().is_none());
        let first = grouper.push(&entry(2, 125), rgba(80)).unwrap().unwrap();
        assert_eq!(first.first_frame, FrameId(0));
        assert_eq!(first.last_frame, FrameId(1));
        assert_eq!(first.frame_count, 2);
        assert_eq!(first.start_timestamp_ticks, 0);
        assert_eq!(first.end_timestamp_ticks, 40);
        let second = grouper.finish().unwrap();
        assert_eq!(second.representative_frame, FrameId(2));
        assert_eq!(second.start_timestamp_ticks, 125);
    }

    #[test]
    fn representative_check_prevents_transitive_chain_drift() {
        let mut grouper = FrameGrouper::new(SimilarityMode::LumaMeanAbsolute {
            minimum_similarity: 9_600,
        })
        .unwrap();

        // 0 -> 5 and 5 -> 10 are each close enough, but 0 -> 10 crosses the threshold.
        assert!(grouper.push(&entry(0, 0), rgba(0)).unwrap().is_none());
        assert!(grouper.push(&entry(1, 40), rgba(5)).unwrap().is_none());
        let completed = grouper.push(&entry(2, 80), rgba(10)).unwrap().unwrap();
        assert_eq!(completed.first_frame, FrameId(0));
        assert_eq!(completed.last_frame, FrameId(1));
        assert_eq!(completed.frame_count, 2);
    }

    #[test]
    fn rejects_non_contiguous_timeline_input() {
        let mut grouper = FrameGrouper::new(SimilarityMode::Exact).unwrap();
        grouper.push(&entry(0, 0), rgba(1)).unwrap();
        assert!(matches!(
            grouper.push(&entry(2, 80), rgba(1)),
            Err(SimilarityError::NonContiguousFrameIds { .. })
        ));
    }
}
