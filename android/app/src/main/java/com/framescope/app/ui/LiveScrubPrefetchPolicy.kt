package com.framescope.app.ui

import kotlin.math.abs

/**
 * Small deterministic admission policy for disposable scrub warming.
 *
 * It observes only frames that were actually resolved and published by demand work. Stable motion
 * earns one directional neighbor; faster cadence/velocity increases that look-ahead slightly. The
 * first sample, reversals, long pauses, disabled acceleration, and invalid frame bounds never
 * speculate. The caller remains responsible for giving demand and exact navigation priority over
 * any admitted prefetch.
 */
internal class LiveScrubPrefetchPolicy {
    private var sessionId: Long? = null
    private var lastFrameId: Long? = null
    private var lastCompletedAtMs: Long? = null
    private var lastDirection = 0

    fun candidate(
        sessionId: Long,
        frameId: Long,
        frameCount: Long,
        completedAtMs: Long,
        accelerationEnabled: Boolean,
    ): Long? {
        if (sessionId <= 0L || frameId < 0L || frameCount <= 1L || frameId >= frameCount) {
            reset()
            return null
        }
        if (this.sessionId != sessionId) {
            reset()
            this.sessionId = sessionId
        }

        val previousFrame = lastFrameId
        val previousAt = lastCompletedAtMs
        lastFrameId = frameId
        lastCompletedAtMs = completedAtMs

        if (!accelerationEnabled || previousFrame == null || previousAt == null) {
            lastDirection = 0
            return null
        }

        val delta = frameId - previousFrame
        if (delta == 0L) return null
        val direction = if (delta > 0L) 1 else -1
        if (lastDirection != 0 && direction != lastDirection) {
            lastDirection = direction
            return null
        }
        lastDirection = direction

        val elapsedMs = completedAtMs - previousAt
        if (elapsedMs <= 0L || elapsedMs > MAX_PREFETCH_CADENCE_MS) return null

        val framesPerSecond = abs(delta).toDouble() * 1_000.0 / elapsedMs.toDouble()
        val lookAhead = when {
            elapsedMs <= FAST_CADENCE_MS || framesPerSecond >= FAST_VELOCITY_FPS -> 3L
            elapsedMs <= MEDIUM_CADENCE_MS || framesPerSecond >= MEDIUM_VELOCITY_FPS -> 2L
            else -> 1L
        }
        val candidate = if (direction > 0) {
            frameId + lookAhead
        } else {
            frameId - lookAhead
        }
        return candidate.takeIf { it in 0 until frameCount && it != frameId }
    }

    fun reset() {
        sessionId = null
        lastFrameId = null
        lastCompletedAtMs = null
        lastDirection = 0
    }

    private companion object {
        const val FAST_CADENCE_MS = 90L
        const val MEDIUM_CADENCE_MS = 180L
        const val MAX_PREFETCH_CADENCE_MS = 320L
        const val FAST_VELOCITY_FPS = 60.0
        const val MEDIUM_VELOCITY_FPS = 20.0
    }
}
