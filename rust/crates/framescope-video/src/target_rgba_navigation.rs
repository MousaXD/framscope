use crate::{
    CachedFrameSource, CachedNavigationError, CachedNavigationResult, VideoDecoder,
    ffmpeg::DecodedRgbaFrame,
};
use framescope_cache::{
    CachedFrame, FrameCacheError, FrameCacheHierarchy, FrameCacheKey, FrameId, FrameIndex,
    FrameIndexEntry, FrameIndexLifecycle, FrameIndexStreamIdentity, KeyframeAnchor, OwnedRgbaFrame,
};
use framescope_core::{DecodedFrame, FrameScopeError, StreamInfo};

/// Decoder contract for indexed navigation that keeps intermediate frames metadata-only.
///
/// `snapshot_current_rgba_for_target_navigation` must reject metadata that is not the decoder's
/// exact current presentation frame. [`VideoDecoder`] enforces that invariant directly.
pub trait TargetRgbaNavigationDecoder {
    fn selected_stream_for_target_navigation(&self) -> &StreamInfo;
    fn next_metadata_for_target_navigation(
        &mut self,
    ) -> Result<Option<DecodedFrame>, FrameScopeError>;
    fn snapshot_current_rgba_for_target_navigation(
        &mut self,
        frame: &DecodedFrame,
    ) -> Result<DecodedRgbaFrame, FrameScopeError>;
    fn seek_for_target_navigation(&mut self, timestamp_us: i64) -> Result<(), FrameScopeError>;
}

impl TargetRgbaNavigationDecoder for VideoDecoder {
    fn selected_stream_for_target_navigation(&self) -> &StreamInfo {
        self.selected_stream()
    }

    fn next_metadata_for_target_navigation(
        &mut self,
    ) -> Result<Option<DecodedFrame>, FrameScopeError> {
        self.next_frame()
    }

    fn snapshot_current_rgba_for_target_navigation(
        &mut self,
        frame: &DecodedFrame,
    ) -> Result<DecodedRgbaFrame, FrameScopeError> {
        self.snapshot_current_frame_rgba(frame)
    }

    fn seek_for_target_navigation(&mut self, timestamp_us: i64) -> Result<(), FrameScopeError> {
        self.seek_to_timestamp_us(timestamp_us)
    }
}

#[derive(Debug)]
struct TargetNavigationResult {
    frame: DecodedRgbaFrame,
    decoded_frames: u64,
    used_keyframe_seek: bool,
    fell_back_to_stream_start: bool,
}

/// Source-quality indexed navigation optimized for preview misses.
///
/// All decoded presentation metadata is reconciled against the persistent frame index exactly as in
/// authoritative cached navigation. The difference is allocation policy: intermediate frames remain
/// metadata-only and full-resolution RGBA is copied only after the requested target is verified.
/// Safe cache identity, keyframe anchors, stream identity checks, and stream-start fallback remain
/// unchanged.
pub fn navigate_to_frame_cached_target_only<D, F>(
    index: &FrameIndex,
    cache: &mut FrameCacheHierarchy,
    mut open_fresh_decoder: F,
    frame_id: FrameId,
) -> Result<CachedNavigationResult, CachedNavigationError>
where
    D: TargetRgbaNavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
{
    ensure_complete(index)?;
    let index_entry = index
        .entry(frame_id)?
        .ok_or(CachedNavigationError::FrameNotIndexed)?;
    let key = match FrameCacheKey::new(
        index.source_identity(),
        index.stream_identity().stream_index,
        frame_id,
    ) {
        Ok(key) => Some(key),
        Err(FrameCacheError::UnsafeSourceIdentity) => None,
        Err(error) => return Err(error.into()),
    };

    if let Some(key) = key.as_ref() {
        if let Some(cached) = cache.lookup_full(key) {
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
    }

    let decoded = navigate_target_rgba(index, &mut open_fresh_decoder, frame_id)?;
    let pixels = OwnedRgbaFrame::new(
        decoded.frame.frame.width,
        decoded.frame.frame.height,
        decoded.frame.stride_bytes,
        decoded.frame.pixels,
    )?;
    let cache_insert_result = key.map(|key| {
        cache.insert_full(CachedFrame {
            key,
            pixels: pixels.clone(),
        })
    });

    Ok(CachedNavigationResult {
        frame_id,
        index_entry,
        pixels,
        source: CachedFrameSource::Decoded,
        decoded_frames: decoded.decoded_frames,
        used_keyframe_seek: decoded.used_keyframe_seek,
        fell_back_to_stream_start: decoded.fell_back_to_stream_start,
        cache_insert_result,
    })
}

fn navigate_target_rgba<D, F>(
    index: &FrameIndex,
    open_fresh_decoder: &mut F,
    frame_id: FrameId,
) -> Result<TargetNavigationResult, CachedNavigationError>
where
    D: TargetRgbaNavigationDecoder,
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
            decoder.seek_for_target_navigation(timestamp_us)?;
            match decode_from_seek(index, &mut decoder, anchor_id, frame_id) {
                Ok((frame, decoded_frames)) => {
                    return Ok(TargetNavigationResult {
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
            return Ok(TargetNavigationResult {
                frame,
                decoded_frames,
                used_keyframe_seek: true,
                fell_back_to_stream_start: true,
            });
        }
    }

    let (frame, decoded_frames) = decode_from_start(index, &mut decoder, frame_id)?;
    Ok(TargetNavigationResult {
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
    D: TargetRgbaNavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
{
    let decoder = open()?;
    let identity =
        FrameIndexStreamIdentity::from_stream(decoder.selected_stream_for_target_navigation())?;
    if &identity != index.stream_identity() {
        return Err(CachedNavigationError::StreamIdentityMismatch);
    }
    Ok(decoder)
}

fn decode_from_start<D: TargetRgbaNavigationDecoder>(
    index: &FrameIndex,
    decoder: &mut D,
    target: FrameId,
) -> Result<(DecodedRgbaFrame, u64), CachedNavigationError> {
    let mut current = FrameId::ZERO;
    let mut decoded_frames = 0_u64;
    loop {
        let decoded = next_metadata(decoder, &mut decoded_frames)?;
        verify_decoded(index, current, &decoded)?;
        if current == target {
            let rgba = decoder.snapshot_current_rgba_for_target_navigation(&decoded)?;
            return Ok((rgba, decoded_frames));
        }
        current = next_frame_id(current)?;
    }
}

fn decode_from_seek<D: TargetRgbaNavigationDecoder>(
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
        let candidate = next_metadata(decoder, &mut decoded_frames)?;
        if matches_index_entry(&candidate, &anchor_entry) {
            break candidate;
        }
    };
    let mut current = anchor;

    loop {
        verify_decoded(index, current, &decoded)?;
        if current == target {
            let rgba = decoder.snapshot_current_rgba_for_target_navigation(&decoded)?;
            return Ok((rgba, decoded_frames));
        }
        current = next_frame_id(current)?;
        decoded = next_metadata(decoder, &mut decoded_frames)?;
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

fn next_metadata<D: TargetRgbaNavigationDecoder>(
    decoder: &mut D,
    count: &mut u64,
) -> Result<DecodedFrame, CachedNavigationError> {
    let frame = decoder
        .next_metadata_for_target_navigation()?
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    struct FakeTargetDecoder {
        stream: StreamInfo,
        frames: VecDeque<DecodedRgbaFrame>,
        current: Option<DecodedRgbaFrame>,
        decoded_counter: Arc<AtomicU64>,
        materialized_counter: Arc<AtomicU64>,
    }

    impl TargetRgbaNavigationDecoder for FakeTargetDecoder {
        fn selected_stream_for_target_navigation(&self) -> &StreamInfo {
            &self.stream
        }

        fn next_metadata_for_target_navigation(
            &mut self,
        ) -> Result<Option<DecodedFrame>, FrameScopeError> {
            let frame = self.frames.pop_front();
            self.current = frame;
            let Some(current) = self.current.as_ref() else {
                return Ok(None);
            };
            self.decoded_counter.fetch_add(1, Ordering::Relaxed);
            Ok(Some(current.frame.clone()))
        }

        fn snapshot_current_rgba_for_target_navigation(
            &mut self,
            frame: &DecodedFrame,
        ) -> Result<DecodedRgbaFrame, FrameScopeError> {
            let current = self.current.as_ref().ok_or_else(|| {
                FrameScopeError::DecoderFailure("fake decoder has no current frame".into())
            })?;
            if &current.frame != frame {
                return Err(FrameScopeError::DecoderFailure(
                    "fake decoder current frame mismatch".into(),
                ));
            }
            self.materialized_counter.fetch_add(1, Ordering::Relaxed);
            Ok(current.clone())
        }

        fn seek_for_target_navigation(&mut self, _timestamp_us: i64) -> Result<(), FrameScopeError> {
            self.current = None;
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
            "framescope-target-rgba-navigation-{kind}-{}-{id}",
            std::process::id()
        ))
    }

    fn complete_index() -> (PathBuf, FrameIndex) {
        let path = temp_path("index.sqlite");
        let identity = FrameIndexStreamIdentity::from_stream(&stream()).unwrap();
        let source = SourceIdentity::new(100, None, Some("target-rgba-navigation".into()));
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

    fn fake_decoder(
        decoded_counter: Arc<AtomicU64>,
        materialized_counter: Arc<AtomicU64>,
    ) -> FakeTargetDecoder {
        FakeTargetDecoder {
            stream: stream(),
            frames: VecDeque::from(vec![
                rgba_frame(0, 0, true),
                rgba_frame(1, 40, false),
                rgba_frame(2, 100, true),
                rgba_frame(3, 140, false),
                rgba_frame(4, 220, false),
            ]),
            current: None,
            decoded_counter,
            materialized_counter,
        }
    }

    #[test]
    fn cache_miss_materializes_only_verified_target_rgba() {
        let (index_path, index) = complete_index();
        let cache_root = temp_path("cache");
        let mut cache = FrameCacheHierarchy::open(&cache_root, 1024, 0).unwrap();
        let decoded = Arc::new(AtomicU64::new(0));
        let materialized = Arc::new(AtomicU64::new(0));

        let result = navigate_to_frame_cached_target_only(
            &index,
            &mut cache,
            || Ok(fake_decoder(decoded.clone(), materialized.clone())),
            FrameId(4),
        )
        .unwrap();

        assert_eq!(result.source, CachedFrameSource::Decoded);
        assert!(decoded.load(Ordering::Relaxed) > 1);
        assert_eq!(materialized.load(Ordering::Relaxed), 1);
        assert_eq!(result.pixels.pixels(), &[4; 16]);

        let decoded_after_first = decoded.load(Ordering::Relaxed);
        let second = navigate_to_frame_cached_target_only(
            &index,
            &mut cache,
            || Ok(fake_decoder(decoded.clone(), materialized.clone())),
            FrameId(4),
        )
        .unwrap();
        assert_eq!(second.source, CachedFrameSource::Ram);
        assert_eq!(decoded.load(Ordering::Relaxed), decoded_after_first);
        assert_eq!(materialized.load(Ordering::Relaxed), 1);

        drop(index);
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_dir_all(cache_root);
    }
}
