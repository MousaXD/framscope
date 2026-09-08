#[path = "storage_admin.rs"]
mod storage_admin;

use framescope_video::MicroscopeFramePresentation;
use std::fmt;

/// Metadata describing the exact RGBA payload copied into a caller-owned presentation buffer.
///
/// The buffer contains source-quality pixels from `MicroscopeFramePresentation`; compressed preview
/// proxies cannot reach this boundary because the presentation API structurally excludes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationBufferInfo {
    pub frame_id: u64,
    pub width: u32,
    pub height: u32,
    pub stride_bytes: usize,
    pub byte_len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresentationBufferError {
    BufferTooSmall { required: usize, capacity: usize },
}

impl fmt::Display for PresentationBufferError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BufferTooSmall { required, capacity } => write!(
                formatter,
                "presentation buffer capacity {capacity} is smaller than required RGBA payload {required}",
            ),
        }
    }
}

impl std::error::Error for PresentationBufferError {}

/// Copy one authoritative full-resolution RGBA frame into caller-owned memory.
///
/// This function deliberately owns no JNI lifetime assumptions. Android can provide a direct
/// `ByteBuffer`, Rust can borrow its address only for the JNI call, and this safe core performs the
/// bounded copy. The destination is checked before any write so an undersized buffer is never left
/// with a partial frame.
pub fn copy_presentation_rgba(
    presentation: &MicroscopeFramePresentation,
    destination: &mut [u8],
) -> Result<PresentationBufferInfo, PresentationBufferError> {
    let pixels = presentation.pixels.pixels();
    if destination.len() < pixels.len() {
        return Err(PresentationBufferError::BufferTooSmall {
            required: pixels.len(),
            capacity: destination.len(),
        });
    }

    destination[..pixels.len()].copy_from_slice(pixels);
    Ok(PresentationBufferInfo {
        frame_id: presentation.frame_id().0,
        width: presentation.pixels.width,
        height: presentation.pixels.height,
        stride_bytes: presentation.pixels.stride_bytes,
        byte_len: pixels.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use framescope_cache::{FrameId, FrameIndexEntry, KeyframeAnchor, OwnedRgbaFrame};
    use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};
    use framescope_video::{CachedFrameSource, MicroscopeTarget};

    fn presentation() -> MicroscopeFramePresentation {
        let time_base = TimeBase::new(1, 1_000).unwrap();
        MicroscopeFramePresentation {
            target: MicroscopeTarget {
                entry: FrameIndexEntry {
                    frame_id: FrameId(7),
                    presentation_timestamp: Some(MediaTimestamp {
                        ticks: 280,
                        time_base,
                    }),
                    duration: Some(MediaDuration {
                        ticks: 40,
                        time_base,
                    }),
                    keyframe: false,
                    corrupt: false,
                    anchor: KeyframeAnchor::Keyframe {
                        frame_id: FrameId::ZERO,
                        presentation_timestamp: Some(MediaTimestamp {
                            ticks: 0,
                            time_base,
                        }),
                    },
                },
                frame_count: 10,
            },
            pixels: OwnedRgbaFrame::new(
                2,
                2,
                8,
                vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
            )
            .unwrap(),
            source: CachedFrameSource::Decoded,
            decoded_frames: 3,
            used_keyframe_seek: true,
            fell_back_to_stream_start: false,
            cache_insert_result: None,
        }
    }

    #[test]
    fn copies_exact_source_quality_rgba_into_caller_owned_memory() {
        let presentation = presentation();
        let mut destination = [0_u8; 24];

        let info = copy_presentation_rgba(&presentation, &mut destination).unwrap();

        assert_eq!(info.frame_id, 7);
        assert_eq!(info.width, 2);
        assert_eq!(info.height, 2);
        assert_eq!(info.stride_bytes, 8);
        assert_eq!(info.byte_len, 16);
        assert_eq!(&destination[..16], presentation.pixels.pixels());
        assert_eq!(&destination[16..], &[0; 8]);
    }

    #[test]
    fn undersized_destination_is_rejected_before_any_write() {
        let presentation = presentation();
        let mut destination = [0x5a_u8; 15];
        let before = destination;

        let error = copy_presentation_rgba(&presentation, &mut destination).unwrap_err();

        assert_eq!(
            error,
            PresentationBufferError::BufferTooSmall {
                required: 16,
                capacity: 15,
            }
        );
        assert_eq!(destination, before);
    }
}
