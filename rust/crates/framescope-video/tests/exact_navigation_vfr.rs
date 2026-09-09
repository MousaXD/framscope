use framescope_cache::{
    FrameCacheHierarchy, FrameId, FrameIndex, FrameIndexEntry, FrameIndexOpenDisposition,
    FrameIndexStreamIdentity, KeyframeAnchor, SourceIdentity,
};
use framescope_core::{
    CodecInfo, DecodedFrame, FrameScopeError, MediaDuration, MediaKind, MediaTimestamp, StreamInfo,
    TimeBase,
};
use framescope_video::ffmpeg::DecodedRgbaFrame;
use framescope_video::{
    CachedFrameSource, TargetRgbaNavigationCursor, TargetRgbaNavigationDecoder,
    navigate_to_frame_cached_target_only_with_cursor,
};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
struct VfrFrame {
    pts: i64,
    duration: i64,
    keyframe: bool,
}

struct FakeDecoder {
    stream: StreamInfo,
    frames: VecDeque<DecodedRgbaFrame>,
    current: Option<DecodedRgbaFrame>,
    opens: Arc<AtomicU64>,
}

impl TargetRgbaNavigationDecoder for FakeDecoder {
    fn selected_stream_for_target_navigation(&self) -> &StreamInfo {
        &self.stream
    }

    fn next_metadata_for_target_navigation(
        &mut self,
    ) -> Result<Option<DecodedFrame>, FrameScopeError> {
        self.current = self.frames.pop_front();
        Ok(self.current.as_ref().map(|frame| frame.frame.clone()))
    }

    fn snapshot_current_rgba_for_target_navigation(
        &mut self,
        frame: &DecodedFrame,
    ) -> Result<DecodedRgbaFrame, FrameScopeError> {
        let current = self.current.as_ref().ok_or_else(|| {
            FrameScopeError::DecoderFailure("fake VFR decoder has no current frame".into())
        })?;
        if &current.frame != frame {
            return Err(FrameScopeError::DecoderFailure(
                "fake VFR decoder current frame mismatch".into(),
            ));
        }
        Ok(current.clone())
    }

    fn seek_for_target_navigation(&mut self, timestamp_us: i64) -> Result<(), FrameScopeError> {
        self.current = None;
        let target_ticks = timestamp_us;
        while self.frames.front().is_some_and(|frame| {
            frame
                .frame
                .presentation_timestamp
                .and_then(|timestamp| timestamp.to_microseconds())
                .is_some_and(|pts| pts < target_ticks)
        }) {
            self.frames.pop_front();
        }
        Ok(())
    }
}

fn stream() -> StreamInfo {
    StreamInfo {
        index: 0,
        media_kind: MediaKind::Video,
        codec: CodecInfo {
            id: 27,
            name: "h264".into(),
            decoder_available: true,
        },
        is_default: true,
        time_base: TimeBase::new(1, 1_000),
        duration: None,
        frame_count: None,
        width: Some(2),
        height: Some(2),
        pixel_format: Some("yuv420p".into()),
        average_frame_rate: None,
        nominal_frame_rate: None,
        rotation_degrees: None,
    }
}

fn timestamp(ticks: i64) -> MediaTimestamp {
    MediaTimestamp {
        ticks,
        time_base: TimeBase::new(1, 1_000).unwrap(),
    }
}

fn duration(ticks: i64) -> MediaDuration {
    MediaDuration {
        ticks,
        time_base: TimeBase::new(1, 1_000).unwrap(),
    }
}

fn rgba_frame(id: u64, spec: VfrFrame) -> DecodedRgbaFrame {
    DecodedRgbaFrame {
        frame: DecodedFrame {
            source_id: 1,
            stream_index: 0,
            decode_epoch: 0,
            index: id,
            presentation_timestamp: Some(timestamp(spec.pts)),
            duration: Some(duration(spec.duration)),
            keyframe: spec.keyframe,
            corrupt: false,
            width: 2,
            height: 2,
            pixel_format: Some("yuv420p".into()),
        },
        stride_bytes: 8,
        pixels: vec![id as u8; 16],
    }
}

fn temp_path(kind: &str) -> PathBuf {
    let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "framescope-exact-navigation-vfr-{kind}-{}-{id}",
        std::process::id()
    ))
}

#[test]
fn warm_forward_navigation_preserves_exact_vfr_identity_without_reopen_or_seek() {
    // Deliberately non-uniform PTS deltas and durations. No nominal-FPS inference can satisfy this
    // contract: every crossed frame has to match the persisted presentation timeline exactly.
    let timeline = [
        VfrFrame { pts: 0, duration: 33, keyframe: true },
        VfrFrame { pts: 33, duration: 51, keyframe: false },
        VfrFrame { pts: 84, duration: 17, keyframe: true },
        VfrFrame { pts: 101, duration: 76, keyframe: false },
        VfrFrame { pts: 177, duration: 29, keyframe: false },
    ];
    let index_path = temp_path("index.sqlite3");
    let source = SourceIdentity::new(1_024, None, Some("exact-navigation-vfr".into()));
    let identity = FrameIndexStreamIdentity::from_stream(&stream()).unwrap();
    let (mut index, disposition) = FrameIndex::open_or_create(&index_path, source, identity).unwrap();
    assert_eq!(disposition, FrameIndexOpenDisposition::Created);
    index.mark_building().unwrap();

    let mut anchor_id = FrameId::ZERO;
    let mut anchor_pts = timeline[0].pts;
    let entries = timeline
        .iter()
        .enumerate()
        .map(|(id, spec)| {
            if spec.keyframe {
                anchor_id = FrameId(id as u64);
                anchor_pts = spec.pts;
            }
            FrameIndexEntry {
                frame_id: FrameId(id as u64),
                presentation_timestamp: Some(timestamp(spec.pts)),
                duration: Some(duration(spec.duration)),
                keyframe: spec.keyframe,
                corrupt: false,
                anchor: KeyframeAnchor::Keyframe {
                    frame_id: anchor_id,
                    presentation_timestamp: Some(timestamp(anchor_pts)),
                },
            }
        })
        .collect::<Vec<_>>();
    index.append_batch(&entries).unwrap();
    index.mark_complete().unwrap();

    let cache_root = temp_path("cache");
    let mut cache = FrameCacheHierarchy::open(&cache_root, 1_024, 0).unwrap();
    let opens = Arc::new(AtomicU64::new(0));
    let mut cursor = None;
    let open_decoder = || {
        opens.fetch_add(1, Ordering::Relaxed);
        Ok(FakeDecoder {
            stream: stream(),
            frames: timeline
                .iter()
                .enumerate()
                .map(|(id, spec)| rgba_frame(id as u64, *spec))
                .collect(),
            current: None,
            opens: opens.clone(),
        })
    };

    let first = navigate_to_frame_cached_target_only_with_cursor(
        &index,
        &mut cache,
        &mut cursor,
        open_decoder,
        FrameId(3),
        2,
    )
    .unwrap();
    assert_eq!(first.frame_id, FrameId(3));
    assert_eq!(first.index_entry.presentation_timestamp, Some(timestamp(101)));
    assert_eq!(first.index_entry.duration, Some(duration(76)));
    let opens_after_first = opens.load(Ordering::Relaxed);
    assert!(opens_after_first >= 1);

    let second = navigate_to_frame_cached_target_only_with_cursor(
        &index,
        &mut cache,
        &mut cursor,
        || {
            opens.fetch_add(1, Ordering::Relaxed);
            Ok(FakeDecoder {
                stream: stream(),
                frames: timeline
                    .iter()
                    .enumerate()
                    .map(|(id, spec)| rgba_frame(id as u64, *spec))
                    .collect(),
                current: None,
                opens: opens.clone(),
            })
        },
        FrameId(4),
        2,
    )
    .unwrap();

    assert_eq!(second.source, CachedFrameSource::Decoded);
    assert_eq!(second.frame_id, FrameId(4));
    assert_eq!(second.index_entry.presentation_timestamp, Some(timestamp(177)));
    assert_eq!(second.index_entry.duration, Some(duration(29)));
    assert_eq!(second.decoded_frames, 1);
    assert!(!second.used_keyframe_seek);
    assert_eq!(opens.load(Ordering::Relaxed), opens_after_first);
    assert_eq!(
        cursor.as_ref().map(TargetRgbaNavigationCursor::frame_id),
        Some(FrameId(4)),
    );

    drop(index);
    let _ = std::fs::remove_file(index_path);
    let _ = std::fs::remove_dir_all(cache_root);
}
