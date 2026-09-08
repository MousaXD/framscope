package com.framescope.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp

/** Compatibility overload for shell tests and non-live call sites retained from the Wave 1 shell PR. */
@Composable
internal fun MicroscopePanel(
    state: MicroscopeUiState,
    timelineBounds: IndexedTimelineBounds?,
    rangeSelection: TimelineRangeSelection?,
    onStep: (Int) -> Unit,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
    onCommitRange: (Long, Long) -> Unit,
    onClearRange: () -> Unit,
    onDismissError: () -> Unit,
    compactWorkspace: Boolean = false,
) {
    MicroscopePanel(
        state = state,
        timelineBounds = timelineBounds,
        rangeSelection = rangeSelection,
        scrubPreview = null,
        onStep = onStep,
        onJumpFrame = onJumpFrame,
        onJumpTimestampUs = onJumpTimestampUs,
        onPreviewFrame = {},
        onPreviewTimestampUs = {},
        onFinishScrubFrame = onJumpFrame,
        onFinishScrubTimestampUs = onJumpTimestampUs,
        onCommitRange = onCommitRange,
        onClearRange = onClearRange,
        onDismissError = onDismissError,
    )
}

@Composable
internal fun MicroscopeInspectorPanel(
    state: MicroscopeUiState,
    onStep: (Int) -> Unit,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
) {
    val session = when (state) {
        is MicroscopeUiState.LoadingFrame -> state.session
        is MicroscopeUiState.Navigating -> state.session
        is MicroscopeUiState.Ready -> state.session
        is MicroscopeUiState.Empty -> state.session
        is MicroscopeUiState.Error -> state.session
        else -> null
    }
    val enabled = state is MicroscopeUiState.Ready
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                text = "Frame timing & navigation",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )
            val frame = session?.currentFrame
            if (session == null || frame == null) {
                Text("No authoritative frame is available yet.")
            } else {
                Text("Frame ${frame.frameId + 1} / ${session.frameCount}")
                Text(
                    frame.timestampUs?.let { "Timestamp ${MicroscopePreviewMath.formatTimestampUs(it)} ($it µs)" }
                        ?: "Timestamp unavailable",
                )
                Text("PTS ${MicroscopeUiFormatter.exactTimestamp(frame)}")
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    OutlinedButton(
                        onClick = { onStep(-1) },
                        enabled = enabled && session.canStepPrevious,
                        modifier = Modifier.weight(1f),
                    ) { Text("Previous") }
                    Button(
                        onClick = { onStep(1) },
                        enabled = enabled && session.canStepNext,
                        modifier = Modifier.weight(1f),
                    ) { Text("Next") }
                }
                Text(
                    text = "Exact frame and timestamp jumps remain available in the workspace controls.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}
