use crate::VideoDecoder;
use framescope_cache::{
    FrameId, FrameIndex, FrameIndexEntry, FrameIndexError, FrameIndexLifecycle, FrameIndexStatus,
    FrameIndexStreamIdentity, KeyframeAnchor,
};
use framescope_core::{DecodedFrame, FrameScopeError, StreamInfo};
use thiserror::Error;

const DEFAULT_BATCH_SIZE: usize = 256;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexingReport {
    pub status: FrameIndexStatus,
    pub reused_existing_frames: u64,
    pub newly_indexed_frames: u64,
    pub restarted_after_partial_mismatch: bool,
    pub max_pending_entries: usize,
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
/// FFmpeg and avoids test-only global hooks.
pub trait FrameIndexDecoder {
    fn selected_stream_for_index(&self) -> &StreamInfo;
    fn next_frame_for_index(&mut self) -> Result<Option<DecodedFrame>, FrameScopeError>;
}

impl FrameIndexDecoder for VideoDecoder {
    fn selected_stream_for_index(&self) -> &StreamInfo {
        self.selected_stream()
    }

    fn next_frame_for_index(&mut self) -> Result<Option<DecodedFrame>, FrameScopeError> {
        self.next_frame()
    }
}

/// Build or safely resume a persistent frame index using a fresh decoder factory.
///
/// Existing committed rows are re-decoded from stream start and reconciled before any new rows are
/// appended. This deliberately avoids treating Phase 2's epoch-local decoder frame number as codec
/// state. Only a bounded metadata batch is retained in memory.
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

    let initial_status = index.status()?;
    if initial_status.lifecycle == FrameIndexLifecycle::Complete {
        return Ok(IndexingReport {
            status: initial_status,
            reused_existing_frames: 0,
            newly_indexed_frames: 0,
            restarted_after_partial_mismatch: false,
            max_pending_entries: 0,
        });
    }

    let mut expected_existing = initial_status.indexed_frames;
    let mut restarted = false;
    let mut max_pending = 0_usize;

    for attempt in 0..2 {
        let mut decoder = match open_fresh_decoder() {
            Ok(decoder) => decoder,
            Err(error) => {
                let _ = index.mark_failed_recoverable(&error.to_string());
                return Err(IndexingError::Decoder(error));
            }
        };
        let actual_stream =
            FrameIndexStreamIdentity::from_stream(decoder.selected_stream_for_index())?;
        if &actual_stream != index.stream_identity() {
            let _ = index.mark_failed_recoverable("selected stream identity changed");
            return Err(IndexingError::StreamIdentityMismatch);
        }

        index.mark_building()?;
        let mut frame_id = FrameId::ZERO;
        let mut anchor = KeyframeAnchor::StreamStart;
        let mut batch = Vec::with_capacity(options.batch_size);
        let mut reused = 0_u64;
        let mut added = 0_u64;
        let mut mismatch = false;

        loop {
            let decoded = match decoder.next_frame_for_index() {
                Ok(value) => value,
                Err(FrameScopeError::Cancelled) => {
                    flush_batch(index, &mut batch, &mut added)?;
                    let _ = index.mark_incomplete(Some("cancelled"));
                    return Err(IndexingError::Decoder(FrameScopeError::Cancelled));
                }
                Err(error) => {
                    if let Err(storage) = flush_batch(index, &mut batch, &mut added) {
                        let _ = index.mark_failed_recoverable(&storage.to_string());
                        return Err(storage);
                    }
                    let _ = index.mark_failed_recoverable(&error.to_string());
                    return Err(IndexingError::Decoder(error));
                }
            };

            let Some(decoded) = decoded else {
                if frame_id.0 < expected_existing {
                    mismatch = true;
                    break;
                }
                flush_batch(index, &mut batch, &mut added)?;
                index.mark_complete()?;
                return Ok(IndexingReport {
                    status: index.status()?,
                    reused_existing_frames: reused,
                    newly_indexed_frames: added,
                    restarted_after_partial_mismatch: restarted,
                    max_pending_entries: max_pending,
                });
            };

            let entry = entry_from_decoded(frame_id, &decoded, &mut anchor);
            entry.validate(index.stream_identity())?;

            if frame_id.0 < expected_existing {
                match index.entry(frame_id)? {
                    Some(existing) if existing == entry => {
                        reused = reused.saturating_add(1);
                    }
                    _ => {
                        mismatch = true;
                        break;
                    }
                }
            } else {
                batch.push(entry);
                max_pending = max_pending.max(batch.len());
                if batch.len() >= options.batch_size {
                    flush_batch(index, &mut batch, &mut added)?;
                }
            }

            frame_id = FrameId(
                frame_id
                    .0
                    .checked_add(1)
                    .ok_or_else(|| FrameIndexError::InvalidState("frame id overflow".into()))?,
            );
        }

        if mismatch {
            if attempt != 0 {
                let _ = index.mark_failed_recoverable("partial index reconciliation failed");
                return Err(IndexingError::PartialIndexMismatch);
            }
            index.clear_for_rebuild()?;
            expected_existing = 0;
            restarted = true;
        }
    }

    Err(IndexingError::PartialIndexMismatch)
}

fn flush_batch(
    index: &mut FrameIndex,
    batch: &mut Vec<FrameIndexEntry>,
    added: &mut u64,
) -> Result<(), IndexingError> {
    if batch.is_empty() {
        return Ok(());
    }
    let count = batch.len() as u64;
    if let Err(error) = index.append_batch(batch) {
        let _ = index.mark_failed_recoverable(&error.to_string());
        return Err(IndexingError::Storage(error));
    }
    batch.clear();
    *added = added.saturating_add(count);
    Ok(())
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
    use framescope_cache::{FrameIndexOpenDisposition, SourceIdentity};
    use framescope_core::{
        CodecInfo, MediaDuration, MediaKind, MediaTimestamp, TimeBase,
    };
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    #[derive(Clone)]
    struct FakeDecoder {
        stream: StreamInfo,
        frames: VecDeque<Result<DecodedFrame, FrameScopeError>>,
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
        FakeDecoder {
            stream: stream(),
            frames: sequence
                .iter()
                .enumerate()
                .map(|(index, (ticks, keyframe))| Ok(frame(index as u64, *ticks, *keyframe)))
                .collect(),
        }
    }

    fn temp_db(name: &str) -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "framescope-video-index-{}-{name}-{id}.sqlite",
            std::process::id()
        ))
    }

    fn open_index(path: &PathBuf) -> FrameIndex {
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
        let sequence = [(0, true), (40, false), (100, false), (140, false), (260, true)];
        let report = build_or_resume_frame_index(
            &mut index,
            || Ok(decoder(&sequence)),
            IndexingOptions::with_batch_size(2).unwrap(),
        )
        .unwrap();
        assert_eq!(report.status.lifecycle, FrameIndexLifecycle::Complete);
        assert_eq!(report.status.frame_count, Some(5));
        assert!(report.max_pending_entries <= 2);
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
                })
            },
            IndexingOptions::with_batch_size(2).unwrap(),
        );
        assert!(matches!(
            first,
            Err(IndexingError::Decoder(FrameScopeError::Cancelled))
        ));
        assert_eq!(index.status().unwrap().lifecycle, FrameIndexLifecycle::Incomplete);
        assert_eq!(index.status().unwrap().indexed_frames, 3);

        let full = [(0, true), (40, false), (100, false), (180, false), (260, true)];
        let report = build_or_resume_frame_index(
            &mut index,
            || Ok(decoder(&full)),
            IndexingOptions::with_batch_size(2).unwrap(),
        )
        .unwrap();
        assert_eq!(report.reused_existing_frames, 3);
        assert_eq!(report.newly_indexed_frames, 2);
        assert_eq!(report.status.frame_count, Some(5));
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
