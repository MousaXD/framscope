from pathlib import Path
import re


def load(path: str) -> tuple[Path, str]:
    p = Path(path)
    return p, p.read_text()


def require_replace(text: str, old: str, new: str, *, count: int = 1, label: str) -> str:
    actual = text.count(old)
    if actual != count:
        raise SystemExit(f"{label}: expected {count} occurrences, found {actual}")
    return text.replace(old, new, count)


def require_regex(text: str, pattern: str, replacement: str, *, label: str) -> str:
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.S)
    if count != 1:
        raise SystemExit(f"{label}: expected one regex match, found {count}")
    return updated


# framescope-perceptual: keep dHash as the production candidate gate, replace luma-only final
# confirmation with deterministic RGB mean absolute difference, and expose a diagnostic path that
# measures gate false negatives without changing production acceptance policy.
p, text = load("rust/crates/framescope-perceptual/src/lib.rs")
text = require_replace(
    text,
    "use framescope_similarity::{SimilarityEngine, SimilarityError, SimilarityMode, SimilarityScore};",
    "use framescope_similarity::{SIMILARITY_SCALE, SimilarityError, SimilarityScore};",
    label="perceptual import",
)
text = require_replace(
    text,
    "pub const DHASH_BITS: u32 = 64;\n",
    "pub const DHASH_BITS: u32 = 64;\n"
    "/// Persisted derived-group semantics generation for the hybrid metric.\n"
    "///\n"
    "/// Bump this whenever hash sampling, confirmation color semantics, or score normalization changes.\n"
    "pub const HYBRID_SIMILARITY_ALGORITHM_VERSION: u32 = 2;\n",
    label="algorithm version",
)
replacement = r'''/// Two-stage policy: dHash may reject obvious differences cheaply, but only the full visible-pixel
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
        let confirmation_accepted = confirmation.basis_points >= self.policy.minimum_luma_similarity;
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
        ((total_difference * u128::from(SIMILARITY_SCALE) + max_difference / 2)
            / max_difference)
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

'''
text = require_regex(
    text,
    r"/// Two-stage policy:.*?(?=fn sample_coordinate)",
    replacement,
    label="perceptual policy and engine",
)
text = require_replace(
    text,
    "fn brightness_hash_collision_is_rejected_by_luma_confirmation()",
    "fn brightness_hash_collision_is_rejected_by_color_confirmation()",
    label="brightness test name",
)
text = require_replace(
    text,
    "assert!(matches!(decision, HybridDecision::RejectedByLuma { .. }));",
    "assert!(matches!(\n            decision,\n            HybridDecision::RejectedByConfirmation { .. }\n        ));",
    label="brightness decision assertion",
)
marker = "    #[test]\n    fn obvious_structure_change_is_rejected_before_full_confirmation() {"
insert = r'''    #[test]
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
        assert!(matches!(analysis.decision, HybridDecision::RejectedByHash { .. }));
        assert!(analysis.confirmation.basis_points >= 9_700);
        assert!(analysis.hash_rejected_but_confirmation_accepted);

        let mut counters = SimilarityQualityCounters::default();
        counters.record(true, analysis);
        assert_eq!(counters.false_negative, 1);
        assert_eq!(counters.rejected_by_hash, 1);
        assert_eq!(counters.hash_reject_confirmation_accepts, 1);
        assert_eq!(counters.recall_basis_points(), Some(0));
    }

'''
text = require_replace(text, marker, insert + marker, label="perceptual tests")
p.write_text(text)


# framescope-grouping: consume the renamed confirmation decision, keep anti-chain drift, and make
# the consecutive-only product contract explicit with A-B-A and hue regressions.
p, text = load("rust/crates/framescope-grouping/src/lib.rs")
text = require_replace(
    text,
    "/// Bounded streaming grouper using dHash as a rejection prefilter and full luma confirmation.",
    "/// Bounded streaming grouper using dHash as a rejection prefilter and full color confirmation.",
    label="grouping docs",
)
text = require_replace(text, "accepted_luma(previous)", "accepted_confirmation(previous)", label="previous helper")
text = require_replace(
    text,
    "accepted_luma(representative)",
    "accepted_confirmation(representative)",
    label="representative helper",
)
text = require_replace(text, "representative_luma.basis_points", "representative_confirmation.basis_points", label="score variable")
text = require_replace(text, "let Some(representative_luma) =", "let Some(representative_confirmation) =", label="representative binding")
text = require_regex(
    text,
    r"fn accepted_luma\(decision: HybridDecision\) -> Option<SimilarityScore> \{.*?\n\}\n",
    r'''fn accepted_confirmation(decision: HybridDecision) -> Option<SimilarityScore> {
    match decision {
        HybridDecision::Accepted { confirmation, .. } => Some(confirmation),
        HybridDecision::RejectedByHash { .. }
        | HybridDecision::RejectedByConfirmation { .. } => None,
    }
}
''',
    label="accepted confirmation helper",
)
marker = "    #[test]\n    fn perceptual_rejection_creates_boundary_without_deleting_frames() {"
insert = r'''    fn solid_rgb(r: u8, g: u8, b: u8) -> OwnedRgbaFrame {
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
        let first = grouper.push(&entry(1, 40, 40), solid(220)).unwrap().unwrap();
        let second = grouper.push(&entry(2, 80, 40), solid(10)).unwrap().unwrap();
        let third = grouper.finish().unwrap();
        assert_eq!((first.first_frame.0, first.last_frame.0), (0, 0));
        assert_eq!((second.first_frame.0, second.last_frame.0), (1, 1));
        assert_eq!((third.first_frame.0, third.last_frame.0), (2, 2));
    }

'''
text = require_replace(text, marker, insert + marker, label="grouping tests")
p.write_text(text)


# framescope-similarity-store: bump derived-store generation for changed metric semantics and add
# an authoritative frame-count-aware validator for complete, contiguous, non-overlapping coverage.
p, text = load("rust/crates/framescope-similarity-store/src/lib.rs")
text = require_replace(
    text,
    "use framescope_perceptual::{HybridSimilarityEngine, HybridSimilarityPolicy};",
    "use framescope_perceptual::{\n    HYBRID_SIMILARITY_ALGORITHM_VERSION, HybridSimilarityEngine, HybridSimilarityPolicy,\n};",
    label="store perceptual import",
)
text = require_replace(text, "pub const SIMILARITY_STORE_SCHEMA_VERSION: u32 = 3;", "pub const SIMILARITY_STORE_SCHEMA_VERSION: u32 = 4;", label="store schema version")
text = require_replace(
    text,
    '            }) => format!("hybrid-h{max_hash_distance}-l{minimum_luma_similarity}"),',
    '            }) => format!(\n                "hybrid-a{HYBRID_SIMILARITY_ALGORITHM_VERSION}-h{max_hash_distance}-l{minimum_luma_similarity}"\n            ),',
    label="hybrid filename",
)
text = require_replace(
    text,
    "    Hybrid {\n        max_hash_distance: u8,\n        minimum_luma_similarity: u16,\n    },",
    "    Hybrid {\n        algorithm_version: u32,\n        max_hash_distance: u8,\n        minimum_luma_similarity: u16,\n    },",
    label="persisted hybrid config",
)
text = require_replace(
    text,
    "            }) => Self::Hybrid {\n                max_hash_distance,\n                minimum_luma_similarity,\n            },",
    "            }) => Self::Hybrid {\n                algorithm_version: HYBRID_SIMILARITY_ALGORITHM_VERSION,\n                max_hash_distance,\n                minimum_luma_similarity,\n            },",
    label="persisted hybrid conversion",
)
old = '''    pub fn visit_groups<F>(
        &self,
        expected: &SimilarityStoreKey,
        mut visitor: F,
    ) -> Result<SimilarityStoreLoad, SimilarityStoreError>
    where
        F: FnMut(FrameGroup),
    {
        let path = self.path_for(expected);
'''
new = '''    pub fn visit_groups<F>(
        &self,
        expected: &SimilarityStoreKey,
        visitor: F,
    ) -> Result<SimilarityStoreLoad, SimilarityStoreError>
    where
        F: FnMut(FrameGroup),
    {
        self.visit_groups_with_frame_count(expected, None, visitor)
    }

    /// Validate a reusable contiguous grouping result against the authoritative completed frame
    /// count. Derived state with a missing first frame, a gap, an overlap, or a wrong final frame is
    /// invalidated rather than repaired optimistically.
    pub fn visit_groups_for_frame_count<F>(
        &self,
        expected: &SimilarityStoreKey,
        expected_frame_count: u64,
        visitor: F,
    ) -> Result<SimilarityStoreLoad, SimilarityStoreError>
    where
        F: FnMut(FrameGroup),
    {
        self.visit_groups_with_frame_count(expected, Some(expected_frame_count), visitor)
    }

    fn visit_groups_with_frame_count<F>(
        &self,
        expected: &SimilarityStoreKey,
        expected_frame_count: Option<u64>,
        mut visitor: F,
    ) -> Result<SimilarityStoreLoad, SimilarityStoreError>
    where
        F: FnMut(FrameGroup),
    {
        let path = self.path_for(expected);
'''
text = require_replace(text, old, new, label="store visit API")
text = require_replace(
    text,
    "if !quick_check_ok(&connection)? || !validate_rows(&connection, group_count)? {",
    "if !quick_check_ok(&connection)?\n            || !validate_rows(&connection, group_count, expected_frame_count)?\n        {",
    label="strict validation call",
)
text = require_replace(
    text,
    "fn validate_rows(\n    connection: &Connection,\n    expected_count: u64,\n) -> Result<bool, SimilarityStoreError> {",
    "fn validate_rows(\n    connection: &Connection,\n    expected_count: u64,\n    expected_frame_count: Option<u64>,\n) -> Result<bool, SimilarityStoreError> {",
    label="validate rows signature",
)
old = '''    let rows = statement.query_map([], decode_group)?;
    for row in rows {
        let group = row?;
        if invalid_group(&group) {
            return Ok(false);
        }
    }
    Ok(true)
}
'''
new = '''    let rows = statement.query_map([], decode_group)?;
    let mut previous_last = None;
    let mut observed_groups = 0_u64;
    for row in rows {
        let group = row?;
        if invalid_group(&group) {
            return Ok(false);
        }
        if expected_frame_count.is_some() {
            if observed_groups == 0 && group.first_frame != FrameId(0) {
                return Ok(false);
            }
            if let Some(previous_last) = previous_last {
                if previous_last.checked_add(1) != Some(group.first_frame.0) {
                    return Ok(false);
                }
            }
        }
        previous_last = Some(group.last_frame.0);
        observed_groups = observed_groups.saturating_add(1);
    }

    if let Some(expected_frame_count) = expected_frame_count {
        if expected_frame_count == 0 {
            return Ok(expected_count == 0 && observed_groups == 0);
        }
        if expected_count == 0
            || observed_groups != expected_count
            || previous_last != Some(expected_frame_count - 1)
        {
            return Ok(false);
        }
    }
    Ok(true)
}
'''
text = require_replace(text, old, new, label="cross-group coverage validation")
insert = r'''

    fn assert_strict_coverage_invalid(tag: &str, groups: &[FrameGroup], frame_count: u64) {
        let root = root(tag);
        let store = SimilarityStore::new(&root);
        let key = SimilarityStoreKey::new_hybrid(source("coverage"), stream(), hybrid(8, 9_700))
            .unwrap();
        let mut writer = store.begin(&key).unwrap();
        for group in groups {
            writer.append(group).unwrap();
        }
        writer.finish().unwrap();
        assert_eq!(
            store
                .visit_groups_for_frame_count(&key, frame_count, |_| {})
                .unwrap(),
            SimilarityStoreLoad::InvalidatedCorrupt
        );
        assert!(!store.path_for(&key).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn strict_coverage_accepts_complete_contiguous_groups() {
        let root = root("strict-complete");
        let store = SimilarityStore::new(&root);
        let key = SimilarityStoreKey::new_hybrid(source("a"), stream(), hybrid(8, 9_700)).unwrap();
        let mut writer = store.begin(&key).unwrap();
        writer.append(&group(0, 1, 0)).unwrap();
        writer.append(&group(2, 2, 80)).unwrap();
        writer.finish().unwrap();
        assert_eq!(
            store
                .visit_groups_for_frame_count(&key, 3, |_| {})
                .unwrap(),
            SimilarityStoreLoad::Reused { group_count: 2 }
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn strict_coverage_invalidates_gap() {
        assert_strict_coverage_invalid(
            "strict-gap",
            &[group(0, 0, 0), group(2, 2, 80)],
            3,
        );
    }

    #[test]
    fn strict_coverage_invalidates_overlap() {
        assert_strict_coverage_invalid(
            "strict-overlap",
            &[group(0, 1, 0), group(1, 2, 40)],
            3,
        );
    }

    #[test]
    fn strict_coverage_invalidates_first_group_not_at_zero() {
        assert_strict_coverage_invalid("strict-first", &[group(1, 2, 40)], 3);
    }

    #[test]
    fn strict_coverage_invalidates_final_group_ending_early() {
        assert_strict_coverage_invalid("strict-final", &[group(0, 1, 0)], 3);
    }

    #[test]
    fn hybrid_store_namespace_fences_metric_algorithm_generation() {
        let store = SimilarityStore::new(root("metric-version"));
        let key = SimilarityStoreKey::new_hybrid(source("a"), stream(), hybrid(8, 9_700)).unwrap();
        let path = store.path_for(&key).to_string_lossy().into_owned();
        assert!(path.contains("/v4/"));
        assert!(path.contains(&format!(
            "hybrid-a{HYBRID_SIMILARITY_ALGORITHM_VERSION}-h8-l9700"
        )));
    }
'''
closing = text.rfind("\n}")
if closing < 0:
    raise SystemExit("store test module closing brace not found")
text = text[:closing] + insert + text[closing:]
p.write_text(text)


# Product reuse path must prove full authoritative coverage both before reuse and after a rebuild.
p, text = load("rust/crates/framescope-group-navigation/src/lib.rs")
needle = "store.visit_groups(&key, |_| {})?"
if text.count(needle) != 2:
    raise SystemExit(f"group navigation: expected two store validations, found {text.count(needle)}")
text = text.replace(needle, "store.visit_groups_for_frame_count(&key, frame_count, |_| {})?")
p.write_text(text)


# Android terminology: retain internal enum/test tag compatibility, but stop presenting a temporal
# partition as global uniqueness or arbitrary similar-frame search.
p, text = load("android/app/src/main/java/com/framescope/app/ui/ExtractionWorkflow.kt")
text = require_replace(text, 'secondLabel = "Unique groups"', 'secondLabel = "Consecutive near-duplicates"', label="row label")
text = require_replace(text, 'label = "Unique groups"', 'label = "Consecutive near-duplicates"', label="chip label")
text = require_replace(
    text,
    '"One source-quality representative from each validated similarity group."',
    '"Exports one source-quality representative from each consecutive run of visually similar frames. Repeated similar frames later in the video are treated separately."',
    label="consecutive semantics copy",
)
p.write_text(text)
