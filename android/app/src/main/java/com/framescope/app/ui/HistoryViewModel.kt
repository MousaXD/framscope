package com.framescope.app.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.framescope.app.data.RecentVideoHistory
import com.framescope.app.data.RecentVideoRecord
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

sealed interface HistoryUiState {
    data object Loading : HistoryUiState

    data class Ready(
        val entries: List<RecentVideoRecord>,
    ) : HistoryUiState

    data class Error(
        val message: String,
    ) : HistoryUiState
}

class HistoryViewModel(
    private val history: RecentVideoHistory,
) : ViewModel() {
    private val _state = MutableStateFlow<HistoryUiState>(HistoryUiState.Loading)
    val state: StateFlow<HistoryUiState> = _state.asStateFlow()

    init {
        refresh()
    }

    fun refresh() {
        viewModelScope.launch {
            _state.value = HistoryUiState.Loading
            try {
                _state.value = HistoryUiState.Ready(history.entries(refreshAccess = true))
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Exception) {
                _state.value = HistoryUiState.Error(
                    error.message ?: "FrameScope could not load recent videos.",
                )
            }
        }
    }

    fun remove(recordId: String) {
        viewModelScope.launch {
            try {
                history.remove(recordId)
                _state.value = HistoryUiState.Ready(history.entries(refreshAccess = true))
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Exception) {
                _state.value = HistoryUiState.Error(
                    error.message ?: "FrameScope could not remove this recent video.",
                )
            }
        }
    }

    fun clear() {
        viewModelScope.launch {
            try {
                history.clear()
                _state.value = HistoryUiState.Ready(emptyList())
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Exception) {
                _state.value = HistoryUiState.Error(
                    error.message ?: "FrameScope could not clear recent videos.",
                )
            }
        }
    }
}

class HistoryViewModelFactory(
    private val history: RecentVideoHistory,
) : ViewModelProvider.Factory {
    @Suppress("UNCHECKED_CAST")
    override fun <T : ViewModel> create(modelClass: Class<T>): T {
        require(modelClass.isAssignableFrom(HistoryViewModel::class.java))
        return HistoryViewModel(history) as T
    }
}
