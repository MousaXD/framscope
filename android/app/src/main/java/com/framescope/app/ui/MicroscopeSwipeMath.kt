package com.framescope.app.ui

import kotlin.math.abs
import kotlin.math.max

internal object MicroscopeSwipeMath {
    private const val VIEWPORT_FRACTION = 0.18f

    fun direction(
        horizontalDragPx: Float,
        viewportWidthPx: Float,
        minimumDistancePx: Float,
    ): Int? {
        if (!horizontalDragPx.isFinite() || !viewportWidthPx.isFinite() || !minimumDistancePx.isFinite()) {
            return null
        }
        if (viewportWidthPx <= 0f || minimumDistancePx <= 0f) return null

        val threshold = max(minimumDistancePx, viewportWidthPx * VIEWPORT_FRACTION)
        if (abs(horizontalDragPx) < threshold) return null

        return if (horizontalDragPx < 0f) 1 else -1
    }
}
