package com.framescope.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.framescope.app.data.MicroscopeIndexingStage
import kotlin.math.roundToInt

@Composable
internal fun MicroscopeIndexingCard(progress: IndexingProgressUi?) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("microscope_indexing_progress"),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Row(
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                CircularProgressIndicator(
                    modifier = Modifier.size(24.dp),
                    strokeWidth = 2.dp,
                )
                Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text(
                        text = progress?.stage?.title() ?: "Preparing frame index",
                        style = MaterialTheme.typography.titleMedium,
                        fontWeight = FontWeight.SemiBold,
                    )
                    Text(
                        text = progress?.stage?.detail()
                            ?: "Reading exact presentation timing. Progress will appear as soon as the native indexer reports it.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }

            progress?.let { current ->
                if (
                    current.expectedReuseFrames > 0L &&
                    current.stage in setOf(
                        MicroscopeIndexingStage.ReusingExistingIndex,
                        MicroscopeIndexingStage.ValidatingExistingIndex,
                    )
                ) {
                    Text(
                        text = "${current.reusedFrames} / ${current.expectedReuseFrames} saved frames verified",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }

                if (current.indexedFrames > 0L) {
                    Text(
                        text = "Index contains ${current.indexedFrames} frames",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }

                current.estimatedFraction?.let { fraction ->
                    val percent = (fraction.coerceIn(0.0, 1.0) * 100.0).roundToInt()
                    Text(
                        text = "Estimated progress: $percent%",
                        style = MaterialTheme.typography.bodyMedium,
                        fontWeight = FontWeight.Medium,
                    )
                }

                current.framesPerSecond?.takeIf { it.isFinite() && it > 0.0 }?.let { rate ->
                    Text(
                        text = "Observed indexing rate: ${formatRate(rate)} frames/s",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }

                current.estimatedRemainingSeconds?.let { remaining ->
                    Text(
                        text = "Estimated time remaining: ${formatRemaining(remaining)}",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }

                if (
                    current.stage == MicroscopeIndexingStage.Indexing &&
                    current.currentTimestampUs != null
                ) {
                    Text(
                        text = "Indexed through ${MicroscopePreviewMath.formatTimestampUs(current.currentTimestampUs)}",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
}

private fun MicroscopeIndexingStage.title(): String = when (this) {
    MicroscopeIndexingStage.ProbingMedia -> "Opening media"
    MicroscopeIndexingStage.CheckingExistingIndex -> "Checking saved frame index"
    MicroscopeIndexingStage.ReusingExistingIndex -> "Reusing saved frame index"
    MicroscopeIndexingStage.ValidatingExistingIndex -> "Validating saved frame index"
    MicroscopeIndexingStage.RebuildingIndex -> "Rebuilding frame index"
    MicroscopeIndexingStage.Indexing -> "Indexing exact timestamps"
    MicroscopeIndexingStage.Finalizing -> "Finalizing frame index"
}

private fun MicroscopeIndexingStage.detail(): String = when (this) {
    MicroscopeIndexingStage.ProbingMedia ->
        "Reading stream metadata before indexing begins."
    MicroscopeIndexingStage.CheckingExistingIndex ->
        "Looking for a persistent index that matches this exact source."
    MicroscopeIndexingStage.ReusingExistingIndex ->
        "A matching persistent index was found."
    MicroscopeIndexingStage.ValidatingExistingIndex ->
        "Verifying saved frame identities before reuse."
    MicroscopeIndexingStage.RebuildingIndex ->
        "Saved progress cannot be trusted as-is, so FrameScope is rebuilding it safely."
    MicroscopeIndexingStage.Indexing ->
        "Traversing presentation timestamps. Exact navigation unlocks when the index is complete."
    MicroscopeIndexingStage.Finalizing ->
        "Committing the completed persistent index."
}

private fun formatRate(rate: Double): String = when {
    rate >= 100.0 -> rate.roundToInt().toString()
    rate >= 10.0 -> ((rate * 10.0).roundToInt() / 10.0).toString()
    else -> ((rate * 100.0).roundToInt() / 100.0).toString()
}

private fun formatRemaining(seconds: Long): String {
    val safe = seconds.coerceAtLeast(0L)
    if (safe < 60L) return "about ${safe}s"
    val minutes = safe / 60L
    val remainder = safe % 60L
    return if (minutes < 60L) {
        "about ${minutes}m ${remainder}s"
    } else {
        val hours = minutes / 60L
        val minuteRemainder = minutes % 60L
        "about ${hours}h ${minuteRemainder}m"
    }
}
