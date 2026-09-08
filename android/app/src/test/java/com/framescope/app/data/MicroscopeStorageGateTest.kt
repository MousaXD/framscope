package com.framescope.app.data

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MicroscopeStorageGateTest {
    @Test
    fun activeSessionRejectsDestructiveStorageWork() {
        val gate = MicroscopeStorageGate()
        gate.beginSessionTransition()
        gate.endSessionTransition(hasActiveSession = true)
        var called = false

        val result = gate.runStorageIfIdle {
            called = true
            7
        }

        assertTrue(result is StorageGateResult.Busy)
        assertFalse(called)
    }

    @Test
    fun transitionInFlightRejectsDestructiveStorageWork() {
        val gate = MicroscopeStorageGate()
        gate.beginSessionTransition()

        val result = gate.runStorageIfIdle { 7 }

        assertTrue(result is StorageGateResult.Busy)
        gate.endSessionTransition(hasActiveSession = false)
        assertEquals(
            StorageGateResult.Available(8),
            gate.runStorageIfIdle { 8 },
        )
    }

    @Test
    fun newSessionTransitionWaitsForClearAlreadyInFlight() {
        val gate = MicroscopeStorageGate()
        val clearStarted = CountDownLatch(1)
        val releaseClear = CountDownLatch(1)
        val transitionFinished = CountDownLatch(1)

        val clearThread = Thread {
            gate.runStorageIfIdle {
                clearStarted.countDown()
                check(releaseClear.await(2, TimeUnit.SECONDS))
                1
            }
        }
        clearThread.start()
        assertTrue(clearStarted.await(2, TimeUnit.SECONDS))

        val transitionThread = Thread {
            gate.beginSessionTransition()
            transitionFinished.countDown()
            gate.endSessionTransition(hasActiveSession = false)
        }
        transitionThread.start()

        assertFalse(transitionFinished.await(100, TimeUnit.MILLISECONDS))
        releaseClear.countDown()
        assertTrue(transitionFinished.await(2, TimeUnit.SECONDS))
        clearThread.join(2_000)
        transitionThread.join(2_000)
        assertFalse(clearThread.isAlive)
        assertFalse(transitionThread.isAlive)
    }

    @Test
    fun failedStorageOperationAlwaysReleasesGate() {
        val gate = MicroscopeStorageGate()

        val failure = runCatching {
            gate.runStorageIfIdle<Int> { error("synthetic failure") }
        }

        assertTrue(failure.isFailure)
        assertEquals(
            StorageGateResult.Available(9),
            gate.runStorageIfIdle { 9 },
        )
    }
}
