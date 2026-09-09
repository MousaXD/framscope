package com.framescope.app.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ScrubUxTelemetryTest {
    @Test
    fun pointerAdmissionAndPresentedTargetAgeAreCountedWithoutAffectingBehavior() {
        ScrubUxTelemetry.resetForTest()

        ScrubUxTelemetry.recordPointerToThumb(
            pointerAtNanos = 1_000_000_000L,
            drawnAtNanos = 1_008_000_000L,
        )
        ScrubUxTelemetry.recordAdmissionDelay(
            observedAtNanos = 2_000_000_000L,
            admittedAtNanos = 2_012_000_000L,
        )
        ScrubUxTelemetry.recordRequestStarted(
            requestId = 3L,
            sessionId = 21L,
            submittedAtNanos = 3_000_000_000L,
            startedAtNanos = 3_004_000_000L,
        )
        ScrubUxTelemetry.recordRequestFinished(
            requestId = 3L,
            sessionId = 21L,
            submittedAtNanos = 3_000_000_000L,
            finishedAtNanos = 3_030_000_000L,
            publishable = true,
        )

        // Gate publication alone is not presentation. The target-age sample closes on draw.
        assertEquals(0L, ScrubUxTelemetry.snapshot().targetAgeSamples)
        ScrubUxTelemetry.recordPreviewPresented(
            sessionId = 21L,
            presentedAtNanos = 3_045_000_000L,
        )

        val snapshot = ScrubUxTelemetry.snapshot()
        assertEquals(1L, snapshot.pointerToThumbSamples)
        assertEquals(8_000L, snapshot.pointerToThumbLastUs)
        assertEquals(1L, snapshot.admissionDelaySamples)
        assertEquals(12_000L, snapshot.admissionDelayLastUs)
        assertEquals(1L, snapshot.requestStarts)
        assertEquals(1L, snapshot.requestFinishes)
        assertEquals(1L, snapshot.targetAgeSamples)
        assertEquals(45_000L, snapshot.targetAgeLastUs)
        assertEquals(0L, snapshot.staleResultDrops)
    }

    @Test
    fun newerRequestInvalidatesAnOlderNotYetDrawnPresentationSample() {
        ScrubUxTelemetry.resetForTest()
        ScrubUxTelemetry.recordRequestFinished(
            requestId = 1L,
            sessionId = 21L,
            submittedAtNanos = 4_000_000_000L,
            finishedAtNanos = 4_020_000_000L,
            publishable = true,
        )
        ScrubUxTelemetry.recordRequestStarted(
            requestId = 2L,
            sessionId = 21L,
            submittedAtNanos = 4_025_000_000L,
            startedAtNanos = 4_026_000_000L,
        )

        ScrubUxTelemetry.recordPreviewPresented(
            sessionId = 21L,
            presentedAtNanos = 4_030_000_000L,
        )

        assertEquals(0L, ScrubUxTelemetry.snapshot().targetAgeSamples)
    }

    @Test
    fun staleCompletionIsCountedButNeverCreatesAPresentedTargetSample() {
        ScrubUxTelemetry.resetForTest()

        ScrubUxTelemetry.recordRequestFinished(
            requestId = 8L,
            sessionId = 31L,
            submittedAtNanos = 5_000_000_000L,
            finishedAtNanos = 5_040_000_000L,
            publishable = false,
        )
        ScrubUxTelemetry.recordPreviewPresented(
            sessionId = 31L,
            presentedAtNanos = 5_050_000_000L,
        )

        val snapshot = ScrubUxTelemetry.snapshot()
        assertEquals(1L, snapshot.staleResultDrops)
        assertEquals(0L, snapshot.targetAgeSamples)
        assertNull(snapshot.averageTargetAgeUs)
    }

    @Test
    fun exactSettleOnlyCompletesForTheMatchingSession() {
        ScrubUxTelemetry.resetForTest()
        ScrubUxTelemetry.beginExactSettle(
            sessionId = 41L,
            startedAtNanos = 6_000_000_000L,
        )

        ScrubUxTelemetry.completeExactSettle(
            sessionId = 42L,
            completedAtNanos = 6_010_000_000L,
        )
        assertEquals(0L, ScrubUxTelemetry.snapshot().exactSettleSamples)

        ScrubUxTelemetry.completeExactSettle(
            sessionId = 41L,
            completedAtNanos = 6_025_000_000L,
        )
        val snapshot = ScrubUxTelemetry.snapshot()
        assertEquals(1L, snapshot.exactSettleSamples)
        assertEquals(25_000L, snapshot.exactSettleLastUs)
    }
}
