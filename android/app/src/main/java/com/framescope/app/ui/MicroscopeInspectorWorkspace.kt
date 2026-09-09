package com.framescope.app.ui

import android.graphics.Bitmap
import androidx.compose.foundation.Image
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.weight
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.framescope.app.data.MicroscopeFrame
import com.framescope.app.data.MicroscopeSessionSnapshot
import com.framescope.app.data.VideoMetadata
import java.nio.ByteBuffer
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext

private sealed interface InspectorBitmapState {
    data object Loading : InspectorBitmapState

    data class Ready(val bitmap: Bitmap) : InspectorBitmapState

    data class Error(val message: String) : InspectorBitmapState
}

private data class InspectorTransform(
    val scale: Float = 1f,
    val translationX: Float = 0f,
    val translationY: Float = 0f,
)

@Composable
internal fun MicroscopeInspectorWorkspace(
    state: MicroscopeUiState,
    metadata: VideoMetadata?,
    onStep: (Int) -> Unit,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
) {
    val session = state.inspectorSession()
    val frame = state.inspectorFrame()
    val controlsEnabled = state is MicroscopeUiState.Ready

    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("microscope_inspector_workspace"),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Text(
                text = "Frame Inspector",
                style = MaterialTheme.typography.titleLarge,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                text = "AUTHORITATIVE DECODED FRAME · FULL RESOLUTION",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.primary,
            )

            if (session == null || frame == null) {
                when (state) {
                    MicroscopeUiState.Opening,
                    is MicroscopeUiState.LoadingFrame,
                    -> InspectorBusyMessage("Preparing the authoritative frame…")
                    is MicroscopeUiState.Empty -> Text("This indexed video has no presentation frames.")
                    is MicroscopeUiState.Error -> Text(
                        text = state.message,
                        color = MaterialTheme.colorScheme.error,
                    )
                    else -> Text("No authoritative frame is available yet.")
                }
                return@Column
            }

            if (state is MicroscopeUiState.Navigating) {
                InspectorBusyMessage("Resolving the next exact indexed frame. Showing the retained authoritative frame meanwhile.")
            }

            InspectorFrameSurface(
                frame = frame,
                session = session,
                controlsEnabled = controlsEnabled,
                onStep = onStep,
            )
            InspectorFrameIdentity(
                session = session,
                frame = frame,
                metadata = metadata,
            )
            InspectorExactNavigation(
                session = session,
                enabled = controlsEnabled,
                onStep = onStep,
                onJumpFrame = onJumpFrame,
                onJumpTimestampUs = onJumpTimestampUs,
            )
            InspectorPixelSampler(frame)
        }
    }
}

private fun MicroscopeUiState.inspectorSession(): MicroscopeSessionSnapshot? = when (this) {
    is MicroscopeUiState.LoadingFrame -> session
    is MicroscopeUiState.Navigating -> session
    is MicroscopeUiState.Ready -> session
    is MicroscopeUiState.Empty -> session
    is MicroscopeUiState.Error -> session
    else -> null
}

private fun MicroscopeUiState.inspectorFrame(): MicroscopeFrame? = when (this) {
    is MicroscopeUiState.Ready -> frame
    is MicroscopeUiState.Navigating -> previousFrame
    else -> null
}

@Composable
private fun InspectorFrameSurface(
    frame: MicroscopeFrame,
    session: MicroscopeSessionSnapshot,
    controlsEnabled: Boolean,
    onStep: (Int) -> Unit,
) {
    val bitmapState by produceState<InspectorBitmapState>(
        initialValue = InspectorBitmapState.Loading,
        key1 = frame,
    ) {
        value = withContext(Dispatchers.Default) { frame.toFullResolutionBitmap() }
    }
    RecycleInspectorBitmap(bitmapState)

    var fullScreen by rememberSaveable(frame.descriptor.frameId) { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .height(420.dp)
                .testTag("inspector_frame_surface"),
            contentAlignment = Alignment.Center,
        ) {
            when (val current = bitmapState) {
                InspectorBitmapState.Loading -> CircularProgressIndicator()
                is InspectorBitmapState.Error -> Text(
                    text = current.message,
                    color = MaterialTheme.colorScheme.error,
                )
                is InspectorBitmapState.Ready -> InspectorZoomableImage(
                    bitmap = current.bitmap,
                    contentDescription = "Authoritative frame ${frame.descriptor.frameId + 1} of ${session.frameCount}",
                    enabled = controlsEnabled,
                    modifier = Modifier.fillMaxSize(),
                )
            }
        }
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            OutlinedButton(
                onClick = { onStep(-1) },
                enabled = controlsEnabled && session.canStepPrevious,
                modifier = Modifier.weight(1f),
            ) { Text("Previous exact") }
            Button(
                onClick = { onStep(1) },
                enabled = controlsEnabled && session.canStepNext,
                modifier = Modifier.weight(1f),
            ) { Text("Next exact") }
        }
        val readyBitmap = (bitmapState as? InspectorBitmapState.Ready)?.bitmap
        OutlinedButton(
            onClick = { fullScreen = true },
            enabled = readyBitmap != null,
            modifier = Modifier
                .fillMaxWidth()
                .testTag("inspector_fullscreen_action"),
        ) { Text("Inspect full screen") }

        if (fullScreen && readyBitmap != null) {
            Dialog(
                onDismissRequest = { fullScreen = false },
                properties = DialogProperties(usePlatformDefaultWidth = false),
            ) {
                Surface(modifier = Modifier.fillMaxSize()) {
                    Box(modifier = Modifier.fillMaxSize()) {
                        InspectorZoomableImage(
                            bitmap = readyBitmap,
                            contentDescription = "Full-screen authoritative frame ${frame.descriptor.frameId + 1}",
                            enabled = true,
                            modifier = Modifier
                                .fillMaxSize()
                                .testTag("inspector_fullscreen_surface"),
                        )
                        OutlinedButton(
                            onClick = { fullScreen = false },
                            modifier = Modifier
                                .align(Alignment.TopEnd)
                                .padding(16.dp),
                        ) { Text("Close") }
                    }
                }
            }
        }
    }
}

@Composable
private fun RecycleInspectorBitmap(state: InspectorBitmapState) {
    val ready = state as? InspectorBitmapState.Ready ?: return
    DisposableEffect(ready.bitmap) {
        onDispose {
            if (!ready.bitmap.isRecycled) ready.bitmap.recycle()
        }
    }
}

private suspend fun MicroscopeFrame.toFullResolutionBitmap(): InspectorBitmapState {
    val descriptor = descriptor
    if (!descriptor.isSane()) {
        return InspectorBitmapState.Error("Frame metadata is outside presentation safety bounds.")
    }
    var bitmap: Bitmap? = null
    return try {
        bitmap = Bitmap.createBitmap(descriptor.width, descriptor.height, Bitmap.Config.ARGB_8888)
        val source = rgba.duplicate()
        val row = IntArray(descriptor.width)
        for (y in 0 until descriptor.height) {
            currentCoroutineContext().ensureActive()
            val rowStart = Math.multiplyExact(y.toLong(), descriptor.strideBytes)
            for (x in 0 until descriptor.width) {
                val offset = Math.toIntExact(Math.addExact(rowStart, x.toLong() * 4L))
                val red = source.get(offset).toInt() and 0xff
                val green = source.get(offset + 1).toInt() and 0xff
                val blue = source.get(offset + 2).toInt() and 0xff
                val alpha = source.get(offset + 3).toInt() and 0xff
                row[x] = (alpha shl 24) or (red shl 16) or (green shl 8) or blue
            }
            bitmap.setPixels(row, 0, descriptor.width, 0, y, descriptor.width, 1)
        }
        InspectorBitmapState.Ready(bitmap)
    } catch (cancelled: CancellationException) {
        bitmap?.recycle()
        throw cancelled
    } catch (_: OutOfMemoryError) {
        bitmap?.recycle()
        InspectorBitmapState.Error(
            "Full-resolution inspection could not allocate an additional bitmap. The authoritative frame was not downsampled.",
        )
    } catch (_: RuntimeException) {
        bitmap?.recycle()
        InspectorBitmapState.Error("The authoritative frame could not be converted for full-resolution inspection.")
    }
}

@Composable
private fun InspectorZoomableImage(
    bitmap: Bitmap,
    contentDescription: String,
    enabled: Boolean,
    modifier: Modifier = Modifier,
) {
    val imageBitmap = remember(bitmap) { bitmap.asImageBitmap() }
    var viewportSize by remember(bitmap) { mutableStateOf(IntSize.Zero) }
    var transform by remember(bitmap) { mutableStateOf(InspectorTransform()) }
    val plan = MicroscopeInspectorMath.viewportPlan(
        imageWidth = bitmap.width,
        imageHeight = bitmap.height,
        viewportWidth = viewportSize.width,
        viewportHeight = viewportSize.height,
    )
    val transformableState = rememberTransformableState { centroid, zoomChange, panChange, _ ->
        val currentPlan = MicroscopeInspectorMath.viewportPlan(
            imageWidth = bitmap.width,
            imageHeight = bitmap.height,
            viewportWidth = viewportSize.width,
            viewportHeight = viewportSize.height,
        ) ?: return@rememberTransformableState
        val currentScale = transform.scale
            .takeIf { it.isFinite() }
            ?.coerceIn(currentPlan.minimumScale, currentPlan.maximumScale)
            ?: 1f.coerceIn(currentPlan.minimumScale, currentPlan.maximumScale)
        val safeZoom = zoomChange.takeIf { it.isFinite() && it > 0f } ?: 1f
        val nextScale = (currentScale * safeZoom)
            .coerceIn(currentPlan.minimumScale, currentPlan.maximumScale)
        val effectiveZoom = nextScale / currentScale
        val focalX = if (centroid.x.isFinite()) centroid.x - viewportSize.width / 2f else 0f
        val focalY = if (centroid.y.isFinite()) centroid.y - viewportSize.height / 2f else 0f
        val candidateX = transform.translationX * effectiveZoom +
            focalX * (1f - effectiveZoom) + panChange.x
        val candidateY = transform.translationY * effectiveZoom +
            focalY * (1f - effectiveZoom) + panChange.y
        val (clampedX, clampedY) = MicroscopeInspectorMath.clampTranslation(
            translationX = candidateX,
            translationY = candidateY,
            relativeScale = nextScale,
            imageWidth = bitmap.width,
            imageHeight = bitmap.height,
            viewportWidth = viewportSize.width,
            viewportHeight = viewportSize.height,
        )
        transform = InspectorTransform(nextScale, clampedX, clampedY)
    }

    Box(
        modifier = modifier
            .clipToBounds()
            .onSizeChanged { size ->
                viewportSize = size
                val currentPlan = MicroscopeInspectorMath.viewportPlan(
                    imageWidth = bitmap.width,
                    imageHeight = bitmap.height,
                    viewportWidth = size.width,
                    viewportHeight = size.height,
                )
                if (currentPlan != null) {
                    val scale = transform.scale.coerceIn(currentPlan.minimumScale, currentPlan.maximumScale)
                    val (x, y) = MicroscopeInspectorMath.clampTranslation(
                        transform.translationX,
                        transform.translationY,
                        scale,
                        bitmap.width,
                        bitmap.height,
                        size.width,
                        size.height,
                    )
                    transform = InspectorTransform(scale, x, y)
                }
            }
            .transformable(
                state = transformableState,
                canPan = { true },
                lockRotationOnZoomPan = true,
                enabled = enabled,
            ),
    ) {
        Image(
            bitmap = imageBitmap,
            contentDescription = contentDescription,
            modifier = Modifier
                .fillMaxSize()
                .graphicsLayer {
                    transformOrigin = TransformOrigin.Center
                    scaleX = transform.scale
                    scaleY = transform.scale
                    translationX = transform.translationX
                    translationY = transform.translationY
                },
            contentScale = ContentScale.Fit,
        )
        Row(
            modifier = Modifier
                .align(Alignment.TopStart)
                .padding(8.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            OutlinedButton(
                onClick = { transform = InspectorTransform() },
                enabled = enabled,
            ) { Text("Fit") }
            OutlinedButton(
                onClick = {
                    plan?.let { viewportPlan ->
                        transform = InspectorTransform(scale = viewportPlan.oneToOneScale)
                    }
                },
                enabled = enabled && plan != null,
                modifier = Modifier.testTag("inspector_one_to_one"),
            ) { Text("1:1 pixels") }
        }
    }
}

@Composable
private fun InspectorFrameIdentity(
    session: MicroscopeSessionSnapshot,
    frame: MicroscopeFrame,
    metadata: VideoMetadata?,
) {
    val details = session.currentFrame
    val descriptor = frame.descriptor
    Column(
        modifier = Modifier.testTag("inspector_metadata"),
        verticalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        Text("Exact frame identity", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Medium)
        Text("Frame ${descriptor.frameId + 1} / ${session.frameCount} · id ${descriptor.frameId}")
        Text(
            details?.timestampUs?.let {
                "Timestamp ${MicroscopePreviewMath.formatTimestampUs(it)} · $it µs"
            } ?: "Timestamp unavailable",
        )
        Text("PTS ${details?.let(MicroscopeUiFormatter::exactTimestamp) ?: "Unavailable"}")
        Text("DTS Unavailable · not exposed by the persistent frame index")
        Text("Frame duration ${details?.let(MicroscopeUiFormatter::exactDuration) ?: "Unavailable"}")
        Text("Flags keyframe=${details?.keyframe ?: false} · corrupt=${details?.corrupt ?: false}")
        Text("Decoded resolution ${descriptor.width} × ${descriptor.height}")
        Text("Codec ${metadata?.codec ?: "Unavailable"}")
        Text("Source pixel format ${metadata?.pixelFormat ?: "Unavailable"}")
        Text("Inspector handoff RGBA8888 · stride ${descriptor.strideBytes} bytes")
        Text("Color primaries / transfer / matrix / range Unavailable · not exposed by the current metadata bridge")
    }
}

@Composable
private fun InspectorExactNavigation(
    session: MicroscopeSessionSnapshot,
    enabled: Boolean,
    onStep: (Int) -> Unit,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
) {
    var frameInput by rememberSaveable(session.sessionId) { mutableStateOf("") }
    var timestampInput by rememberSaveable(session.sessionId) { mutableStateOf("") }
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text("Exact navigation", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Medium)
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
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
        OutlinedTextField(
            value = frameInput,
            onValueChange = { frameInput = it.filter(Char::isDigit).take(19) },
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
        ) { Text("Jump to exact frame") }
        OutlinedTextField(
            value = timestampInput,
            onValueChange = { timestampInput = MicroscopePreviewMath.sanitizeSignedTimestampInput(it) },
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
        ) { Text("Jump to nearest indexed timestamp") }
    }
}

@Composable
private fun InspectorPixelSampler(frame: MicroscopeFrame) {
    var xInput by rememberSaveable(frame.descriptor.frameId) { mutableStateOf("0") }
    var yInput by rememberSaveable(frame.descriptor.frameId) { mutableStateOf("0") }
    var sample by remember(frame) { mutableStateOf<InspectorRgbaSample?>(null) }
    val x = xInput.toIntOrNull()
    val y = yInput.toIntOrNull()
    val valid = x != null && y != null &&
        x in 0 until frame.descriptor.width && y in 0 until frame.descriptor.height

    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text("Pixel sampler", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Medium)
        Text(
            text = "Samples the authoritative decoded RGBA handoff. Source YUV planes are not exposed by this bridge.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            OutlinedTextField(
                value = xInput,
                onValueChange = { xInput = it.filter(Char::isDigit).take(5) },
                modifier = Modifier.weight(1f),
                label = { Text("X 0…${frame.descriptor.width - 1}") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
            )
            OutlinedTextField(
                value = yInput,
                onValueChange = { yInput = it.filter(Char::isDigit).take(5) },
                modifier = Modifier.weight(1f),
                label = { Text("Y 0…${frame.descriptor.height - 1}") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
            )
        }
        OutlinedButton(
            onClick = {
                if (x != null && y != null) {
                    sample = MicroscopeInspectorMath.sampleRgba(frame.descriptor, frame.rgba, x, y)
                }
            },
            enabled = valid,
            modifier = Modifier.fillMaxWidth(),
        ) { Text("Sample decoded pixel") }
        sample?.let {
            Text(
                text = "(${it.x}, ${it.y}) · R ${it.red} · G ${it.green} · B ${it.blue} · A ${it.alpha}",
                modifier = Modifier.testTag("inspector_pixel_sample"),
                style = MaterialTheme.typography.bodyMedium,
            )
        }
    }
}

@Composable
private fun InspectorBusyMessage(message: String) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        CircularProgressIndicator()
        Text(message, style = MaterialTheme.typography.bodyMedium)
    }
}
