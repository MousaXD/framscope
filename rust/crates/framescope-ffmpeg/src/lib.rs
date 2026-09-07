//! Narrow FFmpeg ownership and ABI boundary.
//!
//! The public video-engine policy lives in `framescope-video`. This crate owns the unavoidable
//! native lifetime boundary so format/codec contexts, packets, frames, AVIO buffers, and duplicated
//! file descriptors are released in one place on every error path.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkedVersions {
    pub avcodec: u32,
    pub avformat: u32,
    pub avutil: u32,
    pub swscale: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeErrorKind {
    UnsupportedFormat,
    UnsupportedCodec,
    InvalidSource,
    NoVideoStream,
    Decoder,
    Malformed,
    Io,
    Cancelled,
    SeekUnavailable,
    Backend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeError {
    pub kind: NativeErrorKind,
    pub ffmpeg_code: i32,
    pub message: String,
}

impl NativeError {
    fn backend(message: impl Into<String>) -> Self {
        Self {
            kind: NativeErrorKind::Backend,
            ffmpeg_code: 0,
            message: message.into(),
        }
    }
}

/// Cloneable, one-shot cancellation signal shared with FFmpeg's interrupt callback.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    inner: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeContainerInfo {
    pub format_name: String,
    pub format_long_name: Option<String>,
    pub duration_us: Option<i64>,
    pub stream_count: u32,
    pub selected_stream_index: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeStreamInfo {
    pub index: i32,
    pub media_type: i32,
    pub codec_id: i32,
    pub decoder_available: bool,
    pub is_default: bool,
    pub codec_name: String,
    pub time_base_num: i32,
    pub time_base_den: i32,
    pub duration_ticks: Option<i64>,
    pub frame_count: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub pixel_format: Option<i32>,
    pub pixel_format_name: Option<String>,
    pub average_rate_num: i32,
    pub average_rate_den: i32,
    pub nominal_rate_num: i32,
    pub nominal_rate_den: i32,
    pub rotation_degrees: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeFrame {
    pub epoch: u64,
    pub index: u64,
    pub stream_index: i32,
    pub timestamp_ticks: Option<i64>,
    pub duration_ticks: Option<i64>,
    pub time_base_num: i32,
    pub time_base_den: i32,
    pub keyframe: bool,
    pub corrupt: bool,
    pub width: i32,
    pub height: i32,
    pub pixel_format: Option<i32>,
    pub pixel_format_name: Option<String>,
}

/// Owned RGBA snapshot of the most recently decoded native frame.
///
/// The pixel buffer has no lifetime relationship with FFmpeg after this value is returned. This is
/// the boundary Phase 3 cache code can safely retain; raw AVFrame pointers never leave the native
/// session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRgbaFrame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub pixels: Vec<u8>,
}

#[cfg(framescope_ffmpeg_native)]
mod native {
    use super::*;
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::ptr::NonNull;

    #[cfg(unix)]
    use std::os::unix::ffi::OsStrExt;

    const ERROR_MESSAGE_CAPACITY: usize = 256;
    const FORMAT_NAME_CAPACITY: usize = 128;
    const FORMAT_LONG_NAME_CAPACITY: usize = 256;
    const CODEC_NAME_CAPACITY: usize = 64;
    const PIXEL_FORMAT_CAPACITY: usize = 64;
    const AV_NOPTS_VALUE: i64 = i64::MIN;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct FsError {
        kind: i32,
        ffmpeg_code: i32,
        message: [c_char; ERROR_MESSAGE_CAPACITY],
    }

    impl Default for FsError {
        fn default() -> Self {
            Self {
                kind: 0,
                ffmpeg_code: 0,
                message: [0; ERROR_MESSAGE_CAPACITY],
            }
        }
    }

    #[repr(C)]
    struct FsContainerInfo {
        format_name: [c_char; FORMAT_NAME_CAPACITY],
        format_long_name: [c_char; FORMAT_LONG_NAME_CAPACITY],
        duration_us: i64,
        stream_count: u32,
        selected_stream_index: i32,
    }

    impl Default for FsContainerInfo {
        fn default() -> Self {
            Self {
                format_name: [0; FORMAT_NAME_CAPACITY],
                format_long_name: [0; FORMAT_LONG_NAME_CAPACITY],
                duration_us: -1,
                stream_count: 0,
                selected_stream_index: -1,
            }
        }
    }

    #[repr(C)]
    struct FsStreamInfo {
        index: i32,
        media_type: i32,
        codec_id: i32,
        decoder_available: i32,
        is_default: i32,
        codec_name: [c_char; CODEC_NAME_CAPACITY],
        time_base_num: i32,
        time_base_den: i32,
        duration_ticks: i64,
        frame_count: i64,
        width: i32,
        height: i32,
        pixel_format: i32,
        pixel_format_name: [c_char; PIXEL_FORMAT_CAPACITY],
        average_rate_num: i32,
        average_rate_den: i32,
        nominal_rate_num: i32,
        nominal_rate_den: i32,
        has_rotation: i32,
        rotation_degrees: i32,
    }

    impl Default for FsStreamInfo {
        fn default() -> Self {
            Self {
                index: -1,
                media_type: 0,
                codec_id: 0,
                decoder_available: 0,
                is_default: 0,
                codec_name: [0; CODEC_NAME_CAPACITY],
                time_base_num: 0,
                time_base_den: 0,
                duration_ticks: AV_NOPTS_VALUE,
                frame_count: 0,
                width: 0,
                height: 0,
                pixel_format: -1,
                pixel_format_name: [0; PIXEL_FORMAT_CAPACITY],
                average_rate_num: 0,
                average_rate_den: 0,
                nominal_rate_num: 0,
                nominal_rate_den: 0,
                has_rotation: 0,
                rotation_degrees: 0,
            }
        }
    }

    #[repr(C)]
    struct FsFrameInfo {
        epoch: u64,
        index: u64,
        stream_index: i32,
        has_timestamp: i32,
        timestamp_ticks: i64,
        has_duration: i32,
        duration_ticks: i64,
        time_base_num: i32,
        time_base_den: i32,
        keyframe: i32,
        corrupt: i32,
        width: i32,
        height: i32,
        pixel_format: i32,
        pixel_format_name: [c_char; PIXEL_FORMAT_CAPACITY],
    }

    impl Default for FsFrameInfo {
        fn default() -> Self {
            Self {
                epoch: 0,
                index: 0,
                stream_index: -1,
                has_timestamp: 0,
                timestamp_ticks: 0,
                has_duration: 0,
                duration_ticks: 0,
                time_base_num: 0,
                time_base_den: 0,
                keyframe: 0,
                corrupt: 0,
                width: 0,
                height: 0,
                pixel_format: -1,
                pixel_format_name: [0; PIXEL_FORMAT_CAPACITY],
            }
        }
    }

    enum FsSession {}

    type FsCancelFn = unsafe extern "C" fn(*mut c_void) -> i32;

    unsafe extern "C" {
        fn avcodec_version() -> u32;
        fn avformat_version() -> u32;
        fn avutil_version() -> u32;
        fn swscale_version() -> u32;

        fn framescope_ffmpeg_open_path(
            path: *const c_char,
            requested_stream: i32,
            cancel_fn: FsCancelFn,
            cancel_opaque: *mut c_void,
            error: *mut FsError,
        ) -> *mut FsSession;
        fn framescope_ffmpeg_open_fd(
            fd: i32,
            requested_stream: i32,
            cancel_fn: FsCancelFn,
            cancel_opaque: *mut c_void,
            error: *mut FsError,
        ) -> *mut FsSession;
        fn framescope_ffmpeg_close(session: *mut FsSession);
        fn framescope_ffmpeg_container_info(
            session: *mut FsSession,
            out: *mut FsContainerInfo,
            error: *mut FsError,
        ) -> i32;
        fn framescope_ffmpeg_stream_info(
            session: *mut FsSession,
            ordinal: u32,
            out: *mut FsStreamInfo,
            error: *mut FsError,
        ) -> i32;
        fn framescope_ffmpeg_next_frame(
            session: *mut FsSession,
            out: *mut FsFrameInfo,
            error: *mut FsError,
        ) -> i32;
        fn framescope_ffmpeg_copy_current_frame_rgba(
            session: *mut FsSession,
            output: *mut u8,
            output_capacity: usize,
            out_stride: *mut i32,
            error: *mut FsError,
        ) -> i32;
        fn framescope_ffmpeg_seek_us(
            session: *mut FsSession,
            timestamp_us: i64,
            error: *mut FsError,
        ) -> i32;
    }

    unsafe extern "C" fn cancellation_callback(opaque: *mut c_void) -> i32 {
        if opaque.is_null() {
            return 0;
        }
        // SAFETY: every native session stores an opaque pointer produced from an Arc<AtomicBool>.
        // NativeSession retains that Arc until after framescope_ffmpeg_close returns.
        let flag = unsafe { &*opaque.cast::<AtomicBool>() };
        i32::from(flag.load(Ordering::Acquire))
    }

    fn requested_stream_value(requested: Option<u32>) -> Result<i32, NativeError> {
        match requested {
            Some(index) => i32::try_from(index)
                .map_err(|_| NativeError::backend("requested stream index exceeds FFmpeg range")),
            None => Ok(-1),
        }
    }

    fn path_cstring(path: &Path) -> Result<CString, NativeError> {
        #[cfg(unix)]
        let bytes = path.as_os_str().as_bytes();
        #[cfg(not(unix))]
        let owned = path.to_string_lossy().into_owned();
        #[cfg(not(unix))]
        let bytes = owned.as_bytes();

        CString::new(bytes).map_err(|_| NativeError {
            kind: NativeErrorKind::InvalidSource,
            ffmpeg_code: 0,
            message: "video path contains an embedded NUL byte".into(),
        })
    }

    fn error_from_ffi(error: &FsError) -> NativeError {
        let kind = match error.kind {
            1 => NativeErrorKind::UnsupportedFormat,
            2 => NativeErrorKind::UnsupportedCodec,
            3 => NativeErrorKind::InvalidSource,
            4 => NativeErrorKind::NoVideoStream,
            5 => NativeErrorKind::Decoder,
            6 => NativeErrorKind::Malformed,
            7 => NativeErrorKind::Io,
            8 => NativeErrorKind::Cancelled,
            9 => NativeErrorKind::SeekUnavailable,
            _ => NativeErrorKind::Backend,
        };
        NativeError {
            kind,
            ffmpeg_code: error.ffmpeg_code,
            message: char_buffer_to_string(&error.message),
        }
    }

    fn char_buffer_to_string<const N: usize>(buffer: &[c_char; N]) -> String {
        let pointer = buffer.as_ptr();
        // SAFETY: all C shim text fields are always initialized and NUL-terminated by snprintf or
        // zero-fill, and their backing arrays remain alive for this call.
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    }

    fn optional_text<const N: usize>(buffer: &[c_char; N]) -> Option<String> {
        let value = char_buffer_to_string(buffer);
        (!value.is_empty()).then_some(value)
    }

    pub(super) fn linked_versions() -> LinkedVersions {
        // SAFETY: the four functions are argument-free version probes from the FFmpeg libraries
        // linked by build.rs. Reaching this cfg means the native shim and those libraries linked.
        unsafe {
            LinkedVersions {
                avcodec: avcodec_version(),
                avformat: avformat_version(),
                avutil: avutil_version(),
                swscale: swscale_version(),
            }
        }
    }

    pub struct Session {
        raw: NonNull<FsSession>,
        cancellation: CancellationToken,
        last_frame_dimensions: Option<(u32, u32)>,
    }

    // SAFETY: the native FFmpeg state is exclusively owned by Session and every operation requiring
    // mutation takes &mut self. It is never accessed concurrently through Session; cancellation is
    // the only cross-thread state and is an AtomicBool. Session is intentionally not Sync.
    unsafe impl Send for Session {}

    impl Session {
        pub fn open_path(
            path: &Path,
            requested_stream: Option<u32>,
            cancellation: CancellationToken,
        ) -> Result<Self, NativeError> {
            let requested_stream = requested_stream_value(requested_stream)?;
            let path = path_cstring(path)?;
            let mut error = FsError::default();
            let cancel_opaque = Arc::as_ptr(&cancellation.inner).cast_mut().cast::<c_void>();
            // SAFETY: path is NUL-terminated for the duration of the call. The cancellation pointer
            // stays valid because `cancellation` is retained by the returned Session, or remains
            // alive locally until a failed open has fully cleaned up the native session.
            let raw = unsafe {
                framescope_ffmpeg_open_path(
                    path.as_ptr(),
                    requested_stream,
                    cancellation_callback,
                    cancel_opaque,
                    &mut error,
                )
            };
            let raw = NonNull::new(raw).ok_or_else(|| error_from_ffi(&error))?;
            Ok(Self {
                raw,
                cancellation,
                last_frame_dimensions: None,
            })
        }

        #[cfg(unix)]
        pub fn open_fd(
            fd: std::os::fd::RawFd,
            requested_stream: Option<u32>,
            cancellation: CancellationToken,
        ) -> Result<Self, NativeError> {
            let requested_stream = requested_stream_value(requested_stream)?;
            let mut error = FsError::default();
            let cancel_opaque = Arc::as_ptr(&cancellation.inner).cast_mut().cast::<c_void>();
            // SAFETY: the C shim immediately duplicates `fd`, so ownership remains with the caller.
            // The cancellation Arc follows the same lifetime invariant as open_path.
            let raw = unsafe {
                framescope_ffmpeg_open_fd(
                    fd,
                    requested_stream,
                    cancellation_callback,
                    cancel_opaque,
                    &mut error,
                )
            };
            let raw = NonNull::new(raw).ok_or_else(|| error_from_ffi(&error))?;
            Ok(Self {
                raw,
                cancellation,
                last_frame_dimensions: None,
            })
        }

        pub fn container_info(&self) -> Result<NativeContainerInfo, NativeError> {
            let mut raw = FsContainerInfo::default();
            let mut error = FsError::default();
            // SAFETY: self.raw is live until Drop and `raw`/`error` are valid writable outputs.
            let result = unsafe {
                framescope_ffmpeg_container_info(self.raw.as_ptr(), &mut raw, &mut error)
            };
            if result < 0 {
                return Err(error_from_ffi(&error));
            }
            Ok(NativeContainerInfo {
                format_name: char_buffer_to_string(&raw.format_name),
                format_long_name: optional_text(&raw.format_long_name),
                duration_us: (raw.duration_us >= 0).then_some(raw.duration_us),
                stream_count: raw.stream_count,
                selected_stream_index: raw.selected_stream_index,
            })
        }

        pub fn stream_info(&self, ordinal: u32) -> Result<NativeStreamInfo, NativeError> {
            let mut raw = FsStreamInfo::default();
            let mut error = FsError::default();
            // SAFETY: self.raw is live and output storage is valid for this call.
            let result = unsafe {
                framescope_ffmpeg_stream_info(self.raw.as_ptr(), ordinal, &mut raw, &mut error)
            };
            if result < 0 {
                return Err(error_from_ffi(&error));
            }
            Ok(NativeStreamInfo {
                index: raw.index,
                media_type: raw.media_type,
                codec_id: raw.codec_id,
                decoder_available: raw.decoder_available != 0,
                is_default: raw.is_default != 0,
                codec_name: char_buffer_to_string(&raw.codec_name),
                time_base_num: raw.time_base_num,
                time_base_den: raw.time_base_den,
                duration_ticks: (raw.duration_ticks != AV_NOPTS_VALUE)
                    .then_some(raw.duration_ticks),
                frame_count: u64::try_from(raw.frame_count)
                    .ok()
                    .filter(|count| *count > 0),
                width: u32::try_from(raw.width).ok().filter(|value| *value > 0),
                height: u32::try_from(raw.height).ok().filter(|value| *value > 0),
                pixel_format: (raw.pixel_format >= 0).then_some(raw.pixel_format),
                pixel_format_name: optional_text(&raw.pixel_format_name),
                average_rate_num: raw.average_rate_num,
                average_rate_den: raw.average_rate_den,
                nominal_rate_num: raw.nominal_rate_num,
                nominal_rate_den: raw.nominal_rate_den,
                rotation_degrees: (raw.has_rotation != 0).then_some(raw.rotation_degrees),
            })
        }

        pub fn next_frame(&mut self) -> Result<Option<NativeFrame>, NativeError> {
            self.last_frame_dimensions = None;
            let mut raw = FsFrameInfo::default();
            let mut error = FsError::default();
            // SAFETY: &mut self guarantees exclusive access to the decoder state. Outputs are live.
            let result =
                unsafe { framescope_ffmpeg_next_frame(self.raw.as_ptr(), &mut raw, &mut error) };
            match result {
                1 => {
                    self.last_frame_dimensions = u32::try_from(raw.width)
                        .ok()
                        .filter(|width| *width > 0)
                        .zip(u32::try_from(raw.height).ok().filter(|height| *height > 0));
                    Ok(Some(NativeFrame {
                        epoch: raw.epoch,
                        index: raw.index,
                        stream_index: raw.stream_index,
                        timestamp_ticks: (raw.has_timestamp != 0).then_some(raw.timestamp_ticks),
                        duration_ticks: (raw.has_duration != 0).then_some(raw.duration_ticks),
                        time_base_num: raw.time_base_num,
                        time_base_den: raw.time_base_den,
                        keyframe: raw.keyframe != 0,
                        corrupt: raw.corrupt != 0,
                        width: raw.width,
                        height: raw.height,
                        pixel_format: (raw.pixel_format >= 0).then_some(raw.pixel_format),
                        pixel_format_name: optional_text(&raw.pixel_format_name),
                    }))
                }
                0 => Ok(None),
                -1 => Err(error_from_ffi(&error)),
                other => Err(NativeError::backend(format!(
                    "native decoder returned unexpected frame status {other}"
                ))),
            }
        }

        /// Copy the most recently decoded frame into owned tightly-packed RGBA bytes.
        ///
        /// This must be called before another `next_frame` or seek. The returned Vec owns its data
        /// and remains valid after the decoder advances or is dropped.
        pub fn copy_current_frame_rgba(&mut self) -> Result<NativeRgbaFrame, NativeError> {
            let (width, height) = self.last_frame_dimensions.ok_or_else(|| {
                NativeError::backend("no decoded frame is available for RGBA copy")
            })?;
            let stride = usize::try_from(width)
                .ok()
                .and_then(|value| value.checked_mul(4))
                .ok_or_else(|| NativeError::backend("decoded frame RGBA stride overflows usize"))?;
            let required = stride
                .checked_mul(
                    usize::try_from(height)
                        .map_err(|_| NativeError::backend("decoded frame height exceeds usize"))?,
                )
                .ok_or_else(|| NativeError::backend("decoded frame RGBA size overflows usize"))?;
            let mut pixels = Vec::new();
            pixels
                .try_reserve_exact(required)
                .map_err(|_| NativeError::backend("failed to allocate owned RGBA frame buffer"))?;
            pixels.resize(required, 0);

            let mut out_stride = 0_i32;
            let mut error = FsError::default();
            // SAFETY: pixels has exactly `required` initialized writable bytes and remains alive for
            // the call. The native shim validates current-frame state and output capacity before
            // conversion. &mut self prevents the reusable AVFrame from being advanced concurrently.
            let result = unsafe {
                framescope_ffmpeg_copy_current_frame_rgba(
                    self.raw.as_ptr(),
                    pixels.as_mut_ptr(),
                    pixels.len(),
                    &mut out_stride,
                    &mut error,
                )
            };
            if result < 0 {
                return Err(error_from_ffi(&error));
            }
            let out_stride = usize::try_from(out_stride)
                .ok()
                .filter(|value| *value == stride)
                .ok_or_else(|| {
                    NativeError::backend("native RGBA conversion returned an invalid stride")
                })?;

            Ok(NativeRgbaFrame {
                width,
                height,
                stride: out_stride,
                pixels,
            })
        }

        pub fn seek_us(&mut self, timestamp_us: i64) -> Result<(), NativeError> {
            self.last_frame_dimensions = None;
            let mut error = FsError::default();
            // SAFETY: &mut self guarantees exclusive access to format/decoder state.
            let result =
                unsafe { framescope_ffmpeg_seek_us(self.raw.as_ptr(), timestamp_us, &mut error) };
            if result < 0 {
                return Err(error_from_ffi(&error));
            }
            Ok(())
        }

        pub fn cancellation_token(&self) -> CancellationToken {
            self.cancellation.clone()
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            // SAFETY: raw is uniquely owned by this Session. The cancellation Arc is still alive
            // during close, so an FFmpeg interrupt callback cannot observe a dangling opaque ptr.
            unsafe { framescope_ffmpeg_close(self.raw.as_ptr()) };
        }
    }

    pub(super) fn c_int_sanity() {
        let _: c_int = 0;
    }
}

#[cfg(framescope_ffmpeg_native)]
pub use native::Session;

#[cfg(not(framescope_ffmpeg_native))]
pub struct Session;

#[cfg(not(framescope_ffmpeg_native))]
impl Session {
    pub fn open_path(
        _path: &Path,
        _requested_stream: Option<u32>,
        _cancellation: CancellationToken,
    ) -> Result<Self, NativeError> {
        Err(NativeError::backend(
            "FFmpeg backend is not linked; Android links the pinned build automatically and host tests require the system-ffmpeg feature",
        ))
    }

    #[cfg(unix)]
    pub fn open_fd(
        _fd: std::os::fd::RawFd,
        _requested_stream: Option<u32>,
        _cancellation: CancellationToken,
    ) -> Result<Self, NativeError> {
        Err(NativeError::backend(
            "FFmpeg backend is not linked; Android links the pinned build automatically and host tests require the system-ffmpeg feature",
        ))
    }

    pub fn container_info(&self) -> Result<NativeContainerInfo, NativeError> {
        Err(NativeError::backend("FFmpeg backend is not linked"))
    }

    pub fn stream_info(&self, _ordinal: u32) -> Result<NativeStreamInfo, NativeError> {
        Err(NativeError::backend("FFmpeg backend is not linked"))
    }

    pub fn next_frame(&mut self) -> Result<Option<NativeFrame>, NativeError> {
        Err(NativeError::backend("FFmpeg backend is not linked"))
    }

    pub fn copy_current_frame_rgba(&mut self) -> Result<NativeRgbaFrame, NativeError> {
        Err(NativeError::backend("FFmpeg backend is not linked"))
    }

    pub fn seek_us(&mut self, _timestamp_us: i64) -> Result<(), NativeError> {
        Err(NativeError::backend("FFmpeg backend is not linked"))
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        CancellationToken::new()
    }
}

/// Whether this build contains the FFmpeg decoder backend.
pub const fn backend_available() -> bool {
    cfg!(framescope_ffmpeg_native)
}

/// Return linked FFmpeg library version integers when the native backend is present.
pub fn linked_versions() -> Option<LinkedVersions> {
    #[cfg(framescope_ffmpeg_native)]
    {
        native::c_int_sanity();
        Some(native::linked_versions())
    }

    #[cfg(not(framescope_ffmpeg_native))]
    {
        None
    }
}

/// Force the final Android JNI library to retain references to all required FFmpeg libraries.
pub fn link_probe() -> u32 {
    linked_versions()
        .map(|versions| versions.avcodec ^ versions.avformat ^ versions.avutil ^ versions.swscale)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_token_is_cloneable_and_one_shot() {
        let token = CancellationToken::new();
        let clone = token.clone();
        assert!(!clone.is_cancelled());
        token.cancel();
        assert!(clone.is_cancelled());
    }

    #[cfg(not(framescope_ffmpeg_native))]
    #[test]
    fn host_build_does_not_require_ffmpeg_without_feature() {
        assert_eq!(linked_versions(), None);
        assert_eq!(link_probe(), 0);
        assert!(!backend_available());
    }
}
