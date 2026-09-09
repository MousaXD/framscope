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
private const val DEFAULT_CUSTOM_MIB = 128

data class RamAccelerationState(
    val mode: RamAccelerationMode,
    val customTotalMiB: Int,
    val recommendedTotalMiB: Int,
    val activeTotalMiB: Int,
    val sourceCacheMiB: Int,
    val previewCacheMiB: Int,
    val lowRamDevice: Boolean,
    val underMemoryPressure: Boolean,
    val nativeApplied: Boolean,
)

class RamAccelerationController internal constructor(
    context: Context,
    private val cacheRoot: String,
    private val bridge: NativeRamAccelerationBridge = RamAccelerationBridge,
    private val preferences: SharedPreferences = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE),
) {
    private val activityManager = context.getSystemService(ActivityManager::class.java)
    private val profile = readDeviceProfile(activityManager)
    private var underMemoryPressure = profile.systemLowMemory

    private val _state = MutableStateFlow(resolveAndApply(readMode(), readCustomMiB()))
    val state: StateFlow<RamAccelerationState> = _state.asStateFlow()

    fun setMode(mode: RamAccelerationMode) {
        preferences.edit().putString(KEY_MODE, mode.name).apply()
        _state.value = resolveAndApply(mode, _state.value.customTotalMiB)
    }

    fun setCustomTotalMiB(value: Int) {
        val clamped = value.coerceIn(
            RamAccelerationPolicy.MIN_CUSTOM_MIB,
            RamAccelerationPolicy.MAX_CUSTOM_MIB,
        )
        preferences.edit().putInt(KEY_CUSTOM_MIB, clamped).apply()
        _state.value = resolveAndApply(_state.value.mode, clamped)
    }

    /**
     * Memory pressure is sticky for this process. Avoiding automatic re-growth prevents allocation
     * oscillation while Android is reclaiming memory; a process restart re-evaluates live headroom.
     */
    fun onTrimMemory(level: Int) {
        if (!isMemoryPressureLevel(level)) return
        underMemoryPressure = true
        _state.value = resolveAndApply(_state.value.mode, _state.value.customTotalMiB)
    }

    fun onLowMemory() {
        underMemoryPressure = true
        _state.value = resolveAndApply(_state.value.mode, _state.value.customTotalMiB)
    }

    private fun resolveAndApply(
        mode: RamAccelerationMode,
        customMiB: Int,
    ): RamAccelerationState {
        val budget = RamAccelerationPolicy.resolve(
            mode = mode,
            profile = profile,
            customTotalMiB = customMiB,
            underMemoryPressure = underMemoryPressure,
        )
        val applied = bridge.configure(
            cacheRoot = cacheRoot,
            sourceCacheBytes = budget.sourceCacheBytes,
            previewCacheBytes = budget.previewCacheBytes,
        )
        return RamAccelerationState(
            mode = mode,
            customTotalMiB = customMiB,
            recommendedTotalMiB = budget.recommendedTotalBytes.toWholeMiB(),
            activeTotalMiB = budget.totalBytes.toWholeMiB(),
            sourceCacheMiB = budget.sourceCacheBytes.toWholeMiB(),
            previewCacheMiB = budget.previewCacheBytes.toWholeMiB(),
            lowRamDevice = profile.lowRamDevice,
            underMemoryPressure = budget.pressureReduced,
            nativeApplied = applied,
        )
    }

    private fun readMode(): RamAccelerationMode = preferences
        .getString(KEY_MODE, RamAccelerationMode.Automatic.name)
        ?.let { stored -> runCatching { RamAccelerationMode.valueOf(stored) }.getOrNull() }
        ?: RamAccelerationMode.Automatic

    private fun readCustomMiB(): Int = preferences
        .getInt(KEY_CUSTOM_MIB, DEFAULT_CUSTOM_MIB)
        .coerceIn(RamAccelerationPolicy.MIN_CUSTOM_MIB, RamAccelerationPolicy.MAX_CUSTOM_MIB)

    @Suppress("DEPRECATION")
    private fun isMemoryPressureLevel(level: Int): Boolean = when (level) {
        ComponentCallbacks2.TRIM_MEMORY_RUNNING_MODERATE,
        ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW,
        ComponentCallbacks2.TRIM_MEMORY_RUNNING_CRITICAL,
        ComponentCallbacks2.TRIM_MEMORY_BACKGROUND,
        ComponentCallbacks2.TRIM_MEMORY_MODERATE,
        ComponentCallbacks2.TRIM_MEMORY_COMPLETE,
        -> true
        else -> false
    }
}

private fun readDeviceProfile(activityManager: ActivityManager?): RamAccelerationDeviceProfile {
    if (activityManager == null) {
        return RamAccelerationDeviceProfile(
            totalRamBytes = 0L,
            availableRamBytes = 0L,
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
        memoryClassMb = activityManager.memoryClass,
        lowRamDevice = activityManager.isLowRamDevice,
        systemLowMemory = memoryInfo.lowMemory,
    )
}

private fun Long.toWholeMiB(): Int = (this / RamAccelerationPolicy.MIB)
    .coerceAtMost(Int.MAX_VALUE.toLong())
    .toInt()

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

    fun onTrimMemory(level: Int) {
        controller?.onTrimMemory(level)
    }

    fun onLowMemory() {
        controller?.onLowMemory()
    }
}
