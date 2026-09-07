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
            )
        }
    }

    fun onVideoSelected(uri: String) {
        val generation = inspectionGeneration.incrementAndGet()
        cancelRunningInspection()
        invalidateMicroscopeWork(closeSession = false)

        inspectJob = viewModelScope.launch {
            publishIfCurrent(generation, VideoInspectionState.Opening)
            try {
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
        if (delta == 0) return
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

    fun cancelInspection() {
        inspectionGeneration.incrementAndGet()
        cancelRunningInspection()
        invalidateMicroscopeWork(closeSession = true)
        _uiState.update {
            it.copy(
                videoState = VideoInspectionState.Cancelled,
                microscopeState = MicroscopeUiState.Idle,
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
            )
        }
    }

    fun clearError() {
        _uiState.update { state ->
            state.copy(
                videoState = if (state.videoState is VideoInspectionState.Error) {
                    VideoInspectionState.Idle
                } else {
                    state.videoState
                },
                microscopeState = if (state.microscopeState is MicroscopeUiState.Error) {
                    MicroscopeUiState.Idle
                } else {
                    state.microscopeState
                },
            )
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
                repository.closeMicroscope()
                if (!isCurrent(inspectionGenerationAtStart, microscopeRevision)) return@launch
                publishMicroscopeIfCurrent(
                    inspectionGenerationAtStart,
                    microscopeRevision,
                    MicroscopeUiState.Opening,
                )
                repository.openMicroscope(uri)
                    .onSuccess { session ->
                        if (!isCurrent(inspectionGenerationAtStart, microscopeRevision)) return@onSuccess
                        if (session.currentFrame == null) {
                            publishMicroscopeIfCurrent(
                                inspectionGenerationAtStart,
                                microscopeRevision,
                                MicroscopeUiState.Empty(session),
                            )
                        } else {
                            loadFrameForSession(
                                session = session,
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
    ) {
        publishMicroscopeIfCurrent(
            inspectionGenerationAtStart,
            microscopeRevision,
            MicroscopeUiState.LoadingFrame(session),
        )
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
