//! Cache identity primitives only. Frame payload caching is intentionally Phase 3 work.

use serde::{Deserialize, Serialize};

/// Path-independent facts usable to identify a source supplied through Android's SAF.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceVideoIdentity {
    pub size_bytes: u64,
    pub modified_time_ms: Option<u64>,
    /// Optional content-derived tag, intended for a bounded source fingerprint in a later phase.
    pub content_tag: Option<String>,
}

impl SourceVideoIdentity {
    pub fn new(
        size_bytes: u64,
        modified_time_ms: Option<u64>,
        content_tag: Option<String>,
    ) -> Self {
        Self {
            size_bytes,
            modified_time_ms,
            content_tag,
        }
    }

    /// Stable textual key suitable for namespacing cache directories.
    pub fn stable_key(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"framescope-source-v1\0");
        hasher.update(&self.size_bytes.to_le_bytes());
        match self.modified_time_ms {
            Some(value) => {
                hasher.update(&[1]);
                hasher.update(&value.to_le_bytes());
            }
            None => {
                hasher.update(&[0]);
            }
        }
        match &self.content_tag {
            Some(value) => {
                hasher.update(&[1]);
                hasher.update(value.as_bytes());
            }
            None => {
                hasher.update(&[0]);
            }
        }
        hasher.finalize().to_hex().to_string()
    }
}

/// Versioned, path-safe namespace layout for future on-disk cache directories.
///
/// Android owns the absolute cache root. Rust only derives the relative namespace so the
/// media/cache layer never needs an Android `Context` or filesystem permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheDirectoryLayout {
    schema_version: u32,
}

impl CacheDirectoryLayout {
    pub fn new(schema_version: u32) -> Option<Self> {
        (schema_version > 0).then_some(Self { schema_version })
    }

    pub fn source_namespace(&self, source: &SourceVideoIdentity) -> String {
        format!("v{}/{}", self.schema_version, source.stable_key())
    }
}

/// Boundary shared by future bounded RAM and disk cache implementations.
pub trait CacheStore {
    type Error;

    fn invalidate_source(&mut self, source: &SourceVideoIdentity) -> Result<(), Self::Error>;
    fn clear(&mut self) -> Result<(), Self::Error>;
}

/// Real no-op store useful for tests and for wiring phases before payload caching exists.
#[derive(Debug, Default)]
pub struct NoopCacheStore;

impl CacheStore for NoopCacheStore {
    type Error = std::convert::Infallible;

    fn invalidate_source(&mut self, _source: &SourceVideoIdentity) -> Result<(), Self::Error> {
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_key_is_deterministic() {
        let id = SourceVideoIdentity::new(1234, Some(5678), Some("sample".into()));
        let first = id.stable_key();
        let second = id.stable_key();
        assert_eq!(first, second);
    }

    #[test]
    fn stable_key_changes_when_source_changes() {
        let a = SourceVideoIdentity::new(1234, Some(5678), None);
        let b = SourceVideoIdentity::new(1235, Some(5678), None);
        assert_ne!(a.stable_key(), b.stable_key());
    }

    #[test]
    fn cache_directory_namespace_is_versioned_and_path_safe() {
        let layout = CacheDirectoryLayout::new(1).unwrap();
        let id = SourceVideoIdentity::new(1234, Some(5678), Some("sample".into()));
        let namespace = layout.source_namespace(&id);
        assert!(namespace.starts_with("v1/"));
        assert_eq!(namespace.len(), 3 + 64);
        assert!(
            namespace[3..]
                .chars()
                .all(|value| value.is_ascii_hexdigit())
        );
    }

    #[test]
    fn cache_directory_layout_rejects_zero_schema_version() {
        assert!(CacheDirectoryLayout::new(0).is_none());
    }

    #[test]
    fn noop_store_honors_contract() {
        let mut store = NoopCacheStore;
        let id = SourceVideoIdentity::new(1, None, None);
        assert!(store.invalidate_source(&id).is_ok());
        assert!(store.clear().is_ok());
    }
}
