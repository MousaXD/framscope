use crate::FRAME_INDEX_NAMESPACE;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistentFrameIndexStatus {
    Indexed,
    InProgress,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PersistentFrameIndexDescriptor {
    pub source_key: String,
    pub stream_index: u32,
    pub relative_path: String,
    pub status: PersistentFrameIndexStatus,
    pub indexed_frames: u64,
    pub frame_count: Option<u64>,
    pub last_modified_epoch_ms: Option<u64>,
}

#[derive(Debug, Error)]
pub enum FrameIndexCatalogError {
    #[error("FrameScope cache root must not be a symbolic link: {0}")]
    RootIsSymlink(PathBuf),
    #[error("frame-index catalog I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, Clone)]
pub struct FrameIndexCatalog {
    root: PathBuf,
}

impl FrameIndexCatalog {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    /// Discover persistent frame indexes without mutating, opening, rebuilding, or validating a
    /// source. The catalog is intentionally advisory: authoritative reuse validation still happens
    /// in `FrameIndex::open_or_create` when the user reopens the source.
    pub fn entries(&self) -> Result<Vec<PersistentFrameIndexDescriptor>, FrameIndexCatalogError> {
        ensure_root_safe(&self.root)?;
        let index_root = self.root.join(FRAME_INDEX_NAMESPACE);
        let Some(metadata) = symlink_metadata_optional(&index_root)? else {
            return Ok(Vec::new());
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        for version in read_dir(&index_root)? {
            let version = version.map_err(|source| io_error(&index_root, source))?;
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
                if !source_type.is_dir() || source_type.is_symlink() {
                    continue;
                }
                let source_key = source.file_name().to_string_lossy().to_string();
                if !is_safe_source_key(&source_key) {
                    continue;
                }
                for database in read_dir(&source.path())? {
                    let database = database.map_err(|error| io_error(source.path(), error))?;
                    let database_type = database
                        .file_type()
                        .map_err(|error| io_error(database.path(), error))?;
                    if !database_type.is_file() || database_type.is_symlink() {
                        continue;
                    }
                    let Some(stream_index) = stream_index_from_name(&database.file_name().to_string_lossy()) else {
                        continue;
                    };
                    entries.push(read_descriptor(
                        &self.root,
                        database.path(),
                        source_key.clone(),
                        stream_index,
                    ));
                }
            }
        }
        entries.sort_by(|left, right| {
            left.source_key
                .cmp(&right.source_key)
                .then(left.stream_index.cmp(&right.stream_index))
        });
        Ok(entries)
    }
}

fn read_descriptor(
    root: &Path,
    path: PathBuf,
    source_key: String,
    stream_index: u32,
) -> PersistentFrameIndexDescriptor {
    let relative_path = path
        .strip_prefix(root)
        .ok()
        .map(path_to_wire)
        .unwrap_or_else(|| path.file_name().unwrap_or_default().to_string_lossy().to_string());
    let last_modified_epoch_ms = fs::metadata(&path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());

    let mut descriptor = PersistentFrameIndexDescriptor {
        source_key,
        stream_index,
        relative_path,
        status: PersistentFrameIndexStatus::Stale,
        indexed_frames: 0,
        frame_count: None,
        last_modified_epoch_ms,
    };

    let connection = match Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(connection) => connection,
        Err(_) => return descriptor,
    };
    let meta = connection
        .query_row(
            "SELECT lifecycle, indexed_frames, frame_count FROM index_meta WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .optional();
    let Ok(Some((lifecycle, indexed_frames, frame_count))) = meta else {
        return descriptor;
    };
    let Ok(indexed_frames) = u64::try_from(indexed_frames) else {
        return descriptor;
    };
    let frame_count = match frame_count {
        Some(value) => match u64::try_from(value) {
            Ok(value) => Some(value),
            Err(_) => return descriptor,
        },
        None => None,
    };
    let row_summary = connection.query_row(
        "SELECT COUNT(*), MIN(frame_index), MAX(frame_index) FROM frame_index",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        },
    );
    let Ok((row_count, min_frame, max_frame)) = row_summary else {
        return descriptor;
    };
    let Ok(row_count) = u64::try_from(row_count) else {
        return descriptor;
    };
    let contiguous = row_count == indexed_frames
        && if indexed_frames == 0 {
            min_frame.is_none() && max_frame.is_none()
        } else {
            min_frame == Some(0)
                && max_frame.and_then(|value| u64::try_from(value).ok()) == Some(indexed_frames - 1)
        };
    if !contiguous {
        return descriptor;
    }

    descriptor.indexed_frames = indexed_frames;
    descriptor.frame_count = frame_count;
    descriptor.status = match lifecycle {
        1 | 2 if frame_count.is_none() => PersistentFrameIndexStatus::InProgress,
        3 if frame_count == Some(indexed_frames) => PersistentFrameIndexStatus::Indexed,
        4 => PersistentFrameIndexStatus::Stale,
        _ => PersistentFrameIndexStatus::Stale,
    };
    descriptor
}

fn path_to_wire(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn stream_index_from_name(name: &str) -> Option<u32> {
    name.strip_prefix("stream-")?
        .strip_suffix(".sqlite3")?
        .parse::<u32>()
        .ok()
}

fn is_safe_source_key(source_key: &str) -> bool {
    !source_key.is_empty()
        && source_key.len() <= 256
        && source_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn ensure_root_safe(root: &Path) -> Result<(), FrameIndexCatalogError> {
    let Some(metadata) = symlink_metadata_optional(root)? else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() {
        return Err(FrameIndexCatalogError::RootIsSymlink(root.to_path_buf()));
    }
    Ok(())
}

fn read_dir(path: &Path) -> Result<fs::ReadDir, FrameIndexCatalogError> {
    fs::read_dir(path).map_err(|source| io_error(path, source))
}

fn symlink_metadata_optional(path: &Path) -> Result<Option<fs::Metadata>, FrameIndexCatalogError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io_error(path, source)),
    }
}

fn io_error(path: impl AsRef<Path>, source: io::Error) -> FrameIndexCatalogError {
    FrameIndexCatalogError::Io {
        path: path.as_ref().to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn test_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "framescope-index-catalog-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_index(root: &Path, source_key: &str, stream_index: u32, lifecycle: i64, frames: u64) {
        let directory = root
            .join(FRAME_INDEX_NAMESPACE)
            .join("v1")
            .join(source_key);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("stream-{stream_index}.sqlite3"));
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE index_meta (id INTEGER PRIMARY KEY, lifecycle INTEGER NOT NULL, indexed_frames INTEGER NOT NULL, frame_count INTEGER);\n                 CREATE TABLE frame_index (frame_index INTEGER PRIMARY KEY);",
            )
            .unwrap();
        for frame in 0..frames {
            connection
                .execute("INSERT INTO frame_index(frame_index) VALUES (?1)", params![frame])
                .unwrap();
        }
        let complete_count = (lifecycle == 3).then_some(frames);
        connection
            .execute(
                "INSERT INTO index_meta(id, lifecycle, indexed_frames, frame_count) VALUES (1, ?1, ?2, ?3)",
                params![lifecycle, frames, complete_count],
            )
            .unwrap();
    }

    #[test]
    fn discovers_three_persistent_indexes_without_source_history() {
        let root = test_root("three-indexes");
        write_index(&root, "aaaaaaaa", 0, 3, 10);
        write_index(&root, "bbbbbbbb", 0, 3, 20);
        write_index(&root, "cccccccc", 0, 3, 30);

        let entries = FrameIndexCatalog::new(&root).entries().unwrap();

        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|entry| entry.status == PersistentFrameIndexStatus::Indexed));
        assert_eq!(entries.iter().map(|entry| entry.frame_count.unwrap()).sum::<u64>(), 60);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incomplete_and_corrupt_metadata_do_not_masquerade_as_complete() {
        let root = test_root("status");
        write_index(&root, "building", 0, 2, 4);
        write_index(&root, "stale", 0, 4, 2);

        let entries = FrameIndexCatalog::new(&root).entries().unwrap();

        assert_eq!(entries[0].status, PersistentFrameIndexStatus::InProgress);
        assert_eq!(entries[1].status, PersistentFrameIndexStatus::Stale);
        fs::remove_dir_all(root).unwrap();
    }
}
