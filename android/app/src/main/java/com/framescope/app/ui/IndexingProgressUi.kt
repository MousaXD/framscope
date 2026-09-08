package com.framescope.app.ui

import com.framescope.app.data.MicroscopeIndexingProgress
import com.framescope.app.data.MicroscopeIndexingStage
import kotlin.math.ceil

/** Truthful presentation model derived only from native index progress and known media duration. */
data class IndexingProgressUi(
    val stage: MicroscopeIndexingStage,
    val indexedFrames: Long,
    val reusedFrames: Long,
    val expectedReuseFrames: Long,
    val currentTimestampUs: Long?,
    val elapsedMs: Long,
    val framesPerSecond: Double?,
    val estimatedFraction: Double?,
    val estimatedRemainingSeconds: Long?,
)

/**
 * Smooths indexing throughput without inventing precision.
 *
 * Percentage is derived from exact presentation timestamps, never frame count multiplied by FPS.
 * ETA is withheld until several positive media-time samples have been observed for at least one
 * second. Validation/reuse stages intentionally expose counts only because they are not fresh-index
 * throughput. A partial-index rebuild resets all throughput history.
 */
class IndexingProgressEstimator(
    private val durationUs: Long?,
) {
    private var lastIndexedFrames: Long? = null
    private var lastTimestampUs: Long? = null
    private var lastElapsedMs: Long? = null
    private var firstRateElapsedMs: Long? = null
    private var smoothedFramesPerSecond: Double? = null
    private var smoothedMediaSecondsPerWallSecond: Double? = null
    private var positiveTimelineSamples = 0

    fun update(progress: MicroscopeIndexingProgress): IndexingProgressUi {
        if (progress.stage == MicroscopeIndexingStage.RebuildingIndex) {
            resetRates()
        }

        val isFreshIndexing = progress.stage == MicroscopeIndexingStage.Indexing
        if (isFreshIndexing) {
            observeThroughput(progress)
        } else if (progress.stage != MicroscopeIndexingStage.Finalizing) {
            // Do not let validation/reuse elapsed time become the baseline for fresh indexing.
            lastIndexedFrames = null
            lastTimestampUs = null
            lastElapsedMs = null
            firstRateElapsedMs = null
        }

        // A cached complete index can jump straight from "reusing" to "finalizing" without
        // traversing the timeline. Only carry percentage/rate into finalizing if fresh indexing
        // actually produced throughput samples.
        val hasFreshThroughput = smoothedFramesPerSecond != null
        val mayEstimateTimeline = isFreshIndexing ||
            (progress.stage == MicroscopeIndexingStage.Finalizing && hasFreshThroughput)
        val fraction = if (mayEstimateTimeline) estimatedFraction(progress) else null
        val eta = if (isFreshIndexing) estimatedEtaSeconds(progress, fraction) else null

        return IndexingProgressUi(
            stage = progress.stage,
            indexedFrames = progress.indexedFrames,
            reusedFrames = progress.reusedFrames,
            expectedReuseFrames = progress.expectedReuseFrames,
            currentTimestampUs = progress.currentTimestampUs,
            elapsedMs = progress.elapsedMs,
            framesPerSecond = if (mayEstimateTimeline) smoothedFramesPerSecond else null,
            estimatedFraction = fraction,
            estimatedRemainingSeconds = eta,
        )
    }

    private fun observeThroughput(progress: MicroscopeIndexingProgress) {
        val previousFrames = lastIndexedFrames
        val previousTimestamp = lastTimestampUs
        val previousElapsed = lastElapsedMs
        if (firstRateElapsedMs == null) firstRateElapsedMs = progress.elapsedMs

        if (previousFrames != null && previousElapsed != null) {
            val elapsedDeltaMs = progress.elapsedMs - previousElapsed
            val frameDelta = progress.indexedFrames - previousFrames
            if (elapsedDeltaMs > 0L && frameDelta > 0L) {
                val instantFramesPerSecond = frameDelta * 1_000.0 / elapsedDeltaMs
                smoothedFramesPerSecond = smooth(smoothedFramesPerSecond, instantFramesPerSecond)
            }

            if (previousTimestamp != null && progress.currentTimestampUs != null && elapsedDeltaMs > 0L) {
                val mediaDeltaUs = progress.currentTimestampUs - previousTimestamp
                if (mediaDeltaUs > 0L) {
                    val mediaSecondsPerWallSecond = mediaDeltaUs / (elapsedDeltaMs * 1_000.0)
                    smoothedMediaSecondsPerWallSecond = smooth(
                        smoothedMediaSecondsPerWallSecond,
                        mediaSecondsPerWallSecond,
                    )
                    positiveTimelineSamples += 1
                }
            }
        }

        lastIndexedFrames = progress.indexedFrames
        lastTimestampUs = progress.currentTimestampUs
        lastElapsedMs = progress.elapsedMs
    }

    private fun estimatedFraction(progress: MicroscopeIndexingProgress): Double? {
        val duration = durationUs?.takeIf { it > 0L } ?: return null
        val firstTimestamp = progress.firstTimestampUs ?: return null
        val currentTimestamp = progress.currentTimestampUs ?: return null
        val coveredUs = currentTimestamp - firstTimestamp
        if (coveredUs < 0L) return null
        return (coveredUs.toDouble() / duration.toDouble()).coerceIn(0.0, 1.0)
    }

    private fun estimatedEtaSeconds(
        progress: MicroscopeIndexingProgress,
        fraction: Double?,
    ): Long? {
        val duration = durationUs?.takeIf { it > 0L } ?: return null
        val firstTimestamp = progress.firstTimestampUs ?: return null
        val currentTimestamp = progress.currentTimestampUs ?: return null
        val timelineRate = smoothedMediaSecondsPerWallSecond?.takeIf { it > 0.0 } ?: return null
        val observedForMs = firstRateElapsedMs?.let { progress.elapsedMs - it } ?: return null
        if (positiveTimelineSamples < MIN_TIMELINE_SAMPLES || observedForMs < MIN_ETA_OBSERVATION_MS) {
            return null
        }
        val estimatedFraction = fraction ?: return null
        if (estimatedFraction < MIN_ETA_FRACTION || estimatedFraction >= MAX_ETA_FRACTION) return null

        val coveredUs = (currentTimestamp - firstTimestamp).coerceAtLeast(0L)
        val remainingUs = (duration - coveredUs).coerceAtLeast(0L)
        val remainingSeconds = (remainingUs / 1_000_000.0) / timelineRate
        if (!remainingSeconds.isFinite() || remainingSeconds < 0.0) return null
        return ceil(remainingSeconds).toLong()
    }

    private fun resetRates() {
        lastIndexedFrames = null
        lastTimestampUs = null
        lastElapsedMs = null
        firstRateElapsedMs = null
        smoothedFramesPerSecond = null
        smoothedMediaSecondsPerWallSecond = null
        positiveTimelineSamples = 0
    }

    private fun smooth(previous: Double?, sample: Double): Double =
        if (previous == null) sample else previous + EMA_ALPHA * (sample - previous)

    private companion object {
        const val EMA_ALPHA = 0.25
        const val MIN_TIMELINE_SAMPLES = 3
        const val MIN_ETA_OBSERVATION_MS = 1_000L
        const val MIN_ETA_FRACTION = 0.02
        const val MAX_ETA_FRACTION = 0.995
    }
}
