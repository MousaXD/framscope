package com.framescope.app.performance

import android.annotation.TargetApi
import android.app.ActivityManager
import android.content.Context
import android.os.Build
import android.os.Debug
import android.os.PerformanceHintManager
import android.os.PowerManager
import android.os.Process
import android.os.SystemClock
import android.os.Trace
import android.util.Log
import com.framescope.app.data.MicroscopeIndexingProgress
import com.framescope.app.data.MicroscopeIndexingStage
import java.io.Closeable
import java.util.Locale
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference

/**
 * App-level Android performance coordination for long native indexing operations.
 *
 * This intentionally uses only ordinary Android application APIs. It never changes governors,
 * cpusets, thermal controls, kernel knobs, or other device-wide policy. Performance hints are
 * calibrated from genuine native progress cycles rather than from the Kotlin polling cadence.
 */
object FrameScopePerformanceRuntime {
    private val sessions = ConcurrentHashMap<Long, ActiveIndexingSession>()
    private val thermalStatus = AtomicInteger(THERMAL_STATUS_UNKNOWN)
    private val trimMemoryLevel = AtomicInteger(0)
    private val latest = AtomicReference<AndroidPerformanceSnapshot?>(null)
    private val thermalMonitorInitialized = AtomicBoolean(false)
    private val thermalHeadroomLock = Any()

    @Volatile
    private var appContext: Context? = null

    @Volatile
    private var performanceMode: FrameScopePerformanceMode =
        FrameScopePerformanceMode.SustainedThroughput

    private var thermalMonitor: Closeable? = null
    private var lastThermalHeadroomSampleElapsedMs = Long.MIN_VALUE
    private var cachedThermalHeadroom: Float? = null

    fun initialize(context: Context) {
        val applicationContext = context.applicationContext
        appContext = applicationContext
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return
        if (!thermalMonitorInitialized.compareAndSet(false, true)) return

        thermalMonitor = runCatching {
            Api29ThermalMonitor(applicationContext) { status ->
                thermalStatus.set(status)
                sessions.values.forEach { session -> session.onThermalStatus(status) }
            }
        }.onFailure { error ->
            thermalMonitorInitialized.set(false)
            Log.w(TAG, "Thermal telemetry listener unavailable: ${error.message}")
        }.getOrNull()
    }

    fun setPerformanceMode(mode: FrameScopePerformanceMode) {
        performanceMode = mode
    }

    fun currentPerformanceMode(): FrameScopePerformanceMode = performanceMode

    fun latestSnapshot(): AndroidPerformanceSnapshot? = latest.get()

    fun beginIndexing(operationId: Long): Closeable {
        if (operationId <= 0L) return Closeable {}

        val ownerTid = runCatching { Process.myTid() }.getOrDefault(INVALID_TID)
        val originalPriority = if (ownerTid > 0) {
            runCatching { Process.getThreadPriority(ownerTid) }.getOrNull()
        } else {
            null
        }
        val priorityApplied =
            performanceMode == FrameScopePerformanceMode.SustainedThroughput &&
                originalPriority != null &&
                runCatching {
                    // Foreground is deliberate. THREAD_PRIORITY_VIDEO is for playback-oriented video
                    // work and is unnecessarily aggressive for a long indexing coordinator.
                    Process.setThreadPriority(Process.THREAD_PRIORITY_FOREGROUND)
                }.isSuccess
        val traceStarted = runCatching {
            Trace.beginSection(INDEXING_TRACE_SECTION)
            true
        }.getOrDefault(false)

        val session = ActiveIndexingSession(
            operationId = operationId,
            ownerTid = ownerTid,
            originalPriority = originalPriority,
            priorityApplied = priorityApplied,
            traceStarted = traceStarted,
            mode = performanceMode,
            context = appContext,
            initialThermalStatus = thermalStatus.get(),
            trimMemoryLevel = { trimMemoryLevel.get() },
            sampleThermalHeadroom = ::sampleThermalHeadroom,
            publishSnapshot = { snapshot -> latest.set(snapshot) },
            unregister = { id, active -> sessions.remove(id, active) },
        )
        sessions.put(operationId, session)?.close()
        return session
    }

    fun onIndexingProgress(progress: MicroscopeIndexingProgress) {
        if (!progress.telemetryAvailable) return
        sessions[progress.operationId]?.onProgress(progress)
    }

    fun onTrimMemory(level: Int) {
        trimMemoryLevel.set(level)
    }

    fun onLowMemory() {
        trimMemoryLevel.set(LOW_MEMORY_SENTINEL)
    }

    private fun sampleThermalHeadroom(context: Context?, nowElapsedMs: Long): Float? {
        if (context == null || Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return null
        synchronized(thermalHeadroomLock) {
            if (
                lastThermalHeadroomSampleElapsedMs != Long.MIN_VALUE &&
                nowElapsedMs - lastThermalHeadroomSampleElapsedMs < THERMAL_HEADROOM_POLL_MS
            ) {
                return cachedThermalHeadroom
            }
            lastThermalHeadroomSampleElapsedMs = nowElapsedMs
            cachedThermalHeadroom = runCatching {
                Api30ThermalHeadroom.read(context, THERMAL_HEADROOM_FORECAST_SECONDS)
                    ?.takeIf { value -> value.isFinite() }
            }.getOrNull()
            return cachedThermalHeadroom
        }
    }
}

private class ActiveIndexingSession(
    private val operationId: Long,
    private val ownerTid: Int,
    private val originalPriority: Int?,
    private val priorityApplied: Boolean,
    private val traceStarted: Boolean,
    private val mode: FrameScopePerformanceMode,
    private val context: Context?,
    initialThermalStatus: Int,
    private val trimMemoryLevel: () -> Int,
    private val sampleThermalHeadroom: (Context?, Long) -> Float?,
    private val publishSnapshot: (AndroidPerformanceSnapshot) -> Unit,
    private val unregister: (Long, ActiveIndexingSession) -> Unit,
) : Closeable {
    private val closed = AtomicBoolean(false)
    private val cycleTracker = IndexingWorkCycleTracker()
    private val hintLock = Any()
    private var hintController: AdpfController? = null
    private var currentThermalStatus = initialThermalStatus
    private var lastDiagnosticsElapsedMs = Long.MIN_VALUE
    private var lastDiagnosticsWorkUnits: Long? = null
    private var lastDiagnosticsSampleElapsedMs: Long? = null
    private var lastCpuSampleElapsedNanos = safeElapsedRealtimeNanos()
    private var lastCpuSampleProcessMs = safeProcessCpuTimeMs()

    fun onProgress(progress: MicroscopeIndexingProgress) {
        if (closed.get() || progress.operationId != operationId) return
        publishTraceCounters(progress)

        progress.asSchedulingSample()?.let { sample ->
            cycleTracker.observe(sample)?.takeIf { duration -> duration > 0L }?.let { duration ->
                reportAdpfCycle(duration, progress.sampleElapsedMs)
            }
        }
        maybePublishDiagnostics(progress)
    }

    fun onThermalStatus(status: Int) {
        currentThermalStatus = status
        synchronized(hintLock) {
            if (status >= THERMAL_STATUS_SEVERE) {
                closeHintControllerLocked()
            } else {
                hintController?.updateThermalStatus(status)
            }
        }
    }

    private fun reportAdpfCycle(actualDurationNanos: Long, sampleElapsedMs: Long) {
        if (mode != FrameScopePerformanceMode.SustainedThroughput) return
        if (currentThermalStatus >= THERMAL_STATUS_SEVERE) return
        if (context == null || ownerTid <= 0 || Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return

        synchronized(hintLock) {
            if (closed.get()) return
            if (hintController == null) {
                val target = calibratedTargetDurationNanos(actualDurationNanos)
                hintController = runCatching {
                    Api31AdpfController(
                        context = context,
                        ownerTid = ownerTid,
                        targetDurationNanos = target,
                        initialThermalStatus = currentThermalStatus,
                    )
                }.getOrNull() ?: return
            }
            val active = hintController ?: return
            if (!active.report(actualDurationNanos, sampleElapsedMs)) {
                closeHintControllerLocked()
            }
        }
    }

    private fun publishTraceCounters(progress: MicroscopeIndexingProgress) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return
        runCatching {
            Api29TraceCounters.publishProgress(
                indexedFrames = progress.indexedFrames,
                reusedFrames = progress.reusedFrames,
                thermalStatus = currentThermalStatus,
            )
        }
    }

    private fun maybePublishDiagnostics(progress: MicroscopeIndexingProgress) {
        if (
            lastDiagnosticsElapsedMs != Long.MIN_VALUE &&
            progress.operationElapsedMs - lastDiagnosticsElapsedMs < DIAGNOSTICS_INTERVAL_MS
        ) {
            return
        }
        lastDiagnosticsElapsedMs = progress.operationElapsedMs

        val workUnits = progress.workUnitsForScheduling()
        val previousWorkUnits = lastDiagnosticsWorkUnits
        val previousSampleElapsedMs = lastDiagnosticsSampleElapsedMs
        val indexingFps = if (
            previousWorkUnits != null &&
            previousSampleElapsedMs != null &&
            workUnits >= previousWorkUnits &&
            progress.sampleElapsedMs > previousSampleElapsedMs
        ) {
            val frameDelta = workUnits - previousWorkUnits
            val elapsedMs = progress.sampleElapsedMs - previousSampleElapsedMs
            frameDelta.toDouble() * 1_000.0 / elapsedMs.toDouble()
        } else {
            null
        }
        lastDiagnosticsWorkUnits = workUnits
        lastDiagnosticsSampleElapsedMs = progress.sampleElapsedMs

        val nowElapsedNanos = safeElapsedRealtimeNanos()
        val processCpuMs = safeProcessCpuTimeMs()
        val elapsedNanos = nowElapsedNanos - lastCpuSampleElapsedNanos
        val cpuMsDelta = processCpuMs - lastCpuSampleProcessMs
        val cpuCoreEquivalentPercent = if (elapsedNanos > 0L && cpuMsDelta >= 0L) {
            cpuMsDelta.toDouble() * NANOS_PER_MILLISECOND.toDouble() * 100.0 /
                elapsedNanos.toDouble()
        } else {
            null
        }
        lastCpuSampleElapsedNanos = nowElapsedNanos
        lastCpuSampleProcessMs = processCpuMs

        val memoryInfo = ActivityManager.MemoryInfo()
        val activityManager = context?.getSystemService(ActivityManager::class.java)
        val hasMemoryInfo = activityManager != null && runCatching {
            activityManager.getMemoryInfo(memoryInfo)
        }.isSuccess
        val processPssBytes = runCatching { Debug.getPss() * KIBIBYTE }.getOrNull()
        val nowElapsedMs = nowElapsedNanos / NANOS_PER_MILLISECOND
        val activeHintTarget = synchronized(hintLock) { hintController?.targetDurationNanos }
        val snapshot = AndroidPerformanceSnapshot(
            operationId = operationId,
            mode = mode,
            backend = SOFTWARE_BACKEND,
            indexingTid = ownerTid.takeIf { it > 0 },
            indexingThreadPriority = ownerTid.takeIf { it > 0 }?.let { tid ->
                runCatching { Process.getThreadPriority(tid) }.getOrNull()
            },
            adpfPlatformAvailable = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S,
            adpfActive = activeHintTarget != null,
            adpfTargetNanos = activeHintTarget,
            progressSequence = progress.sequence,
            indexedFrames = progress.indexedFrames,
            indexingFps = indexingFps,
            processCpuCoreEquivalentPercent = cpuCoreEquivalentPercent,
            processPssBytes = processPssBytes,
            availableMemoryBytes = memoryInfo.availMem.takeIf { hasMemoryInfo },
            lowMemory = memoryInfo.lowMemory.takeIf { hasMemoryInfo },
            memoryThresholdBytes = memoryInfo.threshold.takeIf { hasMemoryInfo },
            trimMemoryLevel = trimMemoryLevel(),
            thermalStatus = currentThermalStatus,
            thermalHeadroom = sampleThermalHeadroom(context, nowElapsedMs),
            backgroundRestricted = activityManager?.let { manager ->
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                    runCatching { manager.isBackgroundRestricted }.getOrNull()
                } else {
                    null
                }
            },
            logicalProcessorCount = Runtime.getRuntime().availableProcessors(),
        )
        publishSnapshot(snapshot)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            runCatching { Api29TraceCounters.publishDiagnostics(snapshot) }
        }
        if (context != null) Log.i(TAG, snapshot.toLogLine())
    }

    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        synchronized(hintLock) { closeHintControllerLocked() }
        unregister(operationId, this)
        if (priorityApplied && originalPriority != null && ownerTid > 0) {
            runCatching { Process.setThreadPriority(ownerTid, originalPriority) }
        }
        if (traceStarted) runCatching { Trace.endSection() }
    }

    private fun closeHintControllerLocked() {
        val current = hintController ?: return
        hintController = null
        runCatching { current.close() }
    }
}

private interface AdpfController : Closeable {
    val targetDurationNanos: Long
    fun report(actualDurationNanos: Long, sampleElapsedMs: Long): Boolean
    fun updateThermalStatus(status: Int)
}

/** Loaded only after an API 31+ guard in [ActiveIndexingSession]. */
@TargetApi(Build.VERSION_CODES.S)
private class Api31AdpfController(
    context: Context,
    ownerTid: Int,
    override val targetDurationNanos: Long,
    initialThermalStatus: Int,
) : AdpfController {
    private val manager = requireNotNull(context.getSystemService(PerformanceHintManager::class.java))
    private val preferredUpdateRateNanos = manager.preferredUpdateRateNanos.coerceAtLeast(0L)
    private val session = requireNotNull(
        manager.createHintSession(intArrayOf(ownerTid), targetDurationNanos),
    ) { "PerformanceHintManager did not create a session for the indexing thread." }
    private var lastReportSampleNanos = Long.MIN_VALUE

    init {
        updateThermalStatus(initialThermalStatus)
    }

    override fun report(actualDurationNanos: Long, sampleElapsedMs: Long): Boolean {
        val sampleElapsedNanos = millisecondsToNanoseconds(sampleElapsedMs)
        if (
            lastReportSampleNanos != Long.MIN_VALUE &&
            preferredUpdateRateNanos > 0L &&
            sampleElapsedNanos - lastReportSampleNanos < preferredUpdateRateNanos
        ) {
            return true
        }
        return runCatching {
            session.updateTargetWorkDuration(targetDurationNanos)
            session.reportActualWorkDuration(actualDurationNanos)
            lastReportSampleNanos = sampleElapsedNanos
        }.isSuccess
    }

    override fun updateThermalStatus(status: Int) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.VANILLA_ICE_CREAM) {
            runCatching {
                Api35AdpfPowerEfficiency.set(
                    session = session,
                    enabled = status >= THERMAL_STATUS_MODERATE,
                )
            }
        }
    }

    override fun close() {
        session.close()
    }
}

/** Loaded only after an API 35+ guard. */
@TargetApi(Build.VERSION_CODES.VANILLA_ICE_CREAM)
private object Api35AdpfPowerEfficiency {
    fun set(
        session: PerformanceHintManager.Session,
        enabled: Boolean,
    ) {
        session.setPreferPowerEfficiency(enabled)
    }
}

/** Loaded only after an API 29+ guard in [FrameScopePerformanceRuntime.initialize]. */
@TargetApi(Build.VERSION_CODES.Q)
private class Api29ThermalMonitor(
    context: Context,
    private val onStatus: (Int) -> Unit,
) : Closeable {
    private val powerManager = requireNotNull(context.getSystemService(PowerManager::class.java))
    private val listener = PowerManager.OnThermalStatusChangedListener(onStatus)

    init {
        onStatus(powerManager.currentThermalStatus)
        powerManager.addThermalStatusListener(listener)
    }

    override fun close() {
        powerManager.removeThermalStatusListener(listener)
    }
}

/** Loaded only after an API 30+ guard. */
@TargetApi(Build.VERSION_CODES.R)
private object Api30ThermalHeadroom {
    fun read(context: Context, forecastSeconds: Int): Float? =
        context.getSystemService(PowerManager::class.java)?.getThermalHeadroom(forecastSeconds)
}

/** Loaded only after an API 29+ guard. */
@TargetApi(Build.VERSION_CODES.Q)
private object Api29TraceCounters {
    fun publishProgress(
        indexedFrames: Long,
        reusedFrames: Long,
        thermalStatus: Int,
    ) {
        Trace.setCounter("FrameScope indexed frames", indexedFrames)
        Trace.setCounter("FrameScope reused frames", reusedFrames)
        Trace.setCounter("FrameScope thermal status", thermalStatus.toLong())
    }

    fun publishDiagnostics(snapshot: AndroidPerformanceSnapshot) {
        snapshot.indexingFps?.let { fps ->
            Trace.setCounter("FrameScope indexing fps x100", (fps * 100.0).toLong())
        }
        snapshot.processPssBytes?.let { bytes ->
            Trace.setCounter("FrameScope process PSS KiB", bytes / KIBIBYTE)
        }
    }
}

private fun MicroscopeIndexingProgress.asSchedulingSample(): IndexingWorkSample? {
    if (
        stage != MicroscopeIndexingStage.Indexing &&
        stage != MicroscopeIndexingStage.ValidatingExistingIndex
    ) {
        return null
    }
    return IndexingWorkSample(
        sequence = sequence,
        sampleElapsedMs = sampleElapsedMs,
        workUnits = workUnitsForScheduling(),
    )
}

private fun MicroscopeIndexingProgress.workUnitsForScheduling(): Long = when (stage) {
    MicroscopeIndexingStage.ValidatingExistingIndex -> reusedFrames
    else -> indexedFrames
}

private fun AndroidPerformanceSnapshot.toLogLine(): String = buildString {
    append("indexing op=").append(operationId)
    append(" backend=").append(backend)
    append(" fps=").append(indexingFps?.let { String.format(Locale.US, "%.1f", it) } ?: "n/a")
    append(" cpu_core_eq_pct=")
        .append(
            processCpuCoreEquivalentPercent?.let { String.format(Locale.US, "%.1f", it) } ?: "n/a",
        )
    append(" pss_mib=").append(processPssBytes?.div(MEBIBYTE) ?: -1L)
    append(" adpf=").append(adpfActive)
    append(" thermal=").append(thermalStatus)
    append(" headroom=").append(thermalHeadroom ?: Float.NaN)
    append(" low_memory=").append(lowMemory ?: false)
    append(" background_restricted=").append(backgroundRestricted ?: false)
}

private fun safeElapsedRealtimeNanos(): Long =
    runCatching { SystemClock.elapsedRealtimeNanos() }.getOrElse { System.nanoTime() }

private fun safeProcessCpuTimeMs(): Long =
    runCatching { Process.getElapsedCpuTime() }.getOrDefault(0L)

private fun millisecondsToNanoseconds(milliseconds: Long): Long {
    if (milliseconds <= 0L) return 0L
    if (milliseconds > Long.MAX_VALUE / NANOS_PER_MILLISECOND) return Long.MAX_VALUE
    return milliseconds * NANOS_PER_MILLISECOND
}

enum class FrameScopePerformanceMode {
    Balanced,
    SustainedThroughput,
}

data class AndroidPerformanceSnapshot(
    val operationId: Long,
    val mode: FrameScopePerformanceMode,
    val backend: String,
    val indexingTid: Int?,
    val indexingThreadPriority: Int?,
    val adpfPlatformAvailable: Boolean,
    val adpfActive: Boolean,
    val adpfTargetNanos: Long?,
    val progressSequence: Long,
    val indexedFrames: Long,
    val indexingFps: Double?,
    /** Process CPU time divided by wall time. This can exceed 100% on multicore devices. */
    val processCpuCoreEquivalentPercent: Double?,
    val processPssBytes: Long?,
    val availableMemoryBytes: Long?,
    val lowMemory: Boolean?,
    val memoryThresholdBytes: Long?,
    val trimMemoryLevel: Int,
    val thermalStatus: Int,
    val thermalHeadroom: Float?,
    val backgroundRestricted: Boolean?,
    val logicalProcessorCount: Int,
)

private const val TAG = "FrameScopePerf"
private const val INDEXING_TRACE_SECTION = "FrameScope#indexing"
private const val SOFTWARE_BACKEND = "ffmpeg-avcodec"
private const val INVALID_TID = -1
private const val THERMAL_STATUS_UNKNOWN = -1
private const val THERMAL_STATUS_MODERATE = 2
private const val THERMAL_STATUS_SEVERE = 3
private const val LOW_MEMORY_SENTINEL = Int.MAX_VALUE
private const val DIAGNOSTICS_INTERVAL_MS = 5_000L
private const val THERMAL_HEADROOM_POLL_MS = 10_000L
private const val THERMAL_HEADROOM_FORECAST_SECONDS = 30
private const val NANOS_PER_MILLISECOND = 1_000_000L
private const val KIBIBYTE = 1_024L
private const val MEBIBYTE = 1_024L * 1_024L
