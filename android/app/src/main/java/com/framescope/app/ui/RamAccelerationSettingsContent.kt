package com.framescope.app.ui

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.framescope.app.data.RamAccelerationController
import com.framescope.app.data.RamAccelerationMode
import com.framescope.app.data.RamAccelerationPolicy
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import kotlin.math.roundToInt

@Composable
fun RamAccelerationSettingsContent(
    controller: RamAccelerationController,
    modifier: Modifier = Modifier,
) {
    val state by controller.state.collectAsStateWithLifecycle()

    LaunchedEffect(controller) {
        while (true) {
            withContext(Dispatchers.IO) {
                controller.refreshMetrics()
            }
            delay(1_500L)
        }
    }

    Card(
        modifier = modifier
            .fillMaxWidth()
            .testTag("ram-acceleration-card"),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainer),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                text = "RAM acceleration",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                text = "Keeps useful post-index source frames and scrub previews in native memory. Initial indexing does not populate these caches; this setting accelerates navigation after frames have been indexed.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )

            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                RamModeChip(
                    label = "Off",
                    selected = state.mode == RamAccelerationMode.Off,
                    onClick = { controller.setMode(RamAccelerationMode.Off) },
                )
                RamModeChip(
                    label = "Automatic",
                    selected = state.mode == RamAccelerationMode.Automatic,
                    onClick = { controller.setMode(RamAccelerationMode.Automatic) },
                )
                RamModeChip(
                    label = "Aggressive",
                    selected = state.mode == RamAccelerationMode.Aggressive,
                    onClick = { controller.setMode(RamAccelerationMode.Aggressive) },
                )
                RamModeChip(
                    label = "Custom",
                    selected = state.mode == RamAccelerationMode.Custom,
                    onClick = { controller.setMode(RamAccelerationMode.Custom) },
                )
            }

            Text(
                text = "Recommended ${state.recommendedTotalMiB} MiB · Aggressive ${state.aggressiveTotalMiB} MiB",
                style = MaterialTheme.typography.bodyMedium,
                fontWeight = FontWeight.Medium,
            )
            Text(
                text = "Requested ${state.requestedTotalMiB} MiB · Active ${state.activeTotalMiB} MiB",
                style = MaterialTheme.typography.bodyMedium,
                fontWeight = FontWeight.Medium,
            )
            Text(
                text = "Active split: source ${state.sourceCacheMiB} MiB · previews ${state.previewCacheMiB} MiB",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )

            if (state.mode == RamAccelerationMode.Custom) {
                Text(
                    text = "Custom budget: ${state.customTotalMiB} MiB · device-safe max ${state.customMaximumMiB} MiB",
                    style = MaterialTheme.typography.bodyMedium,
                )
                val rangeEnd = state.customMaximumMiB
                    .coerceAtLeast(RamAccelerationPolicy.MIN_CUSTOM_MIB)
                val stepCount = ((rangeEnd - RamAccelerationPolicy.MIN_CUSTOM_MIB) / 16 - 1)
                    .coerceAtLeast(0)
                Slider(
                    value = state.customTotalMiB.coerceIn(
                        RamAccelerationPolicy.MIN_CUSTOM_MIB,
                        rangeEnd,
                    ).toFloat(),
                    onValueChange = { raw ->
                        val stepped = (raw / 16f).roundToInt() * 16
                        controller.setCustomTotalMiB(stepped)
                    },
                    valueRange = RamAccelerationPolicy.MIN_CUSTOM_MIB.toFloat()..rangeEnd.toFloat(),
                    steps = stepCount,
                    modifier = Modifier.testTag("ram-custom-slider"),
                )
            }

            val metrics = state.metrics
            if (metrics != null) {
                Text(
                    text = "Resident cache payload: ${formatRamBytes(metrics.residentBytes)}",
                    style = MaterialTheme.typography.bodyMedium,
                    fontWeight = FontWeight.SemiBold,
                )
                Text(
                    text = "Source: ${formatRamBytes(metrics.source.residentBytes)} in ${metrics.source.residentFrames} frames · ${formatHits(metrics.source.hits, metrics.source.misses)}",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    text = "Previews: ${formatRamBytes(metrics.preview.residentBytes)} in ${metrics.preview.residentFrames} frames · ${formatHits(metrics.preview.hits, metrics.preview.misses)}",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    text = "Evictions: source ${metrics.source.evictions} · previews ${metrics.preview.evictions} · pressure reductions ${state.pressureReductionCount}",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    text = "Resident payload counts cache-owned RGBA bytes only; decoder buffers, JNI handoff buffers, Bitmaps, and the rest of the process are intentionally not disguised as cache residency.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                Text(
                    text = "Native cache residency metrics are unavailable.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }

            if (state.headroomLimited) {
                Text(
                    text = "Current Android memory headroom is below the requested budget, so FrameScope is using a smaller active ceiling without changing your saved preference.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.tertiary,
                )
            }
            if (state.underMemoryPressure) {
                Text(
                    text = "Memory pressure reduced the active cache ceiling by ${state.pressureReductionPercent}%. Cached pixels are evicted immediately when the ceiling shrinks and can grow lazily again after pressure clears.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.tertiary,
                )
            }
            if (state.lowRamDevice) {
                Text(
                    text = "Android marks this as a low-RAM device, so Automatic and Aggressive stay deliberately conservative.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Text(
                text = "Android reports ${state.availableMemoryMiB} MiB currently available · managed heap class ${state.managedHeapClassMiB} MiB. Native cache policy preserves headroom instead of treating the Java heap class as the Rust cache limit.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (!state.nativeApplied) {
                Text(
                    text = "RAM acceleration could not be applied to the native engine. FrameScope will continue without relying on the requested cache budget.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.error,
                )
            }
        }
    }
}

@Composable
private fun RamModeChip(
    label: String,
    selected: Boolean,
    onClick: () -> Unit,
) {
    FilterChip(
        selected = selected,
        onClick = onClick,
        label = { Text(label) },
        modifier = Modifier.testTag("ram-mode-${label.lowercase()}"),
    )
}

private fun formatHits(hits: Long, misses: Long): String {
    val total = if (Long.MAX_VALUE - hits < misses) Long.MAX_VALUE else hits + misses
    if (total <= 0L) return "no cache requests yet"
    val percent = (hits.toDouble() * 100.0 / total.toDouble()).roundToInt()
    return "$percent% hit rate ($hits hit / $misses miss)"
}

private fun formatRamBytes(bytes: Long): String {
    if (bytes < RamAccelerationPolicy.MIB) {
        return "${bytes / 1024L} KiB"
    }
    val tenths = bytes * 10L / RamAccelerationPolicy.MIB
    return "${tenths / 10L}.${tenths % 10L} MiB"
}
