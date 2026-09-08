package com.framescope.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.weight
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import com.framescope.app.data.FrameScopeStorageStats
import com.framescope.app.data.StorageCategoryStats
import com.framescope.app.data.StorageClearScope

@Composable
fun StorageSettingsContent(
    state: StorageUiState,
    onRefresh: () -> Unit,
    onRequestClear: (StorageClearScope) -> Unit,
    onConfirmClear: () -> Unit,
    onDismissClear: () -> Unit,
    onDismissStatus: () -> Unit,
    modifier: Modifier = Modifier,
    historyStorage: StorageCategoryStats? = null,
) {
    Column(
        modifier = modifier
            .testTag("storage-settings")
            .verticalScroll(rememberScrollState())
            .padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text(
            text = "Storage & cache",
            style = MaterialTheme.typography.headlineSmall,
            fontWeight = FontWeight.SemiBold,
        )
        Text(
            text = "FrameScope-owned local data only. Video files and history records are never removed by these cache actions.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )

        if (state.loading) {
            LinearProgressIndicator(
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag("storage-loading"),
            )
        }

        state.storage?.let { storage ->
            StorageTotalCard(storage)

            StorageCategoryCard(
                title = "Persistent frame indexes",
                stats = storage.persistentIndexes,
                detail = indexedSourceDetail(storage.indexedSources),
                actionLabel = "Clear indexes",
                enabled = state.clearingScope == null,
                onAction = { onRequestClear(StorageClearScope.PersistentIndexes) },
            )

            StorageCategoryCard(
                title = "Preview / proxy cache",
                stats = storage.previewProxy,
                detail = if (storage.previewProxyEnabled) {
                    "Compressed preview data used for interactive navigation."
                } else {
                    "Disk previews disabled. The current native proxy budget is 0 B."
                },
                actionLabel = "Clear previews",
                enabled = state.clearingScope == null && storage.previewProxy.bytes > 0L,
                onAction = { onRequestClear(StorageClearScope.PreviewProxy) },
                testTag = "preview-cache-card",
            )

            StorageCategoryCard(
                title = "Disposable cache",
                stats = storage.disposable,
                detail = "Other removable data inside FrameScope's cache namespace.",
                actionLabel = "Clear disposable",
                enabled = state.clearingScope == null,
                onAction = { onRequestClear(StorageClearScope.Disposable) },
            )

            HistoryStorageCard(historyStorage)
        } ?: Text(
            text = if (state.loading) "Reading FrameScope storage…" else "Storage summary unavailable.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )

        state.clearingScope?.let { scope ->
            Card(
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag("storage-clearing"),
                colors = CardDefaults.cardColors(
                    containerColor = MaterialTheme.colorScheme.secondaryContainer,
                ),
            ) {
                Text(
                    text = "Clearing ${scope.displayName()}…",
                    modifier = Modifier.padding(16.dp),
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
        }

        state.message?.let { message ->
            StatusCard(
                message = message,
                isError = false,
                onDismiss = onDismissStatus,
            )
        }
        state.error?.let { error ->
            StatusCard(
                message = error,
                isError = true,
                onDismiss = onDismissStatus,
            )
        }

        HorizontalDivider()
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            OutlinedButton(
                onClick = onRefresh,
                enabled = !state.loading && state.clearingScope == null,
                modifier = Modifier.weight(1f),
            ) {
                Text("Refresh")
            }
            Button(
                onClick = { onRequestClear(StorageClearScope.All) },
                enabled = state.clearingScope == null,
                modifier = Modifier
                    .weight(1f)
                    .testTag("clear-all-cache"),
            ) {
                Text("Clear all cache")
            }
        }
    }

    state.pendingClear?.let { scope ->
        ClearConfirmationDialog(
            scope = scope,
            onConfirm = onConfirmClear,
            onDismiss = onDismissClear,
        )
    }
}

@Composable
fun StorageSettingsDialog(
    visible: Boolean,
    state: StorageUiState,
    onDismiss: () -> Unit,
    onRefresh: () -> Unit,
    onRequestClear: (StorageClearScope) -> Unit,
    onConfirmClear: () -> Unit,
    onDismissClear: () -> Unit,
    onDismissStatus: () -> Unit,
    historyStorage: StorageCategoryStats? = null,
) {
    if (!visible) return
    Dialog(onDismissRequest = onDismiss) {
        Surface(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(max = 720.dp),
            shape = RoundedCornerShape(24.dp),
            tonalElevation = 6.dp,
        ) {
            StorageSettingsContent(
                state = state,
                onRefresh = onRefresh,
                onRequestClear = onRequestClear,
                onConfirmClear = onConfirmClear,
                onDismissClear = onDismissClear,
                onDismissStatus = onDismissStatus,
                historyStorage = historyStorage,
            )
        }
    }
}

@Composable
private fun StorageTotalCard(storage: FrameScopeStorageStats) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("storage-total"),
        shape = RoundedCornerShape(18.dp),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.primaryContainer),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(
                text = "FrameScope storage",
                style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.onPrimaryContainer,
            )
            Text(
                text = formatStorageBytes(storage.totalBytes),
                style = MaterialTheme.typography.headlineMedium,
                fontWeight = FontWeight.SemiBold,
                color = MaterialTheme.colorScheme.onPrimaryContainer,
            )
        }
    }
}

@Composable
private fun StorageCategoryCard(
    title: String,
    stats: StorageCategoryStats,
    detail: String,
    actionLabel: String,
    enabled: Boolean,
    onAction: () -> Unit,
    testTag: String? = null,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .then(if (testTag != null) Modifier.testTag(testTag) else Modifier),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text(
                    text = title,
                    style = MaterialTheme.typography.titleMedium,
                    fontWeight = FontWeight.Medium,
                    modifier = Modifier.weight(1f),
                )
                Text(
                    text = formatStorageBytes(stats.bytes),
                    style = MaterialTheme.typography.titleMedium,
                    fontWeight = FontWeight.SemiBold,
                )
            }
            Text(
                text = detail,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                text = "${stats.files} ${plural(stats.files, "file", "files")}",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            OutlinedButton(
                onClick = onAction,
                enabled = enabled,
            ) {
                Text(actionLabel)
            }
        }
    }
}

@Composable
private fun HistoryStorageCard(historyStorage: StorageCategoryStats?) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Text(
                text = "History data",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.Medium,
            )
            if (historyStorage == null) {
                Text(
                    text = "History storage is managed separately. No history backend is present on this branch yet.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                Text(
                    text = formatStorageBytes(historyStorage.bytes),
                    style = MaterialTheme.typography.titleMedium,
                    fontWeight = FontWeight.SemiBold,
                )
                Text(
                    text = "Shown for visibility only. Cache clear actions do not delete history records.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun StatusCard(
    message: String,
    isError: Boolean,
    onDismiss: () -> Unit,
) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(
            containerColor = if (isError) {
                MaterialTheme.colorScheme.errorContainer
            } else {
                MaterialTheme.colorScheme.secondaryContainer
            },
        ),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                text = message,
                style = MaterialTheme.typography.bodyMedium,
                color = if (isError) {
                    MaterialTheme.colorScheme.onErrorContainer
                } else {
                    MaterialTheme.colorScheme.onSecondaryContainer
                },
            )
            TextButton(onClick = onDismiss) {
                Text("Dismiss")
            }
        }
    }
}

@Composable
private fun ClearConfirmationDialog(
    scope: StorageClearScope,
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Clear ${scope.displayName()}?") },
        text = {
            Text(
                when (scope) {
                    StorageClearScope.PersistentIndexes ->
                        "Frame indexes will be rebuilt when videos are opened again. The current video must be closed before clearing."
                    StorageClearScope.PreviewProxy ->
                        "Compressed preview cache data will be removed. Original videos are not affected."
                    StorageClearScope.Disposable ->
                        "Other removable FrameScope cache data will be deleted. Video files and history records are not affected."
                    StorageClearScope.All ->
                        "Persistent indexes, preview cache, and other disposable FrameScope cache data will be deleted. Video files and history records are not affected."
                },
            )
        },
        confirmButton = {
            Button(
                onClick = onConfirm,
                modifier = Modifier.testTag("confirm-storage-clear"),
            ) {
                Text("Clear")
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text("Cancel")
            }
        },
    )
}

private fun StorageClearScope.displayName(): String = when (this) {
    StorageClearScope.PreviewProxy -> "preview cache"
    StorageClearScope.PersistentIndexes -> "persistent indexes"
    StorageClearScope.Disposable -> "disposable cache"
    StorageClearScope.All -> "all FrameScope cache"
}

private fun indexedSourceDetail(indexedSources: Long): String = when (indexedSources) {
    0L -> "No indexed videos."
    1L -> "1 indexed video."
    else -> "$indexedSources indexed videos."
}

private fun plural(value: Long, singular: String, plural: String): String =
    if (value == 1L) singular else plural
