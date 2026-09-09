package com.framescope.app.ui

import kotlin.math.abs
import kotlin.math.ceil

/**
 * Velocity-aware admission for disposable live-scrub preview work.
 *
 * Pointer samples are never throttled here: the slider owns its gesture fraction independently.
 * This policy only decides how often a sample is worth turning into native preview work. Fast
 * movement favors low age-of-information by spacing requests far enough apart for useful work to
 * complete; slow movement admits more frequently so the visible preview can converge toward the
 * finger. Exact release navigation never consults this policy.
 */
internal class LiveScrubAdmissionPolicy {
    private var sessionId: Long? = null
    private var lastObservedFraction: Float? = null
    private var lastObservedAtNanos: Long? = null
    private var lastDirection = 0
    private var lastAdmittedFraction: Float? = null
    private var lastAdmittedAtNanos: Long? = null
    private var lastAdmittedTargetKey: Long? = null

    fun plan(
        sessionId: Long,
        fraction: Float,
        targetKey: Long,
        observedAtNanos: Long,
    ): LiveScrubAdmissionPlan {
        if (sessionId <= 0L || !fraction.isFinite() || observedAtNanos < 0L) {
            reset()
            return LiveScrubAdmissionPlan.NoPreview
        }
        if (this.sessionId != sessionId) {
            reset()
            this.sessionId = sessionId
        }

        val clampedFraction = fraction.coerceIn(0f, 1f)
        val previousFraction = lastObservedFraction
        val previousAt = lastObservedAtNanos
        val previousDirection = lastDirection

        val sampleDelta = previousFraction?.let { clampedFraction - it } ?: 0f
        val sampleDirection = when {
            sampleDelta > DIRECTION_EPSILON -> 1
            sampleDelta < -DIRECTION_EPSILON -> -1
            else -> 0
        }
        val elapsedSampleNanos = if (previousAt != null && observedAtNanos >= previousAt) {
            observedAtNanos - previousAt
        } else {
            0L
        }
        val velocityPerSecond = if (elapsedSampleNanos > 0L) {
            abs(sampleDelta).toDouble() * NANOS_PER_SECOND.toDouble() / elapsedSampleNanos.toDouble()
        } else {
            0.0
        }

        lastObservedFraction = clampedFraction
        lastObservedAtNanos = observedAtNanos
        if (sampleDirection != 0) lastDirection = sampleDirection

        if (lastAdmittedTargetKey == targetKey) {
            return LiveScrubAdmissionPlan.NoPreview
        }

        val admittedAt = lastAdmittedAtNanos
        val admittedFraction = lastAdmittedFraction
        if (admittedAt == null || admittedFraction == null) {
            return LiveScrubAdmissionPlan.Immediate
        }

        val elapsedSinceAdmission = (observedAtNanos - admittedAt).coerceAtLeast(0L)
        val minimumInterval = minimumIntervalNanos(velocityPerSecond)
        val reversed = previousDirection != 0 && sampleDirection != 0 && sampleDirection != previousDirection
        val largeJump = abs(clampedFraction - admittedFraction) >= LARGE_JUMP_FRACTION
        val reversalCanInterrupt = reversed && elapsedSinceAdmission >= MIN_REVERSAL_GAP_NANOS
        val largeJumpCanInterrupt = largeJump && elapsedSinceAdmission >= LARGE_JUMP_GAP_NANOS

        if (
            elapsedSinceAdmission >= minimumInterval ||
            reversalCanInterrupt ||
            largeJumpCanInterrupt
        ) {
            return LiveScrubAdmissionPlan.Immediate
        }

        val nextEligibleAt = minOf(
            minimumInterval,
            if (largeJump) LARGE_JUMP_GAP_NANOS else Long.MAX_VALUE,
        )
        val remainingNanos = (nextEligibleAt - elapsedSinceAdmission).coerceAtLeast(1L)
        val delayMs = ceil(remainingNanos.toDouble() / NANOS_PER_MILLISECOND.toDouble())
            .toLong()
            .coerceAtLeast(1L)
        return LiveScrubAdmissionPlan.After(delayMs)
    }

    fun markAdmitted(
        fraction: Float,
        targetKey: Long,
        admittedAtNanos: Long,
    ) {
        if (sessionId == null || !fraction.isFinite() || admittedAtNanos < 0L) return
        lastAdmittedFraction = fraction.coerceIn(0f, 1f)
        lastAdmittedTargetKey = targetKey
        lastAdmittedAtNanos = admittedAtNanos
    }

    fun reset() {
        sessionId = null
        lastObservedFraction = null
        lastObservedAtNanos = null
        lastDirection = 0
        lastAdmittedFraction = null
        lastAdmittedAtNanos = null
        lastAdmittedTargetKey = null
    }

    private fun minimumIntervalNanos(velocityPerSecond: Double): Long = when {
        velocityPerSecond >= FAST_VELOCITY_FRACTIONS_PER_SECOND -> FAST_INTERVAL_NANOS
        velocityPerSecond >= MEDIUM_VELOCITY_FRACTIONS_PER_SECOND -> MEDIUM_INTERVAL_NANOS
        else -> SLOW_INTERVAL_NANOS
    }

    private companion object {
        const val NANOS_PER_MILLISECOND = 1_000_000L
        const val NANOS_PER_SECOND = 1_000_000_000L
        const val DIRECTION_EPSILON = 0.000_01f
        const val LARGE_JUMP_FRACTION = 0.10f

        // These are admission ceilings, not performance claims or correctness thresholds.
        const val FAST_INTERVAL_NANOS = 50L * NANOS_PER_MILLISECOND
        const val MEDIUM_INTERVAL_NANOS = 32L * NANOS_PER_MILLISECOND
        const val SLOW_INTERVAL_NANOS = 16L * NANOS_PER_MILLISECOND
        const val MIN_REVERSAL_GAP_NANOS = 12L * NANOS_PER_MILLISECOND
        const val LARGE_JUMP_GAP_NANOS = 32L * NANOS_PER_MILLISECOND
        const val FAST_VELOCITY_FRACTIONS_PER_SECOND = 1.5
        const val MEDIUM_VELOCITY_FRACTIONS_PER_SECOND = 0.35
    }
}

internal sealed interface LiveScrubAdmissionPlan {
    data object Immediate : LiveScrubAdmissionPlan

    data class After(
        val delayMs: Long,
    ) : LiveScrubAdmissionPlan

    data object NoPreview : LiveScrubAdmissionPlan
}
