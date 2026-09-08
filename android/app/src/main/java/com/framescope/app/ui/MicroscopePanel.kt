package com.framescope.app.ui

import android.graphics.Bitmap
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.framescope.app.data.FrameDetails
import com.framescope.app.data.MicroscopeFrame
import com.framescope.app.data.MicroscopeSessionSnapshot
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext

private sealed interface MicroscopePreviewState {
    data object Loading : MicroscopePreviewState

    data class Ready(
        val bitmap: Bitmap,
        val plan: MicroscopePreviewPlan,
    ) : MicroscopePreviewState

    data class Error(
        val message: String,
    ) : MicroscopePreviewState
}

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
    when (state) {
        MicroscopeUiState.Idle -> Unit
        MicroscopeUiState.Opening -> MicroscopeBusyCard(
            if (compactWorkspace) {
                "Indexing video timeline…"
            } else {
                "Indexing presentation timestamps in Rust… Exact timeline navigation unlocks when the complete index is ready."
            },
        )
        is MicroscopeUiState.LoadingFrame -> MicroscopeBusyCard(
            if (compactWorkspace) "Loading frame…" else "Decoding source-quality frame…",
        )
        is MicroscopeUiState.Navigating -> {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                state.previousFrame?.let { frame ->
                    MicroscopeFrameCard(
                        session = state.session,
                        frame = frame,
                        timelineBounds = timelineBounds,
                        rangeSelection = rangeSelection,
                        controlsEnabled = false,
                        onStep = onStep,
                        onJumpFrame = onJumpFrame,
                        onJumpTimestampUs = onJumpTimestampUs,
                        onCommitRange = onCommitRange,
                        onClearRange = onClearRange,
                        compactWorkspace = compactWorkspace,
                    )
                }
                MicroscopeBusyCard(
                    if (compactWorkspace) "Moving to frame…" else "Resolving the requested indexed frame…",
                )
            }
        }
        is MicroscopeUiState.Ready -> MicroscopeFrameCard(
            session = state.session,
            frame = state.frame,
            timelineBounds = timelineBounds,
            rangeSelection = rangeSelection,
            controlsEnabled = true,
            onStep = onStep,
            onJumpFrame = onJumpFrame,
            onJumpTimestampUs = onJumpTimestampUs,
            onCommitRange = onCommitRange,
            onClearRange = onClearRange,
            compactWorkspace = compactWorkspace,
        )
        is MicroscopeUiState.Empty -> MicroscopeStatusCard(
            "The selected video has no indexed presentation frames.",
        )
        is MicroscopeUiState.Error -> MicroscopeErrorCard(
            message = state.message,
            code = state.code,
            onDismiss = onDismissError,
        )
    }
}

@Composable
internal fun MicroscopeInspectorPanel(
    state: MicroscopeUiState,
    onStep: (Int) -> Unit,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
) {
    when (state) {
        MicroscopeUiState.Idle -> MicroscopeStatusCard("No microscope session is open.")
        MicroscopeUiState.Opening -> MicroscopeStatusCard("Frame index is still being prepared.")
        is MicroscopeUiState.LoadingFrame -> InspectorFrameTools(
            session = state.session,
            enabled = false,
            onStep = onStep,
            onJumpFrame = onJumpFrame,
            onJumpTimestampUs = onJumpTimestampUs,
        )
        is MicroscopeUiState.Navigating -> InspectorFrameTools(
            session = state.session,
            enabled = false,
            onStep = onStep,
            onJumpFrame = onJumpFrame,
            onJumpTimestampUs = onJumpTimestampUs,
        )
        is MicroscopeUiState.Ready -> InspectorFrameTools(
            session = state.session,
            enabled = true,
            onStep = onStep,
            onJumpFrame = onJumpFrame,
            onJumpTimestampUs = onJumpTimestampUs,
        )
        is MicroscopeUiState.Empty -> MicroscopeStatusCard("The video has no indexed presentation frames.")
        is MicroscopeUiState.Error -> MicroscopeStatusCard(
            buildString {
                append(state.message)
                state.code?.let { append(" ($it)") }
            },
        )
    }
}

@Composable
private fun InspectorFrameTools(
    session: MicroscopeSessionSnapshot,
    enabled: Boolean,
    onStep: (Int) -> Unit,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Text(
                text = "Frame timing & navigation",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )
            FrameIdentity(frame = session.currentFrame, frameCount = session.frameCount)
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                OutlinedButton(
                    onClick = { onStep(-1) },
                    enabled = enabled && session.canStepPrevious,
                    modifier = Modifier.weight(1f),
                ) {
                    Text("Previous")
                }
                Button(
                    onClick = { onStep(1) },
                    enabled = enabled && session.canStepNext,
                    modifier = Modifier.weight(1f),
                ) {
                    Text("Next")
                }
            }
            JumpControls(
                session = session,
                enabled = enabled,
                onJumpFrame = onJumpFrame,
                onJumpTimestampUs = onJumpTimestampUs,
            )
        }
    }
}

@Composable
private fun MicroscopeFrameCard(
    session: MicroscopeSessionSnapshot,
    frame: MicroscopeFrame,
    timelineBounds: IndexedTimelineBounds?,
    rangeSelection: TimelineRangeSelection?,
    controlsEnabled: Boolean,
    onStep: (Int) -> Unit,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
    onCommitRange: (Long, Long) -> Unit,
    onClearRange: () -> Unit,
    compactWorkspace: Boolean,
) {
    val descriptor = frame.descriptor
    val preview by key(frame) {
        produceState<MicroscopePreviewState>(
            initialValue = MicroscopePreviewState.Loading,
        ) {
            value = withContext(Dispatchers.Default) {
                frame.toBoundedPreview()
            }
        }
    }

    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(
            modifier = Modifier.padding(if (compactWorkspace) 12.dp else 16.dp),
            verticalArrangement = Arrangement.spacedBy(if (compactWorkspace) 10.dp else 14.dp),
        ) {
            if (!compactWorkspace) {
                Text(
                    text = "Frame microscope",
                    style = MaterialTheme.typography.titleLarge,
                    fontWeight = FontWeight.SemiBold,
                )
            }

            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(min = if (compactWorkspace) 220.dp else 180.dp, max = 560.dp)
                    .aspectRatio(descriptor.width.toFloat() / descriptor.height.toFloat()),
                contentAlignment = Alignment.Center,
            ) {
                when (val current = preview) {
                    MicroscopePreviewState.Loading -> CircularProgressIndicator()
                    is MicroscopePreviewState.Error -> Text(
                        text = current.message,
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.error,
                    )
                    is MicroscopePreviewState.Ready -> {
                        DisposableEffect(current.bitmap) {
                            onDispose {
                                if (!current.bitmap.isRecycled) {
                                    current.bitmap.recycle()
                                }
                            }
                        }
                        MicroscopeZoomableImage(
                            bitmap = current.bitmap,
                            contentDescription =
                                "Video frame ${descriptor.frameId + 1} of ${session.frameCount}",
                            enabled = controlsEnabled,
                            swipeEnabled = controlsEnabled &&
                                (session.canStepPrevious || session.canStepNext),
                            onSwipe = { direction ->
                                when {
                                    direction < 0 && session.canStepPrevious -> onStep(-1)
                                    direction > 0 && session.canStepNext -> onStep(1)
                                }
                            },
                            modifier = Modifier.fillMaxSize(),
                        )
                    }
                }
            }

            if (!compactWorkspace) {
                when (val current = preview) {
                    is MicroscopePreviewState.Ready -> {
                        val plan = current.plan
                        Text(
                            text = if (plan.isDownscaled) {
                                "Display preview ${plan.targetWidth} × ${plan.targetHeight} from authoritative source frame " +
                                    "${plan.sourceWidth} × ${plan.sourceHeight}. Pinch to zoom; pan is enabled only above 1×. Swipe at 1× to step one frame."
                            } else {
                                "Display preview uses the authoritative source-frame dimensions. Pinch to zoom; pan is enabled only above 1×. Swipe at 1× to step one frame."
                            },
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    else -> Unit
                }
            }

            if (compactWorkspace) {
                WorkspaceFrameIdentity(frame = session.currentFrame, frameCount = session.frameCount)
            } else {
                FrameIdentity(frame = session.currentFrame, frameCount = session.frameCount)
            }

            MicroscopeTimelineControls(
                session = session,
                timelineBounds = timelineBounds,
                rangeSelection = rangeSelection,
                enabled = controlsEnabled,
                onJumpFrame = onJumpFrame,
                onJumpTimestampUs = onJumpTimestampUs,
                onCommitRange = onCommitRange,
                onClearRange = onClearRange,
                showImplementationGuidance = !compactWorkspace,
            )

            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                OutlinedButton(
                    onClick = { onStep(-1) },
                    enabled = controlsEnabled && session.canStepPrevious,
                    modifier = Modifier.weight(1f),
                ) {
                    Text(if (compactWorkspace) "Previous" else "Previous frame")
                }
                Button(
                    onClick = { onStep(1) },
                    enabled = controlsEnabled && session.canStepNext,
                    modifier = Modifier.weight(1f),
                ) {
                    Text(if (compactWorkspace) "Next" else "Next frame")
                }
            }

            if (!compactWorkspace) {
                JumpControls(
                    session = session,
                    enabled = controlsEnabled,
                    onJumpFrame = onJumpFrame,
                    onJumpTimestampUs = onJumpTimestampUs,
                )

                Text(
                    text = "Preview pixels are sampled directly from the caller-owned authoritative RGBA frame; " +
                        "disk proxies are never used for microscope identity, navigation, or extraction.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun WorkspaceFrameIdentity(frame: FrameDetails?, frameCount: Long) {
    if (frame == null) return
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = "Frame ${frame.frameId + 1} / $frameCount",
            style = MaterialTheme.typography.titleSmall,
            fontWeight = FontWeight.SemiBold,
        )
        Text(
            text = frame.timestampUs?.let(MicroscopePreviewMath::formatTimestampUs)
                ?: "Timestamp unavailable",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun FrameIdentity(frame: FrameDetails?, frameCount: Long) {
    if (frame == null) return
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(
            text = "Frame ${frame.frameId + 1} / $frameCount",
            style = MaterialTheme.typography.titleMedium,
            fontWeight = FontWeight.Medium,
        )
        Text(
            text = frame.timestampUs?.let {
                "Timestamp: ${MicroscopePreviewMath.formatTimestampUs(it)} ($it µs)"
            } ?: "Timestamp: unavailable",
            style = MaterialTheme.typography.bodyMedium,
        )
        Text(
            text = "PTS: ${MicroscopeUiFormatter.exactTimestamp(frame)}",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        MicroscopeUiFormatter.exactDuration(frame)?.let {
            Text(
                text = "Duration: $it",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        if (frame.keyframe || frame.corrupt) {
            Text(
                text = buildList {
                    if (frame.keyframe) add("keyframe")
                    if (frame.corrupt) add("corrupt")
                }.joinToString(" · "),
                style = MaterialTheme.typography.labelMedium,
                color = if (frame.corrupt) {
                    MaterialTheme.colorScheme.error
                } else {
                    MaterialTheme.colorScheme.secondary
                },
            )
        }
    }
}

@Composable
private fun JumpControls(
    session: MicroscopeSessionSnapshot,
    enabled: Boolean,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
) {
    var frameInput by rememberSaveable(session.sessionId) { mutableStateOf("") }
    var timestampInput by rememberSaveable(session.sessionId) { mutableStateOf("") }

    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        OutlinedTextField(
            value = frameInput,
            onValueChange = { value -> frameInput = value.filter(Char::isDigit).take(19) },
            enabled = enabled,
            modifier = Modifier.fillMaxWidth(),
            label = { Text("Jump to frame (1…${session.frameCount})") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
        )
        Button(
            onClick = {
                frameInput.toLongOrNull()
                    ?.takeIf { it in 1..session.frameCount }
                    ?.let { onJumpFrame(it - 1L) }
            },
            enabled = enabled && frameInput.toLongOrNull()?.let { it in 1..session.frameCount } == true,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Jump to exact frame")
        }

        OutlinedTextField(
            value = timestampInput,
            onValueChange = { value ->
                timestampInput = MicroscopePreviewMath.sanitizeSignedTimestampInput(value)
            },
            enabled = enabled,
            modifier = Modifier.fillMaxWidth(),
            label = { Text("Jump to timestamp (µs, signed)") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Text),
        )
        OutlinedButton(
            onClick = { timestampInput.toLongOrNull()?.let(onJumpTimestampUs) },
            enabled = enabled && timestampInput.toLongOrNull() != null,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Jump to nearest indexed timestamp")
        }
    }
}

private suspend fun MicroscopeFrame.toBoundedPreview(): MicroscopePreviewState {
    val metadata = descriptor
    if (!metadata.isSane()) {
        return MicroscopePreviewState.Error("Frame metadata is outside presentation safety bounds.")
    }
    val plan = MicroscopePreviewMath.plan(metadata.width, metadata.height)
        ?: return MicroscopePreviewState.Error("Could not plan a bounded frame preview.")

    var bitmap: Bitmap? = null
    try {
        bitmap = Bitmap.createBitmap(plan.targetWidth, plan.targetHeight, Bitmap.Config.ARGB_8888)
        val source = rgba.duplicate()
        val row = IntArray(plan.targetWidth)
        val stride = metadata.strideBytes

        for (outputY in 0 until plan.targetHeight) {
            currentCoroutineContext().ensureActive()
            val sourceY = plan.sourceY(outputY)
            val rowStart = Math.multiplyExact(sourceY.toLong(), stride)
            for (outputX in 0 until plan.targetWidth) {
                val sourceX = plan.sourceX(outputX)
                val offsetLong = Math.addExact(rowStart, sourceX.toLong() * 4L)
                val offset = Math.toIntExact(offsetLong)
                val red = source.get(offset).toInt() and 0xff
                val green = source.get(offset + 1).toInt() and 0xff
                val blue = source.get(offset + 2).toInt() and 0xff
                val alpha = source.get(offset + 3).toInt() and 0xff
                row[outputX] =
                    (alpha shl 24) or (red shl 16) or (green shl 8) or blue
            }
            bitmap.setPixels(row, 0, plan.targetWidth, 0, outputY, plan.targetWidth, 1)
        }
        return MicroscopePreviewState.Ready(bitmap = bitmap, plan = plan)
    } catch (cancelled: CancellationException) {
        bitmap?.recycle()
        throw cancelled
    } catch (_: OutOfMemoryError) {
        bitmap?.recycle()
        return MicroscopePreviewState.Error(
            "Android could not allocate the bounded display preview for this frame.",
        )
    } catch (_: RuntimeException) {
        bitmap?.recycle()
        return MicroscopePreviewState.Error(
            "The authoritative frame could not be converted for display.",
        )
    }
}

@Composable
private fun MicroscopeBusyCard(message: String) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        shape = RoundedCornerShape(18.dp),
    ) {
        Row(
            modifier = Modifier.padding(18.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            CircularProgressIndicator()
            Text(message, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

@Composable
private fun MicroscopeStatusCard(message: String) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        shape = RoundedCornerShape(18.dp),
    ) {
        Text(
            text = message,
            modifier = Modifier.padding(18.dp),
            style = MaterialTheme.typography.bodyMedium,
        )
    }
}

@Composable
private fun MicroscopeErrorCard(
    message: String,
    code: String?,
    onDismiss: () -> Unit,
) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                text = "Frame microscope error",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.Medium,
                color = MaterialTheme.colorScheme.onErrorContainer,
            )
            Text(
                text = message,
                color = MaterialTheme.colorScheme.onErrorContainer,
            )
            code?.let {
                Text(
                    text = "Code: $it",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onErrorContainer,
                )
            }
            OutlinedButton(onClick = onDismiss) {
                Text("Dismiss")
            }
        }
    }
}
