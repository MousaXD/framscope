//! Sequential source-quality export of Phase 4 similarity-group representatives.
//!
//! This adapter deliberately composes the accepted group cursor with the accepted Phase 6 batch
//! decoder. It does not implement a second seek/timeline engine. The batch decoder scans the bounded
//! frame span once; this layer forwards only representative frames to the caller.

use framescope_cache::{
    FrameId, FrameIndex, FrameIndexEntry, FrameIndexError, FrameIndexLifecycle, OwnedRgbaFrame,
};
use framescope_extraction::{
    ExtractionPlanError, ExtractionProgress, ExtractionRequest, ExtractionSampling,
    ExtractionSelection, plan_extraction,
};
use framescope_extraction_groups::{GroupRepresentativeCursor, GroupRepresentativeError};
use framescope_extraction_video::{
    BatchExtractionError, BatchExtractionFailure, ExtractedRgbaFrame, extract_rgba_plan,
};
use framescope_group_navigation::{
    GroupNavigationError, GroupNavigationTarget, SimilarityGroupNavigator,
};
use framescope_video::{FrameScopeError, RgbaNavigationDecoder};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedGroupRepresentative {
    pub progress: ExtractionProgress,
    pub group: GroupNavigationTarget,
    pub index_entry: FrameIndexEntry,
    pub pixels: OwnedRgbaFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupBatchExtractionReport {
    pub representative_frames: u64,
    pub scanned_frames: u64,
    pub decoded_frames: u64,
    pub used_keyframe_seek: bool,
    pub fell_back_to_stream_start: bool,
}

#[derive(Debug, Error)]
pub enum GroupBatchExtractionError<E> {
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
    #[error("source-quality batch extraction failed: {0}")]
    Batch(BatchExtractionFailure),
    #[error("group representative visitor failed")]
    Visitor(E),
    #[error("group representative extraction invariant failed: {0}")]
    InvalidNavigation(String),
}

enum FilterVisitorError<E> {
    Groups(GroupRepresentativeError),
    Visitor(E),
    InvalidNavigation(String),
}

/// Export exactly one source-quality RGBA frame per persisted similarity group.
///
/// The decoder scans only from the first representative through the final representative. Frames
/// between representatives are decoded and timeline-verified by `framescope-extraction-video`, then
/// immediately discarded here. No representative list is materialized and no per-group random seek
/// is performed.
pub fn extract_group_representatives<D, F, C, V, E>(
    index: &FrameIndex,
    navigator: &SimilarityGroupNavigator,
    open_fresh_decoder: F,
    is_cancelled: C,
    mut visitor: V,
) -> Result<GroupBatchExtractionReport, GroupBatchExtractionError<E>>
where
    D: RgbaNavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
    C: FnMut() -> bool,
    V: FnMut(ExtractedGroupRepresentative) -> Result<(), E>,
{
    let status = index.status()?;
    if status.lifecycle != FrameIndexLifecycle::Complete {
        return Err(GroupBatchExtractionError::IncompleteIndex);
    }
    let frame_count = status
        .frame_count
        .ok_or(GroupBatchExtractionError::IncompleteIndex)?;
    if frame_count == 0 {
        return Err(GroupBatchExtractionError::EmptyIndex);
    }

    let group_count = navigator.group_count();
    if group_count == 0 {
        return Err(GroupBatchExtractionError::NoGroups);
    }

    let mut cursor = GroupRepresentativeCursor::new(navigator);
    let first_group = cursor
        .next_group()?
        .ok_or(GroupBatchExtractionError::NoGroups)?;
    let last_frame = FrameId(frame_count - 1);
    let last_group = navigator.group_containing(last_frame)?;
    if last_group.group_count != group_count
        || last_group.ordinal.checked_add(1) != Some(group_count)
    {
        return Err(GroupBatchExtractionError::InvalidNavigation(format!(
            "last indexed frame resolved to group {} of {}, expected final group",
            last_group.ordinal, group_count
        )));
    }
    if last_group.representative_frame.0 < first_group.representative_frame.0 {
        return Err(GroupBatchExtractionError::InvalidNavigation(
            "final representative precedes first representative".into(),
        ));
    }

    let plan = plan_extraction(
        index,
        ExtractionRequest {
            selection: ExtractionSelection::FrameRangeInclusive {
                start: first_group.representative_frame,
                end: last_group.representative_frame,
            },
            sampling: ExtractionSampling::EveryFrame,
        },
    )?;

    let mut next_group = Some(first_group);
    let mut representative_frames = 0_u64;
    let batch_result = extract_rgba_plan(
        index,
        plan,
        open_fresh_decoder,
        is_cancelled,
        |frame| {
            filter_representative(
                frame,
                &mut cursor,
                &mut next_group,
                &mut representative_frames,
                group_count,
                &mut visitor,
            )
        },
    );

    let batch = match batch_result {
        Ok(report) => report,
        Err(BatchExtractionError::Failure(error)) => {
            return Err(GroupBatchExtractionError::Batch(error));
        }
        Err(BatchExtractionError::Visitor(FilterVisitorError::Groups(error))) => {
            return Err(GroupBatchExtractionError::Groups(error));
        }
        Err(BatchExtractionError::Visitor(FilterVisitorError::Visitor(error))) => {
            return Err(GroupBatchExtractionError::Visitor(error));
        }
        Err(BatchExtractionError::Visitor(
            FilterVisitorError::InvalidNavigation(message),
        )) => return Err(GroupBatchExtractionError::InvalidNavigation(message)),
    };

    if representative_frames != group_count || next_group.is_some() || cursor.remaining() != 0 {
        return Err(GroupBatchExtractionError::InvalidNavigation(format!(
            "export emitted {representative_frames} representatives for {group_count} groups"
        )));
    }

    Ok(GroupBatchExtractionReport {
        representative_frames,
        scanned_frames: batch.selected_frames,
        decoded_frames: batch.decoded_frames,
        used_keyframe_seek: batch.used_keyframe_seek,
        fell_back_to_stream_start: batch.fell_back_to_stream_start,
    })
}

fn filter_representative<E, V>(
    frame: ExtractedRgbaFrame,
    cursor: &mut GroupRepresentativeCursor<'_>,
    next_group: &mut Option<GroupNavigationTarget>,
    representative_frames: &mut u64,
    group_count: u64,
    visitor: &mut V,
) -> Result<(), FilterVisitorError<E>>
where
    V: FnMut(ExtractedGroupRepresentative) -> Result<(), E>,
{
    let Some(expected) = next_group.as_ref() else {
        return Err(FilterVisitorError::InvalidNavigation(
            "batch decoder produced frames after the final representative".into(),
        ));
    };
    let frame_id = frame.progress.frame_id;
    if frame_id.0 < expected.representative_frame.0 {
        return Ok(());
    }
    if frame_id != expected.representative_frame {
        return Err(FilterVisitorError::InvalidNavigation(format!(
            "decoder passed representative {} at frame {}",
            expected.representative_frame.0, frame_id.0
        )));
    }

    let group = next_group.take().ok_or_else(|| {
        FilterVisitorError::InvalidNavigation("representative target disappeared".into())
    })?;
    *representative_frames = representative_frames.checked_add(1).ok_or_else(|| {
        FilterVisitorError::InvalidNavigation("representative progress overflow".into())
    })?;
    visitor(ExtractedGroupRepresentative {
        progress: ExtractionProgress {
            frame_id,
            ordinal: *representative_frames,
            total: group_count,
        },
        group,
        index_entry: frame.index_entry,
        pixels: frame.pixels,
    })
    .map_err(FilterVisitorError::Visitor)?;

    *next_group = cursor
        .next_group()
        .map_err(FilterVisitorError::Groups)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{
        FrameIndexEntry, FrameIndexOpenDisposition, FrameIndexStreamIdentity, KeyframeAnchor,
        SourceIdentity,
    };
    use framescope_core::{MediaDuration, MediaKind, MediaTimestamp, TimeBase};
    use framescope_group_navigation::{
        IndexedRgbaFrame, IndexedRgbaStream, SimilaritySourceError, open_or_build_group_navigation,
    };
    use framescope_perceptual::HybridSimilarityPolicy;
    use framescope_video::{CodecInfo, DecodedFrame, StreamInfo, ffmpeg::DecodedRgbaFrame};
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

    struct FakeSimilarityStream {
        frames: VecDeque<IndexedRgbaFrame>,
    }

    impl IndexedRgbaStream for FakeSimilarityStream {
        fn next_frame(&mut self) -> Result<Option<IndexedRgbaFrame>, SimilaritySourceError> {
            Ok(self.frames.pop_front())
        }
    }

    #[derive(Clone)]
    struct DecodeCounters {
        nexts: Rc<Cell<u64>>,
        seeks: Rc<Cell<u64>>,
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
            self.counters.seeks.set(self.counters.seeks.get() + 1);
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

    fn temp_root(label: &str) -> PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "framescope-extraction-groups-video-{label}-{}-{id}",
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
            width: Some(9),
            height: Some(8),
            pixel_format: Some("rgba".into()),
            average_frame_rate: None,
            nominal_frame_rate: None,
            rotation_degrees: None,
        }
    }

    fn timestamp(frame_id: u64) -> MediaTimestamp {
        MediaTimestamp {
            ticks: i64::try_from(frame_id).unwrap() * 40,
            time_base: TimeBase::new(1, 1_000).unwrap(),
        }
    }

    fn entry(frame_id: u64) -> FrameIndexEntry {
        let timestamp = timestamp(frame_id);
        FrameIndexEntry {
            frame_id: FrameId(frame_id),
            presentation_timestamp: Some(timestamp),
            duration: Some(MediaDuration {
                ticks: 40,
                time_base: timestamp.time_base,
            }),
            keyframe: true,
            corrupt: false,
            anchor: KeyframeAnchor::Keyframe {
                frame_id: FrameId(frame_id),
                presentation_timestamp: Some(timestamp),
            },
        }
    }

    fn pixels(value: u8) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(9 * 8 * 4);
        for _ in 0..(9 * 8) {
            bytes.extend_from_slice(&[value, value, value, 255]);
        }
        bytes
    }

    fn owned(value: u8) -> OwnedRgbaFrame {
        OwnedRgbaFrame::new(9, 8, 36, pixels(value)).unwrap()
    }

    fn indexed_frame(frame_id: u64, value: u8) -> IndexedRgbaFrame {
        let entry = entry(frame_id);
        IndexedRgbaFrame {
            frame_id: entry.frame_id,
            presentation_timestamp: entry.presentation_timestamp,
            duration: entry.duration,
            keyframe: entry.keyframe,
            corrupt: entry.corrupt,
            pixels: owned(value),
        }
    }

    fn decoded_frame(frame_id: u64, value: u8) -> DecodedRgbaFrame {
        let entry = entry(frame_id);
        DecodedRgbaFrame {
            frame: DecodedFrame {
                source_id: 1,
                stream_index: 0,
                decode_epoch: 0,
                index: frame_id,
                presentation_timestamp: entry.presentation_timestamp,
                duration: entry.duration,
                keyframe: entry.keyframe,
                corrupt: entry.corrupt,
                width: 9,
                height: 8,
                pixel_format: Some("rgba".into()),
            },
            stride_bytes: 36,
            pixels: pixels(value),
        }
    }

    fn similarity_stream(values: &[u8]) -> FakeSimilarityStream {
        FakeSimilarityStream {
            frames: values
                .iter()
                .enumerate()
                .map(|(frame_id, value)| indexed_frame(frame_id as u64, *value))
                .collect(),
        }
    }

    fn decoded_frames(values: &[u8]) -> Vec<DecodedRgbaFrame> {
        values
            .iter()
            .enumerate()
            .map(|(frame_id, value)| decoded_frame(frame_id as u64, *value))
            .collect()
    }

    fn complete_index(root: &Path, frame_count: u64) -> FrameIndex {
        let identity = FrameIndexStreamIdentity::from_stream(&stream()).unwrap();
        let source = SourceIdentity::new(
            22_222,
            None,
            Some(format!(
                "group-batch-test-{}",
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

    fn policy() -> HybridSimilarityPolicy {
        HybridSimilarityPolicy {
            max_hash_distance: 8,
            minimum_luma_similarity: 9_700,
        }
    }

    fn counters() -> DecodeCounters {
        DecodeCounters {
            nexts: Rc::new(Cell::new(0)),
            seeks: Rc::new(Cell::new(0)),
        }
    }

    fn fake_decoder(frames: Vec<DecodedRgbaFrame>, counters: DecodeCounters) -> FakeDecoder {
        FakeDecoder {
            stream: stream(),
            frames: frames.into(),
            counters,
        }
    }

    #[test]
    fn unique_export_reuses_one_forward_batch_decode_and_stops_at_last_representative() {
        let root = temp_root("ordered");
        let values = [10, 10, 240, 240, 80, 80];
        let index = complete_index(&root, values.len() as u64);
        let analysis = open_or_build_group_navigation(
            &index,
            root.join("similarity"),
            policy(),
            || Ok(similarity_stream(&values)),
            || false,
        )
        .unwrap();
        assert_eq!(analysis.navigator.group_count(), 3);

        let source = decoded_frames(&values);
        let counters = counters();
        let opened = Rc::new(Cell::new(0_u64));
        let mut emitted = Vec::new();
        let report = extract_group_representatives(
            &index,
            &analysis.navigator,
            {
                let counters = counters.clone();
                let opened = opened.clone();
                move || {
                    opened.set(opened.get() + 1);
                    Ok::<_, FrameScopeError>(fake_decoder(source.clone(), counters.clone()))
                }
            },
            || false,
            |frame| {
                emitted.push((
                    frame.progress.frame_id,
                    frame.progress.ordinal,
                    frame.progress.total,
                    frame.pixels.pixels()[0],
                ));
                Ok::<(), ()>(())
            },
        )
        .unwrap();

        assert_eq!(
            emitted,
            vec![
                (FrameId(0), 1, 3, 10),
                (FrameId(2), 2, 3, 240),
                (FrameId(4), 3, 3, 80),
            ]
        );
        assert_eq!(report.representative_frames, 3);
        assert_eq!(report.scanned_frames, 5);
        assert_eq!(report.decoded_frames, 5);
        assert_eq!(opened.get(), 1);
        assert_eq!(counters.nexts.get(), 5);
        assert_eq!(counters.seeks.get(), 1);

        drop(analysis);
        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cancellation_after_first_representative_stops_before_next_intermediate_decode() {
        let root = temp_root("cancel");
        let values = [10, 10, 240, 240];
        let index = complete_index(&root, values.len() as u64);
        let analysis = open_or_build_group_navigation(
            &index,
            root.join("similarity"),
            policy(),
            || Ok(similarity_stream(&values)),
            || false,
        )
        .unwrap();
        let source = decoded_frames(&values);
        let counters = counters();
        let emitted = Rc::new(Cell::new(0_u64));

        let result = extract_group_representatives(
            &index,
            &analysis.navigator,
            {
                let counters = counters.clone();
                move || Ok::<_, FrameScopeError>(fake_decoder(source.clone(), counters.clone()))
            },
            {
                let emitted = emitted.clone();
                move || emitted.get() >= 1
            },
            {
                let emitted = emitted.clone();
                move |_frame| {
                    emitted.set(emitted.get() + 1);
                    Ok::<(), ()>(())
                }
            },
        );

        assert!(matches!(
            result,
            Err(GroupBatchExtractionError::Batch(
                BatchExtractionFailure::Cancelled
            ))
        ));
        assert_eq!(emitted.get(), 1);
        assert_eq!(counters.nexts.get(), 1);

        drop(analysis);
        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }
}
