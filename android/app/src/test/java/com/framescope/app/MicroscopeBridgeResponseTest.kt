package com.framescope.app

import com.framescope.app.data.NativeMicroscope
import com.framescope.app.data.RustBridge
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MicroscopeBridgeResponseTest {
    @Test
    fun parsesExactFrameIdentityAndSignedTimestamp() {
        val response = RustBridge.parseMicroscopeResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "session":{
                "session_id":7,
                "frame_count":12,
                "current_frame":{
                  "frame_id":4,
                  "timestamp_ticks":-90,
                  "timestamp_us":-1000,
                  "time_base_numerator":1,
                  "time_base_denominator":90000,
                  "duration_ticks":3003,
                  "keyframe":false,
                  "corrupt":false
                },
                "can_step_previous":true,
                "can_step_next":true
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeMicroscope.Success)
        response as NativeMicroscope.Success
        assertEquals(7L, response.session.sessionId)
        assertEquals(12L, response.session.frameCount)
        assertEquals(4L, response.session.currentFrame?.frameId)
        assertEquals(-90L, response.session.currentFrame?.timestampTicks)
        assertEquals(-1000L, response.session.currentFrame?.timestampUs)
        assertEquals(1, response.session.currentFrame?.timeBaseNumerator)
        assertEquals(90000, response.session.currentFrame?.timeBaseDenominator)
        assertEquals(3003L, response.session.currentFrame?.durationTicks)
    }

    @Test
    fun parsesEmptyCompleteIndexWithoutInventingFrameZero() {
        val response = RustBridge.parseMicroscopeResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "session":{
                "session_id":8,
                "frame_count":0,
                "current_frame":null,
                "can_step_previous":false,
                "can_step_next":false
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeMicroscope.Success)
        response as NativeMicroscope.Success
        assertNull(response.session.currentFrame)
    }

    @Test
    fun rejectsInconsistentStepFlags() {
        val response = RustBridge.parseMicroscopeResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "session":{
                "session_id":9,
                "frame_count":3,
                "current_frame":{
                  "frame_id":0,
                  "timestamp_ticks":0,
                  "timestamp_us":0,
                  "time_base_numerator":1,
                  "time_base_denominator":1000,
                  "duration_ticks":40,
                  "keyframe":true,
                  "corrupt":false
                },
                "can_step_previous":true,
                "can_step_next":true
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeMicroscope.Failure)
        response as NativeMicroscope.Failure
        assertEquals("malformed_microscope_state", response.code)
    }

    @Test
    fun rejectsFrameOutsideReportedIndex() {
        val response = RustBridge.parseMicroscopeResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "session":{
                "session_id":10,
                "frame_count":2,
                "current_frame":{
                  "frame_id":2,
                  "timestamp_ticks":80,
                  "timestamp_us":80000,
                  "time_base_numerator":1,
                  "time_base_denominator":1000,
                  "duration_ticks":40,
                  "keyframe":false,
                  "corrupt":false
                },
                "can_step_previous":true,
                "can_step_next":false
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeMicroscope.Failure)
        response as NativeMicroscope.Failure
        assertEquals("malformed_microscope_state", response.code)
    }

    @Test
    fun preservesNativeNavigationErrorCode() {
        val response = RustBridge.parseMicroscopeResponse(
            """
            {
              "status":"error",
              "engine":"framescope-rust/0.1.0",
              "code":"frame_boundary",
              "message":"cannot step beyond the indexed frame range"
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeMicroscope.Failure)
        response as NativeMicroscope.Failure
        assertEquals("frame_boundary", response.code)
    }
}
