//! Source-quality sequential batch extraction over an accepted FrameScope extraction plan.
//!
//! This adapter intentionally sits above `framescope-extraction`: selection stays independent from
//! the video engine, while this crate executes an already-resolved plan against one authoritative
//! source-quality RGBA decoder. It never reads lossy disk proxy bytes and never retains more than
//! one decoded frame on behalf of the caller.

use framescope_cache::{
    FrameCacheError, FrameId, FrameIndex, FrameIndexEntry, FrameIndexError, FrameIndexLifecycle,
    FrameIndexStreamIdentity, KeyframeAnchor, OwnedRgbaFrame,
};
use framescope_extraction::{ExtractionPlan, ExtractionProgress};
use framescope_video::{
    FrameScopeError, RgbaNavigationDecoder, StreamInfo, ffmpeg::DecodedRgbaFrame,
};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedRgbaFrame {
    pub progress: ExtractionProgress,
    pub index_entry: FrameIndexEntry,
    pub pixels: OwnedRgbaFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchExtractionReport {
    pub selected_frames: u64,
    pub decoded_frames: u64,
    pub used_keyframe_seek: bool,
    pub fell_back_to_stream_start: bool,
}

#[derive(Debug, Error)]
pub enum BatchExtractionFailure {
    #[error("batch extraction was cancelled")]
    Cancelled,
    #[error("video decode failed: {0}")]
    Decoder(#[from] FrameScopeError),
    #[error("frame-index lookup failed: {0}")]
    Index(#[from] FrameIndexError),
    #[error("source-quality RGBA validation failed: {0}")]
    Frame(#[from] FrameCacheError),
    #[error("batch extraction requires a complete frame index")]
    IncompleteIndex,
    #[error("extraction plan is invalid: {0}")]
    InvalidPlan(&'static str),
    #[error("fresh decoder stream does not match the stream bound to the frame index")]
    StreamIdentityMismatch,
    #[error("decoded presentation timeline diverged at frame {0}")]
    TimelineMismatch(u64),
    #[error("decoder reached EOF before the extraction plan completed")]
    UnexpectedEof,
    #[error("numeric overflow while advancing batch extraction")]
    NumericRange,
}

#[derive(Debug, Error)]
pub enum BatchExtractionError<E> {
    #[error(transparent)]
    Failure(#[from] BatchExtractionFailure),
    #[error("extraction visitor failed")]
    Visitor(E),
}

/// Execute a resolved extraction plan with one sequential source-quality RGBA decoder.
///
/// The first selected frame is reached through its persisted safe keyframe anchor when possible.
/// If seek alignment cannot be proven before any visitor side effect occurs, extraction safely
/// reopens the source and reconciles from stream start. After the first selected frame is emitted,
/// timeline divergence is returned as an error rather than retrying and risking duplicate output.
///
/// Frames between selected identities are decoded only to preserve authoritative timeline
/// reconciliation. They are immediately discarded. The adapter never materializes the whole plan
/// or retains output frames after the visitor returns.
pub fn extract_rgba_plan<D, F, C, V, E>(
    index: &FrameIndex,
    plan: ExtractionPlan,
    mut open_fresh_decoder: F,
    mut is_cancelled: C,
    mut visitor: V,
) -> Result<BatchExtractionReport, BatchExtractionError<E>>
where
    D: RgbaNavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
    C: FnMut() -> bool,
    V: FnMut(ExtractedRgbaFrame) -> Result<(), E>,
{
    let last_selected = validate_plan(index, plan)?;
    let first_entry = index
        .entry(plan.first_frame)
        .map_err(BatchExtractionFailure::Index)?
        .ok_or(BatchExtractionFailure::InvalidPlan(
            "first frame is absent from the complete index",
        ))?;

    let mut decoder = open_checked_decoder(index, &mut open_fresh_decoder)?;
    let mut decoded_frames = 0_u64;
    let mut used_keyframe_seek = false;
    let mut fell_back_to_stream_start = false;

    let mut decoded = if let KeyframeAnchor::Keyframe {
        frame_id: anchor_id,
        presentation_timestamp: Some(anchor_timestamp),
    } = &first_entry.anchor
    {
        if let Some(timestamp_us) = anchor_timestamp
            .to_microseconds()
            .filter(|value| *value >= 0)
        {
            used_keyframe_seek = true;
            decoder
                .seek_for_rgba_navigation(timestamp_us)
                .map_err(BatchExtractionFailure::Decoder)?;
            match align_from_seek(
                index,
                &mut decoder,
                *anchor_id,
                plan.first_frame,
                &mut decoded_frames,
                &mut is_cancelled,
            ) {
                Ok(frame) => frame,
                Err(
                    BatchExtractionFailure::TimelineMismatch(_)
                    | BatchExtractionFailure::UnexpectedEof,
                ) => {
                    fell_back_to_stream_start = true;
                    decoder = open_checked_decoder(index, &mut open_fresh_decoder)?;
                    align_from_start(
                        index,
                        &mut decoder,
                        plan.first_frame,
                        &mut decoded_frames,
                        &mut is_cancelled,
                    )?
                }
                Err(error) => return Err(error.into()),
            }
        } else {
            align_from_start(
                index,
                &mut decoder,
                plan.first_frame,
                &mut decoded_frames,
                &mut is_cancelled,
            )?
        }
    } else {
        align_from_start(
            index,
            &mut decoder,
            plan.first_frame,
            &mut decoded_frames,
            &mut is_cancelled,
        )?
    };

    let mut current = plan.first_frame;
    let mut selected_frames = 0_u64;

    loop {
        let entry = require_matching_entry(index, current, &decoded)?;
        if plan.contains(current) {
            if is_cancelled() {
                return Err(BatchExtractionFailure::Cancelled.into());
            }
            selected_frames = selected_frames
                .checked_add(1)
                .ok_or(BatchExtractionFailure::NumericRange)?;
            let DecodedRgbaFrame {
                frame,
                stride_bytes,
                pixels,
            } = decoded;
            let pixels = OwnedRgbaFrame::new(frame.width, frame.height, stride_bytes, pixels)
                .map_err(BatchExtractionFailure::Frame)?;
            visitor(ExtractedRgbaFrame {
                progress: ExtractionProgress {
                    frame_id: current,
                    ordinal: selected_frames,
                    total: plan.selected_count,
                },
                index_entry: entry,
                pixels,
            })
            .map_err(BatchExtractionError::Visitor)?;
        }

        if current == last_selected {
            break;
        }

        current = next_frame_id(current)?;
        decoded = next_decoded(&mut decoder, &mut decoded_frames, &mut is_cancelled)?;
    }

    if selected_frames != plan.selected_count {
        return Err(BatchExtractionFailure::InvalidPlan(
            "selected frame count diverged during execution",
        )
        .into());
    }

    Ok(BatchExtractionReport {
        selected_frames,
        decoded_frames,
        used_keyframe_seek,
        fell_back_to_stream_start,
    })
}

fn validate_plan(
    index: &FrameIndex,
    plan: ExtractionPlan,
) -> Result<FrameId, BatchExtractionFailure> {
    let status = index.status()?;
    if status.lifecycle != FrameIndexLifecycle::Complete {
        return Err(BatchExtractionFailure::IncompleteIndex);
    }
    let frame_count = status
        .frame_count
        .ok_or(BatchExtractionFailure::InvalidPlan(
            "complete index is missing its frame count",
        ))?;
    if plan.selected_count == 0 {
        return Err(BatchExtractionFailure::InvalidPlan(
            "selected frame count must be positive",
        ));
    }
    if plan.first_frame.0 > plan.last_frame.0 {
        return Err(BatchExtractionFailure::InvalidPlan(
            "first frame follows last frame",
        ));
    }
    if plan.last_frame.0 >= frame_count {
        return Err(BatchExtractionFailure::InvalidPlan(
            "plan extends beyond the complete frame index",
        ));
    }

    let span = plan
        .last_frame
        .0
        .checked_sub(plan.first_frame.0)
        .ok_or(BatchExtractionFailure::NumericRange)?;
    let expected_count = span
        .checked_div(plan.stride_frames.get())
        .and_then(|value| value.checked_add(1))
        .ok_or(BatchExtractionFailure::NumericRange)?;
    if expected_count != plan.selected_count {
        return Err(BatchExtractionFailure::InvalidPlan(
            "selected frame count does not match plan bounds and stride",
        ));
    }

    let selected_offset = plan
        .selected_count
        .checked_sub(1)
        .and_then(|value| value.checked_mul(plan.stride_frames.get()))
        .ok_or(BatchExtractionFailure::NumericRange)?;
    let last_selected = plan
        .first_frame
        .0
        .checked_add(selected_offset)
        .map(FrameId)
        .ok_or(BatchExtractionFailure::NumericRange)?;
    if last_selected.0 > plan.last_frame.0 {
        return Err(BatchExtractionFailure::InvalidPlan(
            "last selected frame exceeds plan boundary",
        ));
    }
    Ok(last_selected)
}

fn open_checked_decoder<D, F>(index: &FrameIndex, open: &mut F) -> Result<D, BatchExtractionFailure>
where
    D: RgbaNavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
{
    let decoder = open().map_err(BatchExtractionFailure::Decoder)?;
    ensure_stream_identity(index, decoder.selected_stream_for_rgba_navigation())?;
    Ok(decoder)
}

fn ensure_stream_identity(
    index: &FrameIndex,
    stream: &StreamInfo,
) -> Result<(), BatchExtractionFailure> {
    let identity = FrameIndexStreamIdentity::from_stream(stream)?;
    if &identity == index.stream_identity() {
        Ok(())
    } else {
        Err(BatchExtractionFailure::StreamIdentityMismatch)
    }
}

fn align_from_start<D, C>(
    index: &FrameIndex,
    decoder: &mut D,
    first: FrameId,
    decoded_frames: &mut u64,
    is_cancelled: &mut C,
) -> Result<DecodedRgbaFrame, BatchExtractionFailure>
where
    D: RgbaNavigationDecoder,
    C: FnMut() -> bool,
{
    let mut current = FrameId::ZERO;
    loop {
        let decoded = next_decoded(decoder, decoded_frames, is_cancelled)?;
        require_matching_entry(index, current, &decoded)?;
        if current == first {
            return Ok(decoded);
        }
        current = next_frame_id(current)?;
    }
}

fn align_from_seek<D, C>(
    index: &FrameIndex,
    decoder: &mut D,
    anchor: FrameId,
    first: FrameId,
    decoded_frames: &mut u64,
    is_cancelled: &mut C,
) -> Result<DecodedRgbaFrame, BatchExtractionFailure>
where
    D: RgbaNavigationDecoder,
    C: FnMut() -> bool,
{
    let anchor_entry = index
        .entry(anchor)?
        .ok_or(BatchExtractionFailure::TimelineMismatch(anchor.0))?;
    let mut decoded = loop {
        let candidate = next_decoded(decoder, decoded_frames, is_cancelled)?;
        if matches_index_entry(&candidate, &anchor_entry) {
            break candidate;
        }
    };
    let mut current = anchor;

    loop {
        require_matching_entry(index, current, &decoded)?;
        if current == first {
            return Ok(decoded);
        }
        current = next_frame_id(current)?;
        decoded = next_decoded(decoder, decoded_frames, is_cancelled)?;
    }
}

fn next_decoded<D, C>(
    decoder: &mut D,
    decoded_frames: &mut u64,
    is_cancelled: &mut C,
) -> Result<DecodedRgbaFrame, BatchExtractionFailure>
where
    D: RgbaNavigationDecoder,
    C: FnMut() -> bool,
{
    if is_cancelled() {
        return Err(BatchExtractionFailure::Cancelled);
    }
    let decoded = decoder
        .next_rgba_for_navigation()
        .map_err(BatchExtractionFailure::Decoder)?
        .ok_or(BatchExtractionFailure::UnexpectedEof)?;
    *decoded_frames = decoded_frames
        .checked_add(1)
        .ok_or(BatchExtractionFailure::NumericRange)?;
    Ok(decoded)
}

fn require_matching_entry(
    index: &FrameIndex,
    frame_id: FrameId,
    decoded: &DecodedRgbaFrame,
) -> Result<FrameIndexEntry, BatchExtractionFailure> {
    let entry = index
        .entry(frame_id)?
        .ok_or(BatchExtractionFailure::TimelineMismatch(frame_id.0))?;
    if matches_index_entry(decoded, &entry) {
        Ok(entry)
    } else {
        Err(BatchExtractionFailure::TimelineMismatch(frame_id.0))
    }
}

fn matches_index_entry(decoded: &DecodedRgbaFrame, indexed: &FrameIndexEntry) -> bool {
    decoded.frame.presentation_timestamp == indexed.presentation_timestamp
        && decoded.frame.duration == indexed.duration
        && decoded.frame.keyframe == indexed.keyframe
        && decoded.frame.corrupt == indexed.corrupt
}

fn next_frame_id(frame_id: FrameId) -> Result<FrameId, BatchExtractionFailure> {
    frame_id
        .0
        .checked_add(1)
        .map(FrameId)
        .ok_or(BatchExtractionFailure::NumericRange)
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{FrameIndexOpenDisposition, SourceIdentity};
    use framescope_extraction::{
        ExtractionRequest, ExtractionSampling, ExtractionSelection, plan_extraction,
    };
    use framescope_video::{
        CodecInfo, DecodedFrame, MediaDuration, MediaKind, MediaTimestamp, TimeBase,
    };
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::num::NonZeroU64;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    #[derive(Clone)]
    struct Counters {
        seeks: Rc<Cell<u64>>,
        nexts: Rc<Cell<u64>>,
    }

    struct FakeDecoder {
        stream: StreamInfo,
        frames: VecDeque<DecodedRgbaFrame>,
        counters: Counters,
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

    fn timestamp(ticks: i64) -> MediaTimestamp {
        MediaTimestamp {
            ticks,
            time_base: TimeBase::new(1, 1_000).unwrap(),
        }
    }

    fn decoded_frame(id: u64, ticks: i64, keyframe: bool) -> DecodedRgbaFrame {
        let value = u8::try_from(10 + id).unwrap();
        DecodedRgbaFrame {
            frame: DecodedFrame {
                source_id: 1,
                stream_index: 0,
                decode_epoch: 0,
                index: id,
                presentation_timestamp: Some(timestamp(ticks)),
                duration: Some(MediaDuration {
                    ticks: 40,
                    time_base: TimeBase::new(1, 1_000).unwrap(),
                }),
                keyframe,
                corrupt: false,
                width: 2,
                height: 1,
                pixel_format: Some("rgba".into()),
            },
            stride_bytes: 8,
            pixels: vec![value; 8],
        }
    }

    fn source_frames() -> Vec<DecodedRgbaFrame> {
        [0, 40, 90, 130, 200, 260]
            .into_iter()
            .enumerate()
            .map(|(id, ticks)| decoded_frame(id as u64, ticks, id == 0 || id == 3))
            .collect()
    }

    fn temp_db() -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "framescope-extraction-video-{}-{id}.sqlite",
            std::process::id()
        ))
    }

    fn complete_index() -> (PathBuf, FrameIndex) {
        let path = temp_db();
        let identity = FrameIndexStreamIdentity::from_stream(&stream()).unwrap();
        let source = SourceIdentity::new(
            10_000,
            None,
            Some(format!(
                "extraction-video-test-{}",
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            )),
        );
        let (mut index, disposition) = FrameIndex::open_or_create(&path, source, identity).unwrap();
        assert_eq!(disposition, FrameIndexOpenDisposition::Created);

        let frames = source_frames();
        let mut anchor_id = FrameId::ZERO;
        let mut anchor_timestamp = Some(timestamp(0));
        let entries: Vec<_> = frames
            .iter()
            .enumerate()
            .map(|(id, frame)| {
                let frame_id = FrameId(id as u64);
                if frame.frame.keyframe {
                    anchor_id = frame_id;
                    anchor_timestamp = frame.frame.presentation_timestamp;
                }
                FrameIndexEntry {
                    frame_id,
                    presentation_timestamp: frame.frame.presentation_timestamp,
                    duration: frame.frame.duration,
                    keyframe: frame.frame.keyframe,
                    corrupt: frame.frame.corrupt,
                    anchor: KeyframeAnchor::Keyframe {
                        frame_id: anchor_id,
                        presentation_timestamp: anchor_timestamp,
                    },
                }
            })
            .collect();
        index.append_batch(&entries).unwrap();
        index.mark_complete().unwrap();
        (path, index)
    }

    fn counters() -> Counters {
        Counters {
            seeks: Rc::new(Cell::new(0)),
            nexts: Rc::new(Cell::new(0)),
        }
    }

    fn fake(frames: Vec<DecodedRgbaFrame>, counters: Counters) -> FakeDecoder {
        FakeDecoder {
            stream: stream(),
            frames: frames.into(),
            counters,
        }
    }

    #[test]
    fn stride_plan_decodes_forward_once_and_emits_only_selected_frames() {
        let (path, index) = complete_index();
        let plan = plan_extraction(
            &index,
            ExtractionRequest {
                selection: ExtractionSelection::FrameRangeInclusive {
                    start: FrameId(1),
                    end: FrameId(6 - 1),
                },
                sampling: ExtractionSampling::EveryNthFrame(NonZeroU64::new(2).unwrap()),
            },
        )
        .unwrap();
        let frames = source_frames();
        let counters = counters();
        let opened = Rc::new(Cell::new(0_u64));
        let mut emitted = Vec::new();

        let report = extract_rgba_plan(
            &index,
            plan,
            {
                let counters = counters.clone();
                let opened = opened.clone();
                move || {
                    opened.set(opened.get() + 1);
                    Ok::<_, FrameScopeError>(fake(frames.clone(), counters.clone()))
                }
            },
            || false,
            |frame| {
                emitted.push((frame.progress.frame_id, frame.pixels.pixels()[0]));
                Ok::<(), ()>(())
            },
        )
        .unwrap();

        assert_eq!(
            emitted,
            vec![(FrameId(1), 11), (FrameId(3), 13), (FrameId(5), 15)]
        );
        assert_eq!(report.selected_frames, 3);
        assert_eq!(report.decoded_frames, 6);
        assert!(report.used_keyframe_seek);
        assert!(!report.fell_back_to_stream_start);
        assert_eq!(opened.get(), 1);
        assert_eq!(counters.seeks.get(), 1);
        assert_eq!(counters.nexts.get(), 6);

        drop(index);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn seek_alignment_failure_falls_back_before_any_output() {
        let (path, index) = complete_index();
        let plan = plan_extraction(&index, ExtractionRequest::current_frame(FrameId(1))).unwrap();
        let good = source_frames();
        let counters = counters();
        let opened = Rc::new(Cell::new(0_u64));
        let mut emitted = Vec::new();

        let report = extract_rgba_plan(
            &index,
            plan,
            {
                let counters = counters.clone();
                let opened = opened.clone();
                move || {
                    let attempt = opened.get();
                    opened.set(attempt + 1);
                    let frames = if attempt == 0 {
                        good[1..].to_vec()
                    } else {
                        good.clone()
                    };
                    Ok::<_, FrameScopeError>(fake(frames, counters.clone()))
                }
            },
            || false,
            |frame| {
                emitted.push(frame.progress.frame_id);
                Ok::<(), ()>(())
            },
        )
        .unwrap();

        assert_eq!(emitted, vec![FrameId(1)]);
        assert_eq!(opened.get(), 2);
        assert!(report.used_keyframe_seek);
        assert!(report.fell_back_to_stream_start);
        assert!(report.decoded_frames > 2);

        drop(index);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn timeline_failure_after_first_output_does_not_retry_and_duplicate() {
        let (path, index) = complete_index();
        let plan = plan_extraction(
            &index,
            ExtractionRequest {
                selection: ExtractionSelection::FrameRangeInclusive {
                    start: FrameId(0),
                    end: FrameId(2),
                },
                sampling: ExtractionSampling::EveryFrame,
            },
        )
        .unwrap();
        let mut broken = source_frames();
        broken[1].frame.presentation_timestamp = Some(timestamp(999));
        let counters = counters();
        let opened = Rc::new(Cell::new(0_u64));
        let mut emitted = Vec::new();

        let result = extract_rgba_plan(
            &index,
            plan,
            {
                let counters = counters.clone();
                let opened = opened.clone();
                move || {
                    opened.set(opened.get() + 1);
                    Ok::<_, FrameScopeError>(fake(broken.clone(), counters.clone()))
                }
            },
            || false,
            |frame| {
                emitted.push(frame.progress.frame_id);
                Ok::<(), ()>(())
            },
        );

        assert!(matches!(
            result,
            Err(BatchExtractionError::Failure(
                BatchExtractionFailure::TimelineMismatch(1)
            ))
        ));
        assert_eq!(emitted, vec![FrameId(0)]);
        assert_eq!(opened.get(), 1);

        drop(index);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cancellation_after_output_stops_before_decoding_another_frame() {
        let (path, index) = complete_index();
        let plan = plan_extraction(&index, ExtractionRequest::all_frames()).unwrap();
        let frames = source_frames();
        let counters = counters();
        let emitted = Rc::new(Cell::new(0_u64));

        let result = extract_rgba_plan(
            &index,
            plan,
            {
                let counters = counters.clone();
                move || Ok::<_, FrameScopeError>(fake(frames.clone(), counters.clone()))
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
            Err(BatchExtractionError::Failure(
                BatchExtractionFailure::Cancelled
            ))
        ));
        assert_eq!(emitted.get(), 1);
        assert_eq!(counters.nexts.get(), 1);

        drop(index);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn forged_plan_is_rejected_before_decoder_open() {
        let (path, index) = complete_index();
        let opened = Rc::new(Cell::new(false));
        let plan = ExtractionPlan {
            first_frame: FrameId(0),
            last_frame: FrameId(2),
            stride_frames: NonZeroU64::new(1).unwrap(),
            selected_count: 99,
        };

        let result = extract_rgba_plan(
            &index,
            plan,
            {
                let opened = opened.clone();
                move || {
                    opened.set(true);
                    Ok::<_, FrameScopeError>(fake(source_frames(), counters()))
                }
            },
            || false,
            |_frame| Ok::<(), ()>(()),
        );

        assert!(matches!(
            result,
            Err(BatchExtractionError::Failure(
                BatchExtractionFailure::InvalidPlan(_)
            ))
        ));
        assert!(!opened.get());

        drop(index);
        let _ = std::fs::remove_file(path);
    }
}
