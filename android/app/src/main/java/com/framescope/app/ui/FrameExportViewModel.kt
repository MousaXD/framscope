package com.framescope.app.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.framescope.app.data.ExportedFrameDocument
import com.framescope.app.data.FrameExportException
import com.framescope.app.data.FrameExportFormat
import com.framescope.app.data.FrameScopeRepository
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

data class CurrentFrameExportRequest(
    val sessionId: Long,
    val frameId: Long,
    val format: FrameExportFormat,
)

sealed interface FrameExportUiState {
    data object Idle : FrameExportUiState

    data class AwaitingDestination(
        val request: CurrentFrameExportRequest,
    ) : FrameExportUiState

    data class Exporting(
        val request: CurrentFrameExportRequest,
    ) : FrameExportUiState

    data class Success(
        val document: ExportedFrameDocument,
    ) : FrameExportUiState

    data class Error(
        val message: String,
        val code: String? = null,
    ) : FrameExportUiState
}

class FrameExportViewModel(
    private val repository: FrameScopeRepository,
) : ViewModel() {
    private val _state = MutableStateFlow<FrameExportUiState>(FrameExportUiState.Idle)
    val state: StateFlow<FrameExportUiState> = _state.asStateFlow()

    private val generation = AtomicLong(0L)
    private var exportJob: Job? = null

    fun beginCurrentFrameExport(
        sessionId: Long,
        frameId: Long,
        format: FrameExportFormat,
    ) {
        if (sessionId <= 0L || frameId < 0L) return
        cancelWork(publishIdle = false)
        _state.value = FrameExportUiState.AwaitingDestination(
            CurrentFrameExportRequest(
                sessionId = sessionId,
                frameId = frameId,
                format = format,
            ),
        )
    }

    fun onDestinationSelected(
        treeUri: String,
        currentSessionId: Long?,
        currentFrameId: Long?,
    ) {
        val awaiting = _state.value as? FrameExportUiState.AwaitingDestination ?: return
        val request = awaiting.request
        if (
            currentSessionId != request.sessionId ||
            currentFrameId != request.frameId
        ) {
            _state.value = FrameExportUiState.Error(
                message = "The microscope moved while the export folder was being selected.",
                code = "stale_result",
            )
            return
        }
        val revision = generation.incrementAndGet()
        exportJob?.cancel()
        _state.value = FrameExportUiState.Exporting(request)
        exportJob = viewModelScope.launch {
            try {
                repository.exportCurrentFrame(
                    treeUri = treeUri,
                    format = request.format,
                ).onSuccess { document ->
                    if (revision == generation.get()) {
                        if (
                            document.export.sessionId == request.sessionId &&
                            document.export.frameId == request.frameId &&
                            document.export.format == request.format
                        ) {
                            _state.value = FrameExportUiState.Success(document)
                        } else {
                            _state.value = FrameExportUiState.Error(
                                message = "The exported document does not match the requested microscope frame.",
                                code = "export_identity_mismatch",
                            )
                        }
                    }
                }.onFailure { error ->
                    if (revision == generation.get()) {
                        val exportError = error as? FrameExportException
                        _state.value = FrameExportUiState.Error(
                            message = error.message ?: "Could not export this frame.",
                            code = exportError?.code,
                        )
                    }
                }
            } catch (cancelled: CancellationException) {
                if (revision == generation.get()) {
                    _state.value = FrameExportUiState.Idle
                }
                throw cancelled
            }
        }
    }

    fun onDestinationPickerCancelled() {
        if (_state.value is FrameExportUiState.AwaitingDestination) {
            generation.incrementAndGet()
            _state.value = FrameExportUiState.Idle
        }
    }

    fun cancelForMicroscopeChange() {
        cancelWork(publishIdle = true)
    }

    fun dismissStatus() {
        when (_state.value) {
            is FrameExportUiState.Success,
            is FrameExportUiState.Error,
            -> _state.value = FrameExportUiState.Idle

            else -> Unit
        }
    }

    private fun cancelWork(publishIdle: Boolean) {
        generation.incrementAndGet()
        repository.cancelActiveNativeOperation()
        exportJob?.cancel()
        exportJob = null
        if (publishIdle) {
            _state.value = FrameExportUiState.Idle
        }
    }

    override fun onCleared() {
        cancelWork(publishIdle = false)
        super.onCleared()
    }
}

class FrameExportViewModelFactory(
    private val repository: FrameScopeRepository,
) : ViewModelProvider.Factory {
    @Suppress("UNCHECKED_CAST")
    override fun <T : ViewModel> create(modelClass: Class<T>): T {
        require(modelClass.isAssignableFrom(FrameExportViewModel::class.java))
        return FrameExportViewModel(repository) as T
    }
}
