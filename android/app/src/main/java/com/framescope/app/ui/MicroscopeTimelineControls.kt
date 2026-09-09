package com.framescope.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RangeSlider
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
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
internal const val TIMELINE_SETTLING_TAG = "microscope_timeline_settling"

private enum class TimelineInteractionState {
    Idle,
    Dragging,
    AwaitingExactSettle,
}

@Composable
internal fun MicroscopeTimelineControls(
    session: MicroscopeSessionSnapshot,
    timelineBounds: IndexedTimelineBounds?,
    rangeSelection: TimelineRangeSelection?,
    enabled: Boolean,
    exactSettleInProgress: Boolean = false,
    onStep: (Int) -> Unit,
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
        // Keep high-frequency gesture state below this composition boundary. Range controls do not
        // need to recompose for every pointer sample.
        MicroscopeTimelineScrubber(
            session = session,
            bounds = bounds,
            currentFrameTimestampUs = currentFrame.timestampUs,
            currentFrameId = currentFrame.frameId,
            enabled = enabled,
            exactSettleInProgress = exactSettleInProgress,
            onStep = onStep,
            onPreviewFrame = onPreviewFrame,
            onPreviewTimestampUs = onPreviewTimestampUs,
            onFinishScrubFrame = onFinishScrubFrame,
            onFinishScrubTimestampUs = onFinishScrubTimestampUs,
        )

        if (bounds != null) {
            TimelineRangeControls(
                sessionId = session.sessionId,
                bounds = bounds,
                committedRange = committedRange,
                enabled = enabled && !exactSettleInProgress,
                onCommitRange = onCommitRange,
                onClearRange = onClearRange,
            )
        }
    }
}

@Composable
private fun MicroscopeTimelineScrubber(
    session: MicroscopeSessionSnapshot,
    bounds: IndexedTimelineBounds?,
    currentFrameTimestampUs: Long?,
    currentFrameId: Long,
    enabled: Boolean,
    exactSettleInProgress: Boolean,
    onStep: (Int) -> Unit,
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
    var interactionState by remember(session.sessionId) {
        mutableStateOf(TimelineInteractionState.Idle)
    }
    var settleObservedNavigation by remember(session.sessionId) { mutableStateOf(false) }
    var settleAnchorFrameId by remember(session.sessionId) { mutableStateOf<Long?>(null) }
    val previewAdmissionPolicy = remember(session.sessionId) { LiveScrubAdmissionPolicy() }
    val delayedPreviewAdmission = remember(session.sessionId) { DelayedScrubPreviewAdmission() }
    val thumbDrawTracker = remember(session.sessionId) { ScrubThumbDrawTracker() }
    val previewScope = rememberCoroutineScope()

    DisposableEffect(session.sessionId) {
        onDispose {
            delayedPreviewAdmission.cancel()
            previewAdmissionPolicy.reset()
        }
    }

    LaunchedEffect(
        authoritativeFraction,
        currentFrameId,
        exactSettleInProgress,
        interactionState,
    ) {
        when (interactionState) {
            TimelineInteractionState.Idle -> {
                scrubFraction = authoritativeFraction
                delayedPreviewAdmission.cancel()
                previewAdmissionPolicy.reset()
                settleObservedNavigation = false
                settleAnchorFrameId = null
            }
            TimelineInteractionState.Dragging -> Unit
            TimelineInteractionState.AwaitingExactSettle -> {
                if (exactSettleInProgress) {
                    settleObservedNavigation = true
                }
                val authoritativeFrameChanged = settleAnchorFrameId?.let { anchor ->
                    currentFrameId != anchor
                } == true
                if (
                    !exactSettleInProgress &&
                    (settleObservedNavigation || authoritativeFrameChanged)
                ) {
                    // Only the completed authoritative navigation is allowed to correct the local
                    // release position. Until then the thumb stays where the finger left it.
                    scrubFraction = authoritativeFraction
                    interactionState = TimelineInteractionState.Idle
                    settleObservedNavigation = false
                    settleAnchorFrameId = null
                    delayedPreviewAdmission.cancel()
                    previewAdmissionPolicy.reset()
                }
            }
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

    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(
            text = previewTimestampUs?.let(MicroscopePreviewMath::formatTimestampUs)
                ?: previewFrameId?.let { frameId ->
                    MicroscopeTimelineMath.framePositionLabel(frameId, session.frameCount)
                }
                ?: "Position unavailable",
            style = MaterialTheme.typography.labelLarge,
        )
        Text(
            text = bounds?.let { MicroscopePreviewMath.formatTimestampUs(it.endUs) }
                ?: "${session.frameCount} frames",
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }

    Slider(
        value = scrubFraction,
        onValueChange = { fraction ->
            val observedAtNanos = System.nanoTime()
            thumbDrawTracker.onPointerInput(observedAtNanos)
            interactionState = TimelineInteractionState.Dragging
            settleObservedNavigation = false
            settleAnchorFrameId = null
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
            when {
                targetTimestampUs != null -> {
                    settleAnchorFrameId = currentFrameId
                    settleObservedNavigation = false
                    interactionState = TimelineInteractionState.AwaitingExactSettle
                    ScrubUxTelemetry.beginExactSettle(session.sessionId)
                    onFinishScrubTimestampUs(targetTimestampUs)
                }
                targetFrameId != null -> {
                    settleAnchorFrameId = currentFrameId
                    settleObservedNavigation = false
                    interactionState = TimelineInteractionState.AwaitingExactSettle
                    ScrubUxTelemetry.beginExactSettle(session.sessionId)
                    onFinishScrubFrame(targetFrameId)
                }
                else -> interactionState = TimelineInteractionState.Idle
            }
        },
        enabled = enabled && !exactSettleInProgress && session.frameCount > 1L,
        valueRange = 0f..1f,
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .drawWithContent {
                drawContent()
                thumbDrawTracker.onDrawn(System.nanoTime())
            }
            .testTag(TIMELINE_SLIDER_TAG)
            .semantics { contentDescription = "Video timeline scrubber" },
    )

    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        TextButton(
            onClick = { onStep(-1) },
            enabled = enabled &&
                interactionState == TimelineInteractionState.Idle &&
                session.canStepPrevious,
        ) {
            Text("‹ Frame")
        }
        if (
            interactionState == TimelineInteractionState.AwaitingExactSettle ||
            exactSettleInProgress
        ) {
            Text(
                text = "Settling exact frame…",
                modifier = Modifier.testTag(TIMELINE_SETTLING_TAG),
                style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.primary,
            )
        } else {
            Text(
                text = MicroscopeTimelineMath.framePositionLabel(
                    currentFrameId,
                    session.frameCount,
                ),
                style = MaterialTheme.typography.labelLarge,
            )
        }
        TextButton(
            onClick = { onStep(1) },
            enabled = enabled &&
                interactionState == TimelineInteractionState.Idle &&
                session.canStepNext,
        ) {
            Text("Frame ›")
        }
    }
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
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        TextButton(
            onClick = { rangeVisible = !rangeVisible },
            enabled = enabled,
        ) {
            Text(if (rangeVisible) "Hide range" else "Range")
        }
        if (committedRange != null) {
            TextButton(
                onClick = {
                    rangeVisible = false
                    onClearRange()
                },
                enabled = enabled,
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
            .heightIn(min = 48.dp)
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

    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(
            text = previewStartUs?.let(MicroscopePreviewMath::formatTimestampUs) ?: "Start unavailable",
            style = MaterialTheme.typography.labelMedium,
        )
        Text(
            text = durationUs?.let { "${MicroscopePreviewMath.formatTimestampUs(it)} selected" }
                ?: "Duration unavailable",
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            text = previewEndUs?.let(MicroscopePreviewMath::formatTimestampUs) ?: "End unavailable",
            style = MaterialTheme.typography.labelMedium,
        )
    }
}
