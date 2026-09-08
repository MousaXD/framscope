//! Streaming image-output orchestration for Phase 6 extraction.
//!
//! This crate composes the accepted constant-memory selection planner, source-quality sequential
//! decoder, image encoder contract, and JSONL manifest without owning Android storage. A caller
//! provides a sink that commits exactly one encoded frame at a time. Successfully committed outputs
//! are appended to the manifest only after the sink returns, so a partial export never claims an
//! artifact that was not committed.

use framescope_cache::{FrameIndex, FrameIndexEntry, OwnedRgbaFrame};
use framescope_extraction::{
    ExtractionPlan, ExtractionPlanError, ExtractionRequest, ExtractionSampling, ExtractionSelection,
    ExtractionProgress, plan_extraction,
};
use framescope_extraction_image::{EncodedImageReport, ExtractionImageFormat, frame_file_name};
use framescope_extraction_manifest::{
    ExtractionManifestWriter, ManifestError, ManifestFrame, ManifestHeader, ManifestSelection,
};
use framescope_extraction_video::{
    BatchExtractionError, BatchExtractionFailure, BatchExtractionReport, extract_rgba_plan,
};
use framescope_video::{FrameScopeError, RgbaNavigationDecoder};
use std::error::Error;
use std::io::Write;
use thiserror::Error;

/// One source-quality frame offered to a streaming output sink.
///
/// The references are valid only for the duration of `FrameOutputSink::write_frame`; sinks that need
/// metadata later must copy the small scalar fields, not retain the RGBA payload.
pub struct FrameOutput<'a> {
    pub progress: ExtractionProgress,
    pub index_entry: &'a FrameIndexEntry,
    pub file_name: &'a str,
    pub format: ExtractionImageFormat,
    pub pixels: &'a OwnedRgbaFrame,
}

/// Storage boundary for a single selected frame.
///
/// Implementations must either commit the complete artifact before returning `Ok`, or leave no
/// committed artifact when returning `Err`. Android SAF adapters can satisfy this by creating a new
/// document, encoding through its file descriptor, committing on success, and deleting on failure.
pub trait FrameOutputSink {
    type Error: Error + 'static;

    fn write_frame(
        &mut self,
        output: FrameOutput<'_>,
    ) -> Result<EncodedImageReport, Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamingExtractionReport {
    pub plan: ExtractionPlan,
    pub batch: BatchExtractionReport,
    pub committed_frames: u64,
    pub encoded_bytes: u64,
}

#[derive(Debug, Error)]
pub enum StreamingExtractionError<E>
where
    E: Error + 'static,
{
    #[error(transparent)]
    Plan(#[from] ExtractionPlanError),
    #[error("source-quality batch extraction failed: {0}")]
    Batch(BatchExtractionFailure),
    #[error("frame output sink failed: {0}")]
    Output(#[source] E),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("frame output sink violated the encoding contract: {0}")]
    SinkContract(&'static str),
    #[error("streaming extraction was cancelled")]
    Cancelled,
}

enum VisitorError<E> {
    Output(E),
    Manifest(ManifestError),
    SinkContract(&'static str),
}

/// Plan, decode, encode/commit through `sink`, and record a streaming manifest.
///
/// No selected-frame vector, encoded-image batch, or manifest record list is materialized. The
/// accepted batch decoder owns at most one source-quality RGBA frame for delivery at a time. The
/// manifest is append-only and receives a terminal record for cancellation, decoder failure, sink
/// failure, or sink-contract failure whenever its writer remains usable.
pub fn export_request<D, F, C, S, M>(
    index: &FrameIndex,
    request: ExtractionRequest,
    format: ExtractionImageFormat,
    mut open_fresh_decoder: F,
    mut is_cancelled: C,
    sink: &mut S,
    manifest_output: &mut M,
) -> Result<StreamingExtractionReport, StreamingExtractionError<S::Error>>
where
    D: RgbaNavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
    C: FnMut() -> bool,
    S: FrameOutputSink,
    M: Write,
{
    let plan = plan_extraction(index, request)?;
    let header = ManifestHeader::new(manifest_selection(request), format, plan.selected_count)?;
    let mut manifest = ExtractionManifestWriter::new(manifest_output, header)?;

    let batch_result = extract_rgba_plan(
        index,
        plan,
        &mut open_fresh_decoder,
        &mut is_cancelled,
        |frame| {
            let file_name = frame_file_name(frame.progress.frame_id, format);
            let report = sink
                .write_frame(FrameOutput {
                    progress: frame.progress,
                    index_entry: &frame.index_entry,
                    file_name: &file_name,
                    format,
                    pixels: &frame.pixels,
                })
                .map_err(VisitorError::Output)?;
            if report.format != format {
                return Err(VisitorError::SinkContract(
                    "reported image format differs from the requested format",
                ));
            }
            if report.byte_len == 0 {
                return Err(VisitorError::SinkContract(
                    "committed image report has a zero byte count",
                ));
            }
            manifest
                .write_frame(ManifestFrame {
                    progress: frame.progress,
                    index_entry: &frame.index_entry,
                    file_name: &file_name,
                    encoded_bytes: report.byte_len,
                    group: None,
                })
                .map_err(VisitorError::Manifest)
        },
    );

    let batch = match batch_result {
        Ok(report) => report,
        Err(BatchExtractionError::Failure(BatchExtractionFailure::Cancelled)) => {
            manifest.finish_cancelled(Some("streaming extraction was cancelled"))?;
            return Err(StreamingExtractionError::Cancelled);
        }
        Err(BatchExtractionError::Failure(error)) => {
            let message = error.to_string();
            manifest.finish_failed("decode_error", Some(&message))?;
            return Err(StreamingExtractionError::Batch(error));
        }
        Err(BatchExtractionError::Visitor(VisitorError::Output(error))) => {
            let message = error.to_string();
            manifest.finish_failed("output_error", Some(&message))?;
            return Err(StreamingExtractionError::Output(error));
        }
        Err(BatchExtractionError::Visitor(VisitorError::SinkContract(message))) => {
            manifest.finish_failed("output_contract_error", Some(message))?;
            return Err(StreamingExtractionError::SinkContract(message));
        }
        Err(BatchExtractionError::Visitor(VisitorError::Manifest(error))) => {
            return Err(StreamingExtractionError::Manifest(error));
        }
    };

    let committed_frames = manifest.emitted_frames();
    let encoded_bytes = manifest.encoded_bytes();
    manifest.finish_complete()?;
    Ok(StreamingExtractionReport {
        plan,
        batch,
        committed_frames,
        encoded_bytes,
    })
}

fn manifest_selection(request: ExtractionRequest) -> ManifestSelection {
    let every_n = request.sampling.stride_frames().get();
    match request.selection {
        ExtractionSelection::CurrentFrame(frame_id) => ManifestSelection::CurrentFrame {
            frame_id: frame_id.0,
        },
        ExtractionSelection::FrameRangeInclusive { start, end } => {
            ManifestSelection::FrameRangeInclusive {
                start_frame: start.0,
                end_frame: end.0,
                every_n,
            }
        }
        ExtractionSelection::TimestampRangeUsInclusive { start_us, end_us } => {
            ManifestSelection::TimeRangeMicroseconds {
                start_us,
                end_us,
                every_n,
            }
        }
        ExtractionSelection::AllFrames => ManifestSelection::AllFrames { every_n },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{
        FrameId, FrameIndexEntry, FrameIndexOpenDisposition, FrameIndexStreamIdentity,
        KeyframeAnchor, SourceIdentity,
    };
    use framescope_core::{MediaDuration, MediaKind, MediaTimestamp, TimeBase};
    use framescope_extraction::ExtractionSampling;
    use framescope_extraction_image::{ImageExportError, encode_frame};
    use framescope_video::{CodecInfo, DecodedFrame, StreamInfo, ffmpeg::DecodedRgbaFrame};
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::num::NonZeroU64;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

    #[derive(Clone)]
    struct DecodeCounters {
        opens: Rc<Cell<u64>>,
        nexts: Rc<Cell<u64>>,
    }

    struct FakeDecoder {
        stream: StreamInfo,
        frames: VecDeque<DecodedRgbaFrame>,
        counters: DecodeCounters,
    }

    impl RgbaNavigationDecoder for FakeDecoder {
        fn selected_stream_for_rgba_navigation(&self) -> &StreamInfo {
            &self.stream
        }

        fn next_rgba_for_navigation(
            &mut self,
        ) -> Result<Option<DecodedRgbaFrame>, FrameScopeError> {
            self.counters.nexts.set(self.counters.nexts.get() + 1);
            Ok(self.frames.pop_front())
        }

        fn seek_for_rgba_navigation(&mut self, timestamp_us: i64) -> Result<(), FrameScopeError> {
            while self.frames.front().is_some_and(|frame| {
                frame
                    .frame
                    .presentation_timestamp
                    .and_then(|value| value.to_microseconds())
                    .is_some_and(|value| value < timestamp_us)
            }) {
                self.frames.pop_front();
            }
            Ok(())
        }
    }

    #[derive(Debug, Error)]
    enum SinkError {
        #[error("forced sink failure")]
        Forced,
        #[error(transparent)]
        Image(#[from] ImageExportError),
    }

    struct MemorySink {
        outputs: Vec<(String, Vec<u8>)>,
        fail_on_ordinal: Option<u64>,
        committed: Rc<Cell<u64>>,
        wrong_format: bool,
    }

    impl MemorySink {
        fn new(committed: Rc<Cell<u64>>) -> Self {
            Self {
                outputs: Vec::new(),
                fail_on_ordinal: None,
                committed,
                wrong_format: false,
            }
        }
    }

    impl FrameOutputSink for MemorySink {
        type Error = SinkError;

        fn write_frame(
            &mut self,
            output: FrameOutput<'_>,
        ) -> Result<EncodedImageReport, Self::Error> {
            if self.fail_on_ordinal == Some(output.progress.ordinal) {
                return Err(SinkError::Forced);
            }
            let mut bytes = Vec::new();
            let mut report = encode_frame(output.pixels, output.format, &mut bytes)?;
            self.outputs.push((output.file_name.to_owned(), bytes));
            self.committed.set(self.committed.get() + 1);
            if self.wrong_format {
                report.format = match output.format {
                    ExtractionImageFormat::Png => ExtractionImageFormat::WebPLossless,
                    _ => ExtractionImageFormat::Png,
                };
            }
            Ok(report)
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "framescope-extraction-output-{label}-{}-{id}",
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
        let time_base = TimeBase::new(1, 1_000).unwrap();
        let timestamp = MediaTimestamp {
            ticks: i64::try_from(frame_id).unwrap() * 40,
            time_base,
        };
        FrameIndexEntry {
            frame_id: FrameId(frame_id),
            presentation_timestamp: Some(timestamp),
            duration: Some(MediaDuration {
                ticks: 40,
                time_base,
            }),
            keyframe: true,
            corrupt: false,
            anchor: KeyframeAnchor::Keyframe {
                frame_id: FrameId(frame_id),
                presentation_timestamp: Some(timestamp),
            },
        }
    }

    fn decoded(frame_id: u64) -> DecodedRgbaFrame {
        let indexed = entry(frame_id);
        let value = u8::try_from(frame_id + 1).unwrap();
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
        let identity = FrameIndexStreamIdentity::from_stream(&stream()).unwrap();
        let source = SourceIdentity::new(
            123_456,
            None,
            Some(format!(
                "output-test-{}",
                NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
            )),
        );
        let (mut index, disposition) =
            FrameIndex::open_or_create(root.join("index.sqlite3"), source, identity).unwrap();
        assert_eq!(disposition, FrameIndexOpenDisposition::Created);
        let entries: Vec<_> = (0..frame_count).map(entry).collect();
        index.append_batch(&entries).unwrap();
        index.mark_complete().unwrap();
        index
    }

    fn decoder_source(frame_count: u64) -> Vec<DecodedRgbaFrame> {
        (0..frame_count).map(decoded).collect()
    }

    fn counters() -> DecodeCounters {
        DecodeCounters {
            opens: Rc::new(Cell::new(0)),
            nexts: Rc::new(Cell::new(0)),
        }
    }

    #[test]
    fn every_n_export_uses_one_decoder_and_records_only_committed_frames() {
        let root = temp_root("every-n");
        let index = complete_index(&root, 5);
        let source = decoder_source(5);
        let counters = counters();
        let committed = Rc::new(Cell::new(0));
        let mut sink = MemorySink::new(committed);
        let mut manifest = Vec::new();

        let report = export_request(
            &index,
            ExtractionRequest {
                selection: ExtractionSelection::AllFrames,
                sampling: ExtractionSampling::EveryNthFrame(NonZeroU64::new(2).unwrap()),
            },
            ExtractionImageFormat::Png,
            {
                let counters = counters.clone();
                move || {
                    counters.opens.set(counters.opens.get() + 1);
                    Ok::<_, FrameScopeError>(FakeDecoder {
                        stream: stream(),
                        frames: source.clone().into(),
                        counters: counters.clone(),
                    })
                }
            },
            || false,
            &mut sink,
            &mut manifest,
        )
        .unwrap();

        assert_eq!(report.committed_frames, 3);
        assert_eq!(report.encoded_bytes, report.batch.selected_frames * sink.outputs[0].1.len() as u64);
        assert_eq!(counters.opens.get(), 1);
        assert_eq!(counters.nexts.get(), 5);
        assert_eq!(
            sink.outputs.iter().map(|output| output.0.as_str()).collect::<Vec<_>>(),
            vec![
                "frame_00000000000000000000.png",
                "frame_00000000000000000002.png",
                "frame_00000000000000000004.png",
            ]
        );

        let text = String::from_utf8(manifest).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 5);
        let header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(header["selection"]["kind"], "all_frames");
        assert_eq!(header["selection"]["every_n"], 2);
        let terminal: serde_json::Value = serde_json::from_str(lines[4]).unwrap();
        assert_eq!(terminal["status"], "complete");
        assert_eq!(terminal["emitted_frames"], 3);

        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sink_failure_preserves_committed_prefix_and_writes_failed_terminal() {
        let root = temp_root("sink-failure");
        let index = complete_index(&root, 4);
        let source = decoder_source(4);
        let counters = counters();
        let committed = Rc::new(Cell::new(0));
        let mut sink = MemorySink::new(committed.clone());
        sink.fail_on_ordinal = Some(2);
        let mut manifest = Vec::new();

        let result = export_request(
            &index,
            ExtractionRequest::all_frames(),
            ExtractionImageFormat::Png,
            {
                let counters = counters.clone();
                move || {
                    counters.opens.set(counters.opens.get() + 1);
                    Ok::<_, FrameScopeError>(FakeDecoder {
                        stream: stream(),
                        frames: source.clone().into(),
                        counters: counters.clone(),
                    })
                }
            },
            || false,
            &mut sink,
            &mut manifest,
        );

        assert!(matches!(result, Err(StreamingExtractionError::Output(SinkError::Forced))));
        assert_eq!(committed.get(), 1);
        assert_eq!(sink.outputs.len(), 1);
        let text = String::from_utf8(manifest).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        let terminal: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(terminal["status"], "failed");
        assert_eq!(terminal["code"], "output_error");
        assert_eq!(terminal["emitted_frames"], 1);

        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cancellation_after_first_commit_stops_before_next_decode_and_marks_manifest() {
        let root = temp_root("cancel");
        let index = complete_index(&root, 4);
        let source = decoder_source(4);
        let counters = counters();
        let committed = Rc::new(Cell::new(0));
        let mut sink = MemorySink::new(committed.clone());
        let mut manifest = Vec::new();

        let result = export_request(
            &index,
            ExtractionRequest::all_frames(),
            ExtractionImageFormat::Png,
            {
                let counters = counters.clone();
                move || {
                    counters.opens.set(counters.opens.get() + 1);
                    Ok::<_, FrameScopeError>(FakeDecoder {
                        stream: stream(),
                        frames: source.clone().into(),
                        counters: counters.clone(),
                    })
                }
            },
            {
                let committed = committed.clone();
                move || committed.get() >= 1
            },
            &mut sink,
            &mut manifest,
        );

        assert!(matches!(result, Err(StreamingExtractionError::Cancelled)));
        assert_eq!(committed.get(), 1);
        assert_eq!(counters.nexts.get(), 1);
        let text = String::from_utf8(manifest).unwrap();
        let terminal: serde_json::Value =
            serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(terminal["status"], "cancelled");
        assert_eq!(terminal["emitted_frames"], 1);

        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sink_cannot_lie_about_committed_format() {
        let root = temp_root("contract");
        let index = complete_index(&root, 1);
        let source = decoder_source(1);
        let counters = counters();
        let committed = Rc::new(Cell::new(0));
        let mut sink = MemorySink::new(committed);
        sink.wrong_format = true;
        let mut manifest = Vec::new();

        let result = export_request(
            &index,
            ExtractionRequest::all_frames(),
            ExtractionImageFormat::Png,
            {
                let counters = counters.clone();
                move || {
                    counters.opens.set(counters.opens.get() + 1);
                    Ok::<_, FrameScopeError>(FakeDecoder {
                        stream: stream(),
                        frames: source.clone().into(),
                        counters: counters.clone(),
                    })
                }
            },
            || false,
            &mut sink,
            &mut manifest,
        );

        assert!(matches!(result, Err(StreamingExtractionError::SinkContract(_))));
        let text = String::from_utf8(manifest).unwrap();
        let terminal: serde_json::Value =
            serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(terminal["status"], "failed");
        assert_eq!(terminal["code"], "output_contract_error");
        assert_eq!(terminal["emitted_frames"], 0);

        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }
}
