use framescope_cache::{FrameId, FrameIndexStreamIdentity, SourceIdentity};
use framescope_group_navigation::timeline_global::{
    TimelineGlobalSimilarityDisposition, TimelineGlobalSimilarityError,
};
use framescope_group_navigation_video::open_or_build_global_similarity_from_fd_with_timeline;
use framescope_video::CancellationToken;
use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jlong, jstring};
use serde::Serialize;
use std::collections::HashMap;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::ptr;
use std::sync::{Mutex, OnceLock};

use crate::{ENGINE_VERSION, microscope, to_jstring};

const MAX_CACHE_ROOT_LENGTH: usize = 4_096;
const MINIMUM_SIMILARITY: u16 = 9_300;
const MAX_SIMILARITY_OPERATIONS: usize = 16;

static SIMILARITY_TOKENS: OnceLock<Mutex<HashMap<i64, CancellationToken>>> = OnceLock::new();

#[derive(Debug)]
struct SimilaritySessionSnapshot {
    source_fd: OwnedFd,
    source_identity: SourceIdentity,
    stream_identity: FrameIndexStreamIdentity,
    frame_count: u64,
}

#[derive(Debug)]
struct SimilarityBridgeFailure {
    code: String,
    message: String,
}

impl SimilarityBridgeFailure {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

struct SimilarityOperation {
    id: i64,
}

impl Drop for SimilarityOperation {
    fn drop(&mut self) {
        if let Ok(mut tokens) = similarity_tokens().lock() {
            tokens.remove(&self.id);
        }
    }
}

fn similarity_tokens() -> &'static Mutex<HashMap<i64, CancellationToken>> {
    SIMILARITY_TOKENS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn operation_token(
    operation_id: i64,
) -> Result<(CancellationToken, SimilarityOperation), SimilarityBridgeFailure> {
    if operation_id <= 0 {
        return Err(SimilarityBridgeFailure::new(
            "invalid_request",
            "similarity operation id must be positive",
        ));
    }
    let mut tokens = similarity_tokens().lock().map_err(|_| {
        SimilarityBridgeFailure::new(
            "similarity_busy",
            "similarity cancellation state is unavailable",
        )
    })?;
    if tokens.len() >= MAX_SIMILARITY_OPERATIONS && !tokens.contains_key(&operation_id) {
        tokens.retain(|_, token| !token.is_cancelled());
    }
    if tokens.len() >= MAX_SIMILARITY_OPERATIONS && !tokens.contains_key(&operation_id) {
        return Err(SimilarityBridgeFailure::new(
            "similarity_busy",
            "too many similarity operations are active",
        ));
    }
    let token = tokens
        .entry(operation_id)
        .or_insert_with(CancellationToken::new)
        .clone();
    Ok((token, SimilarityOperation { id: operation_id }))
}

fn cancel_similarity_operation(operation_id: i64) -> bool {
    if operation_id <= 0 {
        return false;
    }
    let Ok(mut tokens) = similarity_tokens().lock() else {
        return false;
    };
    if tokens.len() >= MAX_SIMILARITY_OPERATIONS && !tokens.contains_key(&operation_id) {
        tokens.retain(|_, token| !token.is_cancelled());
    }
    if tokens.len() >= MAX_SIMILARITY_OPERATIONS && !tokens.contains_key(&operation_id) {
        return false;
    }
    let token = tokens
        .entry(operation_id)
        .or_insert_with(CancellationToken::new)
        .clone();
    token.cancel();
    true
}

#[derive(Debug, Serialize)]
struct SimilarityMatchPayload {
    frame_id: u64,
    similarity: u16,
}

#[derive(Debug, Serialize)]
struct SimilarityResultPayload {
    session_id: i64,
    target_frame_id: u64,
    descriptor_count: u64,
    candidate_count: u64,
    matched_count: u64,
    truncated: bool,
    minimum_similarity: u16,
    disposition: &'static str,
    matches: Vec<SimilarityMatchPayload>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum SimilarityResponse {
    Ok {
        engine: &'static str,
        result: SimilarityResultPayload,
    },
    Error {
        engine: &'static str,
        code: String,
        message: String,
    },
}

fn validate_request(
    session_id: i64,
    target_frame_id: i64,
    operation_id: i64,
    cache_root: &str,
) -> Result<FrameId, SimilarityBridgeFailure> {
    if session_id <= 0 {
        return Err(SimilarityBridgeFailure::new(
            "invalid_request",
            "similarity query requires a positive microscope session id",
        ));
    }
    if target_frame_id < 0 {
        return Err(SimilarityBridgeFailure::new(
            "invalid_request",
            "similarity query requires a non-negative target frame id",
        ));
    }
    if operation_id <= 0 {
        return Err(SimilarityBridgeFailure::new(
            "invalid_request",
            "similarity query requires a positive operation id",
        ));
    }
    if cache_root.is_empty()
        || cache_root.len() > MAX_CACHE_ROOT_LENGTH
        || cache_root.contains('\0')
    {
        return Err(SimilarityBridgeFailure::new(
            "invalid_cache_root",
            "FrameScope cache root is invalid",
        ));
    }
    let frame_id = u64::try_from(target_frame_id).map_err(|_| {
        SimilarityBridgeFailure::new(
            "invalid_request",
            "similarity target frame id is outside the supported range",
        )
    })?;
    Ok(FrameId(frame_id))
}

#[cfg(unix)]
fn snapshot_session(session_id: i64) -> Result<SimilaritySessionSnapshot, SimilarityBridgeFailure> {
    microscope::with_extraction_context(session_id, |fd, index| {
        let frame_count = index
            .frame_count()
            .map_err(|error| SimilarityBridgeFailure::new("index_error", error.to_string()))?
            .ok_or_else(|| {
                SimilarityBridgeFailure::new(
                    "incomplete_index",
                    "global similarity requires a complete authoritative frame index",
                )
            })?;
        // SAFETY: `with_extraction_context` guarantees this raw descriptor belongs to the live
        // microscope session for the duration of this callback. `try_clone_to_owned` duplicates it
        // before the short session lock is released, so the analysis owns an independent descriptor.
        let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
        let source_fd = borrowed.try_clone_to_owned().map_err(|error| {
            SimilarityBridgeFailure::new(
                "source_unavailable",
                format!("could not duplicate microscope source descriptor: {error}"),
            )
        })?;
        Ok(SimilaritySessionSnapshot {
            source_fd,
            source_identity: index.source_identity().clone(),
            stream_identity: index.stream_identity().clone(),
            frame_count,
        })
    })
    .map_err(|error| SimilarityBridgeFailure::new(error.code(), error.message()))?
}

#[cfg(not(unix))]
fn snapshot_session(
    _session_id: i64,
) -> Result<SimilaritySessionSnapshot, SimilarityBridgeFailure> {
    Err(SimilarityBridgeFailure::new(
        "not_supported",
        "global similarity is available only on Android/Unix microscope sessions",
    ))
}

#[cfg(unix)]
fn query_similarity(
    session_id: i64,
    target_frame_id: i64,
    operation_id: i64,
    cache_root: &str,
) -> Result<SimilarityResultPayload, SimilarityBridgeFailure> {
    let target_frame = validate_request(session_id, target_frame_id, operation_id, cache_root)?;
    let (cancellation, _operation) = operation_token(operation_id)?;
    let snapshot = snapshot_session(session_id)?;
    if !snapshot.source_identity.is_reuse_safe() {
        return Err(SimilarityBridgeFailure::new(
            "unsafe_source_identity",
            "the selected video does not have a strong enough identity for reusable similarity data",
        ));
    }
    if target_frame.0 >= snapshot.frame_count {
        return Err(SimilarityBridgeFailure::new(
            "frame_out_of_range",
            format!(
                "target frame {} is outside the completed index containing {} frames",
                target_frame.0, snapshot.frame_count
            ),
        ));
    }

    let store_root = Path::new(cache_root).join("frame-index");
    let result = open_or_build_global_similarity_from_fd_with_timeline(
        &snapshot.source_identity,
        &snapshot.stream_identity,
        snapshot.frame_count,
        |frame_id| {
            microscope::with_extraction_context(session_id, |_fd, index| {
                index.entry(frame_id).map_err(|error| {
                    TimelineGlobalSimilarityError::TimelineProvider(error.to_string())
                })
            })
            .map_err(|error| {
                TimelineGlobalSimilarityError::TimelineProvider(format!(
                    "{}: {}",
                    error.code(),
                    error.message()
                ))
            })?
        },
        store_root,
        target_frame,
        snapshot.source_fd.as_fd(),
        cancellation,
    )
    .map_err(map_timeline_error)?;

    if result.query.target_frame != target_frame
        || result.query.descriptor_count != result.descriptor_count
    {
        return Err(SimilarityBridgeFailure::new(
            "similarity_identity_mismatch",
            "global similarity result no longer matches the requested target/index identity",
        ));
    }
    let disposition = match result.disposition {
        TimelineGlobalSimilarityDisposition::Reused => "reused",
        TimelineGlobalSimilarityDisposition::Built => "built",
    };
    let matches = result
        .query
        .matches
        .into_iter()
        .map(|matched| SimilarityMatchPayload {
            frame_id: matched.frame_id.0,
            similarity: matched.similarity,
        })
        .collect();

    Ok(SimilarityResultPayload {
        session_id,
        target_frame_id: target_frame.0,
        descriptor_count: result.descriptor_count,
        candidate_count: result.query.candidate_count,
        matched_count: result.query.matched_count,
        truncated: result.query.truncated,
        minimum_similarity: MINIMUM_SIMILARITY,
        disposition,
        matches,
    })
}

#[cfg(not(unix))]
fn query_similarity(
    session_id: i64,
    target_frame_id: i64,
    operation_id: i64,
    cache_root: &str,
) -> Result<SimilarityResultPayload, SimilarityBridgeFailure> {
    let _ = validate_request(session_id, target_frame_id, operation_id, cache_root)?;
    Err(SimilarityBridgeFailure::new(
        "not_supported",
        "global similarity is available only on Android/Unix microscope sessions",
    ))
}

fn map_timeline_error(error: TimelineGlobalSimilarityError) -> SimilarityBridgeFailure {
    let code = match &error {
        TimelineGlobalSimilarityError::UnsafeSourceIdentity => "unsafe_source_identity",
        TimelineGlobalSimilarityError::Cancelled => "cancelled",
        TimelineGlobalSimilarityError::Source(source) => source.code,
        TimelineGlobalSimilarityError::Store(_) => "similarity_store_error",
        TimelineGlobalSimilarityError::TimelineProvider(_) => "timeline_unavailable",
        TimelineGlobalSimilarityError::TimelineMismatch(_) => "timeline_mismatch",
        TimelineGlobalSimilarityError::FrameOutsideIndex { .. } => "frame_out_of_range",
        TimelineGlobalSimilarityError::InvalidFreshStore(_) => "similarity_store_invalid",
    };
    SimilarityBridgeFailure::new(code, error.to_string())
}

fn response_json(
    session_id: i64,
    target_frame_id: i64,
    operation_id: i64,
    cache_root: &str,
) -> String {
    let response = match query_similarity(session_id, target_frame_id, operation_id, cache_root) {
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
    serialize_response(response)
}

fn serialize_response(response: SimilarityResponse) -> String {
    serde_json::to_string(&response).unwrap_or_else(|_| {
        format!(
            "{{\"status\":\"error\",\"engine\":\"{ENGINE_VERSION}\",\"code\":\"bridge_error\",\"message\":\"failed to serialize similarity response\"}}"
        )
    })
}

fn panic_response() -> String {
    serialize_response(SimilarityResponse::Error {
        engine: ENGINE_VERSION,
        code: "bridge_error".into(),
        message: "native similarity query aborted safely after an internal panic".into(),
    })
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
        Err(_) => return ptr::null_mut(),
    };
    let json = catch_unwind(AssertUnwindSafe(|| {
        response_json(session_id, target_frame_id, operation_id, &cache_root)
    }))
    .unwrap_or_else(|_| panic_response());
    to_jstring(&mut env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_MicroscopeSimilarityBridge_nativeCancelSimilarity(
    _env: JNIEnv,
    _class: JClass,
    operation_id: jlong,
) -> jboolean {
    let cancelled = catch_unwind(AssertUnwindSafe(|| {
        cancel_similarity_operation(operation_id)
    }))
    .unwrap_or(false);
    if cancelled { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_request_fails_before_session_access() {
        let json = response_json(0, 4, 9, "/tmp/framescope");
        assert!(json.contains("invalid_request"));
    }

    #[test]
    fn invalid_cache_root_fails_before_session_access() {
        let json = response_json(1, 4, 9, "");
        assert!(json.contains("invalid_cache_root"));
    }

    #[test]
    fn cancellation_error_has_stable_wire_code() {
        let failure = map_timeline_error(TimelineGlobalSimilarityError::Cancelled);
        assert_eq!(failure.code, "cancelled");
    }

    #[test]
    fn cancellation_requested_before_native_start_is_preserved_and_removed_on_drop() {
        let operation_id = 880_001;
        assert!(cancel_similarity_operation(operation_id));
        let (token, operation) = operation_token(operation_id).unwrap();
        assert!(token.is_cancelled());
        drop(operation);
        assert!(
            !similarity_tokens()
                .lock()
                .unwrap()
                .contains_key(&operation_id)
        );
    }
}
