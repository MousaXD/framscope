//! Deterministic perceptual candidate signals and safe confirmation for frame similarity.
//!
//! Perceptual hashes are never final grouping verdicts. A small Hamming distance means two frames
//! are worth confirming with a stronger pixel-domain metric; it does not mean a literal percentage
//! of pixels changed.

use framescope_cache::OwnedRgbaFrame;
use framescope_similarity::{SIMILARITY_SCALE, SimilarityError, SimilarityScore};
use thiserror::Error;

pub const PERCEPTUAL_SCALE: u16 = 10_000;
pub const DHASH_BITS: u32 = 64;
/// Persisted derived-group semantics generation for the hybrid metric.
///
/// Bump this whenever hash sampling, confirmation color semantics, or score normalization changes.
pub const HYBRID_SIMILARITY_ALGORITHM_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DHash64(pub u64);

impl DHash64 {
    pub fn hamming_distance(self, other: Self) -> u8 {
        (self.0 ^ other.0).count_ones() as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerceptualScore {
    pub hamming_distance: u8,
    /// 0..=10_000 candidate similarity derived from 64-bit Hamming distance.
    pub basis_points: u16,
}

impl PerceptualScore {
    pub fn from_hashes(left: DHash64, right: DHash64) -> Self {
        let hamming_distance = left.hamming_distance(right);
        let difference = (u32::from(hamming_distance) * u32::from(PERCEPTUAL_SCALE)
            + DHASH_BITS / 2)
            / DHASH_BITS;
        Self {
            hamming_distance,
            basis_points: PERCEPTUAL_SCALE - difference.min(u32::from(PERCEPTUAL_SCALE)) as u16,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DHashEngine;

impl DHashEngine {
    /// Compute a 64-bit horizontal difference hash from a 9x8 deterministic luma sample grid.
    ///
    /// Row padding is ignored. Sampling is resolution-independent and uses integer coordinates to
    /// avoid floating-point platform drift. Because dHash can intentionally collapse brightness
    /// shifts and other detail, callers must treat it as a prefilter/candidate signal only.
    pub fn hash(frame: &OwnedRgbaFrame) -> DHash64 {
        let mut samples = [[0_u16; 9]; 8];
        for (row, values) in samples.iter_mut().enumerate() {
            let y = sample_coordinate(frame.height, row, 8);
            for (column, value) in values.iter_mut().enumerate() {
                let x = sample_coordinate(frame.width, column, 9);
                *value = luma_at(frame, x, y);
            }
        }

        let mut bits = 0_u64;
        let mut bit = 0_u32;
        for row in samples {
            for column in 0..8 {
                if row[column] > row[column + 1] {
                    bits |= 1_u64 << bit;
                }
                bit += 1;
            }
        }
        DHash64(bits)
    }

    pub fn compare(left: &OwnedRgbaFrame, right: &OwnedRgbaFrame) -> PerceptualScore {
        PerceptualScore::from_hashes(Self::hash(left), Self::hash(right))
    }
}

/// Two-stage policy: dHash may reject obvious differences cheaply, but only the full visible-pixel
/// color confirmation is allowed to accept a pair as similar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HybridSimilarityPolicy {
    pub max_hash_distance: u8,
    /// Minimum deterministic RGB confirmation score on the 0..=10_000 scale.
    ///
    /// The legacy field name is retained for source compatibility with the Phase 4 API. Persisted
    /// hybrid results are separately fenced by `HYBRID_SIMILARITY_ALGORITHM_VERSION`.
    pub minimum_luma_similarity: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HybridDecision {
    RejectedByHash {
        perceptual: PerceptualScore,
    },
    RejectedByConfirmation {
        perceptual: PerceptualScore,
        confirmation: SimilarityScore,
    },
    Accepted {
        perceptual: PerceptualScore,
        confirmation: SimilarityScore,
    },
}

impl HybridDecision {
    pub fn accepted(self) -> bool {
        matches!(self, Self::Accepted { .. })
    }
}

/// Diagnostic result that deliberately runs the stronger confirmation even when the production
/// dHash gate rejects the pair. It is for quality measurement, not a second acceptance path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HybridAnalysis {
    pub decision: HybridDecision,
    pub confirmation: SimilarityScore,
    pub hash_rejected_but_confirmation_accepted: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SimilarityQualityCounters {
    pub compared_pairs: u64,
    pub true_positive: u64,
    pub false_positive: u64,
    pub true_negative: u64,
    pub false_negative: u64,
    pub rejected_by_hash: u64,
    pub hash_reject_confirmation_accepts: u64,
}

impl SimilarityQualityCounters {
    pub fn record(&mut self, expected_similar: bool, analysis: HybridAnalysis) {
        self.compared_pairs = self.compared_pairs.saturating_add(1);
        let accepted = analysis.decision.accepted();
        match (expected_similar, accepted) {
            (true, true) => self.true_positive = self.true_positive.saturating_add(1),
            (false, true) => self.false_positive = self.false_positive.saturating_add(1),
            (false, false) => self.true_negative = self.true_negative.saturating_add(1),
            (true, false) => self.false_negative = self.false_negative.saturating_add(1),
        }
        if matches!(analysis.decision, HybridDecision::RejectedByHash { .. }) {
            self.rejected_by_hash = self.rejected_by_hash.saturating_add(1);
        }
        if analysis.hash_rejected_but_confirmation_accepted {
            self.hash_reject_confirmation_accepts =
                self.hash_reject_confirmation_accepts.saturating_add(1);
        }
    }

    pub fn precision_basis_points(self) -> Option<u16> {
        ratio_basis_points(self.true_positive, self.true_positive + self.false_positive)
    }

    pub fn recall_basis_points(self) -> Option<u16> {
        ratio_basis_points(self.true_positive, self.true_positive + self.false_negative)
    }

    pub fn f1_basis_points(self) -> Option<u16> {
        let denominator = self
            .true_positive
            .saturating_mul(2)
            .saturating_add(self.false_positive)
            .saturating_add(self.false_negative);
        ratio_basis_points(self.true_positive.saturating_mul(2), denominator)
    }
}

#[derive(Debug, Error)]
pub enum HybridSimilarityError {
    #[error("dHash distance threshold {0} exceeds {DHASH_BITS} bits")]
    InvalidHashDistance(u8),
    #[error(transparent)]
    Similarity(#[from] SimilarityError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HybridSimilarityEngine {
    policy: HybridSimilarityPolicy,
}

impl HybridSimilarityEngine {
    pub fn new(policy: HybridSimilarityPolicy) -> Result<Self, HybridSimilarityError> {
        if u32::from(policy.max_hash_distance) > DHASH_BITS {
            return Err(HybridSimilarityError::InvalidHashDistance(
                policy.max_hash_distance,
            ));
        }
        if policy.minimum_luma_similarity > SIMILARITY_SCALE {
            return Err(SimilarityError::InvalidThreshold(policy.minimum_luma_similarity).into());
        }
        Ok(Self { policy })
    }

    pub fn policy(&self) -> HybridSimilarityPolicy {
        self.policy
    }

    pub fn compare(
        &self,
        left: &OwnedRgbaFrame,
        right: &OwnedRgbaFrame,
    ) -> Result<HybridDecision, HybridSimilarityError> {
        validate_dimensions(left, right)?;
        let perceptual = DHashEngine::compare(left, right);
        if perceptual.hamming_distance > self.policy.max_hash_distance {
            return Ok(HybridDecision::RejectedByHash { perceptual });
        }
        let confirmation = rgb_mean_absolute_similarity(left, right)?;
        if confirmation.basis_points >= self.policy.minimum_luma_similarity {
            Ok(HybridDecision::Accepted {
                perceptual,
                confirmation,
            })
        } else {
            Ok(HybridDecision::RejectedByConfirmation {
                perceptual,
                confirmation,
            })
        }
    }

    /// Run full confirmation even for a dHash reject so corpus/benchmark code can quantify the
    /// hard-gate recall cost without changing the production decision.
    pub fn analyze(
        &self,
        left: &OwnedRgbaFrame,
        right: &OwnedRgbaFrame,
    ) -> Result<HybridAnalysis, HybridSimilarityError> {
        validate_dimensions(left, right)?;
        let perceptual = DHashEngine::compare(left, right);
        let confirmation = rgb_mean_absolute_similarity(left, right)?;
        let confirmation_accepted =
            confirmation.basis_points >= self.policy.minimum_luma_similarity;
        let decision = if perceptual.hamming_distance > self.policy.max_hash_distance {
            HybridDecision::RejectedByHash { perceptual }
        } else if confirmation_accepted {
            HybridDecision::Accepted {
                perceptual,
                confirmation,
            }
        } else {
            HybridDecision::RejectedByConfirmation {
                perceptual,
                confirmation,
            }
        };
        Ok(HybridAnalysis {
            decision,
            confirmation,
            hash_rejected_but_confirmation_accepted: matches!(
                decision,
                HybridDecision::RejectedByHash { .. }
            ) && confirmation_accepted,
        })
    }
}

fn validate_dimensions(
    left: &OwnedRgbaFrame,
    right: &OwnedRgbaFrame,
) -> Result<(), SimilarityError> {
    if (left.width, left.height) != (right.width, right.height) {
        return Err(SimilarityError::DimensionMismatch {
            left: (left.width, left.height),
            right: (right.width, right.height),
        });
    }
    Ok(())
}

fn rgb_mean_absolute_similarity(
    left: &OwnedRgbaFrame,
    right: &OwnedRgbaFrame,
) -> Result<SimilarityScore, SimilarityError> {
    let width_bytes = usize::try_from(left.width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or(SimilarityError::FrameLayoutOverflow)?;
    let height = usize::try_from(left.height).map_err(|_| SimilarityError::FrameLayoutOverflow)?;

    let visible_pixels_identical = (0..height).all(|y| {
        let left_start = y * left.stride_bytes;
        let right_start = y * right.stride_bytes;
        left.pixels()[left_start..left_start + width_bytes]
            == right.pixels()[right_start..right_start + width_bytes]
    });
    if visible_pixels_identical {
        return Ok(SimilarityScore::IDENTICAL);
    }

    let mut total_difference: u128 = 0;
    for y in 0..height {
        let left_start = y * left.stride_bytes;
        let right_start = y * right.stride_bytes;
        let left_row = &left.pixels()[left_start..left_start + width_bytes];
        let right_row = &right.pixels()[right_start..right_start + width_bytes];
        for (left_px, right_px) in left_row.chunks_exact(4).zip(right_row.chunks_exact(4)) {
            total_difference += u128::from(left_px[0].abs_diff(right_px[0]));
            total_difference += u128::from(left_px[1].abs_diff(right_px[1]));
            total_difference += u128::from(left_px[2].abs_diff(right_px[2]));
        }
    }

    let pixel_count = u128::from(left.width) * u128::from(left.height);
    let max_difference = pixel_count * 3 * 255;
    let difference_bps = if max_difference == 0 {
        0
    } else {
        ((total_difference * u128::from(SIMILARITY_SCALE) + max_difference / 2) / max_difference)
            .min(u128::from(SIMILARITY_SCALE)) as u16
    };
    Ok(SimilarityScore {
        basis_points: SIMILARITY_SCALE - difference_bps,
        exact: false,
    })
}

fn ratio_basis_points(numerator: u64, denominator: u64) -> Option<u16> {
    if denominator == 0 {
        return None;
    }
    let scaled = (u128::from(numerator) * u128::from(SIMILARITY_SCALE)
        + u128::from(denominator) / 2)
        / u128::from(denominator);
    Some(scaled.min(u128::from(SIMILARITY_SCALE)) as u16)
}

fn sample_coordinate(size: u32, index: usize, sample_count: usize) -> usize {
    debug_assert!(size > 0);
    debug_assert!(sample_count > 1);
    let last = u64::from(size - 1);
    let numerator = last * index as u64;
    let denominator = (sample_count - 1) as u64;
    usize::try_from(numerator / denominator).unwrap_or(0)
}

fn luma_at(frame: &OwnedRgbaFrame, x: usize, y: usize) -> u16 {
    let offset = y * frame.stride_bytes + x * 4;
    let pixels = frame.pixels();
    integer_luma(pixels[offset], pixels[offset + 1], pixels[offset + 2])
}

fn integer_luma(r: u8, g: u8, b: u8) -> u16 {
    ((u32::from(r) * 77 + u32::from(g) * 150 + u32::from(b) * 29 + 128) >> 8) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, stride: usize, pixels: Vec<u8>) -> OwnedRgbaFrame {
        OwnedRgbaFrame::new(width, height, stride, pixels).unwrap()
    }

    fn solid(value: u8) -> OwnedRgbaFrame {
        frame(9, 8, 36, vec![value; 9 * 8 * 4])
    }

    fn gradients() -> (OwnedRgbaFrame, OwnedRgbaFrame) {
        let mut left = vec![0_u8; 9 * 8 * 4];
        let mut right = vec![0_u8; 9 * 8 * 4];
        for y in 0..8 {
            for x in 0..9 {
                let left_value = (x * 20) as u8;
                let right_value = ((8 - x) * 20) as u8;
                let offset = (y * 9 + x) * 4;
                left[offset..offset + 4]
                    .copy_from_slice(&[left_value, left_value, left_value, 255]);
                right[offset..offset + 4].copy_from_slice(&[
                    right_value,
                    right_value,
                    right_value,
                    255,
                ]);
            }
        }
        (frame(9, 8, 36, left), frame(9, 8, 36, right))
    }

    #[test]
    fn identical_frames_have_zero_distance() {
        let left = solid(40);
        let right = solid(40);
        let score = DHashEngine::compare(&left, &right);
        assert_eq!(score.hamming_distance, 0);
        assert_eq!(score.basis_points, PERCEPTUAL_SCALE);
    }

    #[test]
    fn uniform_brightness_shift_collapses_and_must_be_confirmed_elsewhere() {
        let dark = solid(10);
        let bright = solid(240);
        assert_eq!(DHashEngine::hash(&dark), DHashEngine::hash(&bright));
    }

    #[test]
    fn horizontal_structure_changes_hash() {
        let (left, right) = gradients();
        let score = DHashEngine::compare(&left, &right);
        assert!(score.hamming_distance > 0);
        assert!(score.basis_points < PERCEPTUAL_SCALE);
    }

    #[test]
    fn row_padding_does_not_affect_hash() {
        let visible = [10, 20, 30, 255];
        let mut compact = Vec::new();
        let mut padded = Vec::new();
        for _ in 0..8 {
            for _ in 0..9 {
                compact.extend_from_slice(&visible);
                padded.extend_from_slice(&visible);
            }
            padded.extend_from_slice(&[1, 2, 3, 4]);
        }
        let compact = frame(9, 8, 36, compact);
        let padded = frame(9, 8, 40, padded);
        assert_eq!(DHashEngine::hash(&compact), DHashEngine::hash(&padded));
    }

    #[test]
    fn resolution_changes_can_preserve_hash_candidate() {
        let small = solid(80);
        let large = frame(18, 16, 72, vec![80; 18 * 16 * 4]);
        assert_eq!(DHashEngine::hash(&small), DHashEngine::hash(&large));
    }

    #[test]
    fn invalid_hash_threshold_is_rejected() {
        let error = HybridSimilarityEngine::new(HybridSimilarityPolicy {
            max_hash_distance: 65,
            minimum_luma_similarity: 9_900,
        })
        .unwrap_err();
        assert!(matches!(
            error,
            HybridSimilarityError::InvalidHashDistance(65)
        ));
    }

    #[test]
    fn brightness_hash_collision_is_rejected_by_color_confirmation() {
        let engine = HybridSimilarityEngine::new(HybridSimilarityPolicy {
            max_hash_distance: 4,
            minimum_luma_similarity: 9_900,
        })
        .unwrap();
        let decision = engine.compare(&solid(10), &solid(240)).unwrap();
        assert!(matches!(
            decision,
            HybridDecision::RejectedByConfirmation { .. }
        ));
        assert!(!decision.accepted());
    }

    #[test]
    fn equal_luma_different_hue_is_rejected_by_color_confirmation() {
        let engine = HybridSimilarityEngine::new(HybridSimilarityPolicy {
            max_hash_distance: 8,
            minimum_luma_similarity: 9_700,
        })
        .unwrap();
        let red = frame(9, 8, 36, [255_u8, 0, 0, 255].repeat(9 * 8));
        let green = frame(9, 8, 36, [0_u8, 131, 0, 255].repeat(9 * 8));
        assert_eq!(DHashEngine::hash(&red), DHashEngine::hash(&green));
        assert_eq!(integer_luma(255, 0, 0), integer_luma(0, 131, 0));
        let analysis = engine.analyze(&red, &green).unwrap();
        assert!(matches!(
            analysis.decision,
            HybridDecision::RejectedByConfirmation { .. }
        ));
        assert!(analysis.confirmation.basis_points < 9_700);
    }

    #[test]
    fn alpha_only_difference_has_explicit_rgb_confirmation_policy() {
        let engine = HybridSimilarityEngine::new(HybridSimilarityPolicy {
            max_hash_distance: 8,
            minimum_luma_similarity: 9_700,
        })
        .unwrap();
        let opaque = frame(9, 8, 36, [40_u8, 80, 120, 255].repeat(9 * 8));
        let translucent = frame(9, 8, 36, [40_u8, 80, 120, 64].repeat(9 * 8));
        let analysis = engine.analyze(&opaque, &translucent).unwrap();
        assert!(!analysis.confirmation.exact);
        assert_eq!(analysis.confirmation.basis_points, SIMILARITY_SCALE);
        assert!(analysis.decision.accepted());
    }

    #[test]
    fn hard_dhash_gate_false_negative_is_measurable_without_policy_tuning() {
        let engine = HybridSimilarityEngine::new(HybridSimilarityPolicy {
            max_hash_distance: 8,
            minimum_luma_similarity: 9_700,
        })
        .unwrap();
        let mut left = Vec::with_capacity(9 * 8 * 4);
        let mut right = Vec::with_capacity(9 * 8 * 4);
        for _ in 0..8 {
            for x in 0..9 {
                let a = if x % 2 == 0 { 101_u8 } else { 100_u8 };
                let b = if x % 2 == 0 { 100_u8 } else { 101_u8 };
                left.extend_from_slice(&[a, a, a, 255]);
                right.extend_from_slice(&[b, b, b, 255]);
            }
        }
        let analysis = engine
            .analyze(&frame(9, 8, 36, left), &frame(9, 8, 36, right))
            .unwrap();
        assert!(matches!(
            analysis.decision,
            HybridDecision::RejectedByHash { .. }
        ));
        assert!(analysis.confirmation.basis_points >= 9_700);
        assert!(analysis.hash_rejected_but_confirmation_accepted);

        let mut counters = SimilarityQualityCounters::default();
        counters.record(true, analysis);
        assert_eq!(counters.false_negative, 1);
        assert_eq!(counters.rejected_by_hash, 1);
        assert_eq!(counters.hash_reject_confirmation_accepts, 1);
        assert_eq!(counters.recall_basis_points(), Some(0));
    }

    #[test]
    fn obvious_structure_change_is_rejected_before_full_confirmation() {
        let engine = HybridSimilarityEngine::new(HybridSimilarityPolicy {
            max_hash_distance: 8,
            minimum_luma_similarity: 9_900,
        })
        .unwrap();
        let (left, right) = gradients();
        let decision = engine.compare(&left, &right).unwrap();
        assert!(matches!(decision, HybridDecision::RejectedByHash { .. }));
    }

    #[test]
    fn small_luma_change_can_be_confirmed() {
        let engine = HybridSimilarityEngine::new(HybridSimilarityPolicy {
            max_hash_distance: 4,
            minimum_luma_similarity: 9_900,
        })
        .unwrap();
        let decision = engine.compare(&solid(100), &solid(101)).unwrap();
        assert!(decision.accepted());
        assert!(matches!(decision, HybridDecision::Accepted { .. }));
    }

    #[test]
    fn resolution_mismatch_is_rejected_before_hash_short_circuit() {
        let engine = HybridSimilarityEngine::new(HybridSimilarityPolicy {
            max_hash_distance: 0,
            minimum_luma_similarity: 9_900,
        })
        .unwrap();
        let (small, _) = gradients();
        let mut pixels = vec![0_u8; 18 * 16 * 4];
        for y in 0..16 {
            for x in 0..18 {
                let value = ((17 - x) * 12) as u8;
                let offset = (y * 18 + x) * 4;
                pixels[offset..offset + 4].copy_from_slice(&[value, value, value, 255]);
            }
        }
        let large = frame(18, 16, 72, pixels);
        assert!(DHashEngine::compare(&small, &large).hamming_distance > 0);
        assert!(matches!(
            engine.compare(&small, &large),
            Err(HybridSimilarityError::Similarity(
                SimilarityError::DimensionMismatch { .. }
            ))
        ));
    }
}
