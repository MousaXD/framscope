#![cfg(feature = "system-ffmpeg")]

use framescope_cache::{
    FrameId, FrameIndex, FrameIndexLifecycle, FrameIndexStreamIdentity, SourceIdentity,
};
use framescope_video::{
    IndexingOptions, IndexingReport, VideoDecoder, build_or_resume_frame_index,
};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

fn fixture(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../build/video-fixtures")
        .join(name);
    assert!(
        path.is_file(),
        "missing generated fixture {}",
        path.display()
    );
    path
}

fn temp_db(name: &str) -> PathBuf {
    let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "framescope-real-index-{}-{name}-{id}.sqlite",
        std::process::id()
    ))
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(sidecar(path, "-wal"));
    let _ = std::fs::remove_file(sidecar(path, "-shm"));
}

fn source_identity(path: &Path) -> SourceIdentity {
    let mut file = File::open(path).unwrap();
    SourceIdentity::from_seekable(&mut file, None, None).unwrap()
}

fn decoder_timestamps(path: &Path) -> Vec<Option<i64>> {
    let mut decoder = VideoDecoder::open_path(path).unwrap();
    let mut timestamps = Vec::new();
    while let Some(frame) = decoder.next_frame().unwrap() {
        timestamps.push(
            frame
                .presentation_timestamp
                .map(|timestamp| timestamp.ticks),
        );
    }
    timestamps
}

fn build_fixture_index(name: &str, batch_size: usize) -> (PathBuf, FrameIndex, IndexingReport) {
    let path = fixture(name);
    let decoder = VideoDecoder::open_path(&path).unwrap();
    let stream = FrameIndexStreamIdentity::from_stream(decoder.selected_stream()).unwrap();
    drop(decoder);

    let db = temp_db(name);
    cleanup(&db);
    let (mut index, _) = FrameIndex::open_or_create(&db, source_identity(&path), stream).unwrap();
    let report = build_or_resume_frame_index(
        &mut index,
        || VideoDecoder::open_path(&path),
        IndexingOptions::with_batch_size(batch_size).unwrap(),
    )
    .unwrap();
    assert_eq!(report.status.lifecycle, FrameIndexLifecycle::Complete);
    assert!(report.max_pending_entries <= batch_size);
    (db, index, report)
}

#[test]
fn cfr_index_matches_independent_decoder_pts() {
    let path = fixture("h264-cfr.mp4");
    let truth = decoder_timestamps(&path);
    let (db, index, report) = build_fixture_index("h264-cfr.mp4", 3);
    assert_eq!(index.frame_count().unwrap(), Some(12));
    assert_eq!(report.frames_decoded, 12);
    assert_eq!(report.sqlite_rows_inserted, 12);
    assert_eq!(report.batch_commits, 4);
    assert_eq!(report.decoder_open_count, 1);
    assert!(!report.pipeline_enabled);
    assert_eq!(report.pipeline_queue_idle_elapsed_us, 0);
    assert!(report.sqlite_commit_elapsed_us <= report.sqlite_batch_elapsed_us);
    assert!(report.sqlite_transaction_begin_elapsed_us <= report.sqlite_batch_elapsed_us);
    assert!(report.miscellaneous_elapsed_us <= report.total_elapsed_us);

    let mut persisted = Vec::new();
    index
        .visit_range(FrameId(0), FrameId(12), |entry| {
            persisted.push(
                entry
                    .presentation_timestamp
                    .map(|timestamp| timestamp.ticks),
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(persisted, truth);
    drop(index);
    cleanup(&db);
}

#[test]
fn vfr_index_preserves_actual_non_uniform_decoded_pts() {
    let path = fixture("h264-vfr.mp4");
    let truth = decoder_timestamps(&path);
    let (db, index, report) = build_fixture_index("h264-vfr.mp4", 2);
    assert_eq!(index.frame_count().unwrap(), Some(8));
    assert_eq!(report.frames_decoded, 8);
    assert_eq!(report.sqlite_rows_inserted, 8);
    assert_eq!(report.batch_commits, 4);

    let mut persisted = Vec::new();
    index
        .visit_range(FrameId(0), FrameId(8), |entry| {
            persisted.push(
                entry
                    .presentation_timestamp
                    .map(|timestamp| timestamp.ticks),
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(persisted, truth);
    let ticks = persisted
        .into_iter()
        .map(Option::unwrap)
        .collect::<Vec<_>>();
    let deltas = ticks
        .windows(2)
        .map(|window| window[1] - window[0])
        .collect::<Vec<_>>();
    assert!(deltas.windows(2).any(|window| window[0] != window[1]));
    drop(index);
    cleanup(&db);
}

#[test]
fn index_handles_phase2_edge_fixtures_without_pixel_storage() {
    for (name, expected) in [
        ("very-short.mp4", 1_u64),
        ("unusual-dimensions.mp4", 4),
        ("rotated-portrait.mp4", 6),
        ("h264-with-audio.mp4", 6),
        ("multi-stream.mkv", 4),
    ] {
        let (db, index, report) = build_fixture_index(name, 2);
        assert_eq!(index.frame_count().unwrap(), Some(expected), "{name}");
        assert_eq!(report.frames_decoded, expected, "{name}");
        assert_eq!(report.sqlite_rows_inserted, expected, "{name}");
        drop(index);
        cleanup(&db);
    }
}
