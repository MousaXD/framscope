package com.framescope.app.ui

import android.graphics.Bitmap
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.framescope.app.data.FrameDetails
import com.framescope.app.data.MicroscopeFrame
import com.framescope.app.data.MicroscopeSessionSnapshot
import java.util.Locale
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext

@Composable
internal fun MicroscopePanel(
    state: MicroscopeUiState,
    onStep: (Int) -> Unit,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
    onDismissError: () -> Unit,
) {
    when (state) {
        MicroscopeUiState.Idle -> Unit
        MicroscopeUiState.Opening -> MicroscopeBusyCard("Building frame index…")
        is MicroscopeUiState.LoadingFrame -> MicroscopeBusyCard("Decoding source-quality frame…")
        is MicroscopeUiState.Navigating -> {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                state.previousFrame?.let { frame ->
                    MicroscopeFrameCard(
                        session = state.session,
                        frame = frame,
                        controlsEnabled = false,
                        onStep = onStep,
                        onJumpFrame = onJumpFrame,
                        onJumpTimestampUs = onJumpTimestampUs,
                    )
                }
                MicroscopeBusyCard("Navigating indexed timeline…")
            }
        }
        is MicroscopeUiState.Ready -> MicroscopeFrameCard(
            session = state.session,
            frame = state.frame,
            controlsEnabled = true,
            onStep = onStep,
            onJumpFrame = onJumpFrame,
            onJumpTimestampUs = onJumpTimestampUs,
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
    controlsEnabled: Boolean,
    onStep: (Int) -> Unit,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
) {
    val descriptor = frame.descriptor
    val bitmap by produceState<Bitmap?>(initialValue = null, frame) {
        value = withContext(Dispatchers.Default) {
            frame.toArgbBitmapOrNull()
        }
    }

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
                val image = bitmap
                if (image == null) {
                    CircularProgressIndicator()
                } else {
                    Image(
                        bitmap = image.asImageBitmap(),
                        contentDescription = "Video frame ${descriptor.frameId + 1} of ${session.frameCount}",
                        modifier = Modifier.fillMaxWidth(),
                        contentScale = ContentScale.Fit,
                    )
                }
            }

            FrameIdentity(frame = session.currentFrame, frameCount = session.frameCount)

            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                OutlinedButton(
                    onClick = { onStep(-1) },
                    enabled = controlsEnabled && session.canStepPrevious,
                    modifier = Modifier.weight(1f),
                ) {
                    Text("Previous")
                }
                Button(
                    onClick = { onStep(1) },
                    enabled = controlsEnabled && session.canStepNext,
                    modifier = Modifier.weight(1f),
                ) {
                    Text("Next")
                }
            }

            JumpControls(
                session = session,
                enabled = controlsEnabled,
                onJumpFrame = onJumpFrame,
                onJumpTimestampUs = onJumpTimestampUs,
            )

            Text(
                text = "Pixels are decoded from the authoritative source timeline; lossy preview proxies are not used for this frame.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
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
            text = frame.timestampUs?.let { "Timestamp: ${formatMicros(it)} ($it µs)" }
                ?: "Timestamp: unavailable",
            style = MaterialTheme.typography.bodyMedium,
        )
        Text(
            text = "PTS: ${frame.timestampTicks ?: "unknown"} ticks · time base ${frame.timeBaseNumerator}/${frame.timeBaseDenominator}",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        frame.durationTicks?.let {
            Text(
                text = "Duration: $it ticks",
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
                    ?.let { onJumpFrame(it - 1) }
            },
            enabled = enabled && frameInput.toLongOrNull()?.let { it in 1..session.frameCount } == true,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Jump to exact frame")
        }

        OutlinedTextField(
            value = timestampInput,
            onValueChange = { value -> timestampInput = value.filter(Char::isDigit).take(19) },
            enabled = enabled,
            modifier = Modifier.fillMaxWidth(),
            label = { Text("Jump to timestamp (µs)") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
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

private suspend fun MicroscopeFrame.toArgbBitmapOrNull(): Bitmap? {
    val metadata = descriptor
    if (!metadata.isSane()) return null

    var bitmap: Bitmap? = null
    try {
        bitmap = Bitmap.createBitmap(metadata.width, metadata.height, Bitmap.Config.ARGB_8888)
        val source = rgba.duplicate()
        val row = IntArray(metadata.width)
        val stride = metadata.strideBytes

        for (y in 0 until metadata.height) {
            currentCoroutineContext().ensureActive()
            val rowStart = Math.multiplyExact(y.toLong(), stride)
            for (x in 0 until metadata.width) {
                val offsetLong = Math.addExact(rowStart, x.toLong() * 4L)
                val offset = Math.toIntExact(offsetLong)
                val red = source.get(offset).toInt() and 0xff
                val green = source.get(offset + 1).toInt() and 0xff
                val blue = source.get(offset + 2).toInt() and 0xff
                val alpha = source.get(offset + 3).toInt() and 0xff
                row[x] = (alpha shl 24) or (red shl 16) or (green shl 8) or blue
            }
            bitmap.setPixels(row, 0, metadata.width, 0, y, metadata.width, 1)
        }
        return bitmap
    } catch (cancelled: CancellationException) {
        bitmap?.recycle()
        throw cancelled
    } catch (_: RuntimeException) {
        bitmap?.recycle()
        return null
    } catch (_: ArithmeticException) {
        bitmap?.recycle()
        return null
    } catch (_: OutOfMemoryError) {
        bitmap?.recycle()
        return null
    }
}

private fun formatMicros(timestampUs: Long): String {
    val nonNegative = timestampUs.coerceAtLeast(0L)
    val totalSeconds = nonNegative / 1_000_000L
    val micros = nonNegative % 1_000_000L
    val hours = totalSeconds / 3_600L
    val minutes = (totalSeconds % 3_600L) / 60L
    val seconds = totalSeconds % 60L
    return if (hours > 0L) {
        String.format(Locale.US, "%d:%02d:%02d.%06d", hours, minutes, seconds, micros)
    } else {
        String.format(Locale.US, "%02d:%02d.%06d", minutes, seconds, micros)
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
