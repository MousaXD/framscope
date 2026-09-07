package com.framescope.app.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.framescope.app.data.FrameScopeRepository
import com.framescope.app.data.InspectedVideo
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

data class FrameScopeUiState(
    val engineVersion: String? = null,
    val engineError: String? = null,
    val isInspecting: Boolean = false,
    val video: InspectedVideo? = null,
    val errorMessage: String? = null,
)

class MainViewModel(
    private val repository: FrameScopeRepository,
) : ViewModel() {
    private val _uiState = MutableStateFlow(FrameScopeUiState())
    val uiState: StateFlow<FrameScopeUiState> = _uiState.asStateFlow()

    private var inspectJob: Job? = null

    init {
        viewModelScope.launch {
            repository.engineVersion()
                .onSuccess { version -> _uiState.update { it.copy(engineVersion = version, engineError = null) } }
                .onFailure { error ->
                    _uiState.update {
                        it.copy(engineError = error.message ?: "Rust engine is unavailable.")
                    }
                }
        }
    }

    fun onVideoSelected(uri: String) {
        inspectJob?.cancel()
        inspectJob = viewModelScope.launch {
            _uiState.update {
                it.copy(isInspecting = true, video = null, errorMessage = null)
            }
            repository.inspect(uri)
                .onSuccess { video ->
                    _uiState.update {
                        it.copy(
                            isInspecting = false,
                            video = video,
                            errorMessage = null,
                            engineVersion = video.engine,
                            engineError = null,
                        )
                    }
                }
                .onFailure { error ->
                    _uiState.update {
                        it.copy(
                            isInspecting = false,
                            errorMessage = error.message ?: "Could not inspect this video.",
                        )
                    }
                }
        }
    }

    fun onPickerCancelled() {
        _uiState.update { it.copy(isInspecting = false) }
    }

    fun clearError() {
        _uiState.update { it.copy(errorMessage = null) }
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
