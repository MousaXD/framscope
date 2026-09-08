package com.framescope.app.data

import java.util.concurrent.atomic.AtomicLong

/**
 * Owns the Android-side lifetime and ordering of one native microscope session.
 *
 * Native decode/index/frame work is deliberately performed outside [stateLock]. Every mutating
 * request receives a monotonically increasing revision; a result may update state only when that
 * revision is still current and the session it targeted is still the active session. This prevents
 * late JNI results from resurrecting a replaced/closed session or overwriting newer navigation.
 */
class MicroscopeSessionController(
    private val nativeBridge: NativeBridge,
    private val frameBridge: NativeMicroscopeFrameBridge,
) {
    private val stateLock = Any()
    private val revision = AtomicLong(0L)
    private val storageGate = MicroscopeStorageGate()

    private var snapshot: MicroscopeSessionSnapshot? = null
    private var engine: String? = null

    fun currentSnapshot(): MicroscopeSessionSnapshot? = synchronized(stateLock) { snapshot }

    fun currentEngine(): String? = synchronized(stateLock) { engine }

    internal fun <T> runStorageAdminIfIdle(operation: () -> T): StorageGateResult<T> =
        storageGate.runStorageIfIdle(operation)

    fun open(
        fd: Int,
        operationId: Long,
        cacheRoot: String,
    ): NativeMicroscope {
        storageGate.beginSessionTransition()
        try {
            val requestRevision = nextRevision() ?: return revisionExhaustedFailure(currentEngine())
            val result = nativeBridge.openMicroscopeSession(fd, operationId, cacheRoot)
            val success = when (result) {
                is NativeMicroscope.Failure -> {
                    return if (revision.get() == requestRevision) {
                        result
                    } else {
                        staleFailure(result.engine)
                    }
                }
                is NativeMicroscope.Success -> result
            }

            var previousSessionId: Long? = null
            val committed = synchronized(stateLock) {
                if (revision.get() != requestRevision) {
                    false
                } else {
                    previousSessionId = snapshot?.sessionId
                    snapshot = success.session
                    engine = success.engine
                    true
                }
            }

            if (!committed) {
                nativeBridge.closeMicroscopeSession(success.session.sessionId)
                return staleFailure(success.engine)
            }
            previousSessionId
                ?.takeIf { it != success.session.sessionId }
                ?.let(nativeBridge::closeMicroscopeSession)
            return success
        } finally {
            storageGate.endSessionTransition(currentSnapshot() != null)
        }
    }

    fun step(delta: Int): NativeMicroscope = navigate { sessionId ->
        nativeBridge.stepMicroscope(sessionId, delta)
    }

    fun jumpToFrame(frameId: Long): NativeMicroscope = navigate { sessionId ->
        nativeBridge.jumpMicroscopeFrame(sessionId, frameId)
    }

    fun jumpToTimestamp(
        timestampUs: Long,
        selection: TimestampSelectionPolicy,
    ): NativeMicroscope = navigate { sessionId ->
        nativeBridge.jumpMicroscopeTimestampUs(sessionId, timestampUs, selection)
    }

    fun loadCurrentFrame(): NativeFrameCopy {
        val target = synchronized(stateLock) { snapshot }
            ?: return noSessionFrameFailure()
        val requestRevision = revision.get()
        val expectedFrameId = target.currentFrame?.frameId
            ?: return NativeFrameCopy.Failure(
                code = "no_frames",
                message = "The current microscope session contains no indexed frames.",
            )

        val prepared = frameBridge.prepare(target.sessionId)
        if (prepared is NativeFramePreparation.Failure) {
            return if (isStillCurrent(requestRevision, target.sessionId, expectedFrameId)) {
                NativeFrameCopy.Failure(
                    code = prepared.code,
                    message = prepared.message,
                )
            } else {
                staleFrameFailure()
            }
        }
        if (prepared !is NativeFramePreparation.Success) {
            return if (isStillCurrent(requestRevision, target.sessionId, expectedFrameId)) {
                NativeFrameCopy.Failure(
                    code = "bridge_error",
                    message = "Native frame preparation returned an unsupported result.",
                )
            } else {
                staleFrameFailure()
            }
        }
        if (prepared.frame.sessionId != target.sessionId || prepared.frame.frameId != expectedFrameId) {
            return if (isStillCurrent(requestRevision, target.sessionId, expectedFrameId)) {
                presentationIdentityFailure()
            } else {
                staleFrameFailure()
            }
        }
        if (!isStillCurrent(requestRevision, target.sessionId, expectedFrameId)) {
            return staleFrameFailure()
        }

        val copied = frameBridge.copy(prepared.frame)
        if (copied is NativeFrameCopy.Failure) {
            return if (isStillCurrent(requestRevision, target.sessionId, expectedFrameId)) {
                copied
            } else {
                staleFrameFailure()
            }
        }
        if (copied !is NativeFrameCopy.Success) {
            return if (isStillCurrent(requestRevision, target.sessionId, expectedFrameId)) {
                NativeFrameCopy.Failure(
                    code = "bridge_error",
                    message = "Native frame copy returned an unsupported result.",
                )
            } else {
                staleFrameFailure()
            }
        }
        if (copied.frame != prepared.frame) {
            return if (isStillCurrent(requestRevision, target.sessionId, expectedFrameId)) {
                presentationIdentityFailure()
            } else {
                staleFrameFailure()
            }
        }
        return if (isStillCurrent(requestRevision, target.sessionId, expectedFrameId)) {
            copied
        } else {
            staleFrameFailure()
        }
    }

    fun exportCurrentFrame(
        outputFd: Int,
        operationId: Long,
        format: FrameExportFormat,
        jpegQuality: Int,
    ): NativeFrameExport {
        val target = synchronized(stateLock) { snapshot }
            ?: return noSessionExportFailure()
        val requestRevision = revision.get()
        val expectedFrameId = target.currentFrame?.frameId
            ?: return NativeFrameExport.Failure(
                code = "no_frames",
                message = "The current microscope session contains no indexed frames.",
                engine = currentEngine(),
            )

        val result = nativeBridge.exportCurrentMicroscopeFrame(
            sessionId = target.sessionId,
            outputFd = outputFd,
            operationId = operationId,
            format = format,
            jpegQuality = jpegQuality,
        )
        if (!isStillCurrent(requestRevision, target.sessionId, expectedFrameId)) {
            return staleExportFailure(result.engineOrNull())
        }
        return when (result) {
            is NativeFrameExport.Failure -> result
            is NativeFrameExport.Success -> {
                if (
                    result.export.sessionId != target.sessionId ||
                    result.export.frameId != expectedFrameId ||
                    result.export.format != format
                ) {
                    exportIdentityFailure(result.engine)
                } else {
                    result
                }
            }
        }
    }

    fun closeCurrent(): Boolean {
        storageGate.beginSessionTransition()
        try {
            invalidateRevision()
            val sessionId = synchronized(stateLock) {
                val value = snapshot?.sessionId
                snapshot = null
                engine = null
                value
            } ?: return true
            return nativeBridge.closeMicroscopeSession(sessionId)
        } finally {
            storageGate.endSessionTransition(currentSnapshot() != null)
        }
    }

    /**
     * Detach and close [sessionId] only when it is still the active session.
     *
     * Unlike [closeCurrent], this intentionally does not advance the global revision. It is used to
     * clean up a successfully opened session whose coroutine was cancelled before publication. A
     * newer open may already be in flight, and cancellation cleanup for the older request must not
     * invalidate or close that replacement.
     */
    fun closeIfCurrent(sessionId: Long): Boolean {
        storageGate.beginSessionTransition()
        try {
            if (sessionId <= 0L) return false
            val detached = synchronized(stateLock) {
                if (snapshot?.sessionId != sessionId) {
                    false
                } else {
                    snapshot = null
                    engine = null
                    true
                }
            }
            return if (detached) nativeBridge.closeMicroscopeSession(sessionId) else true
        } finally {
            storageGate.endSessionTransition(currentSnapshot() != null)
        }
    }

    private fun navigate(call: (Long) -> NativeMicroscope): NativeMicroscope {
        val target = synchronized(stateLock) { snapshot }
            ?: return noSessionFailure()
        val requestRevision = nextRevision() ?: return revisionExhaustedFailure(currentEngine())
        val result = call(target.sessionId)
        val success = when (result) {
            is NativeMicroscope.Failure -> {
                return if (isRequestCurrent(requestRevision, target.sessionId)) {
                    result
                } else {
                    staleFailure(result.engine)
                }
            }
            is NativeMicroscope.Success -> result
        }
        if (success.session.sessionId != target.sessionId) {
            return if (isRequestCurrent(requestRevision, target.sessionId)) {
                sessionIdentityFailure(success.engine)
            } else {
                staleFailure(success.engine)
            }
        }

        val committed = synchronized(stateLock) {
            if (
                revision.get() != requestRevision ||
                snapshot?.sessionId != target.sessionId
            ) {
                false
            } else {
                snapshot = success.session
                engine = success.engine
                true
            }
        }
        return if (committed) success else staleFailure(success.engine)
    }

    private fun nextRevision(): Long? {
        while (true) {
            val current = revision.get()
            if (current < 0L || current == Long.MAX_VALUE) return null
            val next = current + 1L
            if (revision.compareAndSet(current, next)) return next
        }
    }

    /**
     * Invalidate in-flight work. If the monotonic counter is exhausted, move it to a terminal
     * negative sentinel. No later request can receive a revision after that point.
     */
    private fun invalidateRevision() {
        while (true) {
            val current = revision.get()
            if (current < 0L) return
            val next = if (current == Long.MAX_VALUE) TERMINAL_REVISION else current + 1L
            if (revision.compareAndSet(current, next)) return
        }
    }

    private fun isRequestCurrent(
        requestRevision: Long,
        sessionId: Long,
    ): Boolean = synchronized(stateLock) {
        revision.get() == requestRevision && snapshot?.sessionId == sessionId
    }

    private fun isStillCurrent(
        requestRevision: Long,
        sessionId: Long,
        frameId: Long,
    ): Boolean = synchronized(stateLock) {
        revision.get() == requestRevision &&
            snapshot?.sessionId == sessionId &&
            snapshot?.currentFrame?.frameId == frameId
    }

    private fun noSessionFailure(): NativeMicroscope.Failure = NativeMicroscope.Failure(
        code = "session_not_found",
        message = "No microscope session is currently open.",
        engine = currentEngine(),
    )

    private fun staleFailure(engine: String?): NativeMicroscope.Failure = NativeMicroscope.Failure(
        code = "stale_result",
        message = "A newer microscope request superseded this native result.",
        engine = engine,
    )

    private fun revisionExhaustedFailure(engine: String?): NativeMicroscope.Failure =
        NativeMicroscope.Failure(
            code = "request_revision_exhausted",
            message = "Microscope request ordering space is exhausted for this controller.",
            engine = engine,
        )

    private fun sessionIdentityFailure(engine: String?): NativeMicroscope.Failure =
        NativeMicroscope.Failure(
            code = "session_identity_mismatch",
            message = "Native navigation returned a different microscope session.",
            engine = engine,
        )

    private fun noSessionFrameFailure(): NativeFrameCopy.Failure = NativeFrameCopy.Failure(
        code = "session_not_found",
        message = "No microscope session is currently open.",
    )

    private fun staleFrameFailure(): NativeFrameCopy.Failure = NativeFrameCopy.Failure(
        code = "stale_result",
        message = "A newer microscope request superseded this frame result.",
    )

    private fun presentationIdentityFailure(): NativeFrameCopy.Failure = NativeFrameCopy.Failure(
        code = "presentation_identity_mismatch",
        message = "Prepared or copied RGBA metadata does not match the authoritative microscope target.",
    )

    private fun noSessionExportFailure(): NativeFrameExport.Failure = NativeFrameExport.Failure(
        code = "session_not_found",
        message = "No microscope session is currently open.",
        engine = currentEngine(),
    )

    private fun staleExportFailure(engine: String?): NativeFrameExport.Failure = NativeFrameExport.Failure(
        code = "stale_result",
        message = "A newer microscope request superseded this frame export result.",
        engine = engine,
    )

    private fun exportIdentityFailure(engine: String?): NativeFrameExport.Failure = NativeFrameExport.Failure(
        code = "export_identity_mismatch",
        message = "Native export metadata does not match the authoritative microscope target.",
        engine = engine,
    )

    private fun NativeFrameExport.engineOrNull(): String? = when (this) {
        is NativeFrameExport.Success -> engine
        is NativeFrameExport.Failure -> engine
    }

    private companion object {
        const val TERMINAL_REVISION = Long.MIN_VALUE
    }
}
