package com.framescope.app.performance

/**
 * Pure scheduling policy for Android indexing performance hints.
 *
 * Native indexing progress is sampled by the Rust operation itself and may be re-read by Android
 * polling. Only monotonic, advancing native samples form an ADPF work cycle. This deliberately
 * avoids turning the Kotlin poll cadence into synthetic work timing.
 */
internal data class IndexingWorkSample(
    val sequence: Long,
    val sampleElapsedMs: Long,
    val workUnits: Long,
)

internal class IndexingWorkCycleTracker {
    private var highestSequence = -1L
    private var lastAdvancingSample: IndexingWorkSample? = null

    /**
     * Returns the elapsed duration of one genuine native work cycle, or null when this observation
     * is duplicated, stale, non-advancing, or cannot form a positive duration.
     */
    fun observe(sample: IndexingWorkSample): Long? {
        if (sample.sequence < 0L || sample.sampleElapsedMs < 0L || sample.workUnits < 0L) {
            return null
        }
        if (sample.sequence <= highestSequence) {
            return null
        }
        highestSequence = sample.sequence

        val previous = lastAdvancingSample
        if (previous == null) {
            lastAdvancingSample = sample
            return null
        }
        if (sample.workUnits <= previous.workUnits || sample.sampleElapsedMs <= previous.sampleElapsedMs) {
            return null
        }

        lastAdvancingSample = sample
        val deltaMs = sample.sampleElapsedMs - previous.sampleElapsedMs
        return millisecondsToNanosecondsSaturated(deltaMs)
    }
}

/**
 * ADPF requires a positive target duration. We calibrate from observed native work rather than a
 * guessed FPS target, while bounding corrupt/absurd samples before they reach the platform API.
 */
internal fun calibratedTargetDurationNanos(observedDurationNanos: Long): Long =
    observedDurationNanos.coerceIn(MIN_TARGET_NANOS, MAX_TARGET_NANOS)

private fun millisecondsToNanosecondsSaturated(milliseconds: Long): Long {
    if (milliseconds <= 0L) return 0L
    if (milliseconds > Long.MAX_VALUE / NANOS_PER_MILLISECOND) return Long.MAX_VALUE
    return milliseconds * NANOS_PER_MILLISECOND
}

private const val NANOS_PER_MILLISECOND = 1_000_000L
private const val MIN_TARGET_NANOS = 1_000_000L
private const val MAX_TARGET_NANOS = 5_000_000_000L
