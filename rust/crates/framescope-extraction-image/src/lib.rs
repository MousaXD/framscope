//! Bounded single-frame image encoding for FrameScope extraction.
//!
//! This crate owns only image-format conversion. It does not select frames, decode video, choose an
//! Android destination, or accumulate a batch in memory. Callers provide one validated source-quality
//! RGBA frame and a writer; encoded bytes are streamed to that writer before the next frame is
//! requested from the extraction engine.

use framescope_cache::{FrameId, OwnedRgbaFrame};
use image::codecs::{jpeg::JpegEncoder, png::PngEncoder, webp::WebPEncoder};
use image::{ExtendedColorType, ImageEncoder};
use std::borrow::Cow;
use std::io::{self, Write};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionImageFormat {
    Png,
    Jpeg { quality: u8 },
    WebPLossless,
}

impl ExtractionImageFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg { .. } => "jpg",
            Self::WebPLossless => "webp",
        }
    }

    pub fn mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg { .. } => "image/jpeg",
            Self::WebPLossless => "image/webp",
        }
    }

    fn validate(self) -> Result<(), ImageExportError> {
        if let Self::Jpeg { quality } = self {
            if !(1..=100).contains(&quality) {
                return Err(ImageExportError::InvalidJpegQuality(quality));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedImageReport {
    pub format: ExtractionImageFormat,
    pub byte_len: u64,
}

#[derive(Debug, Error)]
pub enum ImageExportError {
    #[error("JPEG quality must be between 1 and 100, got {0}")]
    InvalidJpegQuality(u8),
    #[error("image dimensions or row layout overflow the supported numeric range")]
    NumericRange,
    #[error("image encoder failed: {0}")]
    Image(#[from] image::ImageError),
    #[error("image output writer failed: {0}")]
    Writer(#[from] io::Error),
}

/// Encode one source-quality RGBA frame directly into `writer`.
///
/// PNG and lossless WebP preserve RGBA. JPEG deliberately drops alpha and encodes RGB at the
/// requested quality. If the source stride contains row padding, only the visible RGBA bytes are
/// copied into a temporary tight frame; otherwise PNG/WebP borrow the source bytes without a full
/// frame copy. JPEG requires one bounded RGB conversion buffer because the format has no alpha.
pub fn encode_frame<W: Write>(
    frame: &OwnedRgbaFrame,
    format: ExtractionImageFormat,
    writer: &mut W,
) -> Result<EncodedImageReport, ImageExportError> {
    format.validate()?;
    let tight_rgba = tight_rgba(frame)?;
    let mut counting = CountingWriter::new(writer);

    match format {
        ExtractionImageFormat::Png => {
            PngEncoder::new(&mut counting).write_image(
                tight_rgba.as_ref(),
                frame.width,
                frame.height,
                ExtendedColorType::Rgba8,
            )?;
        }
        ExtractionImageFormat::Jpeg { quality } => {
            let rgb = rgba_to_rgb(tight_rgba.as_ref())?;
            JpegEncoder::new_with_quality(&mut counting, quality).encode(
                &rgb,
                frame.width,
                frame.height,
                ExtendedColorType::Rgb8,
            )?;
        }
        ExtractionImageFormat::WebPLossless => {
            WebPEncoder::new_lossless(&mut counting).encode(
                tight_rgba.as_ref(),
                frame.width,
                frame.height,
                ExtendedColorType::Rgba8,
            )?;
        }
    }

    counting.flush()?;
    Ok(EncodedImageReport {
        format,
        byte_len: counting.bytes_written,
    })
}

/// Stable lexicographically sortable name for one persistent FrameId.
pub fn frame_file_name(frame_id: FrameId, format: ExtractionImageFormat) -> String {
    format!("frame_{:020}.{}", frame_id.0, format.extension())
}

fn tight_rgba(frame: &OwnedRgbaFrame) -> Result<Cow<'_, [u8]>, ImageExportError> {
    let width = usize::try_from(frame.width).map_err(|_| ImageExportError::NumericRange)?;
    let height = usize::try_from(frame.height).map_err(|_| ImageExportError::NumericRange)?;
    let row_bytes = width
        .checked_mul(4)
        .ok_or(ImageExportError::NumericRange)?;
    let tight_len = row_bytes
        .checked_mul(height)
        .ok_or(ImageExportError::NumericRange)?;

    if frame.stride_bytes == row_bytes {
        return Ok(Cow::Borrowed(frame.pixels()));
    }

    let mut tight = Vec::with_capacity(tight_len);
    for row in 0..height {
        let start = row
            .checked_mul(frame.stride_bytes)
            .ok_or(ImageExportError::NumericRange)?;
        let end = start
            .checked_add(row_bytes)
            .ok_or(ImageExportError::NumericRange)?;
        tight.extend_from_slice(&frame.pixels()[start..end]);
    }
    debug_assert_eq!(tight.len(), tight_len);
    Ok(Cow::Owned(tight))
}

fn rgba_to_rgb(rgba: &[u8]) -> Result<Vec<u8>, ImageExportError> {
    if rgba.len() % 4 != 0 {
        return Err(ImageExportError::NumericRange);
    }
    let pixel_count = rgba.len() / 4;
    let rgb_len = pixel_count
        .checked_mul(3)
        .ok_or(ImageExportError::NumericRange)?;
    let mut rgb = Vec::with_capacity(rgb_len);
    for pixel in rgba.chunks_exact(4) {
        rgb.extend_from_slice(&pixel[..3]);
    }
    Ok(rgb)
}

struct CountingWriter<W> {
    inner: W,
    bytes_written: u64,
}

impl<W> CountingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            bytes_written: 0,
        }
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.bytes_written = self
            .bytes_written
            .checked_add(u64::try_from(written).map_err(|_| io::Error::other("write size overflow"))?)
            .ok_or_else(|| io::Error::other("encoded image byte count overflow"))?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, RgbaImage};

    fn padded_rgba() -> OwnedRgbaFrame {
        let mut pixels = Vec::new();
        pixels.extend_from_slice(&[255, 0, 0, 255, 0, 255, 0, 128, 9, 9, 9, 9]);
        pixels.extend_from_slice(&[0, 0, 255, 64, 255, 255, 255, 0, 8, 8, 8, 8]);
        OwnedRgbaFrame::new(2, 2, 12, pixels).unwrap()
    }

    fn tight_expected() -> Vec<u8> {
        vec![
            255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 64, 255, 255, 255, 0,
        ]
    }

    #[test]
    fn png_preserves_rgba_and_ignores_stride_padding() {
        let frame = padded_rgba();
        let mut encoded = Vec::new();
        let report = encode_frame(&frame, ExtractionImageFormat::Png, &mut encoded).unwrap();

        assert_eq!(report.byte_len, encoded.len() as u64);
        assert!(encoded.starts_with(b"\x89PNG\r\n\x1a\n"));
        let decoded = image::load_from_memory_with_format(&encoded, ImageFormat::Png)
            .unwrap()
            .to_rgba8();
        assert_eq!(decoded.dimensions(), (2, 2));
        assert_eq!(decoded.into_raw(), tight_expected());
    }

    #[test]
    fn jpeg_validates_quality_before_writing() {
        let frame = padded_rgba();
        let mut encoded = Vec::new();
        let result = encode_frame(
            &frame,
            ExtractionImageFormat::Jpeg { quality: 0 },
            &mut encoded,
        );

        assert!(matches!(result, Err(ImageExportError::InvalidJpegQuality(0))));
        assert!(encoded.is_empty());
    }

    #[test]
    fn jpeg_encodes_visible_rgb_pixels() {
        let frame = padded_rgba();
        let mut encoded = Vec::new();
        let report = encode_frame(
            &frame,
            ExtractionImageFormat::Jpeg { quality: 90 },
            &mut encoded,
        )
        .unwrap();

        assert_eq!(report.byte_len, encoded.len() as u64);
        assert_eq!(&encoded[..2], &[0xff, 0xd8]);
        let decoded = image::load_from_memory_with_format(&encoded, ImageFormat::Jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
    }

    #[test]
    fn webp_lossless_preserves_rgba() {
        let frame = padded_rgba();
        let mut encoded = Vec::new();
        let report = encode_frame(
            &frame,
            ExtractionImageFormat::WebPLossless,
            &mut encoded,
        )
        .unwrap();

        assert_eq!(report.byte_len, encoded.len() as u64);
        assert_eq!(&encoded[..4], b"RIFF");
        assert_eq!(&encoded[8..12], b"WEBP");
        let decoded: RgbaImage = image::load_from_memory_with_format(&encoded, ImageFormat::WebP)
            .unwrap()
            .to_rgba8();
        assert_eq!(decoded.into_raw(), tight_expected());
    }

    #[test]
    fn file_names_are_stable_sortable_and_format_specific() {
        assert_eq!(
            frame_file_name(FrameId(42), ExtractionImageFormat::Png),
            "frame_00000000000000000042.png"
        );
        assert_eq!(
            frame_file_name(
                FrameId(42),
                ExtractionImageFormat::Jpeg { quality: 92 }
            ),
            "frame_00000000000000000042.jpg"
        );
        assert_eq!(
            frame_file_name(FrameId(42), ExtractionImageFormat::WebPLossless),
            "frame_00000000000000000042.webp"
        );
    }
}
