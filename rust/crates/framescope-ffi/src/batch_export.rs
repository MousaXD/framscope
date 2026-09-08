use framescope_cache::FrameId;
use framescope_extraction::{ExtractionRequest, ExtractionSampling, ExtractionSelection};
use framescope_extraction_image::{
    EncodedImageReport, ExtractionImageFormat, ImageExportError, encode_frame,
};
use framescope_extraction_output::{
    FrameOutput, FrameOutputSink, StreamingExtractionError, export_request,
};
use framescope_video::{OpenOptions, VideoDecoder};
use jni::JNIEnv;
use jni::objects::{JClass, JObject, JString, JValue};
use jni::sys::{jint, jlong, jstring};
use serde::Serialize;
use std::fs::File;
use std::io::{self, Write};
use std::num::NonZeroU64;
#[cfg(unix)]
use std::os::fd::{BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::panic::{AssertUnwindSafe, catch_unwind};
use thiserror::Error;

use super::{ENGINE_VERSION, microscope, operation_token, to_jstring};

const SELECTION_CURRENT_FRAME: i32 = 0;
const SELECTION_FRAME_RANGE: i32 = 1;
const SELECTION_TIMESTAMP_RANGE: i32 = 2;
const SELECTION_ALL_FRAMES: i32 = 3;

const EXPORT_FORMAT_PNG: i32 = 0;
const EXPORT_FORMAT_JPEG: i32 = 1;
const EXPORT_FORMAT_WEBP_LOSSLESS: i32 = 2;

const OPEN_FRAME_METHOD: &str = "openFrame";
const OPEN_FRAME_SIGNATURE: &str = "(Ljava/lang/String;Ljava/lang/String;JJJ)I";
const COMMIT_FRAME_METHOD: &str = "commitFrame";
const COMMIT_FRAME_SIGNATURE: &str = "(Ljava/lang/String;JJJJ)Z";
const ABORT_FRAME_METHOD: &str = "abortFrame";
const ABORT_FRAME_SIGNATURE: &str = "(Ljava/lang/String;)V";
const FAILURE_CODE_METHOD: &str = "failureCode";
const FAILURE_CODE_SIGNATURE: &str = "()Ljava/lang/String;";
const STORAGE_FULL_CODE: &str = "storage_full";
const STORAGE_FULL_MESSAGE: &str = "The export destination is out of space or has reached its storage quota. Free space or choose another folder and try again.";

#[derive(Debug, Serialize)]
struct BatchExportDetails {
    session_id: i64,
    expected_frames: u64,
    committed_frames: u64,
    encoded_bytes: u64,
    decoded_frames: u64,
    used_keyframe_seek: bool,
    fell_back_to_stream_start: bool,
    format: &'static str,
    mime_type: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum BatchExportResponse {
    Ok {
        engine: &'static str,
        export: BatchExportDetails,
    },
    Error {
        engine: &'static str,
        code: String,
        message: String,
    },
}

#[derive(Debug)]
struct BatchExportFailure {
    code: String,
    message: String,
}

impl BatchExportFailure {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Error)]
enum JniFrameSinkError {
    #[error("frame sink metadata exceeds the JVM signed integer range")]
    NumericRange,
    #[error("frame sink JNI call failed: {0}")]
    Jni(#[from] jni::errors::Error),
    #[error("frame sink did not return a writable file descriptor")]
    InvalidDestination,
    #[error("{STORAGE_FULL_MESSAGE}")]
    StorageFull,
    #[error("image encoding failed: {0}")]
    Image(#[from] ImageExportError),
    #[error("frame sink rejected the committed artifact")]
    CommitRejected,
}

impl JniFrameSinkError {
    fn code(&self) -> &'static str {
        match self {
            Self::NumericRange => "numeric_range",
            Self::Jni(_) => "output_callback_error",
            Self::InvalidDestination => "invalid_destination",
            Self::StorageFull => STORAGE_FULL_CODE,
            Self::Image(_) => "image_encode_error",
            Self::CommitRejected => "output_commit_error",
        }
    }
}

struct JniFrameSink<'env, 'local> {
    env: &'env mut JNIEnv<'local>,
    callback: JObject<'local>,
}

impl JniFrameSink<'_, '_> {
    fn open_frame(&mut self, output: &FrameOutput<'_>) -> Result<RawFd, JniFrameSinkError> {
        let frame_id = jlong::try_from(output.progress.frame_id.0)
            .map_err(|_| JniFrameSinkError::NumericRange)?;
        let ordinal = jlong::try_from(output.progress.ordinal)
            .map_err(|_| JniFrameSinkError::NumericRange)?;
        let total =
            jlong::try_from(output.progress.total).map_err(|_| JniFrameSinkError::NumericRange)?;
        let file_name = JObject::from(self.env.new_string(output.file_name)?);
        let mime_type = JObject::from(self.env.new_string(output.format.mime_type())?);
        let result = self.env.call_method(
            &self.callback,
            OPEN_FRAME_METHOD,
            OPEN_FRAME_SIGNATURE,
            &[
                JValue::Object(&file_name),
                JValue::Object(&mime_type),
                JValue::Long(frame_id),
                JValue::Long(ordinal),
                JValue::Long(total),
            ],
        );
        let _ = self.env.delete_local_ref(file_name);
        let _ = self.env.delete_local_ref(mime_type);
        let fd = result?.i()?;
        if fd < 0 {
            return Err(if self.reported_storage_full() {
                JniFrameSinkError::StorageFull
            } else {
                JniFrameSinkError::InvalidDestination
            });
        }
        Ok(fd)
    }

    fn commit_frame(
        &mut self,
        output: &FrameOutput<'_>,
        byte_len: u64,
    ) -> Result<(), JniFrameSinkError> {
        let frame_id = jlong::try_from(output.progress.frame_id.0)
            .map_err(|_| JniFrameSinkError::NumericRange)?;
        let ordinal = jlong::try_from(output.progress.ordinal)
            .map_err(|_| JniFrameSinkError::NumericRange)?;
        let total =
            jlong::try_from(output.progress.total).map_err(|_| JniFrameSinkError::NumericRange)?;
        let byte_len = jlong::try_from(byte_len).map_err(|_| JniFrameSinkError::NumericRange)?;
        let file_name = JObject::from(self.env.new_string(output.file_name)?);
        let result = self.env.call_method(
            &self.callback,
            COMMIT_FRAME_METHOD,
            COMMIT_FRAME_SIGNATURE,
            &[
                JValue::Object(&file_name),
                JValue::Long(frame_id),
                JValue::Long(ordinal),
                JValue::Long(total),
                JValue::Long(byte_len),
            ],
        );
        let _ = self.env.delete_local_ref(file_name);
        if !result?.z()? {
            return Err(if self.reported_storage_full() {
                JniFrameSinkError::StorageFull
            } else {
                JniFrameSinkError::CommitRejected
            });
        }
        Ok(())
    }

    fn reported_storage_full(&mut self) -> bool {
        let result = match self.env.call_method(
            &self.callback,
            FAILURE_CODE_METHOD,
            FAILURE_CODE_SIGNATURE,
            &[],
        ) {
            Ok(result) => result,
            Err(_) => {
                let _ = self.env.exception_clear();
                return false;
            }
        };
        let Ok(object) = result.l() else {
            return false;
        };
        if object.is_null() {
            return false;
        }
        let code = JString::from(object);
        let matches = self
            .env
            .get_string(&code)
            .map(|value| value.to_string_lossy() == STORAGE_FULL_CODE)
            .unwrap_or(false);
        let _ = self.env.delete_local_ref(code);
        matches
    }

    fn abort_frame(&mut self, file_name: &str) {
        let Ok(file_name) = self.env.new_string(file_name) else {
            return;
        };
        let file_name = JObject::from(file_name);
        let _ = self.env.call_method(
            &self.callback,
            ABORT_FRAME_METHOD,
            ABORT_FRAME_SIGNATURE,
            &[JValue::Object(&file_name)],
        );
        let _ = self.env.delete_local_ref(file_name);
    }
}

impl FrameOutputSink for JniFrameSink<'_, '_> {
    type Error = JniFrameSinkError;

    fn write_frame(&mut self, output: FrameOutput<'_>) -> Result<EncodedImageReport, Self::Error> {
        let output_fd = self.open_frame(&output)?;
        let duplicate = match duplicate_fd(output_fd) {
            Ok(duplicate) => duplicate,
            Err(_) => {
                self.abort_frame(output.file_name);
                return Err(JniFrameSinkError::InvalidDestination);
            }
        };
        let mut destination = File::from(duplicate);
        let report = match encode_frame(output.pixels, output.format, &mut destination) {
            Ok(report) => report,
            Err(error) => {
                drop(destination);
                self.abort_frame(output.file_name);
                return Err(JniFrameSinkError::Image(error));
            }
        };
        if destination.flush().is_err() {
            drop(destination);
            self.abort_frame(output.file_name);
            return Err(JniFrameSinkError::InvalidDestination);
        }
        drop(destination);
        if let Err(error) = self.commit_frame(&output, report.byte_len) {
            self.abort_frame(output.file_name);
            return Err(error);
        }
        Ok(report)
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_RustBridge_nativeExportMicroscopeBatchFd<
    'local,
>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    session_id: jlong,
    manifest_fd: jint,
    operation_id: jlong,
    selection_kind: jint,
    start: jlong,
    end: jlong,
    every_n: jlong,
    format_code: jint,
    jpeg_quality: jint,
    sink: JObject<'local>,
) -> jstring {
    let json = catch_unwind(AssertUnwindSafe(|| {
        batch_export_response_json(
            &mut env,
            sink,
            session_id,
            manifest_fd,
            operation_id,
            selection_kind,
            start,
            end,
            every_n,
            format_code,
            jpeg_quality,
        )
    }))
    .unwrap_or_else(|_| panic_batch_export_json());
    to_jstring(&mut env, &json)
}

#[allow(clippy::too_many_arguments)]
fn batch_export_response_json<'local>(
    env: &mut JNIEnv<'local>,
    sink: JObject<'local>,
    session_id: i64,
    manifest_fd: i32,
    operation_id: i64,
    selection_kind: i32,
    start: i64,
    end: i64,
    every_n: i64,
    format_code: i32,
    jpeg_quality: i32,
) -> String {
    let response = match export_batch(
        env,
        sink,
        session_id,
        manifest_fd,
        operation_id,
        selection_kind,
        start,
        end,
        every_n,
        format_code,
        jpeg_quality,
    ) {
        Ok(export) => BatchExportResponse::Ok {
            engine: ENGINE_VERSION,
            export,
        },
        Err(error) => BatchExportResponse::Error {
            engine: ENGINE_VERSION,
            code: error.code,
            message: error.message,
        },
    };
    serde_json::to_string(&response).unwrap_or_else(|_| {
        concat!(
            r#"{"status":"error","engine":"framescope-rust/unknown","code":"bridge_error","#,
            r#""message":"failed to serialize batch export response"}"#,
        )
        .into()
    })
}

fn panic_batch_export_json() -> String {
    serde_json::to_string(&BatchExportResponse::Error {
        engine: ENGINE_VERSION,
        code: "bridge_error".into(),
        message: "native batch export aborted safely after an internal panic".into(),
    })
    .unwrap_or_else(|_| "{\"status\":\"error\"}".into())
}

#[allow(clippy::too_many_arguments)]
fn export_batch<'local>(
    env: &mut JNIEnv<'local>,
    sink: JObject<'local>,
    session_id: i64,
    manifest_fd: i32,
    operation_id: i64,
    selection_kind: i32,
    start: i64,
    end: i64,
    every_n: i64,
    format_code: i32,
    jpeg_quality: i32,
) -> Result<BatchExportDetails, BatchExportFailure> {
    if session_id <= 0 {
        return Err(BatchExportFailure::new(
            "invalid_request",
            "batch export requires a positive microscope session id",
        ));
    }
    if manifest_fd < 0 {
        return Err(BatchExportFailure::new(
            "invalid_destination",
            "batch export requires a writable manifest file descriptor",
        ));
    }
    if sink.is_null() {
        return Err(BatchExportFailure::new(
            "invalid_destination",
            "batch export requires a frame output callback",
        ));
    }

    let request = parse_request(selection_kind, start, end, every_n)?;
    let format = parse_export_format(format_code, jpeg_quality)?;
    let (cancellation, _operation) = operation_token(operation_id)
        .map_err(|error| BatchExportFailure::new(error.code(), error.to_string()))?;
    let manifest_duplicate = duplicate_fd(manifest_fd)
        .map_err(|error| BatchExportFailure::new("invalid_destination", error.to_string()))?;
    let mut manifest_output = File::from(manifest_duplicate);
    let mut frame_sink = JniFrameSink {
        env,
        callback: sink,
    };

    let export = microscope::with_extraction_context(session_id, |source_fd, index| {
        export_request(
            index,
            request,
            format,
            || {
                // SAFETY: `with_extraction_context` holds the owning microscope session lock for
                // this entire synchronous operation. `VideoDecoder` duplicates the descriptor in
                // its constructor and never takes ownership of this borrowed raw descriptor.
                let borrowed = unsafe { BorrowedFd::borrow_raw(source_fd) };
                VideoDecoder::open_file_descriptor_with_options(
                    borrowed,
                    OpenOptions::default(),
                    cancellation.clone(),
                )
            },
            || cancellation.is_cancelled(),
            &mut frame_sink,
            &mut manifest_output,
        )
    })
    .map_err(|error| BatchExportFailure::new(error.code(), error.message()))?
    .map_err(streaming_failure)?;

    Ok(BatchExportDetails {
        session_id,
        expected_frames: export.plan.selected_count,
        committed_frames: export.committed_frames,
        encoded_bytes: export.encoded_bytes,
        decoded_frames: export.batch.decoded_frames,
        used_keyframe_seek: export.batch.used_keyframe_seek,
        fell_back_to_stream_start: export.batch.fell_back_to_stream_start,
        format: export_format_name(format),
        mime_type: format.mime_type(),
    })
}

fn parse_request(
    selection_kind: i32,
    start: i64,
    end: i64,
    every_n: i64,
) -> Result<ExtractionRequest, BatchExportFailure> {
    let every_n = u64::try_from(every_n)
        .ok()
        .and_then(NonZeroU64::new)
        .ok_or_else(|| {
            BatchExportFailure::new(
                "invalid_request",
                "batch export frame interval must be a positive integer",
            )
        })?;
    let sampling = if every_n == NonZeroU64::MIN {
        ExtractionSampling::EveryFrame
    } else {
        ExtractionSampling::EveryNthFrame(every_n)
    };
    let selection = match selection_kind {
        SELECTION_CURRENT_FRAME => {
            let frame_id = u64::try_from(start).map_err(|_| {
                BatchExportFailure::new(
                    "invalid_request",
                    "current-frame export requires a non-negative FrameId",
                )
            })?;
            ExtractionSelection::CurrentFrame(FrameId(frame_id))
        }
        SELECTION_FRAME_RANGE => {
            let start = u64::try_from(start).map_err(|_| {
                BatchExportFailure::new(
                    "invalid_request",
                    "frame-range start must be a non-negative FrameId",
                )
            })?;
            let end = u64::try_from(end).map_err(|_| {
                BatchExportFailure::new(
                    "invalid_request",
                    "frame-range end must be a non-negative FrameId",
                )
            })?;
            ExtractionSelection::FrameRangeInclusive {
                start: FrameId(start),
                end: FrameId(end),
            }
        }
        SELECTION_TIMESTAMP_RANGE => ExtractionSelection::TimestampRangeUsInclusive {
            start_us: start,
            end_us: end,
        },
        SELECTION_ALL_FRAMES => ExtractionSelection::AllFrames,
        _ => {
            return Err(BatchExportFailure::new(
                "invalid_request",
                "batch selection must be 0 (frame), 1 (frame range), 2 (time range), or 3 (all)",
            ));
        }
    };
    Ok(ExtractionRequest {
        selection,
        sampling,
    })
}

fn parse_export_format(
    format_code: i32,
    jpeg_quality: i32,
) -> Result<ExtractionImageFormat, BatchExportFailure> {
    match format_code {
        EXPORT_FORMAT_PNG => Ok(ExtractionImageFormat::Png),
        EXPORT_FORMAT_JPEG => {
            let quality = u8::try_from(jpeg_quality).map_err(|_| {
                BatchExportFailure::new(
                    "invalid_request",
                    "JPEG quality must be in the inclusive range 1..=100",
                )
            })?;
            if !(1..=100).contains(&quality) {
                return Err(BatchExportFailure::new(
                    "invalid_request",
                    "JPEG quality must be in the inclusive range 1..=100",
                ));
            }
            Ok(ExtractionImageFormat::Jpeg { quality })
        }
        EXPORT_FORMAT_WEBP_LOSSLESS => Ok(ExtractionImageFormat::WebPLossless),
        _ => Err(BatchExportFailure::new(
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

fn streaming_failure(error: StreamingExtractionError<JniFrameSinkError>) -> BatchExportFailure {
    match error {
        StreamingExtractionError::Plan(error) => {
            BatchExportFailure::new("selection_error", error.to_string())
        }
        StreamingExtractionError::Batch(error) => {
            BatchExportFailure::new("decode_error", error.to_string())
        }
        StreamingExtractionError::Output(error) => {
            BatchExportFailure::new(error.code(), error.to_string())
        }
        StreamingExtractionError::Manifest(error) => {
            BatchExportFailure::new("manifest_error", error.to_string())
        }
        StreamingExtractionError::SinkContract(message) => {
            BatchExportFailure::new("output_contract_error", message)
        }
        StreamingExtractionError::Cancelled => {
            BatchExportFailure::new("cancelled", "batch export was cancelled")
        }
    }
}

#[cfg(unix)]
fn duplicate_fd(fd: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: fcntl duplicates a caller-owned descriptor and returns a fresh descriptor that this
    // function immediately wraps in `OwnedFd`. The caller retains ownership of the original.
    let duplicated = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicated < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `duplicated` is a fresh descriptor returned by F_DUPFD_CLOEXEC and ownership is
    // transferred exactly once into `OwnedFd`.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicated) })
}

#[cfg(not(unix))]
fn duplicate_fd(_fd: i32) -> io::Result<std::fs::File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "file-descriptor batch export is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_codes_preserve_frame_and_time_domains_without_fps_math() {
        assert_eq!(
            parse_request(SELECTION_CURRENT_FRAME, 7, 0, 1)
                .unwrap()
                .selection,
            ExtractionSelection::CurrentFrame(FrameId(7))
        );
        assert_eq!(
            parse_request(SELECTION_FRAME_RANGE, 3, 9, 2)
                .unwrap()
                .selection,
            ExtractionSelection::FrameRangeInclusive {
                start: FrameId(3),
                end: FrameId(9),
            }
        );
        assert_eq!(
            parse_request(SELECTION_TIMESTAMP_RANGE, -25_000, 90_000, 3)
                .unwrap()
                .selection,
            ExtractionSelection::TimestampRangeUsInclusive {
                start_us: -25_000,
                end_us: 90_000,
            }
        );
        assert_eq!(
            parse_request(SELECTION_ALL_FRAMES, 123, 456, 4)
                .unwrap()
                .selection,
            ExtractionSelection::AllFrames
        );
    }

    #[test]
    fn positive_every_n_sampling_is_preserved_exactly() {
        assert_eq!(
            parse_request(SELECTION_ALL_FRAMES, 0, 0, 1)
                .unwrap()
                .sampling,
            ExtractionSampling::EveryFrame
        );
        assert_eq!(
            parse_request(SELECTION_ALL_FRAMES, 0, 0, 5)
                .unwrap()
                .sampling,
            ExtractionSampling::EveryNthFrame(NonZeroU64::new(5).unwrap())
        );
        assert_eq!(
            parse_request(SELECTION_ALL_FRAMES, 0, 0, 0)
                .unwrap_err()
                .code,
            "invalid_request"
        );
    }

    #[test]
    fn frame_selection_rejects_negative_identity_and_unknown_kind() {
        assert_eq!(
            parse_request(SELECTION_FRAME_RANGE, -1, 3, 1)
                .unwrap_err()
                .code,
            "invalid_request"
        );
        assert_eq!(
            parse_request(99, 0, 0, 1).unwrap_err().code,
            "invalid_request"
        );
    }

    #[test]
    fn format_codes_match_current_frame_export_contract() {
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
        assert_eq!(
            parse_export_format(EXPORT_FORMAT_JPEG, 101)
                .unwrap_err()
                .code,
            "invalid_request"
        );
    }

    #[test]
    fn storage_full_error_code_is_stable() {
        assert_eq!(JniFrameSinkError::StorageFull.code(), STORAGE_FULL_CODE);
        assert_eq!(
            JniFrameSinkError::StorageFull.to_string(),
            STORAGE_FULL_MESSAGE
        );
    }
}
