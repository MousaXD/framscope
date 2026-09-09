use framescope_video::{IndexingProgress, IndexingProgressStage};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::OperationId;

const MAX_ACTIVE_PROGRESS_OPERATIONS: usize = 8;
static MICROSCOPE_INDEX_PROGRESS: OnceLock<Mutex<HashMap<OperationId, ProgressState>>> =
    OnceLock::new();

#[derive(Debug)]
struct ProgressState {
    started_at: Instant,
    last_sample_at: Instant,
    sequence: u64,
    sample_elapsed_ms: u64,
    sample_interval_us: u64,
    sample_frame_advance: u64,
    sample_frames_per_second_milli: u64,
    last_work_advance_elapsed_ms: u64,
    stage: &'static str,
    indexed_frames: u64,
    reused_frames: u64,
    expected_reuse_frames: u64,
    first_timestamp_us: Option<i64>,
    current_timestamp_us: Option<i64>,
    max_presentation_timestamp_us: Option<i64>,
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
    sequence: u64,
    stage: &'static str,
    indexed_frames: u64,
    reused_frames: u64,
    expected_reuse_frames: u64,
    first_timestamp_us: Option<i64>,
    current_timestamp_us: Option<i64>,
    max_presentation_timestamp_us: Option<i64>,
    sample_elapsed_ms: u64,
    /// Monotonic interval represented by this native progress sample.
    sample_interval_us: u64,
    /// Authoritative presentation-frame work completed during this sample. Existing-index discovery
    /// and reuse do not count as decode throughput; validation counts `reused_frames` advancement and
    /// fresh indexing counts `indexed_frames` advancement.
    sample_frame_advance: u64,
    /// Last native indexing sample throughput in frames/second, multiplied by 1000.
    sample_frames_per_second_milli: u64,
    last_work_advance_elapsed_ms: u64,
    operation_elapsed_ms: u64,
    // Kept as an additive compatibility alias for older diagnostics. New estimators must use
    // sample_elapsed_ms for throughput and operation_elapsed_ms only for operation age/stalls.
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

fn elapsed_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn elapsed_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn frames_per_second_milli(frames: u64, interval_us: u64) -> u64 {
    if frames == 0 || interval_us == 0 {
        return 0;
    }
    let scaled = u128::from(frames)
        .saturating_mul(1_000_000_000)
        .checked_div(u128::from(interval_us))
        .unwrap_or(0);
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

pub(crate) fn begin(operation_id: OperationId) -> Option<ProgressOperation> {
    if operation_id <= 0 {
        return None;
    }
    let mut states = progress_states().lock().ok()?;
    if states.len() >= MAX_ACTIVE_PROGRESS_OPERATIONS && !states.contains_key(&operation_id) {
        return None;
    }
    let started_at = Instant::now();
    states.insert(
        operation_id,
        ProgressState {
            started_at,
            last_sample_at: started_at,
            sequence: 0,
            sample_elapsed_ms: 0,
            sample_interval_us: 0,
            sample_frame_advance: 0,
            sample_frames_per_second_milli: 0,
            last_work_advance_elapsed_ms: 0,
            stage: "probing_media",
            indexed_frames: 0,
            reused_frames: 0,
            expected_reuse_frames: 0,
            first_timestamp_us: None,
            current_timestamp_us: None,
            max_presentation_timestamp_us: None,
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

    let sampled_at = Instant::now();
    let sample_elapsed_ms = elapsed_ms(sampled_at.duration_since(state.started_at));
    let sample_interval_us = elapsed_us(sampled_at.duration_since(state.last_sample_at));
    let previous_indexed_frames = state.indexed_frames;
    let previous_reused_frames = state.reused_frames;
    let previous_max_timestamp_us = state.max_presentation_timestamp_us;
    let indexed_advance = progress
        .indexed_frames
        .saturating_sub(previous_indexed_frames);
    let reused_advance = progress.reused_frames.saturating_sub(previous_reused_frames);
    let sample_frame_advance = match progress.stage {
        IndexingProgressStage::ValidatingExistingIndex => reused_advance,
        IndexingProgressStage::Indexing => indexed_advance,
        _ => 0,
    };

    // A rebuild starts a new authoritative coverage attempt. Do not let a rejected persisted tail
    // make fresh indexing appear to have already covered that old presentation range.
    if progress.stage == IndexingProgressStage::RebuildingIndex {
        state.first_timestamp_us = None;
        state.current_timestamp_us = None;
        state.max_presentation_timestamp_us = None;
    }

    state.sequence = state.sequence.saturating_add(1);
    state.sample_elapsed_ms = sample_elapsed_ms;
    state.sample_interval_us = sample_interval_us;
    state.sample_frame_advance = sample_frame_advance;
    state.sample_frames_per_second_milli =
        frames_per_second_milli(sample_frame_advance, sample_interval_us);
    state.last_sample_at = sampled_at;
    state.stage = stage_name(progress.stage);
    state.indexed_frames = progress.indexed_frames;
    state.reused_frames = progress.reused_frames;
    state.expected_reuse_frames = progress.expected_reuse_frames;
    state.current_timestamp_us = progress.current_timestamp_us;

    if let Some(timestamp_us) = progress.current_timestamp_us {
        state.first_timestamp_us = Some(
            state
                .first_timestamp_us
                .map_or(timestamp_us, |first| first.min(timestamp_us)),
        );
        state.max_presentation_timestamp_us = Some(
            state
                .max_presentation_timestamp_us
                .map_or(timestamp_us, |maximum| maximum.max(timestamp_us)),
        );
    }

    let coverage_advanced = match (
        previous_max_timestamp_us,
        state.max_presentation_timestamp_us,
    ) {
        (Some(previous), Some(current)) => current > previous,
        (None, Some(_)) => true,
        _ => false,
    };
    let work_advanced = state.indexed_frames > previous_indexed_frames
        || state.reused_frames > previous_reused_frames
        || coverage_advanced;
    if work_advanced {
        state.last_work_advance_elapsed_ms = sample_elapsed_ms;
    }
}

pub(crate) fn response_json(operation_id: OperationId) -> String {
    let response = progress_states()
        .lock()
        .ok()
        .and_then(|states| {
            states.get(&operation_id).map(|state| {
                let operation_elapsed_ms = elapsed_ms(state.started_at.elapsed());
                ProgressResponse::Ok {
                    progress: ProgressPayload {
                        operation_id,
                        sequence: state.sequence,
                        stage: state.stage,
                        indexed_frames: state.indexed_frames,
                        reused_frames: state.reused_frames,
                        expected_reuse_frames: state.expected_reuse_frames,
                        first_timestamp_us: state.first_timestamp_us,
                        current_timestamp_us: state.current_timestamp_us,
                        max_presentation_timestamp_us: state.max_presentation_timestamp_us,
                        sample_elapsed_ms: state.sample_elapsed_ms,
                        sample_interval_us: state.sample_interval_us,
                        sample_frame_advance: state.sample_frame_advance,
                        sample_frames_per_second_milli: state.sample_frames_per_second_milli,
                        last_work_advance_elapsed_ms: state.last_work_advance_elapsed_ms,
                        operation_elapsed_ms,
                        elapsed_ms: operation_elapsed_ms,
                    },
                }
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
    use serde_json::Value;
    use std::thread;

    fn payload(operation_id: OperationId) -> Value {
        let root: Value = serde_json::from_str(&response_json(operation_id)).expect("valid JSON");
        root.get("progress").cloned().expect("progress payload")
    }

    fn indexing_progress(
        stage: IndexingProgressStage,
        indexed_frames: u64,
        reused_frames: u64,
        expected_reuse_frames: u64,
        current_timestamp_us: Option<i64>,
    ) -> IndexingProgress {
        IndexingProgress {
            stage,
            indexed_frames,
            reused_frames,
            expected_reuse_frames,
            current_timestamp_us,
        }
    }

    #[test]
    fn progress_is_operation_scoped_and_removed_on_drop() {
        let operation_id = 73_001;
        let guard = begin(operation_id).expect("progress slot");
        update(
            operation_id,
            indexing_progress(
                IndexingProgressStage::Indexing,
                128,
                32,
                32,
                Some(4_200_000),
            ),
        );
        let json = response_json(operation_id);
        assert!(json.contains("\"stage\":\"indexing\""));
        assert!(json.contains("\"indexed_frames\":128"));
        assert!(json.contains("\"first_timestamp_us\":4200000"));
        assert!(json.contains("\"sample_frame_advance\":128"));
        drop(guard);
        assert_eq!(response_json(operation_id), r#"{"status":"idle"}"#);
    }

    #[test]
    fn repeated_reads_do_not_create_new_native_samples() {
        let operation_id = 73_002;
        let guard = begin(operation_id).expect("progress slot");
        update(
            operation_id,
            indexing_progress(IndexingProgressStage::Indexing, 64, 0, 0, Some(1_000_000)),
        );
        let first = payload(operation_id);
        thread::sleep(Duration::from_millis(3));
        let repeated = payload(operation_id);

        assert_eq!(first["sequence"], repeated["sequence"]);
        assert_eq!(first["sample_elapsed_ms"], repeated["sample_elapsed_ms"]);
        assert_eq!(first["sample_interval_us"], repeated["sample_interval_us"]);
        assert_eq!(first["sample_frame_advance"], repeated["sample_frame_advance"]);
        assert_eq!(
            first["sample_frames_per_second_milli"],
            repeated["sample_frames_per_second_milli"]
        );
        assert_eq!(
            first["last_work_advance_elapsed_ms"],
            repeated["last_work_advance_elapsed_ms"]
        );
        assert!(
            repeated["operation_elapsed_ms"].as_u64().unwrap()
                >= first["operation_elapsed_ms"].as_u64().unwrap()
        );

        thread::sleep(Duration::from_millis(3));
        update(
            operation_id,
            indexing_progress(IndexingProgressStage::Indexing, 128, 0, 0, Some(2_000_000)),
        );
        let advanced = payload(operation_id);
        assert_eq!(
            advanced["sequence"].as_u64().unwrap(),
            first["sequence"].as_u64().unwrap() + 1
        );
        assert!(
            advanced["sample_elapsed_ms"].as_u64().unwrap()
                > first["sample_elapsed_ms"].as_u64().unwrap()
        );
        assert_eq!(advanced["sample_frame_advance"], 64);
        assert!(advanced["sample_interval_us"].as_u64().unwrap() >= 1_000);
        assert!(advanced["sample_frames_per_second_milli"].as_u64().unwrap() > 0);
        drop(guard);
    }

    #[test]
    fn throughput_ignores_persisted_prefix_discovery_and_counts_validation_work() {
        let operation_id = 73_006;
        let guard = begin(operation_id).expect("progress slot");
        update(
            operation_id,
            indexing_progress(
                IndexingProgressStage::CheckingExistingIndex,
                8_000,
                0,
                8_000,
                Some(20_000_000),
            ),
        );
        let checking = payload(operation_id);
        assert_eq!(checking["sample_frame_advance"], 0);
        assert_eq!(checking["sample_frames_per_second_milli"], 0);

        thread::sleep(Duration::from_millis(2));
        update(
            operation_id,
            indexing_progress(
                IndexingProgressStage::ValidatingExistingIndex,
                8_000,
                64,
                8_000,
                Some(2_000_000),
            ),
        );
        let validating = payload(operation_id);
        assert_eq!(validating["sample_frame_advance"], 64);
        assert!(validating["sample_frames_per_second_milli"].as_u64().unwrap() > 0);
        drop(guard);
    }

    #[test]
    fn current_timestamp_can_regress_while_media_coverage_remains_monotonic() {
        let operation_id = 73_003;
        let guard = begin(operation_id).expect("progress slot");
        for (frames, timestamp_us) in [(64, 4_000_000), (128, 4_200_000), (192, 4_100_000)] {
            update(
                operation_id,
                indexing_progress(
                    IndexingProgressStage::Indexing,
                    frames,
                    0,
                    0,
                    Some(timestamp_us),
                ),
            );
        }
        let progress = payload(operation_id);
        assert_eq!(progress["current_timestamp_us"], 4_100_000);
        assert_eq!(progress["max_presentation_timestamp_us"], 4_200_000);
        assert_eq!(progress["first_timestamp_us"], 4_000_000);
        drop(guard);
    }

    #[test]
    fn rebuild_clears_rejected_persisted_media_coverage() {
        let operation_id = 73_004;
        let guard = begin(operation_id).expect("progress slot");
        update(
            operation_id,
            indexing_progress(
                IndexingProgressStage::CheckingExistingIndex,
                8_000,
                0,
                8_000,
                Some(20_000_000),
            ),
        );
        update(
            operation_id,
            indexing_progress(IndexingProgressStage::RebuildingIndex, 0, 0, 0, None),
        );
        let rebuilding = payload(operation_id);
        assert!(rebuilding["first_timestamp_us"].is_null());
        assert!(rebuilding["current_timestamp_us"].is_null());
        assert!(rebuilding["max_presentation_timestamp_us"].is_null());
        assert_eq!(rebuilding["sample_frame_advance"], 0);

        update(
            operation_id,
            indexing_progress(IndexingProgressStage::Indexing, 1, 0, 0, Some(100_000)),
        );
        let restarted = payload(operation_id);
        assert_eq!(restarted["first_timestamp_us"], 100_000);
        assert_eq!(restarted["max_presentation_timestamp_us"], 100_000);
        assert_eq!(restarted["sample_frame_advance"], 1);
        drop(guard);
    }

    #[test]
    fn partial_resume_can_validate_before_persisted_tail_without_becoming_invalid() {
        let operation_id = 73_005;
        let guard = begin(operation_id).expect("progress slot");
        update(
            operation_id,
            indexing_progress(
                IndexingProgressStage::CheckingExistingIndex,
                64,
                0,
                64,
                Some(2_800_000),
            ),
        );
        update(
            operation_id,
            indexing_progress(
                IndexingProgressStage::ValidatingExistingIndex,
                64,
                1,
                64,
                Some(0),
            ),
        );
        let progress = payload(operation_id);
        assert_eq!(progress["first_timestamp_us"], 0);
        assert_eq!(progress["current_timestamp_us"], 0);
        assert_eq!(progress["max_presentation_timestamp_us"], 2_800_000);
        assert_eq!(progress["sample_frame_advance"], 1);
        drop(guard);
    }
}
