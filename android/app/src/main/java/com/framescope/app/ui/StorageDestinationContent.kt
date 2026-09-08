package com.framescope.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.framescope.app.data.AndroidFrameScopeStorageRepository

/**
 * Shell-facing Storage destination body. The app shell owns navigation; this surface owns only
 * FrameScope storage state and actions.
 */
@Composable
fun StorageDestinationContent(
    cacheRoot: String,
    modifier: Modifier = Modifier,
) {
    val storageViewModel: StorageViewModel = viewModel(
        factory = StorageViewModelFactory(
            AndroidFrameScopeStorageRepository(cacheRoot = cacheRoot),
        ),
    )
    val state by storageViewModel.state.collectAsStateWithLifecycle()

    StorageSettingsContent(
        state = state,
        onRefresh = storageViewModel::refresh,
        onRequestClear = storageViewModel::requestClear,
        onConfirmClear = storageViewModel::confirmClear,
        onDismissClear = storageViewModel::dismissClearConfirmation,
        onDismissStatus = storageViewModel::dismissStatus,
        modifier = modifier,
    )
}

/** Read-only Home summary for Agent 1's storage-summary integration slot. */
@Composable
fun StorageSummaryContent(
    cacheRoot: String,
    modifier: Modifier = Modifier,
) {
    val storageViewModel: StorageViewModel = viewModel(
        factory = StorageViewModelFactory(
            AndroidFrameScopeStorageRepository(cacheRoot = cacheRoot),
        ),
    )
    val state by storageViewModel.state.collectAsStateWithLifecycle()

    Card(
        modifier = modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainer),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(
                text = "Storage",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )
            val storage = state.storage
            when {
                state.loading && storage == null -> Text(
                    text = "Calculating FrameScope cache usage…",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                storage != null -> {
                    Text(
                        text = formatStorageBytes(storage.totalBytes),
                        style = MaterialTheme.typography.headlineSmall,
                    )
                    Text(
                        text = buildString {
                            append("${storage.indexedSources} indexed ")
                            append(if (storage.indexedSources == 1L) "video" else "videos")
                            append(" · ")
                            append(
                                if (storage.previewProxyEnabled) {
                                    "disk previews enabled"
                                } else {
                                    "disk previews disabled"
                                },
                            )
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                state.error != null -> Text(
                    text = state.error,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.error,
                )
                else -> Text(
                    text = "Storage summary unavailable.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}
