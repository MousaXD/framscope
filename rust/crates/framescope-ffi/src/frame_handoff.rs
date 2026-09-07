use jni::JNIEnv;
use jni::objects::{JByteBuffer, JClass};
use jni::sys::{jlong, jstring};
use std::panic::{AssertUnwindSafe, catch_unwind};

use super::{microscope, to_jstring};

const COPY_INVALID_REQUEST: jlong = -1;
const COPY_SESSION_NOT_FOUND: jlong = -2;
const COPY_STALE_GENERATION: jlong = -3;
const COPY_NO_PREPARED_FRAME: jlong = -4;
const COPY_BUFFER_TOO_SMALL: jlong = -5;
const COPY_BRIDGE_ERROR: jlong = -6;
const COPY_INVALID_BUFFER: jlong = -7;

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
    let address = match env.get_direct_buffer_address(destination) {
        Ok(address) => address,
        Err(_) => return COPY_INVALID_BUFFER,
    };

    // SAFETY: JNI guarantees that GetDirectBufferAddress points to `capacity` writable bytes for
    // this direct ByteBuffer. The local `JByteBuffer` reference remains alive for the entire slice
    // lifetime, and the slice is never stored or returned beyond this synchronous JNI call.
    let destination = unsafe { std::slice::from_raw_parts_mut(address, capacity) };
    match microscope::copy_prepared_frame(session_id, generation, destination) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_error_codes_are_stable_for_kotlin_boundary() {
        assert_eq!(copy_error_code("invalid_request"), -1);
        assert_eq!(copy_error_code("session_not_found"), -2);
        assert_eq!(copy_error_code("stale_generation"), -3);
        assert_eq!(copy_error_code("no_prepared_frame"), -4);
        assert_eq!(copy_error_code("buffer_too_small"), -5);
        assert_eq!(copy_error_code("timeline_mismatch"), -6);
    }
}
