from pathlib import Path

def read(path):
    return Path(path).read_text()

def write(path, text):
    Path(path).write_text(text)

def replace_text(path, old, new):
    text = read(path)
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one exact match, found {count}: {old[:80]!r}")
    write(path, text.replace(old, new, 1))

def insert_before(path, marker, addition):
    text = read(path)
    count = text.count(marker)
    if count != 1:
        raise SystemExit(f"{path}: expected one insertion marker, found {count}: {marker[:80]!r}")
    write(path, text.replace(marker, addition + marker, 1))

def replace_region(path, start, end, replacement):
    text = read(path)
    i = text.find(start)
    if i < 0:
        raise SystemExit(f"{path}: missing region start {start!r}")
    j = text.find(end, i)
    if j < 0:
        raise SystemExit(f"{path}: missing region end {end!r}")
    if text.find(start, i + len(start)) >= 0:
        raise SystemExit(f"{path}: region start is not unique {start!r}")
    write(path, text[:i] + replacement + text[j:])

# C shim: add a reusable scaled RGBA copy primitive. Full-quality callers keep
# SWS_BILINEAR; disposable bounded previews use SWS_POINT to preserve the old
# nearest-neighbour preview policy without a full-resolution RGBA intermediate.
c_path = "rust/crates/framescope-ffmpeg/native/framescope_ffmpeg_shim.c"
replace_region(
    c_path,
    "int32_t framescope_ffmpeg_copy_current_frame_rgba(\n",
    "int32_t framescope_ffmpeg_seek_us(",
    r'''static int32_t fs_copy_current_frame_rgba_to(
    FsSession *session,
    int32_t target_width,
    int32_t target_height,
    int flags,
    uint8_t *output,
    size_t output_capacity,
    int32_t *out_stride,
    FsError *error
) {
    uint8_t *destination_data[4] = {NULL, NULL, NULL, NULL};
    int destination_linesize[4] = {0, 0, 0, 0};
    size_t stride;
    size_t required;
    int scaled_rows;

    fs_clear_error(error);
    if (session == NULL || session->frame == NULL || output == NULL || out_stride == NULL) {
        fs_set_error(error, FS_ERR_BACKEND, 0, "decoded frame RGBA copy received invalid arguments");
        return -1;
    }
    if (session->frame->width <= 0 || session->frame->height <= 0 || session->frame->format < 0 || session->frame->data[0] == NULL) {
        fs_set_error(error, FS_ERR_DECODER, 0, "no valid decoded frame is available for RGBA copy");
        return -1;
    }
    if (target_width <= 0 || target_height <= 0 ||
        target_width > session->frame->width || target_height > session->frame->height) {
        fs_set_error(error, FS_ERR_BACKEND, AVERROR(EINVAL), "RGBA target dimensions must be positive and must not upscale the decoded frame");
        return -1;
    }
    if (target_width > INT_MAX / 4) {
        fs_set_error(error, FS_ERR_DECODER, AVERROR(EOVERFLOW), "RGBA target width overflows stride");
        return -1;
    }

    stride = (size_t)target_width * 4U;
    if ((size_t)target_height > SIZE_MAX / stride) {
        fs_set_error(error, FS_ERR_DECODER, AVERROR(EOVERFLOW), "RGBA target dimensions overflow buffer size");
        return -1;
    }
    required = stride * (size_t)target_height;
    if (output_capacity < required) {
        fs_set_error(error, FS_ERR_BACKEND, AVERROR(ENOSPC), "RGBA output buffer is smaller than the requested target frame");
        return -1;
    }

    session->rgba_scaler = sws_getCachedContext(
        session->rgba_scaler,
        session->frame->width,
        session->frame->height,
        (enum AVPixelFormat)session->frame->format,
        target_width,
        target_height,
        AV_PIX_FMT_RGBA,
        flags,
        NULL,
        NULL,
        NULL
    );
    if (session->rgba_scaler == NULL) {
        fs_set_error(error, FS_ERR_DECODER, AVERROR(ENOMEM), "failed to create or reuse RGBA conversion context");
        return -1;
    }

    destination_data[0] = output;
    destination_linesize[0] = (int)stride;
    scaled_rows = sws_scale(
        session->rgba_scaler,
        (const uint8_t *const *)session->frame->data,
        session->frame->linesize,
        0,
        session->frame->height,
        destination_data,
        destination_linesize
    );

    if (scaled_rows != target_height) {
        fs_set_error(error, FS_ERR_DECODER, 0, "FFmpeg did not convert the complete decoded frame to the requested RGBA size");
        return -1;
    }

    *out_stride = (int32_t)stride;
    return 0;
}

int32_t framescope_ffmpeg_copy_current_frame_rgba(
    void *opaque,
    uint8_t *output,
    size_t output_capacity,
    int32_t *out_stride,
    FsError *error
) {
    FsSession *session = (FsSession *)opaque;
    if (session == NULL || session->frame == NULL) {
        return fs_copy_current_frame_rgba_to(
            session, 0, 0, SWS_BILINEAR, output, output_capacity, out_stride, error
        );
    }
    return fs_copy_current_frame_rgba_to(
        session,
        session->frame->width,
        session->frame->height,
        SWS_BILINEAR,
        output,
        output_capacity,
        out_stride,
        error
    );
}

int32_t framescope_ffmpeg_copy_current_frame_rgba_scaled(
    void *opaque,
    int32_t target_width,
    int32_t target_height,
    uint8_t *output,
    size_t output_capacity,
    int32_t *out_stride,
    FsError *error
) {
    return fs_copy_current_frame_rgba_to(
        (FsSession *)opaque,
        target_width,
        target_height,
        SWS_POINT,
        output,
        output_capacity,
        out_stride,
        error
    );
}

''')

# Rust FFmpeg boundary: expose exact target-size conversion without allowing upscale.
ffmpeg_path = "rust/crates/framescope-ffmpeg/src/lib.rs"
replace_text(
    ffmpeg_path,
    '''        fn framescope_ffmpeg_copy_current_frame_rgba(
            session: *mut FsSession,
            output: *mut u8,
            output_capacity: usize,
            out_stride: *mut i32,
            error: *mut FsError,
        ) -> i32;
        fn framescope_ffmpeg_seek_us(
''',
    '''        fn framescope_ffmpeg_copy_current_frame_rgba(
            session: *mut FsSession,
            output: *mut u8,
            output_capacity: usize,
            out_stride: *mut i32,
            error: *mut FsError,
        ) -> i32;
        fn framescope_ffmpeg_copy_current_frame_rgba_scaled(
            session: *mut FsSession,
            target_width: i32,
            target_height: i32,
            output: *mut u8,
            output_capacity: usize,
            out_stride: *mut i32,
            error: *mut FsError,
        ) -> i32;
        fn framescope_ffmpeg_seek_us(
''',
)

replace_region(
    ffmpeg_path,
    "        /// Copy the most recently decoded frame into owned tightly-packed RGBA bytes.\n",
    "        pub fn seek_us(&mut self, timestamp_us: i64) -> Result<(), NativeError> {\n",
    r'''        /// Copy the most recently decoded frame into owned tightly-packed RGBA bytes.
        ///
        /// This must be called before another `next_frame` or seek. The returned Vec owns its data
        /// and remains valid after the decoder advances or is dropped.
        pub fn copy_current_frame_rgba(&mut self) -> Result<NativeRgbaFrame, NativeError> {
            let (width, height) = self.last_frame_dimensions.ok_or_else(|| {
                NativeError::backend("no decoded frame is available for RGBA copy")
            })?;
            self.copy_current_frame_rgba_to_dimensions(width, height, false)
        }

        /// Convert the current frame directly to a caller-selected bounded RGBA size.
        ///
        /// Target dimensions must be positive and may not exceed the decoded frame. This is used
        /// only for disposable previews; authoritative/source-quality callers use
        /// [`Self::copy_current_frame_rgba`].
        pub fn copy_current_frame_rgba_resized(
            &mut self,
            width: u32,
            height: u32,
        ) -> Result<NativeRgbaFrame, NativeError> {
            let (source_width, source_height) = self.last_frame_dimensions.ok_or_else(|| {
                NativeError::backend("no decoded frame is available for RGBA resize")
            })?;
            if width == 0 || height == 0 || width > source_width || height > source_height {
                return Err(NativeError::backend(
                    "RGBA resize dimensions must be positive and must not upscale the decoded frame",
                ));
            }
            self.copy_current_frame_rgba_to_dimensions(width, height, true)
        }

        fn copy_current_frame_rgba_to_dimensions(
            &mut self,
            width: u32,
            height: u32,
            scaled: bool,
        ) -> Result<NativeRgbaFrame, NativeError> {
            let stride = usize::try_from(width)
                .ok()
                .and_then(|value| value.checked_mul(4))
                .ok_or_else(|| NativeError::backend("decoded frame RGBA stride overflows usize"))?;
            let required = stride
                .checked_mul(
                    usize::try_from(height)
                        .map_err(|_| NativeError::backend("decoded frame height exceeds usize"))?,
                )
                .ok_or_else(|| NativeError::backend("decoded frame RGBA size overflows usize"))?;
            let mut pixels = Vec::new();
            pixels
                .try_reserve_exact(required)
                .map_err(|_| NativeError::backend("failed to allocate owned RGBA frame buffer"))?;
            pixels.resize(required, 0);

            let mut out_stride = 0_i32;
            let mut error = FsError::default();
            // SAFETY: pixels has exactly `required` initialized writable bytes and remains alive
            // for the call. The C shim validates target dimensions and capacity. &mut self keeps
            // the reusable AVFrame and cached scaler exclusively owned for the conversion.
            let result = unsafe {
                if scaled {
                    framescope_ffmpeg_copy_current_frame_rgba_scaled(
                        self.raw.as_ptr(),
                        i32::try_from(width).map_err(|_| {
                            NativeError::backend("RGBA resize width exceeds FFmpeg range")
                        })?,
                        i32::try_from(height).map_err(|_| {
                            NativeError::backend("RGBA resize height exceeds FFmpeg range")
                        })?,
                        pixels.as_mut_ptr(),
                        pixels.len(),
                        &mut out_stride,
                        &mut error,
                    )
                } else {
                    framescope_ffmpeg_copy_current_frame_rgba(
                        self.raw.as_ptr(),
                        pixels.as_mut_ptr(),
                        pixels.len(),
                        &mut out_stride,
                        &mut error,
                    )
                }
            };
            if result < 0 {
                return Err(error_from_ffi(&error));
            }
            let out_stride = usize::try_from(out_stride)
                .ok()
                .filter(|value| *value == stride)
                .ok_or_else(|| {
                    NativeError::backend("native RGBA conversion returned an invalid stride")
                })?;

            Ok(NativeRgbaFrame {
                width,
                height,
                stride: out_stride,
                pixels,
            })
        }

''',
)

replace_text(
    ffmpeg_path,
    '''    pub fn copy_current_frame_rgba(&mut self) -> Result<NativeRgbaFrame, NativeError> {
        Err(NativeError::backend("FFmpeg backend is not linked"))
    }

    pub fn seek_us(&mut self, _timestamp_us: i64) -> Result<(), NativeError> {
''',
    '''    pub fn copy_current_frame_rgba(&mut self) -> Result<NativeRgbaFrame, NativeError> {
        Err(NativeError::backend("FFmpeg backend is not linked"))
    }

    pub fn copy_current_frame_rgba_resized(
        &mut self,
        _width: u32,
        _height: u32,
    ) -> Result<NativeRgbaFrame, NativeError> {
        Err(NativeError::backend("FFmpeg backend is not linked"))
    }

    pub fn seek_us(&mut self, _timestamp_us: i64) -> Result<(), NativeError> {
''',
)

# Share the existing aspect-ratio rounding policy with the native preview path.
replace_text(
    "rust/crates/framescope-video/src/scrub_preview.rs",
    "fn preview_dimensions(\n",
    "pub(crate) fn preview_dimensions(\n",
)

# VideoDecoder: preserve source metadata while bounded pixels use a distinct type.
engine_path = "rust/crates/framescope-video/src/engine.rs"
insert_before(
    engine_path,
    "#[derive(Debug, Default)]\nstruct TimestampCadence",
    r'''/// Disposable bounded RGBA pixels for an already-verified decoded presentation frame.
///
/// `frame` retains source-quality metadata and identity. `width`/`height` describe only the
/// disposable preview pixel buffer and must never be used as authoritative frame dimensions or
/// inserted under a source-quality cache key.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedPreviewRgbaFrame {
    pub frame: DecodedFrame,
    pub width: u32,
    pub height: u32,
    pub stride_bytes: usize,
    pub pixels: Vec<u8>,
}

''',
)

insert_before(
    engine_path,
    "    /// Decode the next display frame and copy its full-resolution pixels into owned RGBA storage.\n",
    r'''    /// Materialize a bounded disposable preview for the exact current presentation frame.
    ///
    /// Exact frame identity is checked before pixel conversion. The native scaler converts the
    /// verified AVFrame directly to the requested bounded size, so large sources do not first
    /// allocate a full-resolution RGBA intermediate. Small frames are never upscaled.
    pub fn snapshot_current_frame_preview_rgba(
        &mut self,
        frame: &DecodedFrame,
        max_edge: u32,
    ) -> Result<DecodedPreviewRgbaFrame, FrameScopeError> {
        if self.cancellation.is_cancelled() {
            return Err(FrameScopeError::Cancelled);
        }
        if self.current_frame.as_ref() != Some(frame) {
            return Err(FrameScopeError::DecoderFailure(
                "preview RGBA snapshot request does not match the decoder's current frame".into(),
            ));
        }
        let (target_width, target_height) =
            crate::scrub_preview::preview_dimensions(frame.width, frame.height, max_edge).map_err(
                |error| {
                    FrameScopeError::DecoderFailure(format!(
                        "invalid bounded preview dimensions: {error}"
                    ))
                },
            )?;
        let rgba = if target_width == frame.width && target_height == frame.height {
            self.session.copy_current_frame_rgba()
        } else {
            self.session
                .copy_current_frame_rgba_resized(target_width, target_height)
        }
        .map_err(map_native_error)?;
        if rgba.width != target_width || rgba.height != target_height {
            return Err(FrameScopeError::DecoderFailure(format!(
                "bounded RGBA snapshot dimensions {}x{} do not match requested {}x{}",
                rgba.width, rgba.height, target_width, target_height
            )));
        }
        let expected = rgba
            .stride
            .checked_mul(usize::try_from(target_height).map_err(|_| {
                FrameScopeError::DecoderFailure("bounded preview height exceeds memory size".into())
            })?)
            .ok_or_else(|| {
                FrameScopeError::DecoderFailure(
                    "bounded preview RGBA byte size overflows memory".into(),
                )
            })?;
        if rgba.pixels.len() != expected {
            return Err(FrameScopeError::DecoderFailure(format!(
                "bounded RGBA snapshot has {} bytes, expected {expected}",
                rgba.pixels.len()
            )));
        }

        Ok(DecodedPreviewRgbaFrame {
            frame: frame.clone(),
            width: rgba.width,
            height: rgba.height,
            stride_bytes: rgba.stride,
            pixels: rgba.pixels,
        })
    }

''',
)

# Public exports.
lib_path = "rust/crates/framescope-video/src/lib.rs"
replace_text(
    lib_path,
    '''pub use engine::{
    CancellationToken, ObservedFrameRateMode, OpenOptions, VideoDecoder, VideoStreamSelection,
};
''',
    '''pub use engine::{
    CancellationToken, DecodedPreviewRgbaFrame, ObservedFrameRateMode, OpenOptions, VideoDecoder,
    VideoStreamSelection,
};
''',
)
replace_text(
    lib_path,
    '''pub use target_rgba_navigation::{
    TargetRgbaNavigationCursor, TargetRgbaNavigationDecoder, navigate_to_frame_cached_target_only,
    navigate_to_frame_cached_target_only_with_cursor,
};
''',
    '''pub use target_rgba_navigation::{
    TargetPreviewNavigationError, TargetPreviewNavigationResult, TargetRgbaNavigationCursor,
    TargetRgbaNavigationDecoder, navigate_to_frame_bounded_preview_with_cursor,
    navigate_to_frame_cached_target_only, navigate_to_frame_cached_target_only_with_cursor,
};
''',
)

# Target navigation: add a bounded result/error and separate path. Existing full-quality
# navigation remains unchanged.
target_path = "rust/crates/framescope-video/src/target_rgba_navigation.rs"
replace_text(
    target_path,
    '''use crate::{
    CachedFrameSource, CachedNavigationError, CachedNavigationResult, CancellationToken,
    VideoDecoder, ffmpeg::DecodedRgbaFrame,
};
''',
    '''use crate::{
    CachedFrameSource, CachedNavigationError, CachedNavigationResult, CancellationToken,
    DecodedPreviewRgbaFrame, ScrubPreviewError, VideoDecoder, downscale_scrub_preview,
    ffmpeg::DecodedRgbaFrame,
};
''',
)
insert_before(target_path, "/// Decoder contract for indexed navigation", "use thiserror::Error;\n\n")

replace_text(
    target_path,
    '''    fn snapshot_current_rgba_for_target_navigation(
        &mut self,
        frame: &DecodedFrame,
    ) -> Result<DecodedRgbaFrame, FrameScopeError>;
    fn cancellation_token_for_target_navigation(&self) -> Option<CancellationToken> {
''',
    '''    fn snapshot_current_rgba_for_target_navigation(
        &mut self,
        frame: &DecodedFrame,
    ) -> Result<DecodedRgbaFrame, FrameScopeError>;
    fn snapshot_current_preview_rgba_for_target_navigation(
        &mut self,
        frame: &DecodedFrame,
        max_edge: u32,
    ) -> Result<DecodedPreviewRgbaFrame, FrameScopeError> {
        let DecodedRgbaFrame {
            frame,
            stride_bytes,
            pixels,
        } = self.snapshot_current_rgba_for_target_navigation(frame)?;
        let source = OwnedRgbaFrame::new(frame.width, frame.height, stride_bytes, pixels).map_err(
            |error| {
                FrameScopeError::DecoderFailure(format!(
                    "fallback preview RGBA validation failed: {error}"
                ))
            },
        )?;
        let preview = downscale_scrub_preview(&source, max_edge).map_err(|error| {
            FrameScopeError::DecoderFailure(format!("fallback preview scaling failed: {error}"))
        })?;
        Ok(DecodedPreviewRgbaFrame {
            frame,
            width: preview.width,
            height: preview.height,
            stride_bytes: preview.stride_bytes,
            pixels: preview.pixels().to_vec(),
        })
    }
    fn cancellation_token_for_target_navigation(&self) -> Option<CancellationToken> {
''',
)

replace_text(
    target_path,
    '''    fn snapshot_current_rgba_for_target_navigation(
        &mut self,
        frame: &DecodedFrame,
    ) -> Result<DecodedRgbaFrame, FrameScopeError> {
        self.snapshot_current_frame_rgba(frame)
    }

    fn cancellation_token_for_target_navigation(&self) -> Option<CancellationToken> {
''',
    '''    fn snapshot_current_rgba_for_target_navigation(
        &mut self,
        frame: &DecodedFrame,
    ) -> Result<DecodedRgbaFrame, FrameScopeError> {
        self.snapshot_current_frame_rgba(frame)
    }

    fn snapshot_current_preview_rgba_for_target_navigation(
        &mut self,
        frame: &DecodedFrame,
        max_edge: u32,
    ) -> Result<DecodedPreviewRgbaFrame, FrameScopeError> {
        self.snapshot_current_frame_preview_rgba(frame, max_edge)
    }

    fn cancellation_token_for_target_navigation(&self) -> Option<CancellationToken> {
''',
)

insert_before(
    target_path,
    "/// Source-quality indexed navigation optimized for preview misses.\n",
    r'''#[derive(Debug, Error)]
pub enum TargetPreviewNavigationError {
    #[error("bounded target navigation failed: {0}")]
    Navigation(#[from] CachedNavigationError),
    #[error("bounded target preview scaling failed: {0}")]
    Preview(#[from] ScrubPreviewError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetPreviewNavigationResult {
    pub frame_id: FrameId,
    pub index_entry: FrameIndexEntry,
    pub pixels: OwnedRgbaFrame,
    pub source: CachedFrameSource,
    pub decoded_frames: u64,
    pub used_keyframe_seek: bool,
    pub fell_back_to_stream_start: bool,
}

#[derive(Debug)]
struct TargetPreviewDecodedResult {
    frame: DecodedPreviewRgbaFrame,
    decoded_frames: u64,
    used_keyframe_seek: bool,
    fell_back_to_stream_start: bool,
}

''',
)

insert_before(
    target_path,
    "fn navigate_target_rgba_retained<D, F>(\n",
    r'''/// Decode an exact indexed target directly into bounded disposable preview pixels.
///
/// A source-quality RAM hit may be downscaled because those pixels are already resident. On a miss,
/// only metadata is decoded until the exact indexed target is reconciled; the verified AVFrame is
/// then converted directly to the requested bounded size. Bounded pixels are never inserted into
/// the source-quality cache.
pub fn navigate_to_frame_bounded_preview_with_cursor<D, F>(
    index: &FrameIndex,
    cache: &mut FrameCacheHierarchy,
    cursor: &mut Option<TargetRgbaNavigationCursor<D>>,
    mut open_fresh_decoder: F,
    frame_id: FrameId,
    max_forward_frames: u64,
    max_edge: u32,
) -> Result<TargetPreviewNavigationResult, TargetPreviewNavigationError>
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
        Err(error) => return Err(CachedNavigationError::from(error).into()),
    };

    let can_continue = cursor
        .as_ref()
        .is_some_and(|active| active.can_continue_to(frame_id, max_forward_frames));
    if cursor.is_some() && !can_continue {
        *cursor = None;
    }

    if let Some(key) = key.as_ref() {
        if let Some(cached) = cache.lookup_full(key) {
            let pixels = downscale_scrub_preview(&cached.pixels, max_edge)?;
            return Ok(TargetPreviewNavigationResult {
                frame_id,
                index_entry,
                pixels,
                source: CachedFrameSource::Ram,
                decoded_frames: 0,
                used_keyframe_seek: false,
                fell_back_to_stream_start: false,
            });
        }
    }

    let decoded = if can_continue {
        let continuation = {
            let active = cursor
                .as_mut()
                .expect("cursor availability was checked before continuation");
            decode_preview_forward_from_cursor(
                index,
                &mut active.decoder,
                active.frame_id,
                frame_id,
                max_edge,
            )
        };
        match continuation {
            Ok((frame, decoded_frames)) => {
                cursor
                    .as_mut()
                    .expect("successful continuation keeps the cursor")
                    .frame_id = frame_id;
                TargetPreviewDecodedResult {
                    frame,
                    decoded_frames,
                    used_keyframe_seek: false,
                    fell_back_to_stream_start: false,
                }
            }
            Err(CachedNavigationError::TimelineMismatch | CachedNavigationError::UnexpectedEof) => {
                *cursor = None;
                let (decoded, decoder) = navigate_target_preview_retained(
                    index,
                    &mut open_fresh_decoder,
                    frame_id,
                    max_edge,
                )?;
                *cursor = Some(TargetRgbaNavigationCursor { decoder, frame_id });
                decoded
            }
            Err(error) => {
                *cursor = None;
                return Err(error.into());
            }
        }
    } else {
        let (decoded, decoder) =
            navigate_target_preview_retained(index, &mut open_fresh_decoder, frame_id, max_edge)?;
        *cursor = Some(TargetRgbaNavigationCursor { decoder, frame_id });
        decoded
    };

    let pixels = OwnedRgbaFrame::new(
        decoded.frame.width,
        decoded.frame.height,
        decoded.frame.stride_bytes,
        decoded.frame.pixels,
    )
    .map_err(CachedNavigationError::from)?;

    Ok(TargetPreviewNavigationResult {
        frame_id,
        index_entry,
        pixels,
        source: CachedFrameSource::Decoded,
        decoded_frames: decoded.decoded_frames,
        used_keyframe_seek: decoded.used_keyframe_seek,
        fell_back_to_stream_start: decoded.fell_back_to_stream_start,
    })
}

fn navigate_target_preview_retained<D, F>(
    index: &FrameIndex,
    open_fresh_decoder: &mut F,
    frame_id: FrameId,
    max_edge: u32,
) -> Result<(TargetPreviewDecodedResult, D), CachedNavigationError>
where
    D: TargetRgbaNavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
{
    ensure_complete(index)?;
    let target = index
        .entry(frame_id)?
        .ok_or(CachedNavigationError::FrameNotIndexed)?;
    let timestamp_seek_safe = index.timestamp_seek_safety()?.permits_timestamp_seek();
    let mut decoder = open_checked_decoder(index, open_fresh_decoder)?;

    if timestamp_seek_safe {
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
                match decode_preview_from_seek(
                    index,
                    &mut decoder,
                    anchor_id,
                    frame_id,
                    max_edge,
                ) {
                    Ok((frame, decoded_frames)) => {
                        return Ok((
                            TargetPreviewDecodedResult {
                                frame,
                                decoded_frames,
                                used_keyframe_seek: true,
                                fell_back_to_stream_start: false,
                            },
                            decoder,
                        ));
                    }
                    Err(
                        CachedNavigationError::TimelineMismatch
                        | CachedNavigationError::UnexpectedEof,
                    ) => {}
                    Err(error) => return Err(error),
                }

                let mut fallback = open_checked_decoder(index, open_fresh_decoder)?;
                let (frame, decoded_frames) =
                    decode_preview_from_start(index, &mut fallback, frame_id, max_edge)?;
                return Ok((
                    TargetPreviewDecodedResult {
                        frame,
                        decoded_frames,
                        used_keyframe_seek: true,
                        fell_back_to_stream_start: true,
                    },
                    fallback,
                ));
            }
        }
    }

    let (frame, decoded_frames) =
        decode_preview_from_start(index, &mut decoder, frame_id, max_edge)?;
    Ok((
        TargetPreviewDecodedResult {
            frame,
            decoded_frames,
            used_keyframe_seek: false,
            fell_back_to_stream_start: !timestamp_seek_safe,
        },
        decoder,
    ))
}

fn decode_preview_from_start<D: TargetRgbaNavigationDecoder>(
    index: &FrameIndex,
    decoder: &mut D,
    target: FrameId,
    max_edge: u32,
) -> Result<(DecodedPreviewRgbaFrame, u64), CachedNavigationError> {
    let mut current = FrameId::ZERO;
    let mut decoded_frames = 0_u64;
    loop {
        let decoded = next_metadata(decoder, &mut decoded_frames)?;
        verify_decoded(index, current, &decoded)?;
        if current == target {
            let rgba =
                decoder.snapshot_current_preview_rgba_for_target_navigation(&decoded, max_edge)?;
            return Ok((rgba, decoded_frames));
        }
        current = next_frame_id(current)?;
    }
}

fn decode_preview_from_seek<D: TargetRgbaNavigationDecoder>(
    index: &FrameIndex,
    decoder: &mut D,
    anchor: FrameId,
    target: FrameId,
    max_edge: u32,
) -> Result<(DecodedPreviewRgbaFrame, u64), CachedNavigationError> {
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
            let rgba =
                decoder.snapshot_current_preview_rgba_for_target_navigation(&decoded, max_edge)?;
            return Ok((rgba, decoded_frames));
        }
        current = next_frame_id(current)?;
        decoded = next_metadata(decoder, &mut decoded_frames)?;
    }
}

fn decode_preview_forward_from_cursor<D: TargetRgbaNavigationDecoder>(
    index: &FrameIndex,
    decoder: &mut D,
    current: FrameId,
    target: FrameId,
    max_edge: u32,
) -> Result<(DecodedPreviewRgbaFrame, u64), CachedNavigationError> {
    if target.0 <= current.0 {
        return Err(CachedNavigationError::TimelineMismatch);
    }
    let mut expected = next_frame_id(current)?;
    let mut decoded_frames = 0_u64;
    loop {
        let decoded = next_metadata(decoder, &mut decoded_frames)?;
        verify_decoded(index, expected, &decoded)?;
        if expected == target {
            let rgba =
                decoder.snapshot_current_preview_rgba_for_target_navigation(&decoded, max_edge)?;
            return Ok((rgba, decoded_frames));
        }
        expected = next_frame_id(expected)?;
    }
}

''',
)

# Regression: bounded preview bytes must never populate the full-quality cache key.
replace_text(
    target_path,
    '''        drop(index);
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_dir_all(cache_root);
    }
}
''',
    '''        drop(index);
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_dir_all(cache_root);
    }

    #[test]
    fn bounded_preview_miss_never_populates_full_quality_cache() {
        let (index_path, index) = complete_index();
        let cache_root = temp_path("bounded-preview-cache");
        let mut cache = FrameCacheHierarchy::open(&cache_root, 1024, 0).unwrap();
        let decoded = Arc::new(AtomicU64::new(0));
        let materialized = Arc::new(AtomicU64::new(0));
        let mut cursor = None;

        let preview = navigate_to_frame_bounded_preview_with_cursor(
            &index,
            &mut cache,
            &mut cursor,
            || Ok(fake_decoder(decoded.clone(), materialized.clone())),
            FrameId(4),
            2,
            1,
        )
        .unwrap();

        assert_eq!(preview.source, CachedFrameSource::Decoded);
        assert_eq!((preview.pixels.width, preview.pixels.height), (1, 1));
        assert_eq!(preview.pixels.pixels(), &[4; 4]);

        let full = navigate_to_frame_cached_target_only(
            &index,
            &mut cache,
            || Ok(fake_decoder(decoded.clone(), materialized.clone())),
            FrameId(4),
        )
        .unwrap();
        assert_eq!(
            full.source,
            CachedFrameSource::Decoded,
            "bounded preview bytes must never satisfy the source-quality cache key"
        );
        assert_eq!(full.pixels.pixels(), &[4; 16]);

        drop(index);
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_dir_all(cache_root);
    }
}
''',
)

# FFI render critical path: use bounded navigation on misses. Background prefetch remains
# on the old full-quality path so existing RAM-acceleration warming semantics are unchanged.
ffi_path = "rust/crates/framescope-ffi/src/scrub_handoff.rs"
replace_text(
    ffi_path,
    '''use framescope_video::{
    CachedFrameSource, CachedNavigationError, CancellationToken, MicroscopeTimestampSelection,
    OpenOptions, ScrubPreviewCache, TargetRgbaNavigationCursor, VideoDecoder,
    downscale_scrub_preview, microscope_target, microscope_timestamp_us,
    navigate_to_frame_cached_target_only_with_cursor,
};
''',
    '''use framescope_video::{
    CachedFrameSource, CachedNavigationError, CancellationToken, MicroscopeTimestampSelection,
    OpenOptions, ScrubPreviewCache, TargetPreviewNavigationError, TargetRgbaNavigationCursor,
    VideoDecoder, downscale_scrub_preview, microscope_target, microscope_timestamp_us,
    navigate_to_frame_bounded_preview_with_cursor,
    navigate_to_frame_cached_target_only_with_cursor,
};
''',
)

# Only replace the first full-quality navigation call; the second call belongs to background prefetch.
text = read(ffi_path)
old_nav = '''        navigate_to_frame_cached_target_only_with_cursor(
            index,
            source_cache,
            decoder_cursor,
            || open_decoder(source_fd, decoder_cancellation.clone()),
            frame_id,
            MAX_FORWARD_CURSOR_REUSE_FRAMES,
        )
        .map_err(from_navigation)
'''
if text.count(old_nav) != 2:
    raise SystemExit(f"{ffi_path}: expected two full-quality navigation call sites")
new_nav = '''        navigate_to_frame_bounded_preview_with_cursor(
            index,
            source_cache,
            decoder_cursor,
            || open_decoder(source_fd, decoder_cancellation.clone()),
            frame_id,
            MAX_FORWARD_CURSOR_REUSE_FRAMES,
            max_edge,
        )
        .map_err(from_preview_navigation)
'''
write(ffi_path, text.replace(old_nav, new_nav, 1))

replace_text(
    ffi_path,
    '''    let decoded_frames = navigated.decoded_frames;
    let preview = downscale_scrub_preview(&navigated.pixels, max_edge)
        .map_err(|error| PreviewFailure::new("preview_scale_error", error.to_string()))?;
    ensure_not_cancelled(&cancellation)?;
''',
    '''    let decoded_frames = navigated.decoded_frames;
    let preview = navigated.pixels;
    ensure_not_cancelled(&cancellation)?;
''',
)

insert_before(
    ffi_path,
    "fn serialize_result(result: Result<PreviewDetails, PreviewFailure>) -> String {\n",
    r'''fn from_preview_navigation(error: TargetPreviewNavigationError) -> PreviewFailure {
    match error {
        TargetPreviewNavigationError::Navigation(error) => from_navigation(error),
        TargetPreviewNavigationError::Preview(error) => {
            PreviewFailure::new("preview_scale_error", error.to_string())
        }
    }
}

''',
)

# Real-FFmpeg regressions for direct bounded conversion and scaler switching.
rgba_test = "rust/crates/framescope-video/tests/rgba_fixtures.rs"
insert_before(
    rgba_test,
    "#[test]\nfn video_decoder_exposes_source_quality_owned_rgba_with_pts_metadata()",
    r'''#[test]
fn bounded_rgba_snapshot_avoids_source_sized_output_and_preserves_full_copy() {
    let mut decoder =
        VideoDecoder::open_path(fixture("h264-cfr.mp4")).expect("fixture should open");
    let frame = decoder
        .next_frame()
        .expect("decode should succeed")
        .expect("fixture should contain a frame");
    assert_eq!((frame.width, frame.height), (64, 48));

    let preview = decoder
        .snapshot_current_frame_preview_rgba(&frame, 32)
        .expect("bounded preview conversion should succeed");
    assert_eq!((preview.frame.width, preview.frame.height), (64, 48));
    assert_eq!((preview.width, preview.height), (32, 24));
    assert_eq!(preview.stride_bytes, 32 * 4);
    assert_eq!(preview.pixels.len(), 32 * 24 * 4);

    let full = decoder
        .snapshot_current_frame_rgba(&frame)
        .expect("full RGBA conversion must still work after bounded conversion");
    assert_eq!((full.frame.width, full.frame.height), (64, 48));
    assert_eq!(full.stride_bytes, 64 * 4);
    assert_eq!(full.pixels.len(), 64 * 48 * 4);
}

''',
)

insert_before(
    rgba_test,
    "#[test]\nfn repeated_rgba_copy_of_same_frame_is_pixel_identical()",
    r'''#[test]
fn native_resized_copy_is_bounded_and_rejects_upscale() {
    let mut session =
        Session::open_path(&fixture("h264-cfr.mp4"), None, CancellationToken::new())
            .expect("fixture should open");
    session
        .next_frame()
        .expect("decode should succeed")
        .expect("fixture should contain a frame");

    let preview = session
        .copy_current_frame_rgba_resized(32, 24)
        .expect("native bounded conversion should succeed");
    assert_eq!((preview.width, preview.height), (32, 24));
    assert_eq!(preview.stride, 32 * 4);
    assert_eq!(preview.pixels.len(), 32 * 24 * 4);
    assert!(session.copy_current_frame_rgba_resized(65, 48).is_err());
}

''',
)
