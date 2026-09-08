//! Append-only JSON Lines manifest for bounded Phase 6 extraction.
//!
//! The manifest intentionally writes one record at a time. It never owns a video-wide list of
//! frames, output paths, or encoded byte buffers. A terminal record distinguishes a complete export
//! from a cancelled or failed partial export.

use framescope_cache::{FrameId, FrameIndexEntry};
use framescope_core::{MediaDuration, MediaTimestamp};
use framescope_extraction::ExtractionProgress;
use framescope_extraction_image::ExtractionImageFormat;
use serde::Serialize;
use std::io::{self, Write};
use thiserror::Error;

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ManifestSelection {
    CurrentFrame {
        frame_id: u64,
    },
    FrameRangeInclusive {
        start_frame: u64,
        end_frame: u64,
        every_n: u64,
    },
    TimeRangeMicroseconds {
        start_us: i64,
        end_us: i64,
        every_n: u64,
    },
    AllFrames {
        every_n: u64,
    },
    UniqueGroupRepresentatives,
}

impl ManifestSelection {
    pub fn validate(&self) -> Result<(), ManifestError> {
        match *self {
            Self::CurrentFrame { .. } | Self::UniqueGroupRepresentatives => Ok(()),
            Self::FrameRangeInclusive {
                start_frame,
                end_frame,
                every_n,
            } => {
                if start_frame > end_frame {
                    return Err(ManifestError::InvalidRecord(
                        "frame-range start exceeds end".into(),
                    ));
                }
                if every_n == 0 {
                    return Err(ManifestError::InvalidRecord(
                        "frame sampling interval must be positive".into(),
                    ));
                }
                Ok(())
            }
            Self::TimeRangeMicroseconds {
                start_us,
                end_us,
                every_n,
            } => {
                if start_us > end_us {
                    return Err(ManifestError::InvalidRecord(
                        "time-range start exceeds end".into(),
                    ));
                }
                if every_n == 0 {
                    return Err(ManifestError::InvalidRecord(
                        "time sampling interval must be positive".into(),
                    ));
                }
                Ok(())
            }
            Self::AllFrames { every_n } => {
                if every_n == 0 {
                    return Err(ManifestError::InvalidRecord(
                        "frame sampling interval must be positive".into(),
                    ));
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestImageFormat {
    pub name: &'static str,
    pub mime_type: &'static str,
    pub extension: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jpeg_quality: Option<u8>,
}

impl From<ExtractionImageFormat> for ManifestImageFormat {
    fn from(value: ExtractionImageFormat) -> Self {
        let (name, jpeg_quality) = match value {
            ExtractionImageFormat::Png => ("png", None),
            ExtractionImageFormat::Jpeg { quality } => ("jpeg", Some(quality)),
            ExtractionImageFormat::WebPLossless => ("webp_lossless", None),
        };
        Self {
            name,
            mime_type: value.mime_type(),
            extension: value.extension(),
            jpeg_quality,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestHeader {
    pub schema_version: u32,
    pub selection: ManifestSelection,
    pub image_format: ManifestImageFormat,
    pub expected_frames: u64,
}

impl ManifestHeader {
    pub fn new(
        selection: ManifestSelection,
        image_format: ExtractionImageFormat,
        expected_frames: u64,
    ) -> Result<Self, ManifestError> {
        selection.validate()?;
        if expected_frames == 0 {
            return Err(ManifestError::InvalidRecord(
                "manifest expected frame count must be positive".into(),
            ));
        }
        Ok(Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            selection,
            image_format: image_format.into(),
            expected_frames,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestTimestamp {
    pub ticks: i64,
    pub time_base_numerator: i32,
    pub time_base_denominator: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub microseconds: Option<i64>,
}

impl From<MediaTimestamp> for ManifestTimestamp {
    fn from(value: MediaTimestamp) -> Self {
        Self {
            ticks: value.ticks,
            time_base_numerator: value.time_base.numerator,
            time_base_denominator: value.time_base.denominator,
            microseconds: value.to_microseconds(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestDuration {
    pub ticks: i64,
    pub time_base_numerator: i32,
    pub time_base_denominator: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub microseconds: Option<i64>,
}

impl From<MediaDuration> for ManifestDuration {
    fn from(value: MediaDuration) -> Self {
        Self {
            ticks: value.ticks,
            time_base_numerator: value.time_base.numerator,
            time_base_denominator: value.time_base.denominator,
            microseconds: value.to_microseconds(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestGroup {
    pub ordinal: u64,
    pub group_count: u64,
    pub representative_frame: u64,
    pub first_frame: u64,
    pub last_frame: u64,
    pub represented_frame_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFrame<'a> {
    pub progress: ExtractionProgress,
    pub index_entry: &'a FrameIndexEntry,
    pub file_name: &'a str,
    pub encoded_bytes: u64,
    pub group: Option<ManifestGroup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ManifestFrameLine<'a> {
    record_type: &'static str,
    ordinal: u64,
    total: u64,
    frame_id: u64,
    file_name: &'a str,
    encoded_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    presentation_timestamp: Option<ManifestTimestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration: Option<ManifestDuration>,
    keyframe: bool,
    corrupt: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<ManifestGroup>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestTerminalStatus {
    Complete,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ManifestTerminalLine<'a> {
    record_type: &'static str,
    status: ManifestTerminalStatus,
    emitted_frames: u64,
    expected_frames: u64,
    encoded_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("invalid extraction manifest record: {0}")]
    InvalidRecord(String),
    #[error("extraction manifest already has a terminal record")]
    AlreadyFinished,
    #[error("extraction manifest I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("extraction manifest JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct ExtractionManifestWriter<W: Write> {
    writer: W,
    expected_frames: u64,
    emitted_frames: u64,
    encoded_bytes: u64,
    finished: bool,
}

impl<W: Write> ExtractionManifestWriter<W> {
    pub fn new(writer: W, header: ManifestHeader) -> Result<Self, ManifestError> {
        header.selection.validate()?;
        if header.schema_version != MANIFEST_SCHEMA_VERSION || header.expected_frames == 0 {
            return Err(ManifestError::InvalidRecord(
                "manifest header schema or expected count is invalid".into(),
            ));
        }
        let mut manifest = Self {
            writer,
            expected_frames: header.expected_frames,
            emitted_frames: 0,
            encoded_bytes: 0,
            finished: false,
        };
        manifest.write_json_line(&HeaderLine {
            record_type: "header",
            header: &header,
        })?;
        Ok(manifest)
    }

    pub fn emitted_frames(&self) -> u64 {
        self.emitted_frames
    }

    pub fn expected_frames(&self) -> u64 {
        self.expected_frames
    }

    pub fn encoded_bytes(&self) -> u64 {
        self.encoded_bytes
    }

    pub fn write_frame(&mut self, frame: ManifestFrame<'_>) -> Result<(), ManifestError> {
        self.ensure_open()?;
        let expected_ordinal = self
            .emitted_frames
            .checked_add(1)
            .ok_or_else(|| ManifestError::InvalidRecord("manifest ordinal overflow".into()))?;
        if frame.progress.ordinal != expected_ordinal {
            return Err(ManifestError::InvalidRecord(format!(
                "frame ordinal {} is not expected ordinal {expected_ordinal}",
                frame.progress.ordinal
            )));
        }
        if frame.progress.total != self.expected_frames {
            return Err(ManifestError::InvalidRecord(format!(
                "frame total {} does not match manifest total {}",
                frame.progress.total, self.expected_frames
            )));
        }
        if frame.progress.frame_id != frame.index_entry.frame_id {
            return Err(ManifestError::InvalidRecord(
                "progress FrameId does not match authoritative index entry".into(),
            ));
        }
        if frame.file_name.trim().is_empty() || frame.file_name.contains(['/', '\\']) {
            return Err(ManifestError::InvalidRecord(
                "frame output filename must be a non-empty leaf name".into(),
            ));
        }
        if frame.encoded_bytes == 0 {
            return Err(ManifestError::InvalidRecord(
                "encoded frame byte count must be positive".into(),
            ));
        }
        if let Some(group) = frame.group.as_ref() {
            validate_group(frame.progress.frame_id, group)?;
        }

        let next_bytes = self
            .encoded_bytes
            .checked_add(frame.encoded_bytes)
            .ok_or_else(|| ManifestError::InvalidRecord("encoded byte total overflow".into()))?;
        let line = ManifestFrameLine {
            record_type: "frame",
            ordinal: frame.progress.ordinal,
            total: frame.progress.total,
            frame_id: frame.progress.frame_id.0,
            file_name: frame.file_name,
            encoded_bytes: frame.encoded_bytes,
            presentation_timestamp: frame.index_entry.presentation_timestamp.map(Into::into),
            duration: frame.index_entry.duration.map(Into::into),
            keyframe: frame.index_entry.keyframe,
            corrupt: frame.index_entry.corrupt,
            group: frame.group,
        };
        self.write_json_line(&line)?;
        self.emitted_frames = expected_ordinal;
        self.encoded_bytes = next_bytes;
        Ok(())
    }

    pub fn finish_complete(&mut self) -> Result<(), ManifestError> {
        if self.emitted_frames != self.expected_frames {
            return Err(ManifestError::InvalidRecord(format!(
                "complete manifest emitted {} of {} expected frames",
                self.emitted_frames, self.expected_frames
            )));
        }
        self.finish(ManifestTerminalStatus::Complete, None, None)
    }

    pub fn finish_cancelled(&mut self, message: Option<&str>) -> Result<(), ManifestError> {
        self.finish(
            ManifestTerminalStatus::Cancelled,
            Some("cancelled"),
            message,
        )
    }

    pub fn finish_failed(
        &mut self,
        code: &str,
        message: Option<&str>,
    ) -> Result<(), ManifestError> {
        if code.trim().is_empty() {
            return Err(ManifestError::InvalidRecord(
                "failed manifest terminal code must not be empty".into(),
            ));
        }
        self.finish(ManifestTerminalStatus::Failed, Some(code), message)
    }

    pub fn flush(&mut self) -> Result<(), ManifestError> {
        self.writer.flush().map_err(ManifestError::Io)
    }

    pub fn into_inner(self) -> W {
        self.writer
    }

    fn finish(
        &mut self,
        status: ManifestTerminalStatus,
        code: Option<&str>,
        message: Option<&str>,
    ) -> Result<(), ManifestError> {
        self.ensure_open()?;
        self.write_json_line(&ManifestTerminalLine {
            record_type: "terminal",
            status,
            emitted_frames: self.emitted_frames,
            expected_frames: self.expected_frames,
            encoded_bytes: self.encoded_bytes,
            code,
            message,
        })?;
        self.writer.flush()?;
        self.finished = true;
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), ManifestError> {
        if self.finished {
            Err(ManifestError::AlreadyFinished)
        } else {
            Ok(())
        }
    }

    fn write_json_line<T: Serialize>(&mut self, value: &T) -> Result<(), ManifestError> {
        serde_json::to_writer(&mut self.writer, value)?;
        self.writer.write_all(b"\n")?;
        Ok(())
    }
}

#[derive(Serialize)]
struct HeaderLine<'a> {
    record_type: &'static str,
    #[serde(flatten)]
    header: &'a ManifestHeader,
}

fn validate_group(frame_id: FrameId, group: &ManifestGroup) -> Result<(), ManifestError> {
    if group.group_count == 0 || group.ordinal >= group.group_count {
        return Err(ManifestError::InvalidRecord(
            "group ordinal/count is invalid".into(),
        ));
    }
    if group.first_frame > group.representative_frame
        || group.representative_frame > group.last_frame
        || group.representative_frame != frame_id.0
    {
        return Err(ManifestError::InvalidRecord(
            "group representative/bounds do not match exported FrameId".into(),
        ));
    }
    let expected_count = group
        .last_frame
        .checked_sub(group.first_frame)
        .and_then(|distance| distance.checked_add(1))
        .ok_or_else(|| ManifestError::InvalidRecord("group frame count overflow".into()))?;
    if group.represented_frame_count != expected_count {
        return Err(ManifestError::InvalidRecord(
            "group represented frame count does not match bounds".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::KeyframeAnchor;
    use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};

    fn entry(frame_id: u64) -> FrameIndexEntry {
        let time_base = TimeBase::new(1, 1_000).unwrap();
        let timestamp = MediaTimestamp {
            ticks: i64::try_from(frame_id).unwrap() * 40,
            time_base,
        };
        FrameIndexEntry {
            frame_id: FrameId(frame_id),
            presentation_timestamp: Some(timestamp),
            duration: Some(MediaDuration {
                ticks: 40,
                time_base,
            }),
            keyframe: frame_id == 0,
            corrupt: false,
            anchor: KeyframeAnchor::Keyframe {
                frame_id: FrameId::ZERO,
                presentation_timestamp: Some(MediaTimestamp {
                    ticks: 0,
                    time_base,
                }),
            },
        }
    }

    #[test]
    fn streams_header_frames_and_complete_terminal_as_json_lines() {
        let header = ManifestHeader::new(
            ManifestSelection::AllFrames { every_n: 2 },
            ExtractionImageFormat::Png,
            3,
        )
        .unwrap();
        let mut manifest = ExtractionManifestWriter::new(Vec::new(), header).unwrap();
        for (ordinal, frame_id) in [0_u64, 2, 4].into_iter().enumerate() {
            let entry = entry(frame_id);
            manifest
                .write_frame(ManifestFrame {
                    progress: ExtractionProgress {
                        frame_id: FrameId(frame_id),
                        ordinal: ordinal as u64 + 1,
                        total: 3,
                    },
                    index_entry: &entry,
                    file_name: &format!("frame_{frame_id:020}.png"),
                    encoded_bytes: 100 + frame_id,
                    group: None,
                })
                .unwrap();
        }
        manifest.finish_complete().unwrap();
        let bytes = manifest.into_inner();
        let text = String::from_utf8(bytes).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 5);

        let header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(header["record_type"], "header");
        assert_eq!(header["schema_version"], MANIFEST_SCHEMA_VERSION);
        assert_eq!(header["expected_frames"], 3);
        assert_eq!(header["selection"]["kind"], "all_frames");

        let frame: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(frame["record_type"], "frame");
        assert_eq!(frame["frame_id"], 2);
        assert_eq!(frame["ordinal"], 2);
        assert_eq!(frame["presentation_timestamp"]["microseconds"], 40_000);

        let terminal: serde_json::Value = serde_json::from_str(lines[4]).unwrap();
        assert_eq!(terminal["record_type"], "terminal");
        assert_eq!(terminal["status"], "complete");
        assert_eq!(terminal["emitted_frames"], 3);
        assert_eq!(terminal["encoded_bytes"], 306);
    }

    #[test]
    fn cancelled_manifest_is_explicitly_partial() {
        let header = ManifestHeader::new(
            ManifestSelection::FrameRangeInclusive {
                start_frame: 10,
                end_frame: 20,
                every_n: 1,
            },
            ExtractionImageFormat::Jpeg { quality: 90 },
            11,
        )
        .unwrap();
        let mut manifest = ExtractionManifestWriter::new(Vec::new(), header).unwrap();
        let entry = entry(10);
        manifest
            .write_frame(ManifestFrame {
                progress: ExtractionProgress {
                    frame_id: FrameId(10),
                    ordinal: 1,
                    total: 11,
                },
                index_entry: &entry,
                file_name: "frame_00000000000000000010.jpg",
                encoded_bytes: 77,
                group: None,
            })
            .unwrap();
        manifest
            .finish_cancelled(Some("user cancelled export"))
            .unwrap();

        let text = String::from_utf8(manifest.into_inner()).unwrap();
        let terminal: serde_json::Value =
            serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(terminal["status"], "cancelled");
        assert_eq!(terminal["emitted_frames"], 1);
        assert_eq!(terminal["expected_frames"], 11);
        assert_eq!(terminal["code"], "cancelled");
    }

    #[test]
    fn rejects_out_of_order_progress_without_writing_frame_record() {
        let header = ManifestHeader::new(
            ManifestSelection::AllFrames { every_n: 1 },
            ExtractionImageFormat::Png,
            2,
        )
        .unwrap();
        let mut manifest = ExtractionManifestWriter::new(Vec::new(), header).unwrap();
        let entry = entry(0);
        let error = manifest
            .write_frame(ManifestFrame {
                progress: ExtractionProgress {
                    frame_id: FrameId::ZERO,
                    ordinal: 2,
                    total: 2,
                },
                index_entry: &entry,
                file_name: "frame_00000000000000000000.png",
                encoded_bytes: 10,
                group: None,
            })
            .unwrap_err();
        assert!(matches!(error, ManifestError::InvalidRecord(_)));
        assert_eq!(manifest.emitted_frames(), 0);
        let text = String::from_utf8(manifest.into_inner()).unwrap();
        assert_eq!(text.lines().count(), 1);
    }

    #[test]
    fn unique_group_record_requires_representative_identity_and_exact_bounds() {
        let header = ManifestHeader::new(
            ManifestSelection::UniqueGroupRepresentatives,
            ExtractionImageFormat::WebPLossless,
            1,
        )
        .unwrap();
        let mut manifest = ExtractionManifestWriter::new(Vec::new(), header).unwrap();
        let entry = entry(4);
        manifest
            .write_frame(ManifestFrame {
                progress: ExtractionProgress {
                    frame_id: FrameId(4),
                    ordinal: 1,
                    total: 1,
                },
                index_entry: &entry,
                file_name: "frame_00000000000000000004.webp",
                encoded_bytes: 44,
                group: Some(ManifestGroup {
                    ordinal: 0,
                    group_count: 1,
                    representative_frame: 4,
                    first_frame: 3,
                    last_frame: 5,
                    represented_frame_count: 3,
                }),
            })
            .unwrap();
        manifest.finish_complete().unwrap();
    }

    #[test]
    fn complete_terminal_requires_every_expected_frame() {
        let header = ManifestHeader::new(
            ManifestSelection::AllFrames { every_n: 1 },
            ExtractionImageFormat::Png,
            2,
        )
        .unwrap();
        let mut manifest = ExtractionManifestWriter::new(Vec::new(), header).unwrap();
        let error = manifest.finish_complete().unwrap_err();
        assert!(matches!(error, ManifestError::InvalidRecord(_)));
    }
}
