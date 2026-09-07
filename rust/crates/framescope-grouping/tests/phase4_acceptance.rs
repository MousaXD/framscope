use framescope_cache::{FrameId, FrameIndexEntry, KeyframeAnchor, OwnedRgbaFrame};
use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};
use framescope_grouping::HybridFrameGrouper;
use framescope_perceptual::{HybridDecision, HybridSimilarityEngine, HybridSimilarityPolicy};

fn time_base() -> TimeBase {
    TimeBase::new(1, 1000).unwrap()
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

fn rgba(width: u32, height: u32, mut pixel: impl FnMut(u32, u32) -> [u8; 4]) -> OwnedRgbaFrame {
    let stride = usize::try_from(width).unwrap() * 4;
    let mut pixels = Vec::with_capacity(stride * usize::try_from(height).unwrap());
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&pixel(x, y));
        }
    }
    OwnedRgbaFrame::new(width, height, stride, pixels).unwrap()
}

fn solid(value: u8) -> OwnedRgbaFrame {
    rgba(9, 8, |_x, _y| [value, value, value, 255])
}

fn horizontal_gradient(reverse: bool) -> OwnedRgbaFrame {
    rgba(9, 8, |x, _y| {
        let column = if reverse { 8 - x } else { x };
        let value = u8::try_from(column * 24).unwrap();
        [value, value, value, 255]
    })
}

fn sparse_overlay(base: u8, changed_pixels: u32) -> OwnedRgbaFrame {
    rgba(9, 8, |x, y| {
        let linear = y * 9 + x;
        if linear < changed_pixels {
            [255, 255, 255, 255]
        } else {
            [base, base, base, 255]
        }
    })
}

fn permissive_hash_policy(minimum_luma_similarity: u16) -> HybridSimilarityPolicy {
    HybridSimilarityPolicy {
        max_hash_distance: 64,
        minimum_luma_similarity,
    }
}

#[test]
fn deterministic_sequence_preserves_vfr_timeline_and_expected_boundaries() {
    let mut grouper = HybridFrameGrouper::new(HybridSimilarityPolicy {
        max_hash_distance: 8,
        minimum_luma_similarity: 9_900,
    })
    .unwrap();

    let sequence = [
        (entry(0, 0, 33), solid(100)),
        (entry(1, 33, 51), solid(100)),
        (entry(2, 84, 17), solid(101)),
        (entry(3, 101, 64), solid(150)),
        (entry(4, 165, 40), solid(150)),
    ];

    let mut groups = Vec::new();
    for (metadata, pixels) in sequence {
        if let Some(group) = grouper.push(&metadata, pixels).unwrap() {
            groups.push(group);
        }
    }
    groups.push(grouper.finish().unwrap());

    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].first_frame, FrameId(0));
    assert_eq!(groups[0].last_frame, FrameId(2));
    assert_eq!(groups[0].frame_count, 3);
    assert_eq!(groups[0].representative_frame, FrameId(0));
    assert_eq!(groups[0].start_timestamp.ticks, 0);
    assert_eq!(groups[0].end_timestamp.ticks, 84);
    assert_eq!(groups[0].start_duration.unwrap().ticks, 33);
    assert_eq!(groups[0].end_duration.unwrap().ticks, 17);

    assert_eq!(groups[1].first_frame, FrameId(3));
    assert_eq!(groups[1].last_frame, FrameId(4));
    assert_eq!(groups[1].frame_count, 2);
    assert_eq!(groups[1].start_timestamp.ticks, 101);
    assert_eq!(groups[1].end_timestamp.ticks, 165);
    assert_eq!(groups[1].end_duration.unwrap().ticks, 40);

    let represented: u64 = groups.iter().map(|group| group.frame_count).sum();
    assert_eq!(represented, 5, "grouping must never delete timeline frames");
}

#[test]
fn perceptual_scene_cut_is_rejected_by_hash_before_confirmation() {
    let engine = HybridSimilarityEngine::new(HybridSimilarityPolicy {
        max_hash_distance: 0,
        minimum_luma_similarity: 0,
    })
    .unwrap();

    let decision = engine
        .compare(&horizontal_gradient(false), &horizontal_gradient(true))
        .unwrap();
    assert!(matches!(decision, HybridDecision::RejectedByHash { .. }));
}

#[test]
fn tiny_overlay_can_group_but_larger_overlay_creates_boundary() {
    let policy = permissive_hash_policy(9_900);
    let engine = HybridSimilarityEngine::new(policy).unwrap();
    let base = solid(100);
    let tiny = sparse_overlay(100, 1);
    let large = sparse_overlay(100, 8);

    assert!(engine.compare(&base, &tiny).unwrap().accepted());
    assert!(!engine.compare(&base, &large).unwrap().accepted());

    let mut grouper = HybridFrameGrouper::new(policy).unwrap();
    assert!(grouper.push(&entry(0, 0, 40), base).unwrap().is_none());
    assert!(grouper.push(&entry(1, 40, 40), tiny).unwrap().is_none());
    let completed = grouper.push(&entry(2, 80, 40), large).unwrap().unwrap();

    assert_eq!(completed.first_frame, FrameId(0));
    assert_eq!(completed.last_frame, FrameId(1));
    assert_eq!(completed.frame_count, 2);
    assert_eq!(grouper.finish().unwrap().first_frame, FrameId(2));
}

#[test]
fn representative_check_blocks_gradual_drift_even_when_adjacent_pairs_pass() {
    let policy = permissive_hash_policy(9_700);
    let engine = HybridSimilarityEngine::new(policy).unwrap();

    assert!(engine.compare(&solid(0), &solid(5)).unwrap().accepted());
    assert!(engine.compare(&solid(5), &solid(10)).unwrap().accepted());
    assert!(!engine.compare(&solid(0), &solid(10)).unwrap().accepted());

    let mut grouper = HybridFrameGrouper::new(policy).unwrap();
    assert!(grouper.push(&entry(0, 0, 40), solid(0)).unwrap().is_none());
    assert!(grouper.push(&entry(1, 40, 40), solid(5)).unwrap().is_none());
    let first = grouper.push(&entry(2, 80, 40), solid(10)).unwrap().unwrap();

    assert_eq!(first.first_frame, FrameId(0));
    assert_eq!(first.last_frame, FrameId(1));
    assert_eq!(grouper.finish().unwrap().first_frame, FrameId(2));
}

#[test]
fn exact_duplicate_grouping_keeps_authoritative_frame_ids() {
    let mut grouper = HybridFrameGrouper::new(HybridSimilarityPolicy {
        max_hash_distance: 0,
        minimum_luma_similarity: 10_000,
    })
    .unwrap();

    for id in 0..4 {
        assert!(
            grouper
                .push(&entry(id, i64::try_from(id * 40).unwrap(), 40), solid(42))
                .unwrap()
                .is_none()
        );
    }
    let group = grouper.finish().unwrap();
    assert_eq!(group.representative_frame, FrameId(0));
    assert_eq!(group.first_frame, FrameId(0));
    assert_eq!(group.last_frame, FrameId(3));
    assert_eq!(group.frame_count, 4);
}
