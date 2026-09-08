use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use thiserror::Error;

pub const FRAME_INDEX_NAMESPACE: &str = "frame-index";
pub const PREVIEW_PROXY_NAMESPACE: &str = "microscope-frame-cache";

static STORAGE_ADMIN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StorageCategoryStats {
    pub bytes: u64,
    pub files: u64,
    pub items: u64,
}

impl StorageCategoryStats {
    fn checked_add(self, other: Self) -> Result<Self, StorageAdminError> {
        Ok(Self {
            bytes: self
                .bytes
                .checked_add(other.bytes)
                .ok_or(StorageAdminError::NumericOverflow)?,
            files: self
                .files
                .checked_add(other.files)
                .ok_or(StorageAdminError::NumericOverflow)?,
            items: self
                .items
                .checked_add(other.items)
                .ok_or(StorageAdminError::NumericOverflow)?,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct FrameScopeStorageStats {
    pub total_bytes: u64,
    pub persistent_indexes: StorageCategoryStats,
    pub preview_proxy: StorageCategoryStats,
    pub disposable: StorageCategoryStats,
    pub indexed_sources: u64,
    pub preview_proxy_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageClearScope {
    PreviewProxy,
    PersistentIndexes,
    Disposable,
    All,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StorageClearReport {
    pub cleared_bytes: u64,
    pub cleared_files: u64,
    pub cleared_items: u64,
}

#[derive(Debug, Error)]
pub enum StorageAdminError {
    #[error("FrameScope storage administration state is unavailable")]
    LockPoisoned,
    #[error("FrameScope cache root must not be a symbolic link: {0}")]
    RootIsSymlink(PathBuf),
    #[error("invalid frame-index source key")]
    InvalidSourceKey,
    #[error("storage accounting overflowed the supported byte range")]
    NumericOverflow,
    #[error("storage I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, Clone)]
pub struct StorageAdmin {
    root: PathBuf,
    preview_proxy_enabled: bool,
}

impl StorageAdmin {
    pub fn new(root: impl AsRef<Path>, preview_proxy_enabled: bool) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            preview_proxy_enabled,
        }
    }

    pub fn stats(&self) -> Result<FrameScopeStorageStats, StorageAdminError> {
        self.with_lock(|| self.stats_unlocked())
    }

    pub fn clear(&self, scope: StorageClearScope) -> Result<StorageClearReport, StorageAdminError> {
        self.with_lock(|| {
            self.ensure_root_safe()?;
            match scope {
                StorageClearScope::PreviewProxy => {
                    self.clear_owned_path(&self.root.join(PREVIEW_PROXY_NAMESPACE))
                }
                StorageClearScope::PersistentIndexes => {
                    self.clear_owned_path(&self.root.join(FRAME_INDEX_NAMESPACE))
                }
                StorageClearScope::Disposable => self.clear_disposable_unlocked(),
                StorageClearScope::All => self.clear_all_unlocked(),
            }
        })
    }

    pub fn clear_source_indexes(
        &self,
        source_key: &str,
    ) -> Result<StorageClearReport, StorageAdminError> {
        if !is_safe_source_key(source_key) {
            return Err(StorageAdminError::InvalidSourceKey);
        }
        self.with_lock(|| {
            self.ensure_root_safe()?;
            let index_root = self.root.join(FRAME_INDEX_NAMESPACE);
            let metadata = match symlink_metadata_optional(&index_root)? {
                Some(metadata) => metadata,
                None => return Ok(StorageClearReport::default()),
            };
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Ok(StorageClearReport::default());
            }

            let mut report = StorageClearReport::default();
            for version in read_dir(&index_root)? {
                let version = version.map_err(|source| io_error(&index_root, source))?;
                let version_type = version
                    .file_type()
                    .map_err(|source| io_error(version.path(), source))?;
                if !version_type.is_dir() || version_type.is_symlink() {
                    continue;
                }
                for source_entry in read_dir(&version.path())? {
                    let source_entry =
                        source_entry.map_err(|source| io_error(version.path(), source))?;
                    if source_entry.file_name().to_string_lossy() != source_key {
                        continue;
                    }
                    report =
                        checked_report_add(report, self.clear_owned_path(&source_entry.path())?)?;
                }
            }
            Ok(report)
        })
    }

    fn with_lock<T>(
        &self,
        operation: impl FnOnce() -> Result<T, StorageAdminError>,
    ) -> Result<T, StorageAdminError> {
        let _guard = STORAGE_ADMIN_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .map_err(|_| StorageAdminError::LockPoisoned)?;
        operation()
    }

    fn stats_unlocked(&self) -> Result<FrameScopeStorageStats, StorageAdminError> {
        self.ensure_root_safe()?;
        if !self.root.exists() {
            return Ok(FrameScopeStorageStats {
                preview_proxy_enabled: self.preview_proxy_enabled,
                ..FrameScopeStorageStats::default()
            });
        }

        let persistent_indexes = stats_for_path(&self.root.join(FRAME_INDEX_NAMESPACE))?;
        let preview_proxy = stats_for_path(&self.root.join(PREVIEW_PROXY_NAMESPACE))?;
        let disposable = self.disposable_stats_unlocked()?;
        let total_bytes = persistent_indexes
            .bytes
            .checked_add(preview_proxy.bytes)
            .and_then(|value| value.checked_add(disposable.bytes))
            .ok_or(StorageAdminError::NumericOverflow)?;
        let indexed_sources = count_indexed_sources(&self.root.join(FRAME_INDEX_NAMESPACE))?;

        Ok(FrameScopeStorageStats {
            total_bytes,
            persistent_indexes,
            preview_proxy,
            disposable,
            indexed_sources,
            preview_proxy_enabled: self.preview_proxy_enabled,
        })
    }

    fn ensure_root_safe(&self) -> Result<(), StorageAdminError> {
        let Some(metadata) = symlink_metadata_optional(&self.root)? else {
            return Ok(());
        };
        if metadata.file_type().is_symlink() {
            return Err(StorageAdminError::RootIsSymlink(self.root.clone()));
        }
        Ok(())
    }

    fn disposable_stats_unlocked(&self) -> Result<StorageCategoryStats, StorageAdminError> {
        let mut total = StorageCategoryStats::default();
        for entry in read_dir_optional(&self.root)? {
            let name = entry.file_name();
            if name == FRAME_INDEX_NAMESPACE || name == PREVIEW_PROXY_NAMESPACE {
                continue;
            }
            let mut stats = stats_for_path(&entry.path())?;
            stats.items = 1;
            total = total.checked_add(stats)?;
        }
        Ok(total)
    }

    fn clear_disposable_unlocked(&self) -> Result<StorageClearReport, StorageAdminError> {
        let mut report = StorageClearReport::default();
        for entry in read_dir_optional(&self.root)? {
            let name = entry.file_name();
            if name == FRAME_INDEX_NAMESPACE || name == PREVIEW_PROXY_NAMESPACE {
                continue;
            }
            report = checked_report_add(report, self.clear_owned_path(&entry.path())?)?;
        }
        Ok(report)
    }

    fn clear_all_unlocked(&self) -> Result<StorageClearReport, StorageAdminError> {
        let mut report = StorageClearReport::default();
        for entry in read_dir_optional(&self.root)? {
            report = checked_report_add(report, self.clear_owned_path(&entry.path())?)?;
        }
        Ok(report)
    }

    fn clear_owned_path(&self, path: &Path) -> Result<StorageClearReport, StorageAdminError> {
        let stats = stats_for_path(path)?;
        if symlink_metadata_optional(path)?.is_none() {
            return Ok(StorageClearReport::default());
        }
        remove_tree_without_following_symlinks(path)?;
        Ok(StorageClearReport {
            cleared_bytes: stats.bytes,
            cleared_files: stats.files,
            cleared_items: stats.items.max(1),
        })
    }
}

fn count_indexed_sources(index_root: &Path) -> Result<u64, StorageAdminError> {
    let Some(metadata) = symlink_metadata_optional(index_root)? else {
        return Ok(0);
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok(0);
    }

    let mut count = 0_u64;
    for version in read_dir(index_root)? {
        let version = version.map_err(|source| io_error(index_root, source))?;
        let version_type = version
            .file_type()
            .map_err(|source| io_error(version.path(), source))?;
        if !version_type.is_dir() || version_type.is_symlink() {
            continue;
        }
        for source in read_dir(&version.path())? {
            let source = source.map_err(|error| io_error(version.path(), error))?;
            let source_type = source
                .file_type()
                .map_err(|error| io_error(source.path(), error))?;
            if source_type.is_dir() && !source_type.is_symlink() {
                count = count
                    .checked_add(1)
                    .ok_or(StorageAdminError::NumericOverflow)?;
            }
        }
    }
    Ok(count)
}

fn is_safe_source_key(source_key: &str) -> bool {
    !source_key.is_empty()
        && source_key.len() <= 256
        && source_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn stats_for_path(path: &Path) -> Result<StorageCategoryStats, StorageAdminError> {
    let Some(metadata) = symlink_metadata_optional(path)? else {
        return Ok(StorageCategoryStats::default());
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        let mut total = StorageCategoryStats::default();
        for entry in read_dir(path)? {
            let entry = entry.map_err(|source| io_error(path, source))?;
            total = total.checked_add(stats_for_path(&entry.path())?)?;
        }
        Ok(total)
    } else {
        Ok(StorageCategoryStats {
            bytes: metadata.len(),
            files: 1,
            items: 1,
        })
    }
}

fn remove_tree_without_following_symlinks(path: &Path) -> Result<(), StorageAdminError> {
    let Some(metadata) = symlink_metadata_optional(path)? else {
        return Ok(());
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        for entry in read_dir(path)? {
            let entry = entry.map_err(|source| io_error(path, source))?;
            remove_tree_without_following_symlinks(&entry.path())?;
        }
        match fs::remove_dir(path) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(io_error(path, source)),
        }
    } else {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(io_error(path, source)),
        }
    }
}

fn read_dir(path: &Path) -> Result<fs::ReadDir, StorageAdminError> {
    fs::read_dir(path).map_err(|source| io_error(path, source))
}

fn read_dir_optional(path: &Path) -> Result<Vec<fs::DirEntry>, StorageAdminError> {
    match fs::read_dir(path) {
        Ok(entries) => entries
            .map(|entry| entry.map_err(|source| io_error(path, source)))
            .collect(),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(io_error(path, source)),
    }
}

fn symlink_metadata_optional(path: &Path) -> Result<Option<fs::Metadata>, StorageAdminError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io_error(path, source)),
    }
}

fn io_error(path: impl AsRef<Path>, source: io::Error) -> StorageAdminError {
    StorageAdminError::Io {
        path: path.as_ref().to_path_buf(),
        source,
    }
}

fn checked_report_add(
    left: StorageClearReport,
    right: StorageClearReport,
) -> Result<StorageClearReport, StorageAdminError> {
    Ok(StorageClearReport {
        cleared_bytes: left
            .cleared_bytes
            .checked_add(right.cleared_bytes)
            .ok_or(StorageAdminError::NumericOverflow)?,
        cleared_files: left
            .cleared_files
            .checked_add(right.cleared_files)
            .ok_or(StorageAdminError::NumericOverflow)?,
        cleared_items: left
            .cleared_items
            .checked_add(right.cleared_items)
            .ok_or(StorageAdminError::NumericOverflow)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    fn test_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "framescope-storage-admin-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn storage_stats_include_sqlite_sidecars_and_truthfully_disable_proxy_cache() {
        let root = test_root("stats");
        let source = root.join(FRAME_INDEX_NAMESPACE).join("v3").join("abc_123");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("stream-0.sqlite3"), b"database").unwrap();
        fs::write(source.join("stream-0.sqlite3-wal"), b"wal").unwrap();
        fs::write(source.join("stream-0.sqlite3-shm"), b"shm").unwrap();
        let proxy = root.join(PREVIEW_PROXY_NAMESPACE).join("v1");
        fs::create_dir_all(&proxy).unwrap();
        fs::write(proxy.join("preview.webp"), b"proxy").unwrap();
        fs::write(root.join("orphan.tmp"), b"tmp").unwrap();

        let stats = StorageAdmin::new(&root, false).stats().unwrap();
        assert_eq!(stats.persistent_indexes.bytes, 14);
        assert_eq!(stats.persistent_indexes.files, 3);
        assert_eq!(stats.preview_proxy.bytes, 5);
        assert_eq!(stats.disposable.bytes, 3);
        assert_eq!(stats.indexed_sources, 1);
        assert!(!stats.preview_proxy_enabled);
        assert_eq!(stats.total_bytes, 22);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persistent_index_clear_removes_partial_corrupt_and_sidecar_data_and_is_idempotent() {
        let root = test_root("indexes");
        let index_root = root.join(FRAME_INDEX_NAMESPACE).join("v3").join("source");
        fs::create_dir_all(&index_root).unwrap();
        fs::write(index_root.join("stream-0.sqlite3"), b"not sqlite").unwrap();
        fs::write(index_root.join("stream-0.sqlite3-wal"), b"partial wal").unwrap();
        fs::write(index_root.join("unfinished.partial"), b"partial").unwrap();
        fs::write(root.join("keep.tmp"), b"keep").unwrap();
        let admin = StorageAdmin::new(&root, false);

        let first = admin.clear(StorageClearScope::PersistentIndexes).unwrap();
        assert!(first.cleared_bytes > 0);
        assert!(!root.join(FRAME_INDEX_NAMESPACE).exists());
        assert!(root.join("keep.tmp").exists());
        assert_eq!(
            admin.clear(StorageClearScope::PersistentIndexes).unwrap(),
            StorageClearReport::default()
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn per_source_clear_is_confined_to_exact_valid_namespace() {
        let root = test_root("source");
        for source_key in ["source_A", "source_B"] {
            let source = root.join(FRAME_INDEX_NAMESPACE).join("v3").join(source_key);
            fs::create_dir_all(&source).unwrap();
            fs::write(source.join("stream-0.sqlite3"), source_key).unwrap();
            fs::write(source.join("stream-0.sqlite3-wal"), b"wal").unwrap();
        }
        let admin = StorageAdmin::new(&root, false);

        let report = admin.clear_source_indexes("source_A").unwrap();
        assert!(report.cleared_bytes > 0);
        assert!(
            !root
                .join(FRAME_INDEX_NAMESPACE)
                .join("v3/source_A")
                .exists()
        );
        assert!(
            root.join(FRAME_INDEX_NAMESPACE)
                .join("v3/source_B")
                .exists()
        );
        for invalid in ["", "../source_B", "source/B", "source\\B", "."] {
            assert!(matches!(
                admin.clear_source_indexes(invalid),
                Err(StorageAdminError::InvalidSourceKey)
            ));
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn disposable_clear_preserves_indexes_and_proxy_namespace() {
        let root = test_root("disposable");
        let index = root.join(FRAME_INDEX_NAMESPACE).join("v3/source");
        let proxy = root.join(PREVIEW_PROXY_NAMESPACE).join("v1");
        fs::create_dir_all(&index).unwrap();
        fs::create_dir_all(&proxy).unwrap();
        fs::write(index.join("stream-0.sqlite3"), b"index").unwrap();
        fs::write(proxy.join("preview.webp"), b"proxy").unwrap();
        fs::create_dir_all(root.join("scratch/nested")).unwrap();
        fs::write(root.join("scratch/nested/file.tmp"), b"scratch").unwrap();
        let admin = StorageAdmin::new(&root, false);

        admin.clear(StorageClearScope::Disposable).unwrap();
        assert!(index.exists());
        assert!(proxy.exists());
        assert!(!root.join("scratch").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_clear_operations_are_serialized_and_fail_safe() {
        let root = test_root("concurrent");
        let scratch = root.join("scratch");
        fs::create_dir_all(&scratch).unwrap();
        for index in 0..64 {
            fs::write(scratch.join(format!("{index}.tmp")), vec![index as u8; 128]).unwrap();
        }
        let admin = Arc::new(StorageAdmin::new(&root, false));
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let admin = Arc::clone(&admin);
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                admin.clear(StorageClearScope::Disposable).unwrap()
            }));
        }
        barrier.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(!scratch.exists());
        assert_eq!(admin.stats().unwrap().total_bytes, 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_never_followed_outside_the_framescope_namespace() {
        use std::os::unix::fs::symlink;

        let root = test_root("symlink");
        let outside = test_root("outside");
        let sentinel = outside.join("sentinel.txt");
        fs::write(&sentinel, b"must survive").unwrap();
        symlink(&outside, root.join("scratch-link")).unwrap();
        let admin = StorageAdmin::new(&root, false);

        admin.clear(StorageClearScope::Disposable).unwrap();
        assert!(!root.join("scratch-link").exists());
        assert_eq!(fs::read(&sentinel).unwrap(), b"must survive");

        let symlink_root = root.with_extension("symlink-root");
        let _ = fs::remove_file(&symlink_root);
        symlink(&outside, &symlink_root).unwrap();
        assert!(matches!(
            StorageAdmin::new(&symlink_root, false).clear(StorageClearScope::All),
            Err(StorageAdminError::RootIsSymlink(_))
        ));
        assert_eq!(fs::read(&sentinel).unwrap(), b"must survive");

        let _ = fs::remove_file(symlink_root);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
