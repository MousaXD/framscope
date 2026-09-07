use framescope_cache::OwnedRgbaFrame;
use framescope_extraction_image::{ExtractionImageFormat, ImageExportError, encode_frame};
use jni::JNIEnv;
use jni::objects::{JByteBuffer, JClass};
use jni::sys::{jint, jlong, jstring};
use serde::{Deserialize, Serialize};
use std::io;
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::panic::{AssertUnwindSafe, catch_unwind};

use super::{ENGINE_VERSION, microscope, operation_token, to_jstring};

const COPY_INVALID_REQUEST: jlong = -1;
const COPY_SESSION_NOT_FOUND: jlong = -2;
const COPY_STALE_GENERATION: jlong = -3;
const COPY_NO_PREPARED_FRAME: jlong = -4;
const COPY_BUFFER_TOO_SMALL: jlong = -5;
const COPY_BRIDGE_ERROR: jlong = -6;
const COPY_INVALID_BUFFER: jlong = -7;

const EXPORT_FORMAT_PNG: i32 = 0;
const EXPORT_FORMAT_JPEG: i32 = 1;
const EXPORT_FORMAT_WEBP_LOSSLESS: i32 = 2;

#[derive(Debug, Deserialize)]
struct PreparedFrameEnvelope {
    status: String,
    frame: Option<PreparedFramePayload>,
    code: Option<String>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PreparedFramePayload {
    session_id: i64,
    frame_id: u64,
    generation: i64,
    width: u32,
    height: u32,
    stride_bytes: usize,
    byte_len: usize,
}

#[derive(Debug, Serialize)]
struct ExportedFrameDetails {
    session_id: i64,
    frame_id: u64,
    width: u32,
    height: u32,
    format: &'static str,
    mime_type: &'static str,
    byte_len: u64,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum ExportFrameResponse {
    Ok {
        engine: &'static str,
        export: ExportedFrameDetails,
    },
    Error {
        engine: &'static str,
        code: String,
        message: String,
    },
}

#[derive(Debug)]
struct ExportFailure {
    code: String,
    message: String,
}

impl ExportFailure {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_MicroscopeFrameBridge_nativePrepareMicroscopeFrame(
    mut env: JNIEnv,
    _class: JClass,
    session_id: jlong,
) -> jstring {
    let json = catch_unwind(AssertUnwindSafe(|| {
        microscope::prepare_frame_response(session_id)
    }))
    .unwrap_or_else(|_| microscope::panic_frame_response());
    to_jstring(&mut env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_MicroscopeFrameBridge_nativeCopyMicroscopeFrameRgba(
    env: JNIEnv,
    _class: JClass,
    session_id: jlong,
    generation: jlong,
    destination: JByteBuffer,
) -> jlong {
    catch_unwind(AssertUnwindSafe(|| {
        copy_into_direct_buffer(&env, session_id, generation, &destination)
    }))
    .unwrap_or(COPY_BRIDGE_ERROR)
}

/// Export the current authoritative microscope frame into a caller-owned SAF-compatible output FD.
///
/// Kotlin keeps the original `ParcelFileDescriptor` open for the duration of this synchronous JNI
/// call. Rust duplicates the descriptor before encoding and owns only that duplicate, so returning
/// from this function never closes or invalidates the caller's descriptor. The accepted microscope
/// presentation path structurally excludes compressed preview proxies, and at most one RGBA frame
/// plus the encoder's format-specific scratch space is resident here.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_RustBridge_nativeExportCurrentMicroscopeFrameFd(
    mut env: JNIEnv,
    _class: JClass,
    session_id: jlong,
    output_fd: jint,
    operation_id: jlong,
    format_code: jint,
    jpeg_quality: jint,
) -> jstring {
    let json = catch_unwind(AssertUnwindSafe(|| {
        export_frame_response_json(
            session_id,
            output_fd,
            operation_id,
            format_code,
            jpeg_quality,
        )
    }))
    .unwrap_or_else(|_| panic_export_json());
    to_jstring(&mut env, &json)
}

fn copy_into_direct_buffer(
    env: &JNIEnv<'_>,
    session_id: i64,
    generation: i64,
    destination: &JByteBuffer<'_>,
) -> jlong {
    if session_id <= 0 || generation <= 0 {
        return COPY_INVALID_REQUEST;
    }

    let capacity = match env.get_direct_buffer_capacity(destination) {
        Ok(capacity) => capacity,
        Err(_) => return COPY_INVALID_BUFFER,
    };
    if capacity == 0 {
        return copy_result(microscope::copy_prepared_frame(
            session_id,
            generation,
            &mut [],
        ));
    }
    let address = match env.get_direct_buffer_address(destination) {
        Ok(address) => address,
        Err(_) => return COPY_INVALID_BUFFER,
    };

    // SAFETY: JNI guarantees that GetDirectBufferAddress points to `capacity` writable bytes for
    // this non-empty direct ByteBuffer. The local `JByteBuffer` reference remains alive for the
    // entire slice lifetime, and the slice is never stored or returned beyond this synchronous call.
    let destination = unsafe { std::slice::from_raw_parts_mut(address, capacity) };
    copy_result(microscope::copy_prepared_frame(
        session_id,
        generation,
        destination,
    ))
}

fn copy_result(result: Result<usize, microscope::MicroscopeFailure>) -> jlong {
    match result {
        Ok(copied) => i64::try_from(copied).unwrap_or(COPY_BRIDGE_ERROR),
        Err(error) => copy_error_code(error.code()),
    }
}

fn copy_error_code(code: &str) -> jlong {
    match code {
        "invalid_request" => COPY_INVALID_REQUEST,
        "session_not_found" => COPY_SESSION_NOT_FOUND,
        "stale_generation" => COPY_STALE_GENERATION,
        "no_prepared_frame" => COPY_NO_PREPARED_FRAME,
        "buffer_too_small" => COPY_BUFFER_TOO_SMALL,
        _ => COPY_BRIDGE_ERROR,
    }
}

fn export_frame_response_json(
    session_id: i64,
    output_fd: i32,
    operation_id: i64,
    format_code: i32,
    jpeg_quality: i32,
) -> String {
    let response = match export_current_frame(
        session_id,
        output_fd,
        operation_id,
        format_code,
        jpeg_quality,
    ) {
        Ok(export) => ExportFrameResponse::Ok {
            engine: ENGINE_VERSION,
            export,
        },
        Err(error) => ExportFrameResponse::Error {
            engine: ENGINE_VERSION,
            code: error.code,
            message: error.message,
        },
    };
    serde_json::to_string(&response).unwrap_or_else(|_| {
        concat!(
            r#"{"status":"error","engine":"framescope-rust/unknown","code":"bridge_error","#,
            r#""message":"failed to serialize frame export response"}"#,
        )
        .into()
    })
}

fn panic_export_json() -> String {
    serde_json::to_string(&ExportFrameResponse::Error {
        engine: ENGINE_VERSION,
        code: "bridge_error".into(),
        message: "native frame export aborted safely after an internal panic".into(),
    })
    .unwrap_or_else(|_| "{\"status\":\"error\"}".into())
}

fn export_current_frame(
    session_id: i64,
    output_fd: i32,
    operation_id: i64,
    format_code: i32,
    jpeg_quality: i32,
) -> Result<ExportedFrameDetails, ExportFailure> {
    if session_id <= 0 {
        return Err(ExportFailure::new(
            "invalid_request",
            "frame export requires a positive microscope session id",
        ));
    }
    if output_fd < 0 {
        return Err(ExportFailure::new(
            "invalid_destination",
            "frame export requires a writable output file descriptor",
        ));
    }
    let format = parse_export_format(format_code, jpeg_quality)?;
    let (cancellation, _operation) = operation_token(operation_id)
        .map_err(|error| ExportFailure::new(error.code(), error.to_string()))?;
    if cancellation.is_cancelled() {
        return Err(cancelled_export());
    }

    // Preparing may perform source-quality decode, but does not touch the output destination. If a
    // cancellation arrives during preparation, it is observed below before any encoded bytes are
    // written. The current presentation generation also prevents a concurrent navigation from
    // exporting stale pixels.
    let prepared = parse_prepared_frame(&microscope::prepare_frame_response(session_id))?;
    if cancellation.is_cancelled() {
        return Err(cancelled_export());
    }
    if prepared.session_id != session_id || prepared.generation <= 0 {
        return Err(ExportFailure::new(
            "malformed_response",
            "prepared microscope frame identity is invalid",
        ));
    }

    let mut rgba = vec![0_u8; prepared.byte_len];
    let copied = microscope::copy_prepared_frame(session_id, prepared.generation, &mut rgba)
        .map_err(copy_export_failure)?;
    if copied != prepared.byte_len {
        return Err(ExportFailure::new(
            "bridge_error",
            format!(
                "prepared frame copy returned {copied} bytes, expected {}",
                prepared.byte_len
            ),
        ));
    }
    if cancellation.is_cancelled() {
        return Err(cancelled_export());
    }

    let frame = OwnedRgbaFrame::new(
        prepared.width,
        prepared.height,
        prepared.stride_bytes,
        rgba,
    )
    .map_err(|error| ExportFailure::new("frame_error", error.to_string()))?;
    let report = encode_to_output_fd(output_fd, &frame, format)?;
    Ok(ExportedFrameDetails {
        session_id,
        frame_id: prepared.frame_id,
        width: prepared.width,
        height: prepared.height,
        format: export_format_name(format),
        mime_type: format.mime_type(),
        byte_len: report.byte_len,
    })
}

fn parse_prepared_frame(json: &str) -> Result<PreparedFramePayload, ExportFailure> {
    let envelope: PreparedFrameEnvelope = serde_json::from_str(json).map_err(|error| {
        ExportFailure::new(
            "malformed_response",
            format!("could not decode prepared frame response: {error}"),
        )
    })?;
    match envelope.status.as_str() {
        "ok" => envelope.frame.ok_or_else(|| {
            ExportFailure::new(
                "malformed_response",
                "prepared frame success response omitted frame metadata",
            )
        }),
        "error" => Err(ExportFailure::new(
            envelope.code.unwrap_or_else(|| "bridge_error".into()),
            envelope
                .message
                .unwrap_or_else(|| "microscope frame preparation failed".into()),
        )),
        _ => Err(ExportFailure::new(
            "malformed_response",
            "prepared frame response has an unknown status",
        )),
    }
}

fn parse_export_format(
    format_code: i32,
    jpeg_quality: i32,
) -> Result<ExtractionImageFormat, ExportFailure> {
    match format_code {
        EXPORT_FORMAT_PNG => Ok(ExtractionImageFormat::Png),
        EXPORT_FORMAT_JPEG => {
            let quality = u8::try_from(jpeg_quality).map_err(|_| {
                ExportFailure::new(
                    "invalid_request",
                    "JPEG quality must be in the inclusive range 1..=100",
                )
            })?;
            if !(1..=100).contains(&quality) {
                return Err(ExportFailure::new(
                    "invalid_request",
                    "JPEG quality must be in the inclusive range 1..=100",
                ));
            }
            Ok(ExtractionImageFormat::Jpeg { quality })
        }
        EXPORT_FORMAT_WEBP_LOSSLESS => Ok(ExtractionImageFormat::WebPLossless),
        _ => Err(ExportFailure::new(
            "invalid_request",
            "image format must be 0 (PNG), 1 (JPEG), or 2 (lossless WebP)",
        )),
    }
}

fn export_format_name(format: ExtractionImageFormat) -> &'static str {
    match format {
        ExtractionImageFormat::Png => "png",
        ExtractionImageFormat::Jpeg { .. } => "jpeg",
        ExtractionImageFormat::WebPLossless => "webp_lossless",
    }
}

fn cancelled_export() -> ExportFailure {
    ExportFailure::new("cancelled", "frame export was cancelled before output delivery")
}

fn copy_export_failure(error: microscope::MicroscopeFailure) -> ExportFailure {
    let code = error.code();
    let message = match code {
        "invalid_request" => "microscope frame copy request was invalid",
        "session_not_found" => "microscope session is no longer open",
        "stale_generation" => "microscope position changed before export could copy the frame",
        "no_prepared_frame" => "no source-quality microscope frame is prepared for export",
        "buffer_too_small" => "native export buffer is smaller than the prepared RGBA payload",
        _ => "native microscope frame copy failed",
    };
    ExportFailure::new(code, message)
}

fn image_export_failure(error: ImageExportError) -> ExportFailure {
    match error {
        ImageExportError::InvalidJpegQuality(_) => ExportFailure::new(
            "invalid_request",
            "JPEG quality must be in the inclusive range 1..=100",
        ),
        ImageExportError::Writer(error) => ExportFailure::new("io_error", error.to_string()),
        ImageExportError::NumericRange | ImageExportError::Image(_) => {
            ExportFailure::new("image_encode_error", error.to_string())
        }
    }
}

#[cfg(unix)]
fn encode_to_output_fd(
    output_fd: RawFd,
    frame: &OwnedRgbaFrame,
    format: ExtractionImageFormat,
) -> Result<framescope_extraction_image::EncodedImageReport, ExportFailure> {
    let duplicate = duplicate_fd(output_fd)
        .map_err(|error| ExportFailure::new("invalid_destination", error.to_string()))?;
    let mut output = File::from(duplicate);
    encode_frame(frame, format, &mut output).map_err(image_export_failure)
}

#[cfg(not(unix))]
fn encode_to_output_fd(
    _output_fd: i32,
    _frame: &OwnedRgbaFrame,
    _format: ExtractionImageFormat,
) -> Result<framescope_extraction_image::EncodedImageReport, ExportFailure> {
    Err(ExportFailure::new(
        "bridge_error",
        "file-descriptor frame export is only available on Android/Unix targets",
    ))
}

#[cfg(unix)]
fn duplicate_fd(fd: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: fcntl duplicates a caller-owned descriptor and returns a fresh descriptor that this
    // function immediately wraps in `OwnedFd`. The caller retains ownership of the original.
    let duplicated = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicated < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `duplicated` is a fresh descriptor returned by F_DUPFD_CLOEXEC and is transferred
    // exactly once into `OwnedFd`.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicated) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{FrameId, FrameIndexEntry, KeyframeAnchor};
    use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};
    use framescope_video::{CachedFrameSource, MicroscopeFramePresentation, MicroscopeTarget};
    #[cfg(unix)]
    use std::fs::OpenOptions;
    #[cfg(unix)]
    use std::io::{Read, Seek, SeekFrom, Write};
    #[cfg(unix)]
    use std::os::fd::AsRawFd;
    #[cfg(unix)]
    use std::sync::atomic::{AtomicU64, Ordering};

    #[cfg(unix)]
    static NEXT_EXPORT_TEST_ID: AtomicU64 = AtomicU64::new(1);

    fn presentation() -> MicroscopeFramePresentation {
        let time_base = TimeBase::new(1, 1_000).unwrap();
        MicroscopeFramePresentation {
            target: MicroscopeTarget {
                entry: FrameIndexEntry {
                    frame_id: FrameId(7),
                    presentation_timestamp: Some(MediaTimestamp {
                        ticks: 280,
                        time_base,
                    }),
                    duration: Some(MediaDuration {
                        ticks: 40,
                        time_base,
                    }),
                    keyframe: false,
                    corrupt: false,
                    anchor: KeyframeAnchor::Keyframe {
                        frame_id: FrameId::ZERO,
                        presentation_timestamp: Some(MediaTimestamp {
                            ticks: 0,
                            time_base,
                        }),
                    },
                },
                frame_count: 10,
            },
            pixels: OwnedRgbaFrame::new(
                2,
                2,
                8,
                vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
            )
            .unwrap(),
            source: CachedFrameSource::Decoded,
            decoded_frames: 3,
            used_keyframe_seek: true,
            fell_back_to_stream_start: false,
            cache_insert_result: None,
        }
    }

    #[test]
    fn copy_error_codes_are_stable_for_kotlin_boundary() {
        assert_eq!(copy_error_code("invalid_request"), -1);
        assert_eq!(copy_error_code("session_not_found"), -2);
        assert_eq!(copy_error_code("stale_generation"), -3);
        assert_eq!(copy_error_code("no_prepared_frame"), -4);
        assert_eq!(copy_error_code("buffer_too_small"), -5);
        assert_eq!(copy_error_code("timeline_mismatch"), -6);
    }

    #[test]
    fn export_format_codes_are_stable_and_jpeg_quality_is_bounded() {
        assert_eq!(
            parse_export_format(EXPORT_FORMAT_PNG, 0).unwrap(),
            ExtractionImageFormat::Png
        );
        assert_eq!(
            parse_export_format(EXPORT_FORMAT_JPEG, 92).unwrap(),
            ExtractionImageFormat::Jpeg { quality: 92 }
        );
        assert_eq!(
            parse_export_format(EXPORT_FORMAT_WEBP_LOSSLESS, 0).unwrap(),
            ExtractionImageFormat::WebPLossless
        );
        assert_eq!(parse_export_format(EXPORT_FORMAT_JPEG, 0).unwrap_err().code, "invalid_request");
        assert_eq!(parse_export_format(99, 92).unwrap_err().code, "invalid_request");
    }

    #[test]
    fn prepared_frame_errors_preserve_native_code_and_message() {
        let error = parse_prepared_frame(
            r#"{"status":"error","engine":"framescope-rust/0.1.0","code":"stale_generation","message":"moved"}"#,
        )
        .unwrap_err();
        assert_eq!(error.code, "stale_generation");
        assert_eq!(error.message, "moved");
    }

    #[cfg(unix)]
    #[test]
    fn fd_encoder_writes_png_without_closing_callers_descriptor() {
        let id = NEXT_EXPORT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "framescope-export-fd-{}-{id}.png",
            std::process::id()
        ));
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let frame = presentation().pixels;

        let report = encode_to_output_fd(file.as_raw_fd(), &frame, ExtractionImageFormat::Png)
            .unwrap();
        assert!(report.byte_len > 8);

        // The JNI layer owns only a duplicate. The caller's descriptor must remain valid.
        file.write_all(b"tail").unwrap();
        file.flush().unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(bytes.ends_with(b"tail"));

        drop(file);
        let _ = std::fs::remove_file(path);
    }
}
