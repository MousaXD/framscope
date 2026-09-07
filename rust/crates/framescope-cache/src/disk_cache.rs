use crate::{FrameCacheError, FrameCacheKey, SourceIdentity};
use std::cmp::Ordering;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::SystemTime;
use thiserror::Error;

const PROXY_CACHE_VERSION: &str = "v1";
const MAX_PROXY_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyFormat {
    Jpeg,
    WebP,
}

impl ProxyFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::WebP => "webp",
        }
    }

    fn validate(self, bytes: &[u8]) -> bool {
        match self {
            Self::Jpeg => bytes.len() >= 3 && bytes[..3] == [0xff, 0xd8, 0xff],
            Self::WebP => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        }
    }
}

/// A disk-cached navigation proxy. This is deliberately a preview-quality type and must never be
/// accepted by extraction code as an original-quality decoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyFrame {
    pub key: FrameCacheKey,
    pub format: ProxyFormat,
    bytes: Vec<u8>,
}

impl ProxyFrame {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiskCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub insertions: u64,
    pub evictions: u64,
    pub corrupt_entries: u64,
    pub resident_bytes: u64,
    pub resident_files: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskInsertResult {
    Inserted,
    AlreadyPresent,
    TooLarge,
}

#[derive(Debug, Error)]
pub enum DiskCacheError {
    #[error(transparent)]
    Cache(#[from] FrameCacheError),
    #[error("invalid {format:?} proxy payload")]
    InvalidProxy { format: ProxyFormat },
    #[error("disk proxy cache I/O failed: {0}")]
    Io(#[from] io::Error),
}

/// Disposable, byte-bounded compressed frame proxy cache.
///
/// Entries live below a versioned namespace and are keyed by strong source identity, stream index,
/// and global frame ID. Writes are staged in the destination directory, flushed with `sync_all`,
/// and atomically renamed into place. Eviction uses persistent insertion age from filesystem
/// modification times, avoiding a fragile global metadata database. External deletion is safe.
#[derive(Debug)]
pub struct DiskProxyCache {
    root: PathBuf,
    budget_bytes: u64,
    stats: DiskCacheStats,
}

impl DiskProxyCache {
    pub fn open(root: impl AsRef<Path>, budget_bytes: u64) -> Result<Self, DiskCacheError> {
        let root = root.as_ref().join(PROXY_CACHE_VERSION);
        fs::create_dir_all(&root)?;
        cleanup_temporary_files(&root)?;
        let (resident_bytes, resident_files) = resident_totals(&root)?;
        let mut cache = Self {
            root,
            budget_bytes,
            stats: DiskCacheStats {
                resident_bytes,
                resident_files,
                ..DiskCacheStats::default()
            },
        };
        cache.evict_to_budget()?;
        Ok(cache)
    }

    pub fn budget_bytes(&self) -> u64 {
        self.budget_bytes
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn get(&mut self, key: &FrameCacheKey) -> Result<Option<ProxyFrame>, DiskCacheError> {
        for format in [ProxyFormat::WebP, ProxyFormat::Jpeg] {
            let path = self.entry_path(key, format);
            let mut file = match File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let length = file.metadata()?.len();
            if length > self.budget_bytes || length > MAX_PROXY_ENTRY_BYTES {
                self.remove_corrupt_entry(&path)?;
                continue;
            }
            let length = match usize::try_from(length) {
                Ok(length) => length,
                Err(_) => {
                    self.remove_corrupt_entry(&path)?;
                    continue;
                }
            };
            let mut bytes = vec![0; length];
            if let Err(error) = file.read_exact(&mut bytes) {
                if error.kind() == io::ErrorKind::UnexpectedEof {
                    self.remove_corrupt_entry(&path)?;
                    continue;
                }
                return Err(error.into());
            }
            if !format.validate(&bytes) {
                self.remove_corrupt_entry(&path)?;
                continue;
            }
            self.stats.hits = self.stats.hits.saturating_add(1);
            return Ok(Some(ProxyFrame {
                key: key.clone(),
                format,
                bytes,
            }));
        }
        self.stats.misses = self.stats.misses.saturating_add(1);
        Ok(None)
    }

    pub fn insert(
        &mut self,
        key: &FrameCacheKey,
        format: ProxyFormat,
        bytes: &[u8],
    ) -> Result<DiskInsertResult, DiskCacheError> {
        if !format.validate(bytes) {
            return Err(DiskCacheError::InvalidProxy { format });
        }
        let byte_len = u64::try_from(bytes.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "proxy payload size exceeds u64",
            )
        })?;
        if byte_len > self.budget_bytes || byte_len > MAX_PROXY_ENTRY_BYTES {
            return Ok(DiskInsertResult::TooLarge);
        }

        let path = self.entry_path(key, format);
        if path.is_file() {
            return Ok(DiskInsertResult::AlreadyPresent);
        }
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "proxy cache path has no parent",
            )
        })?;
        fs::create_dir_all(parent)?;
        let temp = temporary_path(parent, key, format);
        let write_result = (|| -> io::Result<()> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&temp, &path)?;
            Ok(())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temp);
            return Err(error.into());
        }

        self.stats.insertions = self.stats.insertions.saturating_add(1);
        self.stats.resident_bytes = self.stats.resident_bytes.saturating_add(byte_len);
        self.stats.resident_files = self.stats.resident_files.saturating_add(1);
        self.evict_to_budget()?;
        Ok(DiskInsertResult::Inserted)
    }

    pub fn invalidate_source(&mut self, source: &SourceIdentity) -> Result<(), DiskCacheError> {
        let path = self.root.join(source.stable_key());
        match fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        self.refresh_resident_totals()?;
        Ok(())
    }

    pub fn clear(&mut self) -> Result<(), DiskCacheError> {
        match fs::remove_dir_all(&self.root) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        fs::create_dir_all(&self.root)?;
        self.stats.resident_bytes = 0;
        self.stats.resident_files = 0;
        Ok(())
    }

    pub fn stats(&self) -> DiskCacheStats {
        self.stats
    }

    fn entry_path(&self, key: &FrameCacheKey, format: ProxyFormat) -> PathBuf {
        self.root
            .join(key.source_key())
            .join(key.stream_index.to_string())
            .join(format!("{}.{}", key.frame_id.0, format.extension()))
    }

    fn remove_corrupt_entry(&mut self, path: &Path) -> Result<(), DiskCacheError> {
        let removed = remove_file_len(path)?;
        self.stats.corrupt_entries = self.stats.corrupt_entries.saturating_add(1);
        self.stats.resident_bytes = self.stats.resident_bytes.saturating_sub(removed);
        if removed > 0 {
            self.stats.resident_files = self.stats.resident_files.saturating_sub(1);
        }
        Ok(())
    }

    fn refresh_resident_totals(&mut self) -> Result<(), DiskCacheError> {
        let (bytes, files) = resident_totals(&self.root)?;
        self.stats.resident_bytes = bytes;
        self.stats.resident_files = files;
        Ok(())
    }

    fn evict_to_budget(&mut self) -> Result<(), DiskCacheError> {
        let mut entries = collect_proxy_files(&self.root)?;
        let total = entries.iter().try_fold(0_u64, |total, entry| {
            total.checked_add(entry.len).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "proxy cache byte count overflow",
                )
            })
        })?;
        self.stats.resident_bytes = total;
        self.stats.resident_files = u64::try_from(entries.len()).unwrap_or(u64::MAX);
        if total <= self.budget_bytes {
            return Ok(());
        }

        entries.sort_by(|a, b| compare_modified(a.modified, b.modified));
        let mut resident = total;
        for entry in entries {
            if resident <= self.budget_bytes {
                break;
            }
            match fs::remove_file(&entry.path) {
                Ok(()) => {
                    resident = resident.saturating_sub(entry.len);
                    self.stats.resident_files = self.stats.resident_files.saturating_sub(1);
                    self.stats.evictions = self.stats.evictions.saturating_add(1);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    resident = resident.saturating_sub(entry.len);
                    self.stats.resident_files = self.stats.resident_files.saturating_sub(1);
                }
                Err(error) => return Err(error.into()),
            }
        }
        self.stats.resident_bytes = resident;
        Ok(())
    }
}

#[derive(Debug)]
struct ProxyFile {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}

fn collect_proxy_files(root: &Path) -> io::Result<Vec<ProxyFile>> {
    let mut files = Vec::new();
    collect_proxy_files_recursive(root, &mut files)?;
    Ok(files)
}

fn collect_proxy_files_recursive(root: &Path, files: &mut Vec<ProxyFile>) -> io::Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_proxy_files_recursive(&entry.path(), files)?;
            continue;
        }
        if !file_type.is_file() || is_temporary(&entry.path()) {
            continue;
        }
        let path = entry.path();
        let extension = path.extension().and_then(|value| value.to_str());
        if !matches!(extension, Some("jpg" | "webp")) {
            continue;
        }
        let metadata = entry.metadata()?;
        files.push(ProxyFile {
            path,
            len: metadata.len(),
            modified: metadata.modified().ok(),
        });
    }
    Ok(())
}

fn resident_totals(root: &Path) -> io::Result<(u64, u64)> {
    let entries = collect_proxy_files(root)?;
    let bytes = entries.iter().try_fold(0_u64, |total, entry| {
        total.checked_add(entry.len).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "proxy cache byte count overflow",
            )
        })
    })?;
    Ok((bytes, u64::try_from(entries.len()).unwrap_or(u64::MAX)))
}

fn cleanup_temporary_files(root: &Path) -> io::Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            cleanup_temporary_files(&entry.path())?;
        } else if file_type.is_file() && is_temporary(&entry.path()) {
            match fs::remove_file(entry.path()) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

fn is_temporary(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some("tmp")
}

fn temporary_path(parent: &Path, key: &FrameCacheKey, format: ProxyFormat) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
    parent.join(format!(
        ".{}.{}.{}.{}.tmp",
        key.frame_id.0,
        format.extension(),
        std::process::id(),
        sequence
    ))
}

fn remove_file_len(path: &Path) -> io::Result<u64> {
    let len = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    match fs::remove_file(path) {
        Ok(()) => Ok(len),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

fn compare_modified(a: Option<SystemTime>, b: Option<SystemTime>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(&b),
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FrameId;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(label: &str) -> Self {
            let sequence = TEMP_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "framescope-{label}-{}-{sequence}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn source(tag: &str) -> SourceIdentity {
        SourceIdentity::new(55, Some(1), Some(tag.into()))
    }

    fn key(source: &SourceIdentity, id: u64) -> FrameCacheKey {
        FrameCacheKey::new(source, 2, FrameId(id)).unwrap()
    }

    fn jpeg(payload_len: usize) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xd8, 0xff];
        bytes.resize(payload_len.max(3), 0x42);
        bytes
    }

    fn webp(payload_len: usize) -> Vec<u8> {
        let mut bytes = b"RIFF\x00\x00\x00\x00WEBP".to_vec();
        bytes.resize(payload_len.max(12), 0x24);
        bytes
    }

    #[test]
    fn round_trip_returns_typed_proxy() {
        let root = TempRoot::new("disk-proxy-round-trip");
        let source = source("content-a");
        let key = key(&source, 7);
        let mut cache = DiskProxyCache::open(&root.0, 1_024).unwrap();
        assert_eq!(
            cache.insert(&key, ProxyFormat::WebP, &webp(32)).unwrap(),
            DiskInsertResult::Inserted
        );
        let proxy = cache.get(&key).unwrap().unwrap();
        assert_eq!(proxy.key, key);
        assert_eq!(proxy.format, ProxyFormat::WebP);
        assert_eq!(proxy.byte_len(), 32);
        assert_eq!(cache.stats().hits, 1);
    }

    #[test]
    fn rejects_payload_that_does_not_match_declared_compressed_format() {
        let root = TempRoot::new("disk-proxy-invalid-format");
        let source = source("content-a");
        let key = key(&source, 1);
        let mut cache = DiskProxyCache::open(&root.0, 100).unwrap();
        let error = cache
            .insert(&key, ProxyFormat::WebP, &jpeg(20))
            .unwrap_err();
        assert!(matches!(
            error,
            DiskCacheError::InvalidProxy {
                format: ProxyFormat::WebP
            }
        ));
    }

    #[test]
    fn enforces_global_disk_budget_by_eviction() {
        let root = TempRoot::new("disk-proxy-budget");
        let source = source("content-a");
        let mut cache = DiskProxyCache::open(&root.0, 40).unwrap();
        cache
            .insert(&key(&source, 1), ProxyFormat::Jpeg, &jpeg(24))
            .unwrap();
        cache
            .insert(&key(&source, 2), ProxyFormat::Jpeg, &jpeg(24))
            .unwrap();
        assert!(cache.stats().resident_bytes <= 40);
        assert_eq!(cache.stats().resident_files, 1);
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn corrupt_entry_is_removed_and_treated_as_miss() {
        let root = TempRoot::new("disk-proxy-corrupt");
        let source = source("content-a");
        let key = key(&source, 3);
        let mut cache = DiskProxyCache::open(&root.0, 1_024).unwrap();
        let path = cache.entry_path(&key, ProxyFormat::Jpeg);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not-a-jpeg").unwrap();
        cache.refresh_resident_totals().unwrap();

        assert!(cache.get(&key).unwrap().is_none());
        assert!(!path.exists());
        assert_eq!(cache.stats().corrupt_entries, 1);
        assert_eq!(cache.stats().misses, 1);
    }

    #[test]
    fn source_invalidation_does_not_touch_other_sources() {
        let root = TempRoot::new("disk-proxy-invalidate");
        let source_a = source("content-a");
        let source_b = source("content-b");
        let key_a = key(&source_a, 4);
        let key_b = key(&source_b, 4);
        let mut cache = DiskProxyCache::open(&root.0, 1_024).unwrap();
        cache.insert(&key_a, ProxyFormat::Jpeg, &jpeg(20)).unwrap();
        cache.insert(&key_b, ProxyFormat::Jpeg, &jpeg(20)).unwrap();

        cache.invalidate_source(&source_a).unwrap();
        assert!(cache.get(&key_a).unwrap().is_none());
        assert!(cache.get(&key_b).unwrap().is_some());
    }

    #[test]
    fn stale_temp_file_is_cleaned_on_reopen() {
        let root = TempRoot::new("disk-proxy-temp-cleanup");
        let cache = DiskProxyCache::open(&root.0, 1_024).unwrap();
        let temp = cache.root().join("orphan.tmp");
        fs::write(&temp, b"partial").unwrap();
        drop(cache);

        let reopened = DiskProxyCache::open(&root.0, 1_024).unwrap();
        assert!(!temp.exists());
        assert_eq!(reopened.stats().resident_files, 0);
    }

    #[test]
    fn external_cache_deletion_is_recoverable() {
        let root = TempRoot::new("disk-proxy-delete");
        let source = source("content-a");
        let key = key(&source, 5);
        let mut cache = DiskProxyCache::open(&root.0, 1_024).unwrap();
        cache.insert(&key, ProxyFormat::WebP, &webp(20)).unwrap();
        fs::remove_dir_all(cache.root()).unwrap();

        assert!(cache.get(&key).unwrap().is_none());
        cache.clear().unwrap();
        assert!(cache.root().is_dir());
    }

    #[test]
    fn oversized_proxy_is_not_written() {
        let root = TempRoot::new("disk-proxy-too-large");
        let source = source("content-a");
        let key = key(&source, 6);
        let mut cache = DiskProxyCache::open(&root.0, 8).unwrap();
        assert_eq!(
            cache.insert(&key, ProxyFormat::Jpeg, &jpeg(20)).unwrap(),
            DiskInsertResult::TooLarge
        );
        assert_eq!(cache.stats().resident_files, 0);
    }
}
