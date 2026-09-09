package com.framescope.app.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class LiveScrubAdmissionPolicyTest {
    @Test
    fun firstTargetIsImmediateAndSameTargetIsNotResubmitted() {
        val policy = LiveScrubAdmissionPolicy()

        assertEquals(
            LiveScrubAdmissionPlan.Immediate,
            policy.plan(
                sessionId = 7L,
                fraction = 0.10f,
                targetKey = 100L,
                observedAtNanos = 1_000_000_000L,
            ),
        )
        policy.markAdmitted(0.10f, 100L, 1_000_000_000L)

        assertEquals(
            LiveScrubAdmissionPlan.NoPreview,
            policy.plan(
                sessionId = 7L,
                fraction = 0.10f,
                targetKey = 100L,
                observedAtNanos = 1_008_000_000L,
            ),
        )
    }

    @Test
    fun rapidMotionIsCoalescedToAUsefulCadence() {
        val policy = LiveScrubAdmissionPolicy()
        val start = 2_000_000_000L
        assertEquals(
            LiveScrubAdmissionPlan.Immediate,
            policy.plan(9L, 0.10f, 10L, start),
        )
        policy.markAdmitted(0.10f, 10L, start)

        val plan = policy.plan(
            sessionId = 9L,
            fraction = 0.13f,
            targetKey = 13L,
            observedAtNanos = start + 8_000_000L,
        )

        assertTrue(plan is LiveScrubAdmissionPlan.After)
        assertTrue((plan as LiveScrubAdmissionPlan.After).delayMs in 40L..50L)
    }

    @Test
    fun slowerMotionAdmitsMoreFrequentlyThanFastMotion() {
        val fast = LiveScrubAdmissionPolicy()
        val slow = LiveScrubAdmissionPolicy()
        val start = 3_000_000_000L

        fast.plan(11L, 0.10f, 10L, start)
        fast.markAdmitted(0.10f, 10L, start)
        fast.plan(11L, 0.30f, 30L, start + 20_000_000L)

        slow.plan(12L, 0.10f, 10L, start)
        slow.markAdmitted(0.10f, 10L, start)
        slow.plan(12L, 0.105f, 11L, start + 20_000_000L)

        val fastPlan = fast.plan(11L, 0.50f, 50L, start + 30_000_000L)
        val slowPlan = slow.plan(12L, 0.106f, 12L, start + 30_000_000L)

        assertTrue(fastPlan is LiveScrubAdmissionPlan.After)
        assertEquals(LiveScrubAdmissionPlan.Immediate, slowPlan)
    }

    @Test
    fun reversalCanRefreshBeforeNormalFastCadenceWithoutBecomingUnbounded() {
        val policy = LiveScrubAdmissionPolicy()
        val start = 4_000_000_000L
        policy.plan(13L, 0.20f, 20L, start)
        policy.markAdmitted(0.20f, 20L, start)

        policy.plan(13L, 0.50f, 50L, start + 10_000_000L)
        val reversalTooSoon = policy.plan(13L, 0.40f, 40L, start + 11_000_000L)
        assertTrue(reversalTooSoon is LiveScrubAdmissionPlan.After)

        policy.plan(13L, 0.60f, 60L, start + 20_000_000L)
        val reversalAfterGap = policy.plan(13L, 0.45f, 45L, start + 25_000_000L)
        assertEquals(LiveScrubAdmissionPlan.Immediate, reversalAfterGap)
    }

    @Test
    fun newSessionStartsFresh() {
        val policy = LiveScrubAdmissionPolicy()
        val start = 5_000_000_000L
        policy.plan(21L, 0.30f, 30L, start)
        policy.markAdmitted(0.30f, 30L, start)

        assertEquals(
            LiveScrubAdmissionPlan.Immediate,
            policy.plan(22L, 0.31f, 31L, start + 1_000_000L),
        )
    }
}
