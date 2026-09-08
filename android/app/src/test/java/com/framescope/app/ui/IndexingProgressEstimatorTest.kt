package com.framescope.app.ui

import com.framescope.app.data.MicroscopeIndexingProgress
import com.framescope.app.data.MicroscopeIndexingStage
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class IndexingProgressEstimatorTest {
    @Test
    fun unknownDurationNeverInventsPercentageOrEta() {
        val estimator = IndexingProgressEstimator(durationUs = null)
        var ui: IndexingProgressUi? = null
        repeat(5) { index ->
            ui = estimator.update(progress(index + 1, (index + 1) * 500_000L, (index + 1) * 400L))
        }

        assertNotNull(ui?.framesPerSecond)
        assertNull(ui?.estimatedFraction)
        assertNull(ui?.estimatedRemainingSeconds)
    }

    @Test
    fun zeroDurationBehavesAsUnknownDuration() {
        val estimator = IndexingProgressEstimator(durationUs = 0L)
        val ui = estimator.update(progress(10, 3_000_000L, 1_000L))
        assertNull(ui.estimatedFraction)
        assertNull(ui.estimatedRemainingSeconds)
    }

    @Test
    fun vfrPercentageUsesPresentationTimeInsteadOfFrameCount() {
        val estimator = IndexingProgressEstimator(durationUs = 10_000_000L)
        val ui = estimator.update(
            progress(
                frames = 9_000,
                timestampUs = 2_500_000L,
                elapsedMs = 600L,
                firstTimestampUs = 500_000L,
            ),
        )

        assertEquals(0.20, ui.estimatedFraction ?: -1.0, 0.0001)
    }

    @Test
    fun etaWaitsForMultipleSamplesAndThenUsesObservedTimelineThroughput() {
        val estimator = IndexingProgressEstimator(durationUs = 10_000_000L)
        val samples = listOf(
            progress(100, 1_000_000L, 0L),
            progress(200, 2_000_000L, 500L),
            progress(300, 3_000_000L, 1_000L),
            progress(400, 4_000_000L, 1_500L),
        )

        assertNull(estimator.update(samples[0]).estimatedRemainingSeconds)
        assertNull(estimator.update(samples[1]).estimatedRemainingSeconds)
        assertNull(estimator.update(samples[2]).estimatedRemainingSeconds)
        val stable = estimator.update(samples[3])

        assertNotNull(stable.framesPerSecond)
        assertEquals(0.30, stable.estimatedFraction ?: -1.0, 0.0001)
        assertEquals(4L, stable.estimatedRemainingSeconds)
    }

    @Test
    fun validationAndReuseExposeCountsWithoutFakeRateOrEta() {
        val estimator = IndexingProgressEstimator(durationUs = 20_000_000L)
        val validation = estimator.update(
            progress(
                frames = 8_000,
                timestampUs = 12_000_000L,
                elapsedMs = 800L,
                stage = MicroscopeIndexingStage.ValidatingExistingIndex,
                reusedFrames = 4_000,
                expectedReuseFrames = 8_000,
            ),
        )

        assertEquals(4_000L, validation.reusedFrames)
        assertNull(validation.framesPerSecond)
        assertNull(validation.estimatedFraction)
        assertNull(validation.estimatedRemainingSeconds)
    }

    @Test
    fun cachedIndexFinalizingDoesNotInventZeroPercent() {
        val estimator = IndexingProgressEstimator(durationUs = 20_000_000L)
        estimator.update(
            progress(
                frames = 8_000,
                timestampUs = 19_800_000L,
                elapsedMs = 25L,
                stage = MicroscopeIndexingStage.ReusingExistingIndex,
                reusedFrames = 8_000,
                expectedReuseFrames = 8_000,
                firstTimestampUs = 19_800_000L,
            ),
        )
        val finalizing = estimator.update(
            progress(
                frames = 8_000,
                timestampUs = 19_800_000L,
                elapsedMs = 30L,
                stage = MicroscopeIndexingStage.Finalizing,
                reusedFrames = 8_000,
                expectedReuseFrames = 8_000,
                firstTimestampUs = 19_800_000L,
            ),
        )

        assertNull(finalizing.framesPerSecond)
        assertNull(finalizing.estimatedFraction)
        assertNull(finalizing.estimatedRemainingSeconds)
    }

    @Test
    fun rebuildResetsPriorRateAndEtaHistory() {
        val estimator = IndexingProgressEstimator(durationUs = 10_000_000L)
        listOf(
            progress(100, 1_000_000L, 0L),
            progress(200, 2_000_000L, 500L),
            progress(300, 3_000_000L, 1_000L),
            progress(400, 4_000_000L, 1_500L),
        ).forEach(estimator::update)

        estimator.update(
            progress(
                frames = 0,
                timestampUs = null,
                elapsedMs = 1_600L,
                stage = MicroscopeIndexingStage.RebuildingIndex,
            ),
        )
        val restarted = estimator.update(
            progress(
                frames = 1,
                timestampUs = 0L,
                elapsedMs = 1_700L,
                firstTimestampUs = 0L,
            ),
        )

        assertNull(restarted.framesPerSecond)
        assertNull(restarted.estimatedRemainingSeconds)
    }

    @Test
    fun frameRateIsSmoothedAndPositiveAfterFreshIndexSamples() {
        val estimator = IndexingProgressEstimator(durationUs = 30_000_000L)
        estimator.update(progress(10, 0L, 100L, firstTimestampUs = 0L))
        estimator.update(progress(110, 1_000_000L, 600L, firstTimestampUs = 0L))
        val ui = estimator.update(progress(310, 2_000_000L, 1_100L, firstTimestampUs = 0L))

        assertTrue((ui.framesPerSecond ?: 0.0) > 0.0)
    }

    private fun progress(
        frames: Int,
        timestampUs: Long?,
        elapsedMs: Long,
        firstTimestampUs: Long? = 1_000_000L,
        stage: MicroscopeIndexingStage = MicroscopeIndexingStage.Indexing,
        reusedFrames: Long = 0L,
        expectedReuseFrames: Long = 0L,
    ) = MicroscopeIndexingProgress(
        operationId = 1L,
        stage = stage,
        indexedFrames = frames.toLong(),
        reusedFrames = reusedFrames,
        expectedReuseFrames = expectedReuseFrames,
        firstTimestampUs = firstTimestampUs,
        currentTimestampUs = timestampUs,
        elapsedMs = elapsedMs,
    )
}
