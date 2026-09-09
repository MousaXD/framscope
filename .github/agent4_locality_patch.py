from pathlib import Path

p = Path("rust/crates/framescope-video/src/target_rgba_navigation.rs")
text = p.read_text()
text = text.replace(
    "CachedFrameSource, CachedNavigationError, CachedNavigationResult, VideoDecoder,\n",
    "CachedFrameSource, CachedNavigationError, CachedNavigationResult, CancellationToken, VideoDecoder,\n",
    1,
)
old = """    fn snapshot_current_rgba_for_target_navigation(
        &mut self,
        frame: &DecodedFrame,
    ) -> Result<DecodedRgbaFrame, FrameScopeError>;
    fn seek_for_target_navigation(&mut self, timestamp_us: i64) -> Result<(), FrameScopeError>;
}"""
new = """    fn snapshot_current_rgba_for_target_navigation(
        &mut self,
        frame: &DecodedFrame,
    ) -> Result<DecodedRgbaFrame, FrameScopeError>;
    fn cancellation_token_for_target_navigation(&self) -> Option<CancellationToken> {
        None
    }
    fn seek_for_target_navigation(&mut self, timestamp_us: i64) -> Result<(), FrameScopeError>;
}"""
if text.count(old) != 1:
    raise SystemExit("target navigation trait shape changed")
text = text.replace(old, new, 1)
old = """    fn seek_for_target_navigation(&mut self, timestamp_us: i64) -> Result<(), FrameScopeError> {
        self.seek_to_timestamp_us(timestamp_us)
    }
}

#[derive(Debug)]
struct TargetNavigationResult"""
new = """    fn cancellation_token_for_target_navigation(&self) -> Option<CancellationToken> {
        Some(self.cancellation_token())
    }

    fn seek_for_target_navigation(&mut self, timestamp_us: i64) -> Result<(), FrameScopeError> {
        self.seek_to_timestamp_us(timestamp_us)
    }
}

pub struct TargetRgbaNavigationCursor<D> {
    decoder: D,
    frame_id: FrameId,
}

impl<D> std::fmt::Debug for TargetRgbaNavigationCursor<D> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TargetRgbaNavigationCursor")
            .field("frame_id", &self.frame_id)
            .finish_non_exhaustive()
    }
}

impl<D: TargetRgbaNavigationDecoder> TargetRgbaNavigationCursor<D> {
    pub fn frame_id(&self) -> FrameId {
        self.frame_id
    }

    pub fn can_continue_to(&self, target: FrameId, max_forward_frames: u64) -> bool {
        target
            .0
            .checked_sub(self.frame_id.0)
            .is_some_and(|delta| delta > 0 && delta <= max_forward_frames)
    }

    pub fn cancellation_token(&self) -> Option<CancellationToken> {
        self.decoder.cancellation_token_for_target_navigation()
    }
}

#[derive(Debug)]
struct TargetNavigationResult"""
if text.count(old) != 1:
    raise SystemExit("VideoDecoder target navigation impl shape changed")
text = text.replace(old, new, 1)

start = text.index("pub fn navigate_to_frame_cached_target_only")
end = text.index("fn ensure_complete", start)
replacement = '''pub fn navigate_to_frame_cached_target_only<D, F>(
    index: &FrameIndex,
    cache: &mut FrameCacheHierarchy,
    mut open_fresh_decoder: F,
    frame_id: FrameId,
) -> Result<CachedNavigationResult, CachedNavigationError>
where
    D: TargetRgbaNavigationDecoder,
    F: FnMut() -> Result<D, FrameScopeError>,
{
    let mut cursor = None;
    navigate_to_frame_cached_target_only_with_cursor(
        index,
        cache,
        &mut cursor,
        &mut open_fresh_decoder,
        frame_id,
        0,
    )
}

/// Target-only RGBA navigation with deterministic nearby-forward decoder locality.
///
/// A verified cursor may only continue to a strictly later FrameId within `max_forward_frames`.
/// Reversal, same-frame requests, and larger jumps discard the decoder before any source work. A
/// cursor failure also discards it; timeline/EOF failures retry through the normal fresh-decoder
/// path, while decoder/cancellation failures are returned to the caller. Every frame crossed by a
/// reused cursor is reconciled against the authoritative persistent index before the target pixels
/// are materialized.
pub fn navigate_to_frame_cached_target_only_with_cursor<D, F>(
    index: &FrameIndex,
    cache: &mut FrameCacheHierarchy,
    cursor: &mut Option<TargetRgbaNavigationCursor<D>>,
    mut open_fresh_decoder: F,
    frame_id: FrameId,
    max_forward_frames: u64,
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

    let can_continue = cursor
        .as_ref()
        .is_some_and(|active| active.can_continue_to(frame_id, max_forward_frames));
    if cursor.is_some() && !can_continue {
        *cursor = None;
    }

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

    let decoded = if can_continue {
        let continuation = {
            let active = cursor
                .as_mut()
                .expect("cursor availability was checked before continuation");
            decode_forward_from_cursor(index, &mut active.decoder, active.frame_id, frame_id)
        };
        match continuation {
            Ok((frame, decoded_frames)) => {
                cursor
                    .as_mut()
                    .expect("successful continuation keeps the cursor")
                    .frame_id = frame_id;
                TargetNavigationResult {
                    frame,
                    decoded_frames,
                    used_keyframe_seek: false,
                    fell_back_to_stream_start: false,
                }
            }
            Err(CachedNavigationError::TimelineMismatch | CachedNavigationError::UnexpectedEof) => {
                *cursor = None;
                let (decoded, decoder) =
                    navigate_target_rgba_retained(index, &mut open_fresh_decoder, frame_id)?;
                *cursor = Some(TargetRgbaNavigationCursor { decoder, frame_id });
                decoded
            }
            Err(error) => {
                *cursor = None;
                return Err(error);
            }
        }
    } else {
        let (decoded, decoder) =
            navigate_target_rgba_retained(index, &mut open_fresh_decoder, frame_id)?;
        *cursor = Some(TargetRgbaNavigationCursor { decoder, frame_id });
        decoded
    };

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

fn navigate_target_rgba_retained<D, F>(
    index: &FrameIndex,
    open_fresh_decoder: &mut F,
    frame_id: FrameId,
) -> Result<(TargetNavigationResult, D), CachedNavigationError>
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
                    return Ok((
                        TargetNavigationResult {
                            frame,
                            decoded_frames,
                            used_keyframe_seek: true,
                            fell_back_to_stream_start: false,
                        },
                        decoder,
                    ));
                }
                Err(
                    CachedNavigationError::TimelineMismatch | CachedNavigationError::UnexpectedEof,
                ) => {}
                Err(error) => return Err(error),
            }

            let mut fallback = open_checked_decoder(index, open_fresh_decoder)?;
            let (frame, decoded_frames) = decode_from_start(index, &mut fallback, frame_id)?;
            return Ok((
                TargetNavigationResult {
                    frame,
                    decoded_frames,
                    used_keyframe_seek: true,
                    fell_back_to_stream_start: true,
                },
                fallback,
            ));
        }
    }

    let (frame, decoded_frames) = decode_from_start(index, &mut decoder, frame_id)?;
    Ok((
        TargetNavigationResult {
            frame,
            decoded_frames,
            used_keyframe_seek: false,
            fell_back_to_stream_start: false,
        },
        decoder,
    ))
}

'''
text = text[:start] + replacement + text[end:]

marker = "fn verify_decoded(\n"
at = text.index(marker)
forward = '''fn decode_forward_from_cursor<D: TargetRgbaNavigationDecoder>(
    index: &FrameIndex,
    decoder: &mut D,
    current: FrameId,
    target: FrameId,
) -> Result<(DecodedRgbaFrame, u64), CachedNavigationError> {
    if target.0 <= current.0 {
        return Err(CachedNavigationError::TimelineMismatch);
    }
    let mut expected = next_frame_id(current)?;
    let mut decoded_frames = 0_u64;
    loop {
        let decoded = next_metadata(decoder, &mut decoded_frames)?;
        verify_decoded(index, expected, &decoded)?;
        if expected == target {
            let rgba = decoder.snapshot_current_rgba_for_target_navigation(&decoded)?;
            return Ok((rgba, decoded_frames));
        }
        expected = next_frame_id(expected)?;
    }
}

'''
text = text[:at] + forward + text[at:]

test = '''
    #[test]
    fn cursor_reuses_nearby_forward_decoder_and_resets_on_reversal_or_large_jump() {
        let (index_path, index) = complete_index();
        let cache_root = temp_path("cursor-cache");
        let mut cache = FrameCacheHierarchy::open(&cache_root, 1024, 0).unwrap();
        let decoded = Arc::new(AtomicU64::new(0));
        let materialized = Arc::new(AtomicU64::new(0));
        let opens = Arc::new(AtomicU64::new(0));
        let mut cursor = None;

        let first = navigate_to_frame_cached_target_only_with_cursor(
            &index,
            &mut cache,
            &mut cursor,
            || {
                opens.fetch_add(1, Ordering::Relaxed);
                Ok(fake_decoder(decoded.clone(), materialized.clone()))
            },
            FrameId(3),
            2,
        )
        .unwrap();
        assert_eq!(first.source, CachedFrameSource::Decoded);
        assert_eq!(
            cursor.as_ref().map(TargetRgbaNavigationCursor::frame_id),
            Some(FrameId(3)),
        );
        let opens_after_first = opens.load(Ordering::Relaxed);
        assert!(opens_after_first >= 1);

        let second = navigate_to_frame_cached_target_only_with_cursor(
            &index,
            &mut cache,
            &mut cursor,
            || {
                opens.fetch_add(1, Ordering::Relaxed);
                Ok(fake_decoder(decoded.clone(), materialized.clone()))
            },
            FrameId(4),
            2,
        )
        .unwrap();
        assert_eq!(second.source, CachedFrameSource::Decoded);
        assert_eq!(second.decoded_frames, 1);
        assert_eq!(opens.load(Ordering::Relaxed), opens_after_first);
        assert_eq!(
            cursor.as_ref().map(TargetRgbaNavigationCursor::frame_id),
            Some(FrameId(4)),
        );

        let reversal = navigate_to_frame_cached_target_only_with_cursor(
            &index,
            &mut cache,
            &mut cursor,
            || {
                opens.fetch_add(1, Ordering::Relaxed);
                Ok(fake_decoder(decoded.clone(), materialized.clone()))
            },
            FrameId(1),
            2,
        )
        .unwrap();
        assert_eq!(reversal.source, CachedFrameSource::Decoded);
        assert!(opens.load(Ordering::Relaxed) > opens_after_first);
        assert_eq!(
            cursor.as_ref().map(TargetRgbaNavigationCursor::frame_id),
            Some(FrameId(1)),
        );

        let opens_after_reversal = opens.load(Ordering::Relaxed);
        let large_jump_to_cached = navigate_to_frame_cached_target_only_with_cursor(
            &index,
            &mut cache,
            &mut cursor,
            || {
                opens.fetch_add(1, Ordering::Relaxed);
                Ok(fake_decoder(decoded.clone(), materialized.clone()))
            },
            FrameId(4),
            1,
        )
        .unwrap();
        assert_eq!(large_jump_to_cached.source, CachedFrameSource::Ram);
        assert!(cursor.is_none());
        assert_eq!(opens.load(Ordering::Relaxed), opens_after_reversal);

        drop(index);
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_dir_all(cache_root);
    }
'''
last = text.rfind("\n}")
if last < 0:
    raise SystemExit("target navigation tests module end not found")
text = text[:last] + test + text[last:]
p.write_text(text)

p = Path("rust/crates/framescope-video/src/lib.rs")
text = p.read_text()
old = """pub use target_rgba_navigation::{
    TargetRgbaNavigationDecoder, navigate_to_frame_cached_target_only,
};"""
new = """pub use target_rgba_navigation::{
    TargetRgbaNavigationCursor, TargetRgbaNavigationDecoder,
    navigate_to_frame_cached_target_only, navigate_to_frame_cached_target_only_with_cursor,
};"""
if text.count(old) != 1:
    raise SystemExit("framescope-video target export shape changed")
p.write_text(text.replace(old, new, 1))

p = Path("rust/crates/framescope-ffi/src/scrub_handoff.rs")
text = p.read_text()
old = """    CachedFrameSource, CachedNavigationError, CancellationToken, MicroscopeTimestampSelection,
    OpenOptions, ScrubPreviewCache, VideoDecoder, downscale_scrub_preview, microscope_target,
    microscope_timestamp_us, navigate_to_frame_cached_target_only,
};"""
new = """    CachedFrameSource, CachedNavigationError, CancellationToken, MicroscopeTimestampSelection,
    OpenOptions, ScrubPreviewCache, TargetRgbaNavigationCursor, VideoDecoder,
    downscale_scrub_preview, microscope_target, microscope_timestamp_us,
    navigate_to_frame_cached_target_only_with_cursor,
};"""
if text.count(old) != 1:
    raise SystemExit("scrub_handoff imports changed")
text = text.replace(old, new, 1)
text = text.replace(
    "const MAX_CONFIGURED_RAM_BUDGET_BYTES: usize = 512 * 1024 * 1024;\n",
    "const MAX_CONFIGURED_RAM_BUDGET_BYTES: usize = 512 * 1024 * 1024;\nconst MAX_FORWARD_CURSOR_REUSE_FRAMES: u64 = 48;\n",
    1,
)
old = """struct ScrubSessionState {
    cache_root: PathBuf,
    source_cache: FrameCacheHierarchy,
    preview_cache: ScrubPreviewCache,
}"""
new = """struct ScrubSessionState {
    cache_root: PathBuf,
    source_cache: FrameCacheHierarchy,
    preview_cache: ScrubPreviewCache,
    decoder_cursor: Option<TargetRgbaNavigationCursor<VideoDecoder>>,
}"""
if text.count(old) != 1:
    raise SystemExit("scrub session state shape changed")
text = text.replace(old, new, 1)

old = """fn begin_preview_operation(
    session_id: i64,
) -> Result<(CancellationToken, PreviewOperationGuard), PreviewFailure> {
    let mut registry = preview_cancellation_registry().lock().map_err(|_| {"""
new = """fn begin_preview_operation(
    session_id: i64,
) -> Result<(CancellationToken, PreviewOperationGuard), PreviewFailure> {
    begin_preview_operation_with_token(session_id, CancellationToken::new())
}

fn begin_preview_operation_with_token(
    session_id: i64,
    cancellation: CancellationToken,
) -> Result<(CancellationToken, PreviewOperationGuard), PreviewFailure> {
    if cancellation.is_cancelled() {
        return Err(PreviewFailure::new(
            "cancelled",
            "Live preview decoder cursor was already cancelled.",
        ));
    }
    let mut registry = preview_cancellation_registry().lock().map_err(|_| {"""
if text.count(old) != 1:
    raise SystemExit("begin_preview_operation shape changed")
text = text.replace(old, new, 1)
old = """    let generation = registry.next_generation;
    let cancellation = CancellationToken::new();
    registry.active.insert("""
new = """    let generation = registry.next_generation;
    registry.active.insert("""
if text.count(old) != 1:
    raise SystemExit("preview cancellation creation shape changed")
text = text.replace(old, new, 1)

insert_after = """fn preview_cache_budget_bytes() -> usize {
    PREVIEW_CACHE_BUDGET_BYTES.load(Ordering::Acquire)
}

"""
helper = '''fn cancellation_for_scrub_target(
    state: &mut ScrubSessionState,
    frame_id: FrameId,
) -> CancellationToken {
    let reusable = state.decoder_cursor.as_ref().and_then(|cursor| {
        cursor
            .can_continue_to(frame_id, MAX_FORWARD_CURSOR_REUSE_FRAMES)
            .then(|| cursor.cancellation_token())
            .flatten()
            .filter(|token| !token.is_cancelled())
    });
    if let Some(cancellation) = reusable {
        cancellation
    } else {
        state.decoder_cursor = None;
        CancellationToken::new()
    }
}

'''
if text.count(insert_after) != 1:
    raise SystemExit("preview budget helper marker changed")
text = text.replace(insert_after, insert_after + helper, 1)

old = """    let cache_root = validate_cache_root(cache_root)?;
    let (cancellation, _operation) = begin_preview_operation(session_id)?;
    ensure_not_cancelled(&cancellation)?;

    // Resolve the persistent target under the authoritative session lock, then release it before
"""
new = """    let cache_root = validate_cache_root(cache_root)?;

    // Resolve the persistent target under the authoritative session lock, then release it before
"""
if text.count(old) != 1:
    raise SystemExit("render_preview cancellation placement changed")
text = text.replace(old, new, 1)
old = """    let frame_id = target.frame_id();
    let timestamp_us = target.entry.timestamp_us();
    let state = session_state(session_id, &cache_root)?;

    let cached_preview = {"""
new = """    let frame_id = target.frame_id();
    let timestamp_us = target.entry.timestamp_us();
    let state = session_state(session_id, &cache_root)?;
    let cancellation = {
        let mut state = state.lock().map_err(|_| {
            PreviewFailure::new("bridge_error", "Live preview cache state is poisoned.")
        })?;
        cancellation_for_scrub_target(&mut state, frame_id)
    };
    let (cancellation, _operation) =
        begin_preview_operation_with_token(session_id, cancellation)?;
    ensure_not_cancelled(&cancellation)?;

    let cached_preview = {"""
if text.count(old) != 1:
    raise SystemExit("render_preview target/state shape changed")
text = text.replace(old, new, 1)
old = """        let decoder_cancellation = cancellation.clone();
        navigate_to_frame_cached_target_only(
            index,
            &mut state.source_cache,
            || open_decoder(source_fd, decoder_cancellation.clone()),
            frame_id,
        )
        .map_err(from_navigation)"""
new = """        let decoder_cancellation = cancellation.clone();
        let ScrubSessionState {
            source_cache,
            decoder_cursor,
            ..
        } = &mut *state;
        navigate_to_frame_cached_target_only_with_cursor(
            index,
            source_cache,
            decoder_cursor,
            || open_decoder(source_fd, decoder_cancellation.clone()),
            frame_id,
            MAX_FORWARD_CURSOR_REUSE_FRAMES,
        )
        .map_err(from_navigation)"""
if text.count(old) != 1:
    raise SystemExit("source navigation call shape changed")
text = text.replace(old, new, 1)

old = """        // Start at zero so adding a session can never transiently oversubscribe the process budget.
        preview_cache: ScrubPreviewCache::new(0, PREVIEW_CACHE_MAX_FRAMES),
    }));"""
new = """        // Start at zero so adding a session can never transiently oversubscribe the process budget.
        preview_cache: ScrubPreviewCache::new(0, PREVIEW_CACHE_MAX_FRAMES),
        decoder_cursor: None,
    }));"""
if text.count(old) != 1:
    raise SystemExit("session state initialization shape changed")
text = text.replace(old, new, 1)

old = """fn trim_removed_preview_state(handle: ScrubSessionHandle) {
    lock_scrub_state(&handle.state)
        .preview_cache
        .set_budget_bytes(0);
}"""
new = """fn trim_removed_preview_state(handle: ScrubSessionHandle) {
    let mut state = lock_scrub_state(&handle.state);
    state.preview_cache.set_budget_bytes(0);
    state.decoder_cursor = None;
}"""
if text.count(old) != 1:
    raise SystemExit("trim_removed_preview_state shape changed")
text = text.replace(old, new, 1)

old = """        if state.cache_root == cache_root {
            state.source_cache.set_ram_budget_bytes(source_cache_bytes);
        }"""
new = """        if state.cache_root == cache_root {
            state.source_cache.set_ram_budget_bytes(source_cache_bytes);
            if source_cache_bytes == 0 {
                state.decoder_cursor = None;
            }
        }"""
if text.count(old) != 1:
    raise SystemExit("RAM budget update shape changed")
text = text.replace(old, new, 1)
p.write_text(text)
