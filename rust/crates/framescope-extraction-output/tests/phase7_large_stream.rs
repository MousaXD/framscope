use framescope_cache::{
    FrameId, FrameIndex, FrameIndexEntry, FrameIndexOpenDisposition, FrameIndexStreamIdentity,
    KeyframeAnchor, SourceIdentity,
};
use framescope_core::{MediaDuration, MediaKind, MediaTimestamp, TimeBase};
use framescope_extraction::{ExtractionRequest, ExtractionSampling, ExtractionSelection};
use framescope_extraction_image::{EncodedImageReport, ExtractionImageFormat};
use framescope_extraction_output::{FrameOutput, FrameOutputSink, export_request};
use framescope_video::{
    CodecInfo, DecodedFrame, FrameScopeError, RgbaNavigationDecoder, StreamInfo,
    ffmpeg::DecodedRgbaFrame,
};
use std::cell::Cell;
use std::convert::Infallible;
use std::io::{self, Write};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

const DEFAULT_STRESS_FRAMES: u64 = 100_000;
const INDEX_BATCH_SIZE: u64 = 256;
const SAMPLE_EVERY_N: u64 = 10;
static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct DecoderCounters {
    opens: Rc<Cell<u64>>,
    nexts: Rc<Cell<u64>>,
    seeks: Rc<Cell<u64>>,
}

impl DecoderCounters {
    fn new() -> Self {
        Self {
            opens: Rc::new(Cell::new(0)),
            nexts: Rc::new(Cell::new(0)),
            seeks: Rc::new(Cell::new(0)),
        }
    }
}

struct GeneratedDecoder {
    stream: StreamInfo,
    next_frame: u64,
    frame_count: u64,
    counters: DecoderCounters,
}

impl RgbaNavigationDecoder for GeneratedDecoder {
    fn selected_stream_for_rgba_navigation(&self) -> &StreamInfo {
        &self.stream
    }

    fn next_rgba_for_navigation(&mut self) -> Result<Option<DecodedRgbaFrame>, FrameScopeError> {
        self.counters.nexts.set(self.counters.nexts.get() + 1);
        if self.next_frame >= self.frame_count {
            return Ok(None);
        }
        let frame_id = self.next_frame;
        self.next_frame += 1;
        Ok(Some(decoded(frame_id)))
    }

    fn seek_for_rgba_navigation(&mut self, timestamp_us: i64) -> Result<(), FrameScopeError> {
        self.counters.seeks.set(self.counters.seeks.get() + 1);
        let frame = if timestamp_us <= 0 {
            0
        } else {
            u64::try_from(timestamp_us).unwrap_or(u64::MAX) / 40_000
        };
        self.next_frame = frame.min(self.frame_count);
        Ok(())
    }
}

struct CountingSink {
    committed: u64,
    last_ordinal: Option<u64>,
    last_frame_id: Option<u64>,
}

impl CountingSink {
    fn new() -> Self {
        Self {
            committed: 0,
            last_ordinal: None,
            last_frame_id: None,
        }
    }
}

impl FrameOutputSink for CountingSink {
    type Error = Infallible;

    fn write_frame(&mut self, output: FrameOutput<'_>) -> Result<EncodedImageReport, Self::Error> {
        if let Some(previous) = self.last_ordinal {
            assert_eq!(output.progress.ordinal, previous + 1);
        }
        if let Some(previous) = self.last_frame_id {
            assert!(output.progress.frame_id.0 > previous);
        }
        assert_eq!(output.pixels.width, 2);
        assert_eq!(output.pixels.height, 1);
        assert_eq!(output.pixels.pixels().len(), 8);

        self.committed += 1;
        self.last_ordinal = Some(output.progress.ordinal);
        self.last_frame_id = Some(output.progress.frame_id.0);
        Ok(EncodedImageReport {
            format: output.format,
            byte_len: 8,
        })
    }
}

#[derive(Default)]
struct CountingWriter {
    bytes: u64,
    writes: u64,
    max_write_bytes: usize,
}

impl Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(u64::try_from(buf.len()).expect("write length fits u64"))
            .expect("manifest byte counter overflow");
        self.writes += 1;
        self.max_write_bytes = self.max_write_bytes.max(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn temp_root() -> PathBuf {
    let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "framescope-phase7-large-stream-{}-{id}",
        std::process::id()
    ))
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
        height: Some(1),
        pixel_format: Some("rgba".into()),
        average_frame_rate: None,
        nominal_frame_rate: None,
        rotation_degrees: None,
    }
}

fn entry(frame_id: u64) -> FrameIndexEntry {
    let time_base = TimeBase::new(1, 1_000).expect("valid stress-test time base");
    let ticks = i64::try_from(frame_id)
        .expect("stress frame id fits i64")
        .checked_mul(40)
        .expect("stress timestamp fits i64");
    let timestamp = MediaTimestamp { ticks, time_base };
    FrameIndexEntry {
        frame_id: FrameId(frame_id),
        presentation_timestamp: Some(timestamp),
        duration: Some(MediaDuration {
            ticks: 40,
            time_base,
        }),
        keyframe: frame_id % 30 == 0,
        corrupt: false,
        anchor: KeyframeAnchor::Keyframe {
            frame_id: FrameId(frame_id - (frame_id % 30)),
            presentation_timestamp: Some(MediaTimestamp {
                ticks: i64::try_from(frame_id - (frame_id % 30)).expect("anchor frame id fits i64")
                    * 40,
                time_base,
            }),
        },
    }
}

fn decoded(frame_id: u64) -> DecodedRgbaFrame {
    let indexed = entry(frame_id);
    let value = u8::try_from(frame_id % 251).expect("modulo value fits u8");
    DecodedRgbaFrame {
        frame: DecodedFrame {
            source_id: 1,
            stream_index: 0,
            decode_epoch: 0,
            index: frame_id,
            presentation_timestamp: indexed.presentation_timestamp,
            duration: indexed.duration,
            keyframe: indexed.keyframe,
            corrupt: indexed.corrupt,
            width: 2,
            height: 1,
            pixel_format: Some("rgba".into()),
        },
        stride_bytes: 8,
        pixels: vec![value, 0, 0, 255, 0, value, 0, 255],
    }
}

fn complete_index(root: &Path, frame_count: u64) -> FrameIndex {
    let identity = FrameIndexStreamIdentity::from_stream(&stream()).expect("valid stream identity");
    let source = SourceIdentity::new(
        frame_count,
        None,
        Some(format!(
            "phase7-large-stream-{}",
            NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
        )),
    );
    let (mut index, disposition) =
        FrameIndex::open_or_create(root.join("index.sqlite3"), source, identity)
            .expect("create stress index");
    assert_eq!(disposition, FrameIndexOpenDisposition::Created);

    let mut start = 0;
    while start < frame_count {
        let end = (start + INDEX_BATCH_SIZE).min(frame_count);
        let batch: Vec<_> = (start..end).map(entry).collect();
        assert!(u64::try_from(batch.len()).expect("batch length fits u64") <= INDEX_BATCH_SIZE);
        index
            .append_batch(&batch)
            .expect("append bounded index batch");
        start = end;
    }
    index.mark_complete().expect("complete stress index");
    index
}

fn stress_frame_count() -> u64 {
    std::env::var("FRAMESCOPE_PHASE7_STRESS_FRAMES")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value >= SAMPLE_EVERY_N)
        .unwrap_or(DEFAULT_STRESS_FRAMES)
}

#[test]
#[ignore = "Phase 7 large-stream stress runs explicitly in the Phase 7 workflow"]
fn phase7_large_streaming_export_stays_bounded() {
    let frame_count = stress_frame_count();
    let root = temp_root();
    let index = complete_index(&root, frame_count);
    let counters = DecoderCounters::new();
    let mut sink = CountingSink::new();
    let mut manifest = CountingWriter::default();

    let report = export_request(
        &index,
        ExtractionRequest {
            selection: ExtractionSelection::AllFrames,
            sampling: ExtractionSampling::EveryNthFrame(
                NonZeroU64::new(SAMPLE_EVERY_N).expect("non-zero sampling stride"),
            ),
        },
        ExtractionImageFormat::Png,
        {
            let counters = counters.clone();
            move || {
                counters.opens.set(counters.opens.get() + 1);
                Ok::<_, FrameScopeError>(GeneratedDecoder {
                    stream: stream(),
                    next_frame: 0,
                    frame_count,
                    counters: counters.clone(),
                })
            }
        },
        || false,
        &mut sink,
        &mut manifest,
    )
    .expect("large streaming export must complete");

    let expected_selected = ((frame_count - 1) / SAMPLE_EVERY_N) + 1;
    let last_selected_frame = (expected_selected - 1) * SAMPLE_EVERY_N;
    let expected_decoded = last_selected_frame + 1;
    assert_eq!(report.plan.selected_count, expected_selected);
    assert_eq!(report.committed_frames, expected_selected);
    assert_eq!(report.batch.selected_frames, expected_selected);
    assert_eq!(report.batch.decoded_frames, expected_decoded);
    assert_eq!(sink.committed, expected_selected);
    assert_eq!(sink.last_frame_id, Some(last_selected_frame));
    assert_eq!(counters.opens.get(), 1);
    assert_eq!(counters.seeks.get(), 1);
    assert_eq!(counters.nexts.get(), expected_decoded);
    assert_eq!(report.encoded_bytes, expected_selected * 8);
    assert!(manifest.bytes > 0);
    assert!(manifest.writes > expected_selected);
    assert!(manifest.max_write_bytes < 64 * 1024);

    println!(
        "phase7 large-stream frames={frame_count} selected={expected_selected} \
         last_selected={last_selected_frame} decoded={expected_decoded} \
         decoder_opens={} decoder_seeks={} decoder_nexts={} index_batch_limit={} \
         manifest_bytes={} manifest_writes={} manifest_max_write_bytes={}",
        counters.opens.get(),
        counters.seeks.get(),
        counters.nexts.get(),
        INDEX_BATCH_SIZE,
        manifest.bytes,
        manifest.writes,
        manifest.max_write_bytes,
    );

    drop(index);
    let _ = std::fs::remove_dir_all(root);
}
