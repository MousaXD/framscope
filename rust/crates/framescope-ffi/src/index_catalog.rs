use framescope_cache::{FrameIndexCatalog, FrameIndexCatalogError, PersistentFrameIndexDescriptor};
use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::jstring;
use serde::Serialize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use crate::{ENGINE_VERSION, to_jstring};

const MAX_CACHE_ROOT_LENGTH: usize = 4_096;

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum IndexCatalogResponse {
    Ok {
        engine: &'static str,
        indexes: Vec<PersistentFrameIndexDescriptor>,
    },
    Error {
        engine: &'static str,
        code: &'static str,
        message: String,
    },
}

fn catalog_response(cache_root: &str) -> String {
    let response = if cache_root.is_empty()
        || cache_root.len() > MAX_CACHE_ROOT_LENGTH
        || cache_root.contains('\0')
    {
        IndexCatalogResponse::Error {
            engine: ENGINE_VERSION,
            code: "invalid_cache_root",
            message: "FrameScope cache root is invalid".into(),
        }
    } else {
        match FrameIndexCatalog::new(cache_root).entries() {
            Ok(indexes) => IndexCatalogResponse::Ok {
                engine: ENGINE_VERSION,
                indexes,
            },
            Err(error) => from_catalog_error(error),
        }
    };
    serialize_response(response)
}

fn from_catalog_error(error: FrameIndexCatalogError) -> IndexCatalogResponse {
    let code = match &error {
        FrameIndexCatalogError::RootIsSymlink(_) => "unsafe_cache_root",
        FrameIndexCatalogError::Io { .. } => "storage_io",
    };
    IndexCatalogResponse::Error {
        engine: ENGINE_VERSION,
        code,
        message: error.to_string(),
    }
}

fn serialize_response(response: IndexCatalogResponse) -> String {
    serde_json::to_string(&response).unwrap_or_else(|_| {
        format!(
            "{{\"status\":\"error\",\"engine\":\"{ENGINE_VERSION}\",\"code\":\"bridge_error\",\"message\":\"failed to serialize frame-index catalog\"}}"
        )
    })
}

fn panic_response() -> String {
    serialize_response(IndexCatalogResponse::Error {
        engine: ENGINE_VERSION,
        code: "bridge_error",
        message: "native frame-index catalog aborted safely after an internal panic".into(),
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_FrameIndexCatalogBridge_nativeIndexCatalog(
    mut env: JNIEnv,
    _class: JClass,
    cache_root: JString,
) -> jstring {
    let cache_root = match env.get_string(&cache_root) {
        Ok(value) => String::from(value),
        Err(_) => return ptr::null_mut(),
    };
    let json = catch_unwind(AssertUnwindSafe(|| catalog_response(&cache_root)))
        .unwrap_or_else(|_| panic_response());
    to_jstring(&mut env, &json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_root_fails_closed() {
        let json = catalog_response("");
        assert!(json.contains("invalid_cache_root"));
    }

    #[test]
    fn missing_root_returns_empty_catalog() {
        let root = std::env::temp_dir().join(format!(
            "framescope-missing-index-catalog-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let json = catalog_response(&root.to_string_lossy());
        assert!(json.contains("\"status\":\"ok\""));
        assert!(json.contains("\"indexes\":[]"));
    }
}
