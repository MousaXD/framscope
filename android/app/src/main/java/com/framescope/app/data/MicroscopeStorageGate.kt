package com.framescope.app.data

import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

internal sealed interface StorageGateResult<out T> {
    data class Available<T>(val value: T) : StorageGateResult<T>
    data object Busy : StorageGateResult<Nothing>
}

/**
 * Serializes destructive FrameScope storage work against microscope session open/close transitions.
 *
 * A clear operation is allowed only when there is no active session and no session transition in
 * flight. Conversely, a new session transition waits for a clear that already owns the gate. This
 * closes the otherwise dangerous window where Android observes no session while the native open or
 * close call still owns an SQLite frame index.
 */
internal class MicroscopeStorageGate {
    private val lock = ReentrantLock()
    private val storageFinished = lock.newCondition()

    private var sessionTransitions = 0
    private var activeSession = false
    private var storageOperationActive = false

    fun beginSessionTransition() {
        lock.withLock {
            while (storageOperationActive) {
                storageFinished.awaitUninterruptibly()
            }
            sessionTransitions += 1
        }
    }

    fun endSessionTransition(hasActiveSession: Boolean) {
        lock.withLock {
            check(sessionTransitions > 0) { "unbalanced microscope storage transition" }
            sessionTransitions -= 1
            activeSession = hasActiveSession
            storageFinished.signalAll()
        }
    }

    fun <T> runStorageIfIdle(operation: () -> T): StorageGateResult<T> {
        lock.withLock {
            if (activeSession || sessionTransitions > 0 || storageOperationActive) {
                return StorageGateResult.Busy
            }
            storageOperationActive = true
        }

        return try {
            StorageGateResult.Available(operation())
        } finally {
            lock.withLock {
                storageOperationActive = false
                storageFinished.signalAll()
            }
        }
    }
}
