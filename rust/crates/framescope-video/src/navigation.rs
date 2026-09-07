use crate::VideoDecoder;
use framescope_cache::{
    FrameId, FrameIndex, FrameIndexEntry, FrameIndexError, FrameIndexLifecycle,
    FrameIndexStreamIdentity, KeyframeAnchor,
};
use framescope_core::{DecodedFrame, FrameScopeError, MediaTimestamp, StreamInfo};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampSelection {
    AtOrBefore,
    AtOrAfter,
    Nearest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationResult {
    pub frame_id: FrameId,
    pub frame: DecodedFrame,
    pub decoded_frames: u64,
    pub used_keyframe_seek: bool,
    pub fell_back_to_stream_start: bool,
}

#[derive(Debug, Error)]
pub enum NavigationError {
    #[error("video navigation failed: {0}")]
    Decoder(#[from] FrameScopeError),
    #[error("frame-index lookup failed: {0}")]
    Index(#[from] FrameIndexError),
    #[error("indexed navigation requires a complete frame index")]
    IncompleteIndex,
    #[error("requested frame is outside the complete frame index")]
    FrameNotIndexed,
    #[error("timestamp does not resolve to an indexed frame under the requested policy")]
    TimestampNotIndexed,
    #[error("fresh decoder stream does not match the stream bound to the frame index")]
    StreamIdentityMismatch,
    #[error("decoded presentation timeline does not reconcile with the persistent frame index")]
    TimelineMismatch,
    #[error("decoder reached EOF before the requested indexed frame")]
    UnexpectedEof,
}

pub trait NavigationDecoder {
    fn selected_stream_for_navigation(&self) -> &StreamInfo;
    fn next_frame_for_navigation(&mut self) -> Result<Option<DecodedFrame>, FrameScopeError>;
    fn seek_for_navigation(&mut self, timestamp_us: i64) -> Result<(), FrameScopeError>;
}

impl NavigationDecoder for VideoDecoder {
    fn selected_stream_for_navigation(&self) -> &StreamInfo {
        self.selected_stream()
    }

    fn next_frame_for_navigation(&mut self) -> Result<Option<DecodedFrame>, FrameScopeError> {
        self.next_frame()
    }

    fn seek_for_navigation(&mut self, timestamp_us: i64) -> Result<(), FrameScopeError> {
        self.seek_to_timestamp_us(timestamp_us)
    }
}

pub fn resolve_timestamp(
    index: &FrameIndex,
    timestamp: MediaTimestamp,
    selection: TimestampSelection,
) -> Result<Option<FrameId>, NavigationError> {
    ensure_complete(index)?;
    let entry = match selection {
        TimestampSelection::AtOrBefore => index.frame_at_or_before(timestamp)?,
        TimestampSelection::AtOrAfter => index.frame_at_or_after(timestamp)?,
        TimestampSelection::Nearest => {
            let before = index.frame_at_or_before(timestamp)?;
            let after = index.frame_at_or_after(timestamp)?;
            choose_nearest(timestamp, before, after)?
        }
    };
    Ok(entry.map(|entry| entry.frame_id))
}

pub fn navigate_to_timestamp<D, F>(
    index: &FrameIndex,
    open_fresh_decoder: F,
    timestamp: MediaTimestamp,
    selection: TimestampSelection,
) -> Result<NavigationResult, NavigationError>
where
    D: NavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
{
    let frame_id = resolve_timestamp(index, timestamp, selection)?
        .ok_or(NavigationError::TimestampNotIndexed)?;
    navigate_to_frame(index, open_fresh_decoder, frame_id)
}

pub fn navigate_to_frame<D, F>(
    index: &FrameIndex,
    mut open_fresh_decoder: F,
    frame_id: FrameId,
) -> Result<NavigationResult, NavigationError>
where
    D: NavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
{
    ensure_complete(index)?;
    let target = index
        .entry(frame_id)?
        .ok_or(NavigationError::FrameNotIndexed)?;

    let mut decoder = open_checked_decoder(index, &mut open_fresh_decoder)?;
    if let KeyframeAnchor::Keyframe {
        frame_id: anchor_id,
        presentation_timestamp: Some(anchor_timestamp),
    } = target.anchor
    {
        if let Some(timestamp_us) = anchor_timestamp.to_microseconds().filter(|value| *value >= 0) {
            decoder.seek_for_navigation(timestamp_us)?;
            match decode_from_seek(index, &mut decoder, anchor_id, frame_id) {
                Ok((frame, decoded_frames)) => {
                    return Ok(NavigationResult {
                        frame_id,
                        frame,
                        decoded_frames,
                        used_keyframe_seek: true,
                        fell_back_to_stream_start: false,
                    });
                }
                Err(NavigationError::TimelineMismatch | NavigationError::UnexpectedEof) => {
                    // Some demuxers may legally land after an imprecise timestamp seek. Reopening
                    // and reconciling from stream start is the correctness fallback.
                }
                Err(error) => return Err(error),
            }

            let mut fallback = open_checked_decoder(index, &mut open_fresh_decoder)?;
            let (frame, decoded_frames) = decode_from_start(index, &mut fallback, frame_id)?;
            return Ok(NavigationResult {
                frame_id,
                frame,
                decoded_frames,
                used_keyframe_seek: true,
                fell_back_to_stream_start: true,
            });
        }
    }

    let (frame, decoded_frames) = decode_from_start(index, &mut decoder, frame_id)?;
    Ok(NavigationResult {
        frame_id,
        frame,
        decoded_frames,
        used_keyframe_seek: false,
        fell_back_to_stream_start: false,
    })
}

fn ensure_complete(index: &FrameIndex) -> Result<(), NavigationError> {
    if index.status()?.lifecycle == FrameIndexLifecycle::Complete {
        Ok(())
    } else {
        Err(NavigationError::IncompleteIndex)
    }
}

fn open_checked_decoder<D, F>(
    index: &FrameIndex,
    open_fresh_decoder: &mut F,
) -> Result<D, NavigationError>
where
    D: NavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
{
    let decoder = open_fresh_decoder()?;
    let identity = FrameIndexStreamIdentity::from_stream(decoder.selected_stream_for_navigation())?;
    if &identity != index.stream_identity() {
        return Err(NavigationError::StreamIdentityMismatch);
    }
    Ok(decoder)
}

fn decode_from_start<D: NavigationDecoder>(
    index: &FrameIndex,
    decoder: &mut D,
    target: FrameId,
) -> Result<(DecodedFrame, u64), NavigationError> {
    let mut current = FrameId::ZERO;
    let mut decoded_frames = 0_u64;
    loop {
        let decoded = decoder
            .next_frame_for_navigation()?
            .ok_or(NavigationError::UnexpectedEof)?;
        decoded_frames = decoded_frames.saturating_add(1);
        let expected = index
            .entry(current)?
            .ok_or(NavigationError::TimelineMismatch)?;
        if !matches_index_entry(&decoded, &expected) {
            return Err(NavigationError::TimelineMismatch);
        }
        if current == target {
            return Ok((decoded, decoded_frames));
        }
        current = next_frame_id(current)?;
    }
}

fn decode_from_seek<D: NavigationDecoder>(
    index: &FrameIndex,
    decoder: &mut D,
    anchor: FrameId,
    target: FrameId,
) -> Result<(DecodedFrame, u64), NavigationError> {
    let anchor_entry = index
        .entry(anchor)?
        .ok_or(NavigationError::TimelineMismatch)?;
    let mut current = anchor;
    let mut aligned = false;
    let mut decoded_frames = 0_u64;

    loop {
        let decoded = decoder
            .next_frame_for_navigation()?
            .ok_or(NavigationError::UnexpectedEof)?;
        decoded_frames = decoded_frames.saturating_add(1);

        if !aligned {
            if !matches_index_entry(&decoded, &anchor_entry) {
                continue;
            }
            aligned = true;
        }

        let expected = index
            .entry(current)?
            .ok_or(NavigationError::TimelineMismatch)?;
        if !matches_index_entry(&decoded, &expected) {
            return Err(NavigationError::TimelineMismatch);
        }
        if current == target {
            return Ok((decoded, decoded_frames));
        }
        current = next_frame_id(current)?;
    }
}

fn next_frame_id(frame_id: FrameId) -> Result<FrameId, NavigationError> {
    frame_id
        .0
        .checked_add(1)
        .map(FrameId)
        .ok_or(NavigationError::TimelineMismatch)
}

fn matches_index_entry(decoded: &DecodedFrame, indexed: &FrameIndexEntry) -> bool {
    decoded.presentation_timestamp == indexed.presentation_timestamp
        && decoded.duration == indexed.duration
        && decoded.keyframe == indexed.keyframe
        && decoded.corrupt == indexed.corrupt
}

fn choose_nearest(
    target: MediaTimestamp,
    before: Option<FrameIndexEntry>,
    after: Option<FrameIndexEntry>,
) -> Result<Option<FrameIndexEntry>, NavigationError> {
    match (before, after) {
        (None, None) => Ok(None),
        (Some(entry), None) | (None, Some(entry)) => Ok(Some(entry)),
        (Some(before), Some(after)) => {
            let before_timestamp = before
                .presentation_timestamp
                .ok_or(NavigationError::TimelineMismatch)?;
            let after_timestamp = after
                .presentation_timestamp
                .ok_or(NavigationError::TimelineMismatch)?;
            let before_distance = target
                .ticks
                .checked_sub(before_timestamp.ticks)
                .ok_or(NavigationError::TimelineMismatch)?;
            let after_distance = after_timestamp
                .ticks
                .checked_sub(target.ticks)
                .ok_or(NavigationError::TimelineMismatch)?;
            if after_distance == 0 {
                // For repeated exact timestamps, AtOrAfter is the first equal frame.
                Ok(Some(after))
            } else if before_distance <= after_distance {
                Ok(Some(before))
            } else {
                Ok(Some(after))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{FrameIndexOpenDisposition, SourceIdentity};
    use framescope_core::{CodecInfo, MediaDuration, MediaKind, TimeBase};
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    struct FakeDecoder {
        stream: StreamInfo,
        frames: VecDeque<DecodedFrame>,
        seeked: bool,
    }

    impl NavigationDecoder for FakeDecoder {
        fn selected_stream_for_navigation(&self) -> &StreamInfo {
            &self.stream
        }

        fn next_frame_for_navigation(&mut self) -> Result<Option<DecodedFrame>, FrameScopeError> {
            Ok(self.frames.pop_front())
        }

        fn seek_for_navigation(&mut self, _timestamp_us: i64) -> Result<(), FrameScopeError> {
            self.seeked = true;
            while self.frames.front().is_some_and(|frame| frame.index < 2) {
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
            width: Some(64),
            height: Some(48),
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

    fn frame(id: u64, ticks: i64, keyframe: bool) -> DecodedFrame {
        DecodedFrame {
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
            width: 64,
            height: 48,
            pixel_format: Some("yuv420p".into()),
        }
    }

    fn temp_db() -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "framescope-navigation-{}-{id}.sqlite",
            std::process::id()
        ))
    }

    fn complete_index() -> (PathBuf, FrameIndex) {
        let path = temp_db();
        let identity = FrameIndexStreamIdentity::from_stream(&stream()).unwrap();
        let source = SourceIdentity::new(100, None, Some("navigation".into()));
        let (mut index, disposition) = FrameIndex::open_or_create(&path, source, identity).unwrap();
        assert_eq!(disposition, FrameIndexOpenDisposition::Created);
        index.mark_building().unwrap();
        let ticks = [0, 40, 100, 140, 220];
        let mut anchor_id = 0;
        let mut anchor_ticks = 0;
        let entries = ticks
            .iter()
            .enumerate()
            .map(|(id, ticks)| {
                let keyframe = id == 0 || id == 2;
                if keyframe {
                    anchor_id = id as u64;
                    anchor_ticks = *ticks;
                }
                FrameIndexEntry {
                    frame_id: FrameId(id as u64),
                    presentation_timestamp: Some(timestamp(*ticks)),
                    duration: Some(MediaDuration {
                        ticks: 40,
                        time_base: TimeBase::new(1, 1_000).unwrap(),
                    }),
                    keyframe,
                    corrupt: false,
                    anchor: KeyframeAnchor::Keyframe {
                        frame_id: FrameId(anchor_id),
                        presentation_timestamp: Some(timestamp(anchor_ticks)),
                    },
                }
            })
            .collect::<Vec<_>>();
        index.append_batch(&entries).unwrap();
        index.mark_complete().unwrap();
        (path, index)
    }

    fn fake_decoder() -> FakeDecoder {
        FakeDecoder {
            stream: stream(),
            frames: VecDeque::from(vec![
                frame(0, 0, true),
                frame(1, 40, false),
                frame(2, 100, true),
                frame(3, 140, false),
                frame(4, 220, false),
            ]),
            seeked: false,
        }
    }

    #[test]
    fn timestamp_policies_are_explicit_and_vfr_safe() {
        let (path, index) = complete_index();
        assert_eq!(
            resolve_timestamp(&index, timestamp(120), TimestampSelection::AtOrBefore).unwrap(),
            Some(FrameId(2))
        );
        assert_eq!(
            resolve_timestamp(&index, timestamp(120), TimestampSelection::AtOrAfter).unwrap(),
            Some(FrameId(3))
        );
        assert_eq!(
            resolve_timestamp(&index, timestamp(120), TimestampSelection::Nearest).unwrap(),
            Some(FrameId(2))
        );
        drop(index);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn distant_frame_uses_keyframe_anchor_and_decodes_forward() {
        let (path, index) = complete_index();
        let result = navigate_to_frame(&index, || Ok(fake_decoder()), FrameId(4)).unwrap();
        assert!(result.used_keyframe_seek);
        assert!(!result.fell_back_to_stream_start);
        assert_eq!(result.frame_id, FrameId(4));
        assert_eq!(result.frame.presentation_timestamp, Some(timestamp(220)));
        assert_eq!(result.decoded_frames, 3);
        drop(index);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn incomplete_index_is_rejected() {
        let path = temp_db();
        let identity = FrameIndexStreamIdentity::from_stream(&stream()).unwrap();
        let source = SourceIdentity::new(100, None, Some("partial".into()));
        let (index, _) = FrameIndex::open_or_create(&path, source, identity).unwrap();
        let error = navigate_to_frame(&index, || Ok(fake_decoder()), FrameId(0)).unwrap_err();
        assert!(matches!(error, NavigationError::IncompleteIndex));
        drop(index);
        let _ = std::fs::remove_file(path);
    }
}
