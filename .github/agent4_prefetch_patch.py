from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one replacement, found {count}")
    p.write_text(text.replace(old, new, 1))


# Android bridge: native warm operation with no destination buffer/copy.
path = "android/app/src/main/java/com/framescope/app/data/MicroscopePreviewBridge.kt"
replace_once(
    path,
    """    fun cancelSession(sessionId: Long): Boolean

    fun forgetSession(sessionId: Long): Boolean
}""",
    """    fun prefetchFrame(
        sessionId: Long,
        frameId: Long,
        cacheRoot: String,
    ): Boolean

    fun cancelSession(sessionId: Long): Boolean

    fun forgetSession(sessionId: Long): Boolean
}""",
)
replace_once(
    path,
    """    @JvmStatic
    private external fun nativeCancelMicroscopePreviewSession(sessionId: Long): Boolean
""",
    """    @JvmStatic
    private external fun nativePrefetchMicroscopePreviewFrame(
        sessionId: Long,
        frameId: Long,
        cacheRoot: String,
    ): Boolean

    @JvmStatic
    private external fun nativeCancelMicroscopePreviewSession(sessionId: Long): Boolean
""",
)
replace_once(
    path,
    """    override fun cancelSession(sessionId: Long): Boolean {
        if (sessionId <= 0L || loadFailure != null) return false
        return runCatching { nativeCancelMicroscopePreviewSession(sessionId) }.getOrDefault(false)
    }
""",
    """    override fun prefetchFrame(
        sessionId: Long,
        frameId: Long,
        cacheRoot: String,
    ): Boolean {
        if (sessionId <= 0L || frameId < 0L || cacheRoot.isBlank() || loadFailure != null) return false
        return runCatching {
            nativePrefetchMicroscopePreviewFrame(sessionId, frameId, cacheRoot)
        }.getOrDefault(false)
    }

    override fun cancelSession(sessionId: Long): Boolean {
        if (sessionId <= 0L || loadFailure != null) return false
        return runCatching { nativeCancelMicroscopePreviewSession(sessionId) }.getOrDefault(false)
    }
""",
)

# Coroutine source: speculation has its own API and never creates an Android RGBA destination.
path = "android/app/src/main/java/com/framescope/app/data/MicroscopeScrubPreviewSource.kt"
replace_once(
    path,
    """    fun cancelSession(sessionId: Long): Boolean

    suspend fun forgetSession(sessionId: Long)
}""",
    """    suspend fun prefetchFrame(
        sessionId: Long,
        frameId: Long,
    ): Boolean

    fun cancelSession(sessionId: Long): Boolean

    suspend fun forgetSession(sessionId: Long)
}""",
)
replace_once(
    path,
    """    override fun cancelSession(sessionId: Long): Boolean = false

    override suspend fun forgetSession(sessionId: Long) = Unit
}""",
    """    override suspend fun prefetchFrame(sessionId: Long, frameId: Long): Boolean = false

    override fun cancelSession(sessionId: Long): Boolean = false

    override suspend fun forgetSession(sessionId: Long) = Unit
}""",
)
replace_once(
    path,
    """    override fun cancelSession(sessionId: Long): Boolean =
        sessionId > 0L && bridge.cancelSession(sessionId)
""",
    """    override suspend fun prefetchFrame(
        sessionId: Long,
        frameId: Long,
    ): Boolean = withContext(ioDispatcher) {
        currentCoroutineContext().ensureActive()
        if (sessionId <= 0L || frameId < 0L) return@withContext false
        bridge.prefetchFrame(
            sessionId = sessionId,
            frameId = frameId,
            cacheRoot = cacheRoot,
        )
    }

    override fun cancelSession(sessionId: Long): Boolean =
        sessionId > 0L && bridge.cancelSession(sessionId)
""",
)

# MainViewModel: independent speculative job, demand/exact navigation always cancel it first.
path = "android/app/src/main/java/com/framescope/app/ui/MainViewModel.kt"
replace_once(
    path,
    """package com.framescope.app.ui

import androidx.lifecycle.ViewModel
""",
    """package com.framescope.app.ui

import android.os.SystemClock
import androidx.lifecycle.ViewModel
""",
)
replace_once(
    path,
    """import com.framescope.app.data.MicroscopeSessionSnapshot
import com.framescope.app.data.TimestampSelectionPolicy
""",
    """import com.framescope.app.data.MicroscopeSessionSnapshot
import com.framescope.app.data.RamAccelerationMode
import com.framescope.app.data.RamAccelerationRuntime
import com.framescope.app.data.TimestampSelectionPolicy
""",
)
replace_once(
    path,
    """    private var microscopeJob: Job? = null
    private var scrubWorkerJob: Job? = null
""",
    """    private var microscopeJob: Job? = null
    private var scrubWorkerJob: Job? = null
    private var scrubPrefetchJob: Job? = null
""",
)
replace_once(
    path,
    """    private val scrubGate = LiveScrubRequestGate()
    private val scrubSignal = Channel<Unit>(capacity = Channel.CONFLATED)
""",
    """    private val scrubGate = LiveScrubRequestGate()
    private val scrubPrefetchPolicy = LiveScrubPrefetchPolicy()
    private val scrubSignal = Channel<Unit>(capacity = Channel.CONFLATED)
""",
)
replace_once(
    path,
    """    private fun enqueueLiveScrub(target: LiveScrubTarget) {
        val ready = _uiState.value.microscopeState as? MicroscopeUiState.Ready ?: return
        val request = scrubGate.submit(ready.session.sessionId, target)
        scrubGate.cancellationForSupersededInFlight(request)?.let { cancellation ->
            scrubPreviewSource.cancelSession(cancellation.sessionId)
        }
        scrubSignal.trySend(Unit)
    }
""",
    """    private fun enqueueLiveScrub(target: LiveScrubTarget) {
        val ready = _uiState.value.microscopeState as? MicroscopeUiState.Ready ?: return
        cancelSpeculativeScrubPrefetch(ready.session.sessionId)
        val request = scrubGate.submit(ready.session.sessionId, target)
        scrubGate.cancellationForSupersededInFlight(request)?.let { cancellation ->
            scrubPreviewSource.cancelSession(cancellation.sessionId)
        }
        scrubSignal.trySend(Unit)
    }
""",
)
old = """            val publishable = scrubGate.finish(request)
            if (publishable) {
                result.onSuccess { preview ->
                    if (preview.descriptor.sessionId == request.sessionId) {
                        _uiState.update { current ->
                            val ready = current.microscopeState as? MicroscopeUiState.Ready
                            if (ready?.session?.sessionId == request.sessionId) {
                                current.copy(scrubPreview = preview)
                            } else {
                                current
                            }
                        }
                    }
                }
            }
            if (!scrubGate.hasPendingWork()) return
"""
new = """            val publishable = scrubGate.finish(request)
            if (publishable) {
                result.onSuccess { preview ->
                    if (preview.descriptor.sessionId == request.sessionId) {
                        var frameCount: Long? = null
                        _uiState.update { current ->
                            val ready = current.microscopeState as? MicroscopeUiState.Ready
                            if (ready?.session?.sessionId == request.sessionId) {
                                frameCount = ready.session.frameCount
                                current.copy(scrubPreview = preview)
                            } else {
                                current
                            }
                        }
                        val count = frameCount
                        if (count != null && !scrubGate.hasPendingWork()) {
                            val ram = RamAccelerationRuntime.current()?.state?.value
                            val accelerationEnabled = ram != null &&
                                ram.mode != RamAccelerationMode.Off &&
                                !ram.underMemoryPressure &&
                                ram.sourceCacheMiB > 0
                            scrubPrefetchPolicy.candidate(
                                sessionId = request.sessionId,
                                frameId = preview.descriptor.frameId,
                                frameCount = count,
                                completedAtMs = SystemClock.uptimeMillis(),
                                accelerationEnabled = accelerationEnabled,
                            )?.let { candidate ->
                                launchSpeculativeScrubPrefetch(request.sessionId, candidate)
                            }
                        }
                    }
                }
            }
            if (!scrubGate.hasPendingWork()) return
"""
replace_once(path, old, new)
replace_once(
    path,
    """    private fun invalidateLiveScrub(clearPreview: Boolean) {
        scrubGate.invalidate()?.let { cancellation ->
            scrubPreviewSource.cancelSession(cancellation.sessionId)
        }
        if (clearPreview) {
            _uiState.update { it.copy(scrubPreview = null) }
        }
    }
""",
    """    private fun invalidateLiveScrub(clearPreview: Boolean) {
        val sessionId = currentMicroscopeSessionId()
        cancelSpeculativeScrubPrefetch(sessionId)
        scrubPrefetchPolicy.reset()
        scrubGate.invalidate()?.let { cancellation ->
            scrubPreviewSource.cancelSession(cancellation.sessionId)
        }
        if (clearPreview) {
            _uiState.update { it.copy(scrubPreview = null) }
        }
    }

    private fun launchSpeculativeScrubPrefetch(sessionId: Long, frameId: Long) {
        if (sessionId <= 0L || frameId < 0L || scrubGate.hasPendingWork()) return
        scrubPrefetchJob?.cancel()
        scrubPrefetchJob = viewModelScope.launch {
            try {
                scrubPreviewSource.prefetchFrame(sessionId, frameId)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                // Speculation is never user-visible and must not affect authoritative navigation.
            }
        }
    }

    private fun cancelSpeculativeScrubPrefetch(sessionId: Long?) {
        val job = scrubPrefetchJob
        if (job?.isActive == true && sessionId != null && sessionId > 0L) {
            scrubPreviewSource.cancelSession(sessionId)
        }
        job?.cancel()
        scrubPrefetchJob = null
    }
""",
)

# Rust JNI: warm source/full preview caches without any Java-side RGBA allocation/copy.
path = "rust/crates/framescope-ffi/src/scrub_handoff.rs"
p = Path(path)
text = p.read_text()
text = text.replace(
    "const MAX_FORWARD_CURSOR_REUSE_FRAMES: u64 = 48;\n",
    "const MAX_FORWARD_CURSOR_REUSE_FRAMES: u64 = 48;\nconst DEFAULT_PREFETCH_EDGE: u32 = 640;\n",
    1,
)
marker = """#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_MicroscopePreviewBridge_nativeCancelMicroscopePreviewSession(
"""
insert = """#[unsafe(no_mangle)]
pub extern "system" fn Java_com_framescope_app_data_MicroscopePreviewBridge_nativePrefetchMicroscopePreviewFrame(
    mut env: JNIEnv,
    _class: JClass,
    session_id: jlong,
    frame_id: jlong,
    cache_root: JString,
) -> jboolean {
    let cache_root: String = match env.get_string(&cache_root) {
        Ok(value) => value.into(),
        Err(_) => return 0,
    };
    let prefetched = catch_unwind(AssertUnwindSafe(|| {
        prefetch_frame_response(session_id, frame_id, &cache_root)
    }))
    .is_ok_and(|result| result.is_ok());
    if prefetched { 1 } else { 0 }
}

"""
if text.count(marker) != 1:
    raise SystemExit("native cancel marker changed")
text = text.replace(marker, insert + marker, 1)

marker = """fn finish_preview(
    session_id: i64,
"""
prefetch = """#[cfg(unix)]
fn prefetch_frame_response(
    session_id: i64,
    frame_id: i64,
    cache_root: &str,
) -> Result<(), PreviewFailure> {
    if session_id <= 0 {
        return Err(PreviewFailure::new(
            "invalid_request",
            "Live preview prefetch requires a positive microscope session id.",
        ));
    }
    let frame_id = u64::try_from(frame_id)
        .map(FrameId)
        .map_err(|_| PreviewFailure::new("invalid_request", "Prefetch frame id must be non-negative."))?;
    if source_cache_budget_bytes() == 0 && preview_cache_budget_bytes() == 0 {
        return Ok(());
    }
    let cache_root = validate_cache_root(cache_root)?;
    microscope::with_extraction_context(session_id, |_source_fd, index| {
        microscope_target(index, frame_id)
            .map(|_| ())
            .map_err(|error| PreviewFailure::new("frame_out_of_range", error.to_string()))
    })
    .map_err(from_microscope_failure)??;
    let state = session_state(session_id, &cache_root)?;

    if preview_cache_budget_bytes() > 0 {
        let cached = state
            .lock()
            .map_err(|_| PreviewFailure::new("bridge_error", "Live preview cache state is poisoned."))?
            .preview_cache
            .get(frame_id, DEFAULT_PREFETCH_EDGE)
            .is_some();
        if cached {
            return Ok(());
        }
    }

    let cancellation = {
        let mut state = state.lock().map_err(|_| {
            PreviewFailure::new("bridge_error", "Live preview cache state is poisoned.")
        })?;
        cancellation_for_scrub_target(&mut state, frame_id)
    };
    let (cancellation, _operation) =
        begin_preview_operation_with_token(session_id, cancellation)?;
    ensure_not_cancelled(&cancellation)?;

    let navigated = microscope::with_extraction_context(session_id, |source_fd, index| {
        ensure_not_cancelled(&cancellation)?;
        let mut state = state.lock().map_err(|_| {
            PreviewFailure::new("bridge_error", "Live preview cache state is poisoned.")
        })?;
        let decoder_cancellation = cancellation.clone();
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
        .map_err(from_navigation)
    })
    .map_err(from_microscope_failure)??;
    ensure_not_cancelled(&cancellation)?;

    if preview_cache_budget_bytes() > 0 {
        let preview = downscale_scrub_preview(&navigated.pixels, DEFAULT_PREFETCH_EDGE)
            .map_err(|error| PreviewFailure::new("preview_scale_error", error.to_string()))?;
        ensure_not_cancelled(&cancellation)?;
        let mut state = state.lock().map_err(|_| {
            PreviewFailure::new("bridge_error", "Live preview cache state is poisoned.")
        })?;
        state
            .preview_cache
            .insert(frame_id, DEFAULT_PREFETCH_EDGE, preview);
    }
    Ok(())
}

#[cfg(not(unix))]
fn prefetch_frame_response(
    _session_id: i64,
    _frame_id: i64,
    _cache_root: &str,
) -> Result<(), PreviewFailure> {
    Err(PreviewFailure::new(
        "bridge_error",
        "Live microscope prefetch is only available on Android/Unix targets.",
    ))
}

"""
if text.count(marker) != 1:
    raise SystemExit("finish_preview marker changed")
text = text.replace(marker, prefetch + marker, 1)
p.write_text(text)
