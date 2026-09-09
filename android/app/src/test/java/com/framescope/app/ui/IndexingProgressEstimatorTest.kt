package com.framescope.app.ui

import com.framescope.app.data.MicroscopeIndexingProgress
import com.framescope.app.data.MicroscopeIndexingStage
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class IndexingProgressEstimatorTest {
    @Test
    fun repeatedUnchangedPollsCannotInflateRate() {
        val estimator = IndexingProgressEstimator(durationUs = 10_000_000L)
        val first = progress(
            sequence = 1L,
            frames = 64L,
            currentTimestampUs = 1_000_000L,
            maxTimestampUs = 1_000_000L,
            sampleElapsedMs = 1_000L,
        )
        estimator.update(first)

        for (operationAge in listOf(1_125L, 1_250L, 1_500L, 1_875L)) {
            estimator.update(
                first.copy(
                    operationElapsedMs = operationAge,
                    elapsedMs = operationAge,
                ),
            )
        }

        val advanced = estimator.update(
            progress(
                sequence = 2L,
                frames = 128L,
                currentTimestampUs = 2_000_000L,
                maxTimestampUs = 2_000_000L,
                sampleElapsedMs = 2_000L,
            ),
        )

        assertEquals(64.0, advanced.framesPerSecond ?: -1.0, 0.0001)
        assertEquals(0.20, advanced.mediaTimelineFraction ?: -1.0, 0.0001)
    }

    @Test
    fun unknownDurationNeverInventsTimelinePercentageOrEta() {
        val estimator = IndexingProgressEstimator(durationUs = null)
        estimator.update(progress(1L, 100L, 1_000_000L, 1_000_000L, 0L))
        val ui = estimator.update(progress(2L, 200L, 2_000_000L, 2_000_000L, 500L))

        assertNotNull(ui.framesPerSecond)
        assertNull(ui.mediaTimelineFraction)
        assertNull(ui.estimatedRemainingRange)
    }

    @Test
    fun zeroDurationBehavesAsUnknownDuration() {
        val estimator = IndexingProgressEstimator(durationUs = 0L)
        val ui = estimator.update(progress(1L, 10L, 3_000_000L, 3_000_000L, 1_000L))
        assertNull(ui.mediaTimelineFraction)
        assertNull(ui.estimatedRemainingRange)
    }

    @Test
    fun vfrCoverageUsesPresentationTimeInsteadOfFrameCount() {
        val estimator = IndexingProgressEstimator(durationUs = 10_000_000L)
        val ui = estimator.update(
            progress(
                sequence = 1L,
                frames = 9_000L,
                currentTimestampUs = 2_500_000L,
                maxTimestampUs = 2_500_000L,
                sampleElapsedMs = 600L,
                firstTimestampUs = 500_000L,
            ),
        )

        assertEquals(0.20, ui.mediaTimelineFraction ?: -1.0, 0.0001)
    }

    @Test
    fun nonZeroAndNegativeTimelineOriginsRemainPresentationCorrect() {
        val positive = IndexingProgressEstimator(durationUs = 10_000_000L).update(
            progress(
                sequence = 1L,
                frames = 100L,
                currentTimestampUs = 2_500_000L,
                maxTimestampUs = 2_500_000L,
                sampleElapsedMs = 500L,
                firstTimestampUs = 500_000L,
            ),
        )
        val negative = IndexingProgressEstimator(durationUs = 10_000_000L).update(
            progress(
                sequence = 1L,
                frames = 100L,
                currentTimestampUs = 1_500_000L,
                maxTimestampUs = 1_500_000L,
                sampleElapsedMs = 500L,
                firstTimestampUs = -500_000L,
            ),
        )

        assertEquals(0.20, positive.mediaTimelineFraction ?: -1.0, 0.0001)
        assertEquals(0.20, negative.mediaTimelineFraction ?: -1.0, 0.0001)
    }

    @Test
    fun stableNativeSamplesProduceConfidenceBoundedEta() {
        val estimator = IndexingProgressEstimator(durationUs = 10_000_000L)
        val samples = stableSamples()
        samples.dropLast(1).forEach { sample ->
            assertNull(estimator.update(sample).estimatedRemainingRange)
        }
        val stable = estimator.update(samples.last())

        assertEquals(0.50, stable.mediaTimelineFraction ?: -1.0, 0.0001)
        assertEquals(IndexingEtaRange(3L, 3L), stable.estimatedRemainingRange)
        assertNotNull(stable.framesPerSecond)
    }

    @Test
    fun stallInvalidatesEtaAndObservedRate() {
        val estimator = IndexingProgressEstimator(durationUs = 10_000_000L)
        val samples = stableSamples()
        var lastUi: IndexingProgressUi? = null
        samples.forEach { lastUi = estimator.update(it) }
        assertNotNull(lastUi?.estimatedRemainingRange)

        val last = samples.last()
        val stalled = estimator.update(
            last.copy(
                operationElapsedMs = last.lastWorkAdvanceElapsedMs + 3_001L,
                elapsedMs = last.lastWorkAdvanceElapsedMs + 3_001L,
            ),
        )

        assertNull(stalled.estimatedRemainingRange)
        assertNull(stalled.framesPerSecond)
    }

    @Test
    fun etaRequiresFreshStableSamplesAfterStall() {
        val estimator = IndexingProgressEstimator(durationUs = 12_000_000L)
        val samples = stableSamples()
        samples.forEach(estimator::update)
        val last = samples.last()
        estimator.update(
            last.copy(
                operationElapsedMs = last.lastWorkAdvanceElapsedMs + 3_001L,
                elapsedMs = last.lastWorkAdvanceElapsedMs + 3_001L,
            ),
        )

        val recovery = listOf(
            progress(6L, 600L, 6_000_000L, 6_000_000L, 5_500L),
            progress(7L, 700L, 6_500_000L, 6_500_000L, 6_000L),
            progress(8L, 800L, 7_000_000L, 7_000_000L, 6_500L),
            progress(9L, 900L, 7_500_000L, 7_500_000L, 7_000L),
            progress(10L, 1_000L, 8_000_000L, 8_000_000L, 7_500L),
        )
        recovery.dropLast(1).forEach { sample ->
            assertNull(estimator.update(sample).estimatedRemainingRange)
        }
        assertNotNull(estimator.update(recovery.last()).estimatedRemainingRange)
    }

    @Test
    fun suddenSlowdownReducesEtaConfidence() {
        val estimator = IndexingProgressEstimator(durationUs = 12_000_000L)
        stableSamples().forEach(estimator::update)

        val slowed = estimator.update(
            progress(6L, 600L, 5_500_000L, 5_500_000L, 2_500L),
        )
        assertNull(slowed.estimatedRemainingRange)
    }

    @Test
    fun suddenSpeedupReducesEtaConfidence() {
        val estimator = IndexingProgressEstimator(durationUs = 12_000_000L)
        stableSamples().forEach(estimator::update)

        val spedUp = estimator.update(
            progress(6L, 600L, 7_000_000L, 7_000_000L, 2_500L),
        )
        assertNull(spedUp.estimatedRemainingRange)
    }

    @Test
    fun timestampGapOutlierCannotCreateConfidentEta() {
        val estimator = IndexingProgressEstimator(durationUs = 14_000_000L)
        stableSamples().forEach(estimator::update)

        val gap = estimator.update(
            progress(6L, 600L, 9_000_000L, 9_000_000L, 2_500L),
        )
        assertNull(gap.estimatedRemainingRange)
    }

    @Test
    fun vfrSparseToDenseDensityChangeReducesEtaConfidence() {
        val estimator = IndexingProgressEstimator(durationUs = 12_000_000L)
        densitySamples(framesPerMediaSecond = 10L).forEach(estimator::update)
        val changed = estimator.update(
            progress(6L, 150L, 6_000_000L, 6_000_000L, 2_500L),
        )

        assertNull(changed.estimatedRemainingRange)
    }

    @Test
    fun vfrDenseToSparseDensityChangeReducesEtaConfidence() {
        val estimator = IndexingProgressEstimator(durationUs = 12_000_000L)
        densitySamples(framesPerMediaSecond = 100L).forEach(estimator::update)
        val changed = estimator.update(
            progress(6L, 510L, 6_000_000L, 6_000_000L, 2_500L),
        )

        assertNull(changed.estimatedRemainingRange)
    }

    @Test
    fun exactValidationStageFractionUsesRealDenominator() {
        val estimator = IndexingProgressEstimator(durationUs = 20_000_000L)
        val validation = estimator.update(
            progress(
                sequence = 1L,
                frames = 8_000L,
                currentTimestampUs = 12_000_000L,
                maxTimestampUs = 12_000_000L,
                sampleElapsedMs = 800L,
                stage = MicroscopeIndexingStage.ValidatingExistingIndex,
                reusedFrames = 4_000L,
                expectedReuseFrames = 8_000L,
            ),
        )

        assertEquals(0.50, validation.stageProgressFraction ?: -1.0, 0.0001)
        assertNull(validation.mediaTimelineFraction)
        assertNull(validation.framesPerSecond)
        assertNull(validation.estimatedRemainingRange)
    }

    @Test
    fun rebuildResetsPreviousEstimatorHistory() {
        val estimator = IndexingProgressEstimator(durationUs = 10_000_000L)
        stableSamples().forEach(estimator::update)

        estimator.update(
            progress(
                sequence = 6L,
                frames = 0L,
                currentTimestampUs = null,
                maxTimestampUs = null,
                sampleElapsedMs = 2_100L,
                stage = MicroscopeIndexingStage.RebuildingIndex,
            ),
        )
        val restarted = estimator.update(
            progress(7L, 1L, 0L, 0L, 2_200L),
        )

        assertNull(restarted.framesPerSecond)
        assertNull(restarted.estimatedRemainingRange)
    }

    @Test
    fun finalizingNeverCarriesTimelinePercentageRateOrEta() {
        val estimator = IndexingProgressEstimator(durationUs = 10_000_000L)
        stableSamples().forEach(estimator::update)

        val finalizing = estimator.update(
            progress(
                sequence = 6L,
                frames = 600L,
                currentTimestampUs = 9_990_000L,
                maxTimestampUs = 9_990_000L,
                sampleElapsedMs = 2_500L,
                stage = MicroscopeIndexingStage.Finalizing,
            ),
        )

        assertNull(finalizing.stageProgressFraction)
        assertNull(finalizing.mediaTimelineFraction)
        assertNull(finalizing.framesPerSecond)
        assertNull(finalizing.estimatedRemainingRange)
    }

    @Test
    fun ninetyNinePointSixTimelineCoverageDoesNotRoundToSemanticCompletion() {
        val estimator = IndexingProgressEstimator(durationUs = 10_000_000L)
        val ui = estimator.update(
            progress(1L, 100L, 9_960_000L, 9_960_000L, 500L),
        )

        assertEquals(0.996, ui.mediaTimelineFraction ?: -1.0, 0.0001)
        assertEquals(99, displayProgressPercent(ui.mediaTimelineFraction ?: 0.0))
    }

    @Test
    fun telemetryFailureClearsFreshnessDependentRateAndEta() {
        val estimator = IndexingProgressEstimator(durationUs = 10_000_000L)
        val samples = stableSamples()
        var ready: IndexingProgressUi? = null
        samples.forEach { ready = estimator.update(it) }
        assertNotNull(ready?.estimatedRemainingRange)

        val unavailable = estimator.update(samples.last().copy(telemetryAvailable = false))
        assertFalse(unavailable.telemetryAvailable)
        assertNull(unavailable.framesPerSecond)
        assertNull(unavailable.estimatedRemainingRange)

        val recovering = estimator.update(
            progress(6L, 600L, 6_000_000L, 6_000_000L, 2_500L),
        )
        assertTrue(recovering.telemetryAvailable)
        assertNull(recovering.framesPerSecond)
        assertNull(recovering.estimatedRemainingRange)
    }

    @Test
    fun exactCurrentTimestampMayRegressWithoutMovingCoverageBackward() {
        val estimator = IndexingProgressEstimator(durationUs = 10_000_000L)
        val first = estimator.update(
            progress(1L, 100L, 4_200_000L, 4_200_000L, 500L),
        )
        val reordered = estimator.update(
            progress(
                sequence = 2L,
                frames = 200L,
                currentTimestampUs = 4_100_000L,
                maxTimestampUs = 4_200_000L,
                sampleElapsedMs = 1_000L,
            ),
        )

        assertEquals(4_100_000L, reordered.currentTimestampUs)
        assertEquals(4_200_000L, reordered.maxPresentationTimestampUs)
        assertEquals(first.mediaTimelineFraction, reordered.mediaTimelineFraction)
    }

    private fun stableSamples(): List<MicroscopeIndexingProgress> = listOf(
        progress(1L, 100L, 1_000_000L, 1_000_000L, 0L),
        progress(2L, 200L, 2_000_000L, 2_000_000L, 500L),
        progress(3L, 300L, 3_000_000L, 3_000_000L, 1_000L),
        progress(4L, 400L, 4_000_000L, 4_000_000L, 1_500L),
        progress(5L, 500L, 5_000_000L, 5_000_000L, 2_000L),
    )

    private fun densitySamples(framesPerMediaSecond: Long): List<MicroscopeIndexingProgress> =
        (1L..5L).map { sequence ->
            progress(
                sequence = sequence,
                frames = sequence * framesPerMediaSecond,
                currentTimestampUs = sequence * 1_000_000L,
                maxTimestampUs = sequence * 1_000_000L,
                sampleElapsedMs = (sequence - 1L) * 500L,
            )
        }

    private fun progress(
        sequence: Long,
        frames: Long,
        currentTimestampUs: Long?,
        maxTimestampUs: Long?,
        sampleElapsedMs: Long,
        firstTimestampUs: Long? = 0L,
        stage: MicroscopeIndexingStage = MicroscopeIndexingStage.Indexing,
        reusedFrames: Long = 0L,
        expectedReuseFrames: Long = 0L,
        operationElapsedMs: Long = sampleElapsedMs,
        lastWorkAdvanceElapsedMs: Long = sampleElapsedMs,
        telemetryAvailable: Boolean = true,
    ) = MicroscopeIndexingProgress(
        operationId = 1L,
        stage = stage,
        indexedFrames = frames,
        reusedFrames = reusedFrames,
        expectedReuseFrames = expectedReuseFrames,
        firstTimestampUs = firstTimestampUs,
        currentTimestampUs = currentTimestampUs,
        elapsedMs = operationElapsedMs,
        sequence = sequence,
        sampleElapsedMs = sampleElapsedMs,
        operationElapsedMs = operationElapsedMs,
        lastWorkAdvanceElapsedMs = lastWorkAdvanceElapsedMs,
        maxPresentationTimestampUs = maxTimestampUs,
        telemetryAvailable = telemetryAvailable,
    )
}
