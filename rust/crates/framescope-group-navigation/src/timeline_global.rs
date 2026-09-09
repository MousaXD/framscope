//! Global similar-frame analysis over a caller-supplied authoritative timeline.
//!
//! This is the integration seam for live microscope sessions. The caller can fetch one immutable
//! [`FrameIndexEntry`] at a time under a short-lived session lock while source-quality decoding,
//! descriptor generation, SQLite persistence, and query confirmation run without holding that lock.

use crate::{IndexedRgbaFrame, IndexedRgbaStream, SimilaritySourceError};
use framescope_cache::{FrameId, FrameIndexEntry, FrameIndexStreamIdentity, SourceIdentity};
use framescope_similarity_store::global::{
    GlobalSimilarityError, GlobalSimilarityPolicy, GlobalSimilarityQuery, GlobalSimilarityStore,
    GlobalSimilarityStoreKey, GlobalSimilarityStoreLoad, descriptor_for_frame,
};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineGlobalSimilarityDisposition {
    Reused,
    Built,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineGlobalSimilarityResult {
    pub disposition: TimelineGlobalSimilarityDisposition,
    pub descriptor_count: u64,
    pub query: GlobalSimilarityQuery,
}

#[derive(Debug, Error)]
pub enum TimelineGlobalSimilarityError {
    #[error("source identity is not strong enough for reusable global similarity")]
    UnsafeSourceIdentity,
    #[error("global similarity analysis was cancelled")]
    Cancelled,
    #[error(transparent)]
    Source(#[from] SimilaritySourceError),
    #[error(transparent)]
    Store(#[from] GlobalSimilarityError),
    #[error("authoritative timeline provider failed: {0}")]
    TimelineProvider(String),
    #[error("global similarity source/index timeline mismatch: {0}")]
    TimelineMismatch(String),
    #[error("frame {frame_id:?} is outside completed index containing {frame_count} frames")]
    FrameOutsideIndex { frame_id: FrameId, frame_count: u64 },
    #[error("fresh global similarity persistence failed validation: {0}")]
    InvalidFreshStore(String),
}

/// Reuse or build the global descriptor store and query one target frame.
///
/// `entry_for_frame` is deliberately invoked one frame at a time. A live-session caller may acquire
/// its own navigation lock only for that callback, clone the immutable authoritative entry, and
/// release the lock before any decode or descriptor work continues. `source_factory` remains lazy:
/// a valid persistent store is queried without opening the video source.
#[allow(clippy::too_many_arguments)]
pub fn open_or_build_and_query_with_timeline<S, F, C, E>(
    source_identity: &SourceIdentity,
    stream_identity: &FrameIndexStreamIdentity,
    frame_count: u64,
    mut entry_for_frame: E,
    store_root: impl AsRef<Path>,
    target_frame: FrameId,
    policy: GlobalSimilarityPolicy,
    mut source_factory: F,
    mut is_cancelled: C,
) -> Result<TimelineGlobalSimilarityResult, TimelineGlobalSimilarityError>
where
    S: IndexedRgbaStream,
    F: FnMut() -> Result<S, SimilaritySourceError>,
    C: FnMut() -> bool,
    E: FnMut(FrameId) -> Result<Option<FrameIndexEntry>, TimelineGlobalSimilarityError>,
{
    if !source_identity.is_reuse_safe() {
        return Err(TimelineGlobalSimilarityError::UnsafeSourceIdentity);
    }
    if target_frame.0 >= frame_count {
        return Err(TimelineGlobalSimilarityError::FrameOutsideIndex {
            frame_id: target_frame,
            frame_count,
        });
    }

    let store = GlobalSimilarityStore::new(store_root);
    let key = GlobalSimilarityStoreKey::new(source_identity.clone(), stream_identity.clone())?;
    let disposition = match store.load(&key, frame_count)? {
        GlobalSimilarityStoreLoad::Reused { descriptor_count } => {
            if descriptor_count != frame_count {
                return Err(TimelineGlobalSimilarityError::InvalidFreshStore(format!(
                    "reused descriptor count {descriptor_count} differs from frame count {frame_count}"
                )));
            }
            TimelineGlobalSimilarityDisposition::Reused
        }
        GlobalSimilarityStoreLoad::Missing
        | GlobalSimilarityStoreLoad::InvalidatedStale
        | GlobalSimilarityStoreLoad::InvalidatedIncomplete
        | GlobalSimilarityStoreLoad::InvalidatedCorrupt => {
            if is_cancelled() {
                return Err(TimelineGlobalSimilarityError::Cancelled);
            }
            let mut source = map_cancelled_source(source_factory())?;
            let mut writer = store.begin(&key)?;

            for raw_frame_id in 0..frame_count {
                if is_cancelled() {
                    return Err(TimelineGlobalSimilarityError::Cancelled);
                }
                let expected_frame_id = FrameId(raw_frame_id);
                let entry = entry_for_frame(expected_frame_id)?.ok_or_else(|| {
                    TimelineGlobalSimilarityError::TimelineMismatch(format!(
                        "authoritative frame {raw_frame_id} disappeared during global analysis"
                    ))
                })?;
                if entry.frame_id != expected_frame_id {
                    return Err(TimelineGlobalSimilarityError::TimelineMismatch(format!(
                        "timeline provider returned frame {} for requested frame {raw_frame_id}",
                        entry.frame_id.0
                    )));
                }
                let source_frame = map_cancelled_source(source.next_frame())?.ok_or_else(|| {
                    TimelineGlobalSimilarityError::TimelineMismatch(format!(
                        "source reached EOF before indexed frame {raw_frame_id}"
                    ))
                })?;
                if is_cancelled() {
                    return Err(TimelineGlobalSimilarityError::Cancelled);
                }
                validate_source_frame(stream_identity, &entry, &source_frame)?;
                writer.append(descriptor_for_frame(entry.frame_id, &source_frame.pixels)?)?;
            }

            if is_cancelled() {
                return Err(TimelineGlobalSimilarityError::Cancelled);
            }
            if map_cancelled_source(source.next_frame())?.is_some() {
                return Err(TimelineGlobalSimilarityError::TimelineMismatch(
                    "source emitted frames beyond the completed authoritative index".into(),
                ));
            }
            if is_cancelled() {
                return Err(TimelineGlobalSimilarityError::Cancelled);
            }

            let written = writer.finish()?;
            if written != frame_count {
                return Err(TimelineGlobalSimilarityError::InvalidFreshStore(format!(
                    "wrote {written} descriptors for {frame_count} indexed frames"
                )));
            }
            match store.load(&key, frame_count)? {
                GlobalSimilarityStoreLoad::Reused { descriptor_count }
                    if descriptor_count == frame_count => {}
                other => {
                    return Err(TimelineGlobalSimilarityError::InvalidFreshStore(format!(
                        "fresh store did not reopen as reusable: {other:?}"
                    )));
                }
            }
            TimelineGlobalSimilarityDisposition::Built
        }
    };

    if is_cancelled() {
        return Err(TimelineGlobalSimilarityError::Cancelled);
    }
    let query = store.query(&key, frame_count, target_frame, policy)?;
    Ok(TimelineGlobalSimilarityResult {
        disposition,
        descriptor_count: frame_count,
        query,
    })
}

fn map_cancelled_source<T>(
    result: Result<T, SimilaritySourceError>,
) -> Result<T, TimelineGlobalSimilarityError> {
    match result {
        Err(error) if error.code == "cancelled" => Err(TimelineGlobalSimilarityError::Cancelled),
        Err(error) => Err(TimelineGlobalSimilarityError::Source(error)),
        Ok(value) => Ok(value),
    }
}

fn validate_source_frame(
    stream: &FrameIndexStreamIdentity,
    entry: &FrameIndexEntry,
    source: &IndexedRgbaFrame,
) -> Result<(), TimelineGlobalSimilarityError> {
    if source.frame_id != entry.frame_id
        || source.presentation_timestamp != entry.presentation_timestamp
        || source.duration != entry.duration
        || source.keyframe != entry.keyframe
        || source.corrupt != entry.corrupt
    {
        return Err(TimelineGlobalSimilarityError::TimelineMismatch(format!(
            "source frame {} does not match authoritative index frame {}",
            source.frame_id.0, entry.frame_id.0
        )));
    }
    if stream
        .width
        .is_some_and(|width| width != source.pixels.width)
        || stream
            .height
            .is_some_and(|height| height != source.pixels.height)
    {
        return Err(TimelineGlobalSimilarityError::TimelineMismatch(format!(
            "source frame {} dimensions {}x{} do not match indexed stream dimensions",
            source.frame_id.0, source.pixels.width, source.pixels.height
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{KeyframeAnchor, OwnedRgbaFrame};
    use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

    fn temp_root(label: &str) -> PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "framescope-timeline-global-{label}-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn source_identity() -> SourceIdentity {
        SourceIdentity::new(
            12_345,
            None,
            Some(format!(
                "timeline-test-content-{}",
                NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
            )),
        )
    }

    fn stream_identity() -> FrameIndexStreamIdentity {
        FrameIndexStreamIdentity {
            stream_index: 0,
            codec_id: 1,
            codec_name: "test".into(),
            time_base: TimeBase::new(1, 1_000).unwrap(),
            width: Some(9),
            height: Some(8),
        }
    }

    fn entry(id: u64) -> FrameIndexEntry {
        let timestamp = MediaTimestamp {
            ticks: i64::try_from(id).unwrap() * 40,
            time_base: TimeBase::new(1, 1_000).unwrap(),
        };
        FrameIndexEntry {
            frame_id: FrameId(id),
            presentation_timestamp: Some(timestamp),
            duration: Some(MediaDuration {
                ticks: 40,
                time_base: timestamp.time_base,
            }),
            keyframe: true,
            corrupt: false,
            anchor: KeyframeAnchor::Keyframe {
                frame_id: FrameId(id),
                presentation_timestamp: Some(timestamp),
            },
        }
    }

    fn solid(value: u8) -> OwnedRgbaFrame {
        let mut bytes = Vec::with_capacity(9 * 8 * 4);
        for _ in 0..(9 * 8) {
            bytes.extend_from_slice(&[value, value, value, 255]);
        }
        OwnedRgbaFrame::new(9, 8, 36, bytes).unwrap()
    }

    fn source_frame(id: u64, value: u8) -> IndexedRgbaFrame {
        let entry = entry(id);
        IndexedRgbaFrame {
            frame_id: entry.frame_id,
            presentation_timestamp: entry.presentation_timestamp,
            duration: entry.duration,
            keyframe: entry.keyframe,
            corrupt: entry.corrupt,
            pixels: solid(value),
        }
    }

    struct FakeStream {
        frames: VecDeque<IndexedRgbaFrame>,
    }

    impl IndexedRgbaStream for FakeStream {
        fn next_frame(&mut self) -> Result<Option<IndexedRgbaFrame>, SimilaritySourceError> {
            Ok(self.frames.pop_front())
        }
    }

    fn fake(values: &[u8]) -> FakeStream {
        FakeStream {
            frames: values
                .iter()
                .enumerate()
                .map(|(id, value)| source_frame(id as u64, *value))
                .collect(),
        }
    }

    #[test]
    fn callback_timeline_finds_non_contiguous_a_b_a_match() {
        let root = temp_root("aba");
        let source = source_identity();
        let stream = stream_identity();
        let result = open_or_build_and_query_with_timeline(
            &source,
            &stream,
            3,
            |frame_id| Ok(Some(entry(frame_id.0))),
            &root,
            FrameId(0),
            GlobalSimilarityPolicy::default(),
            || Ok(fake(&[10, 240, 10])),
            || false,
        )
        .unwrap();
        assert_eq!(result.disposition, TimelineGlobalSimilarityDisposition::Built);
        assert_eq!(result.query.matches.len(), 1);
        assert_eq!(result.query.matches[0].frame_id, FrameId(2));
        assert_eq!(result.query.matches[0].similarity, 10_000);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reuse_does_not_open_source_or_consult_timeline_again() {
        let root = temp_root("reuse");
        let source = source_identity();
        let stream = stream_identity();
        open_or_build_and_query_with_timeline(
            &source,
            &stream,
            3,
            |frame_id| Ok(Some(entry(frame_id.0))),
            &root,
            FrameId(0),
            GlobalSimilarityPolicy::default(),
            || Ok(fake(&[10, 240, 10])),
            || false,
        )
        .unwrap();

        let source_opened = Cell::new(false);
        let timeline_read = Cell::new(false);
        let second = open_or_build_and_query_with_timeline(
            &source,
            &stream,
            3,
            |_frame_id| {
                timeline_read.set(true);
                Ok(Some(entry(0)))
            },
            &root,
            FrameId(0),
            GlobalSimilarityPolicy::default(),
            || {
                source_opened.set(true);
                Ok(fake(&[1, 2, 3]))
            },
            || false,
        )
        .unwrap();
        assert_eq!(second.disposition, TimelineGlobalSimilarityDisposition::Reused);
        assert!(!source_opened.get());
        assert!(!timeline_read.get());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_authoritative_entry_fails_without_completing_store() {
        let root = temp_root("missing");
        let source = source_identity();
        let stream = stream_identity();
        let result = open_or_build_and_query_with_timeline(
            &source,
            &stream,
            2,
            |frame_id| {
                if frame_id == FrameId(1) {
                    Ok(None)
                } else {
                    Ok(Some(entry(frame_id.0)))
                }
            },
            &root,
            FrameId(0),
            GlobalSimilarityPolicy::default(),
            || Ok(fake(&[10, 10])),
            || false,
        );
        assert!(matches!(
            result,
            Err(TimelineGlobalSimilarityError::TimelineMismatch(_))
        ));
        let _ = fs::remove_dir_all(root);
    }
}
