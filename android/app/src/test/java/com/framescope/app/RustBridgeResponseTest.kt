package com.framescope.app

import com.framescope.app.data.NativeInspection
import com.framescope.app.data.RustBridge
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class RustBridgeResponseTest {
    @Test
    fun parsesValidRustMetadataEnvelope() {
        val response = RustBridge.parseResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "metadata":{
                "duration_us":10000000,
                "width":1920,
                "height":1080,
                "estimated_frame_rate":29.97,
                "rotation_degrees":90
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeInspection.Success)
        response as NativeInspection.Success
        assertEquals("framescope-rust/0.1.0", response.engine)
        assertEquals(1920, response.metadata.width)
        assertEquals(90, response.metadata.rotationDegrees)
    }

    @Test
    fun rejectsSuccessEnvelopeWithoutEngineIdentity() {
        val response = RustBridge.parseResponse(
            """
            {
              "status":"ok",
              "metadata":{
                "duration_us":1000000,
                "width":640,
                "height":480,
                "estimated_frame_rate":null,
                "rotation_degrees":0
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeInspection.Failure)
        response as NativeInspection.Failure
        assertEquals("malformed_response", response.code)
    }

    @Test
    fun rejectsUnsafeMetadataFromNativeBoundary() {
        val response = RustBridge.parseResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "metadata":{
                "duration_us":1000000,
                "width":0,
                "height":480,
                "estimated_frame_rate":30.0,
                "rotation_degrees":0
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeInspection.Failure)
        response as NativeInspection.Failure
        assertEquals("malformed_metadata", response.code)
    }

    @Test
    fun preservesStableRustErrorCode() {
        val response = RustBridge.parseResponse(
            """
            {
              "status":"error",
              "engine":"framescope-rust/0.1.0",
              "code":"unsupported_format",
              "message":"unsupported video format"
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeInspection.Failure)
        response as NativeInspection.Failure
        assertEquals("unsupported_format", response.code)
    }
}
