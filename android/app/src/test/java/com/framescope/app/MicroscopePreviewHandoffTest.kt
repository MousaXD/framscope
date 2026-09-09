package com.framescope.app

import com.framescope.app.data.MicroscopePreviewBridge
import com.framescope.app.data.NativeMicroscopePreview
import java.nio.ByteBuffer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MicroscopePreviewHandoffTest {
    @Test
    fun parsesBoundedPreviewIntoCallerOwnedReadOnlyBuffer() {
        val destination = ByteBuffer.allocateDirect(640 * 640 * 4)
        val result = MicroscopePreviewBridge.parseResponse(
            raw = successJson(sessionId = 7L),
            destination = destination,
            expectedSessionId = 7L,
            maxEdge = 640,
        )

        assertTrue(result is NativeMicroscopePreview.Success)
        result as NativeMicroscopePreview.Success
        assertEquals(7L, result.preview.descriptor.sessionId)
        assertEquals(4L, result.preview.descriptor.frameId)
        assertEquals(123_456L, result.preview.descriptor.timestampUs)
        assertEquals("decoded", result.preview.descriptor.source)
        assertEquals(3L, result.preview.descriptor.decodedFrames)
        val rgba = assertNotNull(result.preview.rgba).let { result.preview.rgba!! }
        assertTrue(rgba.isDirect)
        assertTrue(rgba.isReadOnly)
        assertEquals(0, rgba.position())
        assertEquals(16, rgba.limit())
    }

    @Test
    fun rejectsPreviewFromDifferentMicroscopeSession() {
        val result = MicroscopePreviewBridge.parseResponse(
            raw = successJson(sessionId = 8L),
            destination = ByteBuffer.allocateDirect(640 * 640 * 4),
            expectedSessionId = 7L,
            maxEdge = 640,
        )

        assertTrue(result is NativeMicroscopePreview.Failure)
        result as NativeMicroscopePreview.Failure
        assertEquals("malformed_preview_descriptor", result.code)
    }

    @Test
    fun rejectsPreviewWhoseDimensionsExceedRequestedBound() {
        val raw = """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "preview":{
                "session_id":7,
                "frame_id":4,
                "timestamp_us":123456,
                "width":641,
                "height":1,
                "stride_bytes":2564,
                "byte_len":2564,
                "source":"decoded",
                "decoded_frames":3
              }
            }
        """.trimIndent()

        val result = MicroscopePreviewBridge.parseResponse(
            raw = raw,
            destination = ByteBuffer.allocateDirect(640 * 640 * 4),
            expectedSessionId = 7L,
            maxEdge = 640,
        )

        assertTrue(result is NativeMicroscopePreview.Failure)
        result as NativeMicroscopePreview.Failure
        assertEquals("malformed_preview_descriptor", result.code)
    }

    @Test
    fun rejectsPreviewWithPaddedOrInconsistentRgbaLayout() {
        val raw = """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "preview":{
                "session_id":7,
                "frame_id":4,
                "timestamp_us":123456,
                "width":2,
                "height":2,
                "stride_bytes":12,
                "byte_len":24,
                "source":"decoded",
                "decoded_frames":3
              }
            }
        """.trimIndent()

        val result = MicroscopePreviewBridge.parseResponse(
            raw = raw,
            destination = ByteBuffer.allocateDirect(640 * 640 * 4),
            expectedSessionId = 7L,
            maxEdge = 640,
        )

        assertTrue(result is NativeMicroscopePreview.Failure)
        result as NativeMicroscopePreview.Failure
        assertEquals("malformed_preview_descriptor", result.code)
    }

    private fun successJson(sessionId: Long) = """
        {
          "status":"ok",
          "engine":"framescope-rust/0.1.0",
          "preview":{
            "session_id":$sessionId,
            "frame_id":4,
            "timestamp_us":123456,
            "width":2,
            "height":2,
            "stride_bytes":8,
            "byte_len":16,
            "source":"decoded",
            "decoded_frames":3
          }
        }
    """.trimIndent()
}
