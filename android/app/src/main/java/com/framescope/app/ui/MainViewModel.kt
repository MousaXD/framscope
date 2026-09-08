package com.framescope.app.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.framescope.app.data.FrameScopeRepository
import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.InspectionProgress
import com.framescope.app.data.MicroscopeFrame
import com.framescope.app.data.MicroscopeOperationException
import com.framescope.app.data.MicroscopeSessionSnapshot
import com.framescope.app.data.TimestampSelectionPolicy
import com.framescope.app.data.VideoOpenException
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

sealed interface EngineStatus {
    data object Checking : EngineStatus

    data class Ready(
        val version: String,
    ) : EngineStatus

    data class Unavailable(
        val message: String,
    ) : EngineStatus
}

sealed interface VideoInspectionState {
    data object Idle : VideoInspectionState
    data object Picking : VideoInspectionState
    data object Opening : VideoInspectionState
    data object Inspecting : VideoInspectionState

    data class Ready(
        val video: InspectedVideo,
    ) : VideoInspectionState

    data class Error(
        val message: String,
        val diagnostic: String? = null,
    ) : VideoInspectionState

    data object Cancelled : VideoInspectionState
}

sealed interface MicroscopeUiState {
    data object Idle : MicroscopeUiState
    data object Opening : MicroscopeUiState

    data class LoadingFrame(
        val session: MicroscopeSessionSnapshot,
    ) : MicroscopeUiState

    data class Navigating(
        val session: MicroscopeSessionSnapshot,
        val previousFrame: MicroscopeFrame?,
    ) : MicroscopeUiState

    data class Ready(
        val session: MicroscopeSessionSnapshot,
        val frame: MicroscopeFrame,
    ) : MicroscopeUiState

    data class Empty(
        val session: MicroscopeSessionSnapshot,
    ) : MicroscopeUiState

    data class Error(
        val message: String,
        val code: String? = null,
        val session: MicroscopeSessionSnapshot? = null,
    ) : MicroscopeUiState
}

data class FrameScopeUiState(
    val engineStatus: EngineStatus = EngineStatus.Checking,
    val videoState: VideoInspectionState = VideoInspectionState.Idle,
    val microscopeState: MicroscopeUiState = MicroscopeUiState.Idle,
    val timelineBounds: IndexedTimelineBounds? = null,
    val timelineRange: TimelineRangeSelection? = null,
)

class MainViewModel(
    private val repository: FrameScopeRepository,
) : ViewModel() {
    private val _uiState = MutableStateFlow(FrameScopeUiState())
    val uiState: StateFlow<FrameScopeUiState> = _uiState.asStateFlow()

    private var inspectJob: Job? = null
    private var microscopeJob: Job? = null
    private val inspectionGeneration = AtomicLong(0)
    private val microscopeGeneration = AtomicLong(0)
    private val lifecycleCleanupScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    init {
        viewModelScope.launch {
            repository.engineVersion()
                .onSuccess { version -> publishInitialEngineStatus(EngineStatus.Ready(version)) }
                .onFailure { error ->
                    publishInitialEngineStatus(
                        EngineStatus.Unavailable(error.message ?: "Rust engine is unavailable."),
                    )
                }
        }
    }

    fun onPickerStarted() {
        inspectionGeneration.incrementAndGet()
        cancelRunningInspection()
        invalidateMicroscopeWork(closeSession = true)
        _uiState.update {
            it.copy(
                videoState = VideoInspectionState.Picking,
                microscopeState = MicroscopeUiState.Idle,
                timelineBounds = null,
                timelineRange = null,
            )
        }
    }

    fun onVideoSelected(uri: String) {
        val generation = inspectionGeneration.incrementAndGet()
        cancelRunningInspection()
        invalidateMicroscopeWork(closeSession = false)
        // Reflect the user's selection immediately. Native teardown/probing/indexing continues off
        // the UI thread and updates this state as each stage becomes authoritative.
        _uiState.update {
            it.copy(
                videoState = VideoInspectionState.Opening,
                microscopeState = MicroscopeUiState.Idle,
                timelineBounds = null,
                timelineRange = null,
            )
        }

        inspectJob = viewModelScope.launch {
            try {
                // Source replacement owns teardown. Await it before inspecting the replacement so a late
                // close can never destroy the newly opened native session, and a failed replacement still
                // leaves no previous-session ownership behind.
                repository.closeMicroscope()
                if (generation != inspectionGeneration.get()) return@launch

                repository.inspect(uri) { progress ->
                    val nextState = when (progress) {
                        InspectionProgress.Opening -> VideoInspectionState.Opening
                        InspectionProgress.Inspecting -> VideoInspectionState.Inspecting
                    }
                    publishIfCurrent(generation, nextState)
                }.onSuccess { video ->
                    if (generation == inspectionGeneration.get()) {
                        _uiState.update {
                            it.copy(
                                engineStatus = EngineStatus.Ready(video.engine),
                                videoState = VideoInspectionState.Ready(video),
                            )
                        }
                        openMicroscope(uri, generation)
                    }
                }.onFailure { error ->
                    if (generation == inspectionGeneration.get()) {
                        val bridgeError = error as? VideoOpenException
                        _uiState.update {
                            it.copy(
                                videoState = VideoInspectionState.Error(
                                    message = error.message ?: "Could not inspect this video.",
                                    diagnostic = bridgeError?.diagnostic,
                                ),
                                microscopeState = MicroscopeUiState.Idle,
                                timelineBounds = null,
                                timelineRange = null,
                            )
                        }
                    }
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            }
        }
    }

    fun stepMicroscope(delta: Int) {
        if (delta !in setOf(-1, 1)) return
        navigateMicroscope { repository.stepMicroscope(delta) }
    }

    fun jumpMicroscopeFrame(frameId: Long) {
        if (frameId < 0L) return
        navigateMicroscope { repository.jumpMicroscopeFrame(frameId) }
    }

    fun jumpMicroscopeTimestampUs(
        timestampUs: Long,
        selection: TimestampSelectionPolicy = TimestampSelectionPolicy.Nearest,
    ) {
        navigateMicroscope { repository.jumpMicroscopeTimestampUs(timestampUs, selection) }
    }

    fun commitTimelineRange(startUs: Long, endUs: Long) {
        val state = _uiState.value
        val ready = state.microscopeState as? MicroscopeUiState.Ready ?: return
        val bounds = state.timelineBounds?.takeIf {
            it.sessionId == ready.session.sessionId && it.isSane()
        } ?: return
        val selection = TimelineRangeSelection(
            sessionId = ready.session.sessionId,
            startUs = startUs,
            endUs = endUs,
        )
        if (!selection.isSaneFor(bounds)) return
        _uiState.update { current ->
            val currentReady = current.microscopeState as? MicroscopeUiState.Ready
            if (
                currentReady?.session?.sessionId == selection.sessionId &&
                current.timelineBounds == bounds
            ) {
                current.copy(timelineRange = selection)
            } else {
                current
            }
        }
    }

    fun clearTimelineRange() {
        _uiState.update { it.copy(timelineRange = null) }
    }

    fun cancelInspection() {
        inspectionGeneration.incrementAndGet()
        cancelRunningInspection()
        invalidateMicroscopeWork(closeSession = true)
        _uiState.update {
            it.copy(
                videoState = VideoInspectionState.Cancelled,
                microscopeState = MicroscopeUiState.Idle,
                timelineBounds = null,
                timelineRange = null,
            )
        }
    }

    fun onPickerCancelled() {
        inspectionGeneration.incrementAndGet()
        cancelRunningInspection()
        invalidateMicroscopeWork(closeSession = true)
        _uiState.update {
            it.copy(
                videoState = VideoInspectionState.Cancelled,
                microscopeState = MicroscopeUiState.Idle,
                timelineBounds = null,
                timelineRange = null,
            )
        }
    }

    fun clearError() {
        val state = _uiState.value
        val microscopeError = state.microscopeState as? MicroscopeUiState.Error
        if (microscopeError?.session != null) {
            closeMicroscopeErrorBeforeClearing(microscopeError)
            if (state.videoState is VideoInspectionState.Error) {
                _uiState.update { current ->
                    current.copy(videoState = VideoInspectionState.Idle)
                }
            }
            return
        }

        _uiState.update { current ->
            current.copy(
                videoState = if (current.videoState is VideoInspectionState.Error) {
                    VideoInspectionState.Idle
                } else {
                    current.videoState
                },
                microscopeState = if (current.microscopeState is MicroscopeUiState.Error) {
                    MicroscopeUiState.Idle
                } else {
                    current.microscopeState
                },
                timelineBounds = if (current.microscopeState is MicroscopeUiState.Error) {
                    null
                } else {
                    current.timelineBounds
                },
                timelineRange = if (current.microscopeState is MicroscopeUiState.Error) {
                    null
                } else {
                    current.timelineRange
                },
            )
        }
    }

    private fun closeMicroscopeErrorBeforeClearing(error: MicroscopeUiState.Error) {
        val inspectionRevision = inspectionGeneration.get()
        val microscopeRevision = microscopeGeneration.incrementAndGet()
        microscopeJob?.cancel()
        microscopeJob = viewModelScope.launch {
            try {
                repository.closeMicroscope()
                if (!isCurrent(inspectionRevision, microscopeRevision)) return@launch
                _uiState.update { current ->
                    if (current.microscopeState == error) {
                        current.copy(
                            microscopeState = MicroscopeUiState.Idle,
                            timelineBounds = null,
                            timelineRange = null,
                        )
                    } else {
                        current
                    }
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            }
        }
    }

    private fun openMicroscope(
        uri: String,
        inspectionGenerationAtStart: Long,
    ) {
        val microscopeRevision = microscopeGeneration.incrementAndGet()
        microscopeJob?.cancel()
        microscopeJob = viewModelScope.launch {
            try {
                if (!isCurrent(inspectionGenerationAtStart, microscopeRevision)) return@launch
                publishMicroscopeIfCurrent(
                    inspectionGenerationAtStart,
                    microscopeRevision,
                    MicroscopeUiState.Opening,
                )
                repository.openMicroscope(uri)
                    .onSuccess { openedSession ->
                        if (!isCurrent(inspectionGenerationAtStart, microscopeRevision)) return@onSuccess
                        if (openedSession.currentFrame == null) {
                            publishTimelineBoundsIfCurrent(
                                inspectionGenerationAtStart,
                                microscopeRevision,
                                null,
                            )
                            publishMicroscopeIfCurrent(
                                inspectionGenerationAtStart,
                                microscopeRevision,
                                MicroscopeUiState.Empty(openedSession),
                            )
                        } else {
                            val preparedSession = resolveIndexedTimelineBounds(
                                session = openedSession,
                                inspectionRevision = inspectionGenerationAtStart,
                                microscopeRevision = microscopeRevision,
                            ) ?: return@onSuccess
                            loadFrameForSession(
                                session = preparedSession,
                                inspectionGenerationAtStart = inspectionGenerationAtStart,
                                microscopeRevision = microscopeRevision,
                            )
                        }
                    }
                    .onFailure { error ->
                        publishMicroscopeFailure(
                            inspectionGenerationAtStart,
                            microscopeRevision,
                            error,
                            session = null,
                        )
                    }
            } catch (cancelled: CancellationException) {
                throw cancelled
            }
        }
    }

    /**
     * Resolve exact timeline endpoints with indexed O(1) FrameId lookups before first presentation.
     *
     * The native session opens on FrameId 0. Jumping to the last FrameId and immediately restoring
     * FrameId 0 touches only persistent navigation metadata; source-quality pixels are not decoded
     * until [loadFrameForSession]. This avoids a new JNI ABI solely for endpoint metadata while
     * preserving authoritative PTS, VFR, and non-zero-origin behavior.
     */
    private suspend fun resolveIndexedTimelineBounds(
        session: MicroscopeSessionSnapshot,
        inspectionRevision: Long,
        microscopeRevision: Long,
    ): MicroscopeSessionSnapshot? {
        val first = session.currentFrame ?: return session
        if (first.frameId != 0L || session.frameCount <= 0L) {
            publishMicroscopeIfCurrent(
                inspectionRevision,
                microscopeRevision,
                MicroscopeUiState.Error(
                    message = "The complete microscope index did not open on its first presentation frame.",
                    code = "timeline_identity_mismatch",
                    session = session,
                ),
            )
            return null
        }

        if (session.frameCount == 1L) {
            val bounds = first.timestampUs?.let {
                IndexedTimelineBounds(session.sessionId, it, it)
            }?.takeIf(IndexedTimelineBounds::isSane)
            publishTimelineBoundsIfCurrent(
                inspectionRevision,
                microscopeRevision,
                bounds,
            )
            return session
        }

        val lastFrameId = session.frameCount - 1L
        val lastResult = repository.jumpMicroscopeFrame(lastFrameId)
        if (!isCurrent(inspectionRevision, microscopeRevision)) return null
        val lastSession = lastResult.getOrElse { error ->
            publishMicroscopeFailure(
                inspectionRevision,
                microscopeRevision,
                error,
                session,
            )
            return null
        }
        val last = lastSession.currentFrame
        if (
            lastSession.sessionId != session.sessionId ||
            lastSession.frameCount != session.frameCount ||
            last?.frameId != lastFrameId
        ) {
            publishMicroscopeIfCurrent(
                inspectionRevision,
                microscopeRevision,
                MicroscopeUiState.Error(
                    message = "FrameScope could not verify the indexed timeline endpoint identity.",
                    code = "timeline_identity_mismatch",
                    session = lastSession,
                ),
            )
            return null
        }

        val restoreResult = repository.jumpMicroscopeFrame(first.frameId)
        if (!isCurrent(inspectionRevision, microscopeRevision)) return null
        val restored = restoreResult.getOrElse { error ->
            publishMicroscopeFailure(
                inspectionRevision,
                microscopeRevision,
                error,
                lastSession,
            )
            return null
        }
        if (
            restored.sessionId != session.sessionId ||
            restored.frameCount != session.frameCount ||
            restored.currentFrame?.frameId != first.frameId
        ) {
            publishMicroscopeIfCurrent(
                inspectionRevision,
                microscopeRevision,
                MicroscopeUiState.Error(
                    message = "FrameScope could not restore the first indexed frame after resolving timeline bounds.",
                    code = "timeline_identity_mismatch",
                    session = restored,
                ),
            )
            return null
        }

        val bounds = if (first.timestampUs != null && last.timestampUs != null) {
            IndexedTimelineBounds(
                sessionId = session.sessionId,
                startUs = first.timestampUs,
                endUs = last.timestampUs,
            ).takeIf(IndexedTimelineBounds::isSane)
        } else {
            null
        }
        publishTimelineBoundsIfCurrent(
            inspectionRevision,
            microscopeRevision,
            bounds,
        )
        return restored
    }

    private fun navigateMicroscope(
        operation: suspend () -> Result<MicroscopeSessionSnapshot>,
    ) {
        val current = _uiState.value.microscopeState
        val session = when (current) {
            is MicroscopeUiState.Ready -> current.session
            is MicroscopeUiState.Empty -> current.session
            else -> return
        }
        val previousFrame = (current as? MicroscopeUiState.Ready)?.frame
        val inspectionRevision = inspectionGeneration.get()
        val microscopeRevision = microscopeGeneration.incrementAndGet()
        microscopeJob?.cancel()
        publishMicroscopeIfCurrent(
            inspectionRevision,
            microscopeRevision,
            MicroscopeUiState.Navigating(session, previousFrame),
        )
        microscopeJob = viewModelScope.launch {
            try {
                operation()
                    .onSuccess { updated ->
                        if (!isCurrent(inspectionRevision, microscopeRevision)) return@onSuccess
                        if (updated.currentFrame == null) {
                            publishMicroscopeIfCurrent(
                                inspectionRevision,
                                microscopeRevision,
                                MicroscopeUiState.Empty(updated),
                            )
                        } else {
                            loadFrameForSession(
                                session = updated,
                                inspectionGenerationAtStart = inspectionRevision,
                                microscopeRevision = microscopeRevision,
                                publishLoadingState = false,
                            )
                        }
                    }
                    .onFailure { error ->
                        publishMicroscopeFailure(
                            inspectionRevision,
                            microscopeRevision,
                            error,
                            session,
                        )
                    }
            } catch (cancelled: CancellationException) {
                throw cancelled
            }
        }
    }

    private suspend fun loadFrameForSession(
        session: MicroscopeSessionSnapshot,
        inspectionGenerationAtStart: Long,
        microscopeRevision: Long,
        publishLoadingState: Boolean = true,
    ) {
        if (publishLoadingState) {
            publishMicroscopeIfCurrent(
                inspectionGenerationAtStart,
                microscopeRevision,
                MicroscopeUiState.LoadingFrame(session),
            )
        }
        repository.loadMicroscopeFrame()
            .onSuccess { frame ->
                if (!isCurrent(inspectionGenerationAtStart, microscopeRevision)) return@onSuccess
                val expectedFrameId = session.currentFrame?.frameId
                if (
                    frame.descriptor.sessionId != session.sessionId ||
                    expectedFrameId == null ||
                    frame.descriptor.frameId != expectedFrameId
                ) {
                    publishMicroscopeIfCurrent(
                        inspectionGenerationAtStart,
                        microscopeRevision,
                        MicroscopeUiState.Error(
                            message = "FrameScope received pixels for a different microscope frame.",
                            code = "presentation_identity_mismatch",
                            session = session,
                        ),
                    )
                } else {
                    publishMicroscopeIfCurrent(
                        inspectionGenerationAtStart,
                        microscopeRevision,
                        MicroscopeUiState.Ready(session, frame),
                    )
                }
            }
            .onFailure { error ->
                publishMicroscopeFailure(
                    inspectionGenerationAtStart,
                    microscopeRevision,
                    error,
                    session,
                )
            }
    }

    private fun publishMicroscopeFailure(
        inspectionRevision: Long,
        microscopeRevision: Long,
        error: Throwable,
        session: MicroscopeSessionSnapshot?,
    ) {
        val nativeError = error as? MicroscopeOperationException
        publishMicroscopeIfCurrent(
            inspectionRevision,
            microscopeRevision,
            MicroscopeUiState.Error(
                message = error.message ?: "Microscope operation failed.",
                code = nativeError?.code,
                session = session,
            ),
        )
    }

    private fun isCurrent(
        inspectionRevision: Long,
        microscopeRevision: Long,
    ): Boolean =
        inspectionRevision == inspectionGeneration.get() &&
            microscopeRevision == microscopeGeneration.get()

    private fun publishMicroscopeIfCurrent(
        inspectionRevision: Long,
        microscopeRevision: Long,
        state: MicroscopeUiState,
    ) {
        if (isCurrent(inspectionRevision, microscopeRevision)) {
            _uiState.update { it.copy(microscopeState = state) }
        }
    }

    private fun publishTimelineBoundsIfCurrent(
        inspectionRevision: Long,
        microscopeRevision: Long,
        bounds: IndexedTimelineBounds?,
    ) {
        if (isCurrent(inspectionRevision, microscopeRevision)) {
            _uiState.update {
                it.copy(
                    timelineBounds = bounds,
                    timelineRange = null,
                )
            }
        }
    }

    private fun publishInitialEngineStatus(status: EngineStatus) {
        _uiState.update { state ->
            if (state.engineStatus == EngineStatus.Checking) {
                state.copy(engineStatus = status)
            } else {
                state
            }
        }
    }

    private fun publishIfCurrent(
        generation: Long,
        state: VideoInspectionState,
    ) {
        if (generation == inspectionGeneration.get()) {
            _uiState.update { it.copy(videoState = state) }
        }
    }

    private fun cancelRunningInspection() {
        repository.cancelActiveInspection()
        inspectJob?.cancel()
        inspectJob = null
    }

    private fun invalidateMicroscopeWork(closeSession: Boolean) {
        microscopeGeneration.incrementAndGet()
        repository.cancelActiveInspection()
        microscopeJob?.cancel()
        microscopeJob = null
        if (closeSession) {
            microscopeJob = viewModelScope.launch {
                repository.closeMicroscope()
            }
        }
    }

    override fun onCleared() {
        inspectionGeneration.incrementAndGet()
        cancelRunningInspection()
        microscopeGeneration.incrementAndGet()
        microscopeJob?.cancel()
        microscopeJob = null
        lifecycleCleanupScope.launch {
            try {
                repository.closeMicroscope()
            } finally {
                lifecycleCleanupScope.cancel()
            }
        }
        super.onCleared()
    }
}

class MainViewModelFactory(
    private val repository: FrameScopeRepository,
) : ViewModelProvider.Factory {
    @Suppress("UNCHECKED_CAST")
    override fun <T : ViewModel> create(modelClass: Class<T>): T {
        require(modelClass.isAssignableFrom(MainViewModel::class.java))
        return MainViewModel(repository) as T
    }
}
