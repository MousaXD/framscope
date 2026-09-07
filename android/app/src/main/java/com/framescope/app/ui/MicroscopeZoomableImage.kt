package com.framescope.app.ui

import android.graphics.Bitmap
import androidx.compose.foundation.Image
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp

@Composable
internal fun MicroscopeZoomableImage(
    bitmap: Bitmap,
    contentDescription: String,
    enabled: Boolean,
    swipeEnabled: Boolean = false,
    onSwipe: (Int) -> Unit = {},
    modifier: Modifier = Modifier,
) {
    val imageBitmap = remember(bitmap) { bitmap.asImageBitmap() }
    val minimumSwipeDistancePx = with(LocalDensity.current) { 64.dp.toPx() }
    var viewportSize by remember(bitmap) { mutableStateOf(IntSize.Zero) }
    var transform by remember(bitmap) { mutableStateOf(MicroscopeViewportTransform()) }
    val transformableState = rememberTransformableState { centroid, zoomChange, panChange, _ ->
        transform = MicroscopeTransformMath.applyGesture(
            current = transform,
            zoomChange = zoomChange,
            panX = panChange.x,
            panY = panChange.y,
            centroidX = centroid.x,
            centroidY = centroid.y,
            viewportWidth = viewportSize.width.toFloat(),
            viewportHeight = viewportSize.height.toFloat(),
        )
    }

    Box(
        modifier = modifier
            .clipToBounds()
            .onSizeChanged { size ->
                viewportSize = size
                transform = MicroscopeTransformMath.applyGesture(
                    current = transform,
                    zoomChange = 1f,
                    panX = 0f,
                    panY = 0f,
                    centroidX = size.width / 2f,
                    centroidY = size.height / 2f,
                    viewportWidth = size.width.toFloat(),
                    viewportHeight = size.height.toFloat(),
                )
            }
            .pointerInput(bitmap, enabled, swipeEnabled, transform.scale, viewportSize) {
                if (!enabled || !swipeEnabled || transform.scale > MicroscopeTransformMath.MIN_SCALE) {
                    return@pointerInput
                }
                var totalHorizontalDrag by mutableFloatStateOf(0f)
                detectHorizontalDragGestures(
                    onDragStart = { totalHorizontalDrag = 0f },
                    onHorizontalDrag = { _, dragAmount -> totalHorizontalDrag += dragAmount },
                    onDragCancel = { totalHorizontalDrag = 0f },
                    onDragEnd = {
                        MicroscopeSwipeMath.direction(
                            horizontalDragPx = totalHorizontalDrag,
                            viewportWidthPx = viewportSize.width.toFloat(),
                            minimumDistancePx = minimumSwipeDistancePx,
                        )?.let(onSwipe)
                        totalHorizontalDrag = 0f
                    },
                )
            }
            .transformable(
                state = transformableState,
                canPan = { transform.scale > MicroscopeTransformMath.MIN_SCALE },
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

        if (transform.scale > MicroscopeTransformMath.MIN_SCALE) {
            OutlinedButton(
                onClick = { transform = MicroscopeViewportTransform() },
                enabled = enabled,
                modifier = Modifier
                    .align(Alignment.TopEnd)
                    .padding(8.dp),
            ) {
                Text("Reset zoom")
            }
        }
    }
}
