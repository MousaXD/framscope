package com.framescope.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.RangeSlider
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.framescope.app.data.MicroscopeSessionSnapshot

internal const val TIMELINE_SLIDER_TAG = "microscope_timeline_slider"
internal const val TIMELINE_RANGE_SLIDER_TAG = "microscope_timeline_range_slider"

@Composable
internal fun MicroscopeTimelineControls(
    session: MicroscopeSessionSnapshot,
    timelineBounds: IndexedTimelineBounds?,
    rangeSelection: TimelineRangeSelection?,
    enabled: Boolean,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
    onPreviewFrame: (Long) -> Unit,
    onPreviewTimestampUs: (Long) -> Unit,
    onFinishScrubFrame: (Long) -> Unit,
    onFinishScrubTimestampUs: (Long) -> Unit,
    onCommitRange: (Long, Long) -> Unit,
    onClearRange: () -> Unit,
) {
    val currentFrame = session.currentFrame ?: return
    val bounds = timelineBounds?.takeIf {
        it.sessionId == session.sessionId && it.isSane()
    }
    val committedRange = rangeSelection?.takeIf { selection ->
        bounds?.let(selection::isSaneFor) == true
    }

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        // Keep high-frequency gesture state below this composition boundary. Range controls and
        // rapid-step actions do not need to recompose for every pointer sample.
        MicroscopeTimelineScrubber(
            session = session,
            bounds = bounds,
            currentFrameTimestampUs = currentFrame.timestampUs,
            currentFrameId = currentFrame.frameId,
            enabled = enabled,
            onPreviewFrame = onPreviewFrame,
            onPreviewTimestampUs = onPreviewTimestampUs,
            onFinishScrubFrame = onFinishScrubFrame,
            onFinishScrubTimestampUs = onFinishScrubTimestampUs,
        )

        if (bounds != null) {
            val durationUs = MicroscopeTimelineMath.durationUs(bounds.startUs, bounds.endUs)
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text(
                    text = MicroscopePreviewMath.formatTimestampUs(bounds.startUs),
                    style = MaterialTheme.typography.labelMedium,
                )
                Text(
                    text = durationUs?.let {
                        "Duration ${MicroscopePreviewMath.formatTimestampUs(it)}"
                    } ?: "Duration unavailable",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    text = MicroscopePreviewMath.formatTimestampUs(bounds.endUs),
                    style = MaterialTheme.typography.labelMedium,
                )
            }
            Text(
                text = "Drag to preview indexed frames. Release to settle on the exact frame.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            TimelineRangeControls(
                sessionId = session.sessionId,
                bounds = bounds,
                committedRange = committedRange,
                enabled = enabled,
                onCommitRange = onCommitRange,
                onClearRange = onClearRange,
            )
        } else {
            Text(
                text = "Drag to preview by presentation order. Release to settle on the exact indexed frame.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }

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

@Suppress("UNUSED_PARAMETER")
@Composable
private fun MicroscopeTimelineScrubber(
    session: MicroscopeSessionSnapshot,
    bounds: IndexedTimelineBounds?,
    currentFrameTimestampUs: Long?,
    currentFrameId: Long,
    enabled: Boolean,
    onPreviewFrame: (Long) -> Unit,
    onPreviewTimestampUs: (Long) -> Unit,
    onFinishScrubFrame: (Long) -> Unit,
    onFinishScrubTimestampUs: (Long) -> Unit,
) {
    val authoritativeFraction = bounds?.let { indexedBounds ->
        currentFrameTimestampUs?.let { timestamp ->
            MicroscopeTimelineMath.fractionForTimestamp(
                timestampUs = timestamp,
                startUs = indexedBounds.startUs,
                endUs = indexedBounds.endUs,
            )
        }
    } ?: MicroscopeTimelineMath.fractionForFrame(
        frameId = currentFrameId,
        frameCount = session.frameCount,
    )

    var scrubFraction by rememberSaveable(session.sessionId) {
        mutableStateOf(authoritativeFraction)
    }
    // A drag gesture cannot survive disposal/recreation. Persisting this flag could suppress
    // authoritative synchronization even though no gesture is active anymore.
    var scrubbing by remember(session.sessionId) { mutableStateOf(false) }
    val previewAdmissionPolicy = remember(session.sessionId) { LiveScrubAdmissionPolicy() }
    val delayedPreviewAdmission = remember(session.sessionId) { DelayedScrubPreviewAdmission() }
    val thumbDrawTracker = remember(session.sessionId) { ScrubThumbDrawTracker() }
    val previewScope = rememberCoroutineScope()

    DisposableEffect(session.sessionId) {
        onDispose {
            delayedPreviewAdmission.cancel()
            previewAdmissionPolicy.reset()
            ScrubUxTelemetry.cancelExactSettle(session.sessionId)
        }
    }

    // Exact-settle timing starts in the release callback. The navigating composition is disabled,
    // so only the next enabled authoritative Ready composition can close that sample.
    SideEffect {
        if (enabled && !scrubbing) {
            ScrubUxTelemetry.completeExactSettle(session.sessionId)
        }
    }

    LaunchedEffect(authoritativeFraction, scrubbing) {
        if (!scrubbing) {
            scrubFraction = authoritativeFraction
            delayedPreviewAdmission.cancel()
            previewAdmissionPolicy.reset()
        }
    }

    fun admitPreview(
        fraction: Float,
        targetKey: Long,
        observedAtNanos: Long,
        action: () -> Unit,
    ) {
        when (
            val plan = previewAdmissionPolicy.plan(
                sessionId = session.sessionId,
                fraction = fraction,
                targetKey = targetKey,
                observedAtNanos = observedAtNanos,
            )
        ) {
            LiveScrubAdmissionPlan.Immediate -> {
                delayedPreviewAdmission.cancel()
                val admittedAtNanos = System.nanoTime()
                previewAdmissionPolicy.markAdmitted(fraction, targetKey, admittedAtNanos)
                ScrubUxTelemetry.recordAdmissionDelay(observedAtNanos, admittedAtNanos)
                action()
            }
            is LiveScrubAdmissionPlan.After -> {
                delayedPreviewAdmission.replace(previewScope, plan.delayMs) {
                    val admittedAtNanos = System.nanoTime()
                    previewAdmissionPolicy.markAdmitted(fraction, targetKey, admittedAtNanos)
                    ScrubUxTelemetry.recordAdmissionDelay(observedAtNanos, admittedAtNanos)
                    action()
                }
            }
            LiveScrubAdmissionPlan.NoPreview -> delayedPreviewAdmission.cancel()
        }
    }

    val previewFrameId = if (bounds == null) {
        MicroscopeTimelineMath.frameForFraction(
            fraction = scrubFraction,
            frameCount = session.frameCount,
        )
    } else {
        null
    }
    val previewTimestampUs = bounds?.let { indexedBounds ->
        MicroscopeTimelineMath.timestampForFraction(
            fraction = scrubFraction,
            startUs = indexedBounds.startUs,
            endUs = indexedBounds.endUs,
        )
    }

    Text(
        text = if (bounds == null) {
            "Presentation-order scrub"
        } else {
            "Indexed presentation timeline"
        },
        style = MaterialTheme.typography.titleSmall,
    )
    Text(
        text = previewTimestampUs?.let { timestamp ->
            "Preview ${MicroscopePreviewMath.formatTimestampUs(timestamp)}"
        } ?: previewFrameId?.let { frameId ->
            MicroscopeTimelineMath.framePositionLabel(frameId, session.frameCount)
        } ?: "Indexed position unavailable",
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    Slider(
        value = scrubFraction,
        onValueChange = { fraction ->
            val observedAtNanos = System.nanoTime()
            thumbDrawTracker.onPointerInput(observedAtNanos)
            scrubbing = true
            val nextFraction = fraction.coerceIn(0f, 1f)
            // Gesture state is committed before any preview scheduling. Decoder work therefore
            // cannot own the thumb position or decide whether it moves.
            scrubFraction = nextFraction
            val indexedBounds = bounds
            if (indexedBounds != null) {
                MicroscopeTimelineMath.timestampForFraction(
                    fraction = nextFraction,
                    startUs = indexedBounds.startUs,
                    endUs = indexedBounds.endUs,
                )?.let { targetTimestampUs ->
                    admitPreview(
                        fraction = nextFraction,
                        targetKey = targetTimestampUs,
                        observedAtNanos = observedAtNanos,
                    ) {
                        onPreviewTimestampUs(targetTimestampUs)
                    }
                }
            } else {
                MicroscopeTimelineMath.frameForFraction(
                    fraction = nextFraction,
                    frameCount = session.frameCount,
                )?.let { targetFrameId ->
                    admitPreview(
                        fraction = nextFraction,
                        targetKey = targetFrameId,
                        observedAtNanos = observedAtNanos,
                    ) {
                        onPreviewFrame(targetFrameId)
                    }
                }
            }
        },
        onValueChangeFinished = {
            delayedPreviewAdmission.cancel()
            previewAdmissionPolicy.reset()
            val indexedBounds = bounds
            val targetTimestampUs = indexedBounds?.let {
                MicroscopeTimelineMath.timestampForFraction(
                    fraction = scrubFraction,
                    startUs = it.startUs,
                    endUs = it.endUs,
                )
            }
            val targetFrameId = if (indexedBounds == null) {
                MicroscopeTimelineMath.frameForFraction(
                    fraction = scrubFraction,
                    frameCount = session.frameCount,
                )
            } else {
                null
            }
            scrubbing = false
            when {
                targetTimestampUs != null -> {
                    ScrubUxTelemetry.beginExactSettle(session.sessionId)
                    onFinishScrubTimestampUs(targetTimestampUs)
                }
                targetFrameId != null -> {
                    ScrubUxTelemetry.beginExactSettle(session.sessionId)
                    onFinishScrubFrame(targetFrameId)
                }
            }
        },
        enabled = enabled && session.frameCount > 1L,
        valueRange = 0f..1f,
        modifier = Modifier
            .fillMaxWidth()
            .drawWithContent {
                drawContent()
                thumbDrawTracker.onDrawn(System.nanoTime())
            }
            .testTag(TIMELINE_SLIDER_TAG)
            .semantics { contentDescription = "Video timeline scrubber" },
    )
}

@Composable
private fun TimelineRangeControls(
    sessionId: Long,
    bounds: IndexedTimelineBounds,
    committedRange: TimelineRangeSelection?,
    enabled: Boolean,
    onCommitRange: (Long, Long) -> Unit,
    onClearRange: () -> Unit,
) {
    val committedStartFraction = committedRange?.let {
        MicroscopeTimelineMath.fractionForTimestamp(it.startUs, bounds.startUs, bounds.endUs)
    } ?: 0f
    val committedEndFraction = committedRange?.let {
        MicroscopeTimelineMath.fractionForTimestamp(it.endUs, bounds.startUs, bounds.endUs)
    } ?: 1f

    var rangeVisible by rememberSaveable(sessionId) { mutableStateOf(committedRange != null) }
    var startFraction by rememberSaveable(sessionId) { mutableStateOf(committedStartFraction) }
    var endFraction by rememberSaveable(sessionId) { mutableStateOf(committedEndFraction) }
    var draggingRange by remember(sessionId) { mutableStateOf(false) }

    LaunchedEffect(committedStartFraction, committedEndFraction, draggingRange) {
        if (!draggingRange) {
            startFraction = committedStartFraction
            endFraction = committedEndFraction
        }
    }

    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        OutlinedButton(
            onClick = { rangeVisible = !rangeVisible },
            enabled = enabled,
            modifier = Modifier.weight(1f),
        ) {
            Text(if (rangeVisible) "Hide range" else "Select range")
        }
        if (committedRange != null) {
            TextButton(
                onClick = {
                    rangeVisible = false
                    onClearRange()
                },
                enabled = enabled,
                modifier = Modifier.weight(1f),
            ) {
                Text("Clear range")
            }
        }
    }

    if (!rangeVisible) return

    RangeSlider(
        value = startFraction..endFraction,
        onValueChange = { range ->
            draggingRange = true
            startFraction = range.start.coerceIn(0f, 1f)
            endFraction = range.endInclusive.coerceIn(startFraction, 1f)
        },
        onValueChangeFinished = {
            val startUs = MicroscopeTimelineMath.timestampForFraction(
                fraction = startFraction,
                startUs = bounds.startUs,
                endUs = bounds.endUs,
            )
            val endUs = MicroscopeTimelineMath.timestampForFraction(
                fraction = endFraction,
                startUs = bounds.startUs,
                endUs = bounds.endUs,
            )
            draggingRange = false
            if (startUs != null && endUs != null && startUs <= endUs) {
                onCommitRange(startUs, endUs)
            }
        },
        enabled = enabled,
        valueRange = 0f..1f,
        modifier = Modifier
            .fillMaxWidth()
            .testTag(TIMELINE_RANGE_SLIDER_TAG)
            .semantics { contentDescription = "Selected timeline range" },
    )

    val previewStartUs = MicroscopeTimelineMath.timestampForFraction(
        fraction = startFraction,
        startUs = bounds.startUs,
        endUs = bounds.endUs,
    )
    val previewEndUs = MicroscopeTimelineMath.timestampForFraction(
        fraction = endFraction,
        startUs = bounds.startUs,
        endUs = bounds.endUs,
    )
    val durationUs = if (previewStartUs != null && previewEndUs != null) {
        MicroscopeTimelineMath.durationUs(previewStartUs, previewEndUs)
    } else {
        null
    }

    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text("Selected range", style = MaterialTheme.typography.titleSmall)
        Text(
            text = "Start: ${previewStartUs?.let(MicroscopePreviewMath::formatTimestampUs) ?: "unavailable"}",
            style = MaterialTheme.typography.bodyMedium,
        )
        Text(
            text = "End: ${previewEndUs?.let(MicroscopePreviewMath::formatTimestampUs) ?: "unavailable"}",
            style = MaterialTheme.typography.bodyMedium,
        )
        Text(
            text = "Duration: ${durationUs?.let(MicroscopePreviewMath::formatTimestampUs) ?: "unavailable"}",
            style = MaterialTheme.typography.bodyMedium,
        )
        Text(
            text = "Extraction uses inclusive indexed timestamps: start ≤ frame timestamp ≤ end.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
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
