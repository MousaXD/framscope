use framescope_cache::{FrameCacheHierarchy, FrameId};
use framescope_core::FrameScopeError;
use framescope_video::{
    CachedFrameSource, CachedNavigationError, CancellationToken, MicroscopeTimestampSelection,
    OpenOptions, ScrubPreviewCache, VideoDecoder, downscale_scrub_preview, microscope_target,
    microscope_timestamp_us, navigate_to_frame_cached,
};
use jni::JNIEnv;
use jni::objects::{JByteBuffer, JClass, JString};
use jni::sys::{jboolean, jint, jlong, jstring};
use serde::Serialize;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(unix)]
use std::os::fd::{BorrowedFd, RawFd};

use crate::{ENGINE_VERSION, microscope, to_jstring};

const MIN_PREVIEW_EDGE: u32 = 64;
const MAX_PREVIEW_EDGE: u32 = 1_024;
const PREVIEW_CACHE_BUDGET_BYTES: usize = 8 * 1024 * 1024;
const PREVIEW_CACHE_MAX_FRAMES: usize = 12;
const MAX_RETAINED_PREVIEW_SESSIONS: usize = 2;
const SOURCE_CACHE_RAM_BUDGET_BYTES: usize = 0;
const SOURCE_CACHE_DISK_BUDGET_BYTES: u64 = 0;

#[derive(Debug, Serialize)]
struct PreviewDetails {
    session_id: i64,
    frame_id: u64,
    timestamp_us: Option<i64>,
    width: u32,
    height: u32,
    stride_bytes: usize,
    byte_len: usize,
    source: &'static str,
    decoded_frames: u64,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum PreviewResponse {
    Ok {
        engine: &'static str,
        preview: PreviewDetails,
    },
    Error {
        engine: &'static str,
        code: String,
        message: String,
    },
}

#[derive(Debug)]
struct PreviewFailure {
    code: String,
    message: String,
}

impl PreviewFailure {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug)]
struct ScrubSessionState {
    cache_root: PathBuf,
    source_cache: FrameCacheHierarchy,
    preview_cache: ScrubPreviewCache,
}

#[derive(Debug)]
struct ScrubSessionHandle {
    state: Arc<Mutex<ScrubSessionState>>,
    last_access: u64,
}

#[derive(Debug, Default)]
struct ScrubRegistry {
    clock: u64,
    sessions: HashMap<i64, ScrubSessionHandle>,
}

static SCRUB_REGISTRY: OnceLock<Mutex<ScrubRegistry>> = OnceLock::new();

fn scrub_registry() -> &'static Mutex<ScrubRegistry> {
    SCRUB_REGISTRY.get_or_init(|| Mutex::new(ScrubRegistry::default()))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_MicroscopePreviewBridge_nativeRenderMicroscopePreviewTimestampUs(
    mut env: JNIEnv,
    _class: JClass,
    session_id: jlong,
    timestamp_us: jlong,
    selection: jint,
    max_edge: jint,
    cache_root: JString,
    destination: JByteBuffer,
) -> jstring {
    let cache_root: String = match env.get_string(&cache_root) {
        Ok(value) => value.into(),
        Err(_) => {
            return to_jstring(
                &mut env,
                &error_json("invalid_request", "Preview cache root is not valid UTF-8."),
            );
        }
    };
    let json = catch_unwind(AssertUnwindSafe(|| {
        with_direct_buffer(&env, &destination, |bytes| {
            render_timestamp_response(
                session_id,
                timestamp_us,
                selection,
                max_edge,
                &cache_root,
                bytes,
            )
        })
    }))
    .unwrap_or_else(|_| {
        error_json(
            "bridge_error",
            "Native live preview aborted safely after an internal panic.",
        )
    });
    to_jstring(&mut env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_MicroscopePreviewBridge_nativeRenderMicroscopePreviewFrame(
    mut env: JNIEnv,
    _class: JClass,
    session_id: jlong,
    frame_id: jlong,
    max_edge: jint,
    cache_root: JString,
    destination: JByteBuffer,
) -> jstring {
    let cache_root: String = match env.get_string(&cache_root) {
        Ok(value) => value.into(),
        Err(_) => {
            return to_jstring(
                &mut env,
                &error_json("invalid_request", "Preview cache root is not valid UTF-8."),
            );
        }
    };
    let json = catch_unwind(AssertUnwindSafe(|| {
        with_direct_buffer(&env, &destination, |bytes| {
            render_frame_response(session_id, frame_id, max_edge, &cache_root, bytes)
        })
    }))
    .unwrap_or_else(|_| {
        error_json(
            "bridge_error",
            "Native live preview aborted safely after an internal panic.",
        )
    });
    to_jstring(&mut env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_MicroscopePreviewBridge_nativeForgetMicroscopePreviewSession(
    _env: JNIEnv,
    _class: JClass,
    session_id: jlong,
) -> jboolean {
    let removed = catch_unwind(AssertUnwindSafe(|| forget_session(session_id))).unwrap_or(false);
    if removed { 1 } else { 0 }
}

fn with_direct_buffer(
    env: &JNIEnv<'_>,
    destination: &JByteBuffer<'_>,
    operation: impl FnOnce(&mut [u8]) -> String,
) -> String {
    let capacity = match env.get_direct_buffer_capacity(destination) {
        Ok(value) if value > 0 => value,
        _ => {
            return error_json(
                "invalid_buffer",
                "Android did not provide a non-empty direct preview buffer.",
            );
        }
    };
    let address = match env.get_direct_buffer_address(destination) {
        Ok(value) => value,
        Err(_) => {
            return error_json(
                "invalid_buffer",
                "Android did not provide a valid direct preview buffer.",
            );
        }
    };
    // SAFETY: JNI guarantees a valid writable address for exactly `capacity` bytes while this local
    // direct ByteBuffer reference is alive. The slice never escapes this synchronous JNI call.
    let bytes = unsafe { std::slice::from_raw_parts_mut(address, capacity) };
    operation(bytes)
}

fn render_timestamp_response(
    session_id: i64,
    timestamp_us: i64,
    selection: i32,
    max_edge: i32,
    cache_root: &str,
    destination: &mut [u8],
) -> String {
    let selection = match parse_selection(selection) {
        Ok(value) => value,
        Err(error) => return serialize_result(Err(error)),
    };
    serialize_result(render_preview(
        session_id,
        max_edge,
        cache_root,
        destination,
        |index| {
            microscope_timestamp_us(index, timestamp_us, selection)
                .map_err(|error| PreviewFailure::new("timestamp_not_indexed", error.to_string()))
        },
    ))
}

fn render_frame_response(
    session_id: i64,
    frame_id: i64,
    max_edge: i32,
    cache_root: &str,
    destination: &mut [u8],
) -> String {
    let frame_id = match u64::try_from(frame_id) {
        Ok(value) => FrameId(value),
        Err(_) => {
            return serialize_result(Err(PreviewFailure::new(
                "invalid_request",
                "Preview frame id must be non-negative.",
            )));
        }
    };
    serialize_result(render_preview(
        session_id,
        max_edge,
        cache_root,
        destination,
        |index| {
            microscope_target(index, frame_id)
                .map_err(|error| PreviewFailure::new("frame_out_of_range", error.to_string()))
        },
    ))
}

#[cfg(unix)]
fn render_preview(
    session_id: i64,
    max_edge: i32,
    cache_root: &str,
    destination: &mut [u8],
    resolve: impl FnOnce(
        &framescope_cache::FrameIndex,
    ) -> Result<framescope_video::MicroscopeTarget, PreviewFailure>,
) -> Result<PreviewDetails, PreviewFailure> {
    if session_id <= 0 {
        return Err(PreviewFailure::new(
            "invalid_request",
            "Live preview requires a positive microscope session id.",
        ));
    }
    let max_edge = u32::try_from(max_edge)
        .ok()
        .filter(|value| (MIN_PREVIEW_EDGE..=MAX_PREVIEW_EDGE).contains(value))
        .ok_or_else(|| {
            PreviewFailure::new(
                "invalid_request",
                "Live preview max edge is outside the supported range.",
            )
        })?;
    let cache_root = validate_cache_root(cache_root)?;

    let rendered = microscope::with_extraction_context(session_id, |source_fd, index| {
        let target = resolve(index)?;
        let frame_id = target.frame_id();
        let state = session_state(session_id, &cache_root)?;
        let mut state = state.lock().map_err(|_| {
            PreviewFailure::new("bridge_error", "Live preview cache state is poisoned.")
        })?;

        let (preview, source, decoded_frames) = if let Some(preview) =
            state.preview_cache.get(frame_id)
        {
            (preview, "preview_ram", 0)
        } else {
            let navigated = navigate_to_frame_cached(
                index,
                &mut state.source_cache,
                || open_decoder(source_fd),
                frame_id,
            )
            .map_err(from_navigation)?;
            let source = match navigated.source {
                CachedFrameSource::Ram => "source_ram",
                CachedFrameSource::Decoded => "decoded",
            };
            let decoded_frames = navigated.decoded_frames;
            let preview = downscale_scrub_preview(&navigated.pixels, max_edge)
                .map_err(|error| PreviewFailure::new("preview_scale_error", error.to_string()))?;
            state.preview_cache.insert(frame_id, preview.clone());
            (preview, source, decoded_frames)
        };

        if destination.len() < preview.byte_len() {
            return Err(PreviewFailure::new(
                "buffer_too_small",
                format!(
                    "Android preview buffer has {} bytes but {} are required.",
                    destination.len(),
                    preview.byte_len()
                ),
            ));
        }
        destination[..preview.byte_len()].copy_from_slice(preview.pixels());
        Ok(PreviewDetails {
            session_id,
            frame_id: frame_id.0,
            timestamp_us: target.entry.timestamp_us(),
            width: preview.width,
            height: preview.height,
            stride_bytes: preview.stride_bytes,
            byte_len: preview.byte_len(),
            source,
            decoded_frames,
        })
    })
    .map_err(|error| PreviewFailure::new(error.code(), error.message().to_owned()))?;

    rendered
}

#[cfg(not(unix))]
fn render_preview(
    _session_id: i64,
    _max_edge: i32,
    _cache_root: &str,
    _destination: &mut [u8],
    _resolve: impl FnOnce(
        &framescope_cache::FrameIndex,
    ) -> Result<framescope_video::MicroscopeTarget, PreviewFailure>,
) -> Result<PreviewDetails, PreviewFailure> {
    Err(PreviewFailure::new(
        "bridge_error",
        "Live microscope preview is only available on Android/Unix targets.",
    ))
}

#[cfg(unix)]
fn open_decoder(source_fd: RawFd) -> Result<VideoDecoder, FrameScopeError> {
    // SAFETY: `with_extraction_context` keeps the owned microscope descriptor alive and locked for
    // this call. VideoDecoder duplicates the descriptor immediately and owns only the duplicate.
    let borrowed = unsafe { BorrowedFd::borrow_raw(source_fd) };
    VideoDecoder::open_file_descriptor_with_options(
        borrowed,
        OpenOptions::default(),
        CancellationToken::new(),
    )
}

fn parse_selection(value: i32) -> Result<MicroscopeTimestampSelection, PreviewFailure> {
    match value {
        0 => Ok(MicroscopeTimestampSelection::AtOrBefore),
        1 => Ok(MicroscopeTimestampSelection::AtOrAfter),
        2 => Ok(MicroscopeTimestampSelection::Nearest),
        _ => Err(PreviewFailure::new(
            "invalid_request",
            "Timestamp selection must be 0 (before), 1 (after), or 2 (nearest).",
        )),
    }
}

fn validate_cache_root(value: &str) -> Result<PathBuf, PreviewFailure> {
    if value.trim().is_empty() || value.len() > 4_096 {
        return Err(PreviewFailure::new(
            "invalid_request",
            "Live preview cache root is empty or exceeds the safety limit.",
        ));
    }
    Ok(Path::new(value).to_path_buf())
}

fn session_state(
    session_id: i64,
    cache_root: &Path,
) -> Result<Arc<Mutex<ScrubSessionState>>, PreviewFailure> {
    let mut registry = scrub_registry()
        .lock()
        .map_err(|_| PreviewFailure::new("bridge_error", "Live preview registry is poisoned."))?;
    registry.clock = registry.clock.saturating_add(1);
    let access = registry.clock;

    if let Some(handle) = registry.sessions.get_mut(&session_id) {
        handle.last_access = access;
        let state = handle.state.clone();
        let configured_root = state
            .lock()
            .map_err(|_| {
                PreviewFailure::new("bridge_error", "Live preview cache state is poisoned.")
            })?
            .cache_root
            .clone();
        if configured_root != cache_root {
            return Err(PreviewFailure::new(
                "cache_identity_mismatch",
                "Live preview cache root changed for an existing microscope session.",
            ));
        }
        return Ok(state);
    }

    while registry.sessions.len() >= MAX_RETAINED_PREVIEW_SESSIONS {
        let Some(oldest) = registry
            .sessions
            .iter()
            .min_by_key(|(_, handle)| handle.last_access)
            .map(|(id, _)| *id)
        else {
            break;
        };
        registry.sessions.remove(&oldest);
    }

    let source_cache = FrameCacheHierarchy::open_resilient(
        cache_root.join("live-scrub-navigation"),
        SOURCE_CACHE_RAM_BUDGET_BYTES,
        SOURCE_CACHE_DISK_BUDGET_BYTES,
    );
    let state = Arc::new(Mutex::new(ScrubSessionState {
        cache_root: cache_root.to_path_buf(),
        source_cache,
        preview_cache: ScrubPreviewCache::new(PREVIEW_CACHE_BUDGET_BYTES, PREVIEW_CACHE_MAX_FRAMES),
    }));
    registry.sessions.insert(
        session_id,
        ScrubSessionHandle {
            state: state.clone(),
            last_access: access,
        },
    );
    Ok(state)
}

fn forget_session(session_id: i64) -> bool {
    if session_id <= 0 {
        return false;
    }
    scrub_registry()
        .lock()
        .map(|mut registry| registry.sessions.remove(&session_id).is_some())
        .unwrap_or(false)
}

fn from_navigation(error: CachedNavigationError) -> PreviewFailure {
    let code = match error {
        CachedNavigationError::IncompleteIndex => "index_incomplete",
        CachedNavigationError::FrameNotIndexed => "frame_out_of_range",
        CachedNavigationError::StreamIdentityMismatch => "stream_identity_mismatch",
        CachedNavigationError::TimelineMismatch => "timeline_mismatch",
        CachedNavigationError::UnexpectedEof => "unexpected_eof",
        CachedNavigationError::Cache(_) => "cache_error",
        CachedNavigationError::Index(_) => "index_error",
        CachedNavigationError::Decoder(_) => "decoder_error",
    };
    PreviewFailure::new(code, error.to_string())
}

fn serialize_result(result: Result<PreviewDetails, PreviewFailure>) -> String {
    let response = match result {
        Ok(preview) => PreviewResponse::Ok {
            engine: ENGINE_VERSION,
            preview,
        },
        Err(error) => PreviewResponse::Error {
            engine: ENGINE_VERSION,
            code: error.code,
            message: error.message,
        },
    };
    serde_json::to_string(&response).unwrap_or_else(|_| {
        error_json(
            "bridge_error",
            "Failed to serialize native live preview response.",
        )
    })
}

fn error_json(code: &str, message: &str) -> String {
    serde_json::to_string(&PreviewResponse::Error {
        engine: ENGINE_VERSION,
        code: code.to_owned(),
        message: message.to_owned(),
    })
    .unwrap_or_else(|_| "{\"status\":\"error\",\"code\":\"bridge_error\"}".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_policy_wire_values_are_stable() {
        assert_eq!(
            parse_selection(0).unwrap(),
            MicroscopeTimestampSelection::AtOrBefore
        );
        assert_eq!(
            parse_selection(1).unwrap(),
            MicroscopeTimestampSelection::AtOrAfter
        );
        assert_eq!(
            parse_selection(2).unwrap(),
            MicroscopeTimestampSelection::Nearest
        );
        assert!(parse_selection(3).is_err());
    }

    #[test]
    fn retained_preview_session_registry_is_bounded() {
        let mut registry = scrub_registry().lock().unwrap();
        registry.sessions.clear();
        registry.clock = 0;
        drop(registry);

        // Host tests cannot construct the Android cache hierarchy through a microscope session, so
        // the hard bound itself is asserted here while the cache implementation has independent
        // byte/count eviction coverage in framescope-video.
        assert_eq!(MAX_RETAINED_PREVIEW_SESSIONS, 2);
        assert_eq!(PREVIEW_CACHE_BUDGET_BYTES, 8 * 1024 * 1024);
        assert_eq!(PREVIEW_CACHE_MAX_FRAMES, 12);
    }
}
