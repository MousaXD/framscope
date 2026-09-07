package com.framescope.app.ui

import kotlin.math.roundToLong

internal object MicroscopeTimelineMath {
    fun fractionForFrame(
        frameId: Long,
        frameCount: Long,
    ): Float {
        if (frameCount <= 1L) return 0f
        val lastFrameId = frameCount - 1L
        val boundedFrameId = frameId.coerceIn(0L, lastFrameId)
        return (boundedFrameId.toDouble() / lastFrameId.toDouble())
            .toFloat()
            .coerceIn(0f, 1f)
    }

    fun frameForFraction(
        fraction: Float,
        frameCount: Long,
    ): Long? {
        if (frameCount <= 0L || fraction.isNaN()) return null
        if (frameCount == 1L) return 0L

        val lastFrameId = frameCount - 1L
        val boundedFraction = fraction.coerceIn(0f, 1f).toDouble()
        return (lastFrameId.toDouble() * boundedFraction)
            .roundToLong()
            .coerceIn(0L, lastFrameId)
    }

    fun boundedStepTarget(
        currentFrameId: Long,
        frameCount: Long,
        delta: Long,
    ): Long? {
        if (frameCount <= 0L) return null
        val lastFrameId = frameCount - 1L
        if (currentFrameId !in 0L..lastFrameId) return null
        if (delta == 0L) return currentFrameId

        return if (delta > 0L) {
            val remaining = lastFrameId - currentFrameId
            currentFrameId + delta.coerceAtMost(remaining)
        } else {
            val minimumDeltaWithoutUnderflow = -currentFrameId
            if (delta < minimumDeltaWithoutUnderflow) {
                0L
            } else {
                currentFrameId + delta
            }
        }
    }

    fun framePositionLabel(
        frameId: Long,
        frameCount: Long,
    ): String? {
        if (frameCount <= 0L || frameId !in 0L until frameCount) return null
        return "Frame ${frameId + 1L} of $frameCount"
    }
}
