package com.framescope.app

import com.framescope.app.data.NativeInspection
import com.framescope.app.data.RustBridge
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RustBridgeResponseTest {
    @Test
    fun parsesRichRustMetadataEnvelopeWithoutTreatingFpsAsTiming() {
        val response = RustBridge.parseResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.2.0",
              "metadata":{
                "duration_us":10000000,
                "width":1920,
                "height":1080,
                "estimated_frame_rate":29.97,
                "rotation_degrees":90,
                "container":"matroska,webm",
                "codec":"vp9",
                "video_stream_index":1,
                "video_stream_count":2,
                "audio_stream_count":1,
                "pixel_format":"yuv420p",
                "variable_frame_rate":true
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeInspection.Success)
        response as NativeInspection.Success
        assertEquals("framescope-rust/0.2.0", response.engine)
        assertEquals(1920, response.metadata.width)
        assertEquals(90, response.metadata.rotationDegrees)
        assertEquals("matroska,webm", response.metadata.container)
        assertEquals("vp9", response.metadata.codec)
        assertEquals(1, response.metadata.videoStreamIndex)
        assertEquals(2, response.metadata.videoStreamCount)
        assertEquals(true, response.metadata.variableFrameRate)
    }

    @Test
    fun acceptsUnavailableDurationAndOptionalEngineMetadata() {
        val response = RustBridge.parseResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.2.0",
              "metadata":{
                "duration_us":null,
                "width":640,
                "height":480,
                "estimated_frame_rate":null,
                "rotation_degrees":0
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeInspection.Success)
        response as NativeInspection.Success
        assertNull(response.metadata.durationUs)
        assertNull(response.metadata.estimatedFrameRate)
        assertNull(response.metadata.codec)
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
              "engine":"framescope-rust/0.2.0",
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
              "engine":"framescope-rust/0.2.0",
              "code":"unsupported_codec",
              "message":"unsupported video codec"
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeInspection.Failure)
        response as NativeInspection.Failure
        assertEquals("unsupported_codec", response.code)
    }
}
