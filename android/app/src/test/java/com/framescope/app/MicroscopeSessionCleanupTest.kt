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
import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MicroscopeSessionCleanupTest {
    @Test
    fun cleanupOfPublishedOldSessionDoesNotInvalidateNewerOpen() {
        val secondOpenEntered = CountDownLatch(1)
        val releaseSecondOpen = CountDownLatch(1)
        val native = FakeNativeBridge(secondOpenEntered, releaseSecondOpen)
        val controller = MicroscopeSessionController(native, UnusedFrameBridge())

        assertTrue(controller.open(fd = 11, operationId = 1, cacheRoot = "cache") is NativeMicroscope.Success)
        val secondResult = AtomicReference<NativeMicroscope>()
        val secondThread = Thread {
            secondResult.set(controller.open(fd = 12, operationId = 2, cacheRoot = "cache"))
        }
        secondThread.start()
        assertTrue(secondOpenEntered.await(2, TimeUnit.SECONDS))

        assertTrue(controller.closeIfCurrent(1L))
        assertEquals(null, controller.currentSnapshot())
        assertEquals(listOf(1L), native.closedSessionIds)

        releaseSecondOpen.countDown()
        secondThread.join(2_000)
        assertFalse(secondThread.isAlive)
        assertTrue(secondResult.get() is NativeMicroscope.Success)
        assertEquals(2L, controller.currentSnapshot()?.sessionId)
        assertEquals(listOf(1L), native.closedSessionIds)
    }

    private class FakeNativeBridge(
        private val secondOpenEntered: CountDownLatch,
        private val releaseSecondOpen: CountDownLatch,
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
            if (operationId == 2L) {
                secondOpenEntered.countDown()
                check(releaseSecondOpen.await(2, TimeUnit.SECONDS))
            }
            return NativeMicroscope.Success(snapshot(operationId), "test-engine")
        }

        override fun closeMicroscopeSession(sessionId: Long): Boolean {
            closedSessionIds.add(sessionId)
            return true
        }
    }

    private class UnusedFrameBridge : NativeMicroscopeFrameBridge {
        override fun prepare(sessionId: Long): NativeFramePreparation =
            NativeFramePreparation.Failure("unused", "unused", null)

        override fun copy(frame: PreparedMicroscopeFrame): NativeFrameCopy =
            NativeFrameCopy.Failure("unused", "unused")
    }

    private companion object {
        fun snapshot(sessionId: Long): MicroscopeSessionSnapshot = MicroscopeSessionSnapshot(
            sessionId = sessionId,
            frameCount = 1,
            currentFrame = FrameDetails(
                frameId = 0,
                timestampTicks = 0,
                timestampUs = 0,
                timeBaseNumerator = 1,
                timeBaseDenominator = 1_000,
                durationTicks = 40,
                keyframe = true,
                corrupt = false,
            ),
            canStepPrevious = false,
            canStepNext = false,
        )
    }
}
