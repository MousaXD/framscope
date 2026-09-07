//! FrameScope video inspection and decoding.
//!
//! The original bounded ISO BMFF inspector remains available for lightweight Phase 1 metadata
//! compatibility. Phase 2 adds a streaming FFmpeg-backed decoder whose timing is presentation-
//! timestamp driven and safe for variable-frame-rate sources. Phase 3 layers a persistent metadata
//! index over that decoder without changing the timestamp contract.

use std::io::{Read, Seek, SeekFrom};

pub use cached_navigation::{
    CachedFrameSource, CachedNavigationError, CachedNavigationResult, RgbaNavigationDecoder,
    navigate_to_frame_cached,
};
pub use engine::{
    CancellationToken, ObservedFrameRateMode, OpenOptions, VideoDecoder, VideoStreamSelection,
};
pub use framescope_core::{
    CodecInfo, ContainerInfo, DecodedFrame, FrameScopeError, MediaDuration, MediaKind,
    MediaTimestamp, Rational, StreamInfo, TimeBase, VideoInfo, VideoMetadata,
};
pub use indexing::{
    FrameIndexDecoder, IndexingError, IndexingOptions, IndexingReport, build_or_resume_frame_index,
};
pub use microscope::{
    MicroscopeNavigationError, MicroscopeStep, MicroscopeTarget, MicroscopeTimestampSelection,
    microscope_step, microscope_target, microscope_timestamp_us,
};
pub use microscope_presentation::{
    MicroscopeFramePresentation, MicroscopePresentationError, present_microscope_frame,
};
pub use navigation::{
    NavigationDecoder, NavigationError, NavigationResult, TimestampSelection, navigate_to_frame,
    navigate_to_timestamp, resolve_timestamp,
};
pub use preview_navigation::{
    EncodedPreview, PreviewEncoder, PreviewNavigationError, PreviewNavigationResult, PreviewSource,
    navigate_to_frame_preview,
};

const MAX_STTS_ENTRIES: u32 = 1_000_000;

#[derive(Debug, Clone, Copy)]
struct BoxHeader {
    kind: [u8; 4],
    data_start: u64,
    end: u64,
}

#[derive(Debug, Default)]
struct MovieFacts {
    duration: Option<(u64, u32)>,
    video_track: Option<TrackFacts>,
}

#[derive(Debug, Default)]
struct TrackFacts {
    is_video: bool,
    width: Option<u32>,
    height: Option<u32>,
    rotation_degrees: i32,
    duration: Option<(u64, u32)>,
    sample_count: Option<u64>,
}

/// Inspect a seekable ISO BMFF source without reading the entire video into RAM.
///
/// This compatibility API parses container metadata only. Use [`VideoDecoder`] for real stream
/// discovery and frame decoding across the Phase 2 FFmpeg-supported containers/codecs.
pub fn inspect_video<R: Read + Seek>(reader: &mut R) -> Result<VideoMetadata, FrameScopeError> {
    let file_len = reader.seek(SeekFrom::End(0))?;
    if file_len < 8 {
        return Err(FrameScopeError::UnsupportedFormat(
            "file is too small to be an ISO BMFF video".into(),
        ));
    }

    let mut pos = 0;
    let mut saw_iso_box = false;
    let mut movie = None;

    while pos < file_len {
        let header = read_box_header(reader, pos, file_len)?;
        if matches!(
            &header.kind,
            b"ftyp" | b"moov" | b"mdat" | b"free" | b"wide"
        ) {
            saw_iso_box = true;
        }
        if &header.kind == b"moov" {
            movie = Some(parse_moov(reader, header.data_start, header.end)?);
            break;
        }
        pos = header.end;
    }

    if !saw_iso_box {
        return Err(FrameScopeError::UnsupportedFormat(
            "Phase 1 supports MP4/MOV (ISO BMFF) files containing a moov box".into(),
        ));
    }

    let movie = movie.ok_or_else(|| {
        FrameScopeError::UnsupportedFormat(
            "Phase 1 supports MP4/MOV (ISO BMFF) files containing a moov box".into(),
        )
    })?;
    let track = movie.video_track.ok_or(FrameScopeError::NoVideoTrack)?;
    let width = track.width.ok_or_else(|| {
        FrameScopeError::InvalidMetadata("video track does not declare a width".into())
    })?;
    let height = track.height.ok_or_else(|| {
        FrameScopeError::InvalidMetadata("video track does not declare a height".into())
    })?;

    let (duration_units, timescale) = track.duration.or(movie.duration).ok_or_else(|| {
        FrameScopeError::InvalidMetadata("video duration or timescale is missing".into())
    })?;
    if timescale == 0 || duration_units == 0 {
        return Err(FrameScopeError::InvalidMetadata(
            "video duration or timescale is zero".into(),
        ));
    }

    let duration_us = scale_duration_to_us(duration_units, timescale)?;
    let duration_seconds = duration_units as f64 / timescale as f64;
    let estimated_frame_rate = track
        .sample_count
        .filter(|count| *count > 0)
        .map(|count| count as f64 / duration_seconds)
        .filter(|fps| fps.is_finite() && *fps > 0.0 && *fps <= 1_000.0);

    let metadata = VideoMetadata {
        duration_us,
        width,
        height,
        estimated_frame_rate,
        rotation_degrees: track.rotation_degrees,
    };
    metadata.validate()?;
    Ok(metadata)
}

fn scale_duration_to_us(duration: u64, timescale: u32) -> Result<u64, FrameScopeError> {
    let micros = (duration as u128)
        .checked_mul(1_000_000)
        .ok_or_else(|| FrameScopeError::InvalidMetadata("duration overflow".into()))?
        / timescale as u128;
    u64::try_from(micros)
        .map_err(|_| FrameScopeError::InvalidMetadata("duration exceeds u64".into()))
}

fn parse_moov<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    end: u64,
) -> Result<MovieFacts, FrameScopeError> {
    let mut facts = MovieFacts::default();
    let mut pos = start;
    while pos < end {
        let header = read_box_header(reader, pos, end)?;
        match &header.kind {
            b"mvhd" => facts.duration = parse_mvhd(reader, header)?,
            b"trak" => {
                let track = parse_trak(reader, header.data_start, header.end)?;
                if track.is_video && facts.video_track.is_none() {
                    facts.video_track = Some(track);
                }
            }
            _ => {}
        }
        pos = header.end;
    }
    Ok(facts)
}

fn parse_trak<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    end: u64,
) -> Result<TrackFacts, FrameScopeError> {
    let mut facts = TrackFacts::default();
    let mut pos = start;
    while pos < end {
        let header = read_box_header(reader, pos, end)?;
        match &header.kind {
            b"tkhd" => {
                let (width, height, rotation) = parse_tkhd(reader, header)?;
                facts.width = Some(width);
                facts.height = Some(height);
                facts.rotation_degrees = rotation;
            }
            b"mdia" => parse_mdia(reader, header.data_start, header.end, &mut facts)?,
            _ => {}
        }
        pos = header.end;
    }
    Ok(facts)
}

fn parse_mdia<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    end: u64,
    facts: &mut TrackFacts,
) -> Result<(), FrameScopeError> {
    let mut pos = start;
    while pos < end {
        let header = read_box_header(reader, pos, end)?;
        match &header.kind {
            b"mdhd" => facts.duration = parse_mdhd(reader, header)?,
            b"hdlr" => facts.is_video = parse_hdlr_is_video(reader, header)?,
            b"minf" => {
                facts.sample_count = parse_minf_sample_count(reader, header.data_start, header.end)?
            }
            _ => {}
        }
        pos = header.end;
    }
    Ok(())
}

mod cached_navigation;
mod engine;
pub mod ffmpeg;
mod indexing;
mod iso;
mod microscope;
mod microscope_presentation;
mod navigation;
mod preview_navigation;

use iso::{
    parse_hdlr_is_video, parse_mdhd, parse_minf_sample_count, parse_mvhd, parse_tkhd,
    read_box_header,
};

#[cfg(test)]
use iso::{matrix_rotation, parse_stts_sample_count};

#[cfg(test)]
mod tests;
