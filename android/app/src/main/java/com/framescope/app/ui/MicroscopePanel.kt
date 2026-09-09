package com.framescope.app.ui

import android.graphics.Bitmap
import android.os.Trace
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
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.framescope.app.data.DEFAULT_SCRUB_PREVIEW_MAX_EDGE
import com.framescope.app.data.MicroscopeFrame
import com.framescope.app.data.MicroscopeScrubPreview
import com.framescope.app.data.MicroscopeSessionSnapshot
import com.framescope.app.data.PixelTransportTelemetry
import java.nio.ByteBuffer
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext

private const val TRACE_AUTHORITATIVE_BITMAP = "FrameScope.pixels.frame_bitmap"

private sealed interface MicroscopePreviewState {
    data object Loading : MicroscopePreviewState

    data class Ready(
        val bitmap: Bitmap,
        val plan: MicroscopePreviewPlan,
        val sessionId: Long,
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
    // Ready and Navigating deliberately share one composition site. The timeline's local drag and
    // release state therefore survives the brief authoritative navigation state transition.
    val visibleSession = when (state) {
        is MicroscopeUiState.Ready -> state.session
        is MicroscopeUiState.Navigating -> state.session
        else -> null
    }
    val visibleFrame = when (state) {
        is MicroscopeUiState.Ready -> state.frame
        is MicroscopeUiState.Navigating -> state.previousFrame
        else -> null
    }
    if (visibleSession != null && visibleFrame != null) {
        MicroscopeFrameCard(
            session = visibleSession,
            frame = visibleFrame,
            timelineBounds = timelineBounds,
            rangeSelection = rangeSelection,
            scrubPreview = scrubPreview,
            controlsEnabled = state is MicroscopeUiState.Ready,
            exactSettleInProgress = state is MicroscopeUiState.Navigating,
            onStep = onStep,
            onPreviewFrame = onPreviewFrame,
            onPreviewTimestampUs = onPreviewTimestampUs,
            onFinishScrubFrame = onFinishScrubFrame,
            onFinishScrubTimestampUs = onFinishScrubTimestampUs,
            onCommitRange = onCommitRange,
            onClearRange = onClearRange,
        )
        return
    }

    when (state) {
        MicroscopeUiState.Idle -> Unit
        MicroscopeUiState.Opening -> MicroscopeIndexingCard(indexingProgress)
        is MicroscopeUiState.LoadingFrame -> MicroscopeBusyCard("Preparing frame…")
        is MicroscopeUiState.Navigating -> MicroscopeBusyCard("Settling exact frame…")
        is MicroscopeUiState.Ready -> Unit
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
    exactSettleInProgress: Boolean,
    onStep: (Int) -> Unit,
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
    val liveBitmapLease = remember(scrubPreview) {
        scrubPreview?.acquireBitmapLease()
    }
    DisposableEffect(liveBitmapLease) {
        onDispose {
            liveBitmapLease?.close()
        }
    }
    val livePreview = if (scrubPreview != null && liveBitmapLease != null) {
        scrubPreview.toBoundedPreview(liveBitmapLease.bitmap)
    } else {
        null
    }

    RecyclePreviewBitmap(authoritativePreview)
    val displayedPreview = livePreview ?: authoritativePreview

    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        shape = RoundedCornerShape(14.dp),
    ) {
        Column(
            modifier = Modifier.padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(min = 200.dp, max = 560.dp)
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
                            modifier = Modifier
                                .fillMaxSize()
                                .drawWithContent {
                                    drawContent()
                                    val drawnAtNanos = System.nanoTime()
                                    if (current.liveScrub) {
                                        ScrubUxTelemetry.recordPreviewPresented(
                                            sessionId = current.sessionId,
                                            presentedAtNanos = drawnAtNanos,
                                        )
                                    } else if (controlsEnabled) {
                                        ScrubUxTelemetry.completeExactSettle(
                                            sessionId = current.sessionId,
                                            completedAtNanos = drawnAtNanos,
                                        )
                                    }
                                },
                        )
                    }
                }
            }

            // Timeline and frame navigation live directly under the visual content. Technical frame
            // identity and exact-jump controls already belong to the Inspector destination.
            MicroscopeTimelineControls(
                session = session,
                timelineBounds = timelineBounds,
                rangeSelection = rangeSelection,
                enabled = controlsEnabled,
                exactSettleInProgress = exactSettleInProgress,
                authoritativePresentationToken = frame,
                onStep = onStep,
                onPreviewFrame = onPreviewFrame,
                onPreviewTimestampUs = onPreviewTimestampUs,
                onFinishScrubFrame = onFinishScrubFrame,
                onFinishScrubTimestampUs = onFinishScrubTimestampUs,
                onCommitRange = onCommitRange,
                onClearRange = onClearRange,
            )

            when (val current = displayedPreview) {
                is MicroscopePreviewState.Ready -> {
                    if (current.liveScrub) {
                        Text(
                            text = "Live preview",
                            style = MaterialTheme.typography.labelMedium,
                            color = MaterialTheme.colorScheme.primary,
                        )
                    } else {
                        Text(
                            text = "Pinch to zoom · swipe to step",
                            style = MaterialTheme.typography.labelMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
                else -> Unit
            }

            if (session.currentFrame?.corrupt == true) {
                Text(
                    text = "This indexed frame is marked corrupt.",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.error,
                )
            }
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
        sessionId = metadata.sessionId,
        frameId = metadata.frameId,
        liveScrub = false,
        errorMessage = "The authoritative frame could not be converted for display.",
    )
}

private fun MicroscopeScrubPreview.toBoundedPreview(
    displayBitmap: Bitmap,
): MicroscopePreviewState {
    val metadata = descriptor
    if (!metadata.isSane(DEFAULT_SCRUB_PREVIEW_MAX_EDGE)) {
        return MicroscopePreviewState.Error("Live preview metadata is outside display safety bounds.")
    }
    if (
        displayBitmap.isRecycled ||
        displayBitmap.width != metadata.width ||
        displayBitmap.height != metadata.height
    ) {
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
        sessionId = metadata.sessionId,
        frameId = metadata.frameId,
        liveScrub = true,
    )
}

private suspend fun rgbaToBoundedPreview(
    width: Int,
    height: Int,
    strideBytes: Long,
    rgba: ByteBuffer,
    sessionId: Long,
    frameId: Long,
    liveScrub: Boolean,
    errorMessage: String,
): MicroscopePreviewState {
    val plan = MicroscopePreviewMath.plan(width, height)
        ?: return MicroscopePreviewState.Error("Could not plan a bounded frame preview.")

    var bitmap: Bitmap? = null
    val conversionStarted = System.nanoTime()
    val traceStarted = runCatching {
        Trace.beginSection(TRACE_AUTHORITATIVE_BITMAP)
        true
    }.getOrDefault(false)
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
        val copiedBytes = Math.multiplyExact(
            Math.multiplyExact(plan.targetWidth.toLong(), plan.targetHeight.toLong()),
            4L,
        )
        PixelTransportTelemetry.recordAuthoritativeBitmap(
            allocationBytes = bitmap.allocationByteCount.toLong(),
            copiedBytes = copiedBytes,
            conversionUs = elapsedUs(conversionStarted),
        )
        return MicroscopePreviewState.Ready(
            bitmap = bitmap,
            plan = plan,
            sessionId = sessionId,
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
    } finally {
        if (traceStarted) runCatching { Trace.endSection() }
    }
}

private fun elapsedUs(startedNanos: Long): Long =
    ((System.nanoTime() - startedNanos).coerceAtLeast(0L)) / 1_000L

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
