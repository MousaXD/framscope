package com.framescope.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.framescope.app.data.RecentVideoRecord

@Composable
fun RecentVideosHomeContent(
    state: HistoryUiState,
    onOpen: (RecentVideoRecord) -> Unit,
    modifier: Modifier = Modifier,
) {
    Card(
        modifier = modifier
            .fillMaxWidth()
            .testTag("home_recent_videos"),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainer),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                text = "Recent videos",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )
            when (state) {
                HistoryUiState.Loading -> Text(
                    text = "Loading recent sessions…",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                is HistoryUiState.Error -> Text(
                    text = state.message,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.error,
                )
                is HistoryUiState.Ready -> {
                    val recent = state.entries
                        .filter { it.contentUri != null }
                        .sortedByDescending(RecentVideoRecord::lastOpenedEpochMs)
                        .take(3)
                    if (recent.isEmpty()) {
                        Text(
                            text = "Videos you inspect will appear here for quick resume.",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    } else {
                        recent.forEach { record ->
                            Row(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .testTag("home_recent_video_${record.id}"),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(8.dp),
                            ) {
                                Column(modifier = Modifier.weight(1f)) {
                                    Text(
                                        text = record.displayName,
                                        style = MaterialTheme.typography.bodyLarge,
                                        maxLines = 1,
                                        overflow = TextOverflow.Ellipsis,
                                    )
                                    Text(
                                        text = record.lastViewedTimestampUs?.let {
                                            "Resume at ${formatDurationUs(it)}"
                                        } ?: "Start from the beginning",
                                        style = MaterialTheme.typography.bodySmall,
                                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    )
                                }
                                if (record.canOpen()) {
                                    TextButton(onClick = { onOpen(record) }) {
                                        Text(if (record.lastViewedTimestampUs != null) "Resume" else "Open")
                                    }
                                } else {
                                    Text(
                                        text = "Needs access",
                                        style = MaterialTheme.typography.labelMedium,
                                        color = MaterialTheme.colorScheme.error,
                                    )
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
