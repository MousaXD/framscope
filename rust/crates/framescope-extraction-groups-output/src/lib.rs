//! Streaming encoded output for unique similarity-group representatives.
//!
//! This crate composes the accepted Phase 4 representative cursor, the accepted Phase 6
//! source-quality sequential group decoder, the single-frame image sink, and the append-only JSONL
//! manifest. It never materializes a representative list or encoded-image batch.

use framescope_cache::{FrameIndex, FrameIndexError};
use framescope_extraction::ExtractionPlanError;
use framescope_extraction_groups::GroupRepresentativeError;
use framescope_extraction_groups_video::{
    ExtractedGroupRepresentative, GroupBatchExtractionError, GroupBatchExtractionReport,
    extract_group_representatives,
};
use framescope_extraction_image::{ExtractionImageFormat, frame_file_name};
use framescope_extraction_manifest::{
    ExtractionManifestWriter, ManifestError, ManifestFrame, ManifestGroup, ManifestHeader,
    ManifestSelection,
};
use framescope_extraction_output::{FrameOutput, FrameOutputSink};
use framescope_extraction_video::BatchExtractionFailure;
use framescope_group_navigation::{GroupNavigationError, SimilarityGroupNavigator};
use framescope_video::{FrameScopeError, RgbaNavigationDecoder};
use std::error::Error;
use std::io::Write;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupStreamingExtractionReport {
    pub batch: GroupBatchExtractionReport,
    pub committed_frames: u64,
    pub encoded_bytes: u64,
}

#[derive(Debug, Error)]
pub enum GroupStreamingExtractionError<E>
where
    E: Error + 'static,
{
    #[error("group representative extraction requires a complete frame index")]
    IncompleteIndex,
    #[error("group representative extraction has no indexed frames")]
    EmptyIndex,
    #[error("no persisted similarity groups are available for unique-frame export")]
    NoGroups,
    #[error(transparent)]
    Index(#[from] FrameIndexError),
    #[error(transparent)]
    Plan(#[from] ExtractionPlanError),
    #[error(transparent)]
    Navigation(#[from] GroupNavigationError),
    #[error(transparent)]
    Groups(#[from] GroupRepresentativeError),
    #[error("source-quality group extraction failed: {0}")]
    Batch(BatchExtractionFailure),
    #[error("group representative output sink failed: {0}")]
    Output(#[source] E),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("group representative output sink violated the encoding contract: {0}")]
    SinkContract(&'static str),
    #[error("group representative extraction invariant failed: {0}")]
    InvalidNavigation(String),
    #[error("group representative extraction was cancelled")]
    Cancelled,
}

enum VisitorError<E> {
    Output(E),
    Manifest(ManifestError),
    SinkContract(&'static str),
}

/// Encode and commit exactly one source-quality representative per persisted similarity group.
///
/// The caller owns storage through `sink`. Each artifact is committed before its manifest record is
/// appended, so a partial manifest never claims a frame that the storage layer rejected. The
/// underlying group adapter scans the indexed span once and discards non-representative frames
/// immediately.
pub fn export_group_representatives<D, F, C, S, M>(
    index: &FrameIndex,
    navigator: &SimilarityGroupNavigator,
    format: ExtractionImageFormat,
    open_fresh_decoder: F,
    is_cancelled: C,
    sink: &mut S,
    manifest_output: &mut M,
) -> Result<GroupStreamingExtractionReport, GroupStreamingExtractionError<S::Error>>
where
    D: RgbaNavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
    C: FnMut() -> bool,
    S: FrameOutputSink,
    M: Write,
{
    let expected_frames = navigator.group_count();
    if expected_frames == 0 {
        return Err(GroupStreamingExtractionError::NoGroups);
    }
    let header = ManifestHeader::new(
        ManifestSelection::UniqueGroupRepresentatives,
        format,
        expected_frames,
    )?;
    let mut manifest = ExtractionManifestWriter::new(manifest_output, header)?;

    let extraction = extract_group_representatives(
        index,
        navigator,
        open_fresh_decoder,
        is_cancelled,
        |frame| write_representative(frame, format, sink, &mut manifest),
    );

    let batch = match extraction {
        Ok(report) => report,
        Err(GroupBatchExtractionError::Batch(BatchExtractionFailure::Cancelled)) => {
            manifest.finish_cancelled(Some("group representative extraction was cancelled"))?;
            return Err(GroupStreamingExtractionError::Cancelled);
        }
        Err(GroupBatchExtractionError::Batch(error)) => {
            let message = error.to_string();
            manifest.finish_failed("decode_error", Some(&message))?;
            return Err(GroupStreamingExtractionError::Batch(error));
        }
        Err(GroupBatchExtractionError::Visitor(VisitorError::Output(error))) => {
            let message = error.to_string();
            manifest.finish_failed("output_error", Some(&message))?;
            return Err(GroupStreamingExtractionError::Output(error));
        }
        Err(GroupBatchExtractionError::Visitor(VisitorError::Manifest(error))) => {
            return Err(GroupStreamingExtractionError::Manifest(error));
        }
        Err(GroupBatchExtractionError::Visitor(VisitorError::SinkContract(message))) => {
            manifest.finish_failed("output_contract_error", Some(message))?;
            return Err(GroupStreamingExtractionError::SinkContract(message));
        }
        Err(GroupBatchExtractionError::IncompleteIndex) => {
            manifest.finish_failed("incomplete_index", None)?;
            return Err(GroupStreamingExtractionError::IncompleteIndex);
        }
        Err(GroupBatchExtractionError::EmptyIndex) => {
            manifest.finish_failed("empty_index", None)?;
            return Err(GroupStreamingExtractionError::EmptyIndex);
        }
        Err(GroupBatchExtractionError::NoGroups) => {
            manifest.finish_failed("no_groups", None)?;
            return Err(GroupStreamingExtractionError::NoGroups);
        }
        Err(GroupBatchExtractionError::Index(error)) => {
            let message = error.to_string();
            manifest.finish_failed("index_error", Some(&message))?;
            return Err(GroupStreamingExtractionError::Index(error));
        }
        Err(GroupBatchExtractionError::Plan(error)) => {
            let message = error.to_string();
            manifest.finish_failed("selection_error", Some(&message))?;
            return Err(GroupStreamingExtractionError::Plan(error));
        }
        Err(GroupBatchExtractionError::Navigation(error)) => {
            let message = error.to_string();
            manifest.finish_failed("group_navigation_error", Some(&message))?;
            return Err(GroupStreamingExtractionError::Navigation(error));
        }
        Err(GroupBatchExtractionError::Groups(error)) => {
            let message = error.to_string();
            manifest.finish_failed("group_navigation_error", Some(&message))?;
            return Err(GroupStreamingExtractionError::Groups(error));
        }
        Err(GroupBatchExtractionError::InvalidNavigation(message)) => {
            manifest.finish_failed("group_navigation_error", Some(&message))?;
            return Err(GroupStreamingExtractionError::InvalidNavigation(message));
        }
    };

    let committed_frames = manifest.emitted_frames();
    let encoded_bytes = manifest.encoded_bytes();
    if committed_frames != expected_frames || batch.representative_frames != expected_frames {
        let message = format!(
            "committed {committed_frames} representatives while {expected_frames} were expected"
        );
        manifest.finish_failed("group_output_contract_error", Some(&message))?;
        return Err(GroupStreamingExtractionError::InvalidNavigation(message));
    }
    manifest.finish_complete()?;
    Ok(GroupStreamingExtractionReport {
        batch,
        committed_frames,
        encoded_bytes,
    })
}

fn write_representative<S, W>(
    frame: ExtractedGroupRepresentative,
    format: ExtractionImageFormat,
    sink: &mut S,
    manifest: &mut ExtractionManifestWriter<W>,
) -> Result<(), VisitorError<S::Error>>
where
    S: FrameOutputSink,
    W: Write,
{
    let ExtractedGroupRepresentative {
        progress,
        group,
        index_entry,
        pixels,
    } = frame;
    let file_name = frame_file_name(progress.frame_id, format);
    let report = sink
        .write_frame(FrameOutput {
            progress,
            index_entry: &index_entry,
            file_name: &file_name,
            format,
            pixels: &pixels,
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
            progress,
            index_entry: &index_entry,
            file_name: &file_name,
            encoded_bytes: report.byte_len,
            group: Some(ManifestGroup {
                ordinal: group.ordinal,
                group_count: group.group_count,
                representative_frame: group.representative_frame.0,
                first_frame: group.first_frame.0,
                last_frame: group.last_frame.0,
                represented_frame_count: group.represented_frame_count,
            }),
        })
        .map_err(VisitorError::Manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{FrameId, FrameIndexEntry, KeyframeAnchor, OwnedRgbaFrame};
    use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};
    use framescope_extraction::ExtractionProgress;
    use framescope_extraction_image::EncodedImageReport;
    use framescope_group_navigation::GroupNavigationTarget;
    use std::convert::Infallible;

    struct RecordingSink {
        committed: Vec<(FrameId, String)>,
    }

    impl FrameOutputSink for RecordingSink {
        type Error = Infallible;

        fn write_frame(
            &mut self,
            output: FrameOutput<'_>,
        ) -> Result<EncodedImageReport, Self::Error> {
            self.committed
                .push((output.progress.frame_id, output.file_name.to_owned()));
            Ok(EncodedImageReport {
                format: output.format,
                byte_len: 123,
            })
        }
    }

    fn timestamp(ticks: i64) -> MediaTimestamp {
        MediaTimestamp {
            ticks,
            time_base: TimeBase::new(1, 1_000).unwrap(),
        }
    }

    #[test]
    fn representative_commit_precedes_manifest_record_and_preserves_group_identity() {
        let entry = FrameIndexEntry {
            frame_id: FrameId(7),
            presentation_timestamp: Some(timestamp(280)),
            duration: Some(MediaDuration {
                ticks: 40,
                time_base: TimeBase::new(1, 1_000).unwrap(),
            }),
            keyframe: false,
            corrupt: false,
            anchor: KeyframeAnchor::Unknown,
        };
        let group = GroupNavigationTarget {
            ordinal: 2,
            group_count: 4,
            representative_frame: FrameId(7),
            first_frame: FrameId(6),
            last_frame: FrameId(9),
            represented_frame_count: 4,
            start_timestamp: timestamp(240),
            end_timestamp: timestamp(360),
            start_duration: None,
            end_duration: None,
            representative_similarity_floor: 9_800,
            can_previous: true,
            can_next: true,
        };
        let frame = ExtractedGroupRepresentative {
            progress: ExtractionProgress {
                frame_id: FrameId(7),
                ordinal: 1,
                total: 1,
            },
            group,
            index_entry: entry,
            pixels: OwnedRgbaFrame::new(1, 1, 4, vec![10, 20, 30, 255]).unwrap(),
        };
        let header = ManifestHeader::new(
            ManifestSelection::UniqueGroupRepresentatives,
            ExtractionImageFormat::Png,
            1,
        )
        .unwrap();
        let mut bytes = Vec::new();
        let mut manifest = ExtractionManifestWriter::new(&mut bytes, header).unwrap();
        let mut sink = RecordingSink {
            committed: Vec::new(),
        };

        write_representative(
            frame,
            ExtractionImageFormat::Png,
            &mut sink,
            &mut manifest,
        )
        .unwrap();
        assert_eq!(sink.committed.len(), 1);
        assert_eq!(sink.committed[0].0, FrameId(7));
        assert_eq!(sink.committed[0].1, "frame_00000000000000000007.png");
        manifest.finish_complete().unwrap();
        drop(manifest);

        let jsonl = String::from_utf8(bytes).unwrap();
        assert!(jsonl.contains("\"kind\":\"unique_group_representatives\""));
        assert!(jsonl.contains("\"representative_frame\":7"));
        assert!(jsonl.contains("\"first_frame\":6"));
        assert!(jsonl.contains("\"last_frame\":9"));
        assert!(jsonl.contains("\"represented_frame_count\":4"));
        assert!(jsonl.contains("\"status\":\"complete\""));
    }
}
