//! Narrow JNI boundary for the Android app.

mod microscope;

use framescope_core::{FrameScopeError, MediaKind, StreamInfo, VideoInfo};
use framescope_video::{CancellationToken, ObservedFrameRateMode, OpenOptions, VideoDecoder};
use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint, jlong, jstring};
use serde::Serialize;
use std::collections::HashMap;
#[cfg(target_family = "unix")]
use std::os::fd::BorrowedFd;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::{Mutex, OnceLock};

const ENGINE_VERSION: &str = concat!("framescope-rust/", env!("CARGO_PKG_VERSION"));
const MAX_PENDING_CANCELLATIONS: usize = 64;
const VFR_SAMPLE_FRAMES: usize = 12;

type OperationId = i64;

static INSPECTION_TOKENS: OnceLock<Mutex<HashMap<OperationId, CancellationToken>>> =
    OnceLock::new();

#[derive(Debug, Serialize)]
struct InspectionMetadata {
    duration_us: Option<u64>,
    width: u32,
    height: u32,
    estimated_frame_rate: Option<f64>,
    rotation_degrees: i32,
    container: String,
    codec: String,
    video_stream_index: u32,
    video_stream_count: u32,
    audio_stream_count: u32,
    pixel_format: Option<String>,
    variable_frame_rate: Option<bool>,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum InspectResponse {
    Ok {
        engine: &'static str,
        metadata: InspectionMetadata,
    },
    Error {
        engine: &'static str,
        code: &'static str,
        message: String,
    },
}

struct InspectionOperation {
    id: OperationId,
}

impl Drop for InspectionOperation {
    fn drop(&mut self) {
        if let Ok(mut tokens) = inspection_tokens().lock() {
            tokens.remove(&self.id);
        }
    }
}

fn inspection_tokens() -> &'static Mutex<HashMap<OperationId, CancellationToken>> {
    INSPECTION_TOKENS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn operation_token(
    operation_id: OperationId,
) -> Result<(CancellationToken, InspectionOperation), FrameScopeError> {
    if operation_id <= 0 {
        return Err(FrameScopeError::Bridge(
            "inspection operation id must be positive".into(),
        ));
    }

    let mut tokens = inspection_tokens()
        .lock()
        .map_err(|_| FrameScopeError::Bridge("inspection cancellation state is poisoned".into()))?;
    let token = tokens
        .entry(operation_id)
        .or_insert_with(CancellationToken::new)
        .clone();
    Ok((token, InspectionOperation { id: operation_id }))
}

fn cancel_operation(operation_id: OperationId) -> bool {
    if operation_id <= 0 {
        return false;
    }

    let Ok(mut tokens) = inspection_tokens().lock() else {
        return false;
    };

    if tokens.len() >= MAX_PENDING_CANCELLATIONS {
        tokens.retain(|_, token| !token.is_cancelled());
    }
    if tokens.len() >= MAX_PENDING_CANCELLATIONS && !tokens.contains_key(&operation_id) {
        return false;
    }

    let token = tokens
        .entry(operation_id)
        .or_insert_with(CancellationToken::new)
        .clone();
    token.cancel();
    true
}

#[cfg(target_family = "unix")]
fn inspect_fd(fd: i32, operation_id: OperationId) -> Result<InspectionMetadata, FrameScopeError> {
    if fd < 0 {
        return Err(FrameScopeError::Bridge("invalid file descriptor".into()));
    }

    let (cancellation, _operation) = operation_token(operation_id)?;
    // SAFETY: Kotlin keeps the ParcelFileDescriptor open for this JNI call. VideoDecoder duplicates
    // the descriptor immediately and owns only the duplicate, so this borrowed view never closes it.
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let mut decoder = VideoDecoder::open_file_descriptor_with_options(
        borrowed,
        OpenOptions::default(),
        cancellation,
    )?;
    build_inspection_metadata(&mut decoder)
}

#[cfg(not(target_family = "unix"))]
fn inspect_fd(_fd: i32, operation_id: OperationId) -> Result<InspectionMetadata, FrameScopeError> {
    let (_cancellation, _operation) = operation_token(operation_id)?;
    Err(FrameScopeError::Bridge(
        "file-descriptor inspection is only available on Android/Unix targets".into(),
    ))
}

fn build_inspection_metadata(
    decoder: &mut VideoDecoder,
) -> Result<InspectionMetadata, FrameScopeError> {
    let info = decoder.info().clone();
    let selected = decoder.selected_stream().clone();
    let mut decoded_dimensions = None;
    let mut decoded_pixel_format = None;

    for _ in 0..VFR_SAMPLE_FRAMES {
        let Some(frame) = decoder.next_frame()? else {
            break;
        };
        if decoded_dimensions.is_none() {
            decoded_dimensions = Some((frame.width, frame.height));
            decoded_pixel_format = frame.pixel_format;
        }
        if decoder.observed_frame_rate_mode() == ObservedFrameRateMode::Variable {
            break;
        }
    }

    inspection_metadata_from_info(
        &info,
        &selected,
        decoded_dimensions,
        decoded_pixel_format.as_deref(),
        bridge_variable_frame_rate(decoder.observed_frame_rate_mode()),
    )
}

/// A bounded prefix sample can prove that varying presentation intervals were observed, but it
/// cannot prove that the remainder of a source is constant-rate. Keep CFR as unknown unless the
/// complete timeline is inspected by a future indexing layer.
fn bridge_variable_frame_rate(mode: ObservedFrameRateMode) -> Option<bool> {
    match mode {
        ObservedFrameRateMode::Variable => Some(true),
        ObservedFrameRateMode::Undetermined | ObservedFrameRateMode::Constant => None,
    }
}

fn inspection_metadata_from_info(
    info: &VideoInfo,
    selected: &StreamInfo,
    decoded_dimensions: Option<(u32, u32)>,
    decoded_pixel_format: Option<&str>,
    variable_frame_rate: Option<bool>,
) -> Result<InspectionMetadata, FrameScopeError> {
    let width = selected
        .width
        .or_else(|| decoded_dimensions.map(|dimensions| dimensions.0))
        .ok_or_else(|| FrameScopeError::InvalidMetadata("selected stream has no width".into()))?;
    let height = selected
        .height
        .or_else(|| decoded_dimensions.map(|dimensions| dimensions.1))
        .ok_or_else(|| FrameScopeError::InvalidMetadata("selected stream has no height".into()))?;
    if width == 0 || height == 0 || width > 65_535 || height > 65_535 {
        return Err(FrameScopeError::InvalidMetadata(
            "selected stream dimensions are outside Android bridge safety bounds".into(),
        ));
    }

    let rotation_degrees = selected.rotation_degrees.unwrap_or(0).rem_euclid(360);
    if !matches!(rotation_degrees, 0 | 90 | 180 | 270) {
        return Err(FrameScopeError::InvalidMetadata(
            "rotation must resolve to 0, 90, 180, or 270 degrees".into(),
        ));
    }

    let estimated_frame_rate = selected
        .average_frame_rate
        .or(selected.nominal_frame_rate)
        .map(|rate| rate.as_f64())
        .filter(|fps| fps.is_finite() && *fps > 0.0 && *fps <= 1_000.0);

    let duration_us = info.container.duration_us.or_else(|| {
        selected
            .duration
            .and_then(|duration| duration.to_microseconds())
            .and_then(|micros| u64::try_from(micros).ok())
            .filter(|micros| *micros > 0)
    });

    let video_stream_count = u32::try_from(
        info.streams
            .iter()
            .filter(|stream| stream.media_kind == MediaKind::Video)
            .count(),
    )
    .map_err(|_| FrameScopeError::InvalidMetadata("video stream count overflow".into()))?;
    let audio_stream_count = u32::try_from(
        info.streams
            .iter()
            .filter(|stream| stream.media_kind == MediaKind::Audio)
            .count(),
    )
    .map_err(|_| FrameScopeError::InvalidMetadata("audio stream count overflow".into()))?;

    Ok(InspectionMetadata {
        duration_us,
        width,
        height,
        estimated_frame_rate,
        rotation_degrees,
        container: info.container.format_name.clone(),
        codec: selected.codec.name.clone(),
        video_stream_index: selected.index,
        video_stream_count,
        audio_stream_count,
        pixel_format: selected
            .pixel_format
            .clone()
            .or_else(|| decoded_pixel_format.map(str::to_owned)),
        variable_frame_rate,
    })
}

fn response_json(fd: i32, operation_id: OperationId) -> String {
    let response = match inspect_fd(fd, operation_id) {
        Ok(metadata) => InspectResponse::Ok {
            engine: ENGINE_VERSION,
            metadata,
        },
        Err(error) => InspectResponse::Error {
            engine: ENGINE_VERSION,
            code: error.code(),
            message: error.to_string(),
        },
    };
    serde_json::to_string(&response).unwrap_or_else(|_| {
        concat!(
            r#"{"status":"error","engine":"framescope-rust/unknown","code":"bridge_error","#,
            r#""message":"failed to serialize native response"}"#,
        )
        .into()
    })
}

fn panic_json() -> String {
    serde_json::to_string(&InspectResponse::Error {
        engine: ENGINE_VERSION,
        code: "bridge_error",
        message: "native inspection aborted safely after an internal panic".into(),
    })
    .unwrap_or_else(|_| "{\"status\":\"error\"}".into())
}

fn to_jstring(env: &mut JNIEnv<'_>, value: &str) -> jstring {
    match env.new_string(value) {
        Ok(result) => result.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_RustBridge_nativeVersion(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    to_jstring(&mut env, ENGINE_VERSION)
}

/// Build-time/link-time probe used by native verification. It is not part of the Kotlin API.
#[unsafe(no_mangle)]
pub extern "C" fn framescope_ffmpeg_link_probe() -> u32 {
    framescope_video::ffmpeg::link_probe()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_RustBridge_nativeInspectVideoFd(
    mut env: JNIEnv,
    _class: JClass,
    fd: jint,
    operation_id: jlong,
) -> jstring {
    let json = catch_unwind(AssertUnwindSafe(|| response_json(fd, operation_id)))
        .unwrap_or_else(|_| panic_json());
    to_jstring(&mut env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_RustBridge_nativeOpenMicroscopeSession(
    mut env: JNIEnv,
    _class: JClass,
    fd: jint,
    operation_id: jlong,
    cache_root: JString,
) -> jstring {
    let cache_root: String = match env.get_string(&cache_root) {
        Ok(value) => value.into(),
        Err(_) => return ptr::null_mut(),
    };
    let json = catch_unwind(AssertUnwindSafe(|| {
        microscope::open_response(fd, operation_id, &cache_root)
    }))
    .unwrap_or_else(|_| microscope::panic_response());
    to_jstring(&mut env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_RustBridge_nativeStepMicroscope(
    mut env: JNIEnv,
    _class: JClass,
    session_id: jlong,
    delta: jint,
) -> jstring {
    let json = catch_unwind(AssertUnwindSafe(|| {
        microscope::step_response(session_id, delta)
    }))
    .unwrap_or_else(|_| microscope::panic_response());
    to_jstring(&mut env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_RustBridge_nativeJumpMicroscopeFrame(
    mut env: JNIEnv,
    _class: JClass,
    session_id: jlong,
    frame_id: jlong,
) -> jstring {
    let json = catch_unwind(AssertUnwindSafe(|| {
        microscope::jump_frame_response(session_id, frame_id)
    }))
    .unwrap_or_else(|_| microscope::panic_response());
    to_jstring(&mut env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_RustBridge_nativeJumpMicroscopeTimestampUs(
    mut env: JNIEnv,
    _class: JClass,
    session_id: jlong,
    timestamp_us: jlong,
    selection: jint,
) -> jstring {
    let json = catch_unwind(AssertUnwindSafe(|| {
        microscope::jump_timestamp_response(session_id, timestamp_us, selection)
    }))
    .unwrap_or_else(|_| microscope::panic_response());
    to_jstring(&mut env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_RustBridge_nativeCloseMicroscopeSession(
    _env: JNIEnv,
    _class: JClass,
    session_id: jlong,
) -> jboolean {
    let closed = catch_unwind(AssertUnwindSafe(|| microscope::close_session(session_id)))
        .unwrap_or(false);
    if closed { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_RustBridge_nativeCancelInspection(
    _env: JNIEnv,
    _class: JClass,
    operation_id: jlong,
) -> jboolean {
    let cancelled =
        catch_unwind(AssertUnwindSafe(|| cancel_operation(operation_id))).unwrap_or(false);
    if cancelled { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_core::{CodecInfo, ContainerInfo, MediaDuration, Rational, TimeBase};

    #[test]
    fn invalid_fd_is_reported_as_bridge_error() {
        let json = response_json(-1, 1);
        assert!(json.contains("bridge_error"));
        assert!(json.contains("framescope-rust"));
    }

    #[test]
    fn cancellation_requested_before_native_start_is_preserved() {
        let operation_id = 9_001;
        assert!(cancel_operation(operation_id));
        let (token, operation) = operation_token(operation_id).unwrap();
        assert!(token.is_cancelled());
        drop(operation);
        assert!(
            !inspection_tokens()
                .lock()
                .unwrap()
                .contains_key(&operation_id)
        );
    }

    #[test]
    fn bounded_cadence_sample_never_claims_global_cfr() {
        assert_eq!(
            bridge_variable_frame_rate(ObservedFrameRateMode::Undetermined),
            None
        );
        assert_eq!(
            bridge_variable_frame_rate(ObservedFrameRateMode::Constant),
            None
        );
        assert_eq!(
            bridge_variable_frame_rate(ObservedFrameRateMode::Variable),
            Some(true)
        );
    }

    #[test]
    fn projects_phase_two_engine_metadata_for_android() {
        let time_base = TimeBase::new(1, 1_000).unwrap();
        let selected = StreamInfo {
            index: 2,
            media_kind: MediaKind::Video,
            codec: CodecInfo {
                id: 27,
                name: "h264".into(),
                decoder_available: true,
            },
            is_default: true,
            time_base: Some(time_base),
            duration: Some(MediaDuration {
                ticks: 2_000,
                time_base,
            }),
            frame_count: Some(60),
            width: Some(1_920),
            height: Some(1_080),
            pixel_format: Some("yuv420p".into()),
            average_frame_rate: Some(Rational::new(30, 1).unwrap()),
            nominal_frame_rate: None,
            rotation_degrees: Some(-90),
        };
        let info = VideoInfo {
            container: ContainerInfo {
                format_name: "mov,mp4,m4a,3gp,3g2,mj2".into(),
                format_long_name: Some("QuickTime / MOV".into()),
                duration_us: None,
                stream_count: 2,
            },
            streams: vec![
                selected.clone(),
                StreamInfo {
                    index: 3,
                    media_kind: MediaKind::Audio,
                    codec: CodecInfo {
                        id: 86_018,
                        name: "aac".into(),
                        decoder_available: false,
                    },
                    is_default: true,
                    time_base: None,
                    duration: None,
                    frame_count: None,
                    width: None,
                    height: None,
                    pixel_format: None,
                    average_frame_rate: None,
                    nominal_frame_rate: None,
                    rotation_degrees: None,
                },
            ],
            selected_video_stream: 2,
        };

        let metadata = inspection_metadata_from_info(
            &info,
            &selected,
            None,
            None,
            bridge_variable_frame_rate(ObservedFrameRateMode::Constant),
        )
        .unwrap();
        assert_eq!(metadata.duration_us, Some(2_000_000));
        assert_eq!(metadata.width, 1_920);
        assert_eq!(metadata.height, 1_080);
        assert_eq!(metadata.codec, "h264");
        assert_eq!(metadata.video_stream_index, 2);
        assert_eq!(metadata.video_stream_count, 1);
        assert_eq!(metadata.audio_stream_count, 1);
        assert_eq!(metadata.rotation_degrees, 270);
        assert_eq!(metadata.variable_frame_rate, None);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn host_ffmpeg_link_probe_is_unavailable() {
        assert_eq!(framescope_ffmpeg_link_probe(), 0);
    }
}
