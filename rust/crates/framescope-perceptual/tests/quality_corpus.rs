use framescope_cache::OwnedRgbaFrame;
use framescope_perceptual::{
    HybridSimilarityEngine, HybridSimilarityPolicy, SimilarityQualityCounters,
};

fn frame_from_rgb(width: u32, height: u32, rgb: &[(u8, u8, u8)]) -> OwnedRgbaFrame {
    assert_eq!(rgb.len(), (width * height) as usize);
    let mut pixels = Vec::with_capacity(rgb.len() * 4);
    for &(r, g, b) in rgb {
        pixels.extend_from_slice(&[r, g, b, 255]);
    }
    OwnedRgbaFrame::new(width, height, width as usize * 4, pixels).unwrap()
}

fn base_pattern() -> Vec<(u8, u8, u8)> {
    let mut out = Vec::with_capacity(16 * 16);
    for y in 0..16_u8 {
        for x in 0..16_u8 {
            out.push((
                24_u8.saturating_add(x.saturating_mul(9)),
                18_u8.saturating_add(y.saturating_mul(10)),
                12_u8.saturating_add(x.saturating_mul(4).saturating_add(y.saturating_mul(3))),
            ));
        }
    }
    out
}

fn map_pixels(
    input: &[(u8, u8, u8)],
    f: impl Fn(usize, usize, (u8, u8, u8)) -> (u8, u8, u8),
) -> Vec<(u8, u8, u8)> {
    input
        .iter()
        .copied()
        .enumerate()
        .map(|(i, px)| f(i % 16, i / 16, px))
        .collect()
}

fn translated(input: &[(u8, u8, u8)], dx: usize, dy: usize) -> Vec<(u8, u8, u8)> {
    let mut out = vec![(0, 0, 0); 16 * 16];
    for y in 0_usize..16 {
        for x in 0_usize..16 {
            let sx = x.saturating_sub(dx);
            let sy = y.saturating_sub(dy);
            out[y * 16 + x] = input[sy * 16 + sx];
        }
    }
    out
}

fn rotated_180(input: &[(u8, u8, u8)]) -> Vec<(u8, u8, u8)> {
    input.iter().copied().rev().collect()
}

struct Case {
    name: &'static str,
    expected_similar: bool,
    right: Vec<(u8, u8, u8)>,
}

#[test]
fn labeled_similarity_corpus_reports_quality_and_hash_gate_misses() {
    let base = base_pattern();
    let compression = map_pixels(&base, |x, y, (r, g, b)| {
        let q = if (x + y) % 2 == 0 { 1 } else { 0 };
        (
            r.saturating_add(q),
            g.saturating_sub(q),
            b.saturating_add(q),
        )
    });
    let brightness = map_pixels(&base, |_x, _y, (r, g, b)| {
        (
            r.saturating_add(3),
            g.saturating_add(3),
            b.saturating_add(3),
        )
    });
    let exposure_flicker = map_pixels(&base, |x, _y, (r, g, b)| {
        let d = if x % 2 == 0 { 4 } else { 1 };
        (
            r.saturating_add(d),
            g.saturating_add(d),
            b.saturating_add(d),
        )
    });
    let subtitle = map_pixels(&base, |x, y, px| {
        if (3..13).contains(&x) && (12..15).contains(&y) {
            (250, 250, 250)
        } else {
            px
        }
    });
    let moving_foreground = map_pixels(&base, |x, y, px| {
        if (4..12).contains(&x) && (4..12).contains(&y) {
            (220, 35, 35)
        } else {
            px
        }
    });
    let fade = map_pixels(&base, |_x, _y, (r, g, b)| (r / 2, g / 2, b / 2));
    let hard_cut = vec![(8, 220, 170); 16 * 16];
    let animation = map_pixels(
        &base,
        |x, y, px| {
            if (x + y) % 3 == 0 { (20, 210, 240) } else { px }
        },
    );
    let crop_rescaled = map_pixels(&base, |x, y, _| {
        let sx = 2 + x * 12 / 16;
        let sy = 2 + y * 12 / 16;
        base[sy * 16 + sx]
    });
    let scaled = map_pixels(&base, |x, y, _| {
        let sx = x / 2 * 2;
        let sy = y / 2 * 2;
        base[sy * 16 + sx]
    });

    let engine = HybridSimilarityEngine::new(HybridSimilarityPolicy {
        max_hash_distance: 8,
        minimum_luma_similarity: 9_700,
    })
    .unwrap();
    let left = frame_from_rgb(16, 16, &base);
    let cases = vec![
        Case {
            name: "exact_duplicate",
            expected_similar: true,
            right: base.clone(),
        },
        Case {
            name: "compression_difference",
            expected_similar: true,
            right: compression,
        },
        Case {
            name: "brightness_shift",
            expected_similar: true,
            right: brightness,
        },
        Case {
            name: "exposure_flicker",
            expected_similar: true,
            right: exposure_flicker,
        },
        Case {
            name: "small_translation",
            expected_similar: true,
            right: translated(&base, 1, 0),
        },
        Case {
            name: "camera_shake",
            expected_similar: true,
            right: translated(&base, 1, 1),
        },
        Case {
            name: "crop",
            expected_similar: true,
            right: crop_rescaled,
        },
        Case {
            name: "scale",
            expected_similar: true,
            right: scaled,
        },
        Case {
            name: "rotation",
            expected_similar: false,
            right: rotated_180(&base),
        },
        Case {
            name: "subtitle_change",
            expected_similar: false,
            right: subtitle,
        },
        Case {
            name: "moving_foreground",
            expected_similar: false,
            right: moving_foreground,
        },
        Case {
            name: "fade",
            expected_similar: false,
            right: fade,
        },
        Case {
            name: "hard_cut",
            expected_similar: false,
            right: hard_cut,
        },
        Case {
            name: "animation",
            expected_similar: false,
            right: animation,
        },
    ];

    let mut counters = SimilarityQualityCounters::default();
    for case in &cases {
        let right = frame_from_rgb(16, 16, &case.right);
        let analysis = engine.analyze(&left, &right).unwrap();
        eprintln!(
            "{} expected={} accepted={} confirmation={} gate_false_negative={}",
            case.name,
            case.expected_similar,
            analysis.decision.accepted(),
            analysis.confirmation.basis_points,
            analysis.hash_rejected_but_confirmation_accepted,
        );
        counters.record(case.expected_similar, analysis);
    }

    // Equal-luma, radically different hue is the proven false-positive regression from the audit.
    let red = vec![(255, 0, 0); 16 * 16];
    let green = vec![(0, 131, 0); 16 * 16];
    let hue_analysis = engine
        .analyze(
            &frame_from_rgb(16, 16, &red),
            &frame_from_rgb(16, 16, &green),
        )
        .unwrap();
    counters.record(false, hue_analysis);
    assert!(!hue_analysis.decision.accepted());

    let classified = counters.true_positive
        + counters.false_positive
        + counters.true_negative
        + counters.false_negative;
    assert_eq!(classified, counters.compared_pairs);
    assert_eq!(counters.compared_pairs, cases.len() as u64 + 1);
    assert!(counters.precision_basis_points().is_some());
    assert!(counters.recall_basis_points().is_some());
    assert!(counters.f1_basis_points().is_some());
    eprintln!(
        "quality tp={} fp={} tn={} fn={} precision_bps={:?} recall_bps={:?} f1_bps={:?} hash_rejects={} hash_reject_confirmation_accepts={}",
        counters.true_positive,
        counters.false_positive,
        counters.true_negative,
        counters.false_negative,
        counters.precision_basis_points(),
        counters.recall_basis_points(),
        counters.f1_basis_points(),
        counters.rejected_by_hash,
        counters.hash_reject_confirmation_accepts,
    );
}
