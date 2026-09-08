use framescope_video::{IndexingProgress, IndexingProgressStage};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use super::OperationId;

const MAX_ACTIVE_PROGRESS_OPERATIONS: usize = 8;
static MICROSCOPE_INDEX_PROGRESS: OnceLock<Mutex<HashMap<OperationId, ProgressState>>> =
    OnceLock::new();

#[derive(Debug)]
struct ProgressState {
    started_at: Instant,
    stage: &'static str,
    indexed_frames: u64,
    reused_frames: u64,
    expected_reuse_frames: u64,
    first_timestamp_us: Option<i64>,
    current_timestamp_us: Option<i64>,
}

#[derive(Debug)]
pub(crate) struct ProgressOperation {
    id: OperationId,
}

impl Drop for ProgressOperation {
    fn drop(&mut self) {
        if let Ok(mut states) = progress_states().lock() {
            states.remove(&self.id);
        }
    }
}

#[derive(Debug, Serialize)]
struct ProgressPayload {
    operation_id: OperationId,
    stage: &'static str,
    indexed_frames: u64,
    reused_frames: u64,
    expected_reuse_frames: u64,
    first_timestamp_us: Option<i64>,
    current_timestamp_us: Option<i64>,
    elapsed_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum ProgressResponse {
    Ok { progress: ProgressPayload },
    Idle,
}

fn progress_states() -> &'static Mutex<HashMap<OperationId, ProgressState>> {
    MICROSCOPE_INDEX_PROGRESS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn begin(operation_id: OperationId) -> Option<ProgressOperation> {
    if operation_id <= 0 {
        return None;
    }
    let mut states = progress_states().lock().ok()?;
    if states.len() >= MAX_ACTIVE_PROGRESS_OPERATIONS && !states.contains_key(&operation_id) {
        return None;
    }
    states.insert(
        operation_id,
        ProgressState {
            started_at: Instant::now(),
            stage: "probing_media",
            indexed_frames: 0,
            reused_frames: 0,
            expected_reuse_frames: 0,
            first_timestamp_us: None,
            current_timestamp_us: None,
        },
    );
    Some(ProgressOperation { id: operation_id })
}

pub(crate) fn update(operation_id: OperationId, progress: IndexingProgress) {
    let Ok(mut states) = progress_states().lock() else {
        return;
    };
    let Some(state) = states.get_mut(&operation_id) else {
        return;
    };
    state.stage = stage_name(progress.stage);
    state.indexed_frames = progress.indexed_frames;
    state.reused_frames = progress.reused_frames;
    state.expected_reuse_frames = progress.expected_reuse_frames;
    if let Some(timestamp_us) = progress.current_timestamp_us {
        state.first_timestamp_us = Some(
            state
                .first_timestamp_us
                .map_or(timestamp_us, |first| first.min(timestamp_us)),
        );
        state.current_timestamp_us = Some(timestamp_us);
    }
}

pub(crate) fn response_json(operation_id: OperationId) -> String {
    let response = progress_states()
        .lock()
        .ok()
        .and_then(|states| {
            states.get(&operation_id).map(|state| ProgressResponse::Ok {
                progress: ProgressPayload {
                    operation_id,
                    stage: state.stage,
                    indexed_frames: state.indexed_frames,
                    reused_frames: state.reused_frames,
                    expected_reuse_frames: state.expected_reuse_frames,
                    first_timestamp_us: state.first_timestamp_us,
                    current_timestamp_us: state.current_timestamp_us,
                    elapsed_ms: u64::try_from(state.started_at.elapsed().as_millis())
                        .unwrap_or(u64::MAX),
                },
            })
        })
        .unwrap_or(ProgressResponse::Idle);
    serde_json::to_string(&response).unwrap_or_else(|_| r#"{"status":"idle"}"#.into())
}

fn stage_name(stage: IndexingProgressStage) -> &'static str {
    match stage {
        IndexingProgressStage::CheckingExistingIndex => "checking_existing_index",
        IndexingProgressStage::ReusingExistingIndex => "reusing_existing_index",
        IndexingProgressStage::ValidatingExistingIndex => "validating_existing_index",
        IndexingProgressStage::RebuildingIndex => "rebuilding_index",
        IndexingProgressStage::Indexing => "indexing",
        IndexingProgressStage::Finalizing => "finalizing",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_operation_scoped_and_removed_on_drop() {
        let operation_id = 73_001;
        let guard = begin(operation_id).expect("progress slot");
        update(
            operation_id,
            IndexingProgress {
                stage: IndexingProgressStage::Indexing,
                indexed_frames: 128,
                reused_frames: 32,
                expected_reuse_frames: 32,
                current_timestamp_us: Some(4_200_000),
            },
        );
        let json = response_json(operation_id);
        assert!(json.contains("\"stage\":\"indexing\""));
        assert!(json.contains("\"indexed_frames\":128"));
        assert!(json.contains("\"first_timestamp_us\":4200000"));
        drop(guard);
        assert_eq!(response_json(operation_id), r#"{"status":"idle"}"#);
    }

    #[test]
    fn timestamp_origin_tracks_earliest_observed_vfr_pts() {
        let operation_id = 73_002;
        let guard = begin(operation_id).expect("progress slot");
        for timestamp_us in [205_000, 100_000, 141_000] {
            update(
                operation_id,
                IndexingProgress {
                    stage: IndexingProgressStage::Indexing,
                    indexed_frames: 1,
                    reused_frames: 0,
                    expected_reuse_frames: 0,
                    current_timestamp_us: Some(timestamp_us),
                },
            );
        }
        let json = response_json(operation_id);
        assert!(json.contains("\"first_timestamp_us\":100000"));
        assert!(json.contains("\"current_timestamp_us\":141000"));
        drop(guard);
    }

    #[test]
    fn partial_resume_can_validate_before_persisted_tail_without_becoming_invalid() {
        let operation_id = 73_003;
        let guard = begin(operation_id).expect("progress slot");
        update(
            operation_id,
            IndexingProgress {
                stage: IndexingProgressStage::CheckingExistingIndex,
                indexed_frames: 64,
                reused_frames: 0,
                expected_reuse_frames: 64,
                current_timestamp_us: Some(2_800_000),
            },
        );
        update(
            operation_id,
            IndexingProgress {
                stage: IndexingProgressStage::ValidatingExistingIndex,
                indexed_frames: 64,
                reused_frames: 1,
                expected_reuse_frames: 64,
                current_timestamp_us: Some(0),
            },
        );
        let json = response_json(operation_id);
        assert!(json.contains("\"first_timestamp_us\":0"));
        assert!(json.contains("\"current_timestamp_us\":0"));
        drop(guard);
    }
}
