package com.framescope.app

import com.framescope.app.data.MicroscopeFrameBridge
import com.framescope.app.data.NativeFrameCopy
import com.framescope.app.data.NativeFramePreparation
import com.framescope.app.data.PreparedMicroscopeFrame
import java.nio.ByteBuffer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MicroscopeFrameHandoffTest {
    @Test
    fun parsesMetadataOnlyPreparedFrameDescriptor() {
        val raw = """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "frame":{
                "session_id":7,
                "frame_id":4,
                "generation":12,
                "width":2,
                "height":2,
                "stride_bytes":8,
                "byte_len":16
              }
            }
        """.trimIndent()

        val result = MicroscopeFrameBridge.parsePreparationResponse(raw)

        assertTrue(result is NativeFramePreparation.Success)
        result as NativeFramePreparation.Success
        assertEquals(7L, result.frame.sessionId)
        assertEquals(4L, result.frame.frameId)
        assertEquals(12L, result.frame.generation)
        assertEquals(16, result.frame.byteLen)
        assertFalse(raw.contains("pixels"))
        assertFalse(raw.contains("base64"))
    }

    @Test
    fun rejectsStrideAndByteLengthMismatch() {
        val result = MicroscopeFrameBridge.parsePreparationResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "frame":{
                "session_id":7,
                "frame_id":4,
                "generation":12,
                "width":2,
                "height":2,
                "stride_bytes":8,
                "byte_len":15
              }
            }
            """.trimIndent(),
        )

        assertTrue(result is NativeFramePreparation.Failure)
        result as NativeFramePreparation.Failure
        assertEquals("malformed_frame_descriptor", result.code)
    }

    @Test
    fun rejectsFrameAbovePresentationByteLimit() {
        val tooLarge = PreparedMicroscopeFrame(
            sessionId = 1,
            frameId = 0,
            generation = 1,
            width = 65_535,
            height = 1_025,
            strideBytes = 65_535L * 4L,
            byteLen = (65_535L * 4L * 1_025L).toInt(),
        )

        assertFalse(tooLarge.isSane())
    }

    @Test
    fun exactCopyCountReturnsDirectReadOnlyBuffer() {
        val frame = smallFrame()
        val destination = ByteBuffer.allocateDirect(frame.byteLen)

        val result = MicroscopeFrameBridge.interpretCopyResult(
            frame,
            destination,
            frame.byteLen.toLong(),
        )

        assertTrue(result is NativeFrameCopy.Success)
        result as NativeFrameCopy.Success
        assertTrue(result.rgba.isDirect)
        assertTrue(result.rgba.isReadOnly)
        assertEquals(0, result.rgba.position())
        assertEquals(frame.byteLen, result.rgba.limit())
    }

    @Test
    fun staleGenerationIsTypedAndCopyLengthMustBeExact() {
        val frame = smallFrame()
        val stale = MicroscopeFrameBridge.interpretCopyResult(
            frame,
            ByteBuffer.allocateDirect(frame.byteLen),
            -3,
        )
        assertTrue(stale is NativeFrameCopy.Failure)
        stale as NativeFrameCopy.Failure
        assertEquals("stale_generation", stale.code)

        val shortCopy = MicroscopeFrameBridge.interpretCopyResult(
            frame,
            ByteBuffer.allocateDirect(frame.byteLen),
            frame.byteLen.toLong() - 1,
        )
        assertTrue(shortCopy is NativeFrameCopy.Failure)
        shortCopy as NativeFrameCopy.Failure
        assertEquals("copy_length_mismatch", shortCopy.code)
    }

    private fun smallFrame() = PreparedMicroscopeFrame(
        sessionId = 7,
        frameId = 4,
        generation = 12,
        width = 2,
        height = 2,
        strideBytes = 8,
        byteLen = 16,
    )
}
