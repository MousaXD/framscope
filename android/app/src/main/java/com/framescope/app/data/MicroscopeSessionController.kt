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

    private var snapshot: MicroscopeSessionSnapshot? = null
    private var engine: String? = null

    fun currentSnapshot(): MicroscopeSessionSnapshot? = synchronized(stateLock) { snapshot }

    fun currentEngine(): String? = synchronized(stateLock) { engine }

    fun open(
        fd: Int,
        operationId: Long,
        cacheRoot: String,
    ): NativeMicroscope {
        val requestRevision = revision.incrementAndGet()
        val result = nativeBridge.openMicroscopeSession(fd, operationId, cacheRoot)
        if (result !is NativeMicroscope.Success) return result

        var previousSessionId: Long? = null
        val committed = synchronized(stateLock) {
            if (revision.get() != requestRevision) {
                false
            } else {
                previousSessionId = snapshot?.sessionId
                snapshot = result.session
                engine = result.engine
                true
            }
        }

        if (!committed) {
            nativeBridge.closeMicroscopeSession(result.session.sessionId)
            return staleFailure(result.engine)
        }
        previousSessionId
            ?.takeIf { it != result.session.sessionId }
            ?.let(nativeBridge::closeMicroscopeSession)
        return result
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
            return NativeFrameCopy.Failure(
                code = prepared.code,
                message = prepared.message,
            )
        }
        if (prepared !is NativeFramePreparation.Success) {
            return NativeFrameCopy.Failure(
                code = "bridge_error",
                message = "Native frame preparation returned an unsupported result.",
            )
        }
        if (prepared.frame.sessionId != target.sessionId || prepared.frame.frameId != expectedFrameId) {
            return NativeFrameCopy.Failure(
                code = "stale_result",
                message = "Prepared frame no longer matches the requested microscope position.",
            )
        }
        if (!isStillCurrent(requestRevision, target.sessionId, expectedFrameId)) {
            return staleFrameFailure()
        }

        val copied = frameBridge.copy(prepared.frame)
        if (copied !is NativeFrameCopy.Success) return copied
        return if (isStillCurrent(requestRevision, target.sessionId, expectedFrameId)) {
            copied
        } else {
            staleFrameFailure()
        }
    }

    fun closeCurrent(): Boolean {
        revision.incrementAndGet()
        val sessionId = synchronized(stateLock) {
            val value = snapshot?.sessionId
            snapshot = null
            engine = null
            value
        } ?: return true
        return nativeBridge.closeMicroscopeSession(sessionId)
    }

    private fun navigate(call: (Long) -> NativeMicroscope): NativeMicroscope {
        val target = synchronized(stateLock) { snapshot }
            ?: return noSessionFailure()
        val requestRevision = revision.incrementAndGet()
        val result = call(target.sessionId)
        if (result !is NativeMicroscope.Success) return result

        val committed = synchronized(stateLock) {
            if (
                revision.get() != requestRevision ||
                snapshot?.sessionId != target.sessionId ||
                result.session.sessionId != target.sessionId
            ) {
                false
            } else {
                snapshot = result.session
                engine = result.engine
                true
            }
        }
        return if (committed) result else staleFailure(result.engine)
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

    private fun noSessionFrameFailure(): NativeFrameCopy.Failure = NativeFrameCopy.Failure(
        code = "session_not_found",
        message = "No microscope session is currently open.",
    )

    private fun staleFrameFailure(): NativeFrameCopy.Failure = NativeFrameCopy.Failure(
        code = "stale_result",
        message = "A newer microscope request superseded this frame result.",
    )
}
