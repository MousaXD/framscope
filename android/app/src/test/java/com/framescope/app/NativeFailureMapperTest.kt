package com.framescope.app

import com.framescope.app.data.NativeFailureMapper
import com.framescope.app.data.NativeInspection
import com.framescope.app.data.VideoOpenErrorKind
import com.framescope.app.data.VideoOpenException
import kotlinx.coroutines.CancellationException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class NativeFailureMapperTest {
    @Test
    fun unsupportedCodecMapsToUsefulUserError() {
        val error = NativeFailureMapper.toThrowable(
            NativeInspection.Failure(
                code = "unsupported_codec",
                message = "decoder not enabled",
                engine = "framescope-rust/0.2.0",
            ),
        )

        assertTrue(error is VideoOpenException)
        error as VideoOpenException
        assertEquals(VideoOpenErrorKind.UnsupportedVideo, error.kind)
        assertTrue(error.message.orEmpty().contains("not supported"))
        assertEquals("unsupported_codec: decoder not enabled", error.diagnostic)
    }

    @Test
    fun malformedDataMapsToCorruptMedia() {
        val error = NativeFailureMapper.toThrowable(
            NativeInspection.Failure(
                code = "malformed_data",
                message = "invalid packet",
                engine = "framescope-rust/0.2.0",
            ),
        )

        assertTrue(error is VideoOpenException)
        assertEquals(VideoOpenErrorKind.CorruptMedia, (error as VideoOpenException).kind)
    }

    @Test
    fun decoderFailureMapsWithoutLeakingFfmpegCodesToUserMessage() {
        val error = NativeFailureMapper.toThrowable(
            NativeInspection.Failure(
                code = "decoder_failure",
                message = "avcodec_send_packet returned -1094995529",
                engine = "framescope-rust/0.2.0",
            ),
        )

        assertTrue(error is VideoOpenException)
        error as VideoOpenException
        assertEquals(VideoOpenErrorKind.DecoderFailure, error.kind)
        assertEquals("FrameScope could not decode this video's selected video stream.", error.message)
        assertTrue(error.diagnostic.orEmpty().contains("-1094995529"))
    }

    @Test
    fun nativeCancellationRemainsCoroutineCancellation() {
        val error = NativeFailureMapper.toThrowable(
            NativeInspection.Failure(
                code = "cancelled",
                message = "cancel token set",
                engine = "framescope-rust/0.2.0",
            ),
        )

        assertTrue(error is CancellationException)
    }
}
