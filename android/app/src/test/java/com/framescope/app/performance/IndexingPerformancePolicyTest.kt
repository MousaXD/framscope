package com.framescope.app.performance

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class IndexingPerformancePolicyTest {
    @Test
    fun repeatedNativePollDoesNotCreateSyntheticWorkCycle() {
        val tracker = IndexingWorkCycleTracker()
        val first = IndexingWorkSample(sequence = 10L, sampleElapsedMs = 1_000L, workUnits = 64L)
        val duplicate = first.copy()
        val advanced = IndexingWorkSample(sequence = 11L, sampleElapsedMs = 1_180L, workUnits = 128L)

        assertNull(tracker.observe(first))
        assertNull(tracker.observe(duplicate))
        assertEquals(180_000_000L, tracker.observe(advanced))
    }

    @Test
    fun nonAdvancingNativeEventDoesNotResetWorkTimingBaseline() {
        val tracker = IndexingWorkCycleTracker()

        assertNull(
            tracker.observe(
                IndexingWorkSample(sequence = 1L, sampleElapsedMs = 100L, workUnits = 64L),
            ),
        )
        assertNull(
            tracker.observe(
                IndexingWorkSample(sequence = 2L, sampleElapsedMs = 140L, workUnits = 64L),
            ),
        )
        assertEquals(
            100_000_000L,
            tracker.observe(
                IndexingWorkSample(sequence = 3L, sampleElapsedMs = 200L, workUnits = 128L),
            ),
        )
    }

    @Test
    fun staleSequenceIsIgnoredWithoutMovingBaseline() {
        val tracker = IndexingWorkCycleTracker()

        assertNull(
            tracker.observe(
                IndexingWorkSample(sequence = 5L, sampleElapsedMs = 300L, workUnits = 64L),
            ),
        )
        assertNull(
            tracker.observe(
                IndexingWorkSample(sequence = 4L, sampleElapsedMs = 350L, workUnits = 128L),
            ),
        )
        assertEquals(
            120_000_000L,
            tracker.observe(
                IndexingWorkSample(sequence = 6L, sampleElapsedMs = 420L, workUnits = 128L),
            ),
        )
    }

    @Test
    fun adpfTargetIsObservedWorkClampedOnlyForApiSafety() {
        assertEquals(1_000_000L, calibratedTargetDurationNanos(1L))
        assertEquals(250_000_000L, calibratedTargetDurationNanos(250_000_000L))
        assertEquals(5_000_000_000L, calibratedTargetDurationNanos(Long.MAX_VALUE))
    }
}
