//! Sequential source-quality video adapter for bounded similarity-group navigation.
//!
//! The core group-navigation crate intentionally knows nothing about FFmpeg or Android. This crate
//! adapts a fresh `VideoDecoder` into that core contract while preserving the Phase 2/3 rules:
//! presentation timestamps remain authoritative, the selected stream must match the indexed stream,
//! and pixels are owned RGBA snapshots rather than lossy preview proxies.

use framescope_cache::{FrameId, FrameIndex, FrameIndexStreamIdentity, OwnedRgbaFrame};
use framescope_core::{DecodedFrame, FrameScopeError};
use framescope_group_navigation::{
    GroupNavigationError, IndexedRgbaFrame, IndexedRgbaStream, SimilarityGroupAnalysis,
    SimilaritySourceError, open_or_build_group_navigation,
};
use framescope_perceptual::HybridSimilarityPolicy;
use framescope_video::{
    CancellationToken, OpenOptions, VideoDecoder, VideoStreamSelection,
};
use std::path::Path;

#[cfg(unix)]
use std::os::fd::BorrowedFd;

struct DecoderIndexedRgbaStream {
    decoder: VideoDecoder,
    next_frame_id: u64,
}

impl DecoderIndexedRgbaStream {
    fn new(decoder: VideoDecoder) -> Self {
        Self {
            decoder,
            next_frame_id: 0,
        }
    }
}

impl IndexedRgbaStream for DecoderIndexedRgbaStream {
    fn next_frame(&mut self) -> Result<Option<IndexedRgbaFrame>, SimilaritySourceError> {
        let Some(decoded) = self
            .decoder
            .next_frame_rgba()
            .map_err(source_from_frame_scope)?
        else {
            return Ok(None);
        };

        let frame_id = FrameId(self.next_frame_id);
        let frame = indexed_frame_from_decoded(
            frame_id,
            decoded.frame,
            decoded.stride_bytes,
            decoded.pixels,
        )?;
        self.next_frame_id = self.next_frame_id.checked_add(1).ok_or_else(|| {
            SimilaritySourceError::new("frame_id_overflow", "sequential frame id space exhausted")
        })?;
        Ok(Some(frame))
    }
}

/// Reuse or build similarity groups from a caller-owned Unix/Android file descriptor.
///
/// `VideoDecoder` duplicates the descriptor during open, so the caller retains ownership. The
/// decoder is opened lazily only when the validated Phase 4 store cannot be reused. Heavy decode
/// work therefore stays outside JNI registry locks and can run on the repository's IO dispatcher.
#[cfg(unix)]
pub fn open_or_build_group_navigation_from_fd(
    index: &FrameIndex,
    store_root: impl AsRef<Path>,
    policy: HybridSimilarityPolicy,
    fd: BorrowedFd<'_>,
    cancellation: CancellationToken,
) -> Result<SimilarityGroupAnalysis, GroupNavigationError> {
    let expected_stream = index.stream_identity().clone();
    let result = open_or_build_group_navigation(
        index,
        store_root,
        policy,
        || {
            let decoder = VideoDecoder::open_file_descriptor_with_options(
                fd,
                OpenOptions {
                    stream_selection: VideoStreamSelection::Index(expected_stream.stream_index),
                },
                cancellation.clone(),
            )
            .map_err(source_from_frame_scope)?;
            validate_stream_identity(&expected_stream, &decoder)?;
            Ok(DecoderIndexedRgbaStream::new(decoder))
        },
        || cancellation.is_cancelled(),
    );
    match result {
        Err(GroupNavigationError::Source(error)) if error.code == "cancelled" => {
            Err(GroupNavigationError::Cancelled)
        }
        other => other,
    }
}

fn validate_stream_identity(
    expected: &FrameIndexStreamIdentity,
    decoder: &VideoDecoder,
) -> Result<(), SimilaritySourceError> {
    let actual = FrameIndexStreamIdentity::from_stream(decoder.selected_stream())
        .map_err(|error| SimilaritySourceError::new("stream_identity_error", error.to_string()))?;
    if &actual != expected {
        return Err(SimilaritySourceError::new(
            "stream_identity_mismatch",
            "fresh similarity decoder stream does not match the authoritative frame index",
        ));
    }
    Ok(())
}

fn indexed_frame_from_decoded(
    frame_id: FrameId,
    decoded: DecodedFrame,
    stride_bytes: usize,
    pixels: Vec<u8>,
) -> Result<IndexedRgbaFrame, SimilaritySourceError> {
    if decoded.index != frame_id.0 {
        return Err(SimilaritySourceError::new(
            "decoder_sequence_mismatch",
            format!(
                "fresh decoder emitted local frame {} while sequential frame {} was expected",
                decoded.index, frame_id.0
            ),
        ));
    }
    let owned = OwnedRgbaFrame::new(decoded.width, decoded.height, stride_bytes, pixels)
        .map_err(|error| SimilaritySourceError::new("invalid_rgba_frame", error.to_string()))?;
    Ok(IndexedRgbaFrame {
        frame_id,
        presentation_timestamp: decoded.presentation_timestamp,
        duration: decoded.duration,
        keyframe: decoded.keyframe,
        corrupt: decoded.corrupt,
        pixels: owned,
    })
}

fn source_from_frame_scope(error: FrameScopeError) -> SimilaritySourceError {
    SimilaritySourceError::new(error.code(), error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};

    fn decoded(index: u64) -> DecodedFrame {
        let time_base = TimeBase::new(1, 1_000).unwrap();
        DecodedFrame {
            source_id: 7,
            stream_index: 0,
            decode_epoch: 0,
            index,
            presentation_timestamp: Some(MediaTimestamp {
                ticks: i64::try_from(index).unwrap() * 40,
                time_base,
            }),
            duration: Some(MediaDuration {
                ticks: 40,
                time_base,
            }),
            keyframe: index == 0,
            corrupt: false,
            width: 2,
            height: 1,
            pixel_format: Some("rgba".into()),
        }
    }

    #[test]
    fn decoded_rgba_mapping_preserves_authoritative_timing_metadata() {
        let mapped = indexed_frame_from_decoded(
            FrameId(3),
            decoded(3),
            8,
            vec![10, 20, 30, 255, 40, 50, 60, 255],
        )
        .unwrap();
        assert_eq!(mapped.frame_id, FrameId(3));
        assert_eq!(
            mapped.presentation_timestamp,
            decoded(3).presentation_timestamp
        );
        assert_eq!(mapped.duration, decoded(3).duration);
        assert_eq!(mapped.pixels.byte_len(), 8);
    }

    #[test]
    fn decoder_local_sequence_mismatch_is_rejected() {
        let error =
            indexed_frame_from_decoded(FrameId(4), decoded(3), 8, vec![0, 0, 0, 255, 0, 0, 0, 255])
                .unwrap_err();
        assert_eq!(error.code, "decoder_sequence_mismatch");
    }

    #[test]
    fn malformed_rgba_geometry_is_rejected() {
        let error =
            indexed_frame_from_decoded(FrameId(0), decoded(0), 4, vec![0, 0, 0, 255]).unwrap_err();
        assert_eq!(error.code, "invalid_rgba_frame");
    }

    #[test]
    fn native_cancellation_error_maps_to_stable_source_code() {
        let error = source_from_frame_scope(FrameScopeError::Cancelled);
        assert_eq!(error.code, "cancelled");
    }
}
