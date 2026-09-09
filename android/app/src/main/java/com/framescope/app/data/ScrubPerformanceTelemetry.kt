package com.framescope.app.data

import android.os.Trace
import android.util.Log
import java.util.concurrent.atomic.AtomicLong

/**
 * Non-authoritative live-scrub observability for physical-device performance runs.
 *
 * Metrics never participate in request admission, frame identity, timestamp selection, cache keys,
 * or cancellation decisions. Device runs can enable `FrameScopeScrub` DEBUG logging with adb and
 * collect the emitted per-request service lines alongside Perfetto's `FrameScope.scrub.render`
 * sections.
 */
internal object ScrubPerformanceTelemetry {
    private const val TAG = "FrameScopeScrub"
    private const val TRACE_RENDER = "FrameScope.scrub.render"

    private val sliderSubmissions = AtomicLong()
    private val pendingReplacements = AtomicLong()
    private val previewRequests = AtomicLong()
    private val previewSuccesses = AtomicLong()
    private val previewFailures = AtomicLong()
    private val previewCancellations = AtomicLong()
    private val staleCompletions = AtomicLong()
    private val previewRamHits = AtomicLong()
    private val sourceRamHits = AtomicLong()
    private val decodedRequests = AtomicLong()
    private val decodedFrames = AtomicLong()
    private val totalServiceUs = AtomicLong()
    private val maxServiceUs = AtomicLong()
    private val lastServiceUs = AtomicLong()

    fun recordGateSubmission(replacedPending: Boolean) {
        sliderSubmissions.incrementAndGet()
        if (replacedPending) pendingReplacements.incrementAndGet()
    }

    fun recordGateCompletion(publishable: Boolean) {
        if (!publishable) staleCompletions.incrementAndGet()
    }

    fun measureRender(operation: () -> NativeMicroscopePreview): NativeMicroscopePreview {
        previewRequests.incrementAndGet()
        val started = System.nanoTime()
        val traceStarted = runCatching {
            Trace.beginSection(TRACE_RENDER)
            true
        }.getOrDefault(false)
        val result = try {
            operation()
        } finally {
            if (traceStarted) {
                runCatching { Trace.endSection() }
            }
        }
        val serviceUs = ((System.nanoTime() - started).coerceAtLeast(0L)) / 1_000L
        recordResult(result, serviceUs)
        return result
    }

    fun snapshot(): ScrubPerformanceSnapshot = ScrubPerformanceSnapshot(
        sliderSubmissions = sliderSubmissions.get(),
        pendingReplacements = pendingReplacements.get(),
        previewRequests = previewRequests.get(),
        previewSuccesses = previewSuccesses.get(),
        previewFailures = previewFailures.get(),
        previewCancellations = previewCancellations.get(),
        staleCompletions = staleCompletions.get(),
        previewRamHits = previewRamHits.get(),
        sourceRamHits = sourceRamHits.get(),
        decodedRequests = decodedRequests.get(),
        decodedFrames = decodedFrames.get(),
        totalServiceUs = totalServiceUs.get(),
        maxServiceUs = maxServiceUs.get(),
        lastServiceUs = lastServiceUs.get(),
    )

    internal fun resetForTest() {
        listOf(
            sliderSubmissions,
            pendingReplacements,
            previewRequests,
            previewSuccesses,
            previewFailures,
            previewCancellations,
            staleCompletions,
            previewRamHits,
            sourceRamHits,
            decodedRequests,
            decodedFrames,
            totalServiceUs,
            maxServiceUs,
            lastServiceUs,
        ).forEach { it.set(0L) }
    }

    private fun recordResult(result: NativeMicroscopePreview, serviceUs: Long) {
        lastServiceUs.set(serviceUs)
        totalServiceUs.addAndGet(serviceUs)
        updateMax(maxServiceUs, serviceUs)

        val logLine = when (result) {
            is NativeMicroscopePreview.Success -> {
                previewSuccesses.incrementAndGet()
                val descriptor = result.preview.descriptor
                decodedFrames.addAndGet(descriptor.decodedFrames)
                when (descriptor.source) {
                    "preview_ram" -> previewRamHits.incrementAndGet()
                    "source_ram" -> sourceRamHits.incrementAndGet()
                    "decoded" -> decodedRequests.incrementAndGet()
                }
                "status=ok service_us=$serviceUs source=${descriptor.source} " +
                    "decoded_frames=${descriptor.decodedFrames} frame_id=${descriptor.frameId}"
            }
            is NativeMicroscopePreview.Failure -> {
                previewFailures.incrementAndGet()
                if (result.code == "cancelled") previewCancellations.incrementAndGet()
                "status=error service_us=$serviceUs code=${result.code}"
            }
        }
        runCatching {
            if (Log.isLoggable(TAG, Log.DEBUG)) {
                Log.d(TAG, logLine)
            }
        }
    }

    private fun updateMax(target: AtomicLong, candidate: Long) {
        var current = target.get()
        while (candidate > current && !target.compareAndSet(current, candidate)) {
            current = target.get()
        }
    }
}

internal data class ScrubPerformanceSnapshot(
    val sliderSubmissions: Long,
    val pendingReplacements: Long,
    val previewRequests: Long,
    val previewSuccesses: Long,
    val previewFailures: Long,
    val previewCancellations: Long,
    val staleCompletions: Long,
    val previewRamHits: Long,
    val sourceRamHits: Long,
    val decodedRequests: Long,
    val decodedFrames: Long,
    val totalServiceUs: Long,
    val maxServiceUs: Long,
    val lastServiceUs: Long,
) {
    val averageServiceUs: Long?
        get() = previewRequests.takeIf { it > 0L }?.let { totalServiceUs / it }
}
