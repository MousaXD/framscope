use framescope_cache::FrameId;
use framescope_group_navigation::timeline_global::{
    TimelineGlobalSimilarityDisposition, TimelineGlobalSimilarityError,
};
#[cfg(unix)]
use framescope_group_navigation_video::open_or_build_global_similarity_from_fd_with_timeline;
use framescope_similarity_store::global::GlobalSimilarityPolicy;
use framescope_video::CancellationToken;
use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jlong, jstring};
use serde::Serialize;
use std::collections::HashMap;
#[cfg(unix)]
use std::os::fd::AsFd;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use super::{ENGINE_VERSION, OperationId, microscope, to_jstring};

const MAX_CACHE_ROOT_LENGTH: usize = 4_096;
const MAX_PENDING_SIMILARITY_OPERATIONS: usize = 64;

static SIMILARITY_TOKENS: OnceLock<Mutex<HashMap<OperationId, CancellationToken>>> = OnceLock::new();

#[derive(Debug)]
struct SimilarityOperation {
    id: OperationId,
}

impl Drop for SimilarityOperation {
    fn drop(&mut self) {
        if let Ok(mut tokens) = similarity_tokens().lock() {
            tokens.remove(&self.id);
        }
    }
}

#[derive(Debug, Serialize)]
struct SimilarityMatch {
    frame_id: u64,
    similarity: u16,
}

#[derive(Debug, Serialize)]
struct SimilarityDetails {
    session_id: i64,
    target_frame_id: u64,
    descriptor_count: u64,
    candidate_count: u64,
    matched_count: u64,
    truncated: bool,
    minimum_similarity: u16,
    disposition: &'static str,
    matches: Vec<SimilarityMatch>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum SimilarityResponse {
    Ok {
        engine: &'static str,
        result: SimilarityDetails,
    },
    Error {
        engine: &'static str,
        code: String,
        message: String,
    },
}

#[derive(Debug)]
struct SimilarityFailure {
    code: String,
    message: String,
}

impl SimilarityFailure {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

fn similarity_tokens() -> &'static Mutex<HashMap<OperationId, CancellationToken>> {
    SIMILARITY_TOKENS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn similarity_operation(
    operation_id: OperationId,
) -> Result<(CancellationToken, SimilarityOperation), SimilarityFailure> {
    if operation_id <= 0 {
        return Err(SimilarityFailure::new(
            "invalid_request",
            "similarity operation id must be positive",
        ));
    }
    let mut tokens = similarity_tokens().lock().map_err(|_| {
        SimilarityFailure::new("bridge_error", "similarity cancellation state is poisoned")
    })?;
    if tokens.len() >= MAX_PENDING_SIMILARITY_OPERATIONS && !tokens.contains_key(&operation_id) {
        tokens.retain(|_, token| !token.is_cancelled());
    }
    if tokens.len() >= MAX_PENDING_SIMILARITY_OPERATIONS && !tokens.contains_key(&operation_id) {
        return Err(SimilarityFailure::new(
            "operation_busy",
            "too many similarity operations are pending cancellation",
        ));
    }
    let token = tokens
        .entry(operation_id)
        .or_insert_with(CancellationToken::new)
        .clone();
    Ok((token, SimilarityOperation { id: operation_id }))
}

fn cancel_similarity(operation_id: OperationId) -> bool {
    if operation_id <= 0 {
        return false;
    }
    let Ok(mut tokens) = similarity_tokens().lock() else {
        return false;
    };
    if tokens.len() >= MAX_PENDING_SIMILARITY_OPERATIONS && !tokens.contains_key(&operation_id) {
        tokens.retain(|_, token| !token.is_cancelled());
    }
    if tokens.len() >= MAX_PENDING_SIMILARITY_OPERATIONS && !tokens.contains_key(&operation_id) {
        return false;
    }
    let token = tokens
        .entry(operation_id)
        .or_insert_with(CancellationToken::new)
        .clone();
    token.cancel();
    true
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_MicroscopeSimilarityBridge_nativeFindSimilarFrames(
    mut env: JNIEnv,
    _class: JClass,
    session_id: jlong,
    target_frame_id: jlong,
    operation_id: jlong,
    cache_root: JString,
) -> jstring {
    let cache_root: String = match env.get_string(&cache_root) {
        Ok(value) => value.into(),
        Err(_) => {
            return to_jstring(
                &mut env,
                &serialize_similarity_response(Err(SimilarityFailure::new(
                    "invalid_request",
                    "similarity cache root is not valid UTF-8",
                ))),
            );
        }
    };
    let json = catch_unwind(AssertUnwindSafe(|| {
        find_similar_frames(session_id, target_frame_id, operation_id, &cache_root)
    }))
    .map(serialize_similarity_response)
    .unwrap_or_else(|_| {
        serialize_similarity_response(Err(SimilarityFailure::new(
            "bridge_error",
            "native similarity analysis aborted safely after an internal panic",
        )))
    });
    to_jstring(&mut env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_MicroscopeSimilarityBridge_nativeCancelSimilarity(
    _env: JNIEnv,
    _class: JClass,
    operation_id: jlong,
) -> jboolean {
    if catch_unwind(AssertUnwindSafe(|| cancel_similarity(operation_id))).unwrap_or(false) {
        1
    } else {
        0
    }
}

#[cfg(unix)]
fn find_similar_frames(
    session_id: i64,
    target_frame_id: i64,
    operation_id: i64,
    cache_root: &str,
) -> Result<SimilarityDetails, SimilarityFailure> {
    if session_id <= 0 || target_frame_id < 0 || operation_id <= 0 {
        return Err(SimilarityFailure::new(
            "invalid_request",
            "similarity requires a positive session id, non-negative frame id, and positive operation id",
        ));
    }
    if cache_root.trim().is_empty() || cache_root.len() > MAX_CACHE_ROOT_LENGTH {
        return Err(SimilarityFailure::new(
            "invalid_request",
            "similarity cache root is empty or exceeds the safety limit",
        ));
    }

    let target_frame_id = u64::try_from(target_frame_id).map_err(|_| {
        SimilarityFailure::new("invalid_request", "target frame id must be non-negative")
    })?;
    let (cancellation, _operation) = similarity_operation(operation_id)?;
    if cancellation.is_cancelled() {
        return Err(SimilarityFailure::new(
            "cancelled",
            "similarity analysis was cancelled",
        ));
    }

    let context = microscope::similarity_session_context(session_id)
        .map_err(|error| SimilarityFailure::new(error.code(), error.message()))?;
    let store_root = Path::new(cache_root).join("global-similarity");
    let result = open_or_build_global_similarity_from_fd_with_timeline(
        &context.source_identity,
        &context.stream_identity,
        context.frame_count,
        |frame_id| {
            microscope::similarity_index_entry(session_id, frame_id).map_err(|error| {
                TimelineGlobalSimilarityError::TimelineProvider(format!(
                    "{}: {}",
                    error.code(),
                    error.message()
                ))
            })
        },
        &store_root,
        FrameId(target_frame_id),
        context.source_fd.as_fd(),
        cancellation.clone(),
    )
    .map_err(map_similarity_error)?;

    if cancellation.is_cancelled() {
        return Err(SimilarityFailure::new(
            "cancelled",
            "similarity analysis was cancelled",
        ));
    }

    let policy = GlobalSimilarityPolicy::default();
    Ok(SimilarityDetails {
        session_id,
        target_frame_id,
        descriptor_count: result.descriptor_count,
        candidate_count: result.query.candidate_count,
        matched_count: result.query.matched_count,
        truncated: result.query.truncated,
        minimum_similarity: policy.minimum_similarity,
        disposition: match result.disposition {
            TimelineGlobalSimilarityDisposition::Reused => "reused",
            TimelineGlobalSimilarityDisposition::Built => "built",
        },
        matches: result
            .query
            .matches
            .into_iter()
            .map(|value| SimilarityMatch {
                frame_id: value.frame_id.0,
                similarity: value.similarity,
            })
            .collect(),
    })
}

#[cfg(not(unix))]
fn find_similar_frames(
    _session_id: i64,
    _target_frame_id: i64,
    _operation_id: i64,
    _cache_root: &str,
) -> Result<SimilarityDetails, SimilarityFailure> {
    Err(SimilarityFailure::new(
        "bridge_error",
        "microscope similarity is unavailable on this platform",
    ))
}

fn map_similarity_error(error: TimelineGlobalSimilarityError) -> SimilarityFailure {
    match error {
        TimelineGlobalSimilarityError::Cancelled => {
            SimilarityFailure::new("cancelled", "similarity analysis was cancelled")
        }
        TimelineGlobalSimilarityError::UnsafeSourceIdentity => SimilarityFailure::new(
            "unsafe_source_identity",
            "similarity requires a source identity that is safe for reusable derived indexes",
        ),
        TimelineGlobalSimilarityError::FrameOutsideIndex { .. } => SimilarityFailure::new(
            "frame_out_of_range",
            "target frame is outside the completed authoritative index",
        ),
        TimelineGlobalSimilarityError::Source(error) => {
            SimilarityFailure::new(error.code, error.message)
        }
        TimelineGlobalSimilarityError::TimelineProvider(message) => {
            SimilarityFailure::new("timeline_provider_error", message)
        }
        TimelineGlobalSimilarityError::TimelineMismatch(message) => {
            SimilarityFailure::new("timeline_mismatch", message)
        }
        TimelineGlobalSimilarityError::InvalidFreshStore(message) => {
            SimilarityFailure::new("similarity_store_error", message)
        }
        TimelineGlobalSimilarityError::Store(error) => {
            SimilarityFailure::new("similarity_store_error", error.to_string())
        }
    }
}

fn serialize_similarity_response(result: Result<SimilarityDetails, SimilarityFailure>) -> String {
    let response = match result {
        Ok(result) => SimilarityResponse::Ok {
            engine: ENGINE_VERSION,
            result,
        },
        Err(error) => SimilarityResponse::Error {
            engine: ENGINE_VERSION,
            code: error.code,
            message: error.message,
        },
    };
    serde_json::to_string(&response).unwrap_or_else(|_| {
        concat!(
            r#"{"status":"error","engine":"framescope-rust/unknown","code":"bridge_error","#,
            r#""message":"failed to serialize similarity response"}"#,
        )
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_can_arrive_before_operation_registration() {
        let operation_id = 9_100_001;
        assert!(cancel_similarity(operation_id));
        let (token, guard) = similarity_operation(operation_id).unwrap();
        assert!(token.is_cancelled());
        drop(guard);
    }

    #[test]
    fn cancellation_maps_to_stable_android_code() {
        let failure = map_similarity_error(TimelineGlobalSimilarityError::Cancelled);
        assert_eq!(failure.code, "cancelled");
    }

    #[test]
    fn serialized_error_identifies_engine_and_code() {
        let json = serialize_similarity_response(Err(SimilarityFailure::new(
            "test_error",
            "test message",
        )));
        assert!(json.contains("\"status\":\"error\""));
        assert!(json.contains("\"code\":\"test_error\""));
        assert!(json.contains("\"engine\":\"framescope-rust/"));
    }
}
