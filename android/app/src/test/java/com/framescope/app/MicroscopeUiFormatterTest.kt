package com.framescope.app

import com.framescope.app.data.FrameDetails
import com.framescope.app.data.MicroscopeSessionSnapshot
import com.framescope.app.ui.MicroscopeUiFormatter
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MicroscopeUiFormatterTest {
    @Test
    fun exactTimingUsesSignedPtsTicksAndTimeBaseWithoutFpsMath() {
        val frame = frame(
            frameId = 17L,
            timestampTicks = -25L,
            durationTicks = 1_001L,
            timeBaseNumerator = 1,
            timeBaseDenominator = 30_000,
        )

        assertEquals("-25 ticks @ 1/30000 s", MicroscopeUiFormatter.exactTimestamp(frame))
        assertEquals("1001 ticks @ 1/30000 s", MicroscopeUiFormatter.exactDuration(frame))
    }

    @Test
    fun unavailablePtsAndDurationRemainExplicitlyUnavailable() {
        val frame = frame(
            frameId = 0L,
            timestampTicks = null,
            durationTicks = null,
        )

        assertEquals("Unavailable", MicroscopeUiFormatter.exactTimestamp(frame))
        assertNull(MicroscopeUiFormatter.exactDuration(frame))
    }

    @Test
    fun controlsFollowAuthoritativeSessionBoundaries() {
        val session = MicroscopeSessionSnapshot(
            sessionId = 4L,
            frameCount = 3L,
            currentFrame = frame(frameId = 1L, timestampTicks = 1_001L, durationTicks = 1_001L),
            canStepPrevious = true,
            canStepNext = true,
        )

        val controls = requireNotNull(MicroscopeUiFormatter.controls(session, busy = false))

        assertEquals("#2 of 3 · id 1", controls.framePosition)
        assertTrue(controls.canStepPrevious)
        assertTrue(controls.canStepNext)
    }

    @Test
    fun busyNavigationDisablesBothStepDirections() {
        val session = MicroscopeSessionSnapshot(
            sessionId = 5L,
            frameCount = 2L,
            currentFrame = frame(frameId = 0L, timestampTicks = 0L, durationTicks = 1_001L),
            canStepPrevious = false,
            canStepNext = true,
        )

        val controls = requireNotNull(MicroscopeUiFormatter.controls(session, busy = true))

        assertFalse(controls.canStepPrevious)
        assertFalse(controls.canStepNext)
    }

    @Test
    fun emptySessionHasNoFrameControls() {
        val session = MicroscopeSessionSnapshot(
            sessionId = 6L,
            frameCount = 0L,
            currentFrame = null,
            canStepPrevious = false,
            canStepNext = false,
        )

        assertNull(MicroscopeUiFormatter.controls(session, busy = false))
    }

    private fun frame(
        frameId: Long,
        timestampTicks: Long?,
        durationTicks: Long?,
        timeBaseNumerator: Int = 1,
        timeBaseDenominator: Int = 30_000,
    ) = FrameDetails(
        frameId = frameId,
        timestampTicks = timestampTicks,
        timestampUs = null,
        timeBaseNumerator = timeBaseNumerator,
        timeBaseDenominator = timeBaseDenominator,
        durationTicks = durationTicks,
        keyframe = frameId == 0L,
        corrupt = false,
    )
}
