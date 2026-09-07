use super::*;
use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

fn temp_db(name: &str) -> PathBuf {
    let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "framescope-index-{}-{name}-{id}.sqlite",
        std::process::id()
    ))
}

fn source(tag: &str) -> SourceIdentity {
    SourceIdentity::new(10_000, Some(123), Some(tag.into()))
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

fn entry(frame: u64, ticks: i64, keyframe: bool, anchor: u64) -> FrameIndexEntry {
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
                ticks: (anchor as i64) * 40,
                time_base,
            }),
        },
    }
}

fn cleanup(path: &PathBuf) {
    let _ = store::purge_database_files(path);
}

#[test]
fn roundtrip_lookup_and_complete_count() {
    let path = temp_db("roundtrip");
    let (mut index, disposition) = FrameIndex::open_or_create(&path, source("a"), stream()).unwrap();
    assert_eq!(disposition, FrameIndexOpenDisposition::Created);
    index.mark_building().unwrap();
    index
        .append_batch(&[
            entry(0, 0, true, 0),
            entry(1, 40, false, 0),
            entry(2, 100, false, 0),
        ])
        .unwrap();
    index.mark_complete().unwrap();
    assert_eq!(index.frame_count().unwrap(), Some(3));
    assert_eq!(index.entry(FrameId(1)).unwrap().unwrap().timestamp_us(), Some(40_000));
    assert_eq!(
        index
            .frame_at_or_before(MediaTimestamp {
                ticks: 70,
                time_base: stream().time_base,
            })
            .unwrap()
            .unwrap()
            .frame_id,
        FrameId(1)
    );
    assert_eq!(
        index
            .frame_at_or_after(MediaTimestamp {
                ticks: 70,
                time_base: stream().time_base,
            })
            .unwrap()
            .unwrap()
            .frame_id,
        FrameId(2)
    );
    drop(index);
    cleanup(&path);
}

#[test]
fn vfr_signed_and_repeated_timestamps_survive() {
    let path = temp_db("vfr");
    let (mut index, _) = FrameIndex::open_or_create(&path, source("vfr"), stream()).unwrap();
    index
        .append_batch(&[
            entry(0, -20, true, 0),
            entry(1, 0, false, 0),
            entry(2, 0, false, 0),
            entry(3, 75, false, 0),
            entry(4, 210, false, 0),
        ])
        .unwrap();
    assert_eq!(index.frame_at_or_before_us(0).unwrap().unwrap().frame_id, FrameId(2));
    assert_eq!(index.frame_at_or_after_us(0).unwrap().unwrap().frame_id, FrameId(1));
    assert_eq!(
        index.entry(FrameId(4)).unwrap().unwrap().presentation_timestamp.unwrap().ticks,
        210
    );
    drop(index);
    cleanup(&path);
}

#[test]
fn incomplete_index_never_reports_complete_count() {
    let path = temp_db("partial");
    let (mut index, _) = FrameIndex::open_or_create(&path, source("partial"), stream()).unwrap();
    index.mark_building().unwrap();
    index.append_batch(&[entry(0, 0, true, 0)]).unwrap();
    index.mark_incomplete(Some("cancelled")).unwrap();
    let status = index.status().unwrap();
    assert_eq!(status.lifecycle, FrameIndexLifecycle::Incomplete);
    assert_eq!(status.indexed_frames, 1);
    assert_eq!(status.frame_count, None);
    assert_eq!(status.last_error.as_deref(), Some("cancelled"));
    drop(index);
    cleanup(&path);
}

#[test]
fn stale_source_is_rebuilt() {
    let path = temp_db("stale");
    let (mut index, _) = FrameIndex::open_or_create(&path, source("old"), stream()).unwrap();
    index.append_batch(&[entry(0, 0, true, 0)]).unwrap();
    index.mark_complete().unwrap();
    drop(index);
    let (index, disposition) = FrameIndex::open_or_create(&path, source("new"), stream()).unwrap();
    assert_eq!(disposition, FrameIndexOpenDisposition::RebuiltStaleSource);
    assert_eq!(index.status().unwrap().indexed_frames, 0);
    drop(index);
    cleanup(&path);
}

#[test]
fn unverifiable_source_is_rebuilt_on_reopen() {
    let path = temp_db("weak");
    let weak = SourceIdentity::metadata_only(Some(100), Some(5), Some("document:5".into()));
    let (mut index, _) = FrameIndex::open_or_create(&path, weak.clone(), stream()).unwrap();
    index.append_batch(&[entry(0, 0, true, 0)]).unwrap();
    drop(index);
    let (index, disposition) = FrameIndex::open_or_create(&path, weak, stream()).unwrap();
    assert_eq!(
        disposition,
        FrameIndexOpenDisposition::RebuiltUnverifiableSource
    );
    assert_eq!(index.status().unwrap().indexed_frames, 0);
    drop(index);
    cleanup(&path);
}

#[test]
fn garbage_database_is_recreated() {
    let path = temp_db("corrupt");
    fs::write(&path, b"not a sqlite database").unwrap();
    let (index, disposition) = FrameIndex::open_or_create(&path, source("a"), stream()).unwrap();
    assert_eq!(disposition, FrameIndexOpenDisposition::RecoveredCorruptState);
    assert_eq!(index.status().unwrap().indexed_frames, 0);
    drop(index);
    cleanup(&path);
}

#[test]
fn schema_zero_migrates_and_newer_schema_recreates() {
    let path = temp_db("migration");
    Connection::open(&path).unwrap();
    let (index, _) = FrameIndex::open_or_create(&path, source("migration"), stream()).unwrap();
    drop(index);
    cleanup(&path);

    let path = temp_db("newer");
    let connection = Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 999_i64).unwrap();
    drop(connection);
    let (index, disposition) = FrameIndex::open_or_create(&path, source("schema"), stream()).unwrap();
    assert_eq!(
        disposition,
        FrameIndexOpenDisposition::RecreatedUnsupportedSchema
    );
    drop(index);
    cleanup(&path);
}

#[test]
fn range_iteration_is_ordered_without_range_sized_allocation() {
    let path = temp_db("range");
    let (mut index, _) = FrameIndex::open_or_create(&path, source("range"), stream()).unwrap();
    let entries = (0..10)
        .map(|id| entry(id, (id as i64) * 40, id == 0, 0))
        .collect::<Vec<_>>();
    index.append_batch(&entries).unwrap();
    let mut seen = Vec::new();
    index
        .visit_range(FrameId(3), FrameId(7), |entry| {
            seen.push(entry.frame_id.0);
            Ok(())
        })
        .unwrap();
    assert_eq!(seen, vec![3, 4, 5, 6]);
    drop(index);
    cleanup(&path);
}
