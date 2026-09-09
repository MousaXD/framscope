package com.framescope.app.ui

import com.framescope.app.data.PreparedMicroscopeFrame
import java.nio.ByteBuffer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class MicroscopeInspectorMathTest {
    @Test
    fun oneToOneScaleUndoFitDownscaling() {
        val plan = requireNotNull(
            MicroscopeInspectorMath.viewportPlan(
                imageWidth = 1920,
                imageHeight = 1080,
                viewportWidth = 960,
                viewportHeight = 540,
            ),
        )

        assertEquals(0.5f, plan.fitScale, 0.0001f)
        assertEquals(2f, plan.oneToOneScale, 0.0001f)
        assertEquals(1f, plan.minimumScale, 0.0001f)
        assertEquals(8f, plan.maximumScale, 0.0001f)
    }

    @Test
    fun oneToOneScaleCanShrinkSmallImageThatFitWouldUpscale() {
        val plan = requireNotNull(
            MicroscopeInspectorMath.viewportPlan(
                imageWidth = 100,
                imageHeight = 100,
                viewportWidth = 400,
                viewportHeight = 400,
            ),
        )

        assertEquals(4f, plan.fitScale, 0.0001f)
        assertEquals(0.25f, plan.oneToOneScale, 0.0001f)
        assertEquals(0.25f, plan.minimumScale, 0.0001f)
    }

    @Test
    fun translationClampUsesActualFittedImageDimensions() {
        val (x, y) = MicroscopeInspectorMath.clampTranslation(
            translationX = 10_000f,
            translationY = -10_000f,
            relativeScale = 2f,
            imageWidth = 1920,
            imageHeight = 1080,
            viewportWidth = 960,
            viewportHeight = 540,
        )

        assertEquals(480f, x, 0.0001f)
        assertEquals(-270f, y, 0.0001f)
    }

    @Test
    fun rgbaSamplerHonorsStridePaddingAndCoordinates() {
        val descriptor = PreparedMicroscopeFrame(
            sessionId = 7L,
            frameId = 3L,
            generation = 2L,
            width = 2,
            height = 2,
            strideBytes = 12L,
            byteLen = 24,
        )
        val rgba = ByteBuffer.allocateDirect(24)
        rgba.put(16, 0x12.toByte())
        rgba.put(17, 0x34.toByte())
        rgba.put(18, 0x56.toByte())
        rgba.put(19, 0x78.toByte())

        val sample = MicroscopeInspectorMath.sampleRgba(descriptor, rgba, x = 1, y = 1)

        assertEquals(
            InspectorRgbaSample(
                x = 1,
                y = 1,
                red = 0x12,
                green = 0x34,
                blue = 0x56,
                alpha = 0x78,
            ),
            sample,
        )
        assertNull(MicroscopeInspectorMath.sampleRgba(descriptor, rgba, x = 2, y = 1))
    }

    @Test
    fun viewportPlanRejectsInvalidGeometry() {
        assertNull(MicroscopeInspectorMath.viewportPlan(0, 1080, 960, 540))
        assertNull(MicroscopeInspectorMath.viewportPlan(1920, 1080, 0, 540))
    }
}
