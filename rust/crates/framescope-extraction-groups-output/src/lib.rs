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

fn write_representative<S>(
    frame: ExtractedGroupRepresentative,
    format: ExtractionImageFormat,
    sink: &mut S,
    manifest: &mut ExtractionManifestWriter<&mut impl Write>,
) -> Result<(), VisitorError<S::Error>>
where
    S: FrameOutputSink,
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
