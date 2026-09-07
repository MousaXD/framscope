use serde::{Deserialize, Serialize};
use std::io::{self, Read, Seek, SeekFrom};

const SAMPLE_BYTES: u64 = 64 * 1024;
const SAMPLE_DOMAIN: &[u8] = b"framescope-source-sample-v1\0";
const KEY_DOMAIN: &[u8] = b"framescope-source-v2\0";

/// Path-independent identity for a media source.
///
/// `provider_document_id` is useful metadata, but it is never treated as proof that content is
/// unchanged. Persisted indexes are reusable only when a content-derived tag is also available.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceIdentity {
    pub size_bytes: Option<u64>,
    pub modified_time_ms: Option<u64>,
    pub provider_document_id: Option<String>,
    /// Content-derived identity evidence. [`SourceIdentity::from_seekable`] produces a bounded
    /// BLAKE3 sample tag; callers may also supply another strong content-derived tag.
    pub content_tag: Option<String>,
}

/// Backwards-compatible Phase 2 name.
pub type SourceVideoIdentity = SourceIdentity;

impl SourceIdentity {
    /// Preserve the Phase 2 constructor while upgrading the representation to support unknown size
    /// and provider/document metadata.
    pub fn new(
        size_bytes: u64,
        modified_time_ms: Option<u64>,
        content_tag: Option<String>,
    ) -> Self {
        Self {
            size_bytes: Some(size_bytes),
            modified_time_ms,
            provider_document_id: None,
            content_tag,
        }
    }

    pub fn metadata_only(
        size_bytes: Option<u64>,
        modified_time_ms: Option<u64>,
        provider_document_id: Option<String>,
    ) -> Self {
        Self {
            size_bytes,
            modified_time_ms,
            provider_document_id,
            content_tag: None,
        }
    }

    pub fn with_provider_document_id(mut self, provider_document_id: Option<String>) -> Self {
        self.provider_document_id = provider_document_id;
        self
    }

    pub fn with_content_tag(mut self, content_tag: Option<String>) -> Self {
        self.content_tag = content_tag;
        self
    }

    /// Build a strong reusable identity without hashing the entire source.
    ///
    /// At most three 64 KiB windows are read: beginning, middle, and end. The reader position is
    /// restored before returning. This helper is for a seekable source owned by the caller; it must
    /// not be run concurrently against another consumer sharing the same underlying file offset.
    pub fn from_seekable<R: Read + Seek>(
        reader: &mut R,
        modified_time_ms: Option<u64>,
        provider_document_id: Option<String>,
    ) -> io::Result<Self> {
        let original_position = reader.stream_position()?;
        let sampled = sample_seekable(reader);
        let restore = reader.seek(SeekFrom::Start(original_position));

        let (size_bytes, content_tag) = match sampled {
            Ok(value) => value,
            Err(error) => {
                let _ = restore;
                return Err(error);
            }
        };
        restore?;

        Ok(Self {
            size_bytes: Some(size_bytes),
            modified_time_ms,
            provider_document_id,
            content_tag: Some(content_tag),
        })
    }

    /// Persisted indexes are reused only when identity includes content-derived evidence.
    ///
    /// Filename, URI text, display name, provider document ID, size, and modification time are not
    /// sufficient on their own. If this returns false the index layer rebuilds instead of risking a
    /// stale match.
    pub fn is_reuse_safe(&self) -> bool {
        self.size_bytes.is_some()
            && self
                .content_tag
                .as_deref()
                .is_some_and(|tag| !tag.trim().is_empty())
    }

    /// Stable path-safe key used to namespace index/cache storage.
    pub fn stable_key(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(KEY_DOMAIN);
        hash_optional_u64(&mut hasher, self.size_bytes);
        hash_optional_u64(&mut hasher, self.modified_time_ms);
        hash_optional_text(&mut hasher, self.provider_document_id.as_deref());
        hash_optional_text(&mut hasher, self.content_tag.as_deref());
        hasher.finalize().to_hex().to_string()
    }
}

fn sample_seekable<R: Read + Seek>(reader: &mut R) -> io::Result<(u64, String)> {
    let size = reader.seek(SeekFrom::End(0))?;
    let sample_len = size.min(SAMPLE_BYTES);
    let mut offsets = Vec::with_capacity(3);
    offsets.push(0);
    if size > sample_len {
        offsets.push((size / 2).saturating_sub(sample_len / 2));
        offsets.push(size - sample_len);
    }
    offsets.sort_unstable();
    offsets.dedup();

    let mut hasher = blake3::Hasher::new();
    hasher.update(SAMPLE_DOMAIN);
    hasher.update(&size.to_le_bytes());
    let mut buffer = vec![0_u8; usize::try_from(sample_len).unwrap_or(0)];

    for offset in offsets {
        reader.seek(SeekFrom::Start(offset))?;
        if !buffer.is_empty() {
            reader.read_exact(&mut buffer)?;
        }
        hasher.update(&offset.to_le_bytes());
        hasher.update(&(buffer.len() as u64).to_le_bytes());
        hasher.update(&buffer);
    }

    Ok((
        size,
        format!("blake3-sample-v1:{}", hasher.finalize().to_hex()),
    ))
}

fn hash_optional_u64(hasher: &mut blake3::Hasher, value: Option<u64>) {
    match value {
        Some(value) => {
            hasher.update(&[1]);
            hasher.update(&value.to_le_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
}

fn hash_optional_text(hasher: &mut blake3::Hasher, value: Option<&str>) {
    match value {
        Some(value) => {
            hasher.update(&[1]);
            hasher.update(&(value.len() as u64).to_le_bytes());
            hasher.update(value.as_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
}

/// Versioned, path-safe namespace layout for persistent indexes and future payload caches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheDirectoryLayout {
    schema_version: u32,
}

impl CacheDirectoryLayout {
    pub fn new(schema_version: u32) -> Option<Self> {
        (schema_version > 0).then_some(Self { schema_version })
    }

    pub fn source_namespace(&self, source: &SourceIdentity) -> String {
        format!("v{}/{}", self.schema_version, source.stable_key())
    }
}

/// Boundary shared by future bounded RAM and disk cache implementations.
pub trait CacheStore {
    type Error;

    fn invalidate_source(&mut self, source: &SourceIdentity) -> Result<(), Self::Error>;
    fn clear(&mut self) -> Result<(), Self::Error>;
}

/// Real no-op store useful for wiring/tests and future optional payload caching.
#[derive(Debug, Default)]
pub struct NoopCacheStore;

impl CacheStore for NoopCacheStore {
    type Error = std::convert::Infallible;

    fn invalidate_source(&mut self, _source: &SourceIdentity) -> Result<(), Self::Error> {
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn stable_key_is_deterministic() {
        let id = SourceIdentity::new(1234, Some(5678), Some("sample".into()));
        assert_eq!(id.stable_key(), id.stable_key());
    }

    #[test]
    fn stable_key_changes_when_source_changes() {
        let a = SourceIdentity::new(1234, Some(5678), Some("a".into()));
        let b = SourceIdentity::new(1234, Some(5678), Some("b".into()));
        assert_ne!(a.stable_key(), b.stable_key());
    }

    #[test]
    fn metadata_only_identity_is_not_reuse_safe() {
        let id = SourceIdentity::metadata_only(Some(1234), Some(5678), Some("doc:1".into()));
        assert!(!id.is_reuse_safe());
    }

    #[test]
    fn sampled_identity_is_bounded_and_restores_position() {
        let bytes = (0..400_000).map(|value| (value % 251) as u8).collect::<Vec<_>>();
        let mut cursor = Cursor::new(bytes);
        cursor.seek(SeekFrom::Start(123)).unwrap();
        let id = SourceIdentity::from_seekable(&mut cursor, Some(7), Some("doc:7".into())).unwrap();
        assert_eq!(cursor.stream_position().unwrap(), 123);
        assert_eq!(id.size_bytes, Some(400_000));
        assert!(id.is_reuse_safe());
        assert!(id.content_tag.unwrap().starts_with("blake3-sample-v1:"));
    }

    #[test]
    fn sampled_identity_detects_changed_content_with_same_size() {
        let mut a = vec![0_u8; 300_000];
        let mut b = a.clone();
        a[150_000] = 1;
        b[150_000] = 2;
        let id_a = SourceIdentity::from_seekable(&mut Cursor::new(a), None, None).unwrap();
        let id_b = SourceIdentity::from_seekable(&mut Cursor::new(b), None, None).unwrap();
        assert_ne!(id_a.content_tag, id_b.content_tag);
    }

    #[test]
    fn cache_directory_namespace_is_versioned_and_path_safe() {
        let layout = CacheDirectoryLayout::new(1).unwrap();
        let id = SourceIdentity::new(1234, Some(5678), Some("sample".into()));
        let namespace = layout.source_namespace(&id);
        assert!(namespace.starts_with("v1/"));
        assert_eq!(namespace.len(), 3 + 64);
        assert!(namespace[3..].chars().all(|value| value.is_ascii_hexdigit()));
    }

    #[test]
    fn cache_directory_layout_rejects_zero_schema_version() {
        assert!(CacheDirectoryLayout::new(0).is_none());
    }

    #[test]
    fn noop_store_honors_contract() {
        let mut store = NoopCacheStore;
        let id = SourceIdentity::new(1, None, None);
        assert!(store.invalidate_source(&id).is_ok());
        assert!(store.clear().is_ok());
    }
}
