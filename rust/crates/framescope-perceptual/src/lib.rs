//! Deterministic perceptual candidate signals and safe confirmation for frame similarity.
//!
//! Perceptual hashes are never final grouping verdicts. A small Hamming distance means two frames
//! are worth confirming with a stronger pixel-domain metric; it does not mean a literal percentage
//! of pixels changed.

use framescope_cache::OwnedRgbaFrame;
use framescope_similarity::{SimilarityEngine, SimilarityError, SimilarityMode, SimilarityScore};
use thiserror::Error;

pub const PERCEPTUAL_SCALE: u16 = 10_000;
pub const DHASH_BITS: u32 = 64;

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
/// luma metric is allowed to accept a pair as similar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HybridSimilarityPolicy {
    pub max_hash_distance: u8,
    pub minimum_luma_similarity: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HybridDecision {
    RejectedByHash {
        perceptual: PerceptualScore,
    },
    RejectedByLuma {
        perceptual: PerceptualScore,
        luma: SimilarityScore,
    },
    Accepted {
        perceptual: PerceptualScore,
        luma: SimilarityScore,
    },
}

impl HybridDecision {
    pub fn accepted(self) -> bool {
        matches!(self, Self::Accepted { .. })
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
    confirmation: SimilarityEngine,
}

impl HybridSimilarityEngine {
    pub fn new(policy: HybridSimilarityPolicy) -> Result<Self, HybridSimilarityError> {
        if u32::from(policy.max_hash_distance) > DHASH_BITS {
            return Err(HybridSimilarityError::InvalidHashDistance(
                policy.max_hash_distance,
            ));
        }
        let confirmation = SimilarityEngine::new(SimilarityMode::LumaMeanAbsolute {
            minimum_similarity: policy.minimum_luma_similarity,
        })?;
        Ok(Self {
            policy,
            confirmation,
        })
    }

    pub fn policy(&self) -> HybridSimilarityPolicy {
        self.policy
    }

    pub fn compare(
        &self,
        left: &OwnedRgbaFrame,
        right: &OwnedRgbaFrame,
    ) -> Result<HybridDecision, HybridSimilarityError> {
        let perceptual = DHashEngine::compare(left, right);
        if perceptual.hamming_distance > self.policy.max_hash_distance {
            return Ok(HybridDecision::RejectedByHash { perceptual });
        }

        let luma = self.confirmation.compare(left, right)?;
        if self.confirmation.accepts(luma) {
            Ok(HybridDecision::Accepted { perceptual, luma })
        } else {
            Ok(HybridDecision::RejectedByLuma { perceptual, luma })
        }
    }
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
    fn brightness_hash_collision_is_rejected_by_luma_confirmation() {
        let engine = HybridSimilarityEngine::new(HybridSimilarityPolicy {
            max_hash_distance: 4,
            minimum_luma_similarity: 9_900,
        })
        .unwrap();
        let decision = engine.compare(&solid(10), &solid(240)).unwrap();
        assert!(matches!(decision, HybridDecision::RejectedByLuma { .. }));
        assert!(!decision.accepted());
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
    fn resolution_mismatch_is_not_silently_accepted() {
        let engine = HybridSimilarityEngine::new(HybridSimilarityPolicy {
            max_hash_distance: 4,
            minimum_luma_similarity: 9_900,
        })
        .unwrap();
        let small = solid(80);
        let large = frame(18, 16, 72, vec![80; 18 * 16 * 4]);
        assert!(matches!(
            engine.compare(&small, &large),
            Err(HybridSimilarityError::Similarity(
                SimilarityError::DimensionMismatch { .. }
            ))
        ));
    }
}
