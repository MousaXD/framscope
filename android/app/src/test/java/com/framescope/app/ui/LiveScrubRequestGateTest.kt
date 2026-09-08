package com.framescope.app.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LiveScrubRequestGateTest {
    @Test
    fun rapidSubmissionsKeepOneInFlightAndOnlyNewestPending() {
        val gate = LiveScrubRequestGate()
        val first = gate.submit(
            sessionId = 41L,
            target = LiveScrubTarget.Timestamp(100_000L),
        )
        assertEquals(first, gate.beginNext())
        assertEquals(1, gate.inFlightCount())
        assertEquals(0, gate.pendingCount())

        gate.submit(
            sessionId = 41L,
            target = LiveScrubTarget.Timestamp(200_000L),
        )
        val newest = gate.submit(
            sessionId = 41L,
            target = LiveScrubTarget.Timestamp(300_000L),
        )

        assertEquals(1, gate.inFlightCount())
        assertEquals(1, gate.pendingCount())
        assertTrue(gate.hasPendingWork())
        assertNull(gate.beginNext())

        assertFalse(gate.finish(first))
        assertEquals(0, gate.inFlightCount())

        val replacement = gate.beginNext()
        assertEquals(newest, replacement)
        assertEquals(LiveScrubTarget.Timestamp(300_000L), replacement?.target)
        assertEquals(1, gate.inFlightCount())
        assertEquals(0, gate.pendingCount())
        assertTrue(gate.finish(newest))
    }

    @Test
    fun invalidateDropsPendingAndMakesLateInFlightResultUnpublishable() {
        val gate = LiveScrubRequestGate()
        val inFlight = gate.submit(
            sessionId = 7L,
            target = LiveScrubTarget.Frame(10L),
        )
        assertEquals(inFlight, gate.beginNext())
        gate.submit(
            sessionId = 7L,
            target = LiveScrubTarget.Frame(11L),
        )

        gate.invalidate()

        assertEquals(1, gate.inFlightCount())
        assertEquals(0, gate.pendingCount())
        assertFalse(gate.hasPendingWork())
        assertFalse(gate.finish(inFlight))
        assertNull(gate.beginNext())
    }

    @Test
    fun newEpochAfterInvalidationCanPublishNormally() {
        val gate = LiveScrubRequestGate()
        val stale = gate.submit(
            sessionId = 5L,
            target = LiveScrubTarget.Timestamp(-250_000L),
        )
        assertEquals(stale, gate.beginNext())
        gate.invalidate()
        assertFalse(gate.finish(stale))

        val fresh = gate.submit(
            sessionId = 6L,
            target = LiveScrubTarget.Frame(3L),
        )
        assertEquals(fresh, gate.beginNext())
        assertEquals(6L, fresh.sessionId)
        assertEquals(LiveScrubTarget.Frame(3L), fresh.target)
        assertTrue(gate.finish(fresh))
    }
}
