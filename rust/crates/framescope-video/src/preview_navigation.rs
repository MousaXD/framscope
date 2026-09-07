use crate::{
    CachedFrameSource, CachedNavigationError, RgbaNavigationDecoder, navigate_to_frame_cached,
};
use framescope_cache::{
    CacheLookup, DiskCacheError, DiskInsertResult, FrameCacheHierarchy, FrameCacheKey, FrameId,
    FrameIndex, OwnedRgbaFrame, ProxyFormat,
};
use framescope_core::FrameScopeError;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewSource {
    Disk,
    EncodedFromRam,
    EncodedFromDecode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedPreview {
    pub format: ProxyFormat,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewNavigationResult {
    pub frame_id: FrameId,
    pub format: ProxyFormat,
    pub bytes: Vec<u8>,
    pub source: PreviewSource,
    pub decoded_frames: u64,
    pub proxy_insert_result: Option<DiskInsertResult>,
    /// Non-fatal disk-cache failure encountered while serving this preview.
    ///
    /// The source video remains authoritative, so read/write failures in the disposable proxy tier
    /// degrade to source-quality navigation instead of making the frame unavailable.
    pub cache_warning: Option<String>,
}

#[derive(Debug, Error)]
pub enum PreviewNavigationError {
    #[error("frame-cache access failed: {0}")]
    Cache(#[from] DiskCacheError),
    #[error("source-quality indexed navigation failed: {0}")]
    Navigation(#[from] CachedNavigationError),
    #[error("preview encoding failed: {0}")]
    Encoding(String),
}

/// Encoder boundary for compressed navigation proxies.
///
/// Phase 3 intentionally keeps this abstraction free of Android APIs. A caller may supply a JPEG or
/// WebP implementation appropriate to its platform. The returned payload is validated by the disk
/// cache before it becomes reusable derived state.
pub trait PreviewEncoder {
    fn encode(&mut self, frame: &OwnedRgbaFrame) -> Result<EncodedPreview, String>;
}

impl<F> PreviewEncoder for F
where
    F: FnMut(&OwnedRgbaFrame) -> Result<EncodedPreview, String>,
{
    fn encode(&mut self, frame: &OwnedRgbaFrame) -> Result<EncodedPreview, String> {
        self(frame)
    }
}

/// Retrieve a preview-quality compressed frame through the Phase 3 cache hierarchy.
///
/// The lookup order is RAM full-quality frame -> compressed disk proxy -> authoritative indexed
/// source decode. A disk hit returns immediately without opening or decoding the source. If only a
/// full-quality RAM frame is available it is encoded directly, again without source decode. On a
/// true miss the indexed full-quality path decodes safely from the nearest keyframe, then the encoder
/// creates a disposable proxy which is inserted into disk storage. Disk-cache I/O is best-effort:
/// failures are reported through `cache_warning` while the authoritative source path remains usable.
pub fn navigate_to_frame_preview<D, F, E>(
    index: &FrameIndex,
    cache: &mut FrameCacheHierarchy,
    mut open_fresh_decoder: F,
    frame_id: FrameId,
    encoder: &mut E,
) -> Result<PreviewNavigationResult, PreviewNavigationError>
where
    D: RgbaNavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
    E: PreviewEncoder,
{
    let key = FrameCacheKey::new(
        index.source_identity(),
        index.stream_identity().stream_index,
        frame_id,
    )
    .map_err(DiskCacheError::from)?;

    let mut cache_warning = None;
    match cache.lookup(&key) {
        Ok(CacheLookup::Proxy(proxy)) => {
            return Ok(PreviewNavigationResult {
                frame_id,
                format: proxy.format,
                bytes: proxy.bytes().to_vec(),
                source: PreviewSource::Disk,
                decoded_frames: 0,
                proxy_insert_result: None,
                cache_warning: None,
            });
        }
        Ok(CacheLookup::Full(full)) => {
            return encode_and_store(
                cache,
                &key,
                frame_id,
                &full.pixels,
                PreviewSource::EncodedFromRam,
                0,
                encoder,
                None,
            );
        }
        Ok(CacheLookup::Miss) => {}
        Err(error) => {
            cache_warning = Some(format!(
                "disk proxy lookup failed; falling back to authoritative source decode: {error}"
            ));
        }
    }

    let decoded = navigate_to_frame_cached(index, cache, &mut open_fresh_decoder, frame_id)?;
    let source = match decoded.source {
        CachedFrameSource::Ram => PreviewSource::EncodedFromRam,
        CachedFrameSource::Decoded => PreviewSource::EncodedFromDecode,
    };
    encode_and_store(
        cache,
        &key,
        frame_id,
        &decoded.pixels,
        source,
        decoded.decoded_frames,
        encoder,
        cache_warning,
    )
}

fn encode_and_store<E: PreviewEncoder>(
    cache: &mut FrameCacheHierarchy,
    key: &FrameCacheKey,
    frame_id: FrameId,
    pixels: &OwnedRgbaFrame,
    source: PreviewSource,
    decoded_frames: u64,
    encoder: &mut E,
    cache_warning: Option<String>,
) -> Result<PreviewNavigationResult, PreviewNavigationError> {
    let encoded = encoder
        .encode(pixels)
        .map_err(PreviewNavigationError::Encoding)?;
    let (proxy_insert_result, cache_warning) =
        match cache.insert_proxy(key, encoded.format, &encoded.bytes) {
            Ok(result) => (Some(result), cache_warning),
            Err(error) => (
                None,
                Some(append_cache_warning(
                    cache_warning,
                    format!("disk proxy insert failed; preview remains usable: {error}"),
                )),
            ),
        };
    Ok(PreviewNavigationResult {
        frame_id,
        format: encoded.format,
        bytes: encoded.bytes,
        source,
        decoded_frames,
        proxy_insert_result,
        cache_warning,
    })
}

fn append_cache_warning(existing: Option<String>, next: String) -> String {
    match existing {
        Some(existing) => format!("{existing}; {next}"),
        None => next,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::DecodedRgbaFrame;
    use framescope_cache::{
        FrameIndexEntry, FrameIndexOpenDisposition, FrameIndexStreamIdentity, KeyframeAnchor,
        SourceIdentity,
    };
    use framescope_core::{
        CodecInfo, DecodedFrame, MediaDuration, MediaKind, MediaTimestamp, StreamInfo, TimeBase,
    };
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    struct FakeDecoder {
        stream: StreamInfo,
        frames: VecDeque<DecodedRgbaFrame>,
        decoded: Arc<AtomicU64>,
    }

    impl RgbaNavigationDecoder for FakeDecoder {
        fn selected_stream_for_rgba_navigation(&self) -> &StreamInfo {
            &self.stream
        }

        fn next_rgba_for_navigation(
            &mut self,
        ) -> Result<Option<DecodedRgbaFrame>, FrameScopeError> {
            let frame = self.frames.pop_front();
            if frame.is_some() {
                self.decoded.fetch_add(1, Ordering::Relaxed);
            }
            Ok(frame)
        }

        fn seek_for_rgba_navigation(&mut self, _timestamp_us: i64) -> Result<(), FrameScopeError> {
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

    fn frame(id: u64, ticks: i64) -> DecodedRgbaFrame {
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
                keyframe: id == 0,
                corrupt: false,
                width: 2,
                height: 2,
                pixel_format: Some("yuv420p".into()),
            },
            stride_bytes: 8,
            pixels: vec![id as u8; 16],
        }
    }

    fn temp_path(label: &str) -> PathBuf {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "framescope-preview-navigation-{label}-{}-{sequence}",
            std::process::id()
        ))
    }

    fn index() -> (PathBuf, FrameIndex) {
        let path = temp_path("index.sqlite");
        let source = SourceIdentity::new(10, None, Some("preview-navigation".into()));
        let identity = FrameIndexStreamIdentity::from_stream(&stream()).unwrap();
        let (mut index, disposition) = FrameIndex::open_or_create(&path, source, identity).unwrap();
        assert_eq!(disposition, FrameIndexOpenDisposition::Created);
        index.mark_building().unwrap();
        let entries = [0_i64, 40, 80]
            .into_iter()
            .enumerate()
            .map(|(id, ticks)| FrameIndexEntry {
                frame_id: FrameId(id as u64),
                presentation_timestamp: Some(timestamp(ticks)),
                duration: Some(MediaDuration {
                    ticks: 40,
                    time_base: TimeBase::new(1, 1_000).unwrap(),
                }),
                keyframe: id == 0,
                corrupt: false,
                anchor: KeyframeAnchor::Keyframe {
                    frame_id: FrameId(0),
                    presentation_timestamp: Some(timestamp(0)),
                },
            })
            .collect::<Vec<_>>();
        index.append_batch(&entries).unwrap();
        index.mark_complete().unwrap();
        (path, index)
    }

    fn decoder(counter: Arc<AtomicU64>) -> FakeDecoder {
        FakeDecoder {
            stream: stream(),
            frames: VecDeque::from(vec![frame(0, 0), frame(1, 40), frame(2, 80)]),
            decoded: counter,
        }
    }

    fn jpeg_encoder() -> impl PreviewEncoder {
        |_frame: &OwnedRgbaFrame| {
            Ok(EncodedPreview {
                format: ProxyFormat::Jpeg,
                bytes: vec![0xff, 0xd8, 0xff, 0xd9],
            })
        }
    }

    #[test]
    fn cold_preview_populates_disk_then_ram_miss_disk_hit_avoids_decode() {
        let (index_path, index) = index();
        let cache_root = temp_path("cache");
        let decoded = Arc::new(AtomicU64::new(0));
        let mut encoder = jpeg_encoder();

        // Zero RAM budget guarantees the source-quality payload cannot remain resident.
        let mut cache = FrameCacheHierarchy::open(&cache_root, 0, 1024).unwrap();
        let first = navigate_to_frame_preview(
            &index,
            &mut cache,
            || Ok(decoder(decoded.clone())),
            FrameId(2),
            &mut encoder,
        )
        .unwrap();
        assert_eq!(first.source, PreviewSource::EncodedFromDecode);
        assert!(first.cache_warning.is_none());
        let decoded_after_first = decoded.load(Ordering::Relaxed);
        assert!(decoded_after_first > 0);

        let second = navigate_to_frame_preview(
            &index,
            &mut cache,
            || Ok(decoder(decoded.clone())),
            FrameId(2),
            &mut encoder,
        )
        .unwrap();
        assert_eq!(second.source, PreviewSource::Disk);
        assert_eq!(second.decoded_frames, 0);
        assert!(second.cache_warning.is_none());
        assert_eq!(decoded.load(Ordering::Relaxed), decoded_after_first);
        assert_eq!(second.bytes, vec![0xff, 0xd8, 0xff, 0xd9]);

        drop(index);
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_dir_all(cache_root);
    }

    #[test]
    fn ram_full_frame_can_generate_proxy_without_source_decode() {
        let (index_path, index) = index();
        let cache_root = temp_path("ram-cache");
        let mut cache = FrameCacheHierarchy::open(&cache_root, 1024, 1024).unwrap();
        let key = FrameCacheKey::new(index.source_identity(), 0, FrameId(1)).unwrap();
        cache.insert_full(framescope_cache::CachedFrame {
            key,
            pixels: OwnedRgbaFrame::new(2, 2, 8, vec![1; 16]).unwrap(),
        });
        let decoded = Arc::new(AtomicU64::new(0));
        let mut encoder = jpeg_encoder();

        let result = navigate_to_frame_preview(
            &index,
            &mut cache,
            || Ok(decoder(decoded.clone())),
            FrameId(1),
            &mut encoder,
        )
        .unwrap();
        assert_eq!(result.source, PreviewSource::EncodedFromRam);
        assert_eq!(result.decoded_frames, 0);
        assert!(result.cache_warning.is_none());
        assert_eq!(decoded.load(Ordering::Relaxed), 0);

        drop(index);
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_dir_all(cache_root);
    }

    #[test]
    fn disk_io_failure_degrades_to_source_decode_and_keeps_preview_usable() {
        let (index_path, index) = index();
        let cache_root = temp_path("broken-disk-cache");
        let decoded = Arc::new(AtomicU64::new(0));
        let mut encoder = jpeg_encoder();
        let mut cache = FrameCacheHierarchy::open(&cache_root, 0, 1024).unwrap();

        // Replace the version directory with a regular file after open. Subsequent proxy reads and
        // writes fail with a filesystem error, while the RAM/source-quality path remains available.
        let version_root = cache_root.join("v1");
        std::fs::remove_dir_all(&version_root).unwrap();
        std::fs::write(&version_root, b"blocked").unwrap();

        let result = navigate_to_frame_preview(
            &index,
            &mut cache,
            || Ok(decoder(decoded.clone())),
            FrameId(2),
            &mut encoder,
        )
        .unwrap();

        assert_eq!(result.source, PreviewSource::EncodedFromDecode);
        assert!(result.decoded_frames > 0);
        assert!(decoded.load(Ordering::Relaxed) > 0);
        assert_eq!(result.bytes, vec![0xff, 0xd8, 0xff, 0xd9]);
        assert!(result.proxy_insert_result.is_none());
        let warning = result.cache_warning.expect("disk failure should be diagnostic");
        assert!(warning.contains("disk proxy lookup failed"));
        assert!(warning.contains("disk proxy insert failed"));

        drop(index);
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_file(version_root);
        let _ = std::fs::remove_dir_all(cache_root);
    }
}
