use crate::{VideoDecoder, ffmpeg::DecodedRgbaFrame};
use framescope_cache::{
    CachedFrame, FrameCacheError, FrameCacheHierarchy, FrameCacheKey, FrameId, FrameIndex,
    FrameIndexEntry, FrameIndexError, FrameIndexLifecycle, FrameIndexStreamIdentity,
    KeyframeAnchor, OwnedRgbaFrame, RamInsertResult,
};
use framescope_core::{DecodedFrame, FrameScopeError, StreamInfo};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachedFrameSource {
    Ram,
    Decoded,
}

/// Full-quality indexed navigation result.
///
/// `index_entry` remains the authoritative persistent frame/timestamp identity. `pixels` always
/// contains owned full-resolution RGBA decoded from the source; a lossy disk proxy can never satisfy
/// this API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedNavigationResult {
    pub frame_id: FrameId,
    pub index_entry: FrameIndexEntry,
    pub pixels: OwnedRgbaFrame,
    pub source: CachedFrameSource,
    pub decoded_frames: u64,
    pub used_keyframe_seek: bool,
    pub fell_back_to_stream_start: bool,
    pub cache_insert_result: Option<RamInsertResult>,
}

#[derive(Debug, Error)]
pub enum CachedNavigationError {
    #[error("video navigation failed: {0}")]
    Decoder(#[from] FrameScopeError),
    #[error("frame-index lookup failed: {0}")]
    Index(#[from] FrameIndexError),
    #[error("frame-cache key/pixel validation failed: {0}")]
    Cache(#[from] FrameCacheError),
    #[error("indexed navigation requires a complete frame index")]
    IncompleteIndex,
    #[error("requested frame is outside the complete frame index")]
    FrameNotIndexed,
    #[error("fresh decoder stream does not match the stream bound to the frame index")]
    StreamIdentityMismatch,
    #[error("decoded presentation timeline does not reconcile with the persistent frame index")]
    TimelineMismatch,
    #[error("decoder reached EOF before the requested indexed frame")]
    UnexpectedEof,
}

pub trait RgbaNavigationDecoder {
    fn selected_stream_for_rgba_navigation(&self) -> &StreamInfo;
    fn next_rgba_for_navigation(&mut self) -> Result<Option<DecodedRgbaFrame>, FrameScopeError>;
    fn seek_for_rgba_navigation(&mut self, timestamp_us: i64) -> Result<(), FrameScopeError>;
}

impl RgbaNavigationDecoder for VideoDecoder {
    fn selected_stream_for_rgba_navigation(&self) -> &StreamInfo {
        self.selected_stream()
    }

    fn next_rgba_for_navigation(&mut self) -> Result<Option<DecodedRgbaFrame>, FrameScopeError> {
        self.next_frame_rgba()
    }

    fn seek_for_rgba_navigation(&mut self, timestamp_us: i64) -> Result<(), FrameScopeError> {
        self.seek_to_timestamp_us(timestamp_us)
    }
}

#[derive(Debug)]
struct RgbaNavigationResult {
    frame: DecodedRgbaFrame,
    decoded_frames: u64,
    used_keyframe_seek: bool,
    fell_back_to_stream_start: bool,
}

/// Navigate to an indexed frame using the full-quality RAM tier as a true hot cache.
///
/// A RAM hit performs no source decode. On a miss, navigation uses the persisted safe earlier
/// keyframe anchor, decodes forward while reconciling exact presentation metadata, then inserts the
/// owned RGBA payload into the byte-bounded RAM cache. The disk proxy tier is intentionally not read
/// here because this API promises source-quality pixels.
pub fn navigate_to_frame_cached<D, F>(
    index: &FrameIndex,
    cache: &mut FrameCacheHierarchy,
    mut open_fresh_decoder: F,
    frame_id: FrameId,
) -> Result<CachedNavigationResult, CachedNavigationError>
where
    D: RgbaNavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
{
    ensure_complete(index)?;
    let index_entry = index
        .entry(frame_id)?
        .ok_or(CachedNavigationError::FrameNotIndexed)?;
    let key = FrameCacheKey::new(
        index.source_identity(),
        index.stream_identity().stream_index,
        frame_id,
    )?;

    if let Some(cached) = cache.lookup_full(&key) {
        return Ok(CachedNavigationResult {
            frame_id,
            index_entry,
            pixels: cached.pixels,
            source: CachedFrameSource::Ram,
            decoded_frames: 0,
            used_keyframe_seek: false,
            fell_back_to_stream_start: false,
            cache_insert_result: None,
        });
    }

    let decoded = navigate_rgba_to_frame(index, &mut open_fresh_decoder, frame_id)?;
    let pixels = OwnedRgbaFrame::new(
        decoded.frame.frame.width,
        decoded.frame.frame.height,
        decoded.frame.stride_bytes,
        decoded.frame.pixels,
    )?;
    let insert_result = cache.insert_full(CachedFrame {
        key,
        pixels: pixels.clone(),
    });

    Ok(CachedNavigationResult {
        frame_id,
        index_entry,
        pixels,
        source: CachedFrameSource::Decoded,
        decoded_frames: decoded.decoded_frames,
        used_keyframe_seek: decoded.used_keyframe_seek,
        fell_back_to_stream_start: decoded.fell_back_to_stream_start,
        cache_insert_result: Some(insert_result),
    })
}

fn navigate_rgba_to_frame<D, F>(
    index: &FrameIndex,
    open_fresh_decoder: &mut F,
    frame_id: FrameId,
) -> Result<RgbaNavigationResult, CachedNavigationError>
where
    D: RgbaNavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
{
    ensure_complete(index)?;
    let target = index
        .entry(frame_id)?
        .ok_or(CachedNavigationError::FrameNotIndexed)?;
    let mut decoder = open_checked_decoder(index, open_fresh_decoder)?;

    if let KeyframeAnchor::Keyframe {
        frame_id: anchor_id,
        presentation_timestamp: Some(anchor_timestamp),
    } = target.anchor
    {
        if let Some(timestamp_us) = anchor_timestamp
            .to_microseconds()
            .filter(|value| *value >= 0)
        {
            decoder.seek_for_rgba_navigation(timestamp_us)?;
            match decode_from_seek(index, &mut decoder, anchor_id, frame_id) {
                Ok((frame, decoded_frames)) => {
                    return Ok(RgbaNavigationResult {
                        frame,
                        decoded_frames,
                        used_keyframe_seek: true,
                        fell_back_to_stream_start: false,
                    });
                }
                Err(
                    CachedNavigationError::TimelineMismatch | CachedNavigationError::UnexpectedEof,
                ) => {}
                Err(error) => return Err(error),
            }

            let mut fallback = open_checked_decoder(index, open_fresh_decoder)?;
            let (frame, decoded_frames) = decode_from_start(index, &mut fallback, frame_id)?;
            return Ok(RgbaNavigationResult {
                frame,
                decoded_frames,
                used_keyframe_seek: true,
                fell_back_to_stream_start: true,
            });
        }
    }

    let (frame, decoded_frames) = decode_from_start(index, &mut decoder, frame_id)?;
    Ok(RgbaNavigationResult {
        frame,
        decoded_frames,
        used_keyframe_seek: false,
        fell_back_to_stream_start: false,
    })
}

fn ensure_complete(index: &FrameIndex) -> Result<(), CachedNavigationError> {
    if index.status()?.lifecycle == FrameIndexLifecycle::Complete {
        Ok(())
    } else {
        Err(CachedNavigationError::IncompleteIndex)
    }
}

fn open_checked_decoder<D, F>(index: &FrameIndex, open: &mut F) -> Result<D, CachedNavigationError>
where
    D: RgbaNavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
{
    let decoder = open()?;
    let identity =
        FrameIndexStreamIdentity::from_stream(decoder.selected_stream_for_rgba_navigation())?;
    if &identity != index.stream_identity() {
        return Err(CachedNavigationError::StreamIdentityMismatch);
    }
    Ok(decoder)
}

fn decode_from_start<D: RgbaNavigationDecoder>(
    index: &FrameIndex,
    decoder: &mut D,
    target: FrameId,
) -> Result<(DecodedRgbaFrame, u64), CachedNavigationError> {
    let mut current = FrameId::ZERO;
    let mut decoded_frames = 0_u64;
    loop {
        let decoded = next_decoded(decoder, &mut decoded_frames)?;
        verify_decoded(index, current, &decoded.frame)?;
        if current == target {
            return Ok((decoded, decoded_frames));
        }
        current = next_frame_id(current)?;
    }
}

fn decode_from_seek<D: RgbaNavigationDecoder>(
    index: &FrameIndex,
    decoder: &mut D,
    anchor: FrameId,
    target: FrameId,
) -> Result<(DecodedRgbaFrame, u64), CachedNavigationError> {
    let anchor_entry = index
        .entry(anchor)?
        .ok_or(CachedNavigationError::TimelineMismatch)?;
    let mut decoded_frames = 0_u64;

    let mut decoded = loop {
        let candidate = next_decoded(decoder, &mut decoded_frames)?;
        if matches_index_entry(&candidate.frame, &anchor_entry) {
            break candidate;
        }
    };
    let mut current = anchor;

    loop {
        verify_decoded(index, current, &decoded.frame)?;
        if current == target {
            return Ok((decoded, decoded_frames));
        }
        current = next_frame_id(current)?;
        decoded = next_decoded(decoder, &mut decoded_frames)?;
    }
}

fn verify_decoded(
    index: &FrameIndex,
    frame_id: FrameId,
    decoded: &DecodedFrame,
) -> Result<(), CachedNavigationError> {
    let expected = index
        .entry(frame_id)?
        .ok_or(CachedNavigationError::TimelineMismatch)?;
    if matches_index_entry(decoded, &expected) {
        Ok(())
    } else {
        Err(CachedNavigationError::TimelineMismatch)
    }
}

fn next_decoded<D: RgbaNavigationDecoder>(
    decoder: &mut D,
    count: &mut u64,
) -> Result<DecodedRgbaFrame, CachedNavigationError> {
    let frame = decoder
        .next_rgba_for_navigation()?
        .ok_or(CachedNavigationError::UnexpectedEof)?;
    *count = count.saturating_add(1);
    Ok(frame)
}

fn next_frame_id(frame_id: FrameId) -> Result<FrameId, CachedNavigationError> {
    frame_id
        .0
        .checked_add(1)
        .map(FrameId)
        .ok_or(CachedNavigationError::TimelineMismatch)
}

fn matches_index_entry(decoded: &DecodedFrame, indexed: &FrameIndexEntry) -> bool {
    decoded.presentation_timestamp == indexed.presentation_timestamp
        && decoded.duration == indexed.duration
        && decoded.keyframe == indexed.keyframe
        && decoded.corrupt == indexed.corrupt
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{FrameIndexOpenDisposition, SourceIdentity};
    use framescope_core::{CodecInfo, MediaDuration, MediaKind, MediaTimestamp, TimeBase};
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, atomic::AtomicU64 as SharedAtomicU64};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    struct FakeRgbaDecoder {
        stream: StreamInfo,
        frames: VecDeque<DecodedRgbaFrame>,
        decoded_counter: Arc<SharedAtomicU64>,
    }

    impl RgbaNavigationDecoder for FakeRgbaDecoder {
        fn selected_stream_for_rgba_navigation(&self) -> &StreamInfo {
            &self.stream
        }

        fn next_rgba_for_navigation(
            &mut self,
        ) -> Result<Option<DecodedRgbaFrame>, FrameScopeError> {
            let frame = self.frames.pop_front();
            if frame.is_some() {
                self.decoded_counter.fetch_add(1, Ordering::Relaxed);
            }
            Ok(frame)
        }

        fn seek_for_rgba_navigation(&mut self, _timestamp_us: i64) -> Result<(), FrameScopeError> {
            while self
                .frames
                .front()
                .is_some_and(|frame| frame.frame.index < 2)
            {
                self.frames.pop_front();
            }
            Ok(())
        }
    }

    fn stream() -> StreamInfo {
        StreamInfo {
            index: 0,
            media_kind: MediaKind::Video,
            codec: CodecInfo {
                id: 27,
                name: "h264".into(),
                decoder_available: true,
            },
            is_default: true,
            time_base: TimeBase::new(1, 1_000),
            duration: None,
            frame_count: None,
            width: Some(2),
            height: Some(2),
            pixel_format: Some("yuv420p".into()),
            average_frame_rate: None,
            nominal_frame_rate: None,
            rotation_degrees: None,
        }
    }

    fn timestamp(ticks: i64) -> MediaTimestamp {
        MediaTimestamp {
            ticks,
            time_base: TimeBase::new(1, 1_000).unwrap(),
        }
    }

    fn rgba_frame(id: u64, ticks: i64, keyframe: bool) -> DecodedRgbaFrame {
        DecodedRgbaFrame {
            frame: DecodedFrame {
                source_id: 1,
                stream_index: 0,
                decode_epoch: 0,
                index: id,
                presentation_timestamp: Some(timestamp(ticks)),
                duration: Some(MediaDuration {
                    ticks: 40,
                    time_base: TimeBase::new(1, 1_000).unwrap(),
                }),
                keyframe,
                corrupt: false,
                width: 2,
                height: 2,
                pixel_format: Some("yuv420p".into()),
            },
            stride_bytes: 8,
            pixels: vec![id as u8; 16],
        }
    }

    fn temp_path(kind: &str) -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "framescope-cached-navigation-{kind}-{}-{id}",
            std::process::id()
        ))
    }

    fn complete_index() -> (PathBuf, FrameIndex) {
        let path = temp_path("index.sqlite");
        let identity = FrameIndexStreamIdentity::from_stream(&stream()).unwrap();
        let source = SourceIdentity::new(100, None, Some("cached-navigation".into()));
        let (mut index, disposition) = FrameIndex::open_or_create(&path, source, identity).unwrap();
        assert_eq!(disposition, FrameIndexOpenDisposition::Created);
        index.mark_building().unwrap();
        let ticks = [0, 40, 100, 140, 220];
        let mut anchor_id = 0;
        let mut anchor_ticks = 0;
        let entries = ticks
            .iter()
            .enumerate()
            .map(|(id, ticks)| {
                let keyframe = id == 0 || id == 2;
                if keyframe {
                    anchor_id = id as u64;
                    anchor_ticks = *ticks;
                }
                FrameIndexEntry {
                    frame_id: FrameId(id as u64),
                    presentation_timestamp: Some(timestamp(*ticks)),
                    duration: Some(MediaDuration {
                        ticks: 40,
                        time_base: TimeBase::new(1, 1_000).unwrap(),
                    }),
                    keyframe,
                    corrupt: false,
                    anchor: KeyframeAnchor::Keyframe {
                        frame_id: FrameId(anchor_id),
                        presentation_timestamp: Some(timestamp(anchor_ticks)),
                    },
                }
            })
            .collect::<Vec<_>>();
        index.append_batch(&entries).unwrap();
        index.mark_complete().unwrap();
        (path, index)
    }

    fn fake_decoder(counter: Arc<SharedAtomicU64>) -> FakeRgbaDecoder {
        FakeRgbaDecoder {
            stream: stream(),
            frames: VecDeque::from(vec![
                rgba_frame(0, 0, true),
                rgba_frame(1, 40, false),
                rgba_frame(2, 100, true),
                rgba_frame(3, 140, false),
                rgba_frame(4, 220, false),
            ]),
            decoded_counter: counter,
        }
    }

    #[test]
    fn second_source_quality_access_hits_ram_without_decoder_work() {
        let (index_path, index) = complete_index();
        let cache_root = temp_path("cache");
        let mut cache = FrameCacheHierarchy::open(&cache_root, 1024, 1024).unwrap();
        let counter = Arc::new(SharedAtomicU64::new(0));

        let first = navigate_to_frame_cached(
            &index,
            &mut cache,
            || Ok(fake_decoder(counter.clone())),
            FrameId(4),
        )
        .unwrap();
        assert_eq!(first.source, CachedFrameSource::Decoded);
        assert!(first.decoded_frames < 5);
        let decoded_after_first = counter.load(Ordering::Relaxed);
        assert!(decoded_after_first > 0);

        let second = navigate_to_frame_cached(
            &index,
            &mut cache,
            || Ok(fake_decoder(counter.clone())),
            FrameId(4),
        )
        .unwrap();
        assert_eq!(second.source, CachedFrameSource::Ram);
        assert_eq!(second.decoded_frames, 0);
        assert_eq!(counter.load(Ordering::Relaxed), decoded_after_first);
        assert_eq!(first.pixels.pixels(), second.pixels.pixels());

        drop(index);
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_dir_all(cache_root);
    }

    #[test]
    fn lossy_disk_proxy_never_satisfies_source_quality_navigation() {
        let (index_path, index) = complete_index();
        let cache_root = temp_path("proxy-cache");
        let mut cache = FrameCacheHierarchy::open(&cache_root, 1024, 1024).unwrap();
        let key = FrameCacheKey::new(index.source_identity(), 0, FrameId(4)).unwrap();
        cache
            .insert_proxy(
                &key,
                framescope_cache::ProxyFormat::Jpeg,
                &[0xff, 0xd8, 0xff, 0xd9],
            )
            .unwrap();
        let disk_hits_before = cache.stats().disk.hits;
        let counter = Arc::new(SharedAtomicU64::new(0));

        let result = navigate_to_frame_cached(
            &index,
            &mut cache,
            || Ok(fake_decoder(counter.clone())),
            FrameId(4),
        )
        .unwrap();
        assert_eq!(result.source, CachedFrameSource::Decoded);
        assert!(counter.load(Ordering::Relaxed) > 0);
        assert_eq!(cache.stats().disk.hits, disk_hits_before);

        drop(index);
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_dir_all(cache_root);
    }
}
