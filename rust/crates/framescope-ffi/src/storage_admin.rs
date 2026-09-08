use framescope_cache::{
    FrameScopeStorageStats, StorageAdmin, StorageAdminError, StorageClearReport, StorageClearScope,
};
use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jint, jstring};
use serde::Serialize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use crate::{ENGINE_VERSION, to_jstring};

const MAX_CACHE_ROOT_LENGTH: usize = 4_096;
const MAX_SOURCE_KEY_LENGTH: usize = 256;
// The current microscope configuration intentionally gives disk proxies a zero-byte budget.
// Keep the storage report truthful until that production budget changes.
const PREVIEW_PROXY_ENABLED: bool = false;

const CLEAR_PREVIEW_PROXY: i32 = 0;
const CLEAR_PERSISTENT_INDEXES: i32 = 1;
const CLEAR_DISPOSABLE: i32 = 2;
const CLEAR_ALL: i32 = 3;

#[derive(Debug, Serialize)]
struct StorageClearDetails {
    scope: StorageClearScope,
    cleared_bytes: u64,
    cleared_files: u64,
    cleared_items: u64,
}

impl StorageClearDetails {
    fn new(scope: StorageClearScope, report: StorageClearReport) -> Self {
        Self {
            scope,
            cleared_bytes: report.cleared_bytes,
            cleared_files: report.cleared_files,
            cleared_items: report.cleared_items,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum StorageResponse {
    Ok {
        engine: &'static str,
        storage: FrameScopeStorageStats,
        #[serde(skip_serializing_if = "Option::is_none")]
        cleared: Option<StorageClearDetails>,
    },
    Error {
        engine: &'static str,
        code: &'static str,
        message: String,
    },
}

fn validate_cache_root(cache_root: &str) -> Result<(), StorageResponse> {
    if cache_root.is_empty() || cache_root.len() > MAX_CACHE_ROOT_LENGTH || cache_root.contains('\0') {
        return Err(StorageResponse::Error {
            engine: ENGINE_VERSION,
            code: "invalid_cache_root",
            message: "FrameScope cache root is invalid".into(),
        });
    }
    Ok(())
}

fn admin(cache_root: &str) -> Result<StorageAdmin, StorageResponse> {
    validate_cache_root(cache_root)?;
    Ok(StorageAdmin::new(cache_root, PREVIEW_PROXY_ENABLED))
}

fn from_admin_error(error: StorageAdminError) -> StorageResponse {
    let code = match error {
        StorageAdminError::InvalidSourceKey => "invalid_source_key",
        StorageAdminError::RootIsSymlink(_) => "unsafe_cache_root",
        StorageAdminError::LockPoisoned => "storage_busy",
        StorageAdminError::NumericOverflow => "storage_overflow",
        StorageAdminError::Io { .. } => "storage_io",
    };
    StorageResponse::Error {
        engine: ENGINE_VERSION,
        code,
        message: error.to_string(),
    }
}

fn stats_response(cache_root: &str) -> String {
    let response = match admin(cache_root) {
        Ok(admin) => match admin.stats() {
            Ok(storage) => StorageResponse::Ok {
                engine: ENGINE_VERSION,
                storage,
                cleared: None,
            },
            Err(error) => from_admin_error(error),
        },
        Err(response) => response,
    };
    serialize_response(response)
}

fn clear_response(cache_root: &str, scope_code: i32) -> String {
    let scope = match scope_code {
        CLEAR_PREVIEW_PROXY => StorageClearScope::PreviewProxy,
        CLEAR_PERSISTENT_INDEXES => StorageClearScope::PersistentIndexes,
        CLEAR_DISPOSABLE => StorageClearScope::Disposable,
        CLEAR_ALL => StorageClearScope::All,
        _ => {
            return serialize_response(StorageResponse::Error {
                engine: ENGINE_VERSION,
                code: "invalid_clear_scope",
                message: "unknown FrameScope storage clear scope".into(),
            });
        }
    };
    let response = match admin(cache_root) {
        Ok(admin) => match admin.clear(scope) {
            Ok(report) => match admin.stats() {
                Ok(storage) => StorageResponse::Ok {
                    engine: ENGINE_VERSION,
                    storage,
                    cleared: Some(StorageClearDetails::new(scope, report)),
                },
                Err(error) => from_admin_error(error),
            },
            Err(error) => from_admin_error(error),
        },
        Err(response) => response,
    };
    serialize_response(response)
}

fn clear_source_response(cache_root: &str, source_key: &str) -> String {
    if source_key.is_empty() || source_key.len() > MAX_SOURCE_KEY_LENGTH {
        return serialize_response(StorageResponse::Error {
            engine: ENGINE_VERSION,
            code: "invalid_source_key",
            message: "frame-index source key is invalid".into(),
        });
    }
    let response = match admin(cache_root) {
        Ok(admin) => match admin.clear_source_indexes(source_key) {
            Ok(report) => match admin.stats() {
                Ok(storage) => StorageResponse::Ok {
                    engine: ENGINE_VERSION,
                    storage,
                    cleared: Some(StorageClearDetails::new(
                        StorageClearScope::PersistentIndexes,
                        report,
                    )),
                },
                Err(error) => from_admin_error(error),
            },
            Err(error) => from_admin_error(error),
        },
        Err(response) => response,
    };
    serialize_response(response)
}

fn serialize_response(response: StorageResponse) -> String {
    serde_json::to_string(&response).unwrap_or_else(|_| {
        format!(
            "{{\"status\":\"error\",\"engine\":\"{ENGINE_VERSION}\",\"code\":\"bridge_error\",\"message\":\"failed to serialize storage response\"}}"
        )
    })
}

fn panic_response() -> String {
    serialize_response(StorageResponse::Error {
        engine: ENGINE_VERSION,
        code: "bridge_error",
        message: "native storage administration aborted safely after an internal panic".into(),
    })
}

fn jstring_value(env: &mut JNIEnv<'_>, value: JString<'_>) -> Result<String, ()> {
    env.get_string(&value).map(|value| value.into()).map_err(|_| ())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_FrameScopeStorageBridge_nativeStorageStats(
    mut env: JNIEnv,
    _class: JClass,
    cache_root: JString,
) -> jstring {
    let cache_root = match jstring_value(&mut env, cache_root) {
        Ok(value) => value,
        Err(()) => return ptr::null_mut(),
    };
    let json = catch_unwind(AssertUnwindSafe(|| stats_response(&cache_root)))
        .unwrap_or_else(|_| panic_response());
    to_jstring(&mut env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_FrameScopeStorageBridge_nativeClearStorage(
    mut env: JNIEnv,
    _class: JClass,
    cache_root: JString,
    scope: jint,
) -> jstring {
    let cache_root = match jstring_value(&mut env, cache_root) {
        Ok(value) => value,
        Err(()) => return ptr::null_mut(),
    };
    let json = catch_unwind(AssertUnwindSafe(|| clear_response(&cache_root, scope)))
        .unwrap_or_else(|_| panic_response());
    to_jstring(&mut env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_FrameScopeStorageBridge_nativeClearSourceIndexes(
    mut env: JNIEnv,
    _class: JClass,
    cache_root: JString,
    source_key: JString,
) -> jstring {
    let cache_root = match jstring_value(&mut env, cache_root) {
        Ok(value) => value,
        Err(()) => return ptr::null_mut(),
    };
    let source_key = match jstring_value(&mut env, source_key) {
        Ok(value) => value,
        Err(()) => return ptr::null_mut(),
    };
    let json = catch_unwind(AssertUnwindSafe(|| {
        clear_source_response(&cache_root, &source_key)
    }))
    .unwrap_or_else(|_| panic_response());
    to_jstring(&mut env, &json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_scope_fails_closed() {
        let json = clear_response("/tmp/framescope-storage-ffi", 99);
        assert!(json.contains("invalid_clear_scope"));
    }

    #[test]
    fn invalid_root_fails_before_touching_storage() {
        let json = stats_response("");
        assert!(json.contains("invalid_cache_root"));
    }

    #[test]
    fn traversal_source_key_is_rejected() {
        let json = clear_source_response("/tmp/framescope-storage-ffi", "../outside");
        assert!(json.contains("invalid_source_key"));
    }
}
