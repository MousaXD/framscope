package com.framescope.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.framescope.app.data.RamAccelerationController
import com.framescope.app.data.RamAccelerationMode
import com.framescope.app.data.RamAccelerationPolicy
import kotlin.math.roundToInt

@Composable
fun RamAccelerationSettingsContent(
    controller: RamAccelerationController,
    modifier: Modifier = Modifier,
) {
    val state by controller.state.collectAsStateWithLifecycle()

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
                text = "Keeps recently decoded indexed frames in memory for faster scrub and exact navigation. Automatic is recommended for this device.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )

            Row(
                modifier = Modifier.fillMaxWidth(),
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
                    label = "Custom",
                    selected = state.mode == RamAccelerationMode.Custom,
                    onClick = { controller.setMode(RamAccelerationMode.Custom) },
                )
            }

            Text(
                text = "Recommended: ${state.recommendedTotalMiB} MiB · Active: ${state.activeTotalMiB} MiB",
                style = MaterialTheme.typography.bodyMedium,
                fontWeight = FontWeight.Medium,
            )
            Text(
                text = "Source frames ${state.sourceCacheMiB} MiB · previews ${state.previewCacheMiB} MiB",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )

            if (state.mode == RamAccelerationMode.Custom) {
                Text(
                    text = "Custom budget: ${state.customTotalMiB} MiB",
                    style = MaterialTheme.typography.bodyMedium,
                )
                Slider(
                    value = state.customTotalMiB.toFloat(),
                    onValueChange = { raw ->
                        val stepped = (raw / 16f).roundToInt() * 16
                        controller.setCustomTotalMiB(stepped)
                    },
                    valueRange = RamAccelerationPolicy.MIN_CUSTOM_MIB.toFloat()..
                        RamAccelerationPolicy.MAX_CUSTOM_MIB.toFloat(),
                    steps = 30,
                    modifier = Modifier.testTag("ram-custom-slider"),
                )
            }

            if (state.underMemoryPressure) {
                Text(
                    text = "Android reported memory pressure. FrameScope temporarily reduced the active RAM budget by 50%.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.tertiary,
                )
            }
            if (state.lowRamDevice) {
                Text(
                    text = "Android marks this as a low-RAM device, so Automatic uses a conservative 32 MiB ceiling.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
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
