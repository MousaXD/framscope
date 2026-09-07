package com.framescope.app.ui

internal data class MicroscopeViewportTransform(
    val scale: Float = 1f,
    val translationX: Float = 0f,
    val translationY: Float = 0f,
)

internal object MicroscopeTransformMath {
    const val MIN_SCALE = 1f
    const val MAX_SCALE = 8f

    fun applyGesture(
        current: MicroscopeViewportTransform,
        zoomChange: Float,
        panX: Float,
        panY: Float,
        centroidX: Float,
        centroidY: Float,
        viewportWidth: Float,
        viewportHeight: Float,
    ): MicroscopeViewportTransform {
        val safeCurrentScale = current.scale
            .takeIf { it.isFinite() && it in MIN_SCALE..MAX_SCALE }
            ?: MIN_SCALE
        val safeZoomChange = zoomChange.takeIf { it.isFinite() && it > 0f } ?: 1f
        val nextScale = (safeCurrentScale * safeZoomChange)
            .coerceIn(MIN_SCALE, MAX_SCALE)
        val viewportIsUsable =
            viewportWidth.isFinite() && viewportHeight.isFinite() &&
                viewportWidth > 0f && viewportHeight > 0f

        if (nextScale <= MIN_SCALE || !viewportIsUsable) {
            return MicroscopeViewportTransform(scale = nextScale)
        }

        val effectiveZoom = nextScale / safeCurrentScale
        val focalX = if (centroidX.isFinite()) centroidX - viewportWidth / 2f else 0f
        val focalY = if (centroidY.isFinite()) centroidY - viewportHeight / 2f else 0f
        val safePanX = panX.takeIf(Float::isFinite) ?: 0f
        val safePanY = panY.takeIf(Float::isFinite) ?: 0f
        val currentX = current.translationX.takeIf(Float::isFinite) ?: 0f
        val currentY = current.translationY.takeIf(Float::isFinite) ?: 0f

        val candidateX = currentX * effectiveZoom + focalX * (1f - effectiveZoom) + safePanX
        val candidateY = currentY * effectiveZoom + focalY * (1f - effectiveZoom) + safePanY
        val maxX = viewportWidth * (nextScale - 1f) / 2f
        val maxY = viewportHeight * (nextScale - 1f) / 2f

        return MicroscopeViewportTransform(
            scale = nextScale,
            translationX = candidateX.coerceIn(-maxX, maxX),
            translationY = candidateY.coerceIn(-maxY, maxY),
        )
    }
}
