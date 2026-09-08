use super::model::{
    FRAME_INDEX_SCHEMA_VERSION, FRAME_TIMELINE_CONTRACT_GENERATION, FrameId, FrameIndexEntry,
    FrameIndexError, FrameIndexLifecycle, FrameIndexOpenDisposition, FrameIndexStatus,
    FrameIndexStreamIdentity, KeyframeAnchor, TimestampSeekSafety,
};
use crate::SourceIdentity;
use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const META_ROW_ID: i64 = 1;
const ENTRY_COLUMNS: &str = "frame_index,
     timestamp_ticks, time_base_num, time_base_den,
     duration_ticks, duration_time_base_num, duration_time_base_den,
     keyframe, corrupt,
     anchor_kind, anchor_frame_index, anchor_timestamp_ticks,
     anchor_time_base_num, anchor_time_base_den";

#[derive(Debug)]
struct MetaRow {
    lifecycle: FrameIndexLifecycle,
    source_identity: SourceIdentity,
    source_binding_stable_key: String,
    timeline_contract_generation: u32,
    timestamp_seek_safety: TimestampSeekSafety,
    stream_identity: FrameIndexStreamIdentity,
    indexed_frames: u64,
    frame_count: Option<u64>,
    last_error: Option<String>,
}

pub struct FrameIndex {
    connection: Connection,
    path: PathBuf,
    source_identity: SourceIdentity,
    stream_identity: FrameIndexStreamIdentity,
}

impl FrameIndex {
    pub fn open_or_create(
        path: impl AsRef<Path>,
        source_identity: SourceIdentity,
        stream_identity: FrameIndexStreamIdentity,
    ) -> Result<(Self, FrameIndexOpenDisposition), FrameIndexError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let existed = path.exists();
        match Self::try_open(
            path.clone(),
            source_identity.clone(),
            stream_identity.clone(),
        ) {
            Ok((index, disposition)) => Ok((
                index,
                if existed {
                    disposition
                } else {
                    FrameIndexOpenDisposition::Created
                },
            )),
            Err(error) => {
                let Some(disposition) = error.recreation_disposition() else {
                    return Err(error);
                };
                purge_database_files(&path)?;
                let (index, _) = Self::try_open(path, source_identity, stream_identity)?;
                Ok((index, disposition))
            }
        }
    }

    fn try_open(
        path: PathBuf,
        source_identity: SourceIdentity,
        stream_identity: FrameIndexStreamIdentity,
    ) -> Result<(Self, FrameIndexOpenDisposition), FrameIndexError> {
        let connection = Connection::open(&path)?;
        configure_connection(&connection)?;
        migrate(&connection)?;
        let mut index = Self {
            connection,
            path,
            source_identity: source_identity.clone(),
            stream_identity: stream_identity.clone(),
        };
        let disposition = match index.load_meta()? {
            None => {
                index.initialize_meta()?;
                FrameIndexOpenDisposition::Created
            }
            Some(_) if !source_identity.is_reuse_safe() => {
                index.reset_and_rebind()?;
                FrameIndexOpenDisposition::RebuiltUnverifiableSource
            }
            Some(meta)
                if meta.source_identity != source_identity
                    || meta.stream_identity != stream_identity =>
            {
                index.reset_and_rebind()?;
                FrameIndexOpenDisposition::RebuiltStaleSource
            }
            Some(meta)
                if meta.timeline_contract_generation != FRAME_TIMELINE_CONTRACT_GENERATION =>
            {
                index.reset_and_rebind()?;
                FrameIndexOpenDisposition::RebuiltIncompatibleTimelineContract
            }
            Some(meta) => {
                if meta.source_binding_stable_key != source_identity.stable_key() {
                    return Err(FrameIndexError::InvalidState(
                        "stored source binding key does not match stored source identity".into(),
                    ));
                }
                index.validate_persistent_state(&meta)?;
                FrameIndexOpenDisposition::Reused
            }
        };
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

    /// Persisted authority decision for timestamp-keyframe seeking over the currently indexed prefix.
    ///
    /// Future bounded resume code must consume this instead of re-inventing timestamp heuristics.
    pub fn timestamp_seek_safety(&self) -> Result<TimestampSeekSafety, FrameIndexError> {
        self.load_meta()?
            .map(|meta| meta.timestamp_seek_safety)
            .ok_or_else(|| {
                FrameIndexError::InvalidState("frame-index metadata row is missing".into())
            })
    }

    pub fn status(&self) -> Result<FrameIndexStatus, FrameIndexError> {
        let meta = self.load_meta()?.ok_or_else(|| {
            FrameIndexError::InvalidState("frame-index metadata row is missing".into())
        })?;
        let last = if meta.indexed_frames == 0 {
            None
        } else {
            self.entry(FrameId(meta.indexed_frames - 1))?
        };
        Ok(FrameIndexStatus {
            lifecycle: meta.lifecycle,
            indexed_frames: meta.indexed_frames,
            frame_count: meta.frame_count,
            last_frame_id: last.as_ref().map(|entry| entry.frame_id),
            last_presentation_timestamp: last.and_then(|entry| entry.presentation_timestamp),
            last_error: meta.last_error,
        })
    }

    pub fn frame_count(&self) -> Result<Option<u64>, FrameIndexError> {
        let status = self.status()?;
        Ok((status.lifecycle == FrameIndexLifecycle::Complete)
            .then_some(status.frame_count)
            .flatten())
    }

    pub fn mark_building(&mut self) -> Result<(), FrameIndexError> {
        self.update_lifecycle(FrameIndexLifecycle::Building, None, None)
    }

    pub fn mark_incomplete(&mut self, reason: Option<&str>) -> Result<(), FrameIndexError> {
        self.update_lifecycle(FrameIndexLifecycle::Incomplete, None, reason)
    }

    pub fn mark_failed_recoverable(&mut self, reason: &str) -> Result<(), FrameIndexError> {
        self.update_lifecycle(FrameIndexLifecycle::FailedRecoverable, None, Some(reason))
    }

    pub fn mark_complete(&mut self) -> Result<(), FrameIndexError> {
        let count = self.status()?.indexed_frames;
        self.update_lifecycle(FrameIndexLifecycle::Complete, Some(count), None)
    }

    pub fn clear_for_rebuild(&mut self) -> Result<(), FrameIndexError> {
        let source_key =
            source_binding_key(&self.source_identity, TimestampSeekSafety::Unambiguous);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("DELETE FROM frame_index", [])?;
        transaction.execute(
            "UPDATE index_meta
             SET lifecycle = ?1, source_key = ?2, indexed_frames = 0,
                 frame_count = NULL, last_error = NULL
             WHERE id = ?3",
            params![
                FrameIndexLifecycle::Incomplete.as_i64(),
                source_key,
                META_ROW_ID
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn truncate_from(&mut self, frame_id: FrameId) -> Result<(), FrameIndexError> {
        let count = to_sql_u64(frame_id.0, "frame count")?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "DELETE FROM frame_index WHERE frame_index >= ?1",
            params![count],
        )?;
        let seek_safety = compute_timestamp_seek_safety(&transaction)?;
        let source_key = source_binding_key(&self.source_identity, seek_safety);
        transaction.execute(
            "UPDATE index_meta
             SET lifecycle = ?1, source_key = ?2, indexed_frames = ?3,
                 frame_count = NULL, last_error = NULL
             WHERE id = ?4",
            params![
                FrameIndexLifecycle::Incomplete.as_i64(),
                source_key,
                count,
                META_ROW_ID
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn append_batch(&mut self, entries: &[FrameIndexEntry]) -> Result<(), FrameIndexError> {
        if entries.is_empty() {
            return Ok(());
        }
        let current = self.status()?.indexed_frames;
        for (offset, entry) in entries.iter().enumerate() {
            let expected = current
                .checked_add(offset as u64)
                .ok_or_else(|| FrameIndexError::InvalidState("frame id overflow".into()))?;
            if entry.frame_id.0 != expected {
                return Err(FrameIndexError::InvalidState(format!(
                    "batch is not contiguous: expected frame {expected}, got {}",
                    entry.frame_id.0
                )));
            }
            entry.validate(&self.stream_identity)?;
        }

        let mut seek_safety = self.timestamp_seek_safety()?;
        let mut previous_clean_keyframe_ticks = if seek_safety.permits_timestamp_seek() {
            self.connection
                .query_row(
                    "SELECT timestamp_ticks FROM frame_index
                     WHERE keyframe = 1 AND corrupt = 0
                     ORDER BY frame_index DESC LIMIT 1",
                    [],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .optional()?
                .flatten()
        } else {
            None
        };
        if seek_safety.permits_timestamp_seek() {
            for entry in entries {
                if !entry.keyframe || entry.corrupt {
                    continue;
                }
                let Some(timestamp) = entry.presentation_timestamp else {
                    seek_safety = TimestampSeekSafety::Ambiguous;
                    break;
                };
                if previous_clean_keyframe_ticks.is_some_and(|previous| timestamp.ticks <= previous)
                {
                    seek_safety = TimestampSeekSafety::Ambiguous;
                    break;
                }
                previous_clean_keyframe_ticks = Some(timestamp.ticks);
            }
        }

        let source_key = source_binding_key(&self.source_identity, seek_safety);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut statement = transaction.prepare_cached(
                "INSERT INTO frame_index (
                    frame_index,
                    timestamp_ticks, time_base_num, time_base_den, timestamp_us,
                    duration_ticks, duration_time_base_num, duration_time_base_den,
                    keyframe, corrupt,
                    anchor_kind, anchor_frame_index, anchor_timestamp_ticks,
                    anchor_time_base_num, anchor_time_base_den, anchor_timestamp_us
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                    ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
                 )",
            )?;
            for entry in entries {
                let timestamp = entry.presentation_timestamp;
                let duration = entry.duration;
                let (kind, anchor_id, anchor_ticks, anchor_num, anchor_den, anchor_us) =
                    encode_anchor(&entry.anchor)?;
                statement.execute(params![
                    to_sql_u64(entry.frame_id.0, "frame id")?,
                    timestamp.map(|value| value.ticks),
                    timestamp.map(|value| value.time_base.numerator),
                    timestamp.map(|value| value.time_base.denominator),
                    entry.timestamp_us(),
                    duration.map(|value| value.ticks),
                    duration.map(|value| value.time_base.numerator),
                    duration.map(|value| value.time_base.denominator),
                    if entry.keyframe { 1_i64 } else { 0_i64 },
                    if entry.corrupt { 1_i64 } else { 0_i64 },
                    kind,
                    anchor_id,
                    anchor_ticks,
                    anchor_num,
                    anchor_den,
                    anchor_us,
                ])?;
            }
        }
        let next = current
            .checked_add(entries.len() as u64)
            .ok_or_else(|| FrameIndexError::InvalidState("frame count overflow".into()))?;
        transaction.execute(
            "UPDATE index_meta SET source_key = ?1, indexed_frames = ?2 WHERE id = ?3",
            params![source_key, to_sql_u64(next, "frame count")?, META_ROW_ID],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn entry(&self, frame_id: FrameId) -> Result<Option<FrameIndexEntry>, FrameIndexError> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {ENTRY_COLUMNS} FROM frame_index WHERE frame_index = ?1"
        ))?;
        let entry = statement
            .query_row(params![to_sql_u64(frame_id.0, "frame id")?], decode_entry)
            .optional()?;
        self.validate_loaded_entry(entry)
    }

    pub fn frame_at_or_before(
        &self,
        timestamp: MediaTimestamp,
    ) -> Result<Option<FrameIndexEntry>, FrameIndexError> {
        self.ensure_time_base(timestamp.time_base)?;
        self.timestamp_query(timestamp.ticks, true)
    }

    pub fn frame_at_or_after(
        &self,
        timestamp: MediaTimestamp,
    ) -> Result<Option<FrameIndexEntry>, FrameIndexError> {
        self.ensure_time_base(timestamp.time_base)?;
        self.timestamp_query(timestamp.ticks, false)
    }

    pub fn frame_at_or_before_us(
        &self,
        timestamp_us: i64,
    ) -> Result<Option<FrameIndexEntry>, FrameIndexError> {
        self.timestamp_us_query(timestamp_us, true)
    }

    pub fn frame_at_or_after_us(
        &self,
        timestamp_us: i64,
    ) -> Result<Option<FrameIndexEntry>, FrameIndexError> {
        self.timestamp_us_query(timestamp_us, false)
    }

    pub fn nearest_keyframe_anchor(
        &self,
        frame_id: FrameId,
    ) -> Result<Option<KeyframeAnchor>, FrameIndexError> {
        Ok(self.entry(frame_id)?.map(|entry| entry.anchor))
    }

    pub fn visit_range(
        &self,
        start: FrameId,
        end_exclusive: FrameId,
        mut visitor: impl FnMut(FrameIndexEntry) -> Result<(), FrameIndexError>,
    ) -> Result<(), FrameIndexError> {
        if end_exclusive.0 < start.0 {
            return Err(FrameIndexError::InvalidState(
                "frame-index range end precedes start".into(),
            ));
        }
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {ENTRY_COLUMNS} FROM frame_index
             WHERE frame_index >= ?1 AND frame_index < ?2 ORDER BY frame_index ASC"
        ))?;
        let mut rows = statement.query(params![
            to_sql_u64(start.0, "frame id")?,
            to_sql_u64(end_exclusive.0, "frame id")?
        ])?;
        while let Some(row) = rows.next()? {
            let entry = decode_entry(row)?;
            entry.validate(&self.stream_identity)?;
            visitor(entry)?;
        }
        Ok(())
    }

    fn timestamp_query(
        &self,
        ticks: i64,
        before: bool,
    ) -> Result<Option<FrameIndexEntry>, FrameIndexError> {
        let comparison = if before { "<=" } else { ">=" };
        let ordering = if before { "DESC" } else { "ASC" };
        let sql = format!(
            "SELECT {ENTRY_COLUMNS} FROM frame_index
             WHERE timestamp_ticks IS NOT NULL AND timestamp_ticks {comparison} ?1
             ORDER BY timestamp_ticks {ordering}, frame_index {ordering} LIMIT 1"
        );
        let mut statement = self.connection.prepare_cached(&sql)?;
        let entry = statement
            .query_row(params![ticks], decode_entry)
            .optional()?;
        self.validate_loaded_entry(entry)
    }

    fn timestamp_us_query(
        &self,
        timestamp_us: i64,
        before: bool,
    ) -> Result<Option<FrameIndexEntry>, FrameIndexError> {
        let comparison = if before { "<=" } else { ">=" };
        let ordering = if before { "DESC" } else { "ASC" };
        let sql = format!(
            "SELECT {ENTRY_COLUMNS} FROM frame_index
             WHERE timestamp_us IS NOT NULL AND timestamp_us {comparison} ?1
             ORDER BY timestamp_us {ordering}, frame_index {ordering} LIMIT 1"
        );
        let mut statement = self.connection.prepare_cached(&sql)?;
        let entry = statement
            .query_row(params![timestamp_us], decode_entry)
            .optional()?;
        self.validate_loaded_entry(entry)
    }

    fn ensure_time_base(&self, time_base: TimeBase) -> Result<(), FrameIndexError> {
        if time_base == self.stream_identity.time_base {
            Ok(())
        } else {
            Err(FrameIndexError::TimeBaseMismatch)
        }
    }

    fn validate_loaded_entry(
        &self,
        entry: Option<FrameIndexEntry>,
    ) -> Result<Option<FrameIndexEntry>, FrameIndexError> {
        if let Some(entry) = &entry {
            entry.validate(&self.stream_identity)?;
        }
        Ok(entry)
    }

    fn initialize_meta(&mut self) -> Result<(), FrameIndexError> {
        self.connection.execute(
            "INSERT INTO index_meta (
                id, lifecycle, source_key, source_identity_json, stream_identity_json,
                indexed_frames, frame_count, last_error
             ) VALUES (?1, ?2, ?3, ?4, ?5, 0, NULL, NULL)",
            params![
                META_ROW_ID,
                FrameIndexLifecycle::Incomplete.as_i64(),
                source_binding_key(&self.source_identity, TimestampSeekSafety::Unambiguous),
                serde_json::to_string(&self.source_identity)?,
                serde_json::to_string(&self.stream_identity)?,
            ],
        )?;
        Ok(())
    }

    fn reset_and_rebind(&mut self) -> Result<(), FrameIndexError> {
        let source_json = serde_json::to_string(&self.source_identity)?;
        let stream_json = serde_json::to_string(&self.stream_identity)?;
        let source_key =
            source_binding_key(&self.source_identity, TimestampSeekSafety::Unambiguous);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("DELETE FROM frame_index", [])?;
        transaction.execute("DELETE FROM index_meta", [])?;
        transaction.execute(
            "INSERT INTO index_meta (
                id, lifecycle, source_key, source_identity_json, stream_identity_json,
                indexed_frames, frame_count, last_error
             ) VALUES (?1, ?2, ?3, ?4, ?5, 0, NULL, NULL)",
            params![
                META_ROW_ID,
                FrameIndexLifecycle::Incomplete.as_i64(),
                source_key,
                source_json,
                stream_json,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn update_lifecycle(
        &mut self,
        lifecycle: FrameIndexLifecycle,
        frame_count: Option<u64>,
        last_error: Option<&str>,
    ) -> Result<(), FrameIndexError> {
        let frame_count = frame_count
            .map(|value| to_sql_u64(value, "frame count"))
            .transpose()?;
        self.connection.execute(
            "UPDATE index_meta
             SET lifecycle = ?1, frame_count = ?2, last_error = ?3
             WHERE id = ?4",
            params![lifecycle.as_i64(), frame_count, last_error, META_ROW_ID],
        )?;
        Ok(())
    }

    fn load_meta(&self) -> Result<Option<MetaRow>, FrameIndexError> {
        let row = self
            .connection
            .query_row(
                "SELECT lifecycle, source_key, source_identity_json, stream_identity_json,
                        indexed_frames, frame_count, last_error
                 FROM index_meta WHERE id = ?1",
                params![META_ROW_ID],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((lifecycle, source_key, source_json, stream_json, indexed, count, last_error)) =
            row
        else {
            return Ok(None);
        };
        let source_identity: SourceIdentity = serde_json::from_str(&source_json)?;
        let (timeline_contract_generation, timestamp_seek_safety, source_binding_stable_key) =
            decode_source_binding_key(&source_key)?;
        Ok(Some(MetaRow {
            lifecycle: FrameIndexLifecycle::from_i64(lifecycle)?,
            source_identity,
            source_binding_stable_key,
            timeline_contract_generation,
            timestamp_seek_safety,
            stream_identity: serde_json::from_str(&stream_json)?,
            indexed_frames: from_sql_u64(indexed, "indexed frame count")?,
            frame_count: count
                .map(|value| from_sql_u64(value, "complete frame count"))
                .transpose()?,
            last_error,
        }))
    }

    fn validate_persistent_state(&self, meta: &MetaRow) -> Result<(), FrameIndexError> {
        if meta.lifecycle == FrameIndexLifecycle::Complete
            && meta.frame_count != Some(meta.indexed_frames)
        {
            return Err(FrameIndexError::InvalidState(
                "complete index frame count does not match indexed frame count".into(),
            ));
        }
        if meta.lifecycle != FrameIndexLifecycle::Complete && meta.frame_count.is_some() {
            return Err(FrameIndexError::InvalidState(
                "partial index unexpectedly carries a complete frame count".into(),
            ));
        }
        let (count, min_id, max_id): (i64, Option<i64>, Option<i64>) = self.connection.query_row(
            "SELECT COUNT(*), MIN(frame_index), MAX(frame_index) FROM frame_index",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let count = from_sql_u64(count, "stored row count")?;
        if count != meta.indexed_frames {
            return Err(FrameIndexError::InvalidState(format!(
                "metadata says {} indexed frames but database contains {count}",
                meta.indexed_frames
            )));
        }
        if count > 0
            && (min_id != Some(0) || max_id != Some(to_sql_u64(count - 1, "maximum frame id")?))
        {
            return Err(FrameIndexError::InvalidState(
                "stored frame ids are not a contiguous zero-based sequence".into(),
            ));
        }
        let invalid_rows: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM frame_index
             WHERE keyframe NOT IN (0, 1)
                OR corrupt NOT IN (0, 1)
                OR anchor_kind NOT IN (0, 1)
                OR (timestamp_ticks IS NULL) != (time_base_num IS NULL)
                OR (timestamp_ticks IS NULL) != (time_base_den IS NULL)
                OR (timestamp_ticks IS NULL) != (timestamp_us IS NULL)
                OR (timestamp_ticks IS NOT NULL
                    AND (time_base_num != ?1 OR time_base_den != ?2))
                OR (duration_ticks IS NULL) != (duration_time_base_num IS NULL)
                OR (duration_ticks IS NULL) != (duration_time_base_den IS NULL)
                OR (duration_ticks IS NOT NULL
                    AND (duration_ticks <= 0
                        OR duration_time_base_num != ?1
                        OR duration_time_base_den != ?2))
                OR (anchor_timestamp_ticks IS NULL) != (anchor_time_base_num IS NULL)
                OR (anchor_timestamp_ticks IS NULL) != (anchor_time_base_den IS NULL)
                OR (anchor_timestamp_ticks IS NULL) != (anchor_timestamp_us IS NULL)
                OR (anchor_timestamp_ticks IS NOT NULL
                    AND (anchor_time_base_num != ?1 OR anchor_time_base_den != ?2))
                OR (anchor_kind = 0
                    AND (anchor_frame_index IS NOT NULL OR anchor_timestamp_ticks IS NOT NULL))
                OR (anchor_kind = 1
                    AND (anchor_frame_index IS NULL
                        OR anchor_frame_index < 0 OR anchor_frame_index > frame_index))
                OR (keyframe = 1 AND corrupt = 0
                    AND (anchor_kind != 1 OR anchor_frame_index != frame_index))",
            params![
                self.stream_identity.time_base.numerator,
                self.stream_identity.time_base.denominator
            ],
            |row| row.get(0),
        )?;
        if invalid_rows != 0 {
            return Err(FrameIndexError::InvalidState(format!(
                "frame index contains {invalid_rows} impossible row(s)"
            )));
        }
        let invalid_anchors: i64 = self.connection.query_row(
            "SELECT COUNT(*)
             FROM frame_index AS target
             LEFT JOIN frame_index AS anchor
               ON target.anchor_kind = 1
              AND anchor.frame_index = target.anchor_frame_index
             WHERE target.anchor_kind = 1
               AND (anchor.frame_index IS NULL
                    OR anchor.keyframe != 1
                    OR anchor.corrupt != 0
                    OR target.anchor_timestamp_ticks IS NOT anchor.timestamp_ticks
                    OR target.anchor_time_base_num IS NOT anchor.time_base_num
                    OR target.anchor_time_base_den IS NOT anchor.time_base_den
                    OR target.anchor_timestamp_us IS NOT anchor.timestamp_us)",
            [],
            |row| row.get(0),
        )?;
        if invalid_anchors != 0 {
            return Err(FrameIndexError::InvalidState(format!(
                "frame index contains {invalid_anchors} invalid keyframe anchor(s)"
            )));
        }
        let actual_seek_safety = compute_timestamp_seek_safety(&self.connection)?;
        if actual_seek_safety != meta.timestamp_seek_safety {
            return Err(FrameIndexError::InvalidState(
                "stored timestamp-seek safety does not match indexed keyframe timeline".into(),
            ));
        }
        Ok(())
    }
}

fn source_binding_key(source: &SourceIdentity, seek_safety: TimestampSeekSafety) -> String {
    format!(
        "t{}:s{}:{}",
        FRAME_TIMELINE_CONTRACT_GENERATION,
        seek_safety.as_i64(),
        source.stable_key()
    )
}

fn decode_source_binding_key(
    value: &str,
) -> Result<(u32, TimestampSeekSafety, String), FrameIndexError> {
    if !value.starts_with('t') {
        // Pre-contract indexes stored only the source stable key. Treat them as generation 1 so the
        // normal compatibility branch rebuilds them rather than silently accepting old semantics.
        return Ok((1, TimestampSeekSafety::Ambiguous, value.to_owned()));
    }
    let mut parts = value.splitn(3, ':');
    let generation = parts
        .next()
        .and_then(|part| part.strip_prefix('t'))
        .ok_or_else(|| FrameIndexError::InvalidState("invalid source binding generation".into()))?
        .parse::<u32>()
        .map_err(|_| FrameIndexError::InvalidState("invalid source binding generation".into()))?;
    let safety = parts
        .next()
        .and_then(|part| part.strip_prefix('s'))
        .ok_or_else(|| FrameIndexError::InvalidState("invalid source binding seek safety".into()))?
        .parse::<i64>()
        .map_err(|_| FrameIndexError::InvalidState("invalid source binding seek safety".into()))?;
    let stable_key = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| FrameIndexError::InvalidState("source binding key is missing".into()))?;
    Ok((
        generation,
        TimestampSeekSafety::from_i64(safety)?,
        stable_key.to_owned(),
    ))
}

fn compute_timestamp_seek_safety(
    connection: &Connection,
) -> Result<TimestampSeekSafety, FrameIndexError> {
    let mut statement = connection.prepare(
        "SELECT timestamp_ticks FROM frame_index
         WHERE keyframe = 1 AND corrupt = 0
         ORDER BY frame_index ASC",
    )?;
    let mut rows = statement.query([])?;
    let mut previous = None;
    while let Some(row) = rows.next()? {
        let Some(ticks) = row.get::<_, Option<i64>>(0)? else {
            return Ok(TimestampSeekSafety::Ambiguous);
        };
        if previous.is_some_and(|previous| ticks <= previous) {
            return Ok(TimestampSeekSafety::Ambiguous);
        }
        previous = Some(ticks);
    }
    Ok(TimestampSeekSafety::Unambiguous)
}

fn decode_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<FrameIndexEntry> {
    let frame_id = FrameId(sql_i64_to_u64(row.get(0)?, 0)?);
    let presentation_timestamp = decode_timestamp(row.get(1)?, row.get(2)?, row.get(3)?, 1)?;
    let duration = match (
        row.get::<_, Option<i64>>(4)?,
        row.get::<_, Option<i32>>(5)?,
        row.get::<_, Option<i32>>(6)?,
    ) {
        (None, None, None) => None,
        (Some(ticks), Some(num), Some(den)) => {
            let time_base = TimeBase::new(num, den)
                .ok_or(rusqlite::Error::IntegralValueOutOfRange(5, i64::from(num)))?;
            Some(MediaDuration { ticks, time_base })
        }
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let anchor_kind: i64 = row.get(9)?;
    let anchor = match anchor_kind {
        0 => KeyframeAnchor::StreamStart,
        1 => KeyframeAnchor::Keyframe {
            frame_id: FrameId(sql_i64_to_u64(
                row.get::<_, Option<i64>>(10)?
                    .ok_or(rusqlite::Error::InvalidQuery)?,
                10,
            )?),
            presentation_timestamp: decode_timestamp(row.get(11)?, row.get(12)?, row.get(13)?, 11)?,
        },
        _ => return Err(rusqlite::Error::IntegralValueOutOfRange(9, anchor_kind)),
    };
    Ok(FrameIndexEntry {
        frame_id,
        presentation_timestamp,
        duration,
        keyframe: sql_bool(row.get(7)?, 7)?,
        corrupt: sql_bool(row.get(8)?, 8)?,
        anchor,
    })
}

fn decode_timestamp(
    ticks: Option<i64>,
    numerator: Option<i32>,
    denominator: Option<i32>,
    column: usize,
) -> rusqlite::Result<Option<MediaTimestamp>> {
    match (ticks, numerator, denominator) {
        (None, None, None) => Ok(None),
        (Some(ticks), Some(numerator), Some(denominator)) => {
            let time_base = TimeBase::new(numerator, denominator).ok_or(
                rusqlite::Error::IntegralValueOutOfRange(column, i64::from(numerator)),
            )?;
            Ok(Some(MediaTimestamp { ticks, time_base }))
        }
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

#[allow(clippy::type_complexity)]
fn encode_anchor(
    anchor: &KeyframeAnchor,
) -> Result<
    (
        i64,
        Option<i64>,
        Option<i64>,
        Option<i32>,
        Option<i32>,
        Option<i64>,
    ),
    FrameIndexError,
> {
    match anchor {
        KeyframeAnchor::StreamStart => Ok((0, None, None, None, None, None)),
        KeyframeAnchor::Keyframe {
            frame_id,
            presentation_timestamp,
        } => Ok((
            1,
            Some(to_sql_u64(frame_id.0, "anchor frame id")?),
            presentation_timestamp.map(|value| value.ticks),
            presentation_timestamp.map(|value| value.time_base.numerator),
            presentation_timestamp.map(|value| value.time_base.denominator),
            presentation_timestamp.and_then(MediaTimestamp::to_microseconds),
        )),
    }
}

fn configure_connection(connection: &Connection) -> Result<(), FrameIndexError> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

fn migrate(connection: &Connection) -> Result<(), FrameIndexError> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match version {
        FRAME_INDEX_SCHEMA_VERSION => Ok(()),
        0 => {
            connection.execute_batch(
                "CREATE TABLE index_meta (
                    id INTEGER PRIMARY KEY CHECK(id = 1),
                    lifecycle INTEGER NOT NULL CHECK(lifecycle BETWEEN 1 AND 4),
                    source_key TEXT NOT NULL,
                    source_identity_json TEXT NOT NULL,
                    stream_identity_json TEXT NOT NULL,
                    indexed_frames INTEGER NOT NULL CHECK(indexed_frames >= 0),
                    frame_count INTEGER CHECK(frame_count >= 0),
                    last_error TEXT
                 );
                 CREATE TABLE frame_index (
                    frame_index INTEGER PRIMARY KEY CHECK(frame_index >= 0),
                    timestamp_ticks INTEGER,
                    time_base_num INTEGER,
                    time_base_den INTEGER,
                    timestamp_us INTEGER,
                    duration_ticks INTEGER,
                    duration_time_base_num INTEGER,
                    duration_time_base_den INTEGER,
                    keyframe INTEGER NOT NULL CHECK(keyframe IN (0, 1)),
                    corrupt INTEGER NOT NULL CHECK(corrupt IN (0, 1)),
                    anchor_kind INTEGER NOT NULL CHECK(anchor_kind IN (0, 1)),
                    anchor_frame_index INTEGER,
                    anchor_timestamp_ticks INTEGER,
                    anchor_time_base_num INTEGER,
                    anchor_time_base_den INTEGER,
                    anchor_timestamp_us INTEGER
                 );
                 CREATE INDEX frame_index_timestamp_ticks
                    ON frame_index(timestamp_ticks, frame_index)
                    WHERE timestamp_ticks IS NOT NULL;
                 CREATE INDEX frame_index_timestamp_us
                    ON frame_index(timestamp_us, frame_index)
                    WHERE timestamp_us IS NOT NULL;",
            )?;
            connection.pragma_update(None, "user_version", FRAME_INDEX_SCHEMA_VERSION)?;
            Ok(())
        }
        found => Err(FrameIndexError::UnsupportedSchema { found }),
    }
}

pub(super) fn purge_database_files(path: &Path) -> Result<(), FrameIndexError> {
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

fn to_sql_u64(value: u64, label: &str) -> Result<i64, FrameIndexError> {
    i64::try_from(value)
        .map_err(|_| FrameIndexError::InvalidState(format!("{label} exceeds SQLite i64 range")))
}

fn from_sql_u64(value: i64, label: &str) -> Result<u64, FrameIndexError> {
    u64::try_from(value).map_err(|_| FrameIndexError::InvalidState(format!("{label} is negative")))
}

fn sql_i64_to_u64(value: i64, column: usize) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, value))
}

fn sql_bool(value: i64, column: usize) -> rusqlite::Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(rusqlite::Error::IntegralValueOutOfRange(column, value)),
    }
}
