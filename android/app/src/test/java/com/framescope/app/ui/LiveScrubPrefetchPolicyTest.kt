package com.framescope.app.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class LiveScrubPrefetchPolicyTest {
    @Test
    fun stableForwardMotionWarmsAheadAndFasterCadenceLooksFarther() {
        val policy = LiveScrubPrefetchPolicy()

        assertNull(policy.candidate(7L, 10L, 100L, 1_000L, accelerationEnabled = true))
        assertEquals(15L, policy.candidate(7L, 12L, 100L, 1_080L, accelerationEnabled = true))
        assertEquals(17L, policy.candidate(7L, 14L, 100L, 1_140L, accelerationEnabled = true))
    }

    @Test
    fun stableReverseMotionWarmsBehindAfterDirectionIsEstablished() {
        val policy = LiveScrubPrefetchPolicy()

        assertNull(policy.candidate(9L, 30L, 100L, 1_000L, accelerationEnabled = true))
        assertEquals(27L, policy.candidate(9L, 29L, 100L, 1_100L, accelerationEnabled = true))
        assertEquals(26L, policy.candidate(9L, 28L, 100L, 1_200L, accelerationEnabled = true))
    }

    @Test
    fun reversalSuppressesSpeculationForThatSample() {
        val policy = LiveScrubPrefetchPolicy()

        assertNull(policy.candidate(3L, 10L, 100L, 1_000L, accelerationEnabled = true))
        assertEquals(13L, policy.candidate(3L, 11L, 100L, 1_100L, accelerationEnabled = true))
        assertNull(policy.candidate(3L, 9L, 100L, 1_180L, accelerationEnabled = true))
        assertEquals(5L, policy.candidate(3L, 8L, 100L, 1_260L, accelerationEnabled = true))
    }

    @Test
    fun longPauseDisabledAccelerationAndBoundsSuppressPrefetch() {
        val policy = LiveScrubPrefetchPolicy()

        assertNull(policy.candidate(4L, 5L, 8L, 1_000L, accelerationEnabled = true))
        assertNull(policy.candidate(4L, 6L, 8L, 1_500L, accelerationEnabled = true))
        assertNull(policy.candidate(4L, 7L, 8L, 1_580L, accelerationEnabled = false))
        assertNull(policy.candidate(4L, 7L, 8L, 1_660L, accelerationEnabled = true))
    }

    @Test
    fun sessionReplacementResetsHistory() {
        val policy = LiveScrubPrefetchPolicy()

        assertNull(policy.candidate(1L, 10L, 100L, 1_000L, accelerationEnabled = true))
        assertEquals(13L, policy.candidate(1L, 11L, 100L, 1_100L, accelerationEnabled = true))
        assertNull(policy.candidate(2L, 11L, 100L, 1_150L, accelerationEnabled = true))
    }
}
