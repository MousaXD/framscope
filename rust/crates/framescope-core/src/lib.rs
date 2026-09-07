//! Shared domain types for FrameScope.
//! This crate is platform-neutral and must not depend on Android APIs.

use serde::{Deserialize, Serialize};
use thiserror::Error;

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
    #[error("malformed video container: {0}")]
    MalformedContainer(String),
    #[error("no video track was found")]
    NoVideoTrack,
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
            Self::MalformedContainer(_) => "malformed_container",
            Self::NoVideoTrack => "no_video_track",
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
    fn error_codes_are_stable() {
        assert_eq!(
            FrameScopeError::UnsupportedFormat("x".into()).code(),
            "unsupported_format"
        );
        assert_eq!(FrameScopeError::NoVideoTrack.code(), "no_video_track");
    }
}
