package com.framescope.app

import com.framescope.app.data.FrameDetails
import com.framescope.app.data.FrameExportFormat
import com.framescope.app.data.FrameExportResult
import com.framescope.app.data.MicroscopeSessionController
import com.framescope.app.data.MicroscopeSessionSnapshot
import com.framescope.app.data.NativeBridge
import com.framescope.app.data.NativeFrameCopy
import com.framescope.app.data.NativeFrameExport
import com.framescope.app.data.NativeFramePreparation
import com.framescope.app.data.NativeInspection
import com.framescope.app.data.NativeMicroscope
import com.framescope.app.data.NativeMicroscopeFrameBridge
import com.framescope.app.data.TimestampSelectionPolicy
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class MicroscopeExportOrderingTest {
    @Test
    fun matchingCurrentFrameExportIsAccepted() {
        val bridge = FakeBridge()
        val controller = MicroscopeSessionController(bridge, UnusedFrameBridge)
        open(controller)

        val result = controller.exportCurrentFrame(
            outputFd = 11,
            operationId = 91,
            format = FrameExportFormat.Png,
            jpegQuality = 92,
        )

        assertTrue(result is NativeFrameExport.Success)
        result as NativeFrameExport.Success
        assertEquals(7L, result.export.sessionId)
        assertEquals(0L, result.export.frameId)
        assertEquals(FrameExportFormat.Png, result.export.format)
    }

    @Test
    fun mismatchedNativeFrameIdentityIsRejected() {
        val bridge = FakeBridge().apply {
            onExport = { sessionId, _, _, format, _ ->
                NativeFrameExport.Success(
                    export = exportResult(
                        sessionId = sessionId,
                        frameId = 1,
                        format = format,
                    ),
                    engine = ENGINE,
                )
            }
        }
        val controller = MicroscopeSessionController(bridge, UnusedFrameBridge)
        open(controller)

        val result = controller.exportCurrentFrame(
            outputFd = 11,
            operationId = 92,
            format = FrameExportFormat.Png,
            jpegQuality = 92,
        )

        assertTrue(result is NativeFrameExport.Failure)
        result as NativeFrameExport.Failure
        assertEquals("export_identity_mismatch", result.code)
    }

    @Test
    fun navigationRacingSuccessfulNativeExportMakesResultStale() {
        val bridge = FakeBridge()
        lateinit var controller: MicroscopeSessionController
        controller = MicroscopeSessionController(bridge, UnusedFrameBridge)
        open(controller)
        bridge.onExport = { sessionId, _, _, format, _ ->
            val navigation = controller.step(1)
            assertTrue(navigation is NativeMicroscope.Success)
            NativeFrameExport.Success(
                export = exportResult(
                    sessionId = sessionId,
                    frameId = 0,
                    format = format,
                ),
                engine = ENGINE,
            )
        }

        val result = controller.exportCurrentFrame(
            outputFd = 11,
            operationId = 93,
            format = FrameExportFormat.Png,
            jpegQuality = 92,
        )

        assertTrue(result is NativeFrameExport.Failure)
        result as NativeFrameExport.Failure
        assertEquals("stale_result", result.code)
        assertEquals(1L, controller.currentSnapshot()?.currentFrame?.frameId)
    }

    @Test
    fun exportWithoutOpenSessionFailsBeforeNativeCall() {
        val bridge = FakeBridge()
        val controller = MicroscopeSessionController(bridge, UnusedFrameBridge)

        val result = controller.exportCurrentFrame(
            outputFd = 11,
            operationId = 94,
            format = FrameExportFormat.Png,
            jpegQuality = 92,
        )

        assertTrue(result is NativeFrameExport.Failure)
        result as NativeFrameExport.Failure
        assertEquals("session_not_found", result.code)
        assertEquals(0, bridge.exportCalls)
    }

    private fun open(controller: MicroscopeSessionController) {
        val result = controller.open(fd = 3, operationId = 80, cacheRoot = "/tmp/cache")
        assertTrue(result is NativeMicroscope.Success)
    }

    private class FakeBridge : NativeBridge {
        var exportCalls = 0
        var onExport: (
            sessionId: Long,
            outputFd: Int,
            operationId: Long,
            format: FrameExportFormat,
            jpegQuality: Int,
        ) -> NativeFrameExport = { sessionId, _, _, format, _ ->
            NativeFrameExport.Success(
                export = exportResult(
                    sessionId = sessionId,
                    frameId = 0,
                    format = format,
                ),
                engine = ENGINE,
            )
        }

        override fun version(): Result<String> = Result.success(ENGINE)

        override fun inspectVideoFd(fd: Int, operationId: Long): NativeInspection =
            NativeInspection.Failure("unused", "unused", ENGINE)

        override fun openMicroscopeSession(
            fd: Int,
            operationId: Long,
            cacheRoot: String,
        ): NativeMicroscope = NativeMicroscope.Success(snapshot(0), ENGINE)

        override fun stepMicroscope(sessionId: Long, delta: Int): NativeMicroscope =
            NativeMicroscope.Success(snapshot(1), ENGINE)

        override fun jumpMicroscopeFrame(sessionId: Long, frameId: Long): NativeMicroscope =
            NativeMicroscope.Success(snapshot(frameId), ENGINE)

        override fun jumpMicroscopeTimestampUs(
            sessionId: Long,
            timestampUs: Long,
            selection: TimestampSelectionPolicy,
        ): NativeMicroscope = NativeMicroscope.Success(snapshot(0), ENGINE)

        override fun exportCurrentMicroscopeFrame(
            sessionId: Long,
            outputFd: Int,
            operationId: Long,
            format: FrameExportFormat,
            jpegQuality: Int,
        ): NativeFrameExport {
            exportCalls += 1
            return onExport(sessionId, outputFd, operationId, format, jpegQuality)
        }

        override fun closeMicroscopeSession(sessionId: Long): Boolean = true
    }

    private object UnusedFrameBridge : NativeMicroscopeFrameBridge {
        override fun prepare(sessionId: Long): NativeFramePreparation = error("unused")
        override fun copy(frame: com.framescope.app.data.PreparedMicroscopeFrame): NativeFrameCopy =
            error("unused")
    }

    companion object {
        private const val ENGINE = "framescope-rust/test"

        private fun snapshot(frameId: Long): MicroscopeSessionSnapshot = MicroscopeSessionSnapshot(
            sessionId = 7,
            frameCount = 2,
            currentFrame = FrameDetails(
                frameId = frameId,
                timestampTicks = frameId * 40,
                timestampUs = frameId * 40_000,
                timeBaseNumerator = 1,
                timeBaseDenominator = 1_000,
                durationTicks = 40,
                keyframe = frameId == 0L,
                corrupt = false,
            ),
            canStepPrevious = frameId > 0,
            canStepNext = frameId < 1,
        )

        private fun exportResult(
            sessionId: Long,
            frameId: Long,
            format: FrameExportFormat,
        ): FrameExportResult = FrameExportResult(
            sessionId = sessionId,
            frameId = frameId,
            width = 2,
            height = 2,
            format = format,
            mimeType = format.mimeType,
            byteLength = 32,
        )
    }
}
