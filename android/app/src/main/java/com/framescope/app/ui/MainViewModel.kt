package com.framescope.app.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.framescope.app.data.FrameScopeRepository
import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.InspectionProgress
import com.framescope.app.data.VideoOpenException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
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

data class FrameScopeUiState(
    val engineStatus: EngineStatus = EngineStatus.Checking,
    val videoState: VideoInspectionState = VideoInspectionState.Idle,
)

class MainViewModel(
    private val repository: FrameScopeRepository,
) : ViewModel() {
    private val _uiState = MutableStateFlow(FrameScopeUiState())
    val uiState: StateFlow<FrameScopeUiState> = _uiState.asStateFlow()

    private var inspectJob: Job? = null
    private var inspectionGeneration: Long = 0

    init {
        viewModelScope.launch {
            repository.engineVersion()
                .onSuccess { version ->
                    _uiState.update { it.copy(engineStatus = EngineStatus.Ready(version)) }
                }
                .onFailure { error ->
                    _uiState.update {
                        it.copy(
                            engineStatus = EngineStatus.Unavailable(
                                error.message ?: "Rust engine is unavailable.",
                            ),
                        )
                    }
                }
        }
    }

    fun onPickerStarted() {
        cancelRunningInspection()
        inspectionGeneration += 1
        _uiState.update { it.copy(videoState = VideoInspectionState.Picking) }
    }

    fun onVideoSelected(uri: String) {
        cancelRunningInspection()
        inspectionGeneration += 1
        val generation = inspectionGeneration

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
                    if (generation == inspectionGeneration) {
                        _uiState.update {
                            it.copy(
                                engineStatus = EngineStatus.Ready(video.engine),
                                videoState = VideoInspectionState.Ready(video),
                            )
                        }
                    }
                }.onFailure { error ->
                    if (generation == inspectionGeneration) {
                        val bridgeError = error as? VideoOpenException
                        _uiState.update {
                            it.copy(
                                videoState = VideoInspectionState.Error(
                                    message = error.message ?: "Could not inspect this video.",
                                    diagnostic = bridgeError?.diagnostic,
                                ),
                            )
                        }
                    }
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            }
        }
    }

    fun cancelInspection() {
        inspectionGeneration += 1
        cancelRunningInspection()
        _uiState.update { it.copy(videoState = VideoInspectionState.Cancelled) }
    }

    fun onPickerCancelled() {
        inspectionGeneration += 1
        cancelRunningInspection()
        _uiState.update { it.copy(videoState = VideoInspectionState.Cancelled) }
    }

    fun clearError() {
        _uiState.update { state ->
            if (state.videoState is VideoInspectionState.Error) {
                state.copy(videoState = VideoInspectionState.Idle)
            } else {
                state
            }
        }
    }

    private fun publishIfCurrent(
        generation: Long,
        state: VideoInspectionState,
    ) {
        if (generation == inspectionGeneration) {
            _uiState.update { it.copy(videoState = state) }
        }
    }

    private fun cancelRunningInspection() {
        inspectJob?.cancel()
        inspectJob = null
    }

    override fun onCleared() {
        cancelRunningInspection()
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
