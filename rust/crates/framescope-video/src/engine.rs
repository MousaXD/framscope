use framescope_core::{
    CodecInfo, ContainerInfo, DecodedFrame, FrameScopeError, MediaDuration, MediaKind,
    MediaTimestamp, Rational, StreamInfo, TimeBase, VideoInfo,
};
use framescope_ffmpeg::{NativeError, NativeErrorKind, NativeFrame, NativeStreamInfo, Session};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub use framescope_ffmpeg::CancellationToken;

const MAX_STREAMS: u32 = 4_096;
static NEXT_SOURCE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoStreamSelection {
    #[default]
    Default,
    Index(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OpenOptions {
    pub stream_selection: VideoStreamSelection,
}

impl OpenOptions {
    fn requested_stream(self) -> Option<u32> {
        match self.stream_selection {
            VideoStreamSelection::Default => None,
            VideoStreamSelection::Index(index) => Some(index),
        }
    }
}

/// Classification based only on presentation timestamp deltas observed so far.
///
/// This is not used to calculate time. It can evolve from `Constant` to `Variable` if a later
/// frame introduces a different delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservedFrameRateMode {
    Undetermined,
    Constant,
    Variable,
}

/// One decoded presentation frame plus an owned, tightly-packed RGBA snapshot.
///
/// The pixel bytes are independent from FFmpeg's reusable `AVFrame` and remain valid after the
/// decoder advances, seeks, or is dropped. This is source-decoded full-resolution data, not a
/// lossy Phase 3 disk proxy.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedRgbaFrame {
    pub frame: DecodedFrame,
    pub stride_bytes: usize,
    pub pixels: Vec<u8>,
}

#[derive(Debug, Default)]
struct TimestampCadence {
    last: Option<MediaTimestamp>,
    first_delta: Option<i64>,
    intervals: u64,
    variable: bool,
}

impl TimestampCadence {
    fn record(&mut self, timestamp: MediaTimestamp) {
        if let Some(last) = self.last {
            if last.time_base != timestamp.time_base {
                self.variable = true;
            } else if let Some(delta) = timestamp.ticks.checked_sub(last.ticks) {
                if delta <= 0 {
                    self.variable = true;
                } else if let Some(first_delta) = self.first_delta {
                    if delta != first_delta {
                        self.variable = true;
                    }
                } else {
                    self.first_delta = Some(delta);
                }
                self.intervals = self.intervals.saturating_add(1);
            } else {
                self.variable = true;
            }
        }
        self.last = Some(timestamp);
    }

    fn mode(&self) -> ObservedFrameRateMode {
        if self.variable {
            ObservedFrameRateMode::Variable
        } else if self.intervals >= 2 {
            ObservedFrameRateMode::Constant
        } else {
            ObservedFrameRateMode::Undetermined
        }
    }
}

/// Streaming FFmpeg-backed decoder.
///
/// The decoder owns one format context, one codec context, and reusable packet/frame storage. It
/// never predecodes or stores the whole source. Each `next_frame` advances only as far as needed to
/// produce the next selected video frame.
pub struct VideoDecoder {
    session: Session,
    info: VideoInfo,
    source_id: u64,
    cancellation: CancellationToken,
    cadence: TimestampCadence,
    current_frame: Option<DecodedFrame>,
}

impl VideoDecoder {
    pub fn open_path(path: impl AsRef<Path>) -> Result<Self, FrameScopeError> {
        Self::open_path_with_options(path, OpenOptions::default(), CancellationToken::new())
    }

    /// Open a filesystem source using the selected stream policy and cancellation signal.
    ///
    /// Callers that need to cancel a potentially slow probe should keep a clone of `cancellation`
    /// and cancel it from another thread.
    pub fn open_path_with_options(
        path: impl AsRef<Path>,
        options: OpenOptions,
        cancellation: CancellationToken,
    ) -> Result<Self, FrameScopeError> {
        if cancellation.is_cancelled() {
            return Err(FrameScopeError::Cancelled);
        }
        let session = Session::open_path(
            path.as_ref(),
            options.requested_stream(),
            cancellation.clone(),
        )
        .map_err(map_native_error)?;
        Self::from_session(session, cancellation)
    }

    /// Open a Unix/Android file descriptor without taking ownership of the caller's descriptor.
    ///
    /// The native backend duplicates the descriptor immediately and owns/closes only that copy.
    /// Seekable descriptors are read with an independent logical offset so FrameScope does not
    /// mutate the caller's current file position.
    #[cfg(unix)]
    pub fn open_file_descriptor(fd: std::os::fd::BorrowedFd<'_>) -> Result<Self, FrameScopeError> {
        Self::open_file_descriptor_with_options(
            fd,
            OpenOptions::default(),
            CancellationToken::new(),
        )
    }

    #[cfg(unix)]
    pub fn open_file_descriptor_with_options(
        fd: std::os::fd::BorrowedFd<'_>,
        options: OpenOptions,
        cancellation: CancellationToken,
    ) -> Result<Self, FrameScopeError> {
        use std::os::fd::AsRawFd;

        if cancellation.is_cancelled() {
            return Err(FrameScopeError::Cancelled);
        }
        let session = Session::open_fd(
            fd.as_raw_fd(),
            options.requested_stream(),
            cancellation.clone(),
        )
        .map_err(map_native_error)?;
        Self::from_session(session, cancellation)
    }

    fn from_session(
        session: Session,
        cancellation: CancellationToken,
    ) -> Result<Self, FrameScopeError> {
        let container = session.container_info().map_err(map_native_error)?;
        if container.stream_count > MAX_STREAMS {
            return Err(FrameScopeError::MalformedContainer(format!(
                "container declares {} streams, exceeding the safety limit of {MAX_STREAMS}",
                container.stream_count
            )));
        }
        let selected_video_stream =
            u32::try_from(container.selected_stream_index).map_err(|_| {
                FrameScopeError::InvalidMetadata("selected video stream index is invalid".into())
            })?;

        let mut streams = Vec::with_capacity(container.stream_count as usize);
        for ordinal in 0..container.stream_count {
            let native = session.stream_info(ordinal).map_err(map_native_error)?;
            streams.push(map_stream_info(native)?);
        }
        if !streams.iter().any(|stream| {
            stream.index == selected_video_stream && stream.media_kind == MediaKind::Video
        }) {
            return Err(FrameScopeError::InvalidMetadata(
                "selected video stream is absent from discovered stream metadata".into(),
            ));
        }

        let duration_us = container
            .duration_us
            .and_then(|value| u64::try_from(value).ok());
        let info = VideoInfo {
            container: ContainerInfo {
                format_name: container.format_name,
                format_long_name: container.format_long_name,
                duration_us,
                stream_count: container.stream_count,
            },
            streams,
            selected_video_stream,
        };

        Ok(Self {
            session,
            info,
            source_id: next_source_id(),
            cancellation,
            cadence: TimestampCadence::default(),
            current_frame: None,
        })
    }

    pub fn info(&self) -> &VideoInfo {
        &self.info
    }

    pub fn source_id(&self) -> u64 {
        self.source_id
    }

    pub fn selected_stream(&self) -> &StreamInfo {
        self.info
            .streams
            .iter()
            .find(|stream| stream.index == self.info.selected_video_stream)
            .expect("selected stream is validated during VideoDecoder construction")
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn observed_frame_rate_mode(&self) -> ObservedFrameRateMode {
        self.cadence.mode()
    }

    /// Decode the next display frame from the selected video stream.
    ///
    /// `Ok(None)` is stable EOF. Frame time is taken from FFmpeg's decoded presentation timestamp,
    /// never from `index` or frame-rate metadata.
    pub fn next_frame(&mut self) -> Result<Option<DecodedFrame>, FrameScopeError> {
        if self.cancellation.is_cancelled() {
            return Err(FrameScopeError::Cancelled);
        }
        let Some(native) = self.session.next_frame().map_err(map_native_error)? else {
            self.current_frame = None;
            return Ok(None);
        };
        let frame = self.map_frame(native)?;
        if let Some(timestamp) = frame.presentation_timestamp {
            self.cadence.record(timestamp);
        }
        self.current_frame = Some(frame.clone());
        Ok(Some(frame))
    }

    /// Copy source-quality RGBA for the exact frame most recently returned by [`Self::next_frame`].
    ///
    /// The metadata argument is checked against the decoder cursor before any pixels are labelled.
    /// This prevents a stale `DecodedFrame` from being paired with a newer reusable FFmpeg frame.
    pub fn snapshot_current_frame_rgba(
        &mut self,
        frame: &DecodedFrame,
    ) -> Result<DecodedRgbaFrame, FrameScopeError> {
        if self.cancellation.is_cancelled() {
            return Err(FrameScopeError::Cancelled);
        }
        if self.current_frame.as_ref() != Some(frame) {
            return Err(FrameScopeError::DecoderFailure(
                "RGBA snapshot request does not match the decoder's current frame".into(),
            ));
        }
        let rgba = self
            .session
            .copy_current_frame_rgba()
            .map_err(map_native_error)?;
        if rgba.width != frame.width || rgba.height != frame.height {
            return Err(FrameScopeError::DecoderFailure(format!(
                "RGBA snapshot dimensions {}x{} do not match decoded frame {}x{}",
                rgba.width, rgba.height, frame.width, frame.height
            )));
        }
        let expected = rgba
            .stride
            .checked_mul(usize::try_from(frame.height).map_err(|_| {
                FrameScopeError::DecoderFailure("decoded frame height exceeds memory size".into())
            })?)
            .ok_or_else(|| {
                FrameScopeError::DecoderFailure("RGBA snapshot byte size overflows memory".into())
            })?;
        if rgba.pixels.len() != expected {
            return Err(FrameScopeError::DecoderFailure(format!(
                "RGBA snapshot has {} bytes, expected {expected}",
                rgba.pixels.len()
            )));
        }

        Ok(DecodedRgbaFrame {
            frame: frame.clone(),
            stride_bytes: rgba.stride,
            pixels: rgba.pixels,
        })
    }

    /// Decode the next display frame and copy its full-resolution pixels into owned RGBA storage.
    ///
    /// Unlike [`Self::next_frame`], this performs a pixel-format conversion and allocation, so
    /// metadata-only indexing should continue to use `next_frame`. The returned pixels are owned and
    /// can safely be retained by the Phase 3 RAM cache after the decoder advances.
    pub fn next_frame_rgba(&mut self) -> Result<Option<DecodedRgbaFrame>, FrameScopeError> {
        let Some(frame) = self.next_frame()? else {
            return Ok(None);
        };
        self.snapshot_current_frame_rgba(&frame).map(Some)
    }

    /// Foundational timestamp seek. FFmpeg may land on an earlier keyframe.
    ///
    /// Phase 2 intentionally does not promise arbitrary-frame instant seek. A successful seek
    /// flushes decoder state and starts a new `decode_epoch`; Phase 3 can build indexes above this.
    pub fn seek_to_timestamp_us(&mut self, timestamp_us: i64) -> Result<(), FrameScopeError> {
        if timestamp_us < 0 {
            return Err(FrameScopeError::SeekUnavailable(
                "seek timestamp must be non-negative".into(),
            ));
        }
        if self.cancellation.is_cancelled() {
            return Err(FrameScopeError::Cancelled);
        }
        self.session
            .seek_us(timestamp_us)
            .map_err(map_native_error)?;
        self.cadence = TimestampCadence::default();
        self.current_frame = None;
        Ok(())
    }

    fn map_frame(&self, native: NativeFrame) -> Result<DecodedFrame, FrameScopeError> {
        let stream_index = u32::try_from(native.stream_index).map_err(|_| {
            FrameScopeError::DecoderFailure("decoder returned an invalid stream index".into())
        })?;
        if stream_index != self.info.selected_video_stream {
            return Err(FrameScopeError::DecoderFailure(format!(
                "decoder emitted stream {stream_index}, expected {}",
                self.info.selected_video_stream
            )));
        }

        let time_base = TimeBase::new(native.time_base_num, native.time_base_den);
        let presentation_timestamp = match native.timestamp_ticks {
            Some(ticks) => Some(MediaTimestamp {
                ticks,
                time_base: time_base.ok_or_else(|| {
                    FrameScopeError::DecoderFailure(
                        "decoded frame timestamp has an invalid stream time base".into(),
                    )
                })?,
            }),
            None => None,
        };
        let duration = match native.duration_ticks.filter(|ticks| *ticks > 0) {
            Some(ticks) => Some(MediaDuration {
                ticks,
                time_base: time_base.ok_or_else(|| {
                    FrameScopeError::DecoderFailure(
                        "decoded frame duration has an invalid stream time base".into(),
                    )
                })?,
            }),
            None => None,
        };
        let width = u32::try_from(native.width)
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                FrameScopeError::DecoderFailure("decoded frame width is invalid".into())
            })?;
        let height = u32::try_from(native.height)
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                FrameScopeError::DecoderFailure("decoded frame height is invalid".into())
            })?;

        Ok(DecodedFrame {
            source_id: self.source_id,
            stream_index,
            decode_epoch: native.epoch,
            index: native.index,
            presentation_timestamp,
            duration,
            keyframe: native.keyframe,
            corrupt: native.corrupt,
            width,
            height,
            pixel_format: native.pixel_format_name,
        })
    }
}

fn next_source_id() -> u64 {
    let id = NEXT_SOURCE_ID.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        NEXT_SOURCE_ID.fetch_add(1, Ordering::Relaxed)
    } else {
        id
    }
}

fn map_stream_info(native: NativeStreamInfo) -> Result<StreamInfo, FrameScopeError> {
    let index = u32::try_from(native.index).map_err(|_| {
        FrameScopeError::InvalidMetadata("stream has a negative or invalid index".into())
    })?;
    let media_kind = match native.media_type {
        1 => MediaKind::Video,
        2 => MediaKind::Audio,
        3 => MediaKind::Subtitle,
        4 => MediaKind::Data,
        5 => MediaKind::Attachment,
        _ => MediaKind::Unknown,
    };
    let time_base = TimeBase::new(native.time_base_num, native.time_base_den);
    let duration = native
        .duration_ticks
        .filter(|ticks| *ticks >= 0)
        .zip(time_base)
        .map(|(ticks, time_base)| MediaDuration { ticks, time_base });

    Ok(StreamInfo {
        index,
        media_kind,
        codec: CodecInfo {
            id: native.codec_id,
            name: native.codec_name,
            decoder_available: native.decoder_available,
        },
        is_default: native.is_default,
        time_base,
        duration,
        frame_count: native.frame_count,
        width: native.width,
        height: native.height,
        pixel_format: native.pixel_format_name,
        average_frame_rate: Rational::new(native.average_rate_num, native.average_rate_den),
        nominal_frame_rate: Rational::new(native.nominal_rate_num, native.nominal_rate_den),
        rotation_degrees: native.rotation_degrees,
    })
}

fn map_native_error(error: NativeError) -> FrameScopeError {
    match error.kind {
        NativeErrorKind::UnsupportedFormat => FrameScopeError::UnsupportedFormat(error.message),
        NativeErrorKind::UnsupportedCodec => FrameScopeError::UnsupportedCodec(error.message),
        NativeErrorKind::InvalidSource => FrameScopeError::InvalidSource(error.message),
        NativeErrorKind::NoVideoStream => FrameScopeError::NoVideoTrack,
        NativeErrorKind::Decoder => FrameScopeError::DecoderFailure(error.message),
        NativeErrorKind::Malformed => FrameScopeError::MalformedContainer(error.message),
        NativeErrorKind::Io => FrameScopeError::Io(error.message),
        NativeErrorKind::Cancelled => FrameScopeError::Cancelled,
        NativeErrorKind::SeekUnavailable => FrameScopeError::SeekUnavailable(error.message),
        NativeErrorKind::Backend => FrameScopeError::BackendUnavailable(error.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timestamp(ticks: i64) -> MediaTimestamp {
        MediaTimestamp {
            ticks,
            time_base: TimeBase::new(1, 1_000).unwrap(),
        }
    }

    #[test]
    fn cadence_uses_timestamp_deltas_not_frame_indexes() {
        let mut cadence = TimestampCadence::default();
        cadence.record(timestamp(0));
        cadence.record(timestamp(40));
        assert_eq!(cadence.mode(), ObservedFrameRateMode::Undetermined);
        cadence.record(timestamp(80));
        assert_eq!(cadence.mode(), ObservedFrameRateMode::Constant);
        cadence.record(timestamp(140));
        assert_eq!(cadence.mode(), ObservedFrameRateMode::Variable);
    }

    #[test]
    fn cancelled_native_error_maps_to_typed_error() {
        let error = map_native_error(NativeError {
            kind: NativeErrorKind::Cancelled,
            ffmpeg_code: -1,
            message: "cancelled".into(),
        });
        assert!(matches!(error, FrameScopeError::Cancelled));
    }
}
