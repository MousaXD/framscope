use serde::{Deserialize, Serialize};
use std::io::{self, Read, Seek, SeekFrom};

const FULL_HASH_BUFFER_BYTES: usize = 256 * 1024;
const FULL_HASH_DOMAIN: &[u8] = b"framescope-source-full-v2\0";
const FULL_HASH_TAG_PREFIX: &str = "blake3-full-v2:";
const LEGACY_SAMPLE_TAG_PREFIX: &str = "blake3-sample-v1:";
const KEY_DOMAIN: &[u8] = b"framescope-source-v3\0";

/// Path-independent identity for a media source.
///
/// `provider_document_id` is useful metadata, but it is never treated as proof that content is
/// unchanged. Persisted indexes are reusable only when a complete collision-resistant
/// content-derived tag is also available.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceIdentity {
    pub size_bytes: Option<u64>,
    pub modified_time_ms: Option<u64>,
    pub provider_document_id: Option<String>,
    /// Complete content-derived identity evidence. [`SourceIdentity::from_seekable`] produces a
    /// whole-source BLAKE3 tag. Callers that supply another tag are responsible for ensuring that it
    /// is collision-resistant evidence over the complete logical source, not sparse sampling.
    pub content_tag: Option<String>,
}

/// Backwards-compatible Phase 2 name.
pub type SourceVideoIdentity = SourceIdentity;

impl SourceIdentity {
    /// Preserve the Phase 2 constructor while upgrading the representation to support unknown size
    /// and provider/document metadata.
    ///
    /// A caller-supplied `content_tag` is trusted only as an explicit complete-content proof. Legacy
    /// FrameScope sparse-sample tags are rejected by [`SourceIdentity::is_reuse_safe`].
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

    /// Build a reusable identity by hashing the complete logical source with BLAKE3.
    ///
    /// The reader position is restored before returning. This helper is for a seekable source owned
    /// by the caller; it must not be run concurrently against another consumer sharing the same
    /// underlying file offset. Android's microscope path uses `pread`, so hashing does not disturb
    /// the decoder descriptor offset even though it intentionally reads the whole source.
    pub fn from_seekable<R: Read + Seek>(
        reader: &mut R,
        modified_time_ms: Option<u64>,
        provider_document_id: Option<String>,
    ) -> io::Result<Self> {
        let original_position = reader.stream_position()?;
        let hashed = hash_complete_seekable(reader);
        let restore = reader.seek(SeekFrom::Start(original_position));

        let (size_bytes, content_tag) = match hashed {
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

    /// Persisted indexes are reused only when identity includes complete content-derived evidence.
    ///
    /// Filename, URI text, display name, provider document ID, size, modification time, and the old
    /// bounded `blake3-sample-v1` identity are not sufficient. If this returns false the index layer
    /// rebuilds instead of risking a stale match.
    pub fn is_reuse_safe(&self) -> bool {
        self.size_bytes.is_some()
            && self
                .content_tag
                .as_deref()
                .is_some_and(is_complete_content_tag)
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

fn is_complete_content_tag(tag: &str) -> bool {
    let tag = tag.trim();
    !tag.is_empty() && !tag.starts_with(LEGACY_SAMPLE_TAG_PREFIX)
}

fn hash_complete_seekable<R: Read + Seek>(reader: &mut R) -> io::Result<(u64, String)> {
    let size = reader.seek(SeekFrom::End(0))?;
    reader.seek(SeekFrom::Start(0))?;

    let mut hasher = blake3::Hasher::new();
    hasher.update(FULL_HASH_DOMAIN);
    hasher.update(&size.to_le_bytes());

    let mut buffer = vec![0_u8; FULL_HASH_BUFFER_BYTES];
    let mut total_read = 0_u64;
    while total_read < size {
        let remaining = size - total_read;
        let wanted = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let read = reader.read(&mut buffer[..wanted])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "source ended while computing complete reusable identity",
            ));
        }
        hasher.update(&buffer[..read]);
        total_read = total_read
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "source size overflow"))?;
    }

    Ok((
        size,
        format!("{FULL_HASH_TAG_PREFIX}{}", hasher.finalize().to_hex()),
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

/// Real no-op store useful for wiring/tests before payload caching exists.
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
        let id = SourceIdentity::new(1234, Some(5678), Some("complete-proof".into()));
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
    fn legacy_sparse_sample_identity_is_not_reuse_safe() {
        let id = SourceIdentity::new(
            1234,
            None,
            Some("blake3-sample-v1:0123456789abcdef".into()),
        );
        assert!(!id.is_reuse_safe());
    }

    #[test]
    fn complete_identity_hashes_unsampled_bytes_and_restores_position() {
        let a = (0..400_000)
            .map(|value| (value % 251) as u8)
            .collect::<Vec<_>>();
        let mut b = a.clone();
        // This offset sits outside all three windows used by the removed sparse-sample algorithm.
        b[100_000] ^= 0x7f;

        let mut cursor_a = Cursor::new(a);
        cursor_a.seek(SeekFrom::Start(123)).unwrap();
        let id_a =
            SourceIdentity::from_seekable(&mut cursor_a, Some(7), Some("doc:7".into())).unwrap();
        let id_b = SourceIdentity::from_seekable(&mut Cursor::new(b), Some(7), Some("doc:7".into()))
            .unwrap();

        assert_eq!(cursor_a.stream_position().unwrap(), 123);
        assert_eq!(id_a.size_bytes, Some(400_000));
        assert!(id_a.is_reuse_safe());
        assert!(
            id_a
                .content_tag
                .as_deref()
                .unwrap()
                .starts_with(FULL_HASH_TAG_PREFIX)
        );
        assert_ne!(id_a.content_tag, id_b.content_tag);
    }

    #[test]
    fn complete_identity_detects_changed_content_with_same_size() {
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
        let id = SourceIdentity::new(1234, Some(5678), Some("complete-proof".into()));
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
        let id = SourceIdentity::new(1, None, None);
        assert!(store.invalidate_source(&id).is_ok());
        assert!(store.clear().is_ok());
    }
}
