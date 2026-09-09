package com.framescope.app.ui

import com.framescope.app.data.ScrubPerformanceTelemetry

/**
 * Bounds live-scrub work to one native request in flight plus one replaceable pending request.
 *
 * Newer submissions replace the single pending slot and make older results ineligible for
 * publication. Repeated submissions for the same session/target reuse existing pending or in-flight
 * work instead of creating redundant native decodes. Releasing the scrub gesture or replacing the
 * source invalidates the current epoch and returns the active request, if any, so callers can
 * identify disposable work. Publication fencing remains independent from native cancellation: a
 * late native result is never allowed to become visible even if cancellation is delayed or
 * unsupported.
 */
internal class LiveScrubRequestGate(
    private val nanoTime: () -> Long = { System.nanoTime() },
) {
    private var nextRequestId = 1L
    private var epoch = 1L
    private var latestRequestId = 0L
    private var inFlight: LiveScrubRequest? = null
    private var pending: LiveScrubRequest? = null

    @Synchronized
    fun submit(sessionId: Long, target: LiveScrubTarget): LiveScrubRequest {
        require(sessionId > 0L) { "Live scrub requires a positive microscope session id." }
        when (target) {
            is LiveScrubTarget.Frame -> require(target.frameId >= 0L) {
                "Live scrub frame id must be non-negative."
            }
            is LiveScrubTarget.Timestamp -> Unit
        }

        pending?.takeIf { it.sessionId == sessionId && it.target == target }?.let { existing ->
            return existing
        }
        inFlight?.takeIf { it.sessionId == sessionId && it.target == target }?.let { existing ->
            val replacedPending = pending != null
            pending = null
            latestRequestId = existing.requestId
            if (replacedPending) {
                ScrubPerformanceTelemetry.recordGateSubmission(replacedPending = true)
            }
            return existing
        }

        val request = LiveScrubRequest(
            requestId = nextRequestId,
            epoch = epoch,
            sessionId = sessionId,
            target = target,
            submittedAtNanos = nanoTime().coerceAtLeast(0L),
        )
        nextRequestId = increment(nextRequestId, "live scrub request id")
        latestRequestId = request.requestId
        val replacedPending = pending != null
        pending = request
        ScrubPerformanceTelemetry.recordGateSubmission(replacedPending)
        return request
    }

    /**
     * Returns the active native request that [latest] superseded, if any.
     *
     * The caller may use this identity to cancel disposable native work immediately. Publication
     * safety does not depend on cancellation succeeding; [finish] still fences stale results.
     */
    @Synchronized
    fun cancellationForSupersededInFlight(latest: LiveScrubRequest): LiveScrubCancellation? {
        val active = inFlight ?: return null
        if (active.requestId == latest.requestId) return null
        return LiveScrubCancellation(
            sessionId = active.sessionId,
            requestId = active.requestId,
        )
    }

    /** Returns work only when no native preview request is already running. */
    @Synchronized
    fun beginNext(): LiveScrubRequest? {
        if (inFlight != null) return null
        val request = pending ?: return null
        pending = null
        inFlight = request
        ScrubUxTelemetry.recordRequestStarted(
            requestId = request.requestId,
            sessionId = request.sessionId,
            submittedAtNanos = request.submittedAtNanos,
            startedAtNanos = nanoTime().coerceAtLeast(0L),
        )
        return request
    }

    /**
     * Marks [request] complete and reports whether its result is still the newest publishable one.
     */
    @Synchronized
    fun finish(request: LiveScrubRequest): Boolean {
        val finishedAtNanos = nanoTime().coerceAtLeast(0L)
        if (inFlight?.requestId != request.requestId) {
            ScrubPerformanceTelemetry.recordGateCompletion(publishable = false)
            ScrubUxTelemetry.recordRequestFinished(
                requestId = request.requestId,
                sessionId = request.sessionId,
                submittedAtNanos = request.submittedAtNanos,
                finishedAtNanos = finishedAtNanos,
                publishable = false,
            )
            return false
        }
        inFlight = null
        val publishable = request.epoch == epoch && request.requestId == latestRequestId
        ScrubPerformanceTelemetry.recordGateCompletion(publishable)
        ScrubUxTelemetry.recordRequestFinished(
            requestId = request.requestId,
            sessionId = request.sessionId,
            submittedAtNanos = request.submittedAtNanos,
            finishedAtNanos = finishedAtNanos,
            publishable = publishable,
        )
        return publishable
    }

    /**
     * Invalidates queued/current publication and identifies work that was in flight at invalidation.
     *
     * Native exact navigation independently installs an admission barrier before authoritative work.
     * That barrier and the native preview registry share one mutex, so exact-navigation priority does
     * not depend on this Kotlin-side invalidation racing successfully with preview registration.
     */
    @Synchronized
    fun invalidate(): LiveScrubCancellation? {
        val cancellation = inFlight?.let {
            LiveScrubCancellation(
                sessionId = it.sessionId,
                requestId = it.requestId,
            )
        }
        epoch = increment(epoch, "live scrub epoch")
        latestRequestId = 0L
        pending = null
        return cancellation
    }

    @Synchronized
    fun hasPendingWork(): Boolean = pending != null

    @Synchronized
    fun inFlightCount(): Int = if (inFlight == null) 0 else 1

    @Synchronized
    fun pendingCount(): Int = if (pending == null) 0 else 1

    private fun increment(value: Long, label: String): Long {
        check(value < Long.MAX_VALUE) { "$label space exhausted" }
        return value + 1L
    }
}

internal sealed interface LiveScrubTarget {
    data class Timestamp(
        val timestampUs: Long,
    ) : LiveScrubTarget

    data class Frame(
        val frameId: Long,
    ) : LiveScrubTarget
}

internal data class LiveScrubRequest(
    val requestId: Long,
    val epoch: Long,
    val sessionId: Long,
    val target: LiveScrubTarget,
    val submittedAtNanos: Long,
)

internal data class LiveScrubCancellation(
    val sessionId: Long,
    val requestId: Long,
)
