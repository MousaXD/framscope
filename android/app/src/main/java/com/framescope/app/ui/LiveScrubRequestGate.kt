package com.framescope.app.ui

/**
 * Bounds live-scrub work to one native request in flight plus one replaceable pending request.
 *
 * Newer submissions replace the single pending slot and make older results ineligible for
 * publication. Releasing the scrub gesture or replacing the source invalidates the current epoch
 * and returns the active request, if any, so the native layer can cooperatively cancel the exact
 * obsolete request. Publication fencing remains independent from native cancellation: a late native
 * result is never allowed to become visible even if cancellation is delayed or unsupported.
 */
internal class LiveScrubRequestGate {
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
        val request = LiveScrubRequest(
            requestId = nextRequestId,
            epoch = epoch,
            sessionId = sessionId,
            target = target,
        )
        nextRequestId = increment(nextRequestId, "live scrub request id")
        latestRequestId = request.requestId
        pending = request
        return request
    }

    /** Returns work only when no native preview request is already running. */
    @Synchronized
    fun beginNext(): LiveScrubRequest? {
        if (inFlight != null) return null
        val request = pending ?: return null
        pending = null
        inFlight = request
        return request
    }

    /**
     * Marks [request] complete and reports whether its result is still the newest publishable one.
     */
    @Synchronized
    fun finish(request: LiveScrubRequest): Boolean {
        if (inFlight?.requestId != request.requestId) return false
        inFlight = null
        return request.epoch == epoch && request.requestId == latestRequestId
    }

    /**
     * Invalidates queued/current publication and identifies native work that should be preempted.
     *
     * The returned request is the only request that can currently be executing in native code. Its
     * monotonically increasing request id is also used by native cancellation to close the race
     * where invalidation happens just before that request registers its cancellation token.
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
)

internal data class LiveScrubCancellation(
    val sessionId: Long,
    val requestId: Long,
)
