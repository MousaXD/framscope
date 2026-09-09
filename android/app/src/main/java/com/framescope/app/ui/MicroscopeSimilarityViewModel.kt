package com.framescope.app.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.framescope.app.data.MicroscopeSimilarityException
import com.framescope.app.data.MicroscopeSimilarityRepository
import com.framescope.app.data.MicroscopeSimilarityResult
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

sealed interface MicroscopeSimilarityUiState {
    data object Idle : MicroscopeSimilarityUiState

    data class Searching(
        val sessionId: Long,
        val targetFrameId: Long,
    ) : MicroscopeSimilarityUiState

    data class Ready(
        val result: MicroscopeSimilarityResult,
    ) : MicroscopeSimilarityUiState

    data class Error(
        val sessionId: Long,
        val targetFrameId: Long,
        val code: String?,
        val message: String,
    ) : MicroscopeSimilarityUiState
}

internal class MicroscopeSimilarityViewModel(
    private val repository: MicroscopeSimilarityRepository,
) : ViewModel() {
    private val _state = MutableStateFlow<MicroscopeSimilarityUiState>(MicroscopeSimilarityUiState.Idle)
    val state: StateFlow<MicroscopeSimilarityUiState> = _state.asStateFlow()

    private val generation = AtomicLong(0L)
    private var searchJob: Job? = null

    fun findSimilarFrames(sessionId: Long, targetFrameId: Long) {
        if (sessionId <= 0L || targetFrameId < 0L) return
        val revision = generation.incrementAndGet()
        cancelSearchJobOnly()
        repository.cancelActiveSearch()
        _state.value = MicroscopeSimilarityUiState.Searching(sessionId, targetFrameId)
        searchJob = viewModelScope.launch {
            try {
                repository.findSimilarFrames(sessionId, targetFrameId)
                    .onSuccess { result ->
                        if (revision != generation.get()) return@onSuccess
                        if (
                            result.sessionId == sessionId &&
                            result.targetFrameId == targetFrameId
                        ) {
                            _state.value = MicroscopeSimilarityUiState.Ready(result)
                        } else {
                            _state.value = MicroscopeSimilarityUiState.Error(
                                sessionId = sessionId,
                                targetFrameId = targetFrameId,
                                code = "similarity_identity_mismatch",
                                message = "Similarity results no longer match the requested frame.",
                            )
                        }
                    }
                    .onFailure { error ->
                        if (revision != generation.get()) return@onFailure
                        val native = error as? MicroscopeSimilarityException
                        _state.value = MicroscopeSimilarityUiState.Error(
                            sessionId = sessionId,
                            targetFrameId = targetFrameId,
                            code = native?.code,
                            message = error.message ?: "FrameScope could not find similar frames.",
                        )
                    }
            } catch (cancelled: CancellationException) {
                if (revision == generation.get()) {
                    _state.value = MicroscopeSimilarityUiState.Idle
                }
                throw cancelled
            }
        }
    }

    fun cancelSearch() {
        generation.incrementAndGet()
        repository.cancelActiveSearch()
        cancelSearchJobOnly()
        _state.value = MicroscopeSimilarityUiState.Idle
    }

    fun onSessionChanged(sessionId: Long?) {
        val stateSession = when (val current = _state.value) {
            is MicroscopeSimilarityUiState.Searching -> current.sessionId
            is MicroscopeSimilarityUiState.Ready -> current.result.sessionId
            is MicroscopeSimilarityUiState.Error -> current.sessionId
            MicroscopeSimilarityUiState.Idle -> null
        }
        if (stateSession != null && stateSession != sessionId) {
            cancelSearch()
        }
    }

    fun dismissStatus() {
        if (_state.value !is MicroscopeSimilarityUiState.Searching) {
            _state.value = MicroscopeSimilarityUiState.Idle
        }
    }

    private fun cancelSearchJobOnly() {
        searchJob?.cancel()
        searchJob = null
    }

    override fun onCleared() {
        generation.incrementAndGet()
        repository.cancelActiveSearch()
        cancelSearchJobOnly()
        super.onCleared()
    }
}

internal class MicroscopeSimilarityViewModelFactory(
    private val repository: MicroscopeSimilarityRepository,
) : ViewModelProvider.Factory {
    @Suppress("UNCHECKED_CAST")
    override fun <T : ViewModel> create(modelClass: Class<T>): T {
        require(modelClass.isAssignableFrom(MicroscopeSimilarityViewModel::class.java))
        return MicroscopeSimilarityViewModel(repository) as T
    }
}
