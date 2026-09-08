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

    /** Map an indexed presentation timestamp onto a normalized visual timeline. */
    fun fractionForTimestamp(
        timestampUs: Long,
        startUs: Long,
        endUs: Long,
    ): Float? {
        if (endUs < startUs) return null
        if (startUs == endUs) return 0f
        val start = startUs.toDouble()
        val span = endUs.toDouble() - start
        if (!span.isFinite() || span <= 0.0) return null
        return ((timestampUs.coerceIn(startUs, endUs).toDouble() - start) / span)
            .toFloat()
            .coerceIn(0f, 1f)
    }

    /**
     * Map a transient slider fraction to presentation time without using nominal FPS.
     *
     * The returned timestamp is only a seek request. Rust still resolves the final authoritative
     * frame through the persistent timestamp index, so VFR correctness does not depend on this
     * interpolation landing on an existing frame timestamp.
     */
    fun timestampForFraction(
        fraction: Float,
        startUs: Long,
        endUs: Long,
    ): Long? {
        if (fraction.isNaN() || endUs < startUs) return null
        if (startUs == endUs) return startUs
        val boundedFraction = fraction.coerceIn(0f, 1f).toDouble()
        val start = startUs.toDouble()
        val span = endUs.toDouble() - start
        if (!span.isFinite() || span <= 0.0) return null
        val value = start + (span * boundedFraction)
        if (!value.isFinite()) return null
        return value.roundToLong().coerceIn(startUs, endUs)
    }

    fun durationUs(startUs: Long, endUs: Long): Long? {
        if (endUs < startUs) return null
        return runCatching { Math.subtractExact(endUs, startUs) }.getOrNull()
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
