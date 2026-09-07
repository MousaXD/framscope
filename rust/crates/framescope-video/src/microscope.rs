use framescope_cache::{FrameId, FrameIndex, FrameIndexEntry, FrameIndexError, FrameIndexLifecycle};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicroscopeTimestampSelection {
    AtOrBefore,
    AtOrAfter,
    Nearest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicroscopeStep {
    Previous,
    Next,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicroscopeTarget {
    pub entry: FrameIndexEntry,
    pub frame_count: u64,
}

impl MicroscopeTarget {
    pub fn frame_id(&self) -> FrameId {
        self.entry.frame_id
    }

    pub fn has_previous(&self) -> bool {
        self.entry.frame_id != FrameId::ZERO
    }

    pub fn has_next(&self) -> bool {
        self.entry
            .frame_id
            .0
            .checked_add(1)
            .is_some_and(|next| next < self.frame_count)
    }
}

#[derive(Debug, Error)]
pub enum MicroscopeNavigationError {
    #[error("frame-index lookup failed: {0}")]
    Index(#[from] FrameIndexError),
    #[error("frame microscope requires a complete frame index")]
    IncompleteIndex,
    #[error("complete frame index contains no frames")]
    EmptyIndex,
    #[error("requested frame is outside the complete frame index")]
    FrameNotIndexed,
    #[error("requested timestamp does not resolve to an indexed frame")]
    TimestampNotIndexed,
    #[error("already at the first frame")]
    AtFirstFrame,
    #[error("already at the last frame")]
    AtLastFrame,
    #[error("indexed frame timestamp cannot be normalized to microseconds")]
    TimestampOverflow,
}

/// Resolve an exact authoritative frame identity for the microscope UI.
///
/// This performs an indexed O(log N)/primary-key lookup and never derives time from FPS.
pub fn microscope_target(
    index: &FrameIndex,
    frame_id: FrameId,
) -> Result<MicroscopeTarget, MicroscopeNavigationError> {
    let frame_count = complete_frame_count(index)?;
    if frame_id.0 >= frame_count {
        return Err(MicroscopeNavigationError::FrameNotIndexed);
    }
    let entry = index
        .entry(frame_id)?
        .ok_or(MicroscopeNavigationError::FrameNotIndexed)?;
    Ok(MicroscopeTarget { entry, frame_count })
}

/// Step exactly one presentation frame using persistent `FrameId`, never decoder-local counters.
pub fn microscope_step(
    index: &FrameIndex,
    current: FrameId,
    step: MicroscopeStep,
) -> Result<MicroscopeTarget, MicroscopeNavigationError> {
    let frame_count = complete_frame_count(index)?;
    let next = match step {
        MicroscopeStep::Previous => {
            if current == FrameId::ZERO {
                return Err(MicroscopeNavigationError::AtFirstFrame);
            }
            FrameId(current.0 - 1)
        }
        MicroscopeStep::Next => {
            let value = current
                .0
                .checked_add(1)
                .ok_or(MicroscopeNavigationError::AtLastFrame)?;
            if value >= frame_count {
                return Err(MicroscopeNavigationError::AtLastFrame);
            }
            FrameId(value)
        }
    };
    microscope_target(index, next)
}

/// Resolve a user-entered microsecond timestamp with explicit boundary semantics.
///
/// The persistent index stores checked normalized microseconds alongside exact PTS/time-base data,
/// so this remains VFR-safe and does not scan the full timeline.
pub fn microscope_timestamp_us(
    index: &FrameIndex,
    timestamp_us: i64,
    selection: MicroscopeTimestampSelection,
) -> Result<MicroscopeTarget, MicroscopeNavigationError> {
    complete_frame_count(index)?;
    let entry = match selection {
        MicroscopeTimestampSelection::AtOrBefore => index.frame_at_or_before_us(timestamp_us)?,
        MicroscopeTimestampSelection::AtOrAfter => index.frame_at_or_after_us(timestamp_us)?,
        MicroscopeTimestampSelection::Nearest => nearest_timestamp_us(index, timestamp_us)?,
    }
    .ok_or(MicroscopeNavigationError::TimestampNotIndexed)?;
    microscope_target(index, entry.frame_id)
}

fn complete_frame_count(index: &FrameIndex) -> Result<u64, MicroscopeNavigationError> {
    let status = index.status()?;
    if status.lifecycle != FrameIndexLifecycle::Complete {
        return Err(MicroscopeNavigationError::IncompleteIndex);
    }
    let count = status.frame_count.ok_or(MicroscopeNavigationError::EmptyIndex)?;
    if count == 0 {
        return Err(MicroscopeNavigationError::EmptyIndex);
    }
    Ok(count)
}

fn nearest_timestamp_us(
    index: &FrameIndex,
    timestamp_us: i64,
) -> Result<Option<FrameIndexEntry>, MicroscopeNavigationError> {
    let before = index.frame_at_or_before_us(timestamp_us)?;
    let after = index.frame_at_or_after_us(timestamp_us)?;
    match (before, after) {
        (None, None) => Ok(None),
        (Some(entry), None) | (None, Some(entry)) => Ok(Some(entry)),
        (Some(before), Some(after)) => {
            let before_us = before
                .timestamp_us()
                .ok_or(MicroscopeNavigationError::TimestampOverflow)?;
            let after_us = after
                .timestamp_us()
                .ok_or(MicroscopeNavigationError::TimestampOverflow)?;
            let before_distance = timestamp_us
                .checked_sub(before_us)
                .and_then(|value| u64::try_from(value).ok())
                .ok_or(MicroscopeNavigationError::TimestampOverflow)?;
            let after_distance = after_us
                .checked_sub(timestamp_us)
                .and_then(|value| u64::try_from(value).ok())
                .ok_or(MicroscopeNavigationError::TimestampOverflow)?;
            if after_distance == 0 {
                // For repeated exact PTS values, preserve AtOrAfter's first-equal-frame semantics.
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
    use framescope_cache::{
        FrameIndexOpenDisposition, FrameIndexStreamIdentity, KeyframeAnchor, SourceIdentity,
    };
    use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn time_base() -> TimeBase {
        TimeBase::new(1, 1_000).unwrap()
    }

    fn stream() -> FrameIndexStreamIdentity {
        FrameIndexStreamIdentity {
            stream_index: 0,
            codec_id: 27,
            codec_name: "h264".into(),
            time_base: time_base(),
            width: Some(1920),
            height: Some(1080),
        }
    }

    fn temp_path(label: &str) -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("framescope-microscope-{label}-{}-{id}.sqlite3", std::process::id()))
    }

    fn entry(id: u64, ticks: i64, duration: i64) -> FrameIndexEntry {
        let timestamp = MediaTimestamp {
            ticks,
            time_base: time_base(),
        };
        FrameIndexEntry {
            frame_id: FrameId(id),
            presentation_timestamp: Some(timestamp),
            duration: Some(MediaDuration {
                ticks: duration,
                time_base: time_base(),
            }),
            keyframe: id == 0,
            corrupt: false,
            anchor: KeyframeAnchor::Keyframe {
                frame_id: FrameId(0),
                presentation_timestamp: Some(MediaTimestamp {
                    ticks: 0,
                    time_base: time_base(),
                }),
            },
        }
    }

    fn complete_index(label: &str) -> (FrameIndex, PathBuf) {
        let path = temp_path(label);
        let source = SourceIdentity::new(4096, Some(7), Some("strong-test-tag".into()));
        let (mut index, disposition) = FrameIndex::open_or_create(&path, source, stream()).unwrap();
        assert_eq!(disposition, FrameIndexOpenDisposition::Created);
        index.mark_building().unwrap();
        index
            .append_batch(&[
                entry(0, 0, 33),
                entry(1, 33, 51),
                entry(2, 84, 17),
                entry(3, 101, 64),
            ])
            .unwrap();
        index.mark_complete().unwrap();
        (index, path)
    }

    #[test]
    fn exact_previous_next_uses_global_frame_ids() {
        let (index, path) = complete_index("step");
        let target = microscope_target(&index, FrameId(2)).unwrap();
        assert_eq!(target.frame_id(), FrameId(2));
        assert!(target.has_previous());
        assert!(target.has_next());
        assert_eq!(
            microscope_step(&index, FrameId(2), MicroscopeStep::Previous)
                .unwrap()
                .frame_id(),
            FrameId(1)
        );
        assert_eq!(
            microscope_step(&index, FrameId(2), MicroscopeStep::Next)
                .unwrap()
                .frame_id(),
            FrameId(3)
        );
        assert!(matches!(
            microscope_step(&index, FrameId::ZERO, MicroscopeStep::Previous),
            Err(MicroscopeNavigationError::AtFirstFrame)
        ));
        assert!(matches!(
            microscope_step(&index, FrameId(3), MicroscopeStep::Next),
            Err(MicroscopeNavigationError::AtLastFrame)
        ));
        drop(index);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn timestamp_selection_preserves_vfr_boundaries() {
        let (index, path) = complete_index("timestamp");
        assert_eq!(
            microscope_timestamp_us(&index, 70_000, MicroscopeTimestampSelection::AtOrBefore)
                .unwrap()
                .frame_id(),
            FrameId(1)
        );
        assert_eq!(
            microscope_timestamp_us(&index, 70_000, MicroscopeTimestampSelection::AtOrAfter)
                .unwrap()
                .frame_id(),
            FrameId(2)
        );
        assert_eq!(
            microscope_timestamp_us(&index, 70_000, MicroscopeTimestampSelection::Nearest)
                .unwrap()
                .frame_id(),
            FrameId(2)
        );
        drop(index);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn incomplete_index_is_never_exposed_as_navigable() {
        let path = temp_path("incomplete");
        let source = SourceIdentity::new(4096, Some(7), Some("strong-test-tag".into()));
        let (index, _) = FrameIndex::open_or_create(&path, source, stream()).unwrap();
        assert!(matches!(
            microscope_target(&index, FrameId::ZERO),
            Err(MicroscopeNavigationError::IncompleteIndex)
        ));
        drop(index);
        let _ = std::fs::remove_file(path);
    }
}
