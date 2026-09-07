package com.framescope.app

import com.framescope.app.ui.MicroscopeSwipeMath
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class MicroscopeSwipeMathTest {
    @Test
    fun leftSwipeRequestsNextFrame() {
        assertEquals(1, MicroscopeSwipeMath.direction(-120f, 400f, 48f))
    }

    @Test
    fun rightSwipeRequestsPreviousFrame() {
        assertEquals(-1, MicroscopeSwipeMath.direction(120f, 400f, 48f))
    }

    @Test
    fun shortDragDoesNotNavigate() {
        assertNull(MicroscopeSwipeMath.direction(40f, 400f, 48f))
    }

    @Test
    fun viewportFractionCanRaiseThreshold() {
        assertNull(MicroscopeSwipeMath.direction(-100f, 1000f, 48f))
        assertEquals(1, MicroscopeSwipeMath.direction(-181f, 1000f, 48f))
    }

    @Test
    fun malformedInputsCannotNavigate() {
        assertNull(MicroscopeSwipeMath.direction(Float.NaN, 400f, 48f))
        assertNull(MicroscopeSwipeMath.direction(120f, Float.POSITIVE_INFINITY, 48f))
        assertNull(MicroscopeSwipeMath.direction(120f, 400f, 0f))
    }
}
