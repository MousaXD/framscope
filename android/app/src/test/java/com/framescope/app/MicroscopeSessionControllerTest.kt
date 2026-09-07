package com.framescope.app

import com.framescope.app.data.FrameDetails
import com.framescope.app.data.MicroscopeSessionController
import com.framescope.app.data.MicroscopeSessionSnapshot
import com.framescope.app.data.NativeBridge
import com.framescope.app.data.NativeFrameCopy
import com.framescope.app.data.NativeFramePreparation
import com.framescope.app.data.NativeInspection
import com.framescope.app.data.NativeMicroscope
import com.framescope.app.data.NativeMicroscopeFrameBridge
import com.framescope.app.data.PreparedMicroscopeFrame
import com.framescope.app.data.TimestampSelectionPolicy
import java.nio.ByteBuffer
import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MicroscopeSessionControllerTest {
    @Test
    fun lateOpenCannotReplaceNewerSessionAndIsClosed() {
        val firstEntered = CountDownLatch(1)
        val releaseFirst = CountDownLatch(1)
        val native = FakeNativeBridge(
            firstOpenEntered = firstEntered,
            releaseFirstOpen = releaseFirst,
        )
        val controller = MicroscopeSessionController(native, FakeFrameBridge())
        val firstResult = AtomicReference<NativeMicroscope>()

        val firstThread = Thread {
            firstResult.set(controller.open(fd = 11, operationId = 1, cacheRoot = "cache"))
        }
        firstThread.start()
        assertTrue(firstEntered.await(2, TimeUnit.SECONDS))

        val second = controller.open(fd = 12, operationId = 2, cacheRoot = "cache")
        assertTrue(second is NativeMicroscope.Success)
        assertEquals(2L, controller.currentSnapshot()?.sessionId)

        releaseFirst.countDown()
        firstThread.join(2_000)
        assertFalse(firstThread.isAlive)
        val late = firstResult.get()
        assertTrue(late is NativeMicroscope.Failure)
        late as NativeMicroscope.Failure
        assertEquals("stale_result", late.code)
        assertTrue(native.closedSessionIds.contains(1L))
        assertEquals(2L, controller.currentSnapshot()?.sessionId)
    }

    @Test
    fun navigationMakesInFlightFrameResultStaleBeforeCopy() {
        val prepareEntered = CountDownLatch(1)
        val releasePrepare = CountDownLatch(1)
        val frameBridge = FakeFrameBridge(
            prepareEntered = prepareEntered,
            releasePrepare = releasePrepare,
        )
        val native = FakeNativeBridge()
        val controller = MicroscopeSessionController(native, frameBridge)
        assertTrue(controller.open(11, 1, "cache") is NativeMicroscope.Success)

        val frameResult = AtomicReference<NativeFrameCopy>()
        val frameThread = Thread {
            frameResult.set(controller.loadCurrentFrame())
        }
        frameThread.start()
        assertTrue(prepareEntered.await(2, TimeUnit.SECONDS))

        val moved = controller.jumpToFrame(1)
        assertTrue(moved is NativeMicroscope.Success)
        assertEquals(1L, controller.currentSnapshot()?.currentFrame?.frameId)

        releasePrepare.countDown()
        frameThread.join(2_000)
        assertFalse(frameThread.isAlive)
        val stale = frameResult.get()
        assertTrue(stale is NativeFrameCopy.Failure)
        stale as NativeFrameCopy.Failure
        assertEquals("stale_result", stale.code)
        assertEquals(0, frameBridge.copyCalls)
    }

    @Test
    fun closeInvalidatesSessionAndClosesNativeHandleOnce() {
        val native = FakeNativeBridge()
        val controller = MicroscopeSessionController(native, FakeFrameBridge())
        assertTrue(controller.open(11, 7, "cache") is NativeMicroscope.Success)
        val sessionId = controller.currentSnapshot()!!.sessionId

        assertTrue(controller.closeCurrent())
        assertEquals(listOf(sessionId), native.closedSessionIds)
        assertEquals(null, controller.currentSnapshot())
        assertTrue(controller.closeCurrent())
        assertEquals(listOf(sessionId), native.closedSessionIds)

        val frame = controller.loadCurrentFrame()
        assertTrue(frame is NativeFrameCopy.Failure)
        frame as NativeFrameCopy.Failure
        assertEquals("session_not_found", frame.code)
    }

    private class FakeNativeBridge(
        private val firstOpenEntered: CountDownLatch? = null,
        private val releaseFirstOpen: CountDownLatch? = null,
    ) : NativeBridge {
        val closedSessionIds: MutableList<Long> = Collections.synchronizedList(mutableListOf())

        override fun version(): Result<String> = Result.success("test")

        override fun inspectVideoFd(fd: Int, operationId: Long): NativeInspection =
            NativeInspection.Failure("unused", "unused", null)

        override fun openMicroscopeSession(
            fd: Int,
            operationId: Long,
            cacheRoot: String,
        ): NativeMicroscope {
            if (operationId == 1L && firstOpenEntered != null && releaseFirstOpen != null) {
                firstOpenEntered.countDown()
                check(releaseFirstOpen.await(2, TimeUnit.SECONDS))
            }
            return NativeMicroscope.Success(
                session = snapshot(sessionId = operationId, frameId = 0),
                engine = "test-engine",
            )
        }

        override fun jumpMicroscopeFrame(sessionId: Long, frameId: Long): NativeMicroscope =
            NativeMicroscope.Success(snapshot(sessionId, frameId), "test-engine")

        override fun stepMicroscope(sessionId: Long, delta: Int): NativeMicroscope {
            val current = if (delta > 0) 1L else 0L
            return NativeMicroscope.Success(snapshot(sessionId, current), "test-engine")
        }

        override fun jumpMicroscopeTimestampUs(
            sessionId: Long,
            timestampUs: Long,
            selection: TimestampSelectionPolicy,
        ): NativeMicroscope = NativeMicroscope.Success(snapshot(sessionId, 0), "test-engine")

        override fun closeMicroscopeSession(sessionId: Long): Boolean {
            closedSessionIds.add(sessionId)
            return true
        }
    }

    private class FakeFrameBridge(
        private val prepareEntered: CountDownLatch? = null,
        private val releasePrepare: CountDownLatch? = null,
    ) : NativeMicroscopeFrameBridge {
        var copyCalls: Int = 0

        override fun prepare(sessionId: Long): NativeFramePreparation {
            if (prepareEntered != null && releasePrepare != null) {
                prepareEntered.countDown()
                check(releasePrepare.await(2, TimeUnit.SECONDS))
            }
            return NativeFramePreparation.Success(
                frame = PreparedMicroscopeFrame(
                    sessionId = sessionId,
                    frameId = 0,
                    generation = 1,
                    width = 2,
                    height = 2,
                    strideBytes = 8,
                    byteLen = 16,
                ),
                engine = "test-engine",
            )
        }

        override fun copy(frame: PreparedMicroscopeFrame): NativeFrameCopy {
            copyCalls += 1
            return NativeFrameCopy.Success(frame, ByteBuffer.allocateDirect(frame.byteLen).asReadOnlyBuffer())
        }
    }

    companion object {
        fun snapshot(
            sessionId: Long,
            frameId: Long,
        ): MicroscopeSessionSnapshot = MicroscopeSessionSnapshot(
            sessionId = sessionId,
            frameCount = 3,
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
            canStepNext = frameId < 2,
        )
    }
}
