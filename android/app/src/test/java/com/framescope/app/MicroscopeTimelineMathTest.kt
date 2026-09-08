package com.framescope.app

import com.framescope.app.ui.IndexedTimelineBounds
import com.framescope.app.ui.MicroscopeTimelineMath
import com.framescope.app.ui.TimelineRangeSelection
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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
    fun indexedTimestampTimelineMapsFirstMidpointAndLastWithNonZeroOrigin() {
        val startUs = 12_500_000L
        val endUs = 27_900_000L

        assertEquals(0f, MicroscopeTimelineMath.fractionForTimestamp(startUs, startUs, endUs))
        assertEquals(0.5f, MicroscopeTimelineMath.fractionForTimestamp(20_200_000L, startUs, endUs))
        assertEquals(1f, MicroscopeTimelineMath.fractionForTimestamp(endUs, startUs, endUs))

        assertEquals(startUs, MicroscopeTimelineMath.timestampForFraction(0f, startUs, endUs))
        assertEquals(20_200_000L, MicroscopeTimelineMath.timestampForFraction(0.5f, startUs, endUs))
        assertEquals(endUs, MicroscopeTimelineMath.timestampForFraction(1f, startUs, endUs))
    }

    @Test
    fun timestampSliderClampsTransientPointerOvershootWithoutInventingFrames() {
        assertEquals(10_000L, MicroscopeTimelineMath.timestampForFraction(-1f, 10_000L, 50_000L))
        assertEquals(50_000L, MicroscopeTimelineMath.timestampForFraction(2f, 10_000L, 50_000L))
        assertEquals(0f, MicroscopeTimelineMath.fractionForTimestamp(-5L, 10_000L, 50_000L))
        assertEquals(1f, MicroscopeTimelineMath.fractionForTimestamp(99_999L, 10_000L, 50_000L))
    }

    @Test
    fun timestampMappingSupportsSignedVfrPresentationOrigins() {
        val startUs = -33_367L
        val endUs = 83_417L

        assertEquals(startUs, MicroscopeTimelineMath.timestampForFraction(0f, startUs, endUs))
        assertEquals(endUs, MicroscopeTimelineMath.timestampForFraction(1f, startUs, endUs))
        assertEquals(0f, MicroscopeTimelineMath.fractionForTimestamp(startUs, startUs, endUs))
        assertEquals(1f, MicroscopeTimelineMath.fractionForTimestamp(endUs, startUs, endUs))
        assertEquals(116_784L, MicroscopeTimelineMath.durationUs(startUs, endUs))
    }

    @Test
    fun zeroLengthIndexedTimelineIsValidAndStable() {
        assertEquals(0f, MicroscopeTimelineMath.fractionForTimestamp(42L, 42L, 42L))
        assertEquals(42L, MicroscopeTimelineMath.timestampForFraction(0.75f, 42L, 42L))
        assertEquals(0L, MicroscopeTimelineMath.durationUs(42L, 42L))
    }

    @Test
    fun invalidOrOverflowingTimestampTimelinesAreRejected() {
        assertNull(MicroscopeTimelineMath.fractionForTimestamp(0L, 10L, 9L))
        assertNull(MicroscopeTimelineMath.timestampForFraction(0.5f, 10L, 9L))
        assertNull(MicroscopeTimelineMath.timestampForFraction(Float.NaN, 0L, 10L))
        assertNull(MicroscopeTimelineMath.durationUs(10L, 9L))
        assertNull(MicroscopeTimelineMath.durationUs(Long.MIN_VALUE, Long.MAX_VALUE))
    }

    @Test
    fun committedRangeAcceptsInclusiveZeroLengthAndFullVideoBoundaries() {
        val bounds = IndexedTimelineBounds(sessionId = 9L, startUs = 1_000L, endUs = 9_000L)

        assertTrue(TimelineRangeSelection(9L, 1_000L, 9_000L).isSaneFor(bounds))
        assertTrue(TimelineRangeSelection(9L, 4_000L, 4_000L).isSaneFor(bounds))
        assertTrue(TimelineRangeSelection(9L, 1_000L, 1_000L).isSaneFor(bounds))
        assertTrue(TimelineRangeSelection(9L, 9_000L, 9_000L).isSaneFor(bounds))
    }

    @Test
    fun committedRangeRejectsCrossingOutOfBoundsAndWrongSession() {
        val bounds = IndexedTimelineBounds(sessionId = 9L, startUs = 1_000L, endUs = 9_000L)

        assertFalse(TimelineRangeSelection(9L, 8_000L, 7_000L).isSaneFor(bounds))
        assertFalse(TimelineRangeSelection(9L, 999L, 7_000L).isSaneFor(bounds))
        assertFalse(TimelineRangeSelection(9L, 7_000L, 9_001L).isSaneFor(bounds))
        assertFalse(TimelineRangeSelection(10L, 1_000L, 9_000L).isSaneFor(bounds))
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
