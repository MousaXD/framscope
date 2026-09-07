//! Versioned disposable persistence for derived frame-similarity groups.

use framescope_cache::{FrameId, FrameIndexStreamIdentity, SourceIdentity};
use framescope_core::{MediaDuration, MediaTimestamp};
use framescope_similarity::{FrameGroup, SimilarityMode};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const SIMILARITY_STORE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimilarityStoreKey {
    pub source: SourceIdentity,
    pub stream: FrameIndexStreamIdentity,
    pub mode: SimilarityMode,
}

impl SimilarityStoreKey {
    pub fn new(
        source: SourceIdentity,
        stream: FrameIndexStreamIdentity,
        mode: SimilarityMode,
    ) -> Result<Self, SimilarityStoreError> {
        if !source.is_reuse_safe() {
            return Err(SimilarityStoreError::UnsafeSourceIdentity);
        }
        mode.validate()
            .map_err(|error| SimilarityStoreError::InvalidMode(error.to_string()))?;
        Ok(Self {
            source,
            stream,
            mode,
        })
    }

    fn file_name(&self) -> String {
        let suffix = match self.mode {
            SimilarityMode::Exact => "exact".to_owned(),
            SimilarityMode::LumaMeanAbsolute { minimum_similarity } => {
                format!("luma-{minimum_similarity}")
            }
        };
        format!("stream-{}-{suffix}.json", self.stream.stream_index)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimilarityStoreLoad {
    Missing,
    Reused(Vec<FrameGroup>),
    InvalidatedStale,
    InvalidatedCorrupt,
}

#[derive(Debug, Error)]
pub enum SimilarityStoreError {
    #[error("source identity is unsafe for persisted similarity reuse")]
    UnsafeSourceIdentity,
    #[error("invalid similarity mode: {0}")]
    InvalidMode(String),
    #[error("invalid frame group: {0}")]
    InvalidGroup(String),
    #[error("similarity store I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("similarity store serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedKey {
    source: SourceIdentity,
    stream: FrameIndexStreamIdentity,
    mode: PersistedMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PersistedMode {
    Exact,
    LumaMeanAbsolute { minimum_similarity: u16 },
}

impl From<SimilarityMode> for PersistedMode {
    fn from(value: SimilarityMode) -> Self {
        match value {
            SimilarityMode::Exact => Self::Exact,
            SimilarityMode::LumaMeanAbsolute { minimum_similarity } => {
                Self::LumaMeanAbsolute { minimum_similarity }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedFrameGroup {
    representative_frame: FrameId,
    first_frame: FrameId,
    last_frame: FrameId,
    frame_count: u64,
    start_timestamp: MediaTimestamp,
    end_timestamp: MediaTimestamp,
    start_duration: Option<MediaDuration>,
    end_duration: Option<MediaDuration>,
    representative_similarity_floor: u16,
}

impl From<&FrameGroup> for PersistedFrameGroup {
    fn from(value: &FrameGroup) -> Self {
        Self {
            representative_frame: value.representative_frame,
            first_frame: value.first_frame,
            last_frame: value.last_frame,
            frame_count: value.frame_count,
            start_timestamp: value.start_timestamp,
            end_timestamp: value.end_timestamp,
            start_duration: value.start_duration,
            end_duration: value.end_duration,
            representative_similarity_floor: value.representative_similarity_floor,
        }
    }
}

impl From<PersistedFrameGroup> for FrameGroup {
    fn from(value: PersistedFrameGroup) -> Self {
        Self {
            representative_frame: value.representative_frame,
            first_frame: value.first_frame,
            last_frame: value.last_frame,
            frame_count: value.frame_count,
            start_timestamp: value.start_timestamp,
            end_timestamp: value.end_timestamp,
            start_duration: value.start_duration,
            end_duration: value.end_duration,
            representative_similarity_floor: value.representative_similarity_floor,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Manifest {
    schema_version: u32,
    key: PersistedKey,
    groups: Vec<PersistedFrameGroup>,
}

#[derive(Debug, Clone)]
pub struct SimilarityStore {
    root: PathBuf,
}

impl SimilarityStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path_for(&self, key: &SimilarityStoreKey) -> PathBuf {
        self.root
            .join(format!("v{SIMILARITY_STORE_SCHEMA_VERSION}"))
            .join(key.source.stable_key())
            .join(key.file_name())
    }

    pub fn load(
        &self,
        expected: &SimilarityStoreKey,
    ) -> Result<SimilarityStoreLoad, SimilarityStoreError> {
        let path = self.path_for(expected);
        remove_if_present(&temp_path(&path))?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(SimilarityStoreLoad::Missing);
            }
            Err(error) => return Err(error.into()),
        };
        let manifest: Manifest = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => {
                remove_if_present(&path)?;
                return Ok(SimilarityStoreLoad::InvalidatedCorrupt);
            }
        };
        let expected_key = PersistedKey {
            source: expected.source.clone(),
            stream: expected.stream.clone(),
            mode: expected.mode.into(),
        };
        if manifest.schema_version != SIMILARITY_STORE_SCHEMA_VERSION
            || manifest.key != expected_key
        {
            remove_if_present(&path)?;
            return Ok(SimilarityStoreLoad::InvalidatedStale);
        }
        let groups = manifest
            .groups
            .into_iter()
            .map(FrameGroup::from)
            .collect::<Vec<_>>();
        if groups.iter().any(invalid_group) {
            remove_if_present(&path)?;
            return Ok(SimilarityStoreLoad::InvalidatedCorrupt);
        }
        Ok(SimilarityStoreLoad::Reused(groups))
    }

    pub fn save(
        &self,
        key: &SimilarityStoreKey,
        groups: &[FrameGroup],
    ) -> Result<(), SimilarityStoreError> {
        if groups.iter().any(invalid_group) {
            return Err(SimilarityStoreError::InvalidGroup(
                "group ranges must be ordered, non-empty, and match frame_count".into(),
            ));
        }
        let path = self.path_for(key);
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "store path has no parent")
        })?;
        fs::create_dir_all(parent)?;
        let temp = temp_path(&path);
        remove_if_present(&temp)?;
        let manifest = Manifest {
            schema_version: SIMILARITY_STORE_SCHEMA_VERSION,
            key: PersistedKey {
                source: key.source.clone(),
                stream: key.stream.clone(),
                mode: key.mode.into(),
            },
            groups: groups.iter().map(PersistedFrameGroup::from).collect(),
        };
        fs::write(&temp, serde_json::to_vec(&manifest)?)?;
        remove_if_present(&path)?;
        if let Err(error) = fs::rename(&temp, &path) {
            let _ = remove_if_present(&temp);
            return Err(error.into());
        }
        Ok(())
    }

    pub fn invalidate_source(&self, source: &SourceIdentity) -> Result<(), SimilarityStoreError> {
        let path = self
            .root
            .join(format!("v{SIMILARITY_STORE_SCHEMA_VERSION}"))
            .join(source.stable_key());
        match fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

fn invalid_group(group: &FrameGroup) -> bool {
    let expected = group
        .last_frame
        .0
        .checked_sub(group.first_frame.0)
        .and_then(|delta| delta.checked_add(1));
    group.first_frame.0 > group.last_frame.0
        || group.frame_count == 0
        || expected != Some(group.frame_count)
        || group.representative_frame.0 < group.first_frame.0
        || group.representative_frame.0 > group.last_frame.0
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_core::{CodecInfo, MediaKind, StreamInfo, TimeBase};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn root(tag: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("framescope-similarity-store-{tag}-{nonce}"))
    }

    fn source(tag: &str) -> SourceIdentity {
        SourceIdentity::new(100, Some(10), Some(tag.into()))
    }

    fn stream() -> FrameIndexStreamIdentity {
        FrameIndexStreamIdentity::from_stream(&StreamInfo {
            index: 2,
            media_kind: MediaKind::Video,
            codec: CodecInfo {
                id: 27,
                name: "h264".into(),
                decoder_available: true,
            },
            is_default: true,
            time_base: Some(TimeBase::new(1, 1000).unwrap()),
            duration: None,
            frame_count: None,
            width: Some(1920),
            height: Some(1080),
            pixel_format: Some("yuv420p".into()),
            average_frame_rate: None,
            nominal_frame_rate: None,
            rotation_degrees: None,
        })
        .unwrap()
    }

    fn group() -> FrameGroup {
        let time_base = TimeBase::new(1, 1000).unwrap();
        FrameGroup {
            representative_frame: FrameId(10),
            first_frame: FrameId(10),
            last_frame: FrameId(11),
            frame_count: 2,
            start_timestamp: MediaTimestamp {
                ticks: 100,
                time_base,
            },
            end_timestamp: MediaTimestamp {
                ticks: 140,
                time_base,
            },
            start_duration: Some(MediaDuration {
                ticks: 40,
                time_base,
            }),
            end_duration: Some(MediaDuration {
                ticks: 85,
                time_base,
            }),
            representative_similarity_floor: 9_800,
        }
    }

    #[test]
    fn round_trip_reuses_exact_identity() {
        let root = root("roundtrip");
        let store = SimilarityStore::new(&root);
        let key = SimilarityStoreKey::new(source("a"), stream(), SimilarityMode::Exact).unwrap();
        store.save(&key, &[group()]).unwrap();
        assert_eq!(
            store.load(&key).unwrap(),
            SimilarityStoreLoad::Reused(vec![group()])
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn config_changes_namespace() {
        let store = SimilarityStore::new(root("config"));
        let exact = SimilarityStoreKey::new(source("a"), stream(), SimilarityMode::Exact).unwrap();
        let luma = SimilarityStoreKey::new(
            source("a"),
            stream(),
            SimilarityMode::LumaMeanAbsolute {
                minimum_similarity: 9_700,
            },
        )
        .unwrap();
        assert_ne!(store.path_for(&exact), store.path_for(&luma));
    }

    #[test]
    fn corrupt_manifest_is_disposable() {
        let root = root("corrupt");
        let store = SimilarityStore::new(&root);
        let key = SimilarityStoreKey::new(source("a"), stream(), SimilarityMode::Exact).unwrap();
        let path = store.path_for(&key);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not-json").unwrap();
        assert_eq!(
            store.load(&key).unwrap(),
            SimilarityStoreLoad::InvalidatedCorrupt
        );
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn source_invalidation_is_scoped() {
        let root = root("invalidate");
        let store = SimilarityStore::new(&root);
        let a = source("a");
        let b = source("b");
        let ka = SimilarityStoreKey::new(a.clone(), stream(), SimilarityMode::Exact).unwrap();
        let kb = SimilarityStoreKey::new(b, stream(), SimilarityMode::Exact).unwrap();
        store.save(&ka, &[group()]).unwrap();
        store.save(&kb, &[group()]).unwrap();
        store.invalidate_source(&a).unwrap();
        assert_eq!(store.load(&ka).unwrap(), SimilarityStoreLoad::Missing);
        assert!(matches!(
            store.load(&kb).unwrap(),
            SimilarityStoreLoad::Reused(_)
        ));
        let _ = fs::remove_dir_all(root);
    }
}
