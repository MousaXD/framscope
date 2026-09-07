package com.framescope.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.framescope.app.data.MicroscopeSessionSnapshot

@Composable
internal fun MicroscopeTimelineControls(
    session: MicroscopeSessionSnapshot,
    enabled: Boolean,
    onJumpFrame: (Long) -> Unit,
) {
    val currentFrame = session.currentFrame ?: return
    var scrubFraction by rememberSaveable(session.sessionId) {
        mutableStateOf(
            MicroscopeTimelineMath.fractionForFrame(
                frameId = currentFrame.frameId,
                frameCount = session.frameCount,
            ),
        )
    }
    // A drag gesture cannot survive disposal/recreation. Persisting this flag could suppress
    // authoritative frame synchronization after recreation even though no gesture is active.
    var scrubbing by remember(session.sessionId) { mutableStateOf(false) }

    LaunchedEffect(currentFrame.frameId, session.frameCount, scrubbing) {
        if (!scrubbing) {
            scrubFraction = MicroscopeTimelineMath.fractionForFrame(
                frameId = currentFrame.frameId,
                frameCount = session.frameCount,
            )
        }
    }

    val previewFrameId = MicroscopeTimelineMath.frameForFraction(
        fraction = scrubFraction,
        frameCount = session.frameCount,
    ) ?: currentFrame.frameId
    val previewLabel = MicroscopeTimelineMath.framePositionLabel(
        frameId = previewFrameId,
        frameCount = session.frameCount,
    ) ?: "Indexed frame unavailable"

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(
            text = "Presentation-order scrub",
            style = MaterialTheme.typography.titleSmall,
        )
        Text(
            text = previewLabel,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Slider(
            value = scrubFraction,
            onValueChange = { fraction ->
                scrubbing = true
                scrubFraction = fraction.coerceIn(0f, 1f)
            },
            onValueChangeFinished = {
                val target = MicroscopeTimelineMath.frameForFraction(
                    fraction = scrubFraction,
                    frameCount = session.frameCount,
                )
                scrubbing = false
                if (target != null && target != currentFrame.frameId) {
                    onJumpFrame(target)
                }
            },
            enabled = enabled && session.frameCount > 1L,
            valueRange = 0f..1f,
            modifier = Modifier.fillMaxWidth(),
        )
        Text(
            text = "Dragging is local. Releasing performs one indexed frame jump; FPS is never used to infer position.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )

        Text(
            text = "Rapid indexed stepping",
            style = MaterialTheme.typography.titleSmall,
        )
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            RapidStepButton(
                label = "−100",
                delta = -100L,
                session = session,
                enabled = enabled,
                onJumpFrame = onJumpFrame,
                modifier = Modifier.weight(1f),
            )
            RapidStepButton(
                label = "−10",
                delta = -10L,
                session = session,
                enabled = enabled,
                onJumpFrame = onJumpFrame,
                modifier = Modifier.weight(1f),
            )
            RapidStepButton(
                label = "+10",
                delta = 10L,
                session = session,
                enabled = enabled,
                onJumpFrame = onJumpFrame,
                modifier = Modifier.weight(1f),
            )
            RapidStepButton(
                label = "+100",
                delta = 100L,
                session = session,
                enabled = enabled,
                onJumpFrame = onJumpFrame,
                modifier = Modifier.weight(1f),
            )
        }
    }
}

@Composable
private fun RapidStepButton(
    label: String,
    delta: Long,
    session: MicroscopeSessionSnapshot,
    enabled: Boolean,
    onJumpFrame: (Long) -> Unit,
    modifier: Modifier = Modifier,
) {
    val currentFrameId = session.currentFrame?.frameId
    val target = currentFrameId?.let {
        MicroscopeTimelineMath.boundedStepTarget(
            currentFrameId = it,
            frameCount = session.frameCount,
            delta = delta,
        )
    }
    OutlinedButton(
        onClick = { target?.let(onJumpFrame) },
        enabled = enabled && target != null && target != currentFrameId,
        modifier = modifier,
    ) {
        Text(label)
    }
}
