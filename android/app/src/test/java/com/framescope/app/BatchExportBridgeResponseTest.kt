package com.framescope.app

import com.framescope.app.data.FrameExportFormat
import com.framescope.app.data.NativeBatchExport
import com.framescope.app.data.RustBridge
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class BatchExportBridgeResponseTest {
    @Test
    fun `valid complete batch response is accepted`() {
        val result = RustBridge.parseBatchExportResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "export":{
                "session_id":9,
                "expected_frames":3,
                "committed_frames":3,
                "encoded_bytes":4096,
                "decoded_frames":7,
                "used_keyframe_seek":true,
                "fell_back_to_stream_start":false,
                "format":"png",
                "mime_type":"image/png"
              }
            }
            """.trimIndent(),
        )

        assertTrue(result is NativeBatchExport.Success)
        val success = result as NativeBatchExport.Success
        assertEquals(9L, success.export.sessionId)
        assertEquals(3L, success.export.committedFrames)
        assertEquals(7L, success.export.decodedFrames)
        assertEquals(FrameExportFormat.Png, success.export.format)
    }

    @Test
    fun `success cannot claim an incomplete committed frame count`() {
        val result = RustBridge.parseBatchExportResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "export":{
                "session_id":9,
                "expected_frames":3,
                "committed_frames":2,
                "encoded_bytes":4096,
                "decoded_frames":7,
                "used_keyframe_seek":false,
                "fell_back_to_stream_start":true,
                "format":"jpeg",
                "mime_type":"image/jpeg"
              }
            }
            """.trimIndent(),
        )

        assertTrue(result is NativeBatchExport.Failure)
        assertEquals("malformed_export_state", (result as NativeBatchExport.Failure).code)
    }

    @Test
    fun `format and mime mismatch is rejected`() {
        val result = RustBridge.parseBatchExportResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "export":{
                "session_id":9,
                "expected_frames":1,
                "committed_frames":1,
                "encoded_bytes":100,
                "decoded_frames":1,
                "used_keyframe_seek":false,
                "fell_back_to_stream_start":false,
                "format":"webp_lossless",
                "mime_type":"image/png"
              }
            }
            """.trimIndent(),
        )

        assertTrue(result is NativeBatchExport.Failure)
        assertEquals("malformed_export_state", (result as NativeBatchExport.Failure).code)
    }

    @Test
    fun `native failure preserves code message and engine`() {
        val result = RustBridge.parseBatchExportResponse(
            """
            {
              "status":"error",
              "engine":"framescope-rust/0.1.0",
              "code":"cancelled",
              "message":"batch export was cancelled"
            }
            """.trimIndent(),
        )

        assertEquals(
            NativeBatchExport.Failure(
                code = "cancelled",
                message = "batch export was cancelled",
                engine = "framescope-rust/0.1.0",
            ),
            result,
        )
    }
}
