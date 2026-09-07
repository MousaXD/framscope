use framescope_cache::{
    FRAME_INDEX_SCHEMA_VERSION, FrameId, FrameIndex, FrameIndexEntry, FrameIndexError,
    FrameIndexStreamIdentity, SourceIdentity,
};
use framescope_core::FrameScopeError;
use framescope_video::{
    CancellationToken, IndexingError, IndexingOptions, OpenOptions, VideoDecoder,
    build_or_resume_frame_index,
};
use serde::Serialize;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};

#[cfg(unix)]
use std::io::{Read, Seek, SeekFrom};
#[cfg(unix)]
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};

use super::{ENGINE_VERSION, OperationId, operation_token};

const MAX_MICROSCOPE_SESSIONS: usize = 4;
static NEXT_SESSION_ID: AtomicI64 = AtomicI64::new(1);
static MICROSCOPE_SESSIONS: OnceLock<Mutex<HashMap<i64, NavigationSession>>> = OnceLock::new();

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

#[derive(Debug, Serialize)]
struct SessionSnapshot {
    session_id: i64,
    frame_count: u64,
    current_frame: Option<FrameDetails>,
    can_step_previous: bool,
    can_step_next: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum MicroscopeResponse {
    Ok {
        engine: &'static str,
        session: SessionSnapshot,
    },
    Error {
        engine: &'static str,
        code: &'static str,
        message: String,
    },
}

#[derive(Debug)]
struct MicroscopeFailure {
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
}

#[cfg(unix)]
struct NavigationSession {
    _source_fd: OwnedFd,
    index: FrameIndex,
    frame_count: u64,
    current: Option<FrameId>,
}

#[cfg(not(unix))]
struct NavigationSession;

fn microscope_sessions() -> &'static Mutex<HashMap<i64, NavigationSession>> {
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

fn serialize_response(result: Result<SessionSnapshot, MicroscopeFailure>) -> String {
    let response = match result {
        Ok(session) => MicroscopeResponse::Ok {
            engine: ENGINE_VERSION,
            session,
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

#[cfg(unix)]
fn open_session(
    fd: i32,
    operation_id: OperationId,
    cache_root: &str,
) -> Result<SessionSnapshot, MicroscopeFailure> {
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
    let source_identity = source_identity(source_fd.as_raw_fd());

    let probe = open_decoder(source_fd.as_fd(), cancellation.clone()).map_err(from_frame_scope)?;
    let stream_identity =
        FrameIndexStreamIdentity::from_stream(probe.selected_stream()).map_err(from_index)?;
    drop(probe);

    let index_path = frame_index_path(Path::new(cache_root), &source_identity, &stream_identity);
    let (mut index, _) = FrameIndex::open_or_create(index_path, source_identity, stream_identity)
        .map_err(from_index)?;

    build_or_resume_frame_index(
        &mut index,
        || open_decoder(source_fd.as_fd(), cancellation.clone()),
        IndexingOptions::default(),
    )
    .map_err(from_indexing)?;

    let frame_count = index.frame_count().map_err(from_index)?.unwrap_or(0);
    let current = (frame_count > 0).then_some(FrameId::ZERO);
    let session_id = next_session_id()?;
    let session = NavigationSession {
        _source_fd: source_fd,
        index,
        frame_count,
        current,
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
    sessions.insert(session_id, session);
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

#[cfg(unix)]
fn step_session(session_id: i64, delta: i32) -> Result<SessionSnapshot, MicroscopeFailure> {
    if !matches!(delta, -1 | 1) {
        return Err(MicroscopeFailure::new(
            "invalid_request",
            "frame step must be exactly -1 or +1",
        ));
    }
    with_session_mut(session_id, |session| {
        let current = session.current.ok_or_else(|| {
            MicroscopeFailure::new("no_frames", "video contains no indexed frames")
        })?;
        session.current = Some(stepped_frame(current, session.frame_count, delta)?);
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
        if requested >= session.frame_count {
            return Err(MicroscopeFailure::new(
                "frame_out_of_range",
                "requested frame is outside the complete frame index",
            ));
        }
        session.current = Some(FrameId(requested));
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
    with_session_mut(session_id, |session| {
        let entry = match selection {
            0 => session
                .index
                .frame_at_or_before_us(timestamp_us)
                .map_err(from_index)?,
            1 => session
                .index
                .frame_at_or_after_us(timestamp_us)
                .map_err(from_index)?,
            2 => choose_nearest_us(
                timestamp_us,
                session
                    .index
                    .frame_at_or_before_us(timestamp_us)
                    .map_err(from_index)?,
                session
                    .index
                    .frame_at_or_after_us(timestamp_us)
                    .map_err(from_index)?,
            )?,
            _ => {
                return Err(MicroscopeFailure::new(
                    "invalid_request",
                    "timestamp selection must be 0 (before), 1 (after), or 2 (nearest)",
                ));
            }
        }
        .ok_or_else(|| {
            MicroscopeFailure::new(
                "timestamp_not_indexed",
                "timestamp does not resolve to an indexed frame",
            )
        })?;
        session.current = Some(entry.frame_id);
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
    let mut sessions = microscope_sessions().lock().map_err(|_| {
        MicroscopeFailure::new("bridge_error", "microscope session state is poisoned")
    })?;
    let session = sessions.get_mut(&session_id).ok_or_else(|| {
        MicroscopeFailure::new("session_not_found", "microscope session is not open")
    })?;
    operation(session)
}

#[cfg(unix)]
fn snapshot(
    session_id: i64,
    session: &NavigationSession,
) -> Result<SessionSnapshot, MicroscopeFailure> {
    let current_frame = session
        .current
        .map(|frame_id| frame_details(&session.index, frame_id))
        .transpose()?;
    let can_step_previous = session.current.is_some_and(|frame_id| frame_id.0 > 0);
    let can_step_next = session.current.is_some_and(|frame_id| {
        frame_id
            .0
            .checked_add(1)
            .is_some_and(|next| next < session.frame_count)
    });
    Ok(SessionSnapshot {
        session_id,
        frame_count: session.frame_count,
        current_frame,
        can_step_previous,
        can_step_next,
    })
}

#[cfg(unix)]
fn frame_details(index: &FrameIndex, frame_id: FrameId) -> Result<FrameDetails, MicroscopeFailure> {
    let entry = index
        .entry(frame_id)
        .map_err(from_index)?
        .ok_or_else(|| MicroscopeFailure::new("frame_out_of_range", "indexed frame is missing"))?;
    let time_base = index.stream_identity().time_base;
    Ok(FrameDetails {
        frame_id: entry.frame_id.0,
        timestamp_ticks: entry.presentation_timestamp.map(|value| value.ticks),
        timestamp_us: entry.timestamp_us(),
        time_base_numerator: time_base.numerator,
        time_base_denominator: time_base.denominator,
        duration_ticks: entry.duration.map(|value| value.ticks),
        keyframe: entry.keyframe,
        corrupt: entry.corrupt,
    })
}

fn stepped_frame(
    current: FrameId,
    frame_count: u64,
    delta: i32,
) -> Result<FrameId, MicroscopeFailure> {
    match delta {
        -1 if current.0 > 0 => Ok(FrameId(current.0 - 1)),
        1 if current
            .0
            .checked_add(1)
            .is_some_and(|next| next < frame_count) =>
        {
            Ok(FrameId(current.0 + 1))
        }
        -1 | 1 => Err(MicroscopeFailure::new(
            "frame_boundary",
            "cannot step beyond the indexed frame range",
        )),
        _ => Err(MicroscopeFailure::new(
            "invalid_request",
            "frame step must be exactly -1 or +1",
        )),
    }
}

fn choose_nearest_us(
    target_us: i64,
    before: Option<FrameIndexEntry>,
    after: Option<FrameIndexEntry>,
) -> Result<Option<FrameIndexEntry>, MicroscopeFailure> {
    match (before, after) {
        (None, None) => Ok(None),
        (Some(entry), None) | (None, Some(entry)) => Ok(Some(entry)),
        (Some(before), Some(after)) => {
            let before_us = before.timestamp_us().ok_or_else(|| {
                MicroscopeFailure::new(
                    "index_error",
                    "indexed timestamp cannot be converted to microseconds",
                )
            })?;
            let after_us = after.timestamp_us().ok_or_else(|| {
                MicroscopeFailure::new(
                    "index_error",
                    "indexed timestamp cannot be converted to microseconds",
                )
            })?;
            if after_us == target_us {
                return Ok(Some(after));
            }
            let before_distance = i128::from(target_us) - i128::from(before_us);
            let after_distance = i128::from(after_us) - i128::from(target_us);
            if before_distance <= after_distance {
                Ok(Some(before))
            } else {
                Ok(Some(after))
            }
        }
    }
}

fn frame_index_path(
    cache_root: &Path,
    source: &SourceIdentity,
    stream: &FrameIndexStreamIdentity,
) -> PathBuf {
    cache_root
        .join("frame-index")
        .join(format!("v{FRAME_INDEX_SCHEMA_VERSION}"))
        .join(source.stable_key())
        .join(format!("stream-{}.sqlite3", stream.stream_index))
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
fn source_identity(fd: RawFd) -> SourceIdentity {
    match FdLogicalReader::new(fd) {
        Ok(mut reader) if reader.len > 0 => {
            let size = reader.len;
            SourceIdentity::from_seekable(&mut reader, None, None)
                .unwrap_or_else(|_| SourceIdentity::metadata_only(Some(size), None, None))
        }
        Ok(reader) => SourceIdentity::metadata_only(Some(reader.len), None, None),
        Err(_) => SourceIdentity::metadata_only(None, None, None),
    }
}

#[cfg(unix)]
struct FdLogicalReader {
    fd: RawFd,
    position: u64,
    len: u64,
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
        if read < 0 {
            return Err(io::Error::last_os_error());
        }
        let read = usize::try_from(read)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "negative read length"))?;
        self.position = self.position.checked_add(read as u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "source position overflow")
        })?;
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
        self.position = u64::try_from(next)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source seek overflow"))?;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepping_never_crosses_index_boundaries() {
        assert_eq!(stepped_frame(FrameId(1), 3, -1).unwrap(), FrameId(0));
        assert_eq!(stepped_frame(FrameId(1), 3, 1).unwrap(), FrameId(2));
        assert_eq!(
            stepped_frame(FrameId(0), 3, -1).unwrap_err().code,
            "frame_boundary"
        );
        assert_eq!(
            stepped_frame(FrameId(2), 3, 1).unwrap_err().code,
            "frame_boundary"
        );
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
        assert_eq!(file.stream_position().unwrap(), 7);
        let _ = std::fs::remove_file(path);
    }
}
