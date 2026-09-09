package com.framescope.app.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class MicroscopeIndexingProgressTest {
    @Test
    fun parsesOperationScopedNativeEventTimingAndCoverage() {
        val progress = RustMicroscopeIndexingProgressSource.parseResponse(
            raw = rawProgress(),
            expectedOperationId = 17L,
        )

        requireNotNull(progress)
        assertEquals(MicroscopeIndexingStage.Indexing, progress.stage)
        assertEquals(420L, progress.indexedFrames)
        assertEquals(64L, progress.reusedFrames)
        assertEquals(100_000L, progress.firstTimestampUs)
        assertEquals(9_600_000L, progress.currentTimestampUs)
        assertEquals(9_700_000L, progress.maxPresentationTimestampUs)
        assertEquals(12L, progress.sequence)
        assertEquals(1_400L, progress.sampleElapsedMs)
        assertEquals(1_450L, progress.operationElapsedMs)
        assertEquals(1_400L, progress.lastWorkAdvanceElapsedMs)
        assertTrue(progress.telemetryAvailable)
    }

    @Test
    fun exactCurrentTimestampMayRegressBelowPriorCoverageAndRemainSane() {
        val progress = RustMicroscopeIndexingProgressSource.parseResponse(
            raw = rawProgress(
                firstTimestampUs = 4_000_000L,
                currentTimestampUs = 4_100_000L,
                maxTimestampUs = 4_200_000L,
            ),
            expectedOperationId = 17L,
        )

        requireNotNull(progress)
        assertTrue(progress.isSane())
        assertEquals(4_100_000L, progress.currentTimestampUs)
        assertEquals(4_200_000L, progress.maxPresentationTimestampUs)
    }

    @Test
    fun idleMeansOperationHasNoPublishedProgress() {
        assertNull(
            RustMicroscopeIndexingProgressSource.parseResponse(
                raw = """{"status":"idle"}""",
                expectedOperationId = 99L,
            ),
        )
    }

    @Test
    fun rejectsProgressFromDifferentOperation() {
        assertThrows(IllegalArgumentException::class.java) {
            RustMicroscopeIndexingProgressSource.parseResponse(
                raw = rawProgress(operationId = 18L),
                expectedOperationId = 17L,
            )
        }
    }

    @Test
    fun rejectsUnknownStageRatherThanGuessing() {
        assertThrows(IllegalArgumentException::class.java) {
            RustMicroscopeIndexingProgressSource.parseResponse(
                raw = rawProgress(stage = "almost_ready"),
                expectedOperationId = 17L,
            )
        }
    }

    @Test
    fun rejectsSampleTimeThatMovesBeyondOperationAge() {
        assertThrows(IllegalArgumentException::class.java) {
            RustMicroscopeIndexingProgressSource.parseResponse(
                raw = rawProgress(sampleElapsedMs = 1_500L, operationElapsedMs = 1_450L),
                expectedOperationId = 17L,
            )
        }
    }

    @Test
    fun telemetryFailureReturnsOnlySameOperationsLastSampleAsUnavailable() {
        val freshness = MicroscopeIndexingProgressFreshness()
        val current = progress(operationId = 17L)

        freshness.beginOperation(17L)
        val accepted = freshness.onSuccess(17L, current)
        requireNotNull(accepted)
        assertTrue(accepted.telemetryAvailable)

        val unavailable = freshness.onFailure(17L)
        requireNotNull(unavailable)
        assertFalse(unavailable.telemetryAvailable)
        assertEquals(current.sequence, unavailable.sequence)
        assertEquals(current.sampleElapsedMs, unavailable.sampleElapsedMs)

        freshness.beginOperation(18L)
        assertNull(freshness.onFailure(18L))
    }

    private fun rawProgress(
        operationId: Long = 17L,
        stage: String = "indexing",
        firstTimestampUs: Long = 100_000L,
        currentTimestampUs: Long = 9_600_000L,
        maxTimestampUs: Long = 9_700_000L,
        sampleElapsedMs: Long = 1_400L,
        operationElapsedMs: Long = 1_450L,
    ): String =
        """{"status":"ok","progress":{"operation_id":$operationId,"sequence":12,"stage":"$stage","indexed_frames":420,"reused_frames":64,"expected_reuse_frames":64,"first_timestamp_us":$firstTimestampUs,"current_timestamp_us":$currentTimestampUs,"max_presentation_timestamp_us":$maxTimestampUs,"sample_elapsed_ms":$sampleElapsedMs,"last_work_advance_elapsed_ms":$sampleElapsedMs,"operation_elapsed_ms":$operationElapsedMs,"elapsed_ms":$operationElapsedMs}}"""

    private fun progress(operationId: Long) = MicroscopeIndexingProgress(
        operationId = operationId,
        stage = MicroscopeIndexingStage.Indexing,
        indexedFrames = 420L,
        reusedFrames = 64L,
        expectedReuseFrames = 64L,
        firstTimestampUs = 100_000L,
        currentTimestampUs = 9_600_000L,
        elapsedMs = 1_450L,
        sequence = 12L,
        sampleElapsedMs = 1_400L,
        operationElapsedMs = 1_450L,
        lastWorkAdvanceElapsedMs = 1_400L,
        maxPresentationTimestampUs = 9_700_000L,
    )
}
