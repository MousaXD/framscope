use framescope_cache::{
    FRAME_INDEX_SCHEMA_VERSION, FrameCacheHierarchy, FrameId, FrameIndex, FrameIndexError,
    FrameIndexOpenDisposition, FrameIndexStreamIdentity, SourceIdentity,
};
use framescope_core::FrameScopeError;
use framescope_video::{
    CachedNavigationError, CancellationToken, IndexingError, IndexingOptions, IndexingReport,
    MicroscopeFramePresentation, MicroscopeNavigationError, MicroscopePresentationError,
    MicroscopeStep, MicroscopeTarget, MicroscopeTimestampSelection, OpenOptions, VideoDecoder,
    build_or_resume_frame_index, microscope_step, microscope_target, microscope_timestamp_us,
    present_microscope_frame,
};
use serde::Serialize;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

#[cfg(unix)]
use std::io::{Read, Seek, SeekFrom};
#[cfg(unix)]
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};

use super::{ENGINE_VERSION, OperationId, operation_token};

const MAX_MICROSCOPE_SESSIONS: usize = 4;
const MICROSCOPE_RAM_CACHE_BUDGET_BYTES: usize = 64 * 1024 * 1024;
const MICROSCOPE_DISK_CACHE_BUDGET_BYTES: u64 = 0;
const MAX_PRESENTATION_RGBA_BYTES: usize = 256 * 1024 * 1024;
static NEXT_SESSION_ID: AtomicI64 = AtomicI64::new(1);
static MICROSCOPE_SESSIONS: OnceLock<Mutex<HashMap<i64, Arc<Mutex<NavigationSession>>>>> =
    OnceLock::new();

#[derive(Debug, Serialize)]
struct FrameDetails {
    frame_id: u64,
    timestamp_ticks: Option<i64>,
    timestamp_us: Option<i64>,
    time_base_numerator: i32,
    time_base_denominator: i32,
    duration_ticks: Option<i64>,
    keyframe: bool,
    corrupt: bool,
}

#[derive(Debug, Clone, Serialize)]
struct IndexingRuntimeDiagnostics {
    reused_existing_frames: u64,
    newly_indexed_frames: u64,
    restarted_after_partial_mismatch: bool,
    max_pending_entries: usize,
    total_elapsed_us: u64,
    index_status_elapsed_us: u64,
    decoder_open_elapsed_us: u64,
    decoder_open_count: u64,
    frames_decoded: u64,
    decoder_next_frame_elapsed_us: u64,
    frame_metadata_elapsed_us: u64,
    validation_frames_replayed: u64,
    reconciliation_sqlite_elapsed_us: u64,
    reconciliation_range_queries: u64,
    sqlite_batch_elapsed_us: u64,
    sqlite_transaction_begin_elapsed_us: u64,
    sqlite_commit_elapsed_us: u64,
    sqlite_rows_inserted: u64,
    batch_commits: u64,
    progress_observer_elapsed_us: u64,
    pipeline_queue_idle_elapsed_us: u64,
    pipeline_enabled: bool,
    decoded_frames_per_second_milli: u64,
    new_frames_per_second_milli: u64,
    miscellaneous_elapsed_us: u64,
    bounded_resume_attempted: bool,
    bounded_resume_succeeded: bool,
    bounded_resume_fell_back: bool,
    resume_checkpoint_frame_id: Option<u64>,
    resume_seek_scan_frames: u64,
}

impl From<&IndexingReport> for IndexingRuntimeDiagnostics {
    fn from(report: &IndexingReport) -> Self {
        Self {
            reused_existing_frames: report.reused_existing_frames,
            newly_indexed_frames: report.newly_indexed_frames,
            restarted_after_partial_mismatch: report.restarted_after_partial_mismatch,
            max_pending_entries: report.max_pending_entries,
            total_elapsed_us: report.total_elapsed_us,
            index_status_elapsed_us: report.index_status_elapsed_us,
            decoder_open_elapsed_us: report.decoder_open_elapsed_us,
            decoder_open_count: report.decoder_open_count,
            frames_decoded: report.frames_decoded,
            decoder_next_frame_elapsed_us: report.decoder_next_frame_elapsed_us,
            frame_metadata_elapsed_us: report.frame_metadata_elapsed_us,
            validation_frames_replayed: report.validation_frames_replayed,
            reconciliation_sqlite_elapsed_us: report.reconciliation_sqlite_elapsed_us,
            reconciliation_range_queries: report.reconciliation_range_queries,
            sqlite_batch_elapsed_us: report.sqlite_batch_elapsed_us,
            sqlite_transaction_begin_elapsed_us: report.sqlite_transaction_begin_elapsed_us,
            sqlite_commit_elapsed_us: report.sqlite_commit_elapsed_us,
            sqlite_rows_inserted: report.sqlite_rows_inserted,
            batch_commits: report.batch_commits,
            progress_observer_elapsed_us: report.progress_observer_elapsed_us,
            pipeline_queue_idle_elapsed_us: report.pipeline_queue_idle_elapsed_us,
            pipeline_enabled: report.pipeline_enabled,
            decoded_frames_per_second_milli: report.decoded_frames_per_second_milli,
            new_frames_per_second_milli: report.new_frames_per_second_milli,
            miscellaneous_elapsed_us: report.miscellaneous_elapsed_us,
            bounded_resume_attempted: report.bounded_resume_attempted,
            bounded_resume_succeeded: report.bounded_resume_succeeded,
            bounded_resume_fell_back: report.bounded_resume_fell_back,
            resume_checkpoint_frame_id: report
                .resume_checkpoint_frame_id
                .map(|frame_id| frame_id.0),
            resume_seek_scan_frames: report.resume_seek_scan_frames,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct MicroscopeOpenDiagnostics {
    source_seekable: bool,
    source_size_bytes: Option<u64>,
    source_identity_bytes_read: u64,
    source_identity_read_calls: u64,
    source_identity_seek_calls: u64,
    source_identity_io_elapsed_us: u64,
    source_identity_elapsed_us: u64,
    source_reuse_safe: bool,
    persistent_index: bool,
    probe_open_elapsed_us: u64,
    index_open_elapsed_us: u64,
    index_open_disposition: &'static str,
    database_bytes: u64,
    wal_bytes: u64,
    total_open_elapsed_us: u64,
    indexing: IndexingRuntimeDiagnostics,
}

#[derive(Debug, Clone, Copy)]
struct SourceIdentityMetrics {
    seekable: bool,
    size_bytes: Option<u64>,
    bytes_read: u64,
    read_calls: u64,
    seek_calls: u64,
    io_elapsed_us: u64,
    elapsed_us: u64,
}

#[cfg(unix)]
#[derive(Debug)]
struct SourceIdentityOutcome {
    identity: SourceIdentity,
    metrics: SourceIdentityMetrics,
}

#[derive(Debug, Serialize)]
struct SessionSnapshot {
    session_id: i64,
    frame_count: u64,
    current_frame: Option<FrameDetails>,
    can_step_previous: bool,
    can_step_next: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    open_diagnostics: Option<MicroscopeOpenDiagnostics>,
}

#[derive(Debug, Serialize)]
struct PreparedFrameDetails {
    session_id: i64,
    frame_id: u64,
    generation: i64,
    width: u32,
    height: u32,
    stride_bytes: usize,
    byte_len: usize,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum MicroscopeResponse {
    Ok {
        engine: &'static str,
        session: Box<SessionSnapshot>,
    },
    Error {
        engine: &'static str,
        code: &'static str,
        message: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum PreparedFrameResponse {
    Ok {
        engine: &'static str,
        frame: PreparedFrameDetails,
    },
    Error {
        engine: &'static str,
        code: &'static str,
        message: String,
    },
}

#[derive(Debug)]
pub(crate) struct MicroscopeFailure {
    code: &'static str,
    message: String,
}

impl MicroscopeFailure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub(crate) fn code(&self) -> &'static str {
        self.code
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct EphemeralIndexCleanup {
    database_path: PathBuf,
}

#[cfg(unix)]
impl EphemeralIndexCleanup {
    fn new(database_path: PathBuf) -> Self {
        Self { database_path }
    }

    fn cleanup(&self) {
        remove_if_present(&self.database_path);
        remove_if_present(&sqlite_sidecar_path(&self.database_path, "-wal"));
        remove_if_present(&sqlite_sidecar_path(&self.database_path, "-shm"));
        if let Some(namespace) = self.database_path.parent() {
            let _ = std::fs::remove_dir(namespace);
        }
    }
}

#[cfg(unix)]
impl Drop for EphemeralIndexCleanup {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[cfg(unix)]
struct NavigationSession {
    _source_fd: OwnedFd,
    index: FrameIndex,
    // Declared after `index` so the SQLite connection is dropped before cleanup removes the files.
    _ephemeral_index_cleanup: Option<EphemeralIndexCleanup>,
    frame_cache: FrameCacheHierarchy,
    frame_count: u64,
    current: Option<FrameId>,
    presentation_generation: i64,
    current_presentation: Option<MicroscopeFramePresentation>,
    open_diagnostics: MicroscopeOpenDiagnostics,
}

#[cfg(not(unix))]
struct NavigationSession;

type NavigationSessionHandle = Arc<Mutex<NavigationSession>>;

fn microscope_sessions() -> &'static Mutex<HashMap<i64, NavigationSessionHandle>> {
    MICROSCOPE_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn open_response(fd: i32, operation_id: OperationId, cache_root: &str) -> String {
    serialize_response(open_session(fd, operation_id, cache_root))
}

pub(crate) fn step_response(session_id: i64, delta: i32) -> String {
    serialize_response(step_session(session_id, delta))
}

pub(crate) fn jump_frame_response(session_id: i64, frame_id: i64) -> String {
    serialize_response(jump_to_frame(session_id, frame_id))
}

pub(crate) fn jump_timestamp_response(
    session_id: i64,
    timestamp_us: i64,
    selection: i32,
) -> String {
    serialize_response(jump_to_timestamp(session_id, timestamp_us, selection))
}

pub(crate) fn prepare_frame_response(session_id: i64) -> String {
    serialize_prepared_frame_response(prepare_current_frame(session_id))
}

pub(crate) fn close_session(session_id: i64) -> bool {
    if session_id <= 0 {
        return false;
    }
    microscope_sessions()
        .lock()
        .map(|mut sessions| sessions.remove(&session_id).is_some())
        .unwrap_or(false)
}

pub(crate) fn panic_response() -> String {
    serialize_response(Err(MicroscopeFailure::new(
        "bridge_error",
        "native microscope operation aborted safely after an internal panic",
    )))
}

pub(crate) fn panic_frame_response() -> String {
    serialize_prepared_frame_response(Err(MicroscopeFailure::new(
        "bridge_error",
        "native frame preparation aborted safely after an internal panic",
    )))
}

fn serialize_response(result: Result<SessionSnapshot, MicroscopeFailure>) -> String {
    let response = match result {
        Ok(session) => MicroscopeResponse::Ok {
            engine: ENGINE_VERSION,
            session: Box::new(session),
        },
        Err(error) => MicroscopeResponse::Error {
            engine: ENGINE_VERSION,
            code: error.code,
            message: error.message,
        },
    };
    serde_json::to_string(&response).unwrap_or_else(|_| {
        concat!(
            r#"{"status":"error","engine":"framescope-rust/unknown","code":"bridge_error","#,
            r#""message":"failed to serialize microscope response"}"#,
        )
        .into()
    })
}

fn serialize_prepared_frame_response(
    result: Result<PreparedFrameDetails, MicroscopeFailure>,
) -> String {
    let response = match result {
        Ok(frame) => PreparedFrameResponse::Ok {
            engine: ENGINE_VERSION,
            frame,
        },
        Err(error) => PreparedFrameResponse::Error {
            engine: ENGINE_VERSION,
            code: error.code,
            message: error.message,
        },
    };
    serde_json::to_string(&response).unwrap_or_else(|_| {
        concat!(
            r#"{"status":"error","engine":"framescope-rust/unknown","code":"bridge_error","#,
            r#""message":"failed to serialize frame preparation response"}"#,
        )
        .into()
    })
}

#[cfg(unix)]
fn open_session(
    fd: i32,
    operation_id: OperationId,
    cache_root: &str,
) -> Result<SessionSnapshot, MicroscopeFailure> {
    let open_started = Instant::now();
    if fd < 0 {
        return Err(MicroscopeFailure::new(
            "invalid_source",
            "invalid file descriptor",
        ));
    }
    if cache_root.trim().is_empty() || cache_root.len() > 4_096 {
        return Err(MicroscopeFailure::new(
            "invalid_request",
            "microscope cache root is empty or exceeds the safety limit",
        ));
    }
    ensure_session_capacity()?;

    let (cancellation, _operation) = operation_token(operation_id).map_err(from_frame_scope)?;
    let source_fd = duplicate_fd(fd).map_err(from_io)?;
    let identity_outcome = source_identity(source_fd.as_raw_fd(), &cancellation)?;
    let source_identity = identity_outcome.identity;
    let source_metrics = identity_outcome.metrics;
    let reusable_index = source_identity.is_reuse_safe();

    let probe_started = Instant::now();
    let probe = open_decoder(source_fd.as_fd(), cancellation.clone()).map_err(from_frame_scope)?;
    let probe_open_elapsed_us = elapsed_us(probe_started);
    let stream_identity =
        FrameIndexStreamIdentity::from_stream(probe.selected_stream()).map_err(from_index)?;
    drop(probe);

    let cache_root = Path::new(cache_root);
    let index_path = frame_index_path(cache_root, &source_identity, &stream_identity, operation_id);
    // Local variables drop in reverse declaration order. Declaring the cleanup guard before the
    // SQLite index ensures any error after opening the index first closes the connection, then
    // removes the operation-scoped database and sidecars.
    let ephemeral_index_cleanup =
        (!reusable_index).then(|| EphemeralIndexCleanup::new(index_path.clone()));
    let index_open_started = Instant::now();
    let (mut index, index_open_disposition) =
        FrameIndex::open_or_create(index_path.clone(), source_identity, stream_identity)
            .map_err(from_index)?;
    let index_open_elapsed_us = elapsed_us(index_open_started);

    let indexing_report = build_or_resume_frame_index(
        &mut index,
        || open_decoder(source_fd.as_fd(), cancellation.clone()),
        IndexingOptions::default(),
    )
    .map_err(from_indexing)?;

    let frame_count = index.frame_count().map_err(from_index)?.unwrap_or(0);
    let current = (frame_count > 0).then_some(FrameId::ZERO);
    let presentation_generation = if current.is_some() { 1 } else { 0 };
    let frame_cache = FrameCacheHierarchy::open_resilient(
        cache_root.join("microscope-frame-cache"),
        MICROSCOPE_RAM_CACHE_BUDGET_BYTES,
        MICROSCOPE_DISK_CACHE_BUDGET_BYTES,
    );
    let mut open_diagnostics = MicroscopeOpenDiagnostics {
        source_seekable: source_metrics.seekable,
        source_size_bytes: source_metrics.size_bytes,
        source_identity_bytes_read: source_metrics.bytes_read,
        source_identity_read_calls: source_metrics.read_calls,
        source_identity_seek_calls: source_metrics.seek_calls,
        source_identity_io_elapsed_us: source_metrics.io_elapsed_us,
        source_identity_elapsed_us: source_metrics.elapsed_us,
        source_reuse_safe: reusable_index,
        persistent_index: reusable_index,
        probe_open_elapsed_us,
        index_open_elapsed_us,
        index_open_disposition: index_open_disposition_name(index_open_disposition),
        database_bytes: file_len_or_zero(&index_path),
        wal_bytes: file_len_or_zero(&sqlite_sidecar_path(&index_path, "-wal")),
        total_open_elapsed_us: 0,
        indexing: IndexingRuntimeDiagnostics::from(&indexing_report),
    };
    open_diagnostics.total_open_elapsed_us = elapsed_us(open_started);

    let session_id = next_session_id()?;
    let session = NavigationSession {
        _source_fd: source_fd,
        index,
        _ephemeral_index_cleanup: ephemeral_index_cleanup,
        frame_cache,
        frame_count,
        current,
        presentation_generation,
        current_presentation: None,
        open_diagnostics,
    };
    let snapshot = snapshot(session_id, &session)?;

    let mut sessions = microscope_sessions().lock().map_err(|_| {
        MicroscopeFailure::new("bridge_error", "microscope session state is poisoned")
    })?;
    if sessions.len() >= MAX_MICROSCOPE_SESSIONS {
        return Err(MicroscopeFailure::new(
            "too_many_sessions",
            "too many microscope sessions are already open",
        ));
    }
    sessions.insert(session_id, Arc::new(Mutex::new(session)));
    Ok(snapshot)
}

#[cfg(not(unix))]
fn open_session(
    _fd: i32,
    _operation_id: OperationId,
    _cache_root: &str,
) -> Result<SessionSnapshot, MicroscopeFailure> {
    Err(MicroscopeFailure::new(
        "bridge_error",
        "file-descriptor microscope sessions are only available on Android/Unix targets",
    ))
}

#[cfg(unix)]
fn open_decoder(
    fd: std::os::fd::BorrowedFd<'_>,
    cancellation: CancellationToken,
) -> Result<VideoDecoder, FrameScopeError> {
    VideoDecoder::open_file_descriptor_with_options(fd, OpenOptions::default(), cancellation)
}

fn ensure_session_capacity() -> Result<(), MicroscopeFailure> {
    let sessions = microscope_sessions().lock().map_err(|_| {
        MicroscopeFailure::new("bridge_error", "microscope session state is poisoned")
    })?;
    if sessions.len() >= MAX_MICROSCOPE_SESSIONS {
        Err(MicroscopeFailure::new(
            "too_many_sessions",
            "too many microscope sessions are already open",
        ))
    } else {
        Ok(())
    }
}

fn next_session_id() -> Result<i64, MicroscopeFailure> {
    NEXT_SESSION_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1).filter(|next| *next > 0)
        })
        .map_err(|_| {
            MicroscopeFailure::new("bridge_error", "microscope session id space exhausted")
        })
}

fn next_presentation_generation(current: i64) -> Result<i64, MicroscopeFailure> {
    current
        .checked_add(1)
        .filter(|next| *next > 0)
        .ok_or_else(|| {
            MicroscopeFailure::new("bridge_error", "frame presentation generation exhausted")
        })
}

#[cfg(unix)]
fn set_current_frame(
    session: &mut NavigationSession,
    frame_id: FrameId,
) -> Result<(), MicroscopeFailure> {
    if session.current != Some(frame_id) {
        session.current = Some(frame_id);
        session.current_presentation = None;
        session.presentation_generation =
            next_presentation_generation(session.presentation_generation)?;
    }
    Ok(())
}

#[cfg(unix)]
fn step_session(session_id: i64, delta: i32) -> Result<SessionSnapshot, MicroscopeFailure> {
    let step = match delta {
        -1 => MicroscopeStep::Previous,
        1 => MicroscopeStep::Next,
        _ => {
            return Err(MicroscopeFailure::new(
                "invalid_request",
                "frame step must be exactly -1 or +1",
            ));
        }
    };
    with_session_mut(session_id, |session| {
        let current = session.current.ok_or_else(|| {
            MicroscopeFailure::new("no_frames", "video contains no indexed frames")
        })?;
        let target = microscope_step(&session.index, current, step).map_err(from_microscope)?;
        set_current_frame(session, target.frame_id())?;
        snapshot(session_id, session)
    })
}

#[cfg(not(unix))]
fn step_session(_session_id: i64, _delta: i32) -> Result<SessionSnapshot, MicroscopeFailure> {
    Err(MicroscopeFailure::new(
        "bridge_error",
        "microscope sessions are unavailable on this platform",
    ))
}

#[cfg(unix)]
fn jump_to_frame(session_id: i64, frame_id: i64) -> Result<SessionSnapshot, MicroscopeFailure> {
    let requested = u64::try_from(frame_id)
        .map_err(|_| MicroscopeFailure::new("invalid_request", "frame id must be non-negative"))?;
    with_session_mut(session_id, |session| {
        let target =
            microscope_target(&session.index, FrameId(requested)).map_err(from_microscope)?;
        set_current_frame(session, target.frame_id())?;
        snapshot(session_id, session)
    })
}

#[cfg(not(unix))]
fn jump_to_frame(_session_id: i64, _frame_id: i64) -> Result<SessionSnapshot, MicroscopeFailure> {
    Err(MicroscopeFailure::new(
        "bridge_error",
        "microscope sessions are unavailable on this platform",
    ))
}

#[cfg(unix)]
fn jump_to_timestamp(
    session_id: i64,
    timestamp_us: i64,
    selection: i32,
) -> Result<SessionSnapshot, MicroscopeFailure> {
    let selection = match selection {
        0 => MicroscopeTimestampSelection::AtOrBefore,
        1 => MicroscopeTimestampSelection::AtOrAfter,
        2 => MicroscopeTimestampSelection::Nearest,
        _ => {
            return Err(MicroscopeFailure::new(
                "invalid_request",
                "timestamp selection must be 0 (before), 1 (after), or 2 (nearest)",
            ));
        }
    };
    with_session_mut(session_id, |session| {
        let target = microscope_timestamp_us(&session.index, timestamp_us, selection)
            .map_err(from_microscope)?;
        set_current_frame(session, target.frame_id())?;
        snapshot(session_id, session)
    })
}

#[cfg(not(unix))]
fn jump_to_timestamp(
    _session_id: i64,
    _timestamp_us: i64,
    _selection: i32,
) -> Result<SessionSnapshot, MicroscopeFailure> {
    Err(MicroscopeFailure::new(
        "bridge_error",
        "microscope sessions are unavailable on this platform",
    ))
}

#[cfg(unix)]
fn prepare_current_frame(session_id: i64) -> Result<PreparedFrameDetails, MicroscopeFailure> {
    with_session_mut(session_id, |session| {
        let frame_id = session.current.ok_or_else(|| {
            MicroscopeFailure::new("no_frames", "video contains no indexed frames")
        })?;
        let source_fd = session._source_fd.as_fd();
        let presentation = present_microscope_frame(
            &session.index,
            &mut session.frame_cache,
            || open_decoder(source_fd, CancellationToken::new()),
            frame_id,
        )
        .map_err(from_presentation)?;
        if presentation.pixels.byte_len() > MAX_PRESENTATION_RGBA_BYTES {
            return Err(MicroscopeFailure::new(
                "frame_too_large",
                "decoded frame exceeds the Android presentation byte limit",
            ));
        }
        let details = PreparedFrameDetails {
            session_id,
            frame_id: presentation.frame_id().0,
            generation: session.presentation_generation,
            width: presentation.pixels.width,
            height: presentation.pixels.height,
            stride_bytes: presentation.pixels.stride_bytes,
            byte_len: presentation.pixels.byte_len(),
        };
        session.current_presentation = Some(presentation);
        Ok(details)
    })
}

#[cfg(not(unix))]
fn prepare_current_frame(_session_id: i64) -> Result<PreparedFrameDetails, MicroscopeFailure> {
    Err(MicroscopeFailure::new(
        "bridge_error",
        "microscope frame presentation is unavailable on this platform",
    ))
}

#[cfg(unix)]
pub(crate) fn copy_prepared_frame(
    session_id: i64,
    generation: i64,
    destination: &mut [u8],
) -> Result<usize, MicroscopeFailure> {
    if generation <= 0 {
        return Err(MicroscopeFailure::new(
            "invalid_request",
            "frame presentation generation must be positive",
        ));
    }
    with_session_mut(session_id, |session| {
        if generation != session.presentation_generation {
            return Err(MicroscopeFailure::new(
                "stale_generation",
                "requested frame presentation is stale after navigation",
            ));
        }
        let presentation = session.current_presentation.as_ref().ok_or_else(|| {
            MicroscopeFailure::new(
                "no_prepared_frame",
                "no source-quality frame has been prepared for the current microscope position",
            )
        })?;
        if session.current != Some(presentation.frame_id()) {
            return Err(MicroscopeFailure::new(
                "stale_generation",
                "prepared frame no longer matches the current microscope position",
            ));
        }
        let pixels = presentation.pixels.pixels();
        if destination.len() < pixels.len() {
            return Err(MicroscopeFailure::new(
                "buffer_too_small",
                "direct frame buffer is smaller than the prepared RGBA payload",
            ));
        }
        destination[..pixels.len()].copy_from_slice(pixels);
        Ok(pixels.len())
    })
}

#[cfg(not(unix))]
pub(crate) fn copy_prepared_frame(
    _session_id: i64,
    _generation: i64,
    _destination: &mut [u8],
) -> Result<usize, MicroscopeFailure> {
    Err(MicroscopeFailure::new(
        "bridge_error",
        "microscope frame presentation is unavailable on this platform",
    ))
}

/// Execute a synchronous read-only extraction operation against an open microscope session.
///
/// The per-session mutex remains held for the duration of `operation`, so navigation cannot mutate
/// the authoritative frame index or replace the source identity while a batch is decoding. The raw
/// descriptor is borrowed only for this call and must not be retained by the operation after it
/// returns. Decoder constructors are expected to duplicate it immediately, as the video engine does.
#[cfg(unix)]
pub(crate) fn with_extraction_context<T, E>(
    session_id: i64,
    operation: impl FnOnce(RawFd, &FrameIndex) -> Result<T, E>,
) -> Result<Result<T, E>, MicroscopeFailure> {
    with_session_mut(session_id, |session| {
        Ok(operation(session._source_fd.as_raw_fd(), &session.index))
    })
}

#[cfg(not(unix))]
pub(crate) fn with_extraction_context<T, E>(
    _session_id: i64,
    _operation: impl FnOnce(i32, &FrameIndex) -> Result<T, E>,
) -> Result<Result<T, E>, MicroscopeFailure> {
    Err(MicroscopeFailure::new(
        "bridge_error",
        "microscope extraction sessions are unavailable on this platform",
    ))
}

#[cfg(unix)]
fn with_session_mut<T>(
    session_id: i64,
    operation: impl FnOnce(&mut NavigationSession) -> Result<T, MicroscopeFailure>,
) -> Result<T, MicroscopeFailure> {
    if session_id <= 0 {
        return Err(MicroscopeFailure::new(
            "invalid_request",
            "microscope session id must be positive",
        ));
    }

    // Clone the per-session handle while holding the registry briefly, then release the global lock
    // before taking the session lock. Future FFmpeg decode/render work may be expensive, and must
    // never serialize unrelated sessions or block open/close operations behind a global mutex.
    let handle = {
        let sessions = microscope_sessions().lock().map_err(|_| {
            MicroscopeFailure::new("bridge_error", "microscope session state is poisoned")
        })?;
        sessions.get(&session_id).cloned().ok_or_else(|| {
            MicroscopeFailure::new("session_not_found", "microscope session is not open")
        })?
    };
    let mut session = handle.lock().map_err(|_| {
        MicroscopeFailure::new("bridge_error", "microscope session state is poisoned")
    })?;
    operation(&mut session)
}

#[cfg(unix)]
fn snapshot(
    session_id: i64,
    session: &NavigationSession,
) -> Result<SessionSnapshot, MicroscopeFailure> {
    let current_target = session
        .current
        .map(|frame_id| microscope_target(&session.index, frame_id).map_err(from_microscope))
        .transpose()?;
    let current_frame = current_target
        .as_ref()
        .map(|target| frame_details(&session.index, target));
    Ok(SessionSnapshot {
        session_id,
        frame_count: session.frame_count,
        current_frame,
        can_step_previous: current_target
            .as_ref()
            .is_some_and(MicroscopeTarget::has_previous),
        can_step_next: current_target
            .as_ref()
            .is_some_and(MicroscopeTarget::has_next),
        open_diagnostics: Some(session.open_diagnostics.clone()),
    })
}

#[cfg(unix)]
fn frame_details(index: &FrameIndex, target: &MicroscopeTarget) -> FrameDetails {
    let entry = &target.entry;
    let time_base = index.stream_identity().time_base;
    FrameDetails {
        frame_id: entry.frame_id.0,
        timestamp_ticks: entry.presentation_timestamp.map(|value| value.ticks),
        timestamp_us: entry.timestamp_us(),
        time_base_numerator: time_base.numerator,
        time_base_denominator: time_base.denominator,
        duration_ticks: entry.duration.map(|value| value.ticks),
        keyframe: entry.keyframe,
        corrupt: entry.corrupt,
    }
}

fn frame_index_path(
    cache_root: &Path,
    source: &SourceIdentity,
    stream: &FrameIndexStreamIdentity,
    operation_id: OperationId,
) -> PathBuf {
    let source_namespace = if source.is_reuse_safe() {
        source.stable_key()
    } else {
        format!("unverifiable-{}-op-{operation_id}", source.stable_key())
    };
    cache_root
        .join("frame-index")
        .join(format!("v{FRAME_INDEX_SCHEMA_VERSION}"))
        .join(source_namespace)
        .join(format!("stream-{}.sqlite3", stream.stream_index))
}

fn index_open_disposition_name(disposition: FrameIndexOpenDisposition) -> &'static str {
    match disposition {
        FrameIndexOpenDisposition::Created => "created",
        FrameIndexOpenDisposition::Reused => "reused",
        FrameIndexOpenDisposition::RebuiltStaleSource => "rebuilt_stale_source",
        FrameIndexOpenDisposition::RebuiltUnverifiableSource => "rebuilt_unverifiable_source",
        FrameIndexOpenDisposition::RebuiltIncompatibleTimelineContract => {
            "rebuilt_incompatible_timeline_contract"
        }
        FrameIndexOpenDisposition::RecoveredCorruptState => "recovered_corrupt_state",
        FrameIndexOpenDisposition::RecreatedUnsupportedSchema => "recreated_unsupported_schema",
    }
}

fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

#[cfg(unix)]
fn file_len_or_zero(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

#[cfg(unix)]
fn sqlite_sidecar_path(database_path: &Path, suffix: &str) -> PathBuf {
    let mut path = database_path.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
}

#[cfg(unix)]
fn remove_if_present(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

fn from_frame_scope(error: FrameScopeError) -> MicroscopeFailure {
    MicroscopeFailure::new(error.code(), error.to_string())
}

fn from_index(error: FrameIndexError) -> MicroscopeFailure {
    MicroscopeFailure::new("index_error", error.to_string())
}

fn from_indexing(error: IndexingError) -> MicroscopeFailure {
    match error {
        IndexingError::Decoder(error) => from_frame_scope(error),
        other => MicroscopeFailure::new("indexing_error", other.to_string()),
    }
}

fn from_microscope(error: MicroscopeNavigationError) -> MicroscopeFailure {
    let code = match error {
        MicroscopeNavigationError::AtFirstFrame | MicroscopeNavigationError::AtLastFrame => {
            "frame_boundary"
        }
        MicroscopeNavigationError::FrameNotIndexed => "frame_out_of_range",
        MicroscopeNavigationError::TimestampNotIndexed => "timestamp_not_indexed",
        MicroscopeNavigationError::IncompleteIndex => "index_incomplete",
        MicroscopeNavigationError::EmptyIndex => "no_frames",
        MicroscopeNavigationError::Index(_) | MicroscopeNavigationError::TimestampOverflow => {
            "index_error"
        }
    };
    MicroscopeFailure::new(code, error.to_string())
}

fn from_cached_navigation(error: CachedNavigationError) -> MicroscopeFailure {
    match error {
        CachedNavigationError::Decoder(error) => from_frame_scope(error),
        CachedNavigationError::Index(error) => from_index(error),
        CachedNavigationError::Cache(error) => {
            MicroscopeFailure::new("cache_error", error.to_string())
        }
        CachedNavigationError::IncompleteIndex => {
            MicroscopeFailure::new("index_incomplete", "frame index is incomplete")
        }
        CachedNavigationError::FrameNotIndexed => {
            MicroscopeFailure::new("frame_out_of_range", "frame is not indexed")
        }
        CachedNavigationError::StreamIdentityMismatch => MicroscopeFailure::new(
            "stream_identity_mismatch",
            "fresh decoder stream does not match the indexed stream",
        ),
        CachedNavigationError::TimelineMismatch => MicroscopeFailure::new(
            "timeline_mismatch",
            "decoded presentation timeline diverged from the persistent frame index",
        ),
        CachedNavigationError::UnexpectedEof => MicroscopeFailure::new(
            "unexpected_eof",
            "decoder reached EOF before the requested indexed frame",
        ),
    }
}

fn from_presentation(error: MicroscopePresentationError) -> MicroscopeFailure {
    match error {
        MicroscopePresentationError::Target(error) => from_microscope(error),
        MicroscopePresentationError::Navigation(error) => from_cached_navigation(error),
        MicroscopePresentationError::IdentityMismatch => MicroscopeFailure::new(
            "presentation_identity_mismatch",
            "microscope identity diverged from the source-quality frame presentation",
        ),
    }
}

fn from_io(error: io::Error) -> MicroscopeFailure {
    MicroscopeFailure::new("io_error", error.to_string())
}

#[cfg(unix)]
fn duplicate_fd(fd: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: fcntl duplicates a valid caller-owned descriptor and returns a new descriptor that
    // this function immediately wraps in OwnedFd. The caller's descriptor is never closed here.
    let duplicated = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicated < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `duplicated` is a fresh descriptor returned by F_DUPFD_CLOEXEC and ownership is
    // transferred exactly once into OwnedFd.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicated) })
}

#[cfg(unix)]
fn descriptor_seekable(fd: RawFd) -> bool {
    // SAFETY: SEEK_CUR with offset zero only queries the descriptor's current offset. It neither
    // changes the caller-visible offset nor takes ownership. ESPIPE cleanly identifies pipes and
    // other non-seekable descriptors.
    unsafe { libc::lseek(fd, 0, libc::SEEK_CUR) >= 0 }
}

#[cfg(unix)]
fn source_identity(
    fd: RawFd,
    cancellation: &CancellationToken,
) -> Result<SourceIdentityOutcome, MicroscopeFailure> {
    let started = Instant::now();
    let seekable = descriptor_seekable(fd);
    let mut reader = match FdLogicalReader::new(fd) {
        Ok(reader) => reader,
        Err(_) => {
            return Ok(SourceIdentityOutcome {
                identity: SourceIdentity::metadata_only(None, None, None),
                metrics: SourceIdentityMetrics {
                    seekable: false,
                    size_bytes: None,
                    bytes_read: 0,
                    read_calls: 0,
                    seek_calls: 0,
                    io_elapsed_us: 0,
                    elapsed_us: elapsed_us(started),
                },
            });
        }
    };

    let size_bytes = seekable.then_some(reader.len);
    if !seekable || reader.len == 0 {
        return Ok(SourceIdentityOutcome {
            identity: SourceIdentity::metadata_only(size_bytes, None, None),
            metrics: SourceIdentityMetrics {
                seekable,
                size_bytes,
                bytes_read: 0,
                read_calls: 0,
                seek_calls: 0,
                io_elapsed_us: 0,
                elapsed_us: elapsed_us(started),
            },
        });
    }

    let size = reader.len;
    let identity_result =
        SourceIdentity::from_seekable_cancellable(&mut reader, None, None, || {
            if cancellation.is_cancelled() {
                Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "source identity hashing cancelled",
                ))
            } else {
                Ok(())
            }
        });
    let metrics = SourceIdentityMetrics {
        seekable,
        size_bytes: Some(size),
        bytes_read: reader.bytes_read,
        read_calls: reader.read_calls,
        seek_calls: reader.seek_calls,
        io_elapsed_us: reader.io_elapsed_us,
        elapsed_us: elapsed_us(started),
    };

    match identity_result {
        Ok(identity) => Ok(SourceIdentityOutcome { identity, metrics }),
        Err(_) if cancellation.is_cancelled() => Err(from_frame_scope(FrameScopeError::Cancelled)),
        Err(_) => Ok(SourceIdentityOutcome {
            identity: SourceIdentity::metadata_only(Some(size), None, None),
            metrics,
        }),
    }
}

#[cfg(unix)]
struct FdLogicalReader {
    fd: RawFd,
    position: u64,
    len: u64,
    bytes_read: u64,
    read_calls: u64,
    seek_calls: u64,
    io_elapsed_us: u64,
}

#[cfg(unix)]
impl FdLogicalReader {
    fn new(fd: RawFd) -> io::Result<Self> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `stat` points to writable storage and fstat only writes it when successful.
        if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fstat succeeded, so the stat structure is initialized.
        let stat = unsafe { stat.assume_init() };
        let len = u64::try_from(stat.st_size)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "negative source size"))?;
        Ok(Self {
            fd,
            position: 0,
            len,
            bytes_read: 0,
            read_calls: 0,
            seek_calls: 0,
            io_elapsed_us: 0,
        })
    }
}

#[cfg(unix)]
impl Read for FdLogicalReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || self.position >= self.len {
            return Ok(0);
        }
        let remaining = self.len - self.position;
        let count = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let offset = libc::off_t::try_from(self.position)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source offset overflow"))?;
        self.read_calls = self.read_calls.saturating_add(1);
        let read_started = Instant::now();
        // SAFETY: `buffer` is valid for `count` writable bytes. pread does not mutate the shared
        // file offset, which is essential for borrowed SAF descriptors and decoder duplicates.
        let read = unsafe {
            libc::pread(
                self.fd,
                buffer.as_mut_ptr().cast::<libc::c_void>(),
                count,
                offset,
            )
        };
        self.io_elapsed_us = self.io_elapsed_us.saturating_add(elapsed_us(read_started));
        if read < 0 {
            return Err(io::Error::last_os_error());
        }
        let read = usize::try_from(read)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "negative read length"))?;
        self.position = self.position.checked_add(read as u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "source position overflow")
        })?;
        self.bytes_read = self.bytes_read.saturating_add(read as u64);
        Ok(read)
    }
}

#[cfg(unix)]
impl Seek for FdLogicalReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = match position {
            SeekFrom::Start(value) => i128::from(value),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.len) + i128::from(delta),
        };
        if !(0..=i128::from(u64::MAX)).contains(&next) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek would leave the supported source range",
            ));
        }
        self.seek_calls = self.seek_calls.saturating_add(1);
        self.position = u64::try_from(next)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source seek overflow"))?;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_core::TimeBase;

    fn test_stream_identity() -> FrameIndexStreamIdentity {
        FrameIndexStreamIdentity {
            stream_index: 0,
            codec_id: 27,
            codec_name: "h264".into(),
            time_base: TimeBase::new(1, 1_000).unwrap(),
            width: Some(640),
            height: Some(360),
        }
    }

    #[test]
    fn indexing_runtime_diagnostics_serialize_complete_accounting_contract() {
        let diagnostics = IndexingRuntimeDiagnostics {
            reused_existing_frames: 1,
            newly_indexed_frames: 2,
            restarted_after_partial_mismatch: false,
            max_pending_entries: 3,
            total_elapsed_us: 4,
            index_status_elapsed_us: 5,
            decoder_open_elapsed_us: 6,
            decoder_open_count: 7,
            frames_decoded: 8,
            decoder_next_frame_elapsed_us: 9,
            frame_metadata_elapsed_us: 10,
            validation_frames_replayed: 11,
            reconciliation_sqlite_elapsed_us: 12,
            reconciliation_range_queries: 13,
            sqlite_batch_elapsed_us: 14,
            sqlite_transaction_begin_elapsed_us: 15,
            sqlite_commit_elapsed_us: 16,
            sqlite_rows_inserted: 17,
            batch_commits: 18,
            progress_observer_elapsed_us: 19,
            pipeline_queue_idle_elapsed_us: 20,
            pipeline_enabled: false,
            decoded_frames_per_second_milli: 21,
            new_frames_per_second_milli: 22,
            miscellaneous_elapsed_us: 23,
            bounded_resume_attempted: true,
            bounded_resume_succeeded: true,
            bounded_resume_fell_back: false,
            resume_checkpoint_frame_id: Some(24),
            resume_seek_scan_frames: 25,
        };
        let value = serde_json::to_value(diagnostics).expect("diagnostics serialize");
        for key in [
            "total_elapsed_us",
            "decoder_next_frame_elapsed_us",
            "frame_metadata_elapsed_us",
            "reconciliation_sqlite_elapsed_us",
            "sqlite_batch_elapsed_us",
            "sqlite_transaction_begin_elapsed_us",
            "sqlite_commit_elapsed_us",
            "sqlite_rows_inserted",
            "progress_observer_elapsed_us",
            "pipeline_queue_idle_elapsed_us",
            "pipeline_enabled",
            "decoded_frames_per_second_milli",
            "new_frames_per_second_milli",
            "miscellaneous_elapsed_us",
        ] {
            assert!(value.get(key).is_some(), "missing accounting field {key}");
        }
    }

    #[test]
    fn unverifiable_sources_use_operation_isolated_index_paths() {
        let source = SourceIdentity::metadata_only(Some(4_096), None, None);
        let stream = test_stream_identity();
        let root = Path::new("cache-root");
        assert_ne!(
            frame_index_path(root, &source, &stream, 11),
            frame_index_path(root, &source, &stream, 12)
        );
    }

    #[test]
    fn reusable_sources_keep_stable_index_paths_across_operations() {
        let source = SourceIdentity::new(4_096, None, Some("strong-content-tag".into()));
        let stream = test_stream_identity();
        let root = Path::new("cache-root");
        assert_eq!(
            frame_index_path(root, &source, &stream, 11),
            frame_index_path(root, &source, &stream, 12)
        );
    }

    #[test]
    fn presentation_generation_advances_and_rejects_overflow() {
        assert_eq!(next_presentation_generation(1).unwrap(), 2);
        assert!(next_presentation_generation(i64::MAX).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn ephemeral_cleanup_removes_database_sidecars_and_namespace() {
        let root = std::env::temp_dir().join(format!(
            "framescope-ephemeral-index-cleanup-{}",
            std::process::id()
        ));
        let namespace = root.join("unverifiable-op-1");
        std::fs::create_dir_all(&namespace).unwrap();
        let database = namespace.join("stream-0.sqlite3");
        let wal = sqlite_sidecar_path(&database, "-wal");
        let shm = sqlite_sidecar_path(&database, "-shm");
        std::fs::write(&database, b"db").unwrap();
        std::fs::write(&wal, b"wal").unwrap();
        std::fs::write(&shm, b"shm").unwrap();

        {
            let _cleanup = EphemeralIndexCleanup::new(database.clone());
        }

        assert!(!database.exists());
        assert!(!wal.exists());
        assert!(!shm.exists());
        assert!(!namespace.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn pread_identity_reader_does_not_move_caller_offset() {
        use std::fs::{File, OpenOptions};
        use std::io::{Read as _, Seek as _, Write as _};

        let path = std::env::temp_dir().join(format!(
            "framescope-microscope-pread-{}",
            std::process::id()
        ));
        {
            let mut file = File::create(&path).unwrap();
            file.write_all(b"0123456789abcdef").unwrap();
        }
        let mut file = OpenOptions::new().read(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(7)).unwrap();
        let mut reader = FdLogicalReader::new(file.as_raw_fd()).unwrap();
        let mut bytes = [0_u8; 4];
        reader.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"0123");
        assert_eq!(reader.bytes_read, 4);
        assert_eq!(reader.read_calls, 1);
        assert_eq!(file.stream_position().unwrap(), 7);
        let _ = std::fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn source_identity_reports_whole_source_io_and_reuse_safety() {
        use std::fs::File;
        use std::io::Write as _;

        let path = std::env::temp_dir().join(format!(
            "framescope-microscope-identity-metrics-{}",
            std::process::id()
        ));
        let payload = vec![0x2a_u8; 768 * 1024 + 17];
        let mut file = File::create(&path).unwrap();
        file.write_all(&payload).unwrap();
        drop(file);
        let file = File::open(&path).unwrap();
        let cancellation = CancellationToken::new();

        let outcome = source_identity(file.as_raw_fd(), &cancellation).unwrap();
        assert!(outcome.identity.is_reuse_safe());
        assert!(outcome.metrics.seekable);
        assert_eq!(outcome.metrics.size_bytes, Some(payload.len() as u64));
        assert_eq!(outcome.metrics.bytes_read, payload.len() as u64);
        assert!(outcome.metrics.read_calls >= 3);
        assert!(outcome.metrics.seek_calls >= 2);
        let _ = std::fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn zero_size_seekable_source_is_reported_but_never_reuse_safe() {
        use std::fs::File;

        let path = std::env::temp_dir().join(format!(
            "framescope-microscope-zero-size-{}",
            std::process::id()
        ));
        File::create(&path).unwrap();
        let file = File::open(&path).unwrap();
        let outcome = source_identity(file.as_raw_fd(), &CancellationToken::new()).unwrap();
        assert!(outcome.metrics.seekable);
        assert_eq!(outcome.metrics.size_bytes, Some(0));
        assert_eq!(outcome.metrics.bytes_read, 0);
        assert!(!outcome.identity.is_reuse_safe());
        let _ = std::fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn non_seekable_pipe_is_ephemeral_without_claiming_source_size() {
        let mut descriptors = [0_i32; 2];
        // SAFETY: `descriptors` provides storage for the two descriptors returned by pipe.
        assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
        // SAFETY: pipe returned two fresh descriptors and ownership is transferred exactly once.
        let read_fd = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
        // SAFETY: as above for the write end.
        let _write_fd = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };

        let outcome = source_identity(read_fd.as_raw_fd(), &CancellationToken::new()).unwrap();
        assert!(!outcome.metrics.seekable);
        assert_eq!(outcome.metrics.size_bytes, None);
        assert_eq!(outcome.metrics.bytes_read, 0);
        assert!(!outcome.identity.is_reuse_safe());
    }

    #[cfg(unix)]
    #[test]
    fn source_identity_propagates_cancellation_instead_of_downgrading_reuse() {
        use std::fs::File;
        use std::io::Write as _;

        let path = std::env::temp_dir().join(format!(
            "framescope-microscope-identity-cancel-{}",
            std::process::id()
        ));
        let mut file = File::create(&path).unwrap();
        file.write_all(&vec![0x55_u8; 512 * 1024]).unwrap();
        drop(file);
        let file = File::open(&path).unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = source_identity(file.as_raw_fd(), &cancellation).unwrap_err();
        assert_eq!(error.code(), FrameScopeError::Cancelled.code());
        let _ = std::fs::remove_file(path);
    }
}
