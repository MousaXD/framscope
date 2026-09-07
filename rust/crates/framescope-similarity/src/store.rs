use crate::{FrameGroup, SimilarityMode};
use framescope_cache::{FrameIndexStreamIdentity, SourceIdentity};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const SIMILARITY_STORE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        let mode = match self.mode {
            SimilarityMode::Exact => "exact".to_owned(),
            SimilarityMode::LumaMeanAbsolute { minimum_similarity } => {
                format!("luma-{minimum_similarity}")
            }
        };
        format!("stream-{}-{mode}.json", self.stream.stream_index)
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
struct SimilarityManifest {
    schema_version: u32,
    key: SimilarityStoreKey,
    groups: Vec<FrameGroup>,
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

    pub fn load(&self, expected: &SimilarityStoreKey) -> Result<SimilarityStoreLoad, SimilarityStoreError> {
        let path = self.path_for(expected);
        self.remove_stale_temp(&path)?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(SimilarityStoreLoad::Missing);
            }
            Err(error) => return Err(error.into()),
        };

        let manifest: SimilarityManifest = match serde_json::from_slice(&bytes) {
            Ok(manifest) => manifest,
            Err(_) => {
                remove_if_present(&path)?;
                return Ok(SimilarityStoreLoad::InvalidatedCorrupt);
            }
        };

        if manifest.schema_version != SIMILARITY_STORE_SCHEMA_VERSION || manifest.key != *expected {
            remove_if_present(&path)?;
            return Ok(SimilarityStoreLoad::InvalidatedStale);
        }

        if manifest.groups.iter().any(validate_group) {
            remove_if_present(&path)?;
            return Ok(SimilarityStoreLoad::InvalidatedCorrupt);
        }

        Ok(SimilarityStoreLoad::Reused(manifest.groups))
    }

    pub fn save(
        &self,
        key: &SimilarityStoreKey,
        groups: &[FrameGroup],
    ) -> Result<(), SimilarityStoreError> {
        if groups.iter().any(validate_group) {
            return Err(SimilarityStoreError::InvalidGroup(
                "group ranges must be ordered, non-empty, and match frame_count".into(),
            ));
        }

        let path = self.path_for(key);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "store path has no parent"))?;
        fs::create_dir_all(parent)?;
        self.remove_stale_temp(&path)?;

        let manifest = SimilarityManifest {
            schema_version: SIMILARITY_STORE_SCHEMA_VERSION,
            key: key.clone(),
            groups: groups.to_vec(),
        };
        let bytes = serde_json::to_vec(&manifest)?;
        let temp = temp_path(&path);
        fs::write(&temp, bytes)?;

        if path.exists() {
            remove_if_present(&path)?;
        }
        if let Err(error) = fs::rename(&temp, &path) {
            let _ = remove_if_present(&temp);
            return Err(error.into());
        }
        Ok(())
    }

    pub fn invalidate_source(&self, source: &SourceIdentity) -> Result<(), SimilarityStoreError> {
        let source_dir = self
            .root
            .join(format!("v{SIMILARITY_STORE_SCHEMA_VERSION}"))
            .join(source.stable_key());
        match fs::remove_dir_all(source_dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn remove_stale_temp(&self, path: &Path) -> Result<(), SimilarityStoreError> {
        remove_if_present(&temp_path(path)).map_err(Into::into)
    }
}

fn validate_group(group: &FrameGroup) -> bool {
    if group.first_frame.0 > group.last_frame.0 || group.frame_count == 0 {
        return true;
    }
    let expected_count = group
        .last_frame
        .0
        .checked_sub(group.first_frame.0)
        .and_then(|delta| delta.checked_add(1));
    expected_count != Some(group.frame_count)
        || group.representative_frame.0 < group.first_frame.0
        || group.representative_frame.0 > group.last_frame.0
}

fn temp_path(path: &Path) -> PathBuf {
    let mut file_name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    file_name.push(".tmp");
    path.with_file_name(file_name)
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
    use framescope_cache::FrameId;
    use framescope_core::{CodecInfo, MediaDuration, MediaKind, MediaTimestamp, StreamInfo, TimeBase};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(tag: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("framescope-similarity-{tag}-{nonce}"))
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

    fn source(tag: &str) -> SourceIdentity {
        SourceIdentity::new(100, Some(10), Some(tag.into()))
    }

    fn group() -> FrameGroup {
        let time_base = TimeBase::new(1, 1000).unwrap();
        FrameGroup {
            representative_frame: FrameId(10),
            first_frame: FrameId(10),
            last_frame: FrameId(11),
            frame_count: 2,
            start_timestamp: MediaTimestamp { ticks: 100, time_base },
            end_timestamp: MediaTimestamp { ticks: 140, time_base },
            start_duration: Some(MediaDuration { ticks: 40, time_base }),
            end_duration: Some(MediaDuration { ticks: 85, time_base }),
            representative_similarity_floor: 9_800,
        }
    }

    #[test]
    fn round_trip_reuses_exact_identity() {
        let root = temp_root("roundtrip");
        let store = SimilarityStore::new(&root);
        let key = SimilarityStoreKey::new(
            source("content-a"),
            stream(),
            SimilarityMode::LumaMeanAbsolute {
                minimum_similarity: 9_700,
            },
        )
        .unwrap();
        store.save(&key, &[group()]).unwrap();
        assert_eq!(
            store.load(&key).unwrap(),
            SimilarityStoreLoad::Reused(vec![group()])
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn config_identity_changes_file_namespace() {
        let root = temp_root("config");
        let store = SimilarityStore::new(&root);
        let exact = SimilarityStoreKey::new(source("same"), stream(), SimilarityMode::Exact).unwrap();
        let luma = SimilarityStoreKey::new(
            source("same"),
            stream(),
            SimilarityMode::LumaMeanAbsolute {
                minimum_similarity: 9_700,
            },
        )
        .unwrap();
        assert_ne!(store.path_for(&exact), store.path_for(&luma));
    }

    #[test]
    fn embedded_stream_mismatch_is_invalidated() {
        let root = temp_root("stale");
        let store = SimilarityStore::new(&root);
        let key = SimilarityStoreKey::new(source("same"), stream(), SimilarityMode::Exact).unwrap();
        store.save(&key, &[group()]).unwrap();

        let path = store.path_for(&key);
        let mut manifest: SimilarityManifest = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest.key.stream.width = Some(1280);
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();

        assert_eq!(
            store.load(&key).unwrap(),
            SimilarityStoreLoad::InvalidatedStale
        );
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_manifest_is_disposable() {
        let root = temp_root("corrupt");
        let store = SimilarityStore::new(&root);
        let key = SimilarityStoreKey::new(source("same"), stream(), SimilarityMode::Exact).unwrap();
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
    fn unsafe_source_identity_cannot_be_persisted() {
        let unsafe_source = SourceIdentity::metadata_only(Some(100), Some(10), Some("doc".into()));
        assert!(matches!(
            SimilarityStoreKey::new(unsafe_source, stream(), SimilarityMode::Exact),
            Err(SimilarityStoreError::UnsafeSourceIdentity)
        ));
    }

    #[test]
    fn source_invalidation_removes_only_that_source_namespace() {
        let root = temp_root("invalidate");
        let store = SimilarityStore::new(&root);
        let source_a = source("a");
        let source_b = source("b");
        let key_a = SimilarityStoreKey::new(source_a.clone(), stream(), SimilarityMode::Exact).unwrap();
        let key_b = SimilarityStoreKey::new(source_b.clone(), stream(), SimilarityMode::Exact).unwrap();
        store.save(&key_a, &[group()]).unwrap();
        store.save(&key_b, &[group()]).unwrap();

        store.invalidate_source(&source_a).unwrap();
        assert_eq!(store.load(&key_a).unwrap(), SimilarityStoreLoad::Missing);
        assert!(matches!(store.load(&key_b).unwrap(), SimilarityStoreLoad::Reused(_)));
        let _ = fs::remove_dir_all(root);
    }
}
