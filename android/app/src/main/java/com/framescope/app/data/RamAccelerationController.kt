package com.framescope.app.data

import android.app.ActivityManager
import android.content.ComponentCallbacks2
import android.content.Context
import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

private const val PREFS_NAME = "framescope_ram_acceleration"
private const val KEY_MODE = "mode"
private const val KEY_CUSTOM_MIB = "custom_mib"
private const val DEFAULT_CUSTOM_MIB = 256
private const val PRESSURE_NONE_PERCENT = 100
private const val PRESSURE_MODERATE_PERCENT = 75
private const val PRESSURE_STRONG_PERCENT = 50
private const val PRESSURE_CRITICAL_PERCENT = 25

data class RamAccelerationState(
    val mode: RamAccelerationMode,
    val customTotalMiB: Int,
    val customMaximumMiB: Int,
    val recommendedTotalMiB: Int,
    val aggressiveTotalMiB: Int,
    val requestedTotalMiB: Int,
    val activeTotalMiB: Int,
    val sourceCacheMiB: Int,
    val previewCacheMiB: Int,
    val availableMemoryMiB: Int,
    val managedHeapClassMiB: Int,
    val lowRamDevice: Boolean,
    val underMemoryPressure: Boolean,
    val pressureReductionPercent: Int,
    val pressureReductionCount: Long,
    val headroomLimited: Boolean,
    val nativeApplied: Boolean,
    val metrics: RamAccelerationMetrics?,
)

class RamAccelerationController internal constructor(
    context: Context,
    private val cacheRoot: String,
    private val bridge: NativeRamAccelerationBridge = RamAccelerationBridge,
    private val preferences: SharedPreferences = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE),
) {
    private val activityManager = context.getSystemService(ActivityManager::class.java)
    private var profile = readDeviceProfile(activityManager)
    private var memoryPressureScalePercent = if (profile.systemLowMemory) {
        PRESSURE_CRITICAL_PERCENT
    } else {
        PRESSURE_NONE_PERCENT
    }
    private var pressureReductionCount = if (profile.systemLowMemory) 1L else 0L
    private var preferredCustomMiB = readStoredCustomMiB()

    private val _state = MutableStateFlow(resolveAndApply(readMode(), preferredCustomMiB))
    val state: StateFlow<RamAccelerationState> = _state.asStateFlow()

    fun setMode(mode: RamAccelerationMode) {
        preferences.edit().putString(KEY_MODE, mode.name).apply()
        refreshDeviceProfile()
        _state.value = resolveAndApply(mode, preferredCustomMiB)
    }

    fun setCustomTotalMiB(value: Int) {
        refreshDeviceProfile()
        val customMaximum = RamAccelerationPolicy.customMaximumTotalBytes(profile).toWholeMiB()
            .coerceIn(RamAccelerationPolicy.MIN_CUSTOM_MIB, RamAccelerationPolicy.MAX_CUSTOM_MIB)
        preferredCustomMiB = value.coerceIn(RamAccelerationPolicy.MIN_CUSTOM_MIB, customMaximum)
        preferences.edit().putInt(KEY_CUSTOM_MIB, preferredCustomMiB).apply()
        _state.value = resolveAndApply(_state.value.mode, preferredCustomMiB)
    }

    /**
     * Refreshes native cache counters only. This intentionally does not poll ActivityManager: Android
     * recommends trim callbacks for memory-pressure management rather than frequent memory polling.
     */
    fun refreshMetrics() {
        _state.value = _state.value.copy(metrics = bridge.stats(cacheRoot))
    }

    /**
     * Re-evaluate live headroom when the app becomes visible again. A previous trim is allowed to
     * recover only after Android no longer reports low memory, avoiding cache-size oscillation while
     * the process remains under pressure.
     */
    fun onForeground() {
        refreshDeviceProfile()
        memoryPressureScalePercent = if (profile.systemLowMemory) {
            if (memoryPressureScalePercent > PRESSURE_CRITICAL_PERCENT) {
                pressureReductionCount = pressureReductionCount.saturatingIncrement()
            }
            PRESSURE_CRITICAL_PERCENT
        } else {
            PRESSURE_NONE_PERCENT
        }
        _state.value = resolveAndApply(_state.value.mode, preferredCustomMiB)
    }

    fun onTrimMemory(level: Int) {
        val requestedScale = pressureScaleForTrimLevel(level) ?: return
        if (requestedScale >= memoryPressureScalePercent) return
        memoryPressureScalePercent = requestedScale
        pressureReductionCount = pressureReductionCount.saturatingIncrement()
        refreshDeviceProfile()
        _state.value = resolveAndApply(_state.value.mode, preferredCustomMiB)
    }

    fun onLowMemory() {
        if (memoryPressureScalePercent > PRESSURE_CRITICAL_PERCENT) {
            pressureReductionCount = pressureReductionCount.saturatingIncrement()
        }
        memoryPressureScalePercent = PRESSURE_CRITICAL_PERCENT
        refreshDeviceProfile()
        _state.value = resolveAndApply(_state.value.mode, preferredCustomMiB)
    }

    private fun refreshDeviceProfile() {
        profile = readDeviceProfile(activityManager)
        if (profile.systemLowMemory && memoryPressureScalePercent > PRESSURE_CRITICAL_PERCENT) {
            memoryPressureScalePercent = PRESSURE_CRITICAL_PERCENT
            pressureReductionCount = pressureReductionCount.saturatingIncrement()
        }
    }

    private fun resolveAndApply(
        mode: RamAccelerationMode,
        customMiB: Int,
    ): RamAccelerationState {
        val budget = RamAccelerationPolicy.resolve(
            mode = mode,
            profile = profile,
            customTotalMiB = customMiB,
            memoryPressureScalePercent = memoryPressureScalePercent,
        )
        val applied = bridge.configure(
            cacheRoot = cacheRoot,
            sourceCacheBytes = budget.sourceCacheBytes,
            previewCacheBytes = budget.previewCacheBytes,
        )
        return RamAccelerationState(
            mode = mode,
            customTotalMiB = minOf(
                customMiB,
                budget.customMaximumTotalBytes.toWholeMiB(),
            ).coerceAtLeast(RamAccelerationPolicy.MIN_CUSTOM_MIB),
            customMaximumMiB = budget.customMaximumTotalBytes.toWholeMiB(),
            recommendedTotalMiB = budget.recommendedTotalBytes.toWholeMiB(),
            aggressiveTotalMiB = budget.aggressiveTotalBytes.toWholeMiB(),
            requestedTotalMiB = budget.requestedTotalBytes.toWholeMiB(),
            activeTotalMiB = budget.totalBytes.toWholeMiB(),
            sourceCacheMiB = budget.sourceCacheBytes.toWholeMiB(),
            previewCacheMiB = budget.previewCacheBytes.toWholeMiB(),
            availableMemoryMiB = profile.availableRamBytes.toWholeMiB(),
            managedHeapClassMiB = profile.memoryClassMb.coerceAtLeast(0),
            lowRamDevice = profile.lowRamDevice,
            underMemoryPressure = budget.pressureReductionPercent > 0 || profile.systemLowMemory,
            pressureReductionPercent = budget.pressureReductionPercent,
            pressureReductionCount = pressureReductionCount,
            headroomLimited = budget.headroomLimited,
            nativeApplied = applied,
            metrics = bridge.stats(cacheRoot),
        )
    }

    private fun readMode(): RamAccelerationMode = preferences
        .getString(KEY_MODE, RamAccelerationMode.Automatic.name)
        ?.let { stored -> runCatching { RamAccelerationMode.valueOf(stored) }.getOrNull() }
        ?: RamAccelerationMode.Automatic

    private fun readStoredCustomMiB(): Int = preferences
        .getInt(KEY_CUSTOM_MIB, DEFAULT_CUSTOM_MIB)
        .coerceIn(RamAccelerationPolicy.MIN_CUSTOM_MIB, RamAccelerationPolicy.MAX_CUSTOM_MIB)

    @Suppress("DEPRECATION")
    private fun pressureScaleForTrimLevel(level: Int): Int? = when {
        level >= ComponentCallbacks2.TRIM_MEMORY_BACKGROUND -> PRESSURE_CRITICAL_PERCENT
        level >= ComponentCallbacks2.TRIM_MEMORY_UI_HIDDEN -> PRESSURE_STRONG_PERCENT
        level >= ComponentCallbacks2.TRIM_MEMORY_RUNNING_CRITICAL -> PRESSURE_CRITICAL_PERCENT
        level >= ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW -> PRESSURE_STRONG_PERCENT
        level >= ComponentCallbacks2.TRIM_MEMORY_RUNNING_MODERATE -> PRESSURE_MODERATE_PERCENT
        else -> null
    }
}

private fun readDeviceProfile(activityManager: ActivityManager?): RamAccelerationDeviceProfile {
    if (activityManager == null) {
        return RamAccelerationDeviceProfile(
            totalRamBytes = 0L,
            availableRamBytes = 0L,
            lowMemoryThresholdBytes = 0L,
            memoryClassMb = 0,
            lowRamDevice = false,
            systemLowMemory = false,
        )
    }
    val memoryInfo = ActivityManager.MemoryInfo()
    activityManager.getMemoryInfo(memoryInfo)
    return RamAccelerationDeviceProfile(
        totalRamBytes = memoryInfo.totalMem,
        availableRamBytes = memoryInfo.availMem,
        lowMemoryThresholdBytes = memoryInfo.threshold,
        memoryClassMb = activityManager.memoryClass,
        lowRamDevice = activityManager.isLowRamDevice,
        systemLowMemory = memoryInfo.lowMemory,
    )
}

private fun Long.toWholeMiB(): Int = (this / RamAccelerationPolicy.MIB)
    .coerceAtMost(Int.MAX_VALUE.toLong())
    .coerceAtLeast(0L)
    .toInt()

private fun Long.saturatingIncrement(): Long = if (this == Long.MAX_VALUE) this else this + 1L

object RamAccelerationRuntime {
    @Volatile
    private var controller: RamAccelerationController? = null

    fun initialize(context: Context, cacheRoot: String): RamAccelerationController = synchronized(this) {
        controller ?: RamAccelerationController(
            context = context.applicationContext,
            cacheRoot = cacheRoot,
        ).also { controller = it }
    }

    fun current(): RamAccelerationController? = controller

    fun onForeground() {
        controller?.onForeground()
    }

    fun onTrimMemory(level: Int) {
        controller?.onTrimMemory(level)
    }

    fun onLowMemory() {
        controller?.onLowMemory()
    }
}
