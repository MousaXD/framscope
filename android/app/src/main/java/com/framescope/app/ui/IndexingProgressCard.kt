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
import kotlin.math.floor
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
                        text = progress?.let(::stageTitle) ?: "Preparing video…",
                        style = MaterialTheme.typography.titleMedium,
                        fontWeight = FontWeight.SemiBold,
                    )
                    Text(
                        text = progress?.stage?.detail()
                            ?: "Reading media information before frame indexing begins.",
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
                        text = if (current.stage == MicroscopeIndexingStage.Indexing) {
                            "${current.indexedFrames} frames indexed"
                        } else {
                            "Index currently contains ${current.indexedFrames} frames"
                        },
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }

                current.mediaTimelineFraction?.let { fraction ->
                    Text(
                        text = "Media timeline covered: ${displayProgressPercent(fraction)}%",
                        style = MaterialTheme.typography.bodyMedium,
                        fontWeight = FontWeight.Medium,
                    )
                }

                if (!current.telemetryAvailable) {
                    Text(
                        text = "Progress details temporarily unavailable",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                } else {
                    current.framesPerSecond?.takeIf { it.isFinite() && it > 0.0 }?.let { rate ->
                        Text(
                            text = "Observed indexing rate: ${formatRate(rate)} frames/s",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }

                    if (current.stage == MicroscopeIndexingStage.Indexing) {
                        val etaText = current.estimatedRemainingRange?.let(::formatRemainingRange)
                            ?: "Estimating time remaining…"
                        Text(
                            text = etaText,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }

                if (
                    current.stage == MicroscopeIndexingStage.Indexing &&
                    current.currentTimestampUs != null
                ) {
                    Text(
                        text = "Current decoded timestamp: ${MicroscopePreviewMath.formatTimestampUs(current.currentTimestampUs)}",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
}

private fun stageTitle(progress: IndexingProgressUi): String {
    if (
        progress.stage == MicroscopeIndexingStage.ValidatingExistingIndex &&
        progress.stageProgressFraction != null
    ) {
        return "Validating saved index: ${displayProgressPercent(progress.stageProgressFraction)}%"
    }
    return progress.stage.title()
}

private fun MicroscopeIndexingStage.title(): String = when (this) {
    MicroscopeIndexingStage.ProbingMedia -> "Preparing video…"
    MicroscopeIndexingStage.CheckingExistingIndex -> "Checking saved index…"
    MicroscopeIndexingStage.ReusingExistingIndex -> "Reusing saved frame index"
    MicroscopeIndexingStage.ValidatingExistingIndex -> "Validating saved index…"
    MicroscopeIndexingStage.RebuildingIndex -> "Rebuilding frame index…"
    MicroscopeIndexingStage.Indexing -> "Indexing frames"
    MicroscopeIndexingStage.Finalizing -> "Finalizing frame index…"
}

private fun MicroscopeIndexingStage.detail(): String = when (this) {
    MicroscopeIndexingStage.ProbingMedia ->
        "Reading media information before frame indexing begins."
    MicroscopeIndexingStage.CheckingExistingIndex ->
        "Looking for a persistent index that matches this exact source."
    MicroscopeIndexingStage.ReusingExistingIndex ->
        "A matching persistent index was found."
    MicroscopeIndexingStage.ValidatingExistingIndex ->
        "Verifying saved frame identities before reuse."
    MicroscopeIndexingStage.RebuildingIndex ->
        "Saved progress cannot be trusted as-is, so FrameScope is rebuilding it safely."
    MicroscopeIndexingStage.Indexing ->
        "Traversing exact presentation timestamps. Navigation unlocks when the index is complete."
    MicroscopeIndexingStage.Finalizing ->
        "Saving the completed persistent index."
}

internal fun displayProgressPercent(fraction: Double): Int {
    val bounded = fraction.coerceIn(0.0, 1.0)
    return if (bounded >= 1.0) 100 else floor(bounded * 100.0).toInt()
}

private fun formatRate(rate: Double): String = when {
    rate >= 100.0 -> rate.roundToInt().toString()
    rate >= 10.0 -> ((rate * 10.0).roundToInt() / 10.0).toString()
    else -> ((rate * 100.0).roundToInt() / 100.0).toString()
}

internal fun formatRemainingRange(range: IndexingEtaRange): String {
    val minimum = range.minSeconds.coerceAtLeast(0L)
    val maximum = range.maxSeconds.coerceAtLeast(minimum)
    return when {
        maximum < 60L -> {
            val low = floorToBucket(minimum, 10L).coerceAtLeast(10L)
            val high = ceilToBucket(maximum, 10L).coerceAtLeast(low)
            if (low == high) "About ${high}s remaining" else "About ${low}–${high}s remaining"
        }
        maximum < 3_600L -> {
            val lowMinutes = (minimum / 60L).coerceAtLeast(1L)
            val highMinutes = ceilDiv(maximum, 60L).coerceAtLeast(lowMinutes)
            if (lowMinutes == highMinutes) {
                "About ${highMinutes} min remaining"
            } else {
                "About ${lowMinutes}–${highMinutes} min remaining"
            }
        }
        else -> {
            val lowHours = (minimum / 3_600L).coerceAtLeast(1L)
            val highHours = ceilDiv(maximum, 3_600L).coerceAtLeast(lowHours)
            if (lowHours == highHours) {
                "About ${highHours} hr remaining"
            } else {
                "About ${lowHours}–${highHours} hr remaining"
            }
        }
    }
}

private fun floorToBucket(value: Long, bucket: Long): Long = (value / bucket) * bucket

private fun ceilToBucket(value: Long, bucket: Long): Long = ceilDiv(value, bucket) * bucket

private fun ceilDiv(value: Long, divisor: Long): Long =
    if (value <= 0L) 0L else 1L + (value - 1L) / divisor
