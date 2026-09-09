//! Bounded orchestration for non-contiguous similar-frame retrieval.
//!
//! The authoritative frame index remains timeline truth. A valid descriptor store is reused without
//! opening a decoder. A missing, stale, incomplete, or corrupt store is rebuilt by consuming exactly
//! one source-quality RGBA frame for each authoritative indexed frame, in presentation-frame order.

use crate::{IndexedRgbaFrame, IndexedRgbaStream, SimilaritySourceError};
use framescope_cache::{FrameId, FrameIndex, FrameIndexEntry, FrameIndexError};
use framescope_similarity_store::global::{
    GlobalSimilarityError, GlobalSimilarityPolicy, GlobalSimilarityQuery, GlobalSimilarityStore,
    GlobalSimilarityStoreKey, GlobalSimilarityStoreLoad, descriptor_for_frame,
};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalSimilarityAnalysisDisposition {
    Reused,
    Built,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalSimilarityAnalysisSummary {
    pub descriptor_count: u64,
    pub disposition: GlobalSimilarityAnalysisDisposition,
}

#[derive(Debug, Clone)]
pub struct GlobalSimilarityNavigator {
    store_root: PathBuf,
    key: GlobalSimilarityStoreKey,
    frame_count: u64,
}

impl GlobalSimilarityNavigator {
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    pub fn query(
        &self,
        target_frame: FrameId,
        policy: GlobalSimilarityPolicy,
    ) -> Result<GlobalSimilarityQuery, GlobalSimilarityNavigationError> {
        if target_frame.0 >= self.frame_count {
            return Err(GlobalSimilarityNavigationError::FrameOutsideIndex {
                frame_id: target_frame,
                frame_count: self.frame_count,
            });
        }
        let store = GlobalSimilarityStore::new(&self.store_root);
        Ok(store.query(&self.key, self.frame_count, target_frame, policy)?)
    }
}

#[derive(Debug, Clone)]
pub struct GlobalSimilarityAnalysis {
    pub summary: GlobalSimilarityAnalysisSummary,
    pub navigator: GlobalSimilarityNavigator,
}

#[derive(Debug, Error)]
pub enum GlobalSimilarityNavigationError {
    #[error("source identity is not strong enough for reusable global similarity")]
    UnsafeSourceIdentity,
    #[error("frame index is not complete")]
    IncompleteIndex,
    #[error("global similarity analysis was cancelled")]
    Cancelled,
    #[error(transparent)]
    Source(#[from] SimilaritySourceError),
    #[error(transparent)]
    Index(#[from] FrameIndexError),
    #[error(transparent)]
    Store(#[from] GlobalSimilarityError),
    #[error("global similarity source/index timeline mismatch: {0}")]
    TimelineMismatch(String),
    #[error("frame {frame_id:?} is outside completed index containing {frame_count} frames")]
    FrameOutsideIndex { frame_id: FrameId, frame_count: u64 },
    #[error("fresh global similarity persistence failed validation: {0}")]
    InvalidFreshStore(String),
}

/// Reuse or build the global similar-frame acceleration structure.
///
/// `source_factory` is lazy. When a valid persistent store exists it is never called. During a
/// rebuild descriptors are generated and written incrementally, so memory does not grow with video
/// length. `is_cancelled` is checked before decoder open, around every source read, and before the
/// transactional completion marker is written.
pub fn open_or_build_global_similarity<S, F, C>(
    index: &FrameIndex,
    store_root: impl AsRef<Path>,
    mut source_factory: F,
    mut is_cancelled: C,
) -> Result<GlobalSimilarityAnalysis, GlobalSimilarityNavigationError>
where
    S: IndexedRgbaStream,
    F: FnMut() -> Result<S, SimilaritySourceError>,
    C: FnMut() -> bool,
{
    if !index.source_identity().is_reuse_safe() {
        return Err(GlobalSimilarityNavigationError::UnsafeSourceIdentity);
    }
    let frame_count = index
        .frame_count()?
        .ok_or(GlobalSimilarityNavigationError::IncompleteIndex)?;
    let store_root = store_root.as_ref().to_path_buf();
    let store = GlobalSimilarityStore::new(&store_root);
    let key = GlobalSimilarityStoreKey::new(
        index.source_identity().clone(),
        index.stream_identity().clone(),
    )?;

    let disposition = match store.load(&key, frame_count)? {
        GlobalSimilarityStoreLoad::Reused { descriptor_count } => {
            if descriptor_count != frame_count {
                return Err(GlobalSimilarityNavigationError::InvalidFreshStore(format!(
                    "reused descriptor count {descriptor_count} differs from frame count {frame_count}"
                )));
            }
            GlobalSimilarityAnalysisDisposition::Reused
        }
        GlobalSimilarityStoreLoad::Missing
        | GlobalSimilarityStoreLoad::InvalidatedStale
        | GlobalSimilarityStoreLoad::InvalidatedIncomplete
        | GlobalSimilarityStoreLoad::InvalidatedCorrupt => {
            if is_cancelled() {
                return Err(GlobalSimilarityNavigationError::Cancelled);
            }
            let mut source = map_cancelled_source(source_factory())?;
            let mut writer = store.begin(&key)?;

            for raw_frame_id in 0..frame_count {
                if is_cancelled() {
                    return Err(GlobalSimilarityNavigationError::Cancelled);
                }
                let expected_frame_id = FrameId(raw_frame_id);
                let entry = index.entry(expected_frame_id)?.ok_or_else(|| {
                    GlobalSimilarityNavigationError::TimelineMismatch(format!(
                        "authoritative frame {raw_frame_id} disappeared during global analysis"
                    ))
                })?;
                let source_frame = map_cancelled_source(source.next_frame())?.ok_or_else(|| {
                    GlobalSimilarityNavigationError::TimelineMismatch(format!(
                        "source reached EOF before indexed frame {raw_frame_id}"
                    ))
                })?;
                if is_cancelled() {
                    return Err(GlobalSimilarityNavigationError::Cancelled);
                }
                validate_source_frame(index, &entry, &source_frame)?;
                writer.append(descriptor_for_frame(entry.frame_id, &source_frame.pixels)?)?;
            }

            if is_cancelled() {
                return Err(GlobalSimilarityNavigationError::Cancelled);
            }
            if map_cancelled_source(source.next_frame())?.is_some() {
                return Err(GlobalSimilarityNavigationError::TimelineMismatch(
                    "source emitted frames beyond the completed authoritative index".into(),
                ));
            }
            if is_cancelled() {
                return Err(GlobalSimilarityNavigationError::Cancelled);
            }

            let written = writer.finish()?;
            if written != frame_count {
                return Err(GlobalSimilarityNavigationError::InvalidFreshStore(format!(
                    "wrote {written} descriptors for {frame_count} indexed frames"
                )));
            }
            match store.load(&key, frame_count)? {
                GlobalSimilarityStoreLoad::Reused { descriptor_count }
                    if descriptor_count == frame_count => {}
                other => {
                    return Err(GlobalSimilarityNavigationError::InvalidFreshStore(format!(
                        "fresh store did not reopen as reusable: {other:?}"
                    )));
                }
            }
            GlobalSimilarityAnalysisDisposition::Built
        }
    };

    Ok(GlobalSimilarityAnalysis {
        summary: GlobalSimilarityAnalysisSummary {
            descriptor_count: frame_count,
            disposition,
        },
        navigator: GlobalSimilarityNavigator {
            store_root,
            key,
            frame_count,
        },
    })
}

fn map_cancelled_source<T>(
    result: Result<T, SimilaritySourceError>,
) -> Result<T, GlobalSimilarityNavigationError> {
    match result {
        Err(error) if error.code == "cancelled" => Err(GlobalSimilarityNavigationError::Cancelled),
        Err(error) => Err(GlobalSimilarityNavigationError::Source(error)),
        Ok(value) => Ok(value),
    }
}

fn validate_source_frame(
    index: &FrameIndex,
    entry: &FrameIndexEntry,
    source: &IndexedRgbaFrame,
) -> Result<(), GlobalSimilarityNavigationError> {
    if source.frame_id != entry.frame_id
        || source.presentation_timestamp != entry.presentation_timestamp
        || source.duration != entry.duration
        || source.keyframe != entry.keyframe
        || source.corrupt != entry.corrupt
    {
        return Err(GlobalSimilarityNavigationError::TimelineMismatch(format!(
            "source frame {} does not match authoritative index frame {}",
            source.frame_id.0, entry.frame_id.0
        )));
    }
    let stream = index.stream_identity();
    if stream
        .width
        .is_some_and(|width| width != source.pixels.width)
        || stream
            .height
            .is_some_and(|height| height != source.pixels.height)
    {
        return Err(GlobalSimilarityNavigationError::TimelineMismatch(format!(
            "source frame {} dimensions {}x{} do not match indexed stream dimensions",
            source.frame_id.0, source.pixels.width, source.pixels.height
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{
        FrameIndexEntry, FrameIndexStreamIdentity, KeyframeAnchor, OwnedRgbaFrame, SourceIdentity,
    };
    use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};
    use framescope_similarity_store::global::GlobalSimilarityPolicy;
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

    fn temp_root(label: &str) -> PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "framescope-global-navigation-{label}-{}-{id}",
            std::process::id()
        ))
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

    fn complete_index(root: &Path, frame_count: u64, strong_identity: bool) -> FrameIndex {
        let source = if strong_identity {
            SourceIdentity::new(
                12_345,
                None,
                Some(format!(
                    "test-content-{}",
                    NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
                )),
            )
        } else {
            SourceIdentity::metadata_only(Some(12_345), None, None)
        };
        let (mut index, _) =
            FrameIndex::open_or_create(root.join("index.sqlite3"), source, stream_identity())
                .unwrap();
        let entries: Vec<_> = (0..frame_count).map(entry).collect();
        index.append_batch(&entries).unwrap();
        index.mark_complete().unwrap();
        index
    }

    #[test]
    fn a_b_a_sequence_finds_non_contiguous_match_without_changing_adjacent_groups() {
        let root = temp_root("aba");
        let index = complete_index(&root, 3, true);
        let analysis = open_or_build_global_similarity(
            &index,
            root.join("similarity"),
            || Ok(fake(&[10, 240, 10])),
            || false,
        )
        .unwrap();
        assert_eq!(
            analysis.summary.disposition,
            GlobalSimilarityAnalysisDisposition::Built
        );
        let result = analysis
            .navigator
            .query(FrameId(0), GlobalSimilarityPolicy::default())
            .unwrap();
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].frame_id, FrameId(2));
        assert_eq!(result.matches[0].similarity, 10_000);
        drop(analysis);
        drop(index);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn valid_global_descriptors_are_reused_without_opening_source() {
        let root = temp_root("reuse");
        let index = complete_index(&root, 3, true);
        let first = open_or_build_global_similarity(
            &index,
            root.join("similarity"),
            || Ok(fake(&[10, 240, 10])),
            || false,
        )
        .unwrap();
        drop(first);

        let opened = Cell::new(false);
        let second = open_or_build_global_similarity(
            &index,
            root.join("similarity"),
            || {
                opened.set(true);
                Ok(fake(&[1, 2, 3]))
            },
            || false,
        )
        .unwrap();
        assert_eq!(
            second.summary.disposition,
            GlobalSimilarityAnalysisDisposition::Reused
        );
        assert!(!opened.get());
        drop(second);
        drop(index);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn source_timing_mismatch_is_rejected_before_completion() {
        let root = temp_root("timing");
        let index = complete_index(&root, 2, true);
        let mut bad = fake(&[10, 10]);
        bad.frames[1].presentation_timestamp = Some(MediaTimestamp {
            ticks: 999,
            time_base: TimeBase::new(1, 1_000).unwrap(),
        });
        let mut source = Some(bad);
        let result = open_or_build_global_similarity(
            &index,
            root.join("similarity"),
            || Ok(source.take().unwrap()),
            || false,
        );
        assert!(matches!(
            result,
            Err(GlobalSimilarityNavigationError::TimelineMismatch(_))
        ));
        drop(index);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cancellation_before_build_does_not_open_source() {
        let root = temp_root("cancel");
        let index = complete_index(&root, 1, true);
        let opened = Cell::new(false);
        let result = open_or_build_global_similarity(
            &index,
            root.join("similarity"),
            || {
                opened.set(true);
                Ok(fake(&[10]))
            },
            || true,
        );
        assert!(matches!(
            result,
            Err(GlobalSimilarityNavigationError::Cancelled)
        ));
        assert!(!opened.get());
        drop(index);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn weak_identity_is_rejected_before_source_open() {
        let root = temp_root("weak");
        let index = complete_index(&root, 1, false);
        let opened = Cell::new(false);
        let result = open_or_build_global_similarity(
            &index,
            root.join("similarity"),
            || {
                opened.set(true);
                Ok(fake(&[10]))
            },
            || false,
        );
        assert!(matches!(
            result,
            Err(GlobalSimilarityNavigationError::UnsafeSourceIdentity)
        ));
        assert!(!opened.get());
        drop(index);
        let _ = fs::remove_dir_all(root);
    }
}
