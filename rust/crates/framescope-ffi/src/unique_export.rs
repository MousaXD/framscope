use framescope_extraction_groups_output::{
    GroupStreamingExtractionError, export_group_representatives,
};
use framescope_extraction_image::{
    EncodedImageReport, ExtractionImageFormat, ImageExportError, encode_frame,
};
use framescope_extraction_output::{FrameOutput, FrameOutputSink};
use framescope_group_navigation::GroupNavigationError;
use framescope_group_navigation_video::open_or_build_group_navigation_from_fd;
use framescope_perceptual::HybridSimilarityPolicy;
use framescope_video::{OpenOptions, VideoDecoder, VideoStreamSelection};
use jni::JNIEnv;
use jni::objects::{JClass, JObject, JString, JValue};
use jni::sys::{jint, jlong, jstring};
use serde::Serialize;
use std::fs::File;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::fd::{BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use thiserror::Error;

use super::{ENGINE_VERSION, microscope, operation_token, to_jstring};

const EXPORT_FORMAT_PNG: i32 = 0;
const EXPORT_FORMAT_JPEG: i32 = 1;
const EXPORT_FORMAT_WEBP_LOSSLESS: i32 = 2;
const MAX_CACHE_ROOT_LENGTH: usize = 4_096;

// Production Phase 6 unique-frame grouping policy. Scores are on the documented 0..=10_000
// similarity scale and are not literal percentages of changed pixels.
const UNIQUE_GROUP_POLICY: HybridSimilarityPolicy = HybridSimilarityPolicy {
    max_hash_distance: 8,
    minimum_luma_similarity: 9_700,
};

const OPEN_FRAME_METHOD: &str = "openFrame";
const OPEN_FRAME_SIGNATURE: &str = "(Ljava/lang/String;Ljava/lang/String;JJJ)I";
const COMMIT_FRAME_METHOD: &str = "commitFrame";
const COMMIT_FRAME_SIGNATURE: &str = "(Ljava/lang/String;JJJJ)Z";
const ABORT_FRAME_METHOD: &str = "abortFrame";
const ABORT_FRAME_SIGNATURE: &str = "(Ljava/lang/String;)V";

#[derive(Debug, Serialize)]
struct UniqueExportDetails {
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
enum UniqueExportResponse {
    Ok {
        engine: &'static str,
        export: UniqueExportDetails,
    },
    Error {
        engine: &'static str,
        code: String,
        message: String,
    },
}

#[derive(Debug)]
struct UniqueExportFailure {
    code: String,
    message: String,
}

impl UniqueExportFailure {
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
            return Err(JniFrameSinkError::InvalidDestination);
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
            return Err(JniFrameSinkError::CommitRejected);
        }
        Ok(())
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
pub extern "system" fn Java_com_framescope_app_data_RustUniqueExportBridge_nativeExportMicroscopeUniqueGroupsFd<
    'local,
>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    session_id: jlong,
    manifest_fd: jint,
    operation_id: jlong,
    cache_root: JString<'local>,
    format_code: jint,
    jpeg_quality: jint,
    sink: JObject<'local>,
) -> jstring {
    let cache_root: String = match env.get_string(&cache_root) {
        Ok(value) => value.into(),
        Err(_) => return to_jstring(&mut env, &invalid_cache_root_json()),
    };
    let json = catch_unwind(AssertUnwindSafe(|| {
        unique_export_response_json(
            &mut env,
            sink,
            session_id,
            manifest_fd,
            operation_id,
            &cache_root,
            format_code,
            jpeg_quality,
        )
    }))
    .unwrap_or_else(|_| panic_unique_export_json());
    to_jstring(&mut env, &json)
}

#[allow(clippy::too_many_arguments)]
fn unique_export_response_json<'local>(
    env: &mut JNIEnv<'local>,
    sink: JObject<'local>,
    session_id: i64,
    manifest_fd: i32,
    operation_id: i64,
    cache_root: &str,
    format_code: i32,
    jpeg_quality: i32,
) -> String {
    let response = match export_unique_groups(
        env,
        sink,
        session_id,
        manifest_fd,
        operation_id,
        cache_root,
        format_code,
        jpeg_quality,
    ) {
        Ok(export) => UniqueExportResponse::Ok {
            engine: ENGINE_VERSION,
            export,
        },
        Err(error) => UniqueExportResponse::Error {
            engine: ENGINE_VERSION,
            code: error.code,
            message: error.message,
        },
    };
    serde_json::to_string(&response).unwrap_or_else(|_| {
        concat!(
            r#"{"status":"error","engine":"framescope-rust/unknown","code":"bridge_error","#,
            r#""message":"failed to serialize unique export response"}"#,
        )
        .into()
    })
}

fn invalid_cache_root_json() -> String {
    serde_json::to_string(&UniqueExportResponse::Error {
        engine: ENGINE_VERSION,
        code: "invalid_request".into(),
        message: "unique export cache root could not be decoded".into(),
    })
    .unwrap_or_else(|_| "{\"status\":\"error\"}".into())
}

fn panic_unique_export_json() -> String {
    serde_json::to_string(&UniqueExportResponse::Error {
        engine: ENGINE_VERSION,
        code: "bridge_error".into(),
        message: "native unique-group export aborted safely after an internal panic".into(),
    })
    .unwrap_or_else(|_| "{\"status\":\"error\"}".into())
}

#[allow(clippy::too_many_arguments)]
fn export_unique_groups<'local>(
    env: &mut JNIEnv<'local>,
    sink: JObject<'local>,
    session_id: i64,
    manifest_fd: i32,
    operation_id: i64,
    cache_root: &str,
    format_code: i32,
    jpeg_quality: i32,
) -> Result<UniqueExportDetails, UniqueExportFailure> {
    if session_id <= 0 {
        return Err(UniqueExportFailure::new(
            "invalid_request",
            "unique export requires a positive microscope session id",
        ));
    }
    if manifest_fd < 0 {
        return Err(UniqueExportFailure::new(
            "invalid_destination",
            "unique export requires a writable manifest file descriptor",
        ));
    }
    if sink.is_null() {
        return Err(UniqueExportFailure::new(
            "invalid_destination",
            "unique export requires a frame output callback",
        ));
    }
    if cache_root.trim().is_empty() || cache_root.len() > MAX_CACHE_ROOT_LENGTH {
        return Err(UniqueExportFailure::new(
            "invalid_request",
            "unique export cache root is empty or exceeds the safety limit",
        ));
    }

    let format = parse_export_format(format_code, jpeg_quality)?;
    let (cancellation, _operation) = operation_token(operation_id)
        .map_err(|error| UniqueExportFailure::new(error.code(), error.to_string()))?;
    let manifest_duplicate = duplicate_fd(manifest_fd)
        .map_err(|error| UniqueExportFailure::new("invalid_destination", error.to_string()))?;
    let mut manifest_output = File::from(manifest_duplicate);
    let mut frame_sink = JniFrameSink {
        env,
        callback: sink,
    };
    let store_root = Path::new(cache_root).join("similarity-groups");

    let export = microscope::with_extraction_context(session_id, |source_fd, index| {
        // SAFETY: the microscope session owns the source descriptor and holds its session mutex for
        // this synchronous operation. Both video adapters duplicate the descriptor before use.
        let borrowed = unsafe { BorrowedFd::borrow_raw(source_fd) };
        let analysis = open_or_build_group_navigation_from_fd(
            index,
            &store_root,
            UNIQUE_GROUP_POLICY,
            borrowed,
            cancellation.clone(),
        )
        .map_err(group_preflight_failure)?;
        if cancellation.is_cancelled() {
            return Err(UniqueExportFailure::new(
                "cancelled_preflight",
                "unique export was cancelled before representative output began",
            ));
        }

        let selected_stream = index.stream_identity().stream_index;
        let report = export_group_representatives(
            index,
            &analysis.navigator,
            format,
            || {
                // SAFETY: same session-lifetime contract as above. VideoDecoder duplicates this
                // borrowed descriptor and owns only its duplicate.
                let borrowed = unsafe { BorrowedFd::borrow_raw(source_fd) };
                VideoDecoder::open_file_descriptor_with_options(
                    borrowed,
                    OpenOptions {
                        stream_selection: VideoStreamSelection::Index(selected_stream),
                    },
                    cancellation.clone(),
                )
            },
            || cancellation.is_cancelled(),
            &mut frame_sink,
            &mut manifest_output,
        )
        .map_err(group_output_failure)?;
        Ok((analysis.summary.group_count, report))
    })
    .map_err(|error| UniqueExportFailure::new(error.code(), error.message()))??;

    let (expected_frames, report) = export;
    Ok(UniqueExportDetails {
        session_id,
        expected_frames,
        committed_frames: report.committed_frames,
        encoded_bytes: report.encoded_bytes,
        decoded_frames: report.batch.decoded_frames,
        used_keyframe_seek: report.batch.used_keyframe_seek,
        fell_back_to_stream_start: report.batch.fell_back_to_stream_start,
        format: export_format_name(format),
        mime_type: format.mime_type(),
    })
}

fn group_preflight_failure(error: GroupNavigationError) -> UniqueExportFailure {
    match error {
        GroupNavigationError::Cancelled => UniqueExportFailure::new(
            "cancelled_preflight",
            "unique export was cancelled while preparing similarity groups",
        ),
        GroupNavigationError::UnsafeSourceIdentity => UniqueExportFailure::new(
            "unsafe_source_identity",
            "unique export cannot persist similarity groups for an unverifiable source identity",
        ),
        other => UniqueExportFailure::new("group_preflight_error", other.to_string()),
    }
}

fn group_output_failure(
    error: GroupStreamingExtractionError<JniFrameSinkError>,
) -> UniqueExportFailure {
    match error {
        GroupStreamingExtractionError::Cancelled => {
            UniqueExportFailure::new("cancelled", "unique export was cancelled")
        }
        GroupStreamingExtractionError::Output(error) => {
            UniqueExportFailure::new(error.code(), error.to_string())
        }
        GroupStreamingExtractionError::Manifest(error) => {
            UniqueExportFailure::new("manifest_error", error.to_string())
        }
        GroupStreamingExtractionError::SinkContract(message) => {
            UniqueExportFailure::new("output_contract_error", message)
        }
        GroupStreamingExtractionError::Batch(error) => {
            UniqueExportFailure::new("decode_error", error.to_string())
        }
        other => UniqueExportFailure::new("group_output_error", other.to_string()),
    }
}

fn parse_export_format(
    format_code: i32,
    jpeg_quality: i32,
) -> Result<ExtractionImageFormat, UniqueExportFailure> {
    match format_code {
        EXPORT_FORMAT_PNG => Ok(ExtractionImageFormat::Png),
        EXPORT_FORMAT_JPEG => {
            let quality = u8::try_from(jpeg_quality).map_err(|_| {
                UniqueExportFailure::new(
                    "invalid_request",
                    "JPEG quality must be in the inclusive range 1..=100",
                )
            })?;
            if !(1..=100).contains(&quality) {
                return Err(UniqueExportFailure::new(
                    "invalid_request",
                    "JPEG quality must be in the inclusive range 1..=100",
                ));
            }
            Ok(ExtractionImageFormat::Jpeg { quality })
        }
        EXPORT_FORMAT_WEBP_LOSSLESS => Ok(ExtractionImageFormat::WebPLossless),
        _ => Err(UniqueExportFailure::new(
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

#[cfg(unix)]
fn duplicate_fd(fd: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: fcntl duplicates a caller-owned descriptor and returns a fresh descriptor whose
    // ownership is transferred exactly once into OwnedFd.
    let duplicated = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicated < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `duplicated` is a fresh descriptor returned by F_DUPFD_CLOEXEC.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicated) })
}

#[cfg(not(unix))]
fn duplicate_fd(_fd: i32) -> io::Result<std::fs::File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "file-descriptor unique export is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_unique_policy_is_explicit_and_stable() {
        assert_eq!(UNIQUE_GROUP_POLICY.max_hash_distance, 8);
        assert_eq!(UNIQUE_GROUP_POLICY.minimum_luma_similarity, 9_700);
    }

    #[test]
    fn preflight_cancellation_has_non_persistable_code() {
        let error = group_preflight_failure(GroupNavigationError::Cancelled);
        assert_eq!(error.code, "cancelled_preflight");
    }

    #[test]
    fn unique_export_format_contract_matches_batch_export() {
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
    }
}
