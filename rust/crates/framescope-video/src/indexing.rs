use crate::VideoDecoder;
use framescope_cache::{
    FrameId, FrameIndex, FrameIndexEntry, FrameIndexError, FrameIndexLifecycle, FrameIndexStatus,
    FrameIndexStreamIdentity, KeyframeAnchor,
};
use framescope_core::{DecodedFrame, FrameScopeError, MediaTimestamp, StreamInfo};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use thiserror::Error;

const DEFAULT_BATCH_SIZE: usize = 256;
const PROGRESS_FRAME_INTERVAL: u64 = 64;
const RECONCILIATION_CHUNK_SIZE: u64 = 1_024;
const RESUME_OVERLAP_FRAMES: u64 = 32;
const MAX_RESUME_OVERLAP_FRAMES: u64 = 4_096;
const RESUME_SEEK_SCAN_LIMIT: u64 = 4_096;

type IndexingProgressObserver = Box<dyn FnMut(IndexingProgress)>;

thread_local! {
    static INDEXING_PROGRESS_OBSERVER: RefCell<Option<IndexingProgressObserver>> = RefCell::new(None);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexingOptions {
    batch_size: usize,
}

impl Default for IndexingOptions {
    fn default() -> Self {
        Self {
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }
}

impl IndexingOptions {
    pub fn with_batch_size(batch_size: usize) -> Result<Self, IndexingError> {
        if batch_size == 0 {
            return Err(IndexingError::InvalidConfiguration(
                "frame-index batch size must be greater than zero".into(),
            ));
        }
        Ok(Self { batch_size })
    }

    pub fn batch_size(self) -> usize {
        self.batch_size
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexingProgressStage {
    CheckingExistingIndex,
    ReusingExistingIndex,
    ValidatingExistingIndex,
    RebuildingIndex,
    Indexing,
    Finalizing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexingProgress {
    pub stage: IndexingProgressStage,
    /// Number of frames represented by the current index attempt.
    ///
    /// During validation this remains at the persisted frame count while `reused_frames` advances,
    /// so ordinary resume progress never appears to move backwards.
    pub indexed_frames: u64,
    pub reused_frames: u64,
    pub expected_reuse_frames: u64,
    pub current_timestamp_us: Option<i64>,
}

struct ProgressObserverRestore {
    previous: Option<IndexingProgressObserver>,
}

impl Drop for ProgressObserverRestore {
    fn drop(&mut self) {
        let previous = self.previous.take();
        INDEXING_PROGRESS_OBSERVER.with(|slot| {
            slot.replace(previous);
        });
    }
}

/// Run an indexing operation with a thread-scoped observer.
///
/// Microscope session opening is synchronous today. Keeping the observer thread-local lets the FFI
/// layer publish operation-scoped progress concurrently without adding callbacks to every existing
/// index API or introducing a process-global source identity. Nested observers are restored safely.
pub fn with_indexing_progress_observer<R>(
    observer: impl FnMut(IndexingProgress) + 'static,
    operation: impl FnOnce() -> R,
) -> R {
    let previous = INDEXING_PROGRESS_OBSERVER.with(|slot| slot.replace(Some(Box::new(observer))));
    let _restore = ProgressObserverRestore { previous };
    operation()
}

fn emit_progress(progress: IndexingProgress) {
    INDEXING_PROGRESS_OBSERVER.with(|slot| {
        if let Some(observer) = slot.borrow_mut().as_mut() {
            observer(progress);
        }
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexingReport {
    pub status: FrameIndexStatus,
    pub reused_existing_frames: u64,
    pub newly_indexed_frames: u64,
    pub restarted_after_partial_mismatch: bool,
    pub max_pending_entries: usize,
    /// Wall-clock time spent inside `build_or_resume_frame_index`.
    pub total_elapsed_us: u64,
    /// Time spent reading the initial persisted index status for this operation.
    pub index_status_elapsed_us: u64,
    /// Aggregate time spent opening indexing decoders across all attempts.
    pub decoder_open_elapsed_us: u64,
    /// Number of decoder opens attempted during this operation.
    pub decoder_open_count: u64,
    /// Presentation frames emitted by indexing decoders during this operation.
    pub frames_decoded: u64,
    /// Decoded frames replayed solely to validate an already-persisted prefix.
    pub validation_frames_replayed: u64,
    /// Aggregate SQLite time spent loading persisted rows for reconciliation.
    pub reconciliation_sqlite_elapsed_us: u64,
    /// Number of ordered range queries used to load persisted reconciliation rows.
    pub reconciliation_range_queries: u64,
    /// Aggregate time spent appending SQLite frame batches, including transaction commit.
    pub sqlite_batch_elapsed_us: u64,
    /// Number of non-empty SQLite frame batches committed.
    pub batch_commits: u64,
    /// Whether an Agent-1-approved timestamp checkpoint was actually sought.
    pub bounded_resume_attempted: bool,
    /// Whether bounded overlap established exact authoritative alignment through the saved boundary.
    pub bounded_resume_succeeded: bool,
    /// Whether a bounded seek attempt failed proof and the conservative replay path was used.
    pub bounded_resume_fell_back: bool,
    /// Persisted keyframe checkpoint used for bounded reconciliation, when one was attempted.
    pub resume_checkpoint_frame_id: Option<FrameId>,
    /// Frames emitted while scanning from the decoder seek landing point to the checkpoint.
    pub resume_seek_scan_frames: u64,
}

#[derive(Debug, Default)]
struct IndexingCounters {
    index_status_elapsed_us: u64,
    decoder_open_elapsed_us: u64,
    decoder_open_count: u64,
    frames_decoded: u64,
    validation_frames_replayed: u64,
    reconciliation_sqlite_elapsed_us: u64,
    reconciliation_range_queries: u64,
    sqlite_batch_elapsed_us: u64,
    batch_commits: u64,
    bounded_resume_attempted: bool,
    bounded_resume_succeeded: bool,
    bounded_resume_fell_back: bool,
    resume_checkpoint_frame_id: Option<FrameId>,
    resume_seek_scan_frames: u64,
}

impl IndexingCounters {
    fn report(
        &self,
        operation_started: Instant,
        status: FrameIndexStatus,
        reused_existing_frames: u64,
        newly_indexed_frames: u64,
        restarted_after_partial_mismatch: bool,
        max_pending_entries: usize,
    ) -> IndexingReport {
        IndexingReport {
            status,
            reused_existing_frames,
            newly_indexed_frames,
            restarted_after_partial_mismatch,
            max_pending_entries,
            total_elapsed_us: duration_us(operation_started.elapsed()),
            index_status_elapsed_us: self.index_status_elapsed_us,
            decoder_open_elapsed_us: self.decoder_open_elapsed_us,
            decoder_open_count: self.decoder_open_count,
            frames_decoded: self.frames_decoded,
            validation_frames_replayed: self.validation_frames_replayed,
            reconciliation_sqlite_elapsed_us: self.reconciliation_sqlite_elapsed_us,
            reconciliation_range_queries: self.reconciliation_range_queries,
            sqlite_batch_elapsed_us: self.sqlite_batch_elapsed_us,
            batch_commits: self.batch_commits,
            bounded_resume_attempted: self.bounded_resume_attempted,
            bounded_resume_succeeded: self.bounded_resume_succeeded,
            bounded_resume_fell_back: self.bounded_resume_fell_back,
            resume_checkpoint_frame_id: self.resume_checkpoint_frame_id,
            resume_seek_scan_frames: self.resume_seek_scan_frames,
        }
    }
}

#[derive(Debug, Error)]
pub enum IndexingError {
    #[error("video decoding failed while building frame index: {0}")]
    Decoder(#[from] FrameScopeError),
    #[error("frame-index persistence failed: {0}")]
    Storage(#[from] FrameIndexError),
    #[error("frame-index configuration is invalid: {0}")]
    InvalidConfiguration(String),
    #[error("fresh decoder stream does not match the stream bound to the persistent index")]
    StreamIdentityMismatch,
    #[error("partial index could not be reconciled with a fresh decode")]
    PartialIndexMismatch,
}

/// Minimal decoder contract used by the resumable indexer.
///
/// Production uses [`VideoDecoder`]. The trait keeps persistence/recovery tests independent from
/// FFmpeg and avoids test-only global hooks. Timestamp seeking is used only when the persisted
/// Agent-1 seek-safety contract explicitly permits it; otherwise the indexer never calls this hook.
pub trait FrameIndexDecoder {
    fn selected_stream_for_index(&self) -> &StreamInfo;
    fn next_frame_for_index(&mut self) -> Result<Option<DecodedFrame>, FrameScopeError>;
    fn seek_for_index_resume(&mut self, timestamp_us: i64) -> Result<(), FrameScopeError>;
}

impl FrameIndexDecoder for VideoDecoder {
    fn selected_stream_for_index(&self) -> &StreamInfo {
        self.selected_stream()
    }

    fn next_frame_for_index(&mut self) -> Result<Option<DecodedFrame>, FrameScopeError> {
        self.next_frame()
    }

    fn seek_for_index_resume(&mut self, timestamp_us: i64) -> Result<(), FrameScopeError> {
        self.seek_to_timestamp_us(timestamp_us)
    }
}

struct BoundedResumeStart<D> {
    decoder: D,
    checkpoint_frame_id: FrameId,
    checkpoint_frame: DecodedFrame,
}

/// Build or safely resume a persistent frame index using a fresh decoder factory.
///
/// Persisted rows are consumed through bounded ordered range queries instead of one primary-key
/// query per replayed frame. When Agent 1's persisted seek-safety decision is unambiguous, a partial
/// index may resume from a trusted earlier keyframe, decode a bounded overlap, and establish exact
/// row-by-row alignment before appending. Any ambiguity, seek failure, overlap mismatch, excessive
/// GOP/scan distance, or missing proof falls back to the conservative stream-start replay. FrameId
/// alignment is never inferred from nominal FPS or from the decoder's epoch-local frame number.
pub fn build_or_resume_frame_index<D, F>(
    index: &mut FrameIndex,
    mut open_fresh_decoder: F,
    options: IndexingOptions,
) -> Result<IndexingReport, IndexingError>
where
    D: FrameIndexDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
{
    if options.batch_size == 0 {
        return Err(IndexingError::InvalidConfiguration(
            "frame-index batch size must be greater than zero".into(),
        ));
    }

    let operation_started = Instant::now();
    let mut counters = IndexingCounters::default();
    let status_started = Instant::now();
    let initial_status = index.status()?;
    counters.index_status_elapsed_us = duration_us(status_started.elapsed());
    emit_progress(IndexingProgress {
        stage: IndexingProgressStage::CheckingExistingIndex,
        indexed_frames: initial_status.indexed_frames,
        reused_frames: 0,
        expected_reuse_frames: initial_status.indexed_frames,
        current_timestamp_us: initial_status
            .last_presentation_timestamp
            .and_then(|timestamp| timestamp.to_microseconds()),
    });
    if initial_status.lifecycle == FrameIndexLifecycle::Complete {
        emit_progress(IndexingProgress {
            stage: IndexingProgressStage::ReusingExistingIndex,
            indexed_frames: initial_status.indexed_frames,
            reused_frames: initial_status.indexed_frames,
            expected_reuse_frames: initial_status.indexed_frames,
            current_timestamp_us: initial_status
                .last_presentation_timestamp
                .and_then(|timestamp| timestamp.to_microseconds()),
        });
        emit_progress(IndexingProgress {
            stage: IndexingProgressStage::Finalizing,
            indexed_frames: initial_status.indexed_frames,
            reused_frames: initial_status.indexed_frames,
            expected_reuse_frames: initial_status.indexed_frames,
            current_timestamp_us: initial_status
                .last_presentation_timestamp
                .and_then(|timestamp| timestamp.to_microseconds()),
        });
        return Ok(counters.report(operation_started, initial_status, 0, 0, false, 0));
    }

    let mut expected_existing = initial_status.indexed_frames;
    let mut restarted = false;
    let mut max_pending = 0_usize;
    let mut bounded_attempt_consumed = false;

    loop {
        let bounded_start = if expected_existing > 0 && !bounded_attempt_consumed {
            bounded_attempt_consumed = true;
            try_open_bounded_resume(
                index,
                &mut open_fresh_decoder,
                expected_existing,
                &mut counters,
            )?
        } else {
            None
        };
        let bounded_active = bounded_start.is_some();

        let (mut decoder, mut frame_id, mut reused, mut pending_decoded) = match bounded_start {
            Some(start) => (
                start.decoder,
                start.checkpoint_frame_id,
                start.checkpoint_frame_id.0,
                Some(start.checkpoint_frame),
            ),
            None => (
                open_checked_decoder(index, &mut open_fresh_decoder, &mut counters)?,
                FrameId::ZERO,
                0,
                None,
            ),
        };

        index.mark_building()?;
        let mut anchor = KeyframeAnchor::StreamStart;
        let mut batch = Vec::with_capacity(options.batch_size);
        let mut persisted_chunk = VecDeque::new();
        let mut added = 0_u64;
        let mut last_timestamp_us = None;

        if expected_existing > 0 {
            emit_progress(IndexingProgress {
                stage: IndexingProgressStage::ValidatingExistingIndex,
                indexed_frames: expected_existing,
                reused_frames: reused,
                expected_reuse_frames: expected_existing,
                current_timestamp_us: None,
            });
        } else {
            emit_progress(IndexingProgress {
                stage: IndexingProgressStage::Indexing,
                indexed_frames: 0,
                reused_frames: 0,
                expected_reuse_frames: 0,
                current_timestamp_us: None,
            });
        }

        loop {
            let decoded = if let Some(decoded) = pending_decoded.take() {
                Some(decoded)
            } else {
                match decoder.next_frame_for_index() {
                    Ok(value) => value,
                    Err(FrameScopeError::Cancelled) => {
                        flush_batch(index, &mut batch, &mut added, &mut counters)?;
                        let _ = index.mark_incomplete(Some("cancelled"));
                        return Err(IndexingError::Decoder(FrameScopeError::Cancelled));
                    }
                    Err(error) => {
                        if let Err(storage) =
                            flush_batch(index, &mut batch, &mut added, &mut counters)
                        {
                            let _ = index.mark_failed_recoverable(&storage.to_string());
                            return Err(storage);
                        }
                        let _ = index.mark_failed_recoverable(&error.to_string());
                        return Err(IndexingError::Decoder(error));
                    }
                }
            };

            let Some(decoded) = decoded else {
                if frame_id.0 < expected_existing {
                    break;
                }
                if bounded_active && reused == expected_existing {
                    counters.bounded_resume_succeeded = true;
                }
                flush_batch(index, &mut batch, &mut added, &mut counters)?;
                emit_progress(IndexingProgress {
                    stage: IndexingProgressStage::Finalizing,
                    indexed_frames: reused.saturating_add(added),
                    reused_frames: reused,
                    expected_reuse_frames: expected_existing,
                    current_timestamp_us: last_timestamp_us,
                });
                index.mark_complete()?;
                let status = index.status()?;
                return Ok(counters.report(
                    operation_started,
                    status,
                    reused,
                    added,
                    restarted,
                    max_pending,
                ));
            };

            counters.frames_decoded = counters.frames_decoded.saturating_add(1);
            last_timestamp_us = decoded.timestamp_us();
            let entry = entry_from_decoded(frame_id, &decoded, &mut anchor);
            entry.validate(index.stream_identity())?;

            if frame_id.0 < expected_existing {
                counters.validation_frames_replayed =
                    counters.validation_frames_replayed.saturating_add(1);
                if persisted_chunk.is_empty() {
                    load_reconciliation_chunk(
                        index,
                        frame_id,
                        expected_existing,
                        &mut persisted_chunk,
                        &mut counters,
                    )?;
                }
                match persisted_chunk.pop_front() {
                    Some(existing) if existing.frame_id == frame_id && existing == entry => {
                        reused = reused.saturating_add(1);
                    }
                    _ => break,
                }
            } else {
                if bounded_active && reused == expected_existing {
                    counters.bounded_resume_succeeded = true;
                }
                batch.push(entry);
                max_pending = max_pending.max(batch.len());
                if batch.len() >= options.batch_size {
                    flush_batch(index, &mut batch, &mut added, &mut counters)?;
                }
            }

            let processed = frame_id.0.saturating_add(1);
            let stage = if processed <= expected_existing {
                IndexingProgressStage::ValidatingExistingIndex
            } else {
                IndexingProgressStage::Indexing
            };
            if processed == 1
                || processed == expected_existing
                || processed == expected_existing.saturating_add(1)
                || processed % PROGRESS_FRAME_INTERVAL == 0
            {
                emit_progress(IndexingProgress {
                    stage,
                    indexed_frames: if stage == IndexingProgressStage::ValidatingExistingIndex {
                        expected_existing
                    } else {
                        reused
                            .saturating_add(added)
                            .saturating_add(batch.len() as u64)
                    },
                    reused_frames: reused,
                    expected_reuse_frames: expected_existing,
                    current_timestamp_us: last_timestamp_us,
                });
            }

            frame_id = FrameId(
                frame_id
                    .0
                    .checked_add(1)
                    .ok_or_else(|| FrameIndexError::InvalidState("frame id overflow".into()))?,
            );
        }

        if bounded_active {
            counters.bounded_resume_succeeded = false;
            counters.bounded_resume_fell_back = true;
            continue;
        }

        if expected_existing > 0 && !restarted {
            emit_progress(IndexingProgress {
                stage: IndexingProgressStage::RebuildingIndex,
                indexed_frames: 0,
                reused_frames: 0,
                expected_reuse_frames: 0,
                current_timestamp_us: None,
            });
            index.clear_for_rebuild()?;
            expected_existing = 0;
            restarted = true;
            continue;
        }

        let _ = index.mark_failed_recoverable("partial index reconciliation failed");
        return Err(IndexingError::PartialIndexMismatch);
    }
}

fn try_open_bounded_resume<D, F>(
    index: &mut FrameIndex,
    open_fresh_decoder: &mut F,
    expected_existing: u64,
    counters: &mut IndexingCounters,
) -> Result<Option<BoundedResumeStart<D>>, IndexingError>
where
    D: FrameIndexDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
{
    let Some((checkpoint_frame_id, checkpoint_timestamp)) =
        select_resume_checkpoint(index, expected_existing)?
    else {
        return Ok(None);
    };
    let Some(checkpoint_entry) = index.entry(checkpoint_frame_id)? else {
        return Ok(None);
    };
    if !checkpoint_entry.keyframe
        || checkpoint_entry.corrupt
        || checkpoint_entry.presentation_timestamp != Some(checkpoint_timestamp)
    {
        return Ok(None);
    }
    let Some(timestamp_us) = checkpoint_timestamp
        .to_microseconds()
        .filter(|value| *value >= 0)
    else {
        return Ok(None);
    };

    counters.bounded_resume_attempted = true;
    counters.resume_checkpoint_frame_id = Some(checkpoint_frame_id);
    let mut decoder = open_checked_decoder(index, open_fresh_decoder, counters)?;
    match decoder.seek_for_index_resume(timestamp_us) {
        Ok(()) => {}
        Err(FrameScopeError::Cancelled) => {
            let _ = index.mark_incomplete(Some("cancelled"));
            return Err(IndexingError::Decoder(FrameScopeError::Cancelled));
        }
        Err(_) => {
            counters.bounded_resume_fell_back = true;
            return Ok(None);
        }
    }

    for _ in 0..RESUME_SEEK_SCAN_LIMIT {
        let candidate = match decoder.next_frame_for_index() {
            Ok(Some(candidate)) => candidate,
            Ok(None) => {
                counters.bounded_resume_fell_back = true;
                return Ok(None);
            }
            Err(FrameScopeError::Cancelled) => {
                let _ = index.mark_incomplete(Some("cancelled"));
                return Err(IndexingError::Decoder(FrameScopeError::Cancelled));
            }
            Err(_) => {
                counters.bounded_resume_fell_back = true;
                return Ok(None);
            }
        };
        counters.resume_seek_scan_frames = counters.resume_seek_scan_frames.saturating_add(1);
        if matches_index_entry(&candidate, &checkpoint_entry) {
            return Ok(Some(BoundedResumeStart {
                decoder,
                checkpoint_frame_id,
                checkpoint_frame: candidate,
            }));
        }
        counters.frames_decoded = counters.frames_decoded.saturating_add(1);
    }

    counters.bounded_resume_fell_back = true;
    Ok(None)
}

fn select_resume_checkpoint(
    index: &FrameIndex,
    expected_existing: u64,
) -> Result<Option<(FrameId, MediaTimestamp)>, IndexingError> {
    if expected_existing == 0 || !index.timestamp_seek_safety()?.permits_timestamp_seek() {
        return Ok(None);
    }
    let probe_frame_id = FrameId(expected_existing.saturating_sub(RESUME_OVERLAP_FRAMES));
    let Some(probe) = index.entry(probe_frame_id)? else {
        return Ok(None);
    };
    let KeyframeAnchor::Keyframe {
        frame_id: checkpoint_frame_id,
        presentation_timestamp: Some(checkpoint_timestamp),
    } = probe.anchor
    else {
        return Ok(None);
    };
    if checkpoint_frame_id == FrameId::ZERO || checkpoint_frame_id.0 >= expected_existing {
        return Ok(None);
    }
    let overlap = expected_existing.saturating_sub(checkpoint_frame_id.0);
    if overlap == 0 || overlap > MAX_RESUME_OVERLAP_FRAMES {
        return Ok(None);
    }
    if checkpoint_timestamp
        .to_microseconds()
        .is_none_or(|timestamp_us| timestamp_us < 0)
    {
        return Ok(None);
    }
    Ok(Some((checkpoint_frame_id, checkpoint_timestamp)))
}

fn open_checked_decoder<D, F>(
    index: &mut FrameIndex,
    open_fresh_decoder: &mut F,
    counters: &mut IndexingCounters,
) -> Result<D, IndexingError>
where
    D: FrameIndexDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
{
    counters.decoder_open_count = counters.decoder_open_count.saturating_add(1);
    let decoder_open_started = Instant::now();
    let opened_decoder = open_fresh_decoder();
    counters.decoder_open_elapsed_us = counters
        .decoder_open_elapsed_us
        .saturating_add(duration_us(decoder_open_started.elapsed()));
    let decoder = match opened_decoder {
        Ok(decoder) => decoder,
        Err(error) => {
            let _ = index.mark_failed_recoverable(&error.to_string());
            return Err(IndexingError::Decoder(error));
        }
    };
    let actual_stream = FrameIndexStreamIdentity::from_stream(decoder.selected_stream_for_index())?;
    if &actual_stream != index.stream_identity() {
        let _ = index.mark_failed_recoverable("selected stream identity changed");
        return Err(IndexingError::StreamIdentityMismatch);
    }
    Ok(decoder)
}

fn load_reconciliation_chunk(
    index: &FrameIndex,
    start: FrameId,
    expected_existing: u64,
    destination: &mut VecDeque<FrameIndexEntry>,
    counters: &mut IndexingCounters,
) -> Result<(), IndexingError> {
    debug_assert!(destination.is_empty());
    let remaining = expected_existing.saturating_sub(start.0);
    let chunk_len = remaining.min(RECONCILIATION_CHUNK_SIZE);
    let end = start
        .0
        .checked_add(chunk_len)
        .ok_or_else(|| FrameIndexError::InvalidState("frame id overflow".into()))?;
    let query_started = Instant::now();
    let result = index.visit_range(start, FrameId(end), |entry| {
        destination.push_back(entry);
        Ok(())
    });
    counters.reconciliation_sqlite_elapsed_us = counters
        .reconciliation_sqlite_elapsed_us
        .saturating_add(duration_us(query_started.elapsed()));
    counters.reconciliation_range_queries = counters.reconciliation_range_queries.saturating_add(1);
    result?;
    Ok(())
}

fn flush_batch(
    index: &mut FrameIndex,
    batch: &mut Vec<FrameIndexEntry>,
    added: &mut u64,
    counters: &mut IndexingCounters,
) -> Result<(), IndexingError> {
    if batch.is_empty() {
        return Ok(());
    }
    let count = batch.len() as u64;
    let batch_started = Instant::now();
    let result = index.append_batch(batch);
    counters.sqlite_batch_elapsed_us = counters
        .sqlite_batch_elapsed_us
        .saturating_add(duration_us(batch_started.elapsed()));
    counters.batch_commits = counters.batch_commits.saturating_add(1);
    if let Err(error) = result {
        let _ = index.mark_failed_recoverable(&error.to_string());
        return Err(IndexingError::Storage(error));
    }
    batch.clear();
    *added = added.saturating_add(count);
    Ok(())
}

fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn matches_index_entry(decoded: &DecodedFrame, indexed: &FrameIndexEntry) -> bool {
    decoded.presentation_timestamp == indexed.presentation_timestamp
        && decoded.duration == indexed.duration
        && decoded.keyframe == indexed.keyframe
        && decoded.corrupt == indexed.corrupt
}

fn entry_from_decoded(
    frame_id: FrameId,
    decoded: &DecodedFrame,
    anchor: &mut KeyframeAnchor,
) -> FrameIndexEntry {
    if decoded.keyframe && !decoded.corrupt {
        *anchor = KeyframeAnchor::Keyframe {
            frame_id,
            presentation_timestamp: decoded.presentation_timestamp,
        };
    }
    FrameIndexEntry {
        frame_id,
        presentation_timestamp: decoded.presentation_timestamp,
        duration: decoded.duration,
        keyframe: decoded.keyframe,
        corrupt: decoded.corrupt,
        anchor: anchor.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{FrameIndexOpenDisposition, SourceIdentity, TimestampSeekSafety};
    use framescope_core::{CodecInfo, MediaDuration, MediaKind, TimeBase};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    struct FakeDecoder {
        stream: StreamInfo,
        frames: VecDeque<Result<DecodedFrame, FrameScopeError>>,
        seek_lands_at_frame: u64,
    }

    impl FrameIndexDecoder for FakeDecoder {
        fn selected_stream_for_index(&self) -> &StreamInfo {
            &self.stream
        }

        fn next_frame_for_index(&mut self) -> Result<Option<DecodedFrame>, FrameScopeError> {
            match self.frames.pop_front() {
                Some(Ok(frame)) => Ok(Some(frame)),
                Some(Err(error)) => Err(error),
                None => Ok(None),
            }
        }

        fn seek_for_index_resume(&mut self, _timestamp_us: i64) -> Result<(), FrameScopeError> {
            loop {
                let should_drop = match self.frames.front() {
                    Some(Ok(frame)) => frame.index < self.seek_lands_at_frame,
                    _ => false,
                };
                if !should_drop {
                    break;
                }
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

    fn frame(index: u64, ticks: i64, keyframe: bool) -> DecodedFrame {
        let time_base = TimeBase::new(1, 1_000).unwrap();
        DecodedFrame {
            source_id: 99,
            stream_index: 0,
            decode_epoch: 0,
            index,
            presentation_timestamp: Some(MediaTimestamp { ticks, time_base }),
            duration: Some(MediaDuration {
                ticks: 40,
                time_base,
            }),
            keyframe,
            corrupt: false,
            width: 64,
            height: 48,
            pixel_format: Some("yuv420p".into()),
        }
    }

    fn decoder(sequence: &[(i64, bool)]) -> FakeDecoder {
        decoder_with_seek(sequence, 0)
    }

    fn decoder_with_seek(sequence: &[(i64, bool)], seek_lands_at_frame: u64) -> FakeDecoder {
        FakeDecoder {
            stream: stream(),
            frames: sequence
                .iter()
                .enumerate()
                .map(|(index, (ticks, keyframe))| Ok(frame(index as u64, *ticks, *keyframe)))
                .collect(),
            seek_lands_at_frame,
        }
    }

    fn temp_db(name: &str) -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "framescope-video-index-{}-{name}-{id}.sqlite",
            std::process::id()
        ))
    }

    fn open_index(path: &Path) -> FrameIndex {
        let source = SourceIdentity::new(1_000, Some(5), Some("fixture".into()));
        let identity = FrameIndexStreamIdentity::from_stream(&stream()).unwrap();
        let (index, disposition) = FrameIndex::open_or_create(path, source, identity).unwrap();
        assert!(matches!(
            disposition,
            FrameIndexOpenDisposition::Created | FrameIndexOpenDisposition::Reused
        ));
        index
    }

    #[test]
    fn vfr_pts_are_persisted_exactly_and_batches_stay_bounded() {
        let path = temp_db("vfr");
        let mut index = open_index(&path);
        let sequence = [
            (0, true),
            (40, false),
            (100, false),
            (140, false),
            (260, true),
        ];
        let report = build_or_resume_frame_index(
            &mut index,
            || Ok(decoder(&sequence)),
            IndexingOptions::with_batch_size(2).unwrap(),
        )
        .unwrap();
        assert_eq!(report.status.lifecycle, FrameIndexLifecycle::Complete);
        assert_eq!(report.status.frame_count, Some(5));
        assert!(report.max_pending_entries <= 2);
        assert_eq!(report.frames_decoded, 5);
        assert_eq!(report.validation_frames_replayed, 0);
        assert_eq!(report.reconciliation_range_queries, 0);
        assert_eq!(report.batch_commits, 3);
        assert_eq!(report.decoder_open_count, 1);
        let ticks = (0..5)
            .map(|id| {
                index
                    .entry(FrameId(id))
                    .unwrap()
                    .unwrap()
                    .presentation_timestamp
                    .unwrap()
                    .ticks
            })
            .collect::<Vec<_>>();
        assert_eq!(ticks, vec![0, 40, 100, 140, 260]);
    }

    #[test]
    fn cancellation_leaves_incomplete_and_resumes_by_reconciliation() {
        let path = temp_db("cancel-resume");
        let mut index = open_index(&path);
        let first = build_or_resume_frame_index(
            &mut index,
            || {
                Ok(FakeDecoder {
                    stream: stream(),
                    frames: VecDeque::from(vec![
                        Ok(frame(0, 0, true)),
                        Ok(frame(1, 40, false)),
                        Ok(frame(2, 100, false)),
                        Err(FrameScopeError::Cancelled),
                    ]),
                    seek_lands_at_frame: 0,
                })
            },
            IndexingOptions::with_batch_size(2).unwrap(),
        );
        assert!(matches!(
            first,
            Err(IndexingError::Decoder(FrameScopeError::Cancelled))
        ));
        assert_eq!(
            index.status().unwrap().lifecycle,
            FrameIndexLifecycle::Incomplete
        );
        assert_eq!(index.status().unwrap().indexed_frames, 3);

        let full = [
            (0, true),
            (40, false),
            (100, false),
            (180, false),
            (260, true),
        ];
        let report = build_or_resume_frame_index(
            &mut index,
            || Ok(decoder(&full)),
            IndexingOptions::with_batch_size(2).unwrap(),
        )
        .unwrap();
        assert_eq!(report.reused_existing_frames, 3);
        assert_eq!(report.newly_indexed_frames, 2);
        assert_eq!(report.status.frame_count, Some(5));
        assert_eq!(report.validation_frames_replayed, 3);
        assert_eq!(report.reconciliation_range_queries, 1);
        assert!(!report.bounded_resume_attempted);
    }

    #[test]
    fn reconciliation_reads_persisted_prefix_in_bounded_ordered_chunks() {
        let path = temp_db("chunked-reconciliation");
        let mut index = open_index(&path);
        let persisted = RECONCILIATION_CHUNK_SIZE * 2 + 2;
        let first = build_or_resume_frame_index(
            &mut index,
            || {
                let mut frames = (0..persisted)
                    .map(|id| Ok(frame(id, i64::try_from(id * 40).unwrap(), id == 0)))
                    .collect::<Vec<_>>();
                frames.push(Err(FrameScopeError::Cancelled));
                Ok(FakeDecoder {
                    stream: stream(),
                    frames: VecDeque::from(frames),
                    seek_lands_at_frame: 0,
                })
            },
            IndexingOptions::with_batch_size(128).unwrap(),
        );
        assert!(matches!(
            first,
            Err(IndexingError::Decoder(FrameScopeError::Cancelled))
        ));
        assert_eq!(index.status().unwrap().indexed_frames, persisted);

        let total = persisted + 5;
        let full = (0..total)
            .map(|id| (i64::try_from(id * 40).unwrap(), id == 0))
            .collect::<Vec<_>>();
        let report = build_or_resume_frame_index(
            &mut index,
            || Ok(decoder(&full)),
            IndexingOptions::with_batch_size(128).unwrap(),
        )
        .unwrap();
        assert_eq!(report.validation_frames_replayed, persisted);
        assert_eq!(report.reconciliation_range_queries, 3);
        assert!(report.reconciliation_range_queries < persisted);
        assert_eq!(report.status.frame_count, Some(total));
        assert!(!report.bounded_resume_attempted);
    }

    #[test]
    fn complete_index_reopen_performs_zero_decoder_replay() {
        let path = temp_db("complete-reopen");
        let mut index = open_index(&path);
        let sequence = [(0, true), (40, false), (80, false)];
        build_or_resume_frame_index(
            &mut index,
            || Ok(decoder(&sequence)),
            IndexingOptions::default(),
        )
        .unwrap();

        let opens = Rc::new(RefCell::new(0_u64));
        let captured = Rc::clone(&opens);
        let report = build_or_resume_frame_index(
            &mut index,
            move || {
                *captured.borrow_mut() += 1;
                Ok(decoder(&sequence))
            },
            IndexingOptions::default(),
        )
        .unwrap();
        assert_eq!(*opens.borrow(), 0);
        assert_eq!(report.frames_decoded, 0);
        assert_eq!(report.validation_frames_replayed, 0);
        assert_eq!(report.decoder_open_elapsed_us, 0);
        assert_eq!(report.decoder_open_count, 0);
    }

    #[test]
    fn eighty_percent_partial_resume_is_row_identical_without_replaying_frame_zero() {
        let sequence = (0..100_u64)
            .map(|id| (i64::try_from(id * 37).unwrap(), id % 17 == 0))
            .collect::<Vec<_>>();

        let resumed_path = temp_db("row-identical-resumed");
        let mut resumed = open_index(&resumed_path);
        let partial_sequence = sequence[..80].to_vec();
        let first = build_or_resume_frame_index(
            &mut resumed,
            || {
                let mut frames = partial_sequence
                    .iter()
                    .enumerate()
                    .map(|(id, (ticks, keyframe))| Ok(frame(id as u64, *ticks, *keyframe)))
                    .collect::<Vec<_>>();
                frames.push(Err(FrameScopeError::Cancelled));
                Ok(FakeDecoder {
                    stream: stream(),
                    frames: VecDeque::from(frames),
                    seek_lands_at_frame: 0,
                })
            },
            IndexingOptions::with_batch_size(13).unwrap(),
        );
        assert!(matches!(
            first,
            Err(IndexingError::Decoder(FrameScopeError::Cancelled))
        ));
        let resumed_report = build_or_resume_frame_index(
            &mut resumed,
            || Ok(decoder_with_seek(&sequence, 34)),
            IndexingOptions::with_batch_size(13).unwrap(),
        )
        .unwrap();
        assert_eq!(resumed_report.reused_existing_frames, 80);
        assert!(resumed_report.bounded_resume_attempted);
        assert!(resumed_report.bounded_resume_succeeded);
        assert!(!resumed_report.bounded_resume_fell_back);
        assert_eq!(resumed_report.resume_checkpoint_frame_id, Some(FrameId(34)));
        assert!(resumed_report.validation_frames_replayed < 80);
        assert!(resumed_report.frames_decoded < 100);

        let uninterrupted_path = temp_db("row-identical-uninterrupted");
        let mut uninterrupted = open_index(&uninterrupted_path);
        build_or_resume_frame_index(
            &mut uninterrupted,
            || Ok(decoder(&sequence)),
            IndexingOptions::with_batch_size(13).unwrap(),
        )
        .unwrap();

        for id in 0..100_u64 {
            assert_eq!(
                resumed.entry(FrameId(id)).unwrap(),
                uninterrupted.entry(FrameId(id)).unwrap()
            );
        }
    }

    #[test]
    fn ambiguous_timestamp_seek_safety_keeps_conservative_full_replay() {
        let path = temp_db("ambiguous-resume");
        let mut index = open_index(&path);
        let sequence = (0..100_u64)
            .map(|id| {
                let keyframe = matches!(id, 0 | 30 | 60 | 90);
                let ticks = if id == 60 {
                    1_200
                } else {
                    i64::try_from(id * 40).unwrap()
                };
                (ticks, keyframe)
            })
            .collect::<Vec<_>>();
        let partial_sequence = sequence[..80].to_vec();
        let first = build_or_resume_frame_index(
            &mut index,
            || {
                let mut frames = partial_sequence
                    .iter()
                    .enumerate()
                    .map(|(id, (ticks, keyframe))| Ok(frame(id as u64, *ticks, *keyframe)))
                    .collect::<Vec<_>>();
                frames.push(Err(FrameScopeError::Cancelled));
                Ok(FakeDecoder {
                    stream: stream(),
                    frames: VecDeque::from(frames),
                    seek_lands_at_frame: 0,
                })
            },
            IndexingOptions::with_batch_size(11).unwrap(),
        );
        assert!(matches!(
            first,
            Err(IndexingError::Decoder(FrameScopeError::Cancelled))
        ));
        assert_eq!(
            index.timestamp_seek_safety().unwrap(),
            TimestampSeekSafety::Ambiguous
        );

        let report = build_or_resume_frame_index(
            &mut index,
            || Ok(decoder_with_seek(&sequence, 60)),
            IndexingOptions::with_batch_size(11).unwrap(),
        )
        .unwrap();
        assert!(!report.bounded_resume_attempted);
        assert!(!report.bounded_resume_succeeded);
        assert_eq!(report.validation_frames_replayed, 80);
        assert_eq!(report.reused_existing_frames, 80);
    }

    #[test]
    fn bounded_overlap_mismatch_falls_back_to_conservative_rebuild() {
        let path = temp_db("bounded-mismatch");
        let mut index = open_index(&path);
        let original = (0..100_u64)
            .map(|id| (i64::try_from(id * 37).unwrap(), id % 17 == 0))
            .collect::<Vec<_>>();
        let partial_sequence = original[..80].to_vec();
        let first = build_or_resume_frame_index(
            &mut index,
            || {
                let mut frames = partial_sequence
                    .iter()
                    .enumerate()
                    .map(|(id, (ticks, keyframe))| Ok(frame(id as u64, *ticks, *keyframe)))
                    .collect::<Vec<_>>();
                frames.push(Err(FrameScopeError::Cancelled));
                Ok(FakeDecoder {
                    stream: stream(),
                    frames: VecDeque::from(frames),
                    seek_lands_at_frame: 0,
                })
            },
            IndexingOptions::with_batch_size(13).unwrap(),
        );
        assert!(matches!(
            first,
            Err(IndexingError::Decoder(FrameScopeError::Cancelled))
        ));

        let mut changed = original.clone();
        changed[60].0 = changed[60].0.saturating_add(1);
        let report = build_or_resume_frame_index(
            &mut index,
            || Ok(decoder_with_seek(&changed, 34)),
            IndexingOptions::with_batch_size(13).unwrap(),
        )
        .unwrap();
        assert!(report.bounded_resume_attempted);
        assert!(report.bounded_resume_fell_back);
        assert!(!report.bounded_resume_succeeded);
        assert!(report.restarted_after_partial_mismatch);
        assert_eq!(report.status.frame_count, Some(100));
        assert_eq!(
            index
                .entry(FrameId(60))
                .unwrap()
                .unwrap()
                .presentation_timestamp
                .unwrap()
                .ticks,
            changed[60].0
        );
    }

    #[test]
    fn mismatched_partial_timeline_is_discarded_and_rebuilt() {
        let path = temp_db("mismatch");
        let mut index = open_index(&path);
        let _ = build_or_resume_frame_index(
            &mut index,
            || {
                Ok(FakeDecoder {
                    stream: stream(),
                    frames: VecDeque::from(vec![
                        Ok(frame(0, 0, true)),
                        Ok(frame(1, 40, false)),
                        Err(FrameScopeError::Cancelled),
                    ]),
                    seek_lands_at_frame: 0,
                })
            },
            IndexingOptions::with_batch_size(8).unwrap(),
        );
        let changed = [(0, true), (50, false), (100, false)];
        let report = build_or_resume_frame_index(
            &mut index,
            || Ok(decoder(&changed)),
            IndexingOptions::with_batch_size(2).unwrap(),
        )
        .unwrap();
        assert!(report.restarted_after_partial_mismatch);
        assert_eq!(report.status.frame_count, Some(3));
        assert_eq!(
            index
                .entry(FrameId(1))
                .unwrap()
                .unwrap()
                .presentation_timestamp
                .unwrap()
                .ticks,
            50
        );
    }

    #[test]
    fn progress_observer_reports_resume_and_exact_vfr_timestamps() {
        let path = temp_db("progress-resume");
        let mut index = open_index(&path);
        let _ = build_or_resume_frame_index(
            &mut index,
            || {
                Ok(FakeDecoder {
                    stream: stream(),
                    frames: VecDeque::from(vec![
                        Ok(frame(0, 0, true)),
                        Ok(frame(1, 40, false)),
                        Ok(frame(2, 100, false)),
                        Err(FrameScopeError::Cancelled),
                    ]),
                    seek_lands_at_frame: 0,
                })
            },
            IndexingOptions::with_batch_size(2).unwrap(),
        );

        let progress = Rc::new(RefCell::new(Vec::new()));
        let captured = Rc::clone(&progress);
        let full = [
            (0, true),
            (40, false),
            (100, false),
            (180, false),
            (260, true),
        ];
        with_indexing_progress_observer(
            move |event| captured.borrow_mut().push(event),
            || {
                build_or_resume_frame_index(
                    &mut index,
                    || Ok(decoder(&full)),
                    IndexingOptions::with_batch_size(2).unwrap(),
                )
                .unwrap();
            },
        );

        let events = progress.borrow();
        assert!(events.iter().any(|event| {
            event.stage == IndexingProgressStage::ValidatingExistingIndex
                && event.reused_frames == 3
                && event.expected_reuse_frames == 3
        }));
        assert!(events.iter().any(|event| {
            event.stage == IndexingProgressStage::Indexing
                && event.current_timestamp_us == Some(180_000)
        }));
        assert_eq!(
            events.last().unwrap().stage,
            IndexingProgressStage::Finalizing
        );
        assert_eq!(events.last().unwrap().indexed_frames, 5);
        let ordinary_counts = events
            .iter()
            .filter(|event| event.stage != IndexingProgressStage::RebuildingIndex)
            .map(|event| event.indexed_frames)
            .collect::<Vec<_>>();
        assert!(ordinary_counts.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn progress_reports_partial_mismatch_rebuild_without_hiding_it() {
        let path = temp_db("progress-rebuild");
        let mut index = open_index(&path);
        let _ = build_or_resume_frame_index(
            &mut index,
            || {
                Ok(FakeDecoder {
                    stream: stream(),
                    frames: VecDeque::from(vec![
                        Ok(frame(0, 0, true)),
                        Ok(frame(1, 40, false)),
                        Err(FrameScopeError::Cancelled),
                    ]),
                    seek_lands_at_frame: 0,
                })
            },
            IndexingOptions::with_batch_size(8).unwrap(),
        );
        let progress = Rc::new(RefCell::new(Vec::new()));
        let captured = Rc::clone(&progress);
        let changed = [(0, true), (50, false), (100, false)];
        with_indexing_progress_observer(
            move |event| captured.borrow_mut().push(event.stage),
            || {
                build_or_resume_frame_index(
                    &mut index,
                    || Ok(decoder(&changed)),
                    IndexingOptions::with_batch_size(2).unwrap(),
                )
                .unwrap();
            },
        );
        assert!(
            progress
                .borrow()
                .contains(&IndexingProgressStage::RebuildingIndex)
        );
    }

    #[test]
    fn corrupt_keyframe_is_not_promoted_to_anchor() {
        let mut anchor = KeyframeAnchor::StreamStart;
        let mut corrupt = frame(0, 0, true);
        corrupt.corrupt = true;
        let first = entry_from_decoded(FrameId(0), &corrupt, &mut anchor);
        assert_eq!(first.anchor, KeyframeAnchor::StreamStart);
        let second = entry_from_decoded(FrameId(1), &frame(1, 40, true), &mut anchor);
        assert!(matches!(
            second.anchor,
            KeyframeAnchor::Keyframe {
                frame_id: FrameId(1),
                ..
            }
        ));
    }
}
