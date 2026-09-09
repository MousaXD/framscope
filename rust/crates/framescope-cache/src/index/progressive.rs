use super::{
    FRAME_INDEX_SCHEMA_VERSION, FRAME_TIMELINE_CONTRACT_GENERATION, FrameId, FrameIndex,
    FrameIndexError, FrameIndexLifecycle, FrameIndexStreamIdentity,
};
use crate::SourceIdentity;
use framescope_core::{MediaTimestamp, TimeBase};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

pub const PROGRESSIVE_INDEX_SCHEMA_VERSION: i64 = 1;
pub const STRUCTURAL_INDEX_GENERATION: u32 = 1;
pub const VISUAL_INDEX_GENERATION: u32 = 1;

const META_ROW_ID: i64 = 1;
const SIMILARITY_STATUS_ROW_ID: i64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressiveIndexLayer {
    StructuralNavigation,
    VisualAcceleration,
    AuthoritativeExact,
}

impl ProgressiveIndexLayer {
    fn as_i64(self) -> i64 {
        match self {
            Self::StructuralNavigation => 1,
            Self::VisualAcceleration => 2,
            Self::AuthoritativeExact => 3,
        }
    }

    fn from_i64(value: i64) -> Result<Self, ProgressiveIndexError> {
        match value {
            1 => Ok(Self::StructuralNavigation),
            2 => Ok(Self::VisualAcceleration),
            3 => Ok(Self::AuthoritativeExact),
            _ => Err(ProgressiveIndexError::InvalidState(format!(
                "unknown progressive index layer {value}"
            ))),
        }
    }

    pub fn is_authoritative(self) -> bool {
        matches!(self, Self::AuthoritativeExact)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressiveLayerLifecycle {
    NotStarted,
    Building,
    Incomplete,
    Complete,
    FailedRecoverable,
}

impl ProgressiveLayerLifecycle {
    fn as_i64(self) -> i64 {
        match self {
            Self::NotStarted => 0,
            Self::Building => 1,
            Self::Incomplete => 2,
            Self::Complete => 3,
            Self::FailedRecoverable => 4,
        }
    }

    fn from_i64(value: i64) -> Result<Self, ProgressiveIndexError> {
        match value {
            0 => Ok(Self::NotStarted),
            1 => Ok(Self::Building),
            2 => Ok(Self::Incomplete),
            3 => Ok(Self::Complete),
            4 => Ok(Self::FailedRecoverable),
            _ => Err(ProgressiveIndexError::InvalidState(format!(
                "unknown progressive layer lifecycle {value}"
            ))),
        }
    }
}

impl From<FrameIndexLifecycle> for ProgressiveLayerLifecycle {
    fn from(value: FrameIndexLifecycle) -> Self {
        match value {
            FrameIndexLifecycle::Building => Self::Building,
            FrameIndexLifecycle::Incomplete => Self::Incomplete,
            FrameIndexLifecycle::Complete => Self::Complete,
            FrameIndexLifecycle::FailedRecoverable => Self::FailedRecoverable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressiveLayerStatus {
    pub layer: ProgressiveIndexLayer,
    pub lifecycle: ProgressiveLayerLifecycle,
    pub generation: u32,
    /// Layer-specific work units. For the compatibility structural snapshot this is GOP/sync-point
    /// count; for the exact layer it is authoritative FrameId count.
    pub completed_units: u64,
    pub total_units: Option<u64>,
    /// Contiguous authoritative FrameId prefix represented by this layer, when one is proven.
    pub covered_frame_prefix: Option<u64>,
    /// Exact stream-time coverage when known. This is never derived from nominal FPS.
    pub covered_through_timestamp: Option<MediaTimestamp>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuralAnchorKind {
    DecodedCleanKeyframe,
    PacketSyncPoint,
}

impl StructuralAnchorKind {
    fn as_i64(self) -> i64 {
        match self {
            Self::DecodedCleanKeyframe => 1,
            Self::PacketSyncPoint => 2,
        }
    }

    fn from_i64(value: i64) -> Result<Self, ProgressiveIndexError> {
        match value {
            1 => Ok(Self::DecodedCleanKeyframe),
            2 => Ok(Self::PacketSyncPoint),
            _ => Err(ProgressiveIndexError::InvalidState(format!(
                "unknown structural anchor kind {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralAnchor {
    pub ordinal: u64,
    pub kind: StructuralAnchorKind,
    /// Present only after an anchor is reconciled to authoritative display identity.
    pub frame_id: Option<FrameId>,
    pub presentation_timestamp: Option<MediaTimestamp>,
    /// Exact FrameId boundary of the GOP when known. Exclusive.
    pub gop_end_frame_exclusive: Option<FrameId>,
    /// Timeline boundary of the next GOP/sync point when known.
    pub gop_end_timestamp: Option<MediaTimestamp>,
    /// Container packet ordinal when a trustworthy demux path supplies one.
    pub packet_index: Option<u64>,
    /// Container/source byte position when trustworthy. FFmpeg may report this as unknown.
    pub byte_offset: Option<u64>,
    /// Packet PTS/DTS are distinct from decoded presentation identity and remain optional.
    pub packet_pts: Option<MediaTimestamp>,
    pub packet_dts: Option<MediaTimestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualArtifactKind {
    Thumbnail,
    ScrubPreview,
}

impl VisualArtifactKind {
    fn as_i64(self) -> i64 {
        match self {
            Self::Thumbnail => 1,
            Self::ScrubPreview => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisualArtifactMetadata {
    pub kind: VisualArtifactKind,
    /// Exact identity when the artifact was generated from an authoritative frame.
    pub frame_id: Option<FrameId>,
    /// Exact stream timestamp when known. A timestamp-only artifact is provisional, not FrameId
    /// authority.
    pub presentation_timestamp: Option<MediaTimestamp>,
    /// Pyramid/profile tier owned by the visual cache producer.
    pub tier: u32,
    pub generation: u32,
    /// Relative storage key inside the producer-owned FrameScope cache namespace.
    pub storage_key: String,
    pub width: u32,
    pub height: u32,
    pub byte_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimilarityFingerprintStatus {
    pub lifecycle: ProgressiveLayerLifecycle,
    pub generation: u32,
    pub covered_frames: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressiveIndexOpenDisposition {
    Created,
    Reused,
    RebuiltStaleAuthority,
    RebuiltUnverifiableAuthority,
    RebuiltIncompatibleAuthority,
    RecreatedUnsupportedSchema,
    RecoveredCorruptState,
}

#[derive(Debug, Error)]
pub enum ProgressiveIndexError {
    #[error("SQLite progressive-index error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("progressive-index I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("progressive-index serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("authoritative frame-index error: {0}")]
    Authority(#[from] FrameIndexError),
    #[error("invalid progressive-index state: {0}")]
    InvalidState(String),
    #[error("unsupported progressive-index schema version {found}")]
    UnsupportedSchema { found: i64 },
}

impl ProgressiveIndexError {
    fn recreation_disposition(&self) -> Option<ProgressiveIndexOpenDisposition> {
        match self {
            Self::UnsupportedSchema { .. } => {
                Some(ProgressiveIndexOpenDisposition::RecreatedUnsupportedSchema)
            }
            Self::InvalidState(_) | Self::Serialization(_) => {
                Some(ProgressiveIndexOpenDisposition::RecoveredCorruptState)
            }
            Self::Sqlite(rusqlite::Error::SqliteFailure(error, _))
                if matches!(
                    error.code,
                    rusqlite::ErrorCode::DatabaseCorrupt
                        | rusqlite::ErrorCode::NotADatabase
                        | rusqlite::ErrorCode::SchemaChanged
                ) =>
            {
                Some(ProgressiveIndexOpenDisposition::RecoveredCorruptState)
            }
            _ => None,
        }
    }
}

#[derive(Debug)]
struct ProgressiveMetaRow {
    source_identity: SourceIdentity,
    stream_identity: FrameIndexStreamIdentity,
    authority_schema_version: i64,
    timeline_contract_generation: u32,
}

pub struct ProgressiveFrameIndex {
    connection: Connection,
    path: PathBuf,
    source_identity: SourceIdentity,
    stream_identity: FrameIndexStreamIdentity,
}

impl ProgressiveFrameIndex {
    /// Default companion path. It stays beside the authoritative SQLite file so existing FrameScope
    /// per-source storage cleanup removes both databases together.
    pub fn companion_path(authoritative: &FrameIndex) -> PathBuf {
        let mut value = authoritative.path().as_os_str().to_os_string();
        value.push(".v2.sqlite3");
        PathBuf::from(value)
    }

    /// Open the additive V2 catalog without mutating the authoritative V1 frame database.
    pub fn open_or_create(
        path: impl AsRef<Path>,
        authoritative: &FrameIndex,
    ) -> Result<(Self, ProgressiveIndexOpenDisposition), ProgressiveIndexError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let existed = path.exists();
        match Self::try_open(path.clone(), authoritative) {
            Ok((index, disposition)) => Ok((
                index,
                if existed {
                    disposition
                } else {
                    ProgressiveIndexOpenDisposition::Created
                },
            )),
            Err(error) => {
                let Some(disposition) = error.recreation_disposition() else {
                    return Err(error);
                };
                purge_database_files(&path)?;
                let (index, _) = Self::try_open(path, authoritative)?;
                Ok((index, disposition))
            }
        }
    }

    fn try_open(
        path: PathBuf,
        authoritative: &FrameIndex,
    ) -> Result<(Self, ProgressiveIndexOpenDisposition), ProgressiveIndexError> {
        let connection = Connection::open(&path)?;
        configure_connection(&connection)?;
        migrate(&connection)?;
        let mut index = Self {
            connection,
            path,
            source_identity: authoritative.source_identity().clone(),
            stream_identity: authoritative.stream_identity().clone(),
        };
        let disposition = match index.load_meta()? {
            None => {
                index.initialize_meta()?;
                ProgressiveIndexOpenDisposition::Created
            }
            Some(_) if !authoritative.source_identity().is_reuse_safe() => {
                index.reset_and_rebind()?;
                ProgressiveIndexOpenDisposition::RebuiltUnverifiableAuthority
            }
            Some(meta)
                if meta.source_identity != *authoritative.source_identity()
                    || meta.stream_identity != *authoritative.stream_identity() =>
            {
                index.reset_and_rebind()?;
                ProgressiveIndexOpenDisposition::RebuiltStaleAuthority
            }
            Some(meta)
                if meta.authority_schema_version != FRAME_INDEX_SCHEMA_VERSION
                    || meta.timeline_contract_generation != FRAME_TIMELINE_CONTRACT_GENERATION =>
            {
                index.reset_and_rebind()?;
                ProgressiveIndexOpenDisposition::RebuiltIncompatibleAuthority
            }
            Some(_) => ProgressiveIndexOpenDisposition::Reused,
        };
        index.validate_catalog()?;
        Ok((index, disposition))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn source_identity(&self) -> &SourceIdentity {
        &self.source_identity
    }

    pub fn stream_identity(&self) -> &FrameIndexStreamIdentity {
        &self.stream_identity
    }

    pub fn layer_status(
        &self,
        layer: ProgressiveIndexLayer,
    ) -> Result<ProgressiveLayerStatus, ProgressiveIndexError> {
        let row = self.connection.query_row(
            "SELECT layer, lifecycle, generation, completed_units, total_units,
                    covered_frame_prefix, covered_timestamp_ticks,
                    covered_time_base_num, covered_time_base_den, last_error
             FROM layer_status WHERE layer = ?1",
            params![layer.as_i64()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<i32>>(7)?,
                    row.get::<_, Option<i32>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )?;
        Ok(ProgressiveLayerStatus {
            layer: ProgressiveIndexLayer::from_i64(row.0)?,
            lifecycle: ProgressiveLayerLifecycle::from_i64(row.1)?,
            generation: to_u32(row.2, "layer generation")?,
            completed_units: to_u64(row.3, "completed units")?,
            total_units: row.4.map(|value| to_u64(value, "total units")).transpose()?,
            covered_frame_prefix: row
                .5
                .map(|value| to_u64(value, "covered frame prefix"))
                .transpose()?,
            covered_through_timestamp: decode_timestamp(row.6, row.7, row.8)?,
            last_error: row.9,
        })
    }

    pub fn structural_anchor(
        &self,
        ordinal: u64,
    ) -> Result<Option<StructuralAnchor>, ProgressiveIndexError> {
        let row = self
            .connection
            .query_row(
                "SELECT anchor_ordinal, anchor_kind, frame_index,
                        presentation_timestamp_ticks, presentation_time_base_num,
                        presentation_time_base_den, gop_end_frame_exclusive,
                        gop_end_timestamp_ticks, gop_end_time_base_num, gop_end_time_base_den,
                        packet_index, byte_offset, packet_pts_ticks, packet_pts_time_base_num,
                        packet_pts_time_base_den, packet_dts_ticks, packet_dts_time_base_num,
                        packet_dts_time_base_den
                 FROM navigation_anchor WHERE anchor_ordinal = ?1",
                params![to_i64(ordinal, "anchor ordinal")?],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<i32>>(4)?,
                        row.get::<_, Option<i32>>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, Option<i64>>(7)?,
                        row.get::<_, Option<i32>>(8)?,
                        row.get::<_, Option<i32>>(9)?,
                        row.get::<_, Option<i64>>(10)?,
                        row.get::<_, Option<i64>>(11)?,
                        row.get::<_, Option<i64>>(12)?,
                        row.get::<_, Option<i32>>(13)?,
                        row.get::<_, Option<i32>>(14)?,
                        row.get::<_, Option<i64>>(15)?,
                        row.get::<_, Option<i32>>(16)?,
                        row.get::<_, Option<i32>>(17)?,
                    ))
                },
            )
            .optional()?;
        row.map(decode_structural_anchor).transpose()
    }

    pub fn structural_anchor_count(&self) -> Result<u64, ProgressiveIndexError> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM navigation_anchor", [], |row| row.get(0))?;
        to_u64(count, "structural anchor count")
    }

    /// Compatibility bootstrap from the current authoritative frame table.
    ///
    /// This is intentionally metadata-only and transactional. It does not decode media and never
    /// modifies the authoritative database. Packet index, byte offset, packet PTS, and packet DTS
    /// remain NULL because the current decoded-frame contract does not expose trustworthy values.
    pub fn synchronize_authoritative_snapshot(
        &mut self,
        authoritative: &FrameIndex,
    ) -> Result<(), ProgressiveIndexError> {
        self.ensure_authority_binding(authoritative)?;
        let status = authoritative.status()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("DELETE FROM navigation_anchor", [])?;

        let mut insert = transaction.prepare_cached(
            "INSERT INTO navigation_anchor (
                anchor_ordinal, anchor_kind, frame_index,
                presentation_timestamp_ticks, presentation_time_base_num,
                presentation_time_base_den, gop_end_frame_exclusive,
                gop_end_timestamp_ticks, gop_end_time_base_num, gop_end_time_base_den,
                packet_index, byte_offset, packet_pts_ticks, packet_pts_time_base_num,
                packet_pts_time_base_den, packet_dts_ticks, packet_dts_time_base_num,
                packet_dts_time_base_den
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, NULL, NULL,
                       NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL)",
        )?;
        let mut close_previous = transaction.prepare_cached(
            "UPDATE navigation_anchor
             SET gop_end_frame_exclusive = ?1,
                 gop_end_timestamp_ticks = ?2,
                 gop_end_time_base_num = ?3,
                 gop_end_time_base_den = ?4
             WHERE anchor_ordinal = ?5",
        )?;

        let mut next_ordinal = 0_u64;
        let mut previous_ordinal = None;
        authoritative.visit_range(FrameId::ZERO, FrameId(status.indexed_frames), |entry| {
            if !entry.keyframe || entry.corrupt {
                return Ok(());
            }
            let ordinal_i64 = i64::try_from(next_ordinal).map_err(|_| {
                FrameIndexError::InvalidState("structural anchor ordinal overflow".into())
            })?;
            let frame_i64 = i64::try_from(entry.frame_id.0)
                .map_err(|_| FrameIndexError::InvalidState("frame id overflow".into()))?;
            let (ticks, num, den) = encode_timestamp(entry.presentation_timestamp);
            if let Some(previous) = previous_ordinal {
                close_previous.execute(params![frame_i64, ticks, num, den, previous])?;
            }
            insert.execute(params![
                ordinal_i64,
                StructuralAnchorKind::DecodedCleanKeyframe.as_i64(),
                frame_i64,
                ticks,
                num,
                den,
            ])?;
            previous_ordinal = Some(ordinal_i64);
            next_ordinal = next_ordinal.checked_add(1).ok_or_else(|| {
                FrameIndexError::InvalidState("structural anchor count overflow".into())
            })?;
            Ok(())
        })?;

        if status.lifecycle == FrameIndexLifecycle::Complete {
            if let (Some(previous), Some(frame_count)) = (previous_ordinal, status.frame_count) {
                close_previous.execute(params![
                    i64::try_from(frame_count).map_err(|_| {
                        ProgressiveIndexError::InvalidState("frame count overflow".into())
                    })?,
                    Option::<i64>::None,
                    Option::<i32>::None,
                    Option::<i32>::None,
                    previous,
                ])?;
            }
        }
        drop(insert);
        drop(close_previous);

        let lifecycle = ProgressiveLayerLifecycle::from(status.lifecycle);
        write_layer_status(
            &transaction,
            ProgressiveIndexLayer::StructuralNavigation,
            lifecycle,
            STRUCTURAL_INDEX_GENERATION,
            next_ordinal,
            (status.lifecycle == FrameIndexLifecycle::Complete).then_some(next_ordinal),
            Some(status.indexed_frames),
            status.last_presentation_timestamp,
            status.last_error.as_deref(),
        )?;
        write_layer_status(
            &transaction,
            ProgressiveIndexLayer::AuthoritativeExact,
            lifecycle,
            FRAME_TIMELINE_CONTRACT_GENERATION,
            status.indexed_frames,
            status.frame_count,
            Some(status.indexed_frames),
            status.last_presentation_timestamp,
            status.last_error.as_deref(),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn update_visual_layer_status(
        &mut self,
        status: ProgressiveLayerStatus,
    ) -> Result<(), ProgressiveIndexError> {
        if status.layer != ProgressiveIndexLayer::VisualAcceleration {
            return Err(ProgressiveIndexError::InvalidState(
                "only the visual acceleration layer may be updated through this API".into(),
            ));
        }
        if status.generation == 0 {
            return Err(ProgressiveIndexError::InvalidState(
                "visual layer generation must be positive".into(),
            ));
        }
        self.ensure_stream_timestamp(status.covered_through_timestamp)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        write_layer_status(
            &transaction,
            status.layer,
            status.lifecycle,
            status.generation,
            status.completed_units,
            status.total_units,
            status.covered_frame_prefix,
            status.covered_through_timestamp,
            status.last_error.as_deref(),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn record_visual_artifact(
        &mut self,
        artifact: &VisualArtifactMetadata,
    ) -> Result<(), ProgressiveIndexError> {
        validate_visual_artifact(artifact, &self.stream_identity)?;
        let (ticks, num, den) = encode_timestamp(artifact.presentation_timestamp);
        self.connection.execute(
            "INSERT INTO visual_artifact (
                storage_key, artifact_kind, frame_index, presentation_timestamp_ticks,
                presentation_time_base_num, presentation_time_base_den, tier, generation,
                width, height, byte_size
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(storage_key) DO UPDATE SET
                artifact_kind = excluded.artifact_kind,
                frame_index = excluded.frame_index,
                presentation_timestamp_ticks = excluded.presentation_timestamp_ticks,
                presentation_time_base_num = excluded.presentation_time_base_num,
                presentation_time_base_den = excluded.presentation_time_base_den,
                tier = excluded.tier,
                generation = excluded.generation,
                width = excluded.width,
                height = excluded.height,
                byte_size = excluded.byte_size",
            params![
                artifact.storage_key,
                artifact.kind.as_i64(),
                artifact
                    .frame_id
                    .map(|frame| to_i64(frame.0, "visual artifact frame id"))
                    .transpose()?,
                ticks,
                num,
                den,
                i64::from(artifact.tier),
                i64::from(artifact.generation),
                i64::from(artifact.width),
                i64::from(artifact.height),
                to_i64(artifact.byte_size, "visual artifact byte size")?,
            ],
        )?;
        Ok(())
    }

    pub fn visual_artifact_count(&self) -> Result<u64, ProgressiveIndexError> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM visual_artifact", [], |row| row.get(0))?;
        to_u64(count, "visual artifact count")
    }

    pub fn similarity_fingerprint_status(
        &self,
    ) -> Result<SimilarityFingerprintStatus, ProgressiveIndexError> {
        let row = self.connection.query_row(
            "SELECT lifecycle, generation, covered_frames, last_error
             FROM similarity_fingerprint_status WHERE id = ?1",
            params![SIMILARITY_STATUS_ROW_ID],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )?;
        Ok(SimilarityFingerprintStatus {
            lifecycle: ProgressiveLayerLifecycle::from_i64(row.0)?,
            generation: to_u32(row.1, "similarity generation")?,
            covered_frames: to_u64(row.2, "similarity covered frames")?,
            last_error: row.3,
        })
    }

    pub fn update_similarity_fingerprint_status(
        &mut self,
        status: &SimilarityFingerprintStatus,
    ) -> Result<(), ProgressiveIndexError> {
        if status.generation == 0 {
            return Err(ProgressiveIndexError::InvalidState(
                "similarity fingerprint generation must be positive".into(),
            ));
        }
        self.connection.execute(
            "UPDATE similarity_fingerprint_status
             SET lifecycle = ?1, generation = ?2, covered_frames = ?3, last_error = ?4
             WHERE id = ?5",
            params![
                status.lifecycle.as_i64(),
                i64::from(status.generation),
                to_i64(status.covered_frames, "similarity covered frames")?,
                status.last_error,
                SIMILARITY_STATUS_ROW_ID,
            ],
        )?;
        Ok(())
    }

    fn initialize_meta(&mut self) -> Result<(), ProgressiveIndexError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO progressive_meta (
                id, source_key, source_identity_json, stream_identity_json,
                authority_schema_version, timeline_contract_generation
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                META_ROW_ID,
                self.source_identity.stable_key(),
                serde_json::to_string(&self.source_identity)?,
                serde_json::to_string(&self.stream_identity)?,
                FRAME_INDEX_SCHEMA_VERSION,
                i64::from(FRAME_TIMELINE_CONTRACT_GENERATION),
            ],
        )?;
        initialize_status_rows(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    fn reset_and_rebind(&mut self) -> Result<(), ProgressiveIndexError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("DELETE FROM navigation_anchor", [])?;
        transaction.execute("DELETE FROM visual_artifact", [])?;
        transaction.execute("DELETE FROM similarity_fingerprint_status", [])?;
        transaction.execute("DELETE FROM layer_status", [])?;
        transaction.execute("DELETE FROM progressive_meta", [])?;
        transaction.execute(
            "INSERT INTO progressive_meta (
                id, source_key, source_identity_json, stream_identity_json,
                authority_schema_version, timeline_contract_generation
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                META_ROW_ID,
                self.source_identity.stable_key(),
                serde_json::to_string(&self.source_identity)?,
                serde_json::to_string(&self.stream_identity)?,
                FRAME_INDEX_SCHEMA_VERSION,
                i64::from(FRAME_TIMELINE_CONTRACT_GENERATION),
            ],
        )?;
        initialize_status_rows(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    fn load_meta(&self) -> Result<Option<ProgressiveMetaRow>, ProgressiveIndexError> {
        let row = self
            .connection
            .query_row(
                "SELECT source_key, source_identity_json, stream_identity_json,
                        authority_schema_version, timeline_contract_generation
                 FROM progressive_meta WHERE id = ?1",
                params![META_ROW_ID],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((source_key, source_json, stream_json, authority_schema, timeline_generation)) = row
        else {
            return Ok(None);
        };
        let source_identity: SourceIdentity = serde_json::from_str(&source_json)?;
        if source_key != source_identity.stable_key() {
            return Err(ProgressiveIndexError::InvalidState(
                "stored progressive source key does not match source identity".into(),
            ));
        }
        Ok(Some(ProgressiveMetaRow {
            source_identity,
            stream_identity: serde_json::from_str(&stream_json)?,
            authority_schema_version: authority_schema,
            timeline_contract_generation: to_u32(
                timeline_generation,
                "timeline contract generation",
            )?,
        }))
    }

    fn ensure_authority_binding(
        &self,
        authoritative: &FrameIndex,
    ) -> Result<(), ProgressiveIndexError> {
        if authoritative.source_identity() != &self.source_identity
            || authoritative.stream_identity() != &self.stream_identity
        {
            return Err(ProgressiveIndexError::InvalidState(
                "progressive index is bound to a different authoritative source or stream".into(),
            ));
        }
        Ok(())
    }

    fn ensure_stream_timestamp(
        &self,
        timestamp: Option<MediaTimestamp>,
    ) -> Result<(), ProgressiveIndexError> {
        if timestamp.is_some_and(|value| value.time_base != self.stream_identity.time_base) {
            return Err(ProgressiveIndexError::InvalidState(
                "progressive layer timestamp uses a different stream time base".into(),
            ));
        }
        Ok(())
    }

    fn validate_catalog(&self) -> Result<(), ProgressiveIndexError> {
        let layer_count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM layer_status WHERE layer IN (1, 2, 3)",
            [],
            |row| row.get(0),
        )?;
        if layer_count != 3 {
            return Err(ProgressiveIndexError::InvalidState(
                "progressive index must contain exactly three layer status rows".into(),
            ));
        }
        for layer in [
            ProgressiveIndexLayer::StructuralNavigation,
            ProgressiveIndexLayer::VisualAcceleration,
            ProgressiveIndexLayer::AuthoritativeExact,
        ] {
            let status = self.layer_status(layer)?;
            self.ensure_stream_timestamp(status.covered_through_timestamp)?;
            if status.lifecycle == ProgressiveLayerLifecycle::Complete
                && status.total_units != Some(status.completed_units)
            {
                return Err(ProgressiveIndexError::InvalidState(format!(
                    "complete {:?} layer has inconsistent unit totals",
                    status.layer
                )));
            }
        }
        Ok(())
    }
}

fn initialize_status_rows(transaction: &rusqlite::Transaction<'_>) -> Result<(), ProgressiveIndexError> {
    for (layer, generation) in [
        (ProgressiveIndexLayer::StructuralNavigation, STRUCTURAL_INDEX_GENERATION),
        (ProgressiveIndexLayer::VisualAcceleration, VISUAL_INDEX_GENERATION),
        (
            ProgressiveIndexLayer::AuthoritativeExact,
            FRAME_TIMELINE_CONTRACT_GENERATION,
        ),
    ] {
        transaction.execute(
            "INSERT INTO layer_status (
                layer, lifecycle, generation, completed_units, total_units,
                covered_frame_prefix, covered_timestamp_ticks, covered_time_base_num,
                covered_time_base_den, last_error
             ) VALUES (?1, ?2, ?3, 0, NULL, NULL, NULL, NULL, NULL, NULL)",
            params![
                layer.as_i64(),
                ProgressiveLayerLifecycle::NotStarted.as_i64(),
                i64::from(generation),
            ],
        )?;
    }
    transaction.execute(
        "INSERT INTO similarity_fingerprint_status (
            id, lifecycle, generation, covered_frames, last_error
         ) VALUES (?1, ?2, ?3, 0, NULL)",
        params![
            SIMILARITY_STATUS_ROW_ID,
            ProgressiveLayerLifecycle::NotStarted.as_i64(),
            i64::from(VISUAL_INDEX_GENERATION),
        ],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_layer_status(
    transaction: &rusqlite::Transaction<'_>,
    layer: ProgressiveIndexLayer,
    lifecycle: ProgressiveLayerLifecycle,
    generation: u32,
    completed_units: u64,
    total_units: Option<u64>,
    covered_frame_prefix: Option<u64>,
    covered_timestamp: Option<MediaTimestamp>,
    last_error: Option<&str>,
) -> Result<(), ProgressiveIndexError> {
    if generation == 0 {
        return Err(ProgressiveIndexError::InvalidState(
            "progressive layer generation must be positive".into(),
        ));
    }
    if lifecycle == ProgressiveLayerLifecycle::Complete && total_units != Some(completed_units) {
        return Err(ProgressiveIndexError::InvalidState(
            "complete progressive layer must have matching completed and total units".into(),
        ));
    }
    let (ticks, num, den) = encode_timestamp(covered_timestamp);
    transaction.execute(
        "UPDATE layer_status
         SET lifecycle = ?1, generation = ?2, completed_units = ?3, total_units = ?4,
             covered_frame_prefix = ?5, covered_timestamp_ticks = ?6,
             covered_time_base_num = ?7, covered_time_base_den = ?8, last_error = ?9
         WHERE layer = ?10",
        params![
            lifecycle.as_i64(),
            i64::from(generation),
            to_i64(completed_units, "completed units")?,
            total_units
                .map(|value| to_i64(value, "total units"))
                .transpose()?,
            covered_frame_prefix
                .map(|value| to_i64(value, "covered frame prefix"))
                .transpose()?,
            ticks,
            num,
            den,
            last_error,
            layer.as_i64(),
        ],
    )?;
    Ok(())
}

fn validate_visual_artifact(
    artifact: &VisualArtifactMetadata,
    stream: &FrameIndexStreamIdentity,
) -> Result<(), ProgressiveIndexError> {
    if artifact.frame_id.is_none() && artifact.presentation_timestamp.is_none() {
        return Err(ProgressiveIndexError::InvalidState(
            "visual artifact requires an authoritative FrameId or exact stream timestamp".into(),
        ));
    }
    if artifact.generation == 0 || artifact.width == 0 || artifact.height == 0 {
        return Err(ProgressiveIndexError::InvalidState(
            "visual artifact generation and dimensions must be positive".into(),
        ));
    }
    if artifact
        .presentation_timestamp
        .is_some_and(|value| value.time_base != stream.time_base)
    {
        return Err(ProgressiveIndexError::InvalidState(
            "visual artifact timestamp uses a different stream time base".into(),
        ));
    }
    if !safe_relative_storage_key(&artifact.storage_key) {
        return Err(ProgressiveIndexError::InvalidState(
            "visual artifact storage key must be a confined relative path".into(),
        ));
    }
    Ok(())
}

fn safe_relative_storage_key(value: &str) -> bool {
    if value.is_empty() || value.len() > 1_024 {
        return false;
    }
    let path = Path::new(value);
    !path.is_absolute()
        && path.components().all(|component| matches!(component, Component::Normal(_)))
}

fn decode_structural_anchor(
    row: (
        i64,
        i64,
        Option<i64>,
        Option<i64>,
        Option<i32>,
        Option<i32>,
        Option<i64>,
        Option<i64>,
        Option<i32>,
        Option<i32>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i32>,
        Option<i32>,
        Option<i64>,
        Option<i32>,
        Option<i32>,
    ),
) -> Result<StructuralAnchor, ProgressiveIndexError> {
    Ok(StructuralAnchor {
        ordinal: to_u64(row.0, "anchor ordinal")?,
        kind: StructuralAnchorKind::from_i64(row.1)?,
        frame_id: row.2.map(|value| to_u64(value, "anchor frame id").map(FrameId)).transpose()?,
        presentation_timestamp: decode_timestamp(row.3, row.4, row.5)?,
        gop_end_frame_exclusive: row
            .6
            .map(|value| to_u64(value, "GOP end frame").map(FrameId))
            .transpose()?,
        gop_end_timestamp: decode_timestamp(row.7, row.8, row.9)?,
        packet_index: row.10.map(|value| to_u64(value, "packet index")).transpose()?,
        byte_offset: row.11.map(|value| to_u64(value, "byte offset")).transpose()?,
        packet_pts: decode_timestamp(row.12, row.13, row.14)?,
        packet_dts: decode_timestamp(row.15, row.16, row.17)?,
    })
}

fn encode_timestamp(timestamp: Option<MediaTimestamp>) -> (Option<i64>, Option<i32>, Option<i32>) {
    match timestamp {
        Some(value) => (
            Some(value.ticks),
            Some(value.time_base.numerator),
            Some(value.time_base.denominator),
        ),
        None => (None, None, None),
    }
}

fn decode_timestamp(
    ticks: Option<i64>,
    numerator: Option<i32>,
    denominator: Option<i32>,
) -> Result<Option<MediaTimestamp>, ProgressiveIndexError> {
    match (ticks, numerator, denominator) {
        (None, None, None) => Ok(None),
        (Some(ticks), Some(numerator), Some(denominator)) => {
            let time_base = TimeBase::new(numerator, denominator).ok_or_else(|| {
                ProgressiveIndexError::InvalidState("stored timestamp time base is invalid".into())
            })?;
            Ok(Some(MediaTimestamp { ticks, time_base }))
        }
        _ => Err(ProgressiveIndexError::InvalidState(
            "stored timestamp fields are only partially populated".into(),
        )),
    }
}

fn configure_connection(connection: &Connection) -> Result<(), ProgressiveIndexError> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

fn migrate(connection: &Connection) -> Result<(), ProgressiveIndexError> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match version {
        PROGRESSIVE_INDEX_SCHEMA_VERSION => Ok(()),
        0 => {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE progressive_meta (
                    id INTEGER PRIMARY KEY CHECK(id = 1),
                    source_key TEXT NOT NULL,
                    source_identity_json TEXT NOT NULL,
                    stream_identity_json TEXT NOT NULL,
                    authority_schema_version INTEGER NOT NULL CHECK(authority_schema_version > 0),
                    timeline_contract_generation INTEGER NOT NULL CHECK(timeline_contract_generation > 0)
                 );
                 CREATE TABLE layer_status (
                    layer INTEGER PRIMARY KEY CHECK(layer BETWEEN 1 AND 3),
                    lifecycle INTEGER NOT NULL CHECK(lifecycle BETWEEN 0 AND 4),
                    generation INTEGER NOT NULL CHECK(generation > 0),
                    completed_units INTEGER NOT NULL CHECK(completed_units >= 0),
                    total_units INTEGER CHECK(total_units >= 0),
                    covered_frame_prefix INTEGER CHECK(covered_frame_prefix >= 0),
                    covered_timestamp_ticks INTEGER,
                    covered_time_base_num INTEGER,
                    covered_time_base_den INTEGER,
                    last_error TEXT,
                    CHECK((covered_timestamp_ticks IS NULL) = (covered_time_base_num IS NULL)),
                    CHECK((covered_timestamp_ticks IS NULL) = (covered_time_base_den IS NULL))
                 );
                 CREATE TABLE navigation_anchor (
                    anchor_ordinal INTEGER PRIMARY KEY CHECK(anchor_ordinal >= 0),
                    anchor_kind INTEGER NOT NULL CHECK(anchor_kind IN (1, 2)),
                    frame_index INTEGER CHECK(frame_index >= 0),
                    presentation_timestamp_ticks INTEGER,
                    presentation_time_base_num INTEGER,
                    presentation_time_base_den INTEGER,
                    gop_end_frame_exclusive INTEGER CHECK(gop_end_frame_exclusive >= 0),
                    gop_end_timestamp_ticks INTEGER,
                    gop_end_time_base_num INTEGER,
                    gop_end_time_base_den INTEGER,
                    packet_index INTEGER CHECK(packet_index >= 0),
                    byte_offset INTEGER CHECK(byte_offset >= 0),
                    packet_pts_ticks INTEGER,
                    packet_pts_time_base_num INTEGER,
                    packet_pts_time_base_den INTEGER,
                    packet_dts_ticks INTEGER,
                    packet_dts_time_base_num INTEGER,
                    packet_dts_time_base_den INTEGER,
                    CHECK((presentation_timestamp_ticks IS NULL) = (presentation_time_base_num IS NULL)),
                    CHECK((presentation_timestamp_ticks IS NULL) = (presentation_time_base_den IS NULL)),
                    CHECK((gop_end_timestamp_ticks IS NULL) = (gop_end_time_base_num IS NULL)),
                    CHECK((gop_end_timestamp_ticks IS NULL) = (gop_end_time_base_den IS NULL)),
                    CHECK((packet_pts_ticks IS NULL) = (packet_pts_time_base_num IS NULL)),
                    CHECK((packet_pts_ticks IS NULL) = (packet_pts_time_base_den IS NULL)),
                    CHECK((packet_dts_ticks IS NULL) = (packet_dts_time_base_num IS NULL)),
                    CHECK((packet_dts_ticks IS NULL) = (packet_dts_time_base_den IS NULL)),
                    CHECK(frame_index IS NOT NULL OR packet_index IS NOT NULL OR byte_offset IS NOT NULL)
                 );
                 CREATE UNIQUE INDEX navigation_anchor_frame
                    ON navigation_anchor(frame_index) WHERE frame_index IS NOT NULL;
                 CREATE INDEX navigation_anchor_presentation_timestamp
                    ON navigation_anchor(presentation_timestamp_ticks, anchor_ordinal)
                    WHERE presentation_timestamp_ticks IS NOT NULL;
                 CREATE INDEX navigation_anchor_packet_pts
                    ON navigation_anchor(packet_pts_ticks, anchor_ordinal)
                    WHERE packet_pts_ticks IS NOT NULL;
                 CREATE TABLE visual_artifact (
                    storage_key TEXT PRIMARY KEY,
                    artifact_kind INTEGER NOT NULL CHECK(artifact_kind IN (1, 2)),
                    frame_index INTEGER CHECK(frame_index >= 0),
                    presentation_timestamp_ticks INTEGER,
                    presentation_time_base_num INTEGER,
                    presentation_time_base_den INTEGER,
                    tier INTEGER NOT NULL CHECK(tier >= 0),
                    generation INTEGER NOT NULL CHECK(generation > 0),
                    width INTEGER NOT NULL CHECK(width > 0),
                    height INTEGER NOT NULL CHECK(height > 0),
                    byte_size INTEGER NOT NULL CHECK(byte_size >= 0),
                    CHECK(frame_index IS NOT NULL OR presentation_timestamp_ticks IS NOT NULL),
                    CHECK((presentation_timestamp_ticks IS NULL) = (presentation_time_base_num IS NULL)),
                    CHECK((presentation_timestamp_ticks IS NULL) = (presentation_time_base_den IS NULL))
                 );
                 CREATE INDEX visual_artifact_frame
                    ON visual_artifact(frame_index, artifact_kind, tier)
                    WHERE frame_index IS NOT NULL;
                 CREATE TABLE similarity_fingerprint_status (
                    id INTEGER PRIMARY KEY CHECK(id = 1),
                    lifecycle INTEGER NOT NULL CHECK(lifecycle BETWEEN 0 AND 4),
                    generation INTEGER NOT NULL CHECK(generation > 0),
                    covered_frames INTEGER NOT NULL CHECK(covered_frames >= 0),
                    last_error TEXT
                 );
                 PRAGMA user_version = 1;
                 COMMIT;",
            )?;
            Ok(())
        }
        found => Err(ProgressiveIndexError::UnsupportedSchema { found }),
    }
}

fn purge_database_files(path: &Path) -> Result<(), ProgressiveIndexError> {
    remove_if_exists(path)?;
    remove_if_exists(&sidecar_path(path, "-wal"))?;
    remove_if_exists(&sidecar_path(path, "-shm"))?;
    Ok(())
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn remove_if_exists(path: &Path) -> Result<(), std::io::Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn to_i64(value: u64, label: &str) -> Result<i64, ProgressiveIndexError> {
    i64::try_from(value)
        .map_err(|_| ProgressiveIndexError::InvalidState(format!("{label} exceeds SQLite i64 range")))
}

fn to_u64(value: i64, label: &str) -> Result<u64, ProgressiveIndexError> {
    u64::try_from(value)
        .map_err(|_| ProgressiveIndexError::InvalidState(format!("{label} is negative")))
}

fn to_u32(value: i64, label: &str) -> Result<u32, ProgressiveIndexError> {
    u32::try_from(value).map_err(|_| {
        ProgressiveIndexError::InvalidState(format!("{label} is outside the supported u32 range"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameIndexEntry, FrameIndexOpenDisposition, KeyframeAnchor};
    use framescope_core::{MediaDuration, MediaKind, StreamInfo};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_path(name: &str) -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "framescope-progressive-index-{}-{name}-{id}.sqlite3",
            std::process::id()
        ))
    }

    fn source(tag: &str) -> SourceIdentity {
        SourceIdentity::new(10_000, Some(123), Some(format!("proof:{tag}")))
    }

    fn stream() -> FrameIndexStreamIdentity {
        let info = StreamInfo {
            index: 0,
            media_kind: MediaKind::Video,
            codec: framescope_core::CodecInfo {
                id: 27,
                name: "h264".into(),
                decoder_available: true,
            },
            is_default: true,
            time_base: Some(TimeBase::new(1, 1_000).unwrap()),
            duration: None,
            frame_count: None,
            width: Some(64),
            height: Some(48),
            pixel_format: None,
            average_frame_rate: None,
            nominal_frame_rate: None,
            rotation_degrees: None,
        };
        FrameIndexStreamIdentity::from_stream(&info).unwrap()
    }

    fn entry(frame: u64, ticks: i64, keyframe: bool, anchor: u64, anchor_ticks: i64) -> FrameIndexEntry {
        let time_base = TimeBase::new(1, 1_000).unwrap();
        FrameIndexEntry {
            frame_id: FrameId(frame),
            presentation_timestamp: Some(MediaTimestamp { ticks, time_base }),
            duration: Some(MediaDuration {
                ticks: 40,
                time_base,
            }),
            keyframe,
            corrupt: false,
            anchor: KeyframeAnchor::Keyframe {
                frame_id: FrameId(anchor),
                presentation_timestamp: Some(MediaTimestamp {
                    ticks: anchor_ticks,
                    time_base,
                }),
            },
        }
    }

    fn cleanup(path: &Path) {
        let _ = purge_database_files(path);
    }

    #[test]
    fn completed_v1_index_bootstraps_v2_without_mutating_authority() {
        let authority_path = temp_path("authority-complete");
        let companion_path = temp_path("companion-complete");
        let (mut authority, disposition) =
            FrameIndex::open_or_create(&authority_path, source("complete"), stream()).unwrap();
        assert_eq!(disposition, FrameIndexOpenDisposition::Created);
        authority
            .append_batch(&[
                entry(0, 0, true, 0, 0),
                entry(1, 40, false, 0, 0),
                entry(2, 80, true, 2, 80),
                entry(3, 120, false, 2, 80),
            ])
            .unwrap();
        authority.mark_complete().unwrap();
        let before = authority.entry(FrameId(3)).unwrap().unwrap();

        let (mut progressive, _) =
            ProgressiveFrameIndex::open_or_create(&companion_path, &authority).unwrap();
        progressive
            .synchronize_authoritative_snapshot(&authority)
            .unwrap();

        assert_eq!(authority.frame_count().unwrap(), Some(4));
        assert_eq!(authority.entry(FrameId(3)).unwrap().unwrap(), before);
        let exact = progressive
            .layer_status(ProgressiveIndexLayer::AuthoritativeExact)
            .unwrap();
        assert_eq!(exact.lifecycle, ProgressiveLayerLifecycle::Complete);
        assert_eq!(exact.completed_units, 4);
        assert_eq!(exact.total_units, Some(4));
        let structural = progressive
            .layer_status(ProgressiveIndexLayer::StructuralNavigation)
            .unwrap();
        assert_eq!(structural.lifecycle, ProgressiveLayerLifecycle::Complete);
        assert_eq!(structural.completed_units, 2);
        assert_eq!(structural.covered_frame_prefix, Some(4));
        assert_eq!(progressive.structural_anchor_count().unwrap(), 2);
        let first = progressive.structural_anchor(0).unwrap().unwrap();
        assert_eq!(first.frame_id, Some(FrameId(0)));
        assert_eq!(first.gop_end_frame_exclusive, Some(FrameId(2)));
        assert_eq!(first.gop_end_timestamp.unwrap().ticks, 80);
        assert_eq!(first.packet_index, None);
        assert_eq!(first.byte_offset, None);
        assert_eq!(first.packet_pts, None);
        assert_eq!(first.packet_dts, None);
        let second = progressive.structural_anchor(1).unwrap().unwrap();
        assert_eq!(second.frame_id, Some(FrameId(2)));
        assert_eq!(second.gop_end_frame_exclusive, Some(FrameId(4)));
        let visual = progressive
            .layer_status(ProgressiveIndexLayer::VisualAcceleration)
            .unwrap();
        assert_eq!(visual.lifecycle, ProgressiveLayerLifecycle::NotStarted);

        drop(progressive);
        drop(authority);
        cleanup(&companion_path);
        cleanup(&authority_path);
    }

    #[test]
    fn partial_layer_status_remains_non_authoritative_until_exact_completion() {
        let authority_path = temp_path("authority-partial");
        let companion_path = temp_path("companion-partial");
        let (mut authority, _) =
            FrameIndex::open_or_create(&authority_path, source("partial"), stream()).unwrap();
        authority.mark_building().unwrap();
        authority
            .append_batch(&[entry(0, -20, true, 0, -20), entry(1, 5, false, 0, -20)])
            .unwrap();
        let (mut progressive, _) =
            ProgressiveFrameIndex::open_or_create(&companion_path, &authority).unwrap();
        progressive
            .synchronize_authoritative_snapshot(&authority)
            .unwrap();
        let exact = progressive
            .layer_status(ProgressiveIndexLayer::AuthoritativeExact)
            .unwrap();
        assert_eq!(exact.lifecycle, ProgressiveLayerLifecycle::Building);
        assert_eq!(exact.total_units, None);
        assert_eq!(exact.covered_frame_prefix, Some(2));
        assert!(ProgressiveIndexLayer::AuthoritativeExact.is_authoritative());
        assert!(!ProgressiveIndexLayer::StructuralNavigation.is_authoritative());

        authority
            .append_batch(&[entry(2, 5, true, 2, 5), entry(3, 90, false, 2, 5)])
            .unwrap();
        authority.mark_incomplete(Some("cancelled")).unwrap();
        progressive
            .synchronize_authoritative_snapshot(&authority)
            .unwrap();
        let structural = progressive
            .layer_status(ProgressiveIndexLayer::StructuralNavigation)
            .unwrap();
        assert_eq!(structural.lifecycle, ProgressiveLayerLifecycle::Incomplete);
        assert_eq!(structural.covered_frame_prefix, Some(4));
        assert_eq!(progressive.structural_anchor_count().unwrap(), 2);
        assert_eq!(
            progressive
                .structural_anchor(1)
                .unwrap()
                .unwrap()
                .presentation_timestamp
                .unwrap()
                .ticks,
            5
        );

        drop(progressive);
        drop(authority);
        cleanup(&companion_path);
        cleanup(&authority_path);
    }

    #[test]
    fn stale_authority_rebind_clears_only_companion_derived_metadata() {
        let authority_path = temp_path("authority-stale");
        let companion_path = temp_path("companion-stale");
        let (mut authority, _) =
            FrameIndex::open_or_create(&authority_path, source("old"), stream()).unwrap();
        authority.append_batch(&[entry(0, 0, true, 0, 0)]).unwrap();
        authority.mark_complete().unwrap();
        let (mut progressive, _) =
            ProgressiveFrameIndex::open_or_create(&companion_path, &authority).unwrap();
        progressive
            .synchronize_authoritative_snapshot(&authority)
            .unwrap();
        progressive
            .record_visual_artifact(&VisualArtifactMetadata {
                kind: VisualArtifactKind::Thumbnail,
                frame_id: Some(FrameId(0)),
                presentation_timestamp: Some(MediaTimestamp {
                    ticks: 0,
                    time_base: stream().time_base,
                }),
                tier: 0,
                generation: 1,
                storage_key: "thumbs/0.webp".into(),
                width: 160,
                height: 90,
                byte_size: 123,
            })
            .unwrap();
        drop(progressive);
        drop(authority);

        let (authority, disposition) =
            FrameIndex::open_or_create(&authority_path, source("new"), stream()).unwrap();
        assert_eq!(disposition, FrameIndexOpenDisposition::RebuiltStaleSource);
        let (progressive, disposition) =
            ProgressiveFrameIndex::open_or_create(&companion_path, &authority).unwrap();
        assert_eq!(
            disposition,
            ProgressiveIndexOpenDisposition::RebuiltStaleAuthority
        );
        assert_eq!(progressive.visual_artifact_count().unwrap(), 0);
        assert_eq!(authority.status().unwrap().indexed_frames, 0);

        drop(progressive);
        drop(authority);
        cleanup(&companion_path);
        cleanup(&authority_path);
    }

    #[test]
    fn unsupported_companion_schema_recreates_without_deleting_completed_v1_index() {
        let authority_path = temp_path("authority-schema");
        let companion_path = temp_path("companion-schema");
        let (mut authority, _) =
            FrameIndex::open_or_create(&authority_path, source("schema"), stream()).unwrap();
        authority.append_batch(&[entry(0, 0, true, 0, 0)]).unwrap();
        authority.mark_complete().unwrap();
        let (progressive, _) =
            ProgressiveFrameIndex::open_or_create(&companion_path, &authority).unwrap();
        drop(progressive);
        let connection = Connection::open(&companion_path).unwrap();
        connection.pragma_update(None, "user_version", 999_i64).unwrap();
        drop(connection);

        let (progressive, disposition) =
            ProgressiveFrameIndex::open_or_create(&companion_path, &authority).unwrap();
        assert_eq!(
            disposition,
            ProgressiveIndexOpenDisposition::RecreatedUnsupportedSchema
        );
        assert_eq!(authority.frame_count().unwrap(), Some(1));
        assert_eq!(
            progressive
                .layer_status(ProgressiveIndexLayer::AuthoritativeExact)
                .unwrap()
                .lifecycle,
            ProgressiveLayerLifecycle::NotStarted
        );

        drop(progressive);
        drop(authority);
        cleanup(&companion_path);
        cleanup(&authority_path);
    }

    #[test]
    fn visual_metadata_is_confined_and_similarity_progress_is_versioned() {
        let authority_path = temp_path("authority-visual");
        let companion_path = temp_path("companion-visual");
        let (authority, _) =
            FrameIndex::open_or_create(&authority_path, source("visual"), stream()).unwrap();
        let (mut progressive, _) =
            ProgressiveFrameIndex::open_or_create(&companion_path, &authority).unwrap();
        let valid = VisualArtifactMetadata {
            kind: VisualArtifactKind::ScrubPreview,
            frame_id: None,
            presentation_timestamp: Some(MediaTimestamp {
                ticks: -10,
                time_base: stream().time_base,
            }),
            tier: 2,
            generation: 3,
            storage_key: "preview/p2/-10.webp".into(),
            width: 320,
            height: 180,
            byte_size: 456,
        };
        progressive.record_visual_artifact(&valid).unwrap();
        assert_eq!(progressive.visual_artifact_count().unwrap(), 1);
        let mut invalid = valid.clone();
        invalid.storage_key = "../escape.webp".into();
        assert!(progressive.record_visual_artifact(&invalid).is_err());

        let similarity = SimilarityFingerprintStatus {
            lifecycle: ProgressiveLayerLifecycle::Building,
            generation: 7,
            covered_frames: 42,
            last_error: None,
        };
        progressive
            .update_similarity_fingerprint_status(&similarity)
            .unwrap();
        assert_eq!(
            progressive.similarity_fingerprint_status().unwrap(),
            similarity
        );

        drop(progressive);
        drop(authority);
        cleanup(&companion_path);
        cleanup(&authority_path);
    }
}