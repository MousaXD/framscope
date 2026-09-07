//! Constant-memory enumeration of persisted Phase 4 similarity-group representatives.
//!
//! Unique-frame extraction must not materialize every representative FrameId before export. This
//! crate provides a small cursor over the already-validated read-only group navigator, preserving
//! group order and exact progress while keeping only the most recent representative identity.

use framescope_cache::FrameId;
use framescope_group_navigation::{
    GroupDirection, GroupNavigationError, GroupNavigationTarget, SimilarityGroupNavigator,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GroupRepresentativeError {
    #[error(transparent)]
    Navigation(#[from] GroupNavigationError),
    #[error("persisted group navigation violated representative cursor invariants: {0}")]
    InvalidNavigation(String),
}

/// Bounded cursor over one representative frame per persisted similarity group.
pub struct GroupRepresentativeCursor<'a> {
    navigator: &'a SimilarityGroupNavigator,
    group_count: u64,
    emitted: u64,
    previous_representative: Option<FrameId>,
}

impl<'a> GroupRepresentativeCursor<'a> {
    pub fn new(navigator: &'a SimilarityGroupNavigator) -> Self {
        Self {
            navigator,
            group_count: navigator.group_count(),
            emitted: 0,
            previous_representative: None,
        }
    }

    pub fn total(&self) -> u64 {
        self.group_count
    }

    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    pub fn remaining(&self) -> u64 {
        self.group_count.saturating_sub(self.emitted)
    }

    /// Return the next group target in persisted order without collecting future groups.
    pub fn next_group(
        &mut self,
    ) -> Result<Option<GroupNavigationTarget>, GroupRepresentativeError> {
        if self.emitted >= self.group_count {
            return Ok(None);
        }

        let target = match self.previous_representative {
            None => self.navigator.group_containing(FrameId::ZERO)?,
            Some(previous) => self
                .navigator
                .adjacent_group(previous, GroupDirection::Next)?,
        };
        validate_target(&target, self.emitted, self.group_count)?;
        if let Some(previous) = self.previous_representative {
            if target.representative_frame.0 <= previous.0 {
                return Err(GroupRepresentativeError::InvalidNavigation(format!(
                    "representative {} did not follow previous representative {}",
                    target.representative_frame.0, previous.0
                )));
            }
        }

        self.previous_representative = Some(target.representative_frame);
        self.emitted = self.emitted.checked_add(1).ok_or_else(|| {
            GroupRepresentativeError::InvalidNavigation("group ordinal overflow".into())
        })?;
        Ok(Some(target))
    }

    /// Return only the persistent representative FrameId for callers that do not need group metadata.
    pub fn next_frame_id(&mut self) -> Result<Option<FrameId>, GroupRepresentativeError> {
        Ok(self.next_group()?.map(|group| group.representative_frame))
    }
}

fn validate_target(
    target: &GroupNavigationTarget,
    expected_ordinal: u64,
    expected_count: u64,
) -> Result<(), GroupRepresentativeError> {
    if target.ordinal != expected_ordinal {
        return Err(GroupRepresentativeError::InvalidNavigation(format!(
            "expected group ordinal {expected_ordinal}, got {}",
            target.ordinal
        )));
    }
    if target.group_count != expected_count {
        return Err(GroupRepresentativeError::InvalidNavigation(format!(
            "expected group count {expected_count}, got {}",
            target.group_count
        )));
    }
    if target.first_frame.0 > target.last_frame.0
        || target.representative_frame.0 < target.first_frame.0
        || target.representative_frame.0 > target.last_frame.0
    {
        return Err(GroupRepresentativeError::InvalidNavigation(format!(
            "representative {} is outside group {}..={} ",
            target.representative_frame.0, target.first_frame.0, target.last_frame.0
        )));
    }
    let expected_frames = target
        .last_frame
        .0
        .checked_sub(target.first_frame.0)
        .and_then(|span| span.checked_add(1))
        .ok_or_else(|| {
            GroupRepresentativeError::InvalidNavigation("group frame-count overflow".into())
        })?;
    if target.represented_frame_count != expected_frames {
        return Err(GroupRepresentativeError::InvalidNavigation(format!(
            "group {} reports {} frames but bounds contain {expected_frames}",
            target.ordinal, target.represented_frame_count
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{
        FrameIndex, FrameIndexEntry, FrameIndexStreamIdentity, KeyframeAnchor, OwnedRgbaFrame,
        SourceIdentity,
    };
    use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};
    use framescope_group_navigation::{
        IndexedRgbaFrame, IndexedRgbaStream, SimilaritySourceError, open_or_build_group_navigation,
    };
    use framescope_perceptual::HybridSimilarityPolicy;
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

    struct FakeStream {
        frames: VecDeque<IndexedRgbaFrame>,
    }

    impl IndexedRgbaStream for FakeStream {
        fn next_frame(&mut self) -> Result<Option<IndexedRgbaFrame>, SimilaritySourceError> {
            Ok(self.frames.pop_front())
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "framescope-extraction-groups-{label}-{}-{id}",
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

    fn solid(value: u8) -> OwnedRgbaFrame {
        let mut bytes = Vec::with_capacity(9 * 8 * 4);
        for _ in 0..(9 * 8) {
            bytes.extend_from_slice(&[value, value, value, 255]);
        }
        OwnedRgbaFrame::new(9, 8, 36, bytes).unwrap()
    }

    fn source_frame(frame_id: u64, value: u8) -> IndexedRgbaFrame {
        let entry = entry(frame_id);
        IndexedRgbaFrame {
            frame_id: entry.frame_id,
            presentation_timestamp: entry.presentation_timestamp,
            duration: entry.duration,
            keyframe: entry.keyframe,
            corrupt: entry.corrupt,
            pixels: solid(value),
        }
    }

    fn fake(values: &[u8]) -> FakeStream {
        FakeStream {
            frames: values
                .iter()
                .enumerate()
                .map(|(frame_id, value)| source_frame(frame_id as u64, *value))
                .collect(),
        }
    }

    fn complete_index(root: &Path, frame_count: u64) -> FrameIndex {
        let source = SourceIdentity::new(
            12_345,
            None,
            Some(format!(
                "group-export-test-{}",
                NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
            )),
        );
        let (mut index, _) =
            FrameIndex::open_or_create(root.join("index.sqlite3"), source, stream_identity())
                .unwrap();
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

    #[test]
    fn representatives_stream_in_group_order_without_a_video_wide_list() {
        let root = temp_root("ordered");
        let index = complete_index(&root, 5);
        let analysis = open_or_build_group_navigation(
            &index,
            root.join("similarity"),
            policy(),
            || Ok(fake(&[10, 10, 240, 240, 80])),
            || false,
        )
        .unwrap();
        assert_eq!(analysis.navigator.group_count(), 3);

        let mut cursor = GroupRepresentativeCursor::new(&analysis.navigator);
        assert_eq!(cursor.total(), 3);
        assert_eq!(cursor.remaining(), 3);

        let first = cursor.next_group().unwrap().unwrap();
        assert_eq!(first.ordinal, 0);
        assert_eq!(first.representative_frame, FrameId(0));
        assert_eq!((first.first_frame.0, first.last_frame.0), (0, 1));

        assert_eq!(cursor.next_frame_id().unwrap(), Some(FrameId(2)));
        assert_eq!(cursor.emitted(), 2);
        assert_eq!(cursor.remaining(), 1);
        assert_eq!(cursor.next_frame_id().unwrap(), Some(FrameId(4)));
        assert_eq!(cursor.next_frame_id().unwrap(), None);
        assert_eq!(cursor.emitted(), 3);
        assert_eq!(cursor.remaining(), 0);

        drop(analysis);
        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn one_group_yields_one_representative_and_then_finishes() {
        let root = temp_root("single");
        let index = complete_index(&root, 4);
        let analysis = open_or_build_group_navigation(
            &index,
            root.join("similarity"),
            policy(),
            || Ok(fake(&[42, 42, 42, 42])),
            || false,
        )
        .unwrap();
        assert_eq!(analysis.navigator.group_count(), 1);

        let mut cursor = GroupRepresentativeCursor::new(&analysis.navigator);
        assert_eq!(cursor.next_frame_id().unwrap(), Some(FrameId(0)));
        assert_eq!(cursor.next_frame_id().unwrap(), None);

        drop(analysis);
        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }
}
