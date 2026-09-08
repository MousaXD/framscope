package com.framescope.app.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.framescope.app.data.BatchExportProgress
import com.framescope.app.data.BatchExportRequest
import com.framescope.app.data.BatchExportSelection
import com.framescope.app.data.ExportedBatchDocument
import com.framescope.app.data.ExportStorageFailureClassifier
import com.framescope.app.data.FrameExportException
import com.framescope.app.data.FrameScopeRepository
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

data class PendingBatchExport(
    val sessionId: Long,
    val currentFrameIdAtRequest: Long?,
    val request: BatchExportRequest,
)

sealed interface BatchExportUiState {
    data object Idle : BatchExportUiState

    data class AwaitingDestination(
        val pending: PendingBatchExport,
    ) : BatchExportUiState

    data class Exporting(
        val pending: PendingBatchExport,
        val progress: BatchExportProgress? = null,
    ) : BatchExportUiState

    data class Success(
        val document: ExportedBatchDocument,
    ) : BatchExportUiState

    data class Error(
        val message: String,
        val code: String? = null,
    ) : BatchExportUiState
}

class BatchExportViewModel(
    private val repository: FrameScopeRepository,
) : ViewModel() {
    private val _state = MutableStateFlow<BatchExportUiState>(BatchExportUiState.Idle)
    val state: StateFlow<BatchExportUiState> = _state.asStateFlow()

    private val generation = AtomicLong(0L)
    private var exportJob: Job? = null

    fun beginBatchExport(
        sessionId: Long,
        currentFrameId: Long?,
        request: BatchExportRequest,
    ) {
        if (sessionId <= 0L || !request.isSane()) return
        val currentSelection = request.selection as? BatchExportSelection.CurrentFrame
        if (currentSelection != null && currentFrameId != currentSelection.frameId) return

        cancelWork(publishIdle = false)
        _state.value = BatchExportUiState.AwaitingDestination(
            PendingBatchExport(
                sessionId = sessionId,
                currentFrameIdAtRequest = currentFrameId,
                request = request,
            ),
        )
    }

    fun onDestinationSelected(
        treeUri: String,
        currentSessionId: Long?,
        currentFrameId: Long?,
    ) {
        val awaiting = _state.value as? BatchExportUiState.AwaitingDestination ?: return
        val pending = awaiting.pending
        if (!matchesCurrentMicroscope(pending, currentSessionId, currentFrameId)) {
            generation.incrementAndGet()
            _state.value = BatchExportUiState.Error(
                message = "The microscope changed while the export folder was being selected.",
                code = "stale_result",
            )
            return
        }

        val revision = generation.incrementAndGet()
        exportJob?.cancel()
        _state.value = BatchExportUiState.Exporting(pending)
        exportJob = viewModelScope.launch {
            try {
                repository.exportFrames(
                    treeUri = treeUri,
                    request = pending.request,
                    onProgress = { progress ->
                        if (revision == generation.get()) {
                            _state.value = BatchExportUiState.Exporting(
                                pending = pending,
                                progress = progress,
                            )
                        }
                    },
                ).onSuccess { document ->
                    if (revision == generation.get()) {
                        if (
                            document.export.sessionId == pending.sessionId &&
                            document.export.format == pending.request.format
                        ) {
                            _state.value = BatchExportUiState.Success(document)
                        } else {
                            _state.value = BatchExportUiState.Error(
                                message = "The completed batch does not match the requested microscope session.",
                                code = "export_identity_mismatch",
                            )
                        }
                    }
                }.onFailure { error ->
                    if (revision == generation.get()) {
                        val exportError = error as? FrameExportException
                        val storageFailure = exportError?.let {
                            ExportStorageFailureClassifier.classifyNative(
                                code = it.code,
                                message = error.message.orEmpty(),
                            )
                        } ?: ExportStorageFailureClassifier.classify(error)
                        _state.value = BatchExportUiState.Error(
                            message = storageFailure?.message
                                ?: error.message
                                ?: "Could not export the selected frames.",
                            code = storageFailure?.code ?: exportError?.code,
                        )
                    }
                }
            } catch (cancelled: CancellationException) {
                if (revision == generation.get()) {
                    _state.value = BatchExportUiState.Idle
                }
                throw cancelled
            }
        }
    }

    fun onDestinationPickerCancelled() {
        if (_state.value is BatchExportUiState.AwaitingDestination) {
            generation.incrementAndGet()
            _state.value = BatchExportUiState.Idle
        }
    }

    fun cancelForMicroscopeChange() {
        cancelWork(publishIdle = true)
    }

    fun dismissStatus() {
        when (_state.value) {
            is BatchExportUiState.Success,
            is BatchExportUiState.Error,
            -> _state.value = BatchExportUiState.Idle

            else -> Unit
        }
    }

    private fun matchesCurrentMicroscope(
        pending: PendingBatchExport,
        currentSessionId: Long?,
        currentFrameId: Long?,
    ): Boolean {
        if (currentSessionId != pending.sessionId) return false
        val currentSelection = pending.request.selection as? BatchExportSelection.CurrentFrame
            ?: return true
        return currentFrameId == currentSelection.frameId
    }

    private fun cancelWork(publishIdle: Boolean) {
        generation.incrementAndGet()
        repository.cancelActiveNativeOperation()
        exportJob?.cancel()
        exportJob = null
        if (publishIdle) {
            _state.value = BatchExportUiState.Idle
        }
    }

    override fun onCleared() {
        cancelWork(publishIdle = false)
        super.onCleared()
    }
}

class BatchExportViewModelFactory(
    private val repository: FrameScopeRepository,
) : ViewModelProvider.Factory {
    @Suppress("UNCHECKED_CAST")
    override fun <T : ViewModel> create(modelClass: Class<T>): T {
        require(modelClass.isAssignableFrom(BatchExportViewModel::class.java))
        return BatchExportViewModel(repository) as T
    }
}
