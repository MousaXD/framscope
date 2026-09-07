package com.framescope.app

import com.framescope.app.ui.MicroscopeTimelineMath
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MicroscopeTimelineMathTest {
    @Test
    fun presentationFractionMapsAuthoritativeFrameOrder() {
        assertEquals(0f, MicroscopeTimelineMath.fractionForFrame(0L, 101L))
        assertEquals(0.5f, MicroscopeTimelineMath.fractionForFrame(50L, 101L))
        assertEquals(1f, MicroscopeTimelineMath.fractionForFrame(100L, 101L))
    }

    @Test
    fun fractionMappingClampsWithoutFpsOrTimestampMath() {
        assertEquals(0L, MicroscopeTimelineMath.frameForFraction(-1f, 101L))
        assertEquals(50L, MicroscopeTimelineMath.frameForFraction(0.5f, 101L))
        assertEquals(100L, MicroscopeTimelineMath.frameForFraction(2f, 101L))
    }

    @Test
    fun veryLargeIndexesRemainFrameIdentityBased() {
        val frameCount = 5_000_000_001L

        assertEquals(2_500_000_000L, MicroscopeTimelineMath.frameForFraction(0.5f, frameCount))
        assertEquals(0.5f, MicroscopeTimelineMath.fractionForFrame(2_500_000_000L, frameCount))
    }

    @Test
    fun invalidTimelineInputsAreRejectedOrSafelyCollapsed() {
        assertNull(MicroscopeTimelineMath.frameForFraction(0.5f, 0L))
        assertNull(MicroscopeTimelineMath.frameForFraction(Float.NaN, 10L))
        assertEquals(0L, MicroscopeTimelineMath.frameForFraction(1f, 1L))
        assertEquals(0f, MicroscopeTimelineMath.fractionForFrame(99L, 1L))
    }

    @Test
    fun boundedRapidStepsClampAtTimelineEdges() {
        assertEquals(0L, MicroscopeTimelineMath.boundedStepTarget(5L, 1_000L, -100L))
        assertEquals(15L, MicroscopeTimelineMath.boundedStepTarget(5L, 1_000L, 10L))
        assertEquals(999L, MicroscopeTimelineMath.boundedStepTarget(995L, 1_000L, 100L))
        assertEquals(985L, MicroscopeTimelineMath.boundedStepTarget(995L, 1_000L, -10L))
    }

    @Test
    fun extremeRapidStepDeltasNeverOverflow() {
        assertEquals(999L, MicroscopeTimelineMath.boundedStepTarget(500L, 1_000L, Long.MAX_VALUE))
        assertEquals(0L, MicroscopeTimelineMath.boundedStepTarget(500L, 1_000L, Long.MIN_VALUE))
    }

    @Test
    fun invalidRapidStepStateIsRejected() {
        assertNull(MicroscopeTimelineMath.boundedStepTarget(0L, 0L, 10L))
        assertNull(MicroscopeTimelineMath.boundedStepTarget(-1L, 10L, 10L))
        assertNull(MicroscopeTimelineMath.boundedStepTarget(10L, 10L, -1L))
    }

    @Test
    fun framePositionLabelsUseOneBasedPresentationOrder() {
        assertEquals("Frame 1 of 3", MicroscopeTimelineMath.framePositionLabel(0L, 3L))
        assertEquals("Frame 3 of 3", MicroscopeTimelineMath.framePositionLabel(2L, 3L))
        assertNull(MicroscopeTimelineMath.framePositionLabel(3L, 3L))
        assertNull(MicroscopeTimelineMath.framePositionLabel(0L, 0L))
    }

    @Test
    fun fractionOutputAlwaysStaysInsideSliderRange() {
        assertTrue(MicroscopeTimelineMath.fractionForFrame(-100L, 100L) in 0f..1f)
        assertTrue(MicroscopeTimelineMath.fractionForFrame(Long.MAX_VALUE, 100L) in 0f..1f)
    }
}
