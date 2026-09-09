package com.framescope.app.ui

import android.os.Build
import android.os.Trace
import android.util.Log
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * UI-side scrub telemetry that never participates in correctness or admission decisions.
 *
 * The native scrub telemetry measures decoder/cache service. This companion surface measures the
 * pieces only the UI can see: pointer-to-thumb draw latency, admission delay, request queue age,
 * target age when its bitmap is actually drawn, stale-result drops, and authoritative finger-up
 * settle-to-draw latency. Agent 12 can correlate these counters with the existing
 * `FrameScope.scrub.render` Perfetto slices.
 */
internal object ScrubUxTelemetry {
    private const val TAG = "FrameScopeScrubUX"

    private val pointerToThumbSamples = AtomicLong()
    private val pointerToThumbTotalUs = AtomicLong()
    private val pointerToThumbMaxUs = AtomicLong()
    private val pointerToThumbLastUs = AtomicLong()

    private val admissionDelaySamples = AtomicLong()
    private val admissionDelayTotalUs = AtomicLong()
    private val admissionDelayMaxUs = AtomicLong()
    private val admissionDelayLastUs = AtomicLong()

    private val requestStarts = AtomicLong()
    private val requestFinishes = AtomicLong()
    private val staleResultDrops = AtomicLong()
    private val targetAgeSamples = AtomicLong()
    private val targetAgeTotalUs = AtomicLong()
    private val targetAgeMaxUs = AtomicLong()
    private val targetAgeLastUs = AtomicLong()
    private val pendingPreviewPresentation = AtomicReference<PendingPreviewPresentation?>(null)

    private val exactSettleSamples = AtomicLong()
    private val exactSettleTotalUs = AtomicLong()
    private val exactSettleMaxUs = AtomicLong()
    private val exactSettleLastUs = AtomicLong()
    private val pendingExactSettle = AtomicReference<PendingExactSettle?>(null)

    fun recordPointerToThumb(
        pointerAtNanos: Long,
        drawnAtNanos: Long,
    ) {
        elapsedUs(pointerAtNanos, drawnAtNanos)?.let { latencyUs ->
            pointerToThumbSamples.incrementAndGet()
            pointerToThumbTotalUs.addAndGet(latencyUs)
            pointerToThumbLastUs.set(latencyUs)
            updateMax(pointerToThumbMaxUs, latencyUs)
            traceCounter("FrameScope.scrub.pointer_to_thumb_us", latencyUs)
        }
    }

    fun recordAdmissionDelay(
        observedAtNanos: Long,
        admittedAtNanos: Long,
    ) {
        elapsedUs(observedAtNanos, admittedAtNanos)?.let { delayUs ->
            admissionDelaySamples.incrementAndGet()
            admissionDelayTotalUs.addAndGet(delayUs)
            admissionDelayLastUs.set(delayUs)
            updateMax(admissionDelayMaxUs, delayUs)
            traceCounter("FrameScope.scrub.admission_delay_us", delayUs)
        }
    }

    fun recordRequestStarted(
        requestId: Long,
        sessionId: Long,
        submittedAtNanos: Long,
        startedAtNanos: Long,
    ) {
        requestStarts.incrementAndGet()
        // A newer request makes an older not-yet-drawn preview irrelevant for age measurement.
        pendingPreviewPresentation.get()?.takeIf { it.sessionId == sessionId }?.let { pending ->
            pendingPreviewPresentation.compareAndSet(pending, null)
        }
        val queueUs = elapsedUs(submittedAtNanos, startedAtNanos) ?: 0L
        traceCounter("FrameScope.scrub.request_queue_us", queueUs)
        debugLog("request_start id=$requestId session_id=$sessionId queue_us=$queueUs")
    }

    fun recordRequestFinished(
        requestId: Long,
        sessionId: Long,
        submittedAtNanos: Long,
        finishedAtNanos: Long,
        publishable: Boolean,
    ) {
        requestFinishes.incrementAndGet()
        val serviceAgeUs = elapsedUs(submittedAtNanos, finishedAtNanos) ?: 0L
        if (publishable) {
            pendingPreviewPresentation.set(
                PendingPreviewPresentation(
                    requestId = requestId,
                    sessionId = sessionId,
                    submittedAtNanos = submittedAtNanos,
                ),
            )
            traceCounter("FrameScope.scrub.publishable_request_age_us", serviceAgeUs)
        } else {
            staleResultDrops.incrementAndGet()
            traceCounter("FrameScope.scrub.stale_result_drops", staleResultDrops.get())
        }
        debugLog(
            "request_end id=$requestId session_id=$sessionId publishable=$publishable " +
                "request_age_us=$serviceAgeUs",
        )
    }

    /** Completes target-age timing only when the corresponding live image reaches the draw phase. */
    fun recordPreviewPresented(
        sessionId: Long,
        presentedAtNanos: Long,
    ) {
        if (sessionId <= 0L) return
        while (true) {
            val pending = pendingPreviewPresentation.get() ?: return
            if (pending.sessionId != sessionId) return
            if (!pendingPreviewPresentation.compareAndSet(pending, null)) continue
            elapsedUs(pending.submittedAtNanos, presentedAtNanos)?.let { ageUs ->
                targetAgeSamples.incrementAndGet()
                targetAgeTotalUs.addAndGet(ageUs)
                targetAgeLastUs.set(ageUs)
                updateMax(targetAgeMaxUs, ageUs)
                traceCounter("FrameScope.scrub.target_age_when_presented_us", ageUs)
                debugLog(
                    "preview_presented id=${pending.requestId} session_id=$sessionId " +
                        "target_age_us=$ageUs",
                )
            }
            return
        }
    }

    fun beginExactSettle(
        sessionId: Long,
        startedAtNanos: Long = System.nanoTime(),
    ) {
        if (sessionId <= 0L || startedAtNanos < 0L) return
        pendingExactSettle.set(PendingExactSettle(sessionId, startedAtNanos))
    }

    /** Completes finger-up timing only when the new authoritative image reaches the draw phase. */
    fun completeExactSettle(
        sessionId: Long,
        completedAtNanos: Long = System.nanoTime(),
    ) {
        if (sessionId <= 0L) return
        while (true) {
            val pending = pendingExactSettle.get() ?: return
            if (pending.sessionId != sessionId) return
            if (!pendingExactSettle.compareAndSet(pending, null)) continue
            elapsedUs(pending.startedAtNanos, completedAtNanos)?.let { latencyUs ->
                exactSettleSamples.incrementAndGet()
                exactSettleTotalUs.addAndGet(latencyUs)
                exactSettleLastUs.set(latencyUs)
                updateMax(exactSettleMaxUs, latencyUs)
                traceCounter("FrameScope.scrub.exact_settle_to_draw_us", latencyUs)
                debugLog("exact_settle session_id=$sessionId latency_us=$latencyUs")
            }
            return
        }
    }

    fun cancelExactSettle(sessionId: Long) {
        while (true) {
            val pending = pendingExactSettle.get() ?: return
            if (pending.sessionId != sessionId) return
            if (pendingExactSettle.compareAndSet(pending, null)) return
        }
    }

    fun snapshot(): ScrubUxTelemetrySnapshot = ScrubUxTelemetrySnapshot(
        pointerToThumbSamples = pointerToThumbSamples.get(),
        pointerToThumbTotalUs = pointerToThumbTotalUs.get(),
        pointerToThumbMaxUs = pointerToThumbMaxUs.get(),
        pointerToThumbLastUs = pointerToThumbLastUs.get(),
        admissionDelaySamples = admissionDelaySamples.get(),
        admissionDelayTotalUs = admissionDelayTotalUs.get(),
        admissionDelayMaxUs = admissionDelayMaxUs.get(),
        admissionDelayLastUs = admissionDelayLastUs.get(),
        requestStarts = requestStarts.get(),
        requestFinishes = requestFinishes.get(),
        staleResultDrops = staleResultDrops.get(),
        targetAgeSamples = targetAgeSamples.get(),
        targetAgeTotalUs = targetAgeTotalUs.get(),
        targetAgeMaxUs = targetAgeMaxUs.get(),
        targetAgeLastUs = targetAgeLastUs.get(),
        exactSettleSamples = exactSettleSamples.get(),
        exactSettleTotalUs = exactSettleTotalUs.get(),
        exactSettleMaxUs = exactSettleMaxUs.get(),
        exactSettleLastUs = exactSettleLastUs.get(),
    )

    internal fun resetForTest() {
        listOf(
            pointerToThumbSamples,
            pointerToThumbTotalUs,
            pointerToThumbMaxUs,
            pointerToThumbLastUs,
            admissionDelaySamples,
            admissionDelayTotalUs,
            admissionDelayMaxUs,
            admissionDelayLastUs,
            requestStarts,
            requestFinishes,
            staleResultDrops,
            targetAgeSamples,
            targetAgeTotalUs,
            targetAgeMaxUs,
            targetAgeLastUs,
            exactSettleSamples,
            exactSettleTotalUs,
            exactSettleMaxUs,
            exactSettleLastUs,
        ).forEach { it.set(0L) }
        pendingPreviewPresentation.set(null)
        pendingExactSettle.set(null)
    }

    private fun traceCounter(name: String, value: Long) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && Trace.isEnabled()) {
            Trace.setCounter(name, value)
        }
    }

    private fun debugLog(message: String) {
        runCatching {
            if (Log.isLoggable(TAG, Log.DEBUG)) Log.d(TAG, message)
        }
    }

    private fun elapsedUs(startedAtNanos: Long, completedAtNanos: Long): Long? {
        if (startedAtNanos < 0L || completedAtNanos < startedAtNanos) return null
        return (completedAtNanos - startedAtNanos) / 1_000L
    }

    private fun updateMax(target: AtomicLong, candidate: Long) {
        var current = target.get()
        while (candidate > current && !target.compareAndSet(current, candidate)) {
            current = target.get()
        }
    }

    private data class PendingPreviewPresentation(
        val requestId: Long,
        val sessionId: Long,
        val submittedAtNanos: Long,
    )

    private data class PendingExactSettle(
        val sessionId: Long,
        val startedAtNanos: Long,
    )
}

internal data class ScrubUxTelemetrySnapshot(
    val pointerToThumbSamples: Long,
    val pointerToThumbTotalUs: Long,
    val pointerToThumbMaxUs: Long,
    val pointerToThumbLastUs: Long,
    val admissionDelaySamples: Long,
    val admissionDelayTotalUs: Long,
    val admissionDelayMaxUs: Long,
    val admissionDelayLastUs: Long,
    val requestStarts: Long,
    val requestFinishes: Long,
    val staleResultDrops: Long,
    val targetAgeSamples: Long,
    val targetAgeTotalUs: Long,
    val targetAgeMaxUs: Long,
    val targetAgeLastUs: Long,
    val exactSettleSamples: Long,
    val exactSettleTotalUs: Long,
    val exactSettleMaxUs: Long,
    val exactSettleLastUs: Long,
) {
    val averagePointerToThumbUs: Long?
        get() = pointerToThumbSamples.takeIf { it > 0L }?.let { pointerToThumbTotalUs / it }

    val averageAdmissionDelayUs: Long?
        get() = admissionDelaySamples.takeIf { it > 0L }?.let { admissionDelayTotalUs / it }

    val averageTargetAgeUs: Long?
        get() = targetAgeSamples.takeIf { it > 0L }?.let { targetAgeTotalUs / it }

    val averageExactSettleUs: Long?
        get() = exactSettleSamples.takeIf { it > 0L }?.let { exactSettleTotalUs / it }
}

/** Records only the newest pointer sample; older undrawn samples are intentionally superseded. */
internal class ScrubThumbDrawTracker {
    private var pendingPointerAtNanos: Long? = null

    fun onPointerInput(pointerAtNanos: Long) {
        if (pointerAtNanos >= 0L) pendingPointerAtNanos = pointerAtNanos
    }

    fun onDrawn(drawnAtNanos: Long) {
        val pointerAt = pendingPointerAtNanos ?: return
        pendingPointerAtNanos = null
        ScrubUxTelemetry.recordPointerToThumb(pointerAt, drawnAtNanos)
    }
}

/** One replaceable delayed admission; disposal/release cancels it synchronously at the Job level. */
internal class DelayedScrubPreviewAdmission {
    private var pending: Job? = null

    fun replace(
        scope: CoroutineScope,
        delayMs: Long,
        action: () -> Unit,
    ) {
        cancel()
        val scheduled = scope.launch {
            delay(delayMs.coerceAtLeast(1L))
            action()
        }
        pending = scheduled
        scheduled.invokeOnCompletion {
            if (pending === scheduled) pending = null
        }
    }

    fun cancel() {
        pending?.cancel()
        pending = null
    }
}
