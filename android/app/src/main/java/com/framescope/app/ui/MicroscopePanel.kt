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
import com.framescope.app.data.DEFAULT_SCRUB_PREVIEW_MAX_EDGE
import com.framescope.app.data.FrameDetails
import com.framescope.app.data.MicroscopeFrame
import com.framescope.app.data.MicroscopeScrubPreview
import com.framescope.app.data.MicroscopeSessionSnapshot
import java.nio.ByteBuffer
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
        val frameId: Long,
        val liveScrub: Boolean,
    ) : MicroscopePreviewState

    data class Error(
        val message: String,
    ) : MicroscopePreviewState
}

@Composable
internal fun MicroscopePanel(
    state: MicroscopeUiState,
    indexingProgress: IndexingProgressUi? = null,
    timelineBounds: IndexedTimelineBounds?,
    rangeSelection: TimelineRangeSelection?,
    scrubPreview: MicroscopeScrubPreview?,
    onStep: (Int) -> Unit,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
    onPreviewFrame: (Long) -> Unit,
    onPreviewTimestampUs: (Long) -> Unit,
    onFinishScrubFrame: (Long) -> Unit,
    onFinishScrubTimestampUs: (Long) -> Unit,
    onCommitRange: (Long, Long) -> Unit,
    onClearRange: () -> Unit,
    onDismissError: () -> Unit,
) {
    when (state) {
        MicroscopeUiState.Idle -> Unit
        MicroscopeUiState.Opening -> MicroscopeIndexingCard(indexingProgress)
        is MicroscopeUiState.LoadingFrame -> MicroscopeBusyCard("Decoding source-quality frame…")
        is MicroscopeUiState.Navigating -> {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                state.previousFrame?.let { frame ->
                    MicroscopeFrameCard(
                        session = state.session,
                        frame = frame,
                        timelineBounds = timelineBounds,
                        rangeSelection = rangeSelection,
                        scrubPreview = scrubPreview,
                        controlsEnabled = false,
                        onStep = onStep,
                        onJumpFrame = onJumpFrame,
                        onJumpTimestampUs = onJumpTimestampUs,
                        onPreviewFrame = onPreviewFrame,
                        onPreviewTimestampUs = onPreviewTimestampUs,
                        onFinishScrubFrame = onFinishScrubFrame,
                        onFinishScrubTimestampUs = onFinishScrubTimestampUs,
                        onCommitRange = onCommitRange,
                        onClearRange = onClearRange,
                    )
                }
                MicroscopeBusyCard("Resolving the exact indexed frame…")
            }
        }
        is MicroscopeUiState.Ready -> MicroscopeFrameCard(
            session = state.session,
            frame = state.frame,
            timelineBounds = timelineBounds,
            rangeSelection = rangeSelection,
            scrubPreview = scrubPreview,
            controlsEnabled = true,
            onStep = onStep,
            onJumpFrame = onJumpFrame,
            onJumpTimestampUs = onJumpTimestampUs,
            onPreviewFrame = onPreviewFrame,
            onPreviewTimestampUs = onPreviewTimestampUs,
            onFinishScrubFrame = onFinishScrubFrame,
            onFinishScrubTimestampUs = onFinishScrubTimestampUs,
            onCommitRange = onCommitRange,
            onClearRange = onClearRange,
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
private fun MicroscopeFrameCard(
    session: MicroscopeSessionSnapshot,
    frame: MicroscopeFrame,
    timelineBounds: IndexedTimelineBounds?,
    rangeSelection: TimelineRangeSelection?,
    scrubPreview: MicroscopeScrubPreview?,
    controlsEnabled: Boolean,
    onStep: (Int) -> Unit,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
    onPreviewFrame: (Long) -> Unit,
    onPreviewTimestampUs: (Long) -> Unit,
    onFinishScrubFrame: (Long) -> Unit,
    onFinishScrubTimestampUs: (Long) -> Unit,
    onCommitRange: (Long, Long) -> Unit,
    onClearRange: () -> Unit,
) {
    val descriptor = frame.descriptor
    val authoritativePreview by key(frame) {
        produceState<MicroscopePreviewState>(
            initialValue = MicroscopePreviewState.Loading,
        ) {
            value = withContext(Dispatchers.Default) {
                frame.toBoundedPreview()
            }
        }
    }
    val livePreview by produceState<MicroscopePreviewState?>(
        initialValue = null,
        key1 = scrubPreview,
    ) {
        value = scrubPreview?.toBoundedPreview()
    }

    RecyclePreviewBitmap(authoritativePreview)
    val displayedPreview = livePreview ?: authoritativePreview

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
                text = "Frame microscope",
                style = MaterialTheme.typography.titleLarge,
                fontWeight = FontWeight.SemiBold,
            )

            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(min = 180.dp, max = 520.dp)
                    .aspectRatio(descriptor.width.toFloat() / descriptor.height.toFloat()),
                contentAlignment = Alignment.Center,
            ) {
                when (val current = displayedPreview) {
                    MicroscopePreviewState.Loading -> CircularProgressIndicator()
                    is MicroscopePreviewState.Error -> Text(
                        text = current.message,
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.error,
                    )
                    is MicroscopePreviewState.Ready -> {
                        MicroscopeZoomableImage(
                            bitmap = current.bitmap,
                            contentDescription = if (current.liveScrub) {
                                "Timeline preview frame ${current.frameId + 1} of ${session.frameCount}"
                            } else {
                                "Video frame ${current.frameId + 1} of ${session.frameCount}"
                            },
                            enabled = controlsEnabled && !current.liveScrub,
                            swipeEnabled = controlsEnabled &&
                                !current.liveScrub &&
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

            when (val current = displayedPreview) {
                is MicroscopePreviewState.Ready -> {
                    val plan = current.plan
                    Text(
                        text = when {
                            current.liveScrub -> "Live preview · frame ${current.frameId + 1}"
                            plan.isDownscaled ->
                                "Display preview ${plan.targetWidth} × ${plan.targetHeight}. Pinch to zoom; swipe at 1× to step one frame."
                            else -> "Pinch to zoom; swipe at 1× to step one frame."
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                else -> Unit
            }

            FrameIdentity(frame = session.currentFrame, frameCount = session.frameCount)

            MicroscopeTimelineControls(
                session = session,
                timelineBounds = timelineBounds,
                rangeSelection = rangeSelection,
                enabled = controlsEnabled,
                onJumpFrame = onJumpFrame,
                onJumpTimestampUs = onJumpTimestampUs,
                onPreviewFrame = onPreviewFrame,
                onPreviewTimestampUs = onPreviewTimestampUs,
                onFinishScrubFrame = onFinishScrubFrame,
                onFinishScrubTimestampUs = onFinishScrubTimestampUs,
                onCommitRange = onCommitRange,
                onClearRange = onClearRange,
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
                    Text("Previous frame")
                }
                Button(
                    onClick = { onStep(1) },
                    enabled = controlsEnabled && session.canStepNext,
                    modifier = Modifier.weight(1f),
                ) {
                    Text("Next frame")
                }
            }

            JumpControls(
                session = session,
                enabled = controlsEnabled,
                onJumpFrame = onJumpFrame,
                onJumpTimestampUs = onJumpTimestampUs,
            )
        }
    }
}

@Composable
private fun RecyclePreviewBitmap(state: MicroscopePreviewState) {
    val ready = state as? MicroscopePreviewState.Ready ?: return
    if (ready.liveScrub) return
    DisposableEffect(ready.bitmap) {
        onDispose {
            if (!ready.bitmap.isRecycled) {
                ready.bitmap.recycle()
            }
        }
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
    return rgbaToBoundedPreview(
        width = metadata.width,
        height = metadata.height,
        strideBytes = metadata.strideBytes,
        rgba = rgba,
        frameId = metadata.frameId,
        liveScrub = false,
        errorMessage = "The authoritative frame could not be converted for display.",
    )
}

private fun MicroscopeScrubPreview.toBoundedPreview(): MicroscopePreviewState {
    val metadata = descriptor
    if (!metadata.isSane(DEFAULT_SCRUB_PREVIEW_MAX_EDGE)) {
        return MicroscopePreviewState.Error("Live preview metadata is outside display safety bounds.")
    }
    val displayBitmap = bitmap
        ?: return MicroscopePreviewState.Error("The live preview transport did not provide a display bitmap.")
    if (displayBitmap.isRecycled || displayBitmap.width != metadata.width || displayBitmap.height != metadata.height) {
        return MicroscopePreviewState.Error("The live preview bitmap no longer matches its frame descriptor.")
    }
    val plan = MicroscopePreviewMath.plan(metadata.width, metadata.height)
        ?: return MicroscopePreviewState.Error("Could not plan the bounded live preview.")
    if (plan.isDownscaled) {
        return MicroscopePreviewState.Error("The native live preview exceeded the Android display preview budget.")
    }
    return MicroscopePreviewState.Ready(
        bitmap = displayBitmap,
        plan = plan,
        frameId = metadata.frameId,
        liveScrub = true,
    )
}

private suspend fun rgbaToBoundedPreview(
    width: Int,
    height: Int,
    strideBytes: Long,
    rgba: ByteBuffer,
    frameId: Long,
    liveScrub: Boolean,
    errorMessage: String,
): MicroscopePreviewState {
    val plan = MicroscopePreviewMath.plan(width, height)
        ?: return MicroscopePreviewState.Error("Could not plan a bounded frame preview.")

    var bitmap: Bitmap? = null
    try {
        bitmap = Bitmap.createBitmap(plan.targetWidth, plan.targetHeight, Bitmap.Config.ARGB_8888)
        val source = rgba.duplicate()
        val packedRowBytes = width.toLong() * 4L
        if (!plan.isDownscaled && strideBytes == packedRowBytes) {
            val requiredBytes = Math.multiplyExact(packedRowBytes, height.toLong())
            if (requiredBytes > source.capacity().toLong()) {
                bitmap.recycle()
                return MicroscopePreviewState.Error(errorMessage)
            }
            source.position(0)
            source.limit(Math.toIntExact(requiredBytes))
            bitmap.copyPixelsFromBuffer(source)
        } else {
            val row = IntArray(plan.targetWidth)
            for (outputY in 0 until plan.targetHeight) {
                currentCoroutineContext().ensureActive()
                val sourceY = plan.sourceY(outputY)
                val rowStart = Math.multiplyExact(sourceY.toLong(), strideBytes)
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
        }
        return MicroscopePreviewState.Ready(
            bitmap = bitmap,
            plan = plan,
            frameId = frameId,
            liveScrub = liveScrub,
        )
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
        return MicroscopePreviewState.Error(errorMessage)
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
            color = MaterialTheme.colorScheme.onSurfaceVariant,
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
                text = "Microscope error",
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.onErrorContainer,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                text = message,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onErrorContainer,
            )
            code?.let {
                Text(
                    text = it,
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onErrorContainer,
                )
            }
            OutlinedButton(onClick = onDismiss) {
                Text("Dismiss")
            }
        }
    }
}
