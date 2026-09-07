package com.framescope.app

import com.framescope.app.data.FrameExportFormat
import com.framescope.app.data.NativeFrameExport
import com.framescope.app.data.RustBridge
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FrameExportBridgeResponseTest {
    @Test
    fun parsesTypedPngExportMetadata() {
        val response = RustBridge.parseFrameExportResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "export":{
                "session_id":7,
                "frame_id":42,
                "width":1920,
                "height":1080,
                "format":"png",
                "mime_type":"image/png",
                "byte_len":123456
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeFrameExport.Success)
        response as NativeFrameExport.Success
        assertEquals(7L, response.export.sessionId)
        assertEquals(42L, response.export.frameId)
        assertEquals(1920, response.export.width)
        assertEquals(1080, response.export.height)
        assertEquals(FrameExportFormat.Png, response.export.format)
        assertEquals("image/png", response.export.mimeType)
        assertEquals(123456L, response.export.byteLength)
    }

    @Test
    fun rejectsUnknownWireFormat() {
        val response = RustBridge.parseFrameExportResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "export":{
                "session_id":7,
                "frame_id":42,
                "width":1920,
                "height":1080,
                "format":"bmp",
                "mime_type":"image/bmp",
                "byte_len":123456
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeFrameExport.Failure)
        response as NativeFrameExport.Failure
        assertEquals("malformed_export_state", response.code)
    }

    @Test
    fun rejectsMimeTypeThatDoesNotMatchFormat() {
        val response = RustBridge.parseFrameExportResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "export":{
                "session_id":7,
                "frame_id":42,
                "width":1920,
                "height":1080,
                "format":"jpeg",
                "mime_type":"image/png",
                "byte_len":123456
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeFrameExport.Failure)
        response as NativeFrameExport.Failure
        assertEquals("malformed_export_state", response.code)
    }

    @Test
    fun preservesNativeExportFailureCode() {
        val response = RustBridge.parseFrameExportResponse(
            """
            {
              "status":"error",
              "engine":"framescope-rust/0.1.0",
              "code":"stale_generation",
              "message":"microscope moved before export"
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeFrameExport.Failure)
        response as NativeFrameExport.Failure
        assertEquals("stale_generation", response.code)
        assertEquals("microscope moved before export", response.message)
    }

    @Test
    fun rejectsNonPositiveEncodedLength() {
        val response = RustBridge.parseFrameExportResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "export":{
                "session_id":7,
                "frame_id":42,
                "width":1920,
                "height":1080,
                "format":"webp_lossless",
                "mime_type":"image/webp",
                "byte_len":0
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeFrameExport.Failure)
        response as NativeFrameExport.Failure
        assertEquals("malformed_export_state", response.code)
    }
}
