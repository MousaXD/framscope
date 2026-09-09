use framescope_cache::{
    FrameId, FrameIndex, FrameIndexEntry, FrameIndexStreamIdentity, KeyframeAnchor, SourceIdentity,
};
use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

fn temp_db(label: &str) -> PathBuf {
    let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "framescope-index-profile-{}-{label}-{id}.sqlite",
        std::process::id()
    ))
}

fn source() -> SourceIdentity {
    SourceIdentity::new(10_000, Some(123), Some("profile-contract".into()))
}

fn stream() -> FrameIndexStreamIdentity {
    FrameIndexStreamIdentity {
        stream_index: 0,
        codec_id: 27,
        codec_name: "h264".into(),
        time_base: TimeBase::new(1, 1_000).unwrap(),
        width: Some(64),
        height: Some(48),
    }
}

fn entry(
    frame: u64,
    ticks: i64,
    keyframe: bool,
    anchor: u64,
    anchor_ticks: i64,
) -> FrameIndexEntry {
    let time_base = TimeBase::new(1, 1_000).unwrap();
    FrameIndexEntry {
        frame_id: FrameId(frame),
        presentation_timestamp: Some(MediaTimestamp { ticks, time_base }),
        duration: Some(MediaDuration {
            ticks: 40,
            time_base,
        }),
        keyframe,
        corrupt: false,
        anchor: KeyframeAnchor::Keyframe {
            frame_id: FrameId(anchor),
            presentation_timestamp: Some(MediaTimestamp {
                ticks: anchor_ticks,
                time_base,
            }),
        },
    }
}

fn remove_database(path: &Path) {
    for candidate in [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ] {
        let _ = std::fs::remove_file(candidate);
    }
}

#[test]
fn profiled_append_preserves_authoritative_rows_and_status() {
    let ordinary_path = temp_db("ordinary");
    let profiled_path = temp_db("profiled");
    let entries = vec![
        entry(0, 0, true, 0, 0),
        entry(1, 40, false, 0, 0),
        entry(2, 100, false, 0, 0),
        entry(3, 180, true, 3, 180),
        entry(4, 260, false, 3, 180),
    ];

    let (mut ordinary, _) = FrameIndex::open_or_create(&ordinary_path, source(), stream()).unwrap();
    ordinary.mark_building().unwrap();
    ordinary.append_batch(&entries).unwrap();
    ordinary.mark_complete().unwrap();

    let (mut profiled, _) = FrameIndex::open_or_create(&profiled_path, source(), stream()).unwrap();
    profiled.mark_building().unwrap();
    let (transaction_begin_elapsed_us, commit_elapsed_us) =
        profiled.append_batch_profiled(&entries).unwrap();
    profiled.mark_complete().unwrap();

    assert_eq!(profiled.status().unwrap(), ordinary.status().unwrap());
    assert_eq!(
        profiled.timestamp_seek_safety().unwrap(),
        ordinary.timestamp_seek_safety().unwrap()
    );
    for id in 0..entries.len() as u64 {
        assert_eq!(
            profiled.entry(FrameId(id)).unwrap(),
            ordinary.entry(FrameId(id)).unwrap()
        );
    }

    // Timings are allowed to quantize to zero on very fast hosts; successful collection is the
    // contract. These checks also keep the values exercised by the test without adding flaky floors.
    let _ = transaction_begin_elapsed_us;
    let _ = commit_elapsed_us;

    drop(profiled);
    drop(ordinary);
    remove_database(&profiled_path);
    remove_database(&ordinary_path);
}
