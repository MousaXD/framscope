package com.framescope.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.framescope.app.data.RecentVideoAvailability
import com.framescope.app.data.RecentVideoRecord
import com.framescope.app.data.VideoUriPermissionStatus
import java.text.DateFormat
import java.util.Date

@Composable
fun HistoryScreen(
    state: HistoryUiState,
    onRefresh: () -> Unit,
    onOpen: (RecentVideoRecord) -> Unit,
    onReselect: (RecentVideoRecord) -> Unit,
    onRemove: (String) -> Unit,
    onClear: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier = modifier
            .fillMaxSize()
            .padding(horizontal = 20.dp, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Column {
                Text(
                    text = "History",
                    style = MaterialTheme.typography.headlineMedium,
                    fontWeight = FontWeight.SemiBold,
                )
                Text(
                    text = "Recent videos stay on this device.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            if (state is HistoryUiState.Ready && state.entries.isNotEmpty()) {
                TextButton(onClick = onClear) {
                    Text("Clear history")
                }
            }
        }

        when (state) {
            HistoryUiState.Loading -> {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .weight(1f),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.Center,
                ) {
                    CircularProgressIndicator(modifier = Modifier.testTag("history-loading"))
                    Spacer(Modifier.height(12.dp))
                    Text("Checking recent videos…")
                }
            }

            is HistoryUiState.Error -> {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .weight(1f),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.Center,
                ) {
                    Text(
                        text = state.message,
                        style = MaterialTheme.typography.bodyLarge,
                        color = MaterialTheme.colorScheme.error,
                    )
                    Spacer(Modifier.height(12.dp))
                    OutlinedButton(onClick = onRefresh) {
                        Text("Try again")
                    }
                }
            }

            is HistoryUiState.Ready -> {
                if (state.entries.isEmpty()) {
                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .weight(1f),
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.Center,
                    ) {
                        Text(
                            text = "No recent videos yet",
                            style = MaterialTheme.typography.titleLarge,
                        )
                        Spacer(Modifier.height(6.dp))
                        Text(
                            text = "Videos you open will appear here with their last viewed position.",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                } else {
                    LazyColumn(
                        modifier = Modifier
                            .fillMaxWidth()
                            .weight(1f),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        items(
                            items = state.entries,
                            key = RecentVideoRecord::id,
                        ) { record ->
                            RecentVideoCard(
                                record = record,
                                onOpen = { onOpen(record) },
                                onReselect = { onReselect(record) },
                                onRemove = { onRemove(record.id) },
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun RecentVideoCard(
    record: RecentVideoRecord,
    onOpen: () -> Unit,
    onReselect: () -> Unit,
    onRemove: () -> Unit,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("history-item-${record.id}"),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                text = record.displayName,
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                text = metadataSummary(record),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                text = "Last opened ${formatLastOpened(record.lastOpenedEpochMs)}",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            record.lastViewedTimestampUs?.let { timestampUs ->
                Text(
                    text = "Resume at ${formatDurationUs(timestampUs)}",
                    style = MaterialTheme.typography.bodySmall,
                )
            }

            if (record.availability != RecentVideoAvailability.Available) {
                HorizontalDivider()
                Text(
                    text = unavailableMessage(record),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.error,
                )
            } else if (record.permissionStatus != VideoUriPermissionStatus.Persisted) {
                Text(
                    text = "Access is not permanently retained and may need to be selected again later.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }

            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (record.canOpen()) {
                    Button(onClick = onOpen) {
                        Text(if (record.lastViewedTimestampUs != null) "Resume" else "Open")
                    }
                } else {
                    Button(onClick = onReselect) {
                        Text("Reselect")
                    }
                }
                TextButton(onClick = onRemove) {
                    Text("Remove")
                }
            }
        }
    }
}

private fun metadataSummary(record: RecentVideoRecord): String {
    val pieces = buildList {
        record.durationUs?.let { add(formatDurationUs(it)) }
        if (record.width > 0 && record.height > 0) add("${record.width}×${record.height}")
        record.codec?.takeIf(String::isNotBlank)?.let(::add)
        record.container?.takeIf(String::isNotBlank)?.let(::add)
    }
    return pieces.joinToString(" · ").ifBlank { "Video" }
}

private fun unavailableMessage(record: RecentVideoRecord): String = when (record.availability) {
    RecentVideoAvailability.Available -> ""
    RecentVideoAvailability.PermissionLost ->
        "FrameScope no longer has permission to read this video. Reselect it to restore access."
    RecentVideoAvailability.MissingDocument ->
        "This video is no longer available at its saved location. Reselect it or remove this entry."
}

internal fun formatDurationUs(durationUs: Long): String {
    val totalSeconds = (durationUs.coerceAtLeast(0L) / 1_000_000L)
    val hours = totalSeconds / 3_600L
    val minutes = (totalSeconds % 3_600L) / 60L
    val seconds = totalSeconds % 60L
    return if (hours > 0L) {
        "%d:%02d:%02d".format(hours, minutes, seconds)
    } else {
        "%d:%02d".format(minutes, seconds)
    }
}

private fun formatLastOpened(epochMs: Long): String =
    DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(epochMs))
