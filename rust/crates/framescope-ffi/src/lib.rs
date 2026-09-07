//! Narrow JNI boundary for the Android app.

use framescope_core::{FrameScopeError, VideoMetadata};
use jni::JNIEnv;
use jni::objects::JClass;
use jni::sys::{jint, jstring};
use serde::Serialize;
#[cfg(target_family = "unix")]
use std::fs::File;
#[cfg(target_family = "unix")]
use std::os::fd::FromRawFd;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

const ENGINE_VERSION: &str = concat!("framescope-rust/", env!("CARGO_PKG_VERSION"));

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum InspectResponse {
    Ok {
        engine: &'static str,
        metadata: VideoMetadata,
    },
    Error {
        engine: &'static str,
        code: &'static str,
        message: String,
    },
}

#[cfg(target_family = "unix")]
fn inspect_fd(fd: i32) -> Result<VideoMetadata, FrameScopeError> {
    if fd < 0 {
        return Err(FrameScopeError::Bridge("invalid file descriptor".into()));
    }

    // SAF owns the original descriptor. Duplicate it so Rust can safely own/close its copy.
    // SAFETY: `dup` accepts any integer descriptor and reports invalid descriptors with -1.
    let duplicated = unsafe { libc::dup(fd) };
    if duplicated < 0 {
        return Err(FrameScopeError::Bridge(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    // SAFETY: `duplicated` is a fresh, successful `dup`; this `File` becomes its sole owner.
    let mut file = unsafe { File::from_raw_fd(duplicated) };
    framescope_video::inspect_video(&mut file)
}

#[cfg(not(target_family = "unix"))]
fn inspect_fd(_fd: i32) -> Result<VideoMetadata, FrameScopeError> {
    Err(FrameScopeError::Bridge(
        "file-descriptor inspection is only available on Android/Unix targets".into(),
    ))
}

fn response_json(fd: i32) -> String {
    let response = match inspect_fd(fd) {
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

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_RustBridge_nativeInspectVideoFd(
    mut env: JNIEnv,
    _class: JClass,
    fd: jint,
) -> jstring {
    let json = catch_unwind(AssertUnwindSafe(|| response_json(fd)))
        .unwrap_or_else(|_| panic_json());
    to_jstring(&mut env, &json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_fd_is_reported_as_bridge_error() {
        let json = response_json(-1);
        assert!(json.contains("bridge_error"));
        assert!(json.contains("framescope-rust"));
    }
}
