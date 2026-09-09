use crate::{
    CachedFrameSource, CachedNavigationError, CachedNavigationResult, MicroscopeNavigationError,
    MicroscopeTarget, RgbaNavigationDecoder, TargetRgbaNavigationCursor,
    TargetRgbaNavigationDecoder, microscope_target, navigate_to_frame_cached,
    navigate_to_frame_cached_target_only_with_cursor,
};
use framescope_cache::{FrameCacheHierarchy, FrameId, FrameIndex, OwnedRgbaFrame, RamInsertResult};
use framescope_core::FrameScopeError;
use thiserror::Error;

/// Source-quality RGBA presentation for one authoritative microscope frame.
///
/// The persistent index entry in `target` remains the timeline authority. `pixels` are owned
/// full-resolution RGBA produced from the source-quality navigation path. The compressed disk proxy
/// tier cannot satisfy this type.
#[derive(Debug, PartialEq, Eq)]
pub struct MicroscopeFramePresentation {
    pub target: MicroscopeTarget,
    pub pixels: OwnedRgbaFrame,
    pub source: CachedFrameSource,
    pub decoded_frames: u64,
    pub used_keyframe_seek: bool,
    pub fell_back_to_stream_start: bool,
    pub cache_insert_result: Option<RamInsertResult>,
}

impl MicroscopeFramePresentation {
    pub fn frame_id(&self) -> FrameId {
        self.target.frame_id()
    }
}

#[derive(Debug, Error)]
pub enum MicroscopePresentationError {
    #[error("microscope target resolution failed: {0}")]
    Target(#[from] MicroscopeNavigationError),
    #[error("source-quality frame navigation failed: {0}")]
    Navigation(#[from] CachedNavigationError),
    #[error("microscope target identity diverged from source-quality navigation")]
    IdentityMismatch,
}

/// Resolve and decode one microscope frame through the accepted authoritative paths.
///
/// This deliberately composes the Phase 5 microscope identity model with Phase 3 source-quality
/// cached navigation. A RAM hit avoids source decode. A RAM miss seeks from the indexed safe anchor
/// and decodes forward. Lossy disk proxies are never consulted because they are preview-only and
/// must not masquerade as original-quality pixels.
pub fn present_microscope_frame<D, F>(
    index: &FrameIndex,
    cache: &mut FrameCacheHierarchy,
    open_fresh_decoder: F,
    frame_id: FrameId,
) -> Result<MicroscopeFramePresentation, MicroscopePresentationError>
where
    D: RgbaNavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
{
    let target = microscope_target(index, frame_id)?;
    let navigation = navigate_to_frame_cached(index, cache, open_fresh_decoder, frame_id)?;
    presentation_from_navigation(target, navigation)
}

/// Resolve and decode an authoritative microscope frame while retaining a verified decoder cursor.
///
/// The cursor is only eligible for strictly-forward FrameId movement within `max_forward_frames`.
/// Every crossed presentation frame is reconciled against the persistent index before the requested
/// target is materialized as RGBA. Reversal, same-frame requests, large jumps, timeline divergence,
/// EOF, decoder errors, and cancellation all follow the fail-closed reset behavior implemented by
/// [`navigate_to_frame_cached_target_only_with_cursor`].
///
/// This is intentionally separate from the legacy presentation function so callers that cannot own
/// decoder lifetime retain the old API. Session-oriented callers should prefer this path: a warm
/// adjacent request can advance the already-open codec without another container seek or decoder
/// construction, while random seeks still use the persisted timestamp-seek-safety contract.
pub fn present_microscope_frame_with_cursor<D, F>(
    index: &FrameIndex,
    cache: &mut FrameCacheHierarchy,
    cursor: &mut Option<TargetRgbaNavigationCursor<D>>,
    open_fresh_decoder: F,
    frame_id: FrameId,
    max_forward_frames: u64,
) -> Result<MicroscopeFramePresentation, MicroscopePresentationError>
where
    D: TargetRgbaNavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
{
    let target = microscope_target(index, frame_id)?;
    let navigation = navigate_to_frame_cached_target_only_with_cursor(
        index,
        cache,
        cursor,
        open_fresh_decoder,
        frame_id,
        max_forward_frames,
    )?;
    presentation_from_navigation(target, navigation)
}

fn presentation_from_navigation(
    target: MicroscopeTarget,
    navigation: CachedNavigationResult,
) -> Result<MicroscopeFramePresentation, MicroscopePresentationError> {
    if navigation.frame_id != target.frame_id() || navigation.index_entry != target.entry {
        return Err(MicroscopePresentationError::IdentityMismatch);
    }

    Ok(MicroscopeFramePresentation {
        target,
        pixels: navigation.pixels,
        source: navigation.source,
        decoded_frames: navigation.decoded_frames,
        used_keyframe_seek: navigation.used_keyframe_seek,
        fell_back_to_stream_start: navigation.fell_back_to_stream_start,
        cache_insert_result: navigation.cache_insert_result,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{FrameIndexEntry, KeyframeAnchor};
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
    fn presentation_keeps_exact_target_identity_and_owned_pixels() {
        let presentation = MicroscopeFramePresentation {
            target: MicroscopeTarget {
                entry: entry(4),
                frame_count: 8,
            },
            pixels: OwnedRgbaFrame::new(2, 2, 8, vec![7; 16]).unwrap(),
            source: CachedFrameSource::Ram,
            decoded_frames: 0,
            used_keyframe_seek: false,
            fell_back_to_stream_start: false,
            cache_insert_result: None,
        };

        assert_eq!(presentation.frame_id(), FrameId(4));
        assert_eq!(presentation.target.entry, entry(4));
        assert_eq!(presentation.pixels.pixels(), &[7; 16]);
        assert_eq!(presentation.source, CachedFrameSource::Ram);
        assert_eq!(presentation.decoded_frames, 0);
    }
}
