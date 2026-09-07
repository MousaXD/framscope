package com.framescope.app

import com.framescope.app.ui.MicroscopeTransformMath
import com.framescope.app.ui.MicroscopeViewportTransform
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class MicroscopeTransformMathTest {
    @Test
    fun centerPinchZoomKeepsTranslationAtOrigin() {
        val next = MicroscopeTransformMath.applyGesture(
            current = MicroscopeViewportTransform(),
            zoomChange = 2f,
            panX = 0f,
            panY = 0f,
            centroidX = 100f,
            centroidY = 50f,
            viewportWidth = 200f,
            viewportHeight = 100f,
        )

        assertEquals(2f, next.scale)
        assertEquals(0f, next.translationX)
        assertEquals(0f, next.translationY)
    }

    @Test
    fun offCenterPinchPreservesGestureFocus() {
        val next = MicroscopeTransformMath.applyGesture(
            current = MicroscopeViewportTransform(),
            zoomChange = 2f,
            panX = 0f,
            panY = 0f,
            centroidX = 150f,
            centroidY = 50f,
            viewportWidth = 200f,
            viewportHeight = 100f,
        )

        assertEquals(2f, next.scale)
        assertEquals(-50f, next.translationX)
        assertEquals(0f, next.translationY)
    }

    @Test
    fun panIsClampedToScaledViewportBounds() {
        val next = MicroscopeTransformMath.applyGesture(
            current = MicroscopeViewportTransform(scale = 2f),
            zoomChange = 1f,
            panX = 10_000f,
            panY = -10_000f,
            centroidX = 100f,
            centroidY = 50f,
            viewportWidth = 200f,
            viewportHeight = 100f,
        )

        assertEquals(100f, next.translationX)
        assertEquals(-50f, next.translationY)
    }

    @Test
    fun viewportResizeReclampsExistingTranslation() {
        val next = MicroscopeTransformMath.applyGesture(
            current = MicroscopeViewportTransform(scale = 2f, translationX = 100f, translationY = 50f),
            zoomChange = 1f,
            panX = 0f,
            panY = 0f,
            centroidX = 50f,
            centroidY = 25f,
            viewportWidth = 100f,
            viewportHeight = 50f,
        )

        assertEquals(2f, next.scale)
        assertEquals(50f, next.translationX)
        assertEquals(25f, next.translationY)
    }

    @Test
    fun zoomLimitsStayInsideProductBounds() {
        val tooLarge = MicroscopeTransformMath.applyGesture(
            current = MicroscopeViewportTransform(scale = 4f),
            zoomChange = 100f,
            panX = 0f,
            panY = 0f,
            centroidX = 100f,
            centroidY = 50f,
            viewportWidth = 200f,
            viewportHeight = 100f,
        )
        val tooSmall = MicroscopeTransformMath.applyGesture(
            current = tooLarge,
            zoomChange = 0.0001f,
            panX = 0f,
            panY = 0f,
            centroidX = 100f,
            centroidY = 50f,
            viewportWidth = 200f,
            viewportHeight = 100f,
        )

        assertEquals(MicroscopeTransformMath.MAX_SCALE, tooLarge.scale)
        assertEquals(MicroscopeTransformMath.MIN_SCALE, tooSmall.scale)
        assertEquals(0f, tooSmall.translationX)
        assertEquals(0f, tooSmall.translationY)
    }

    @Test
    fun invalidGestureNumbersCannotPoisonTransformState() {
        val next = MicroscopeTransformMath.applyGesture(
            current = MicroscopeViewportTransform(
                scale = Float.NaN,
                translationX = Float.POSITIVE_INFINITY,
                translationY = Float.NEGATIVE_INFINITY,
            ),
            zoomChange = Float.NaN,
            panX = Float.NaN,
            panY = Float.POSITIVE_INFINITY,
            centroidX = Float.NaN,
            centroidY = Float.NaN,
            viewportWidth = 200f,
            viewportHeight = 100f,
        )

        assertTrue(next.scale.isFinite())
        assertTrue(next.translationX.isFinite())
        assertTrue(next.translationY.isFinite())
        assertEquals(1f, next.scale)
    }

    @Test
    fun unknownViewportNeverProducesUnboundedTranslation() {
        val next = MicroscopeTransformMath.applyGesture(
            current = MicroscopeViewportTransform(scale = 2f, translationX = 20f, translationY = 30f),
            zoomChange = 1f,
            panX = 50f,
            panY = 50f,
            centroidX = 0f,
            centroidY = 0f,
            viewportWidth = 0f,
            viewportHeight = 0f,
        )

        assertEquals(2f, next.scale)
        assertEquals(0f, next.translationX)
        assertEquals(0f, next.translationY)
    }

    @Test
    fun nonFiniteViewportCannotProduceNanTranslation() {
        val next = MicroscopeTransformMath.applyGesture(
            current = MicroscopeViewportTransform(scale = 2f, translationX = 20f, translationY = 30f),
            zoomChange = 1f,
            panX = 50f,
            panY = 50f,
            centroidX = 0f,
            centroidY = 0f,
            viewportWidth = Float.NaN,
            viewportHeight = Float.POSITIVE_INFINITY,
        )

        assertEquals(2f, next.scale)
        assertTrue(next.translationX.isFinite())
        assertTrue(next.translationY.isFinite())
        assertEquals(0f, next.translationX)
        assertEquals(0f, next.translationY)
    }
}
