package com.framescope.app.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.framescope.app.data.FrameScopeStorageRepository
import com.framescope.app.data.FrameScopeStorageStats
import com.framescope.app.data.StorageClearReceipt
import com.framescope.app.data.StorageClearScope
import com.framescope.app.data.StorageOperationException
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

data class StorageUiState(
    val storage: FrameScopeStorageStats? = null,
    val loading: Boolean = false,
    val clearingScope: StorageClearScope? = null,
    val pendingClear: StorageClearScope? = null,
    val message: String? = null,
    val error: String? = null,
)

class StorageViewModel(
    private val repository: FrameScopeStorageRepository,
) : ViewModel() {
    private val _state = MutableStateFlow(StorageUiState())
    val state: StateFlow<StorageUiState> = _state.asStateFlow()

    private val generation = AtomicLong(0L)
    private var work: Job? = null

    init {
        refresh()
    }

    fun refresh() {
        if (_state.value.clearingScope != null) return
        val revision = generation.incrementAndGet()
        work?.cancel()
        _state.value = _state.value.copy(
            loading = true,
            message = null,
            error = null,
        )
        work = viewModelScope.launch {
            try {
                repository.stats()
                    .onSuccess { storage ->
                        if (revision == generation.get()) {
                            _state.value = _state.value.copy(
                                storage = storage,
                                loading = false,
                                error = null,
                            )
                        }
                    }
                    .onFailure { failure ->
                        if (revision == generation.get()) {
                            _state.value = _state.value.copy(
                                loading = false,
                                error = failure.userMessage(),
                            )
                        }
                    }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Throwable) {
                if (revision == generation.get()) {
                    _state.value = _state.value.copy(
                        loading = false,
                        error = failure.userMessage(),
                    )
                }
            }
        }
    }

    fun requestClear(scope: StorageClearScope) {
        if (_state.value.clearingScope != null) return
        _state.value = _state.value.copy(
            pendingClear = scope,
            message = null,
            error = null,
        )
    }

    fun dismissClearConfirmation() {
        if (_state.value.clearingScope != null) return
        _state.value = _state.value.copy(pendingClear = null)
    }

    fun confirmClear() {
        val scope = _state.value.pendingClear ?: return
        if (_state.value.clearingScope != null) return
        val revision = generation.incrementAndGet()
        work?.cancel()
        _state.value = _state.value.copy(
            loading = false,
            clearingScope = scope,
            pendingClear = null,
            message = null,
            error = null,
        )
        work = viewModelScope.launch {
            try {
                repository.clear(scope)
                    .onSuccess { result ->
                        if (revision == generation.get()) {
                            _state.value = _state.value.copy(
                                storage = result.storage,
                                clearingScope = null,
                                message = result.receipt.successMessage(),
                                error = null,
                            )
                        }
                    }
                    .onFailure { failure ->
                        if (revision == generation.get()) {
                            _state.value = _state.value.copy(
                                clearingScope = null,
                                error = failure.userMessage(),
                            )
                        }
                    }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Throwable) {
                if (revision == generation.get()) {
                    _state.value = _state.value.copy(
                        clearingScope = null,
                        error = failure.userMessage(),
                    )
                }
            }
        }
    }

    fun dismissStatus() {
        _state.value = _state.value.copy(message = null, error = null)
    }

    private fun StorageClearReceipt.successMessage(): String {
        val category = when (scope) {
            StorageClearScope.PreviewProxy -> "Preview cache"
            StorageClearScope.PersistentIndexes -> "Persistent indexes"
            StorageClearScope.Disposable -> "Disposable cache"
            StorageClearScope.All -> "FrameScope cache"
        }
        return if (clearedBytes == 0L) {
            "$category was already empty."
        } else {
            "$category cleared · ${formatStorageBytes(clearedBytes)} removed."
        }
    }

    private fun Throwable.userMessage(): String = when (this) {
        is StorageOperationException -> message ?: "FrameScope storage operation failed."
        else -> message ?: "FrameScope storage operation failed."
    }
}

class StorageViewModelFactory(
    private val repository: FrameScopeStorageRepository,
) : ViewModelProvider.Factory {
    @Suppress("UNCHECKED_CAST")
    override fun <T : ViewModel> create(modelClass: Class<T>): T {
        require(modelClass.isAssignableFrom(StorageViewModel::class.java))
        return StorageViewModel(repository) as T
    }
}

fun formatStorageBytes(bytes: Long): String {
    require(bytes >= 0L) { "storage bytes must not be negative" }
    if (bytes < 1_024L) return "$bytes B"

    val units = arrayOf("KiB", "MiB", "GiB", "TiB")
    var value = bytes.toDouble()
    var unitIndex = -1
    while (value >= 1_024.0 && unitIndex < units.lastIndex) {
        value /= 1_024.0
        unitIndex += 1
    }
    val rounded = if (value >= 10.0 || value % 1.0 == 0.0) {
        "%.0f".format(java.util.Locale.US, value)
    } else {
        "%.1f".format(java.util.Locale.US, value)
    }
    return "$rounded ${units[unitIndex]}"
}
