//! Shared domain types for FrameScope.
//! This crate is platform-neutral and must not depend on Android APIs.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A generic rational value such as a frame rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rational {
    pub numerator: i32,
    pub denominator: i32,
}

impl Rational {
    pub fn new(numerator: i32, denominator: i32) -> Option<Self> {
        (numerator > 0 && denominator > 0).then_some(Self {
            numerator,
            denominator,
        })
    }

    pub fn as_f64(self) -> f64 {
        f64::from(self.numerator) / f64::from(self.denominator)
    }
}

/// Fundamental unit used by a media stream for timestamps.
///
/// For example, a time base of `1 / 90_000` means one timestamp tick is 1/90,000 second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeBase {
    pub numerator: i32,
    pub denominator: i32,
}

impl TimeBase {
    pub fn new(numerator: i32, denominator: i32) -> Option<Self> {
        (numerator > 0 && denominator > 0).then_some(Self {
            numerator,
            denominator,
        })
    }

    pub fn seconds_per_tick(self) -> f64 {
        f64::from(self.numerator) / f64::from(self.denominator)
    }
}

/// Presentation timestamp in the exact time base reported by the selected stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaTimestamp {
    pub ticks: i64,
    pub time_base: TimeBase,
}

impl MediaTimestamp {
    /// Convert to microseconds without using floating-point arithmetic.
    pub fn to_microseconds(self) -> Option<i64> {
        scale_ticks_to_microseconds(self.ticks, self.time_base)
    }
}

/// Frame or stream duration in the exact time base reported by the media source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaDuration {
    pub ticks: i64,
    pub time_base: TimeBase,
}

impl MediaDuration {
    /// Convert to microseconds without using floating-point arithmetic.
    pub fn to_microseconds(self) -> Option<i64> {
        scale_ticks_to_microseconds(self.ticks, self.time_base)
    }
}

fn scale_ticks_to_microseconds(ticks: i64, time_base: TimeBase) -> Option<i64> {
    let scaled = i128::from(ticks)
        .checked_mul(i128::from(time_base.numerator))?
        .checked_mul(1_000_000)?;
    let micros = scaled / i128::from(time_base.denominator);
    i64::try_from(micros).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Unknown,
    Video,
    Audio,
    Subtitle,
    Data,
    Attachment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodecInfo {
    /// FFmpeg codec identifier. This numeric value is diagnostic, not a stable wire protocol.
    pub id: i32,
    pub name: String,
    pub decoder_available: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamInfo {
    pub index: u32,
    pub media_kind: MediaKind,
    pub codec: CodecInfo,
    pub is_default: bool,
    pub time_base: Option<TimeBase>,
    pub duration: Option<MediaDuration>,
    pub frame_count: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub pixel_format: Option<String>,
    /// Informational only. Actual frame timing always comes from timestamps.
    pub average_frame_rate: Option<Rational>,
    /// Informational/estimated only. Actual frame timing always comes from timestamps.
    pub nominal_frame_rate: Option<Rational>,
    pub rotation_degrees: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub format_name: String,
    pub format_long_name: Option<String>,
    pub duration_us: Option<u64>,
    pub stream_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoInfo {
    pub container: ContainerInfo,
    pub streams: Vec<StreamInfo>,
    pub selected_video_stream: u32,
}

/// One decoded video frame. Pixel planes intentionally remain inside the decoder in Phase 2.
///
/// `index` is a sequential identity within `decode_epoch`; it is never used to derive time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodedFrame {
    pub source_id: u64,
    pub stream_index: u32,
    pub decode_epoch: u64,
    pub index: u64,
    pub presentation_timestamp: Option<MediaTimestamp>,
    pub duration: Option<MediaDuration>,
    pub keyframe: bool,
    pub corrupt: bool,
    pub width: u32,
    pub height: u32,
    pub pixel_format: Option<String>,
}

impl DecodedFrame {
    pub fn timestamp_us(&self) -> Option<i64> {
        self.presentation_timestamp?.to_microseconds()
    }

    pub fn duration_us(&self) -> Option<i64> {
        self.duration?.to_microseconds()
    }
}

/// Metadata extracted from the source video itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoMetadata {
    pub duration_us: u64,
    pub width: u32,
    pub height: u32,
    pub estimated_frame_rate: Option<f64>,
    pub rotation_degrees: i32,
}

impl VideoMetadata {
    /// Reject values that are unusable or dangerous to surface as trusted metadata.
    pub fn validate(&self) -> Result<(), FrameScopeError> {
        if self.width == 0 || self.height == 0 {
            return Err(FrameScopeError::InvalidMetadata(
                "video dimensions must be non-zero".into(),
            ));
        }
        if self.width > 65_535 || self.height > 65_535 {
            return Err(FrameScopeError::InvalidMetadata(
                "video dimensions exceed Phase 1 safety bounds".into(),
            ));
        }
        if self.duration_us == 0 {
            return Err(FrameScopeError::InvalidMetadata(
                "video duration must be non-zero".into(),
            ));
        }
        if self.duration_us > i64::MAX as u64 {
            return Err(FrameScopeError::InvalidMetadata(
                "video duration exceeds the Android bridge range".into(),
            ));
        }
        if !matches!(self.rotation_degrees, 0 | 90 | 180 | 270) {
            return Err(FrameScopeError::InvalidMetadata(
                "rotation must be 0, 90, 180, or 270 degrees".into(),
            ));
        }
        if self
            .estimated_frame_rate
            .is_some_and(|fps| !fps.is_finite() || fps <= 0.0 || fps > 1_000.0)
        {
            return Err(FrameScopeError::InvalidMetadata(
                "estimated frame rate is outside safety bounds".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum FrameScopeError {
    #[error("unsupported video format: {0}")]
    UnsupportedFormat(String),
    #[error("unsupported video codec: {0}")]
    UnsupportedCodec(String),
    #[error("invalid video source: {0}")]
    InvalidSource(String),
    #[error("malformed video container: {0}")]
    MalformedContainer(String),
    #[error("no video track was found")]
    NoVideoTrack,
    #[error("video decoder failure: {0}")]
    DecoderFailure(String),
    #[error("video operation was cancelled")]
    Cancelled,
    #[error("seeking is unavailable: {0}")]
    SeekUnavailable(String),
    #[error("video backend is unavailable: {0}")]
    BackendUnavailable(String),
    #[error("invalid video metadata: {0}")]
    InvalidMetadata(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("native bridge error: {0}")]
    Bridge(String),
}

impl FrameScopeError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedFormat(_) => "unsupported_format",
            Self::UnsupportedCodec(_) => "unsupported_codec",
            Self::InvalidSource(_) => "invalid_source",
            Self::MalformedContainer(_) => "malformed_container",
            Self::NoVideoTrack => "no_video_track",
            Self::DecoderFailure(_) => "decoder_failure",
            Self::Cancelled => "cancelled",
            Self::SeekUnavailable(_) => "seek_unavailable",
            Self::BackendUnavailable(_) => "backend_unavailable",
            Self::InvalidMetadata(_) => "invalid_metadata",
            Self::Io(_) => "io_error",
            Self::Bridge(_) => "bridge_error",
        }
    }
}

impl From<std::io::Error> for FrameScopeError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_metadata() -> VideoMetadata {
        VideoMetadata {
            duration_us: 10_000_000,
            width: 1920,
            height: 1080,
            estimated_frame_rate: Some(29.97),
            rotation_degrees: 0,
        }
    }

    #[test]
    fn accepts_sane_metadata() {
        assert!(valid_metadata().validate().is_ok());
    }

    #[test]
    fn rejects_zero_dimensions() {
        let mut metadata = valid_metadata();
        metadata.width = 0;
        assert!(matches!(
            metadata.validate(),
            Err(FrameScopeError::InvalidMetadata(_))
        ));
    }

    #[test]
    fn rejects_duration_outside_android_bridge_range() {
        let mut metadata = valid_metadata();
        metadata.duration_us = i64::MAX as u64 + 1;
        assert!(metadata.validate().is_err());
    }

    #[test]
    fn rejects_non_finite_fps() {
        let mut metadata = valid_metadata();
        metadata.estimated_frame_rate = Some(f64::INFINITY);
        assert!(metadata.validate().is_err());
    }

    #[test]
    fn timestamp_conversion_is_exact_and_signed() {
        let time_base = TimeBase::new(1, 90_000).unwrap();
        assert_eq!(
            MediaTimestamp {
                ticks: 9_000,
                time_base,
            }
            .to_microseconds(),
            Some(100_000)
        );
        assert_eq!(
            MediaTimestamp {
                ticks: -4_500,
                time_base,
            }
            .to_microseconds(),
            Some(-50_000)
        );
    }

    #[test]
    fn rejects_invalid_time_bases_and_rates() {
        assert!(TimeBase::new(0, 1_000).is_none());
        assert!(TimeBase::new(1, 0).is_none());
        assert!(Rational::new(-1, 30).is_none());
    }

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(
            FrameScopeError::UnsupportedFormat("x".into()).code(),
            "unsupported_format"
        );
        assert_eq!(FrameScopeError::NoVideoTrack.code(), "no_video_track");
        assert_eq!(FrameScopeError::Cancelled.code(), "cancelled");
        assert_eq!(
            FrameScopeError::DecoderFailure("x".into()).code(),
            "decoder_failure"
        );
    }
}
