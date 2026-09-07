use framescope_core::{MediaDuration, MediaKind, MediaTimestamp, StreamInfo, TimeBase};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const FRAME_INDEX_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FrameId(pub u64);

impl FrameId {
    pub const ZERO: Self = Self(0);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameIndexStreamIdentity {
    pub stream_index: u32,
    pub codec_id: i32,
    pub codec_name: String,
    pub time_base: TimeBase,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl FrameIndexStreamIdentity {
    pub fn from_stream(stream: &StreamInfo) -> Result<Self, FrameIndexError> {
        if stream.media_kind != MediaKind::Video {
            return Err(FrameIndexError::InvalidState(
                "frame indexes can only bind to a video stream".into(),
            ));
        }
        let time_base = stream.time_base.ok_or_else(|| {
            FrameIndexError::InvalidState("selected video stream has no valid time base".into())
        })?;
        Ok(Self {
            stream_index: stream.index,
            codec_id: stream.codec.id,
            codec_name: stream.codec.name.clone(),
            time_base,
            width: stream.width,
            height: stream.height,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KeyframeAnchor {
    StreamStart,
    Keyframe {
        frame_id: FrameId,
        presentation_timestamp: Option<MediaTimestamp>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameIndexEntry {
    pub frame_id: FrameId,
    pub presentation_timestamp: Option<MediaTimestamp>,
    pub duration: Option<MediaDuration>,
    pub keyframe: bool,
    pub corrupt: bool,
    pub anchor: KeyframeAnchor,
}

impl FrameIndexEntry {
    pub fn timestamp_us(&self) -> Option<i64> {
        self.presentation_timestamp?.to_microseconds()
    }

    pub fn validate(&self, stream: &FrameIndexStreamIdentity) -> Result<(), FrameIndexError> {
        if let Some(timestamp) = self.presentation_timestamp {
            if timestamp.time_base != stream.time_base {
                return Err(FrameIndexError::TimeBaseMismatch);
            }
            timestamp.to_microseconds().ok_or_else(|| {
                FrameIndexError::InvalidState("frame timestamp overflows microseconds".into())
            })?;
        }
        if let Some(duration) = self.duration {
            if duration.ticks <= 0 {
                return Err(FrameIndexError::InvalidState(
                    "frame duration must be positive when present".into(),
                ));
            }
            if duration.time_base != stream.time_base {
                return Err(FrameIndexError::TimeBaseMismatch);
            }
        }
        match &self.anchor {
            KeyframeAnchor::StreamStart => {
                if self.keyframe && !self.corrupt {
                    return Err(FrameIndexError::InvalidState(
                        "clean keyframe must anchor itself".into(),
                    ));
                }
            }
            KeyframeAnchor::Keyframe {
                frame_id,
                presentation_timestamp,
            } => {
                if frame_id.0 > self.frame_id.0 {
                    return Err(FrameIndexError::InvalidState(
                        "keyframe anchor cannot follow its target frame".into(),
                    ));
                }
                if let Some(timestamp) = presentation_timestamp {
                    if timestamp.time_base != stream.time_base {
                        return Err(FrameIndexError::TimeBaseMismatch);
                    }
                    timestamp.to_microseconds().ok_or_else(|| {
                        FrameIndexError::InvalidState(
                            "keyframe anchor timestamp overflows microseconds".into(),
                        )
                    })?;
                }
                if self.keyframe
                    && !self.corrupt
                    && (*frame_id != self.frame_id
                        || *presentation_timestamp != self.presentation_timestamp)
                {
                    return Err(FrameIndexError::InvalidState(
                        "clean keyframe must anchor itself with the same timestamp".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameIndexLifecycle {
    Building,
    Incomplete,
    Complete,
    FailedRecoverable,
}

impl FrameIndexLifecycle {
    pub(crate) fn as_i64(self) -> i64 {
        match self {
            Self::Building => 1,
            Self::Incomplete => 2,
            Self::Complete => 3,
            Self::FailedRecoverable => 4,
        }
    }

    pub(crate) fn from_i64(value: i64) -> Result<Self, FrameIndexError> {
        match value {
            1 => Ok(Self::Building),
            2 => Ok(Self::Incomplete),
            3 => Ok(Self::Complete),
            4 => Ok(Self::FailedRecoverable),
            _ => Err(FrameIndexError::InvalidState(format!(
                "unknown frame-index lifecycle value {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameIndexStatus {
    pub lifecycle: FrameIndexLifecycle,
    pub indexed_frames: u64,
    pub frame_count: Option<u64>,
    pub last_frame_id: Option<FrameId>,
    pub last_presentation_timestamp: Option<MediaTimestamp>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameIndexOpenDisposition {
    Created,
    Reused,
    RebuiltStaleSource,
    RebuiltUnverifiableSource,
    RecoveredCorruptState,
    RecreatedUnsupportedSchema,
}

#[derive(Debug, Error)]
pub enum FrameIndexError {
    #[error("SQLite frame-index error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("frame-index I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame-index serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid persistent frame-index state: {0}")]
    InvalidState(String),
    #[error("unsupported frame-index schema version {found}")]
    UnsupportedSchema { found: i64 },
    #[error("timestamp uses a different time base from the indexed stream")]
    TimeBaseMismatch,
}

impl FrameIndexError {
    pub(crate) fn recreation_disposition(&self) -> Option<FrameIndexOpenDisposition> {
        match self {
            Self::UnsupportedSchema { .. } => {
                Some(FrameIndexOpenDisposition::RecreatedUnsupportedSchema)
            }
            Self::InvalidState(_) | Self::Serialization(_) => {
                Some(FrameIndexOpenDisposition::RecoveredCorruptState)
            }
            Self::Sqlite(rusqlite::Error::SqliteFailure(error, _))
                if matches!(
                    error.code,
                    rusqlite::ErrorCode::DatabaseCorrupt
                        | rusqlite::ErrorCode::NotADatabase
                        | rusqlite::ErrorCode::SchemaChanged
                ) =>
            {
                Some(FrameIndexOpenDisposition::RecoveredCorruptState)
            }
            _ => None,
        }
    }
}
