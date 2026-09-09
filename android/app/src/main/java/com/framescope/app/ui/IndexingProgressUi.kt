package com.framescope.app.ui

import com.framescope.app.data.MicroscopeIndexingProgress
import com.framescope.app.data.MicroscopeIndexingStage
import kotlin.math.abs
import kotlin.math.ceil
import kotlin.math.max

/** A coarse, confidence-bounded remaining-time interval for presentation. */
data class IndexingEtaRange(
    val minSeconds: Long,
    val maxSeconds: Long,
) {
    fun isSane(): Boolean = minSeconds >= 0L && maxSeconds >= minSeconds
}

/** Truthful presentation model derived only from native progress events and known media duration. */
data class IndexingProgressUi(
    val stage: MicroscopeIndexingStage,
    val indexedFrames: Long,
    val reusedFrames: Long,
    val expectedReuseFrames: Long,
    val currentTimestampUs: Long?,
    val maxPresentationTimestampUs: Long?,
    val elapsedMs: Long,
    val framesPerSecond: Double?,
    /** Exact stage fraction only when the native stage has a real denominator. */
    val stageProgressFraction: Double?,
    /** Presentation timeline coverage. This is not total indexing-work completion. */
    val mediaTimelineFraction: Double?,
    val estimatedRemainingRange: IndexingEtaRange?,
    val telemetryAvailable: Boolean,
) {
    // Compatibility accessors for callers that still consume the prior model. UI code must use the
    // semantically explicit fields above.
    val estimatedFraction: Double?
        get() = mediaTimelineFraction

    val estimatedRemainingSeconds: Long?
        get() = estimatedRemainingRange?.let { range ->
            range.minSeconds + (range.maxSeconds - range.minSeconds) / 2L
        }
}

/**
 * Estimates indexing throughput from genuine advancing native events only.
 *
 * Native sequence/sample time prevent Android polling cadence from becoming work timing. ETA uses a
 * bounded recent window, is suppressed when rate or VFR frame density is unstable, expires on a
 * genuine stall, and must rebuild confidence after telemetry loss, rebuild, or stall recovery.
 */
class IndexingProgressEstimator(
    private val durationUs: Long?,
) {
    private data class NativePoint(
        val indexedFrames: Long,
        val coverageUs: Long?,
        val sampleElapsedMs: Long,
    )

    private data class MediaRateSample(
        val mediaSecondsPerWallSecond: Double,
        val framesPerMediaSecond: Double?,
        val intervalMs: Long,
    )

    private var operationId: Long? = null
    private var activeStage: MicroscopeIndexingStage? = null
    private var lastSeenSequence = -1L
    private var firstIndexingSampleElapsedMs: Long? = null
    private var lastFrameAdvance: NativePoint? = null
    private var lastMediaAdvance: NativePoint? = null
    private val recentFrameRates = mutableListOf<Double>()
    private val recentMediaRates = mutableListOf<MediaRateSample>()

    fun update(progress: MicroscopeIndexingProgress): IndexingProgressUi {
        if (operationId != progress.operationId) {
            resetAll(progress.operationId)
        }

        val stageChanged = activeStage != progress.stage
        if (stageChanged) {
            if (
                progress.stage == MicroscopeIndexingStage.RebuildingIndex ||
                progress.stage == MicroscopeIndexingStage.Indexing ||
                activeStage == MicroscopeIndexingStage.Indexing
            ) {
                resetRateHistory()
            }
            activeStage = progress.stage
        }

        val isNewNativeEvent = progress.sequence > lastSeenSequence
        if (isNewNativeEvent) {
            lastSeenSequence = progress.sequence
        }

        if (!progress.telemetryAvailable) {
            resetRateHistory()
        } else if (progress.stage == MicroscopeIndexingStage.Indexing && isNewNativeEvent) {
            observeThroughput(progress)
        }

        if (
            progress.telemetryAvailable &&
            progress.stage == MicroscopeIndexingStage.Indexing &&
            etaHasStalled(progress)
        ) {
            // A pre-stall rate cannot become authoritative again after one new sample. Rebuild the
            // window from fresh advancing events.
            resetRateHistory()
        }

        val stageFraction = exactStageFraction(progress)
        val mediaFraction = if (progress.stage == MicroscopeIndexingStage.Indexing) {
            mediaTimelineFraction(progress)
        } else {
            null
        }
        val framesPerSecond = if (
            progress.telemetryAvailable &&
            progress.stage == MicroscopeIndexingStage.Indexing
        ) {
            median(recentFrameRates)
        } else {
            null
        }
        val etaRange = if (
            progress.telemetryAvailable &&
            progress.stage == MicroscopeIndexingStage.Indexing
        ) {
            estimatedEtaRange(progress, mediaFraction)
        } else {
            null
        }

        return IndexingProgressUi(
            stage = progress.stage,
            indexedFrames = progress.indexedFrames,
            reusedFrames = progress.reusedFrames,
            expectedReuseFrames = progress.expectedReuseFrames,
            currentTimestampUs = progress.currentTimestampUs,
            maxPresentationTimestampUs = progress.maxPresentationTimestampUs,
            elapsedMs = progress.operationElapsedMs,
            framesPerSecond = framesPerSecond,
            stageProgressFraction = stageFraction,
            mediaTimelineFraction = mediaFraction,
            estimatedRemainingRange = etaRange,
            telemetryAvailable = progress.telemetryAvailable,
        )
    }

    private fun observeThroughput(progress: MicroscopeIndexingProgress) {
        if (firstIndexingSampleElapsedMs == null) {
            firstIndexingSampleElapsedMs = progress.sampleElapsedMs
        }
        val current = NativePoint(
            indexedFrames = progress.indexedFrames,
            coverageUs = progress.maxPresentationTimestampUs,
            sampleElapsedMs = progress.sampleElapsedMs,
        )

        val previousFrame = lastFrameAdvance
        if (previousFrame == null) {
            lastFrameAdvance = current
        } else {
            val frameDelta = current.indexedFrames - previousFrame.indexedFrames
            val elapsedDeltaMs = current.sampleElapsedMs - previousFrame.sampleElapsedMs
            if (frameDelta > 0L && elapsedDeltaMs > 0L) {
                appendBounded(
                    recentFrameRates,
                    frameDelta * 1_000.0 / elapsedDeltaMs,
                )
                lastFrameAdvance = current
            }
        }

        val currentCoverage = current.coverageUs
        val previousMedia = lastMediaAdvance
        if (currentCoverage != null) {
            if (previousMedia?.coverageUs == null) {
                lastMediaAdvance = current
            } else {
                val mediaDeltaUs = currentCoverage - previousMedia.coverageUs
                val elapsedDeltaMs = current.sampleElapsedMs - previousMedia.sampleElapsedMs
                if (mediaDeltaUs > 0L && elapsedDeltaMs > 0L) {
                    val frameDelta = current.indexedFrames - previousMedia.indexedFrames
                    val mediaSeconds = mediaDeltaUs / 1_000_000.0
                    appendBounded(
                        recentMediaRates,
                        MediaRateSample(
                            mediaSecondsPerWallSecond = mediaDeltaUs / (elapsedDeltaMs * 1_000.0),
                            framesPerMediaSecond = if (frameDelta > 0L && mediaSeconds > 0.0) {
                                frameDelta / mediaSeconds
                            } else {
                                null
                            },
                            intervalMs = elapsedDeltaMs,
                        ),
                    )
                    lastMediaAdvance = current
                }
            }
        }
    }

    private fun exactStageFraction(progress: MicroscopeIndexingProgress): Double? {
        if (progress.stage != MicroscopeIndexingStage.ValidatingExistingIndex) return null
        val expected = progress.expectedReuseFrames.takeIf { it > 0L } ?: return null
        return (progress.reusedFrames.toDouble() / expected.toDouble()).coerceIn(0.0, 1.0)
    }

    private fun mediaTimelineFraction(progress: MicroscopeIndexingProgress): Double? {
        val duration = durationUs?.takeIf { it > 0L } ?: return null
        val firstTimestamp = progress.firstTimestampUs ?: return null
        val maximumTimestamp = progress.maxPresentationTimestampUs ?: return null
        val coveredUs = maximumTimestamp - firstTimestamp
        if (coveredUs < 0L) return null
        return (coveredUs.toDouble() / duration.toDouble()).coerceIn(0.0, 1.0)
    }

    private fun estimatedEtaRange(
        progress: MicroscopeIndexingProgress,
        fraction: Double?,
    ): IndexingEtaRange? {
        val duration = durationUs?.takeIf { it > 0L } ?: return null
        val firstTimestamp = progress.firstTimestampUs ?: return null
        val maximumTimestamp = progress.maxPresentationTimestampUs ?: return null
        val estimatedFraction = fraction ?: return null
        if (estimatedFraction < MIN_ETA_FRACTION || estimatedFraction >= MAX_ETA_FRACTION) return null

        val firstSampleElapsed = firstIndexingSampleElapsedMs ?: return null
        val observedForMs = progress.sampleElapsedMs - firstSampleElapsed
        if (observedForMs < MIN_ETA_OBSERVATION_MS) return null

        val stableRates = stableMediaRates() ?: return null
        val coveredUs = (maximumTimestamp - firstTimestamp).coerceAtLeast(0L)
        val remainingUs = (duration - coveredUs).coerceAtLeast(0L)
        val remainingMediaSeconds = remainingUs / 1_000_000.0
        val fastest = stableRates.maxOrNull()?.takeIf { it > 0.0 } ?: return null
        val slowest = stableRates.minOrNull()?.takeIf { it > 0.0 } ?: return null

        val optimistic = (remainingMediaSeconds / fastest) * (1.0 - ETA_RANGE_MARGIN)
        val conservative = (remainingMediaSeconds / slowest) * (1.0 + ETA_RANGE_MARGIN)
        if (!optimistic.isFinite() || !conservative.isFinite() || conservative < 0.0) return null

        val minSeconds = ceil(optimistic.coerceAtLeast(0.0)).toLong()
        val maxSeconds = ceil(conservative.coerceAtLeast(optimistic)).toLong()
        return IndexingEtaRange(minSeconds, maxSeconds)
    }

    private fun stableMediaRates(): List<Double>? {
        if (recentMediaRates.size < MIN_CONFIDENCE_SAMPLES) return null
        val rates = recentMediaRates.map { it.mediaSecondsPerWallSecond }
        val rateMedian = median(rates)?.takeIf { it > 0.0 } ?: return null
        if (maxRelativeDeviation(rates, rateMedian) > MAX_MEDIA_RATE_DEVIATION) return null

        val densities = recentMediaRates.mapNotNull { it.framesPerMediaSecond }
        if (densities.size >= MIN_DENSITY_SAMPLES) {
            val densityMedian = median(densities)?.takeIf { it > 0.0 } ?: return null
            if (maxRelativeDeviation(densities, densityMedian) > MAX_FRAME_DENSITY_DEVIATION) {
                return null
            }
        }
        return rates
    }

    private fun etaHasStalled(progress: MicroscopeIndexingProgress): Boolean {
        if (recentMediaRates.size < MIN_CONFIDENCE_SAMPLES) return false
        val sinceWorkAdvanceMs = progress.operationElapsedMs - progress.lastWorkAdvanceElapsedMs
        if (sinceWorkAdvanceMs <= 0L) return false
        return sinceWorkAdvanceMs > stallThresholdMs()
    }

    private fun stallThresholdMs(): Long {
        val intervals = recentMediaRates.map { it.intervalMs.toDouble() }
        val medianInterval = median(intervals) ?: return MIN_STALL_TIMEOUT_MS
        return max(MIN_STALL_TIMEOUT_MS.toDouble(), medianInterval * STALL_INTERVAL_MULTIPLIER).toLong()
    }

    private fun resetAll(nextOperationId: Long) {
        operationId = nextOperationId
        activeStage = null
        lastSeenSequence = -1L
        resetRateHistory()
    }

    private fun resetRateHistory() {
        firstIndexingSampleElapsedMs = null
        lastFrameAdvance = null
        lastMediaAdvance = null
        recentFrameRates.clear()
        recentMediaRates.clear()
    }

    private fun <T> appendBounded(list: MutableList<T>, value: T) {
        list += value
        while (list.size > RATE_WINDOW_SIZE) {
            list.removeAt(0)
        }
    }

    private fun maxRelativeDeviation(values: List<Double>, center: Double): Double {
        if (center <= 0.0 || values.isEmpty()) return Double.POSITIVE_INFINITY
        return values.maxOf { value -> abs(value - center) / center }
    }

    private fun median(values: List<Double>): Double? {
        if (values.isEmpty()) return null
        val sorted = values.sorted()
        val middle = sorted.size / 2
        return if (sorted.size % 2 == 0) {
            (sorted[middle - 1] + sorted[middle]) / 2.0
        } else {
            sorted[middle]
        }
    }

    private companion object {
        const val RATE_WINDOW_SIZE = 6
        const val MIN_CONFIDENCE_SAMPLES = 4
        const val MIN_DENSITY_SAMPLES = 3
        const val MIN_ETA_OBSERVATION_MS = 1_000L
        const val MIN_ETA_FRACTION = 0.02
        const val MAX_ETA_FRACTION = 0.995
        const val MAX_MEDIA_RATE_DEVIATION = 0.35
        const val MAX_FRAME_DENSITY_DEVIATION = 0.60
        const val MIN_STALL_TIMEOUT_MS = 2_500L
        const val STALL_INTERVAL_MULTIPLIER = 3.0
        const val ETA_RANGE_MARGIN = 0.10
    }
}
