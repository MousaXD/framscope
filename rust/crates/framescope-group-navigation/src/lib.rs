//! Bounded orchestration for Phase 4 similarity groups used by the frame microscope.
//!
//! This crate deliberately does not know about Android or JNI. The authoritative Phase 3 frame
//! index remains the timeline source of truth. Similarity groups are derived, disposable state:
//! a valid persisted result is reused without opening a decoder, while a missing/stale/corrupt
//! result is rebuilt by consuming one sequential owned-RGBA frame at a time.

use framescope_cache::{FrameCacheError, FrameId, FrameIndex, FrameIndexError, OwnedRgbaFrame};
use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};
use framescope_grouping_session::{GroupingSessionError, HybridGroupingSession};
use framescope_perceptual::HybridSimilarityPolicy;
use framescope_similarity_store::{
    SIMILARITY_STORE_SCHEMA_VERSION, SimilarityStore, SimilarityStoreError, SimilarityStoreKey,
    SimilarityStoreLoad,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::path::Path;
use std::time::Duration;
use thiserror::Error;

const COMPLETE_STORE_STATE: i64 = 2;
const META_ROW_ID: i64 = 1;
const SIMILARITY_SCALE: i64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedRgbaFrame {
    pub frame_id: FrameId,
    pub presentation_timestamp: Option<MediaTimestamp>,
    pub duration: Option<MediaDuration>,
    pub keyframe: bool,
    pub corrupt: bool,
    pub pixels: OwnedRgbaFrame,
}

/// Sequential source contract used by similarity analysis.
///
/// Implementations must start at the first presentation frame and return each selected video frame
/// exactly once. The analysis layer validates every returned identity/timestamp against the
/// authoritative Phase 3 index before handing pixels to the Phase 4 grouper.
pub trait IndexedRgbaStream {
    fn next_frame(&mut self) -> Result<Option<IndexedRgbaFrame>, SimilaritySourceError>;
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{code}: {message}")]
pub struct SimilaritySourceError {
    pub code: &'static str,
    pub message: String,
}

impl SimilaritySourceError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimilarityAnalysisDisposition {
    Reused,
    Built,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimilarityAnalysisSummary {
    pub group_count: u64,
    pub disposition: SimilarityAnalysisDisposition,
    pub policy: HybridSimilarityPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupDirection {
    Previous,
    Next,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupNavigationTarget {
    pub ordinal: u64,
    pub group_count: u64,
    pub representative_frame: FrameId,
    pub first_frame: FrameId,
    pub last_frame: FrameId,
    pub represented_frame_count: u64,
    pub start_timestamp: MediaTimestamp,
    pub end_timestamp: MediaTimestamp,
    pub start_duration: Option<MediaDuration>,
    pub end_duration: Option<MediaDuration>,
    /// Lowest representative-confirmation score observed in this group, on the documented
    /// 0..=10_000 similarity scale. This is not a literal percentage of changed pixels.
    pub representative_similarity_floor: u16,
    pub can_previous: bool,
    pub can_next: bool,
}

pub struct SimilarityGroupNavigator {
    connection: Connection,
    group_count: u64,
    policy: HybridSimilarityPolicy,
}

impl SimilarityGroupNavigator {
    fn open(
        path: &Path,
        group_count: u64,
        policy: HybridSimilarityPolicy,
    ) -> Result<Self, GroupNavigationError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != i64::from(SIMILARITY_STORE_SCHEMA_VERSION) {
            return Err(GroupNavigationError::InvalidPersistedNavigation(format!(
                "similarity store schema changed between validation and navigation open: {version}"
            )));
        }
        let meta: Option<(i64, i64)> = connection
            .query_row(
                "SELECT state, group_count FROM similarity_meta WHERE id = ?1",
                params![META_ROW_ID],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((state, stored_count)) = meta else {
            return Err(GroupNavigationError::InvalidPersistedNavigation(
                "similarity metadata row disappeared after validation".into(),
            ));
        };
        if state != COMPLETE_STORE_STATE
            || from_sql_u64(stored_count, "group count")? != group_count
        {
            return Err(GroupNavigationError::InvalidPersistedNavigation(
                "similarity metadata changed after validation".into(),
            ));
        }
        Ok(Self {
            connection,
            group_count,
            policy,
        })
    }

    pub fn group_count(&self) -> u64 {
        self.group_count
    }

    pub fn policy(&self) -> HybridSimilarityPolicy {
        self.policy
    }

    pub fn group_containing(
        &self,
        frame_id: FrameId,
    ) -> Result<GroupNavigationTarget, GroupNavigationError> {
        let ordinal = self.find_group_ordinal(frame_id)?;
        self.group_by_ordinal(ordinal)
    }

    pub fn adjacent_group(
        &self,
        frame_id: FrameId,
        direction: GroupDirection,
    ) -> Result<GroupNavigationTarget, GroupNavigationError> {
        let current = self.find_group_ordinal(frame_id)?;
        let target = match direction {
            GroupDirection::Previous => current
                .checked_sub(1)
                .ok_or(GroupNavigationError::GroupBoundary(direction))?,
            GroupDirection::Next => {
                let next = current
                    .checked_add(1)
                    .ok_or(GroupNavigationError::NumericRange("group ordinal"))?;
                if next >= self.group_count {
                    return Err(GroupNavigationError::GroupBoundary(direction));
                }
                next
            }
        };
        self.group_by_ordinal(target)
    }

    fn find_group_ordinal(&self, frame_id: FrameId) -> Result<u64, GroupNavigationError> {
        if self.group_count == 0 {
            return Err(GroupNavigationError::NoGroups);
        }

        let mut low = 0_u64;
        let mut high = self.group_count;
        while low < high {
            let mid = low + (high - low) / 2;
            let (first, last) = self.group_bounds(mid)?;
            if frame_id.0 < first.0 {
                high = mid;
            } else if frame_id.0 > last.0 {
                low = mid + 1;
            } else {
                return Ok(mid);
            }
        }
        Err(GroupNavigationError::FrameNotGrouped(frame_id.0))
    }

    fn group_bounds(&self, ordinal: u64) -> Result<(FrameId, FrameId), GroupNavigationError> {
        let ordinal = to_sql_u64(ordinal, "group ordinal")?;
        let row: Option<(i64, i64)> = self
            .connection
            .query_row(
                "SELECT first_frame, last_frame FROM similarity_groups WHERE ordinal = ?1",
                params![ordinal],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((first, last)) = row else {
            return Err(GroupNavigationError::InvalidPersistedNavigation(
                "similarity group ordinal is missing after validation".into(),
            ));
        };
        Ok((
            FrameId(from_sql_u64(first, "first frame")?),
            FrameId(from_sql_u64(last, "last frame")?),
        ))
    }

    fn group_by_ordinal(
        &self,
        ordinal: u64,
    ) -> Result<GroupNavigationTarget, GroupNavigationError> {
        let sql_ordinal = to_sql_u64(ordinal, "group ordinal")?;
        let raw: Option<RawGroup> = self
            .connection
            .query_row(
                "SELECT representative_frame, first_frame, last_frame, frame_count,
                        start_ticks, start_tb_num, start_tb_den,
                        end_ticks, end_tb_num, end_tb_den,
                        start_duration_ticks, start_duration_tb_num, start_duration_tb_den,
                        end_duration_ticks, end_duration_tb_num, end_duration_tb_den,
                        representative_similarity_floor
                 FROM similarity_groups WHERE ordinal = ?1",
                params![sql_ordinal],
                |row| {
                    Ok(RawGroup {
                        representative_frame: row.get(0)?,
                        first_frame: row.get(1)?,
                        last_frame: row.get(2)?,
                        frame_count: row.get(3)?,
                        start_ticks: row.get(4)?,
                        start_tb_num: row.get(5)?,
                        start_tb_den: row.get(6)?,
                        end_ticks: row.get(7)?,
                        end_tb_num: row.get(8)?,
                        end_tb_den: row.get(9)?,
                        start_duration_ticks: row.get(10)?,
                        start_duration_tb_num: row.get(11)?,
                        start_duration_tb_den: row.get(12)?,
                        end_duration_ticks: row.get(13)?,
                        end_duration_tb_num: row.get(14)?,
                        end_duration_tb_den: row.get(15)?,
                        representative_similarity_floor: row.get(16)?,
                    })
                },
            )
            .optional()?;
        let raw = raw.ok_or_else(|| {
            GroupNavigationError::InvalidPersistedNavigation(
                "similarity group ordinal is missing after validation".into(),
            )
        })?;
        decode_group(ordinal, self.group_count, raw)
    }
}

pub struct SimilarityGroupAnalysis {
    pub summary: SimilarityAnalysisSummary,
    pub navigator: SimilarityGroupNavigator,
}

/// Reuse a valid Phase 4 result or build it from a bounded sequential RGBA source.
///
/// The source factory is lazy: a valid persisted result never opens or decodes the source. During a
/// rebuild, at most the Phase 4 grouper's representative/previous frames plus one incoming frame are
/// alive. Completed group metadata is written incrementally by `SimilarityStoreWriter`.
pub fn open_or_build_group_navigation<S, F, C>(
    index: &FrameIndex,
    store_root: impl AsRef<Path>,
    policy: HybridSimilarityPolicy,
    mut source_factory: F,
    mut is_cancelled: C,
) -> Result<SimilarityGroupAnalysis, GroupNavigationError>
where
    S: IndexedRgbaStream,
    F: FnMut() -> Result<S, SimilaritySourceError>,
    C: FnMut() -> bool,
{
    if !index.source_identity().is_reuse_safe() {
        return Err(GroupNavigationError::UnsafeSourceIdentity);
    }
    let frame_count = index
        .frame_count()?
        .ok_or(GroupNavigationError::IncompleteIndex)?;
    let store = SimilarityStore::new(store_root.as_ref().to_path_buf());
    let key = SimilarityStoreKey::new_hybrid(
        index.source_identity().clone(),
        index.stream_identity().clone(),
        policy,
    )?;

    let initial = store.visit_groups_for_frame_count(&key, frame_count, |_| {})?;
    let (group_count, disposition) = match initial {
        SimilarityStoreLoad::Reused { group_count } => {
            (group_count, SimilarityAnalysisDisposition::Reused)
        }
        SimilarityStoreLoad::Missing
        | SimilarityStoreLoad::InvalidatedStale
        | SimilarityStoreLoad::InvalidatedIncomplete
        | SimilarityStoreLoad::InvalidatedCorrupt => {
            if is_cancelled() {
                return Err(GroupNavigationError::Cancelled);
            }
            let mut source = source_factory()?;
            let mut writer = store.begin(&key)?;
            let mut grouper = HybridGroupingSession::new(policy)?;

            for raw_frame_id in 0..frame_count {
                if is_cancelled() {
                    return Err(GroupNavigationError::Cancelled);
                }
                let expected = FrameId(raw_frame_id);
                let entry = index.entry(expected)?.ok_or_else(|| {
                    GroupNavigationError::TimelineMismatch(format!(
                        "authoritative frame {raw_frame_id} disappeared during analysis"
                    ))
                })?;
                let source_frame = source.next_frame()?.ok_or_else(|| {
                    GroupNavigationError::TimelineMismatch(format!(
                        "source reached EOF before indexed frame {raw_frame_id}"
                    ))
                })?;
                if is_cancelled() {
                    return Err(GroupNavigationError::Cancelled);
                }
                validate_source_frame(index, &entry, &source_frame)?;
                if let Some(group) = grouper.push(&entry, source_frame.pixels)? {
                    writer.append(&group)?;
                }
            }

            if is_cancelled() {
                return Err(GroupNavigationError::Cancelled);
            }
            if source.next_frame()?.is_some() {
                return Err(GroupNavigationError::TimelineMismatch(
                    "source emitted frames beyond the completed authoritative index".into(),
                ));
            }
            if let Some(group) = grouper.finish()? {
                writer.append(&group)?;
            }
            let written = writer.finish()?;
            let verified = store.visit_groups_for_frame_count(&key, frame_count, |_| {})?;
            let SimilarityStoreLoad::Reused { group_count } = verified else {
                return Err(GroupNavigationError::InvalidPersistedNavigation(
                    "fresh similarity result did not validate after transactional commit".into(),
                ));
            };
            if group_count != written {
                return Err(GroupNavigationError::InvalidPersistedNavigation(
                    "fresh similarity group count changed during validation".into(),
                ));
            }
            (group_count, SimilarityAnalysisDisposition::Built)
        }
    };

    let navigator = SimilarityGroupNavigator::open(&store.path_for(&key), group_count, policy)?;
    Ok(SimilarityGroupAnalysis {
        summary: SimilarityAnalysisSummary {
            group_count,
            disposition,
            policy,
        },
        navigator,
    })
}

fn validate_source_frame(
    index: &FrameIndex,
    entry: &framescope_cache::FrameIndexEntry,
    source: &IndexedRgbaFrame,
) -> Result<(), GroupNavigationError> {
    if source.frame_id != entry.frame_id
        || source.presentation_timestamp != entry.presentation_timestamp
        || source.duration != entry.duration
        || source.keyframe != entry.keyframe
        || source.corrupt != entry.corrupt
    {
        return Err(GroupNavigationError::TimelineMismatch(format!(
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
        return Err(GroupNavigationError::TimelineMismatch(format!(
            "source frame {} dimensions {}x{} do not match indexed stream dimensions",
            source.frame_id.0, source.pixels.width, source.pixels.height
        )));
    }
    Ok(())
}

#[derive(Debug)]
struct RawGroup {
    representative_frame: i64,
    first_frame: i64,
    last_frame: i64,
    frame_count: i64,
    start_ticks: i64,
    start_tb_num: i32,
    start_tb_den: i32,
    end_ticks: i64,
    end_tb_num: i32,
    end_tb_den: i32,
    start_duration_ticks: Option<i64>,
    start_duration_tb_num: Option<i32>,
    start_duration_tb_den: Option<i32>,
    end_duration_ticks: Option<i64>,
    end_duration_tb_num: Option<i32>,
    end_duration_tb_den: Option<i32>,
    representative_similarity_floor: i64,
}

fn decode_group(
    ordinal: u64,
    group_count: u64,
    raw: RawGroup,
) -> Result<GroupNavigationTarget, GroupNavigationError> {
    let start_time_base = TimeBase::new(raw.start_tb_num, raw.start_tb_den).ok_or_else(|| {
        GroupNavigationError::InvalidPersistedNavigation("invalid group start time base".into())
    })?;
    let end_time_base = TimeBase::new(raw.end_tb_num, raw.end_tb_den).ok_or_else(|| {
        GroupNavigationError::InvalidPersistedNavigation("invalid group end time base".into())
    })?;
    let start_duration = decode_duration(
        raw.start_duration_ticks,
        raw.start_duration_tb_num,
        raw.start_duration_tb_den,
        "start",
    )?;
    let end_duration = decode_duration(
        raw.end_duration_ticks,
        raw.end_duration_tb_num,
        raw.end_duration_tb_den,
        "end",
    )?;
    let similarity = u16::try_from(raw.representative_similarity_floor)
        .ok()
        .filter(|score| i64::from(*score) <= SIMILARITY_SCALE)
        .ok_or_else(|| {
            GroupNavigationError::InvalidPersistedNavigation(
                "group similarity floor is outside the documented scale".into(),
            )
        })?;
    let next = ordinal.checked_add(1);
    Ok(GroupNavigationTarget {
        ordinal,
        group_count,
        representative_frame: FrameId(from_sql_u64(
            raw.representative_frame,
            "representative frame",
        )?),
        first_frame: FrameId(from_sql_u64(raw.first_frame, "first frame")?),
        last_frame: FrameId(from_sql_u64(raw.last_frame, "last frame")?),
        represented_frame_count: from_sql_u64(raw.frame_count, "represented frame count")?,
        start_timestamp: MediaTimestamp {
            ticks: raw.start_ticks,
            time_base: start_time_base,
        },
        end_timestamp: MediaTimestamp {
            ticks: raw.end_ticks,
            time_base: end_time_base,
        },
        start_duration,
        end_duration,
        representative_similarity_floor: similarity,
        can_previous: ordinal > 0,
        can_next: next.is_some_and(|value| value < group_count),
    })
}

fn decode_duration(
    ticks: Option<i64>,
    numerator: Option<i32>,
    denominator: Option<i32>,
    label: &'static str,
) -> Result<Option<MediaDuration>, GroupNavigationError> {
    match (ticks, numerator, denominator) {
        (None, None, None) => Ok(None),
        (Some(ticks), Some(numerator), Some(denominator)) if ticks > 0 => {
            let time_base = TimeBase::new(numerator, denominator).ok_or_else(|| {
                GroupNavigationError::InvalidPersistedNavigation(format!(
                    "invalid group {label} duration time base"
                ))
            })?;
            Ok(Some(MediaDuration { ticks, time_base }))
        }
        _ => Err(GroupNavigationError::InvalidPersistedNavigation(format!(
            "incomplete group {label} duration metadata"
        ))),
    }
}

fn to_sql_u64(value: u64, label: &'static str) -> Result<i64, GroupNavigationError> {
    i64::try_from(value).map_err(|_| GroupNavigationError::NumericRange(label))
}

fn from_sql_u64(value: i64, label: &'static str) -> Result<u64, GroupNavigationError> {
    u64::try_from(value).map_err(|_| GroupNavigationError::NumericRange(label))
}

#[derive(Debug, Error)]
pub enum GroupNavigationError {
    #[error("source identity is not strong enough for reusable similarity analysis")]
    UnsafeSourceIdentity,
    #[error("frame index is not complete")]
    IncompleteIndex,
    #[error("similarity analysis was cancelled")]
    Cancelled,
    #[error(transparent)]
    Source(#[from] SimilaritySourceError),
    #[error(transparent)]
    Index(#[from] FrameIndexError),
    #[error(transparent)]
    Store(#[from] SimilarityStoreError),
    #[error(transparent)]
    Grouping(#[from] GroupingSessionError),
    #[error(transparent)]
    Frame(#[from] FrameCacheError),
    #[error("similarity source/index timeline mismatch: {0}")]
    TimelineMismatch(String),
    #[error("invalid persisted similarity navigation state: {0}")]
    InvalidPersistedNavigation(String),
    #[error("similarity navigation SQLite read failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("similarity store numeric value is outside the supported range: {0}")]
    NumericRange(&'static str),
    #[error("no similarity groups exist for this indexed source")]
    NoGroups,
    #[error("frame {0} is not covered by the validated similarity groups")]
    FrameNotGrouped(u64),
    #[error("already at the {0:?} similarity-group boundary")]
    GroupBoundary(GroupDirection),
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{
        FrameIndexEntry, FrameIndexStreamIdentity, KeyframeAnchor, SourceIdentity,
    };
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

    fn policy() -> HybridSimilarityPolicy {
        HybridSimilarityPolicy {
            max_hash_distance: 8,
            minimum_luma_similarity: 9_700,
        }
    }

    fn temp_root(label: &str) -> std::path::PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "framescope-group-navigation-{label}-{}-{id}",
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
    fn builds_groups_and_navigates_by_adjacent_representatives() {
        let root = temp_root("build");
        let index = complete_index(&root, 4, true);
        let analysis = open_or_build_group_navigation(
            &index,
            root.join("similarity"),
            policy(),
            || Ok(fake(&[10, 10, 240, 240])),
            || false,
        )
        .unwrap();

        assert_eq!(
            analysis.summary.disposition,
            SimilarityAnalysisDisposition::Built
        );
        assert_eq!(analysis.summary.group_count, 2);
        let first = analysis.navigator.group_containing(FrameId(1)).unwrap();
        assert_eq!((first.first_frame.0, first.last_frame.0), (0, 1));
        assert_eq!(first.representative_frame, FrameId(0));
        assert!(!first.can_previous);
        assert!(first.can_next);

        let second = analysis
            .navigator
            .adjacent_group(FrameId(1), GroupDirection::Next)
            .unwrap();
        assert_eq!((second.first_frame.0, second.last_frame.0), (2, 3));
        assert_eq!(second.representative_frame, FrameId(2));
        assert!(second.can_previous);
        assert!(!second.can_next);
        assert!(matches!(
            analysis
                .navigator
                .adjacent_group(FrameId(3), GroupDirection::Next),
            Err(GroupNavigationError::GroupBoundary(GroupDirection::Next))
        ));

        drop(analysis);
        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn valid_persisted_groups_are_reused_without_opening_source() {
        let root = temp_root("reuse");
        let index = complete_index(&root, 4, true);
        let first = open_or_build_group_navigation(
            &index,
            root.join("similarity"),
            policy(),
            || Ok(fake(&[10, 10, 240, 240])),
            || false,
        )
        .unwrap();
        drop(first);

        let opened = Cell::new(false);
        let second = open_or_build_group_navigation(
            &index,
            root.join("similarity"),
            policy(),
            || {
                opened.set(true);
                Ok(fake(&[10, 10, 240, 240]))
            },
            || false,
        )
        .unwrap();
        assert_eq!(
            second.summary.disposition,
            SimilarityAnalysisDisposition::Reused
        );
        assert!(!opened.get());

        drop(second);
        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn weak_source_identity_is_rejected_before_decoder_open() {
        let root = temp_root("weak");
        let index = complete_index(&root, 1, false);
        let opened = Cell::new(false);
        let result = open_or_build_group_navigation(
            &index,
            root.join("similarity"),
            policy(),
            || {
                opened.set(true);
                Ok(fake(&[10]))
            },
            || false,
        );
        assert!(matches!(
            result,
            Err(GroupNavigationError::UnsafeSourceIdentity)
        ));
        assert!(!opened.get());
        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cancellation_before_build_does_not_open_source() {
        let root = temp_root("cancel");
        let index = complete_index(&root, 1, true);
        let opened = Cell::new(false);
        let result = open_or_build_group_navigation(
            &index,
            root.join("similarity"),
            policy(),
            || {
                opened.set(true);
                Ok(fake(&[10]))
            },
            || true,
        );
        assert!(matches!(result, Err(GroupNavigationError::Cancelled)));
        assert!(!opened.get());
        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn source_timing_mismatch_is_rejected_before_group_persistence() {
        let root = temp_root("mismatch");
        let index = complete_index(&root, 2, true);
        let mut bad = fake(&[10, 10]);
        bad.frames[1].presentation_timestamp = Some(MediaTimestamp {
            ticks: 999,
            time_base: TimeBase::new(1, 1_000).unwrap(),
        });
        let mut source = Some(bad);
        let result = open_or_build_group_navigation(
            &index,
            root.join("similarity"),
            policy(),
            || Ok(source.take().unwrap()),
            || false,
        );
        assert!(matches!(
            result,
            Err(GroupNavigationError::TimelineMismatch(_))
        ));
        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn empty_complete_index_produces_empty_navigation_without_panics() {
        let root = temp_root("empty");
        let index = complete_index(&root, 0, true);
        let analysis = open_or_build_group_navigation(
            &index,
            root.join("similarity"),
            policy(),
            || Ok(fake(&[])),
            || false,
        )
        .unwrap();
        assert_eq!(analysis.summary.group_count, 0);
        assert!(matches!(
            analysis.navigator.group_containing(FrameId::ZERO),
            Err(GroupNavigationError::NoGroups)
        ));
        drop(analysis);
        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }
}
