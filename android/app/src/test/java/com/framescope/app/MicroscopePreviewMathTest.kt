package com.framescope.app

import com.framescope.app.ui.MICROSCOPE_PREVIEW_PIXEL_BUDGET
import com.framescope.app.ui.MicroscopePreviewMath
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MicroscopePreviewMathTest {
    @Test
    fun fourKLandscapeIsBoundedTo1080pEquivalentPixels() {
        val plan = assertNotNull(MicroscopePreviewMath.plan(3_840, 2_160))

        assertEquals(1_920, plan.targetWidth)
        assertEquals(1_080, plan.targetHeight)
        assertTrue(plan.isDownscaled)
        assertTrue(plan.targetWidth.toLong() * plan.targetHeight <= MICROSCOPE_PREVIEW_PIXEL_BUDGET)
    }

    @Test
    fun portraitPreviewPreservesEquivalentPixelBudget() {
        val plan = assertNotNull(MicroscopePreviewMath.plan(2_160, 3_840))

        assertEquals(1_080, plan.targetWidth)
        assertEquals(1_920, plan.targetHeight)
        assertTrue(plan.isDownscaled)
    }

    @Test
    fun smallFrameKeepsSourceDimensions() {
        val plan = assertNotNull(MicroscopePreviewMath.plan(1_280, 720))

        assertEquals(1_280, plan.targetWidth)
        assertEquals(720, plan.targetHeight)
        assertFalse(plan.isDownscaled)
    }

    @Test
    fun centeredSamplingCoversBothEndsWithoutReadingPastSource() {
        val plan = assertNotNull(MicroscopePreviewMath.plan(4, 2, maxPixels = 2L))

        assertEquals(2, plan.targetWidth)
        assertEquals(1, plan.targetHeight)
        assertEquals(1, plan.sourceX(0))
        assertEquals(3, plan.sourceX(1))
        assertEquals(1, plan.sourceY(0))
    }

    @Test
    fun invalidPreviewBoundsAreRejected() {
        assertNull(MicroscopePreviewMath.plan(0, 720))
        assertNull(MicroscopePreviewMath.plan(1_280, -1))
        assertNull(MicroscopePreviewMath.plan(1_280, 720, maxPixels = 0L))
    }

    @Test
    fun signedTimestampFormattingNeverClampsPrerollToZero() {
        assertEquals("-00:00.000025", MicroscopePreviewMath.formatTimestampUs(-25L))
        assertEquals("-00:01.000025", MicroscopePreviewMath.formatTimestampUs(-1_000_025L))
        assertEquals("01:02:03.000004", MicroscopePreviewMath.formatTimestampUs(3_723_000_004L))
    }

    @Test
    fun signedTimestampInputAllowsOnlyOneLeadingMinusAndDigits() {
        assertEquals("-25000", MicroscopePreviewMath.sanitizeSignedTimestampInput("-25a000"))
        assertEquals("25000", MicroscopePreviewMath.sanitizeSignedTimestampInput("2-5a000"))
        assertEquals("-", MicroscopePreviewMath.sanitizeSignedTimestampInput("-"))
    }
}
