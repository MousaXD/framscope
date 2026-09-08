package com.framescope.app.data

import kotlin.math.min

enum class RamAccelerationMode {
    Off,
    Automatic,
    Custom,
}

data class RamAccelerationDeviceProfile(
    val totalRamBytes: Long,
    val memoryClassMb: Int,
    val lowRamDevice: Boolean,
)

data class RamAccelerationBudget(
    val mode: RamAccelerationMode,
    val sourceCacheBytes: Long,
    val previewCacheBytes: Long,
    val recommendedTotalBytes: Long,
    val pressureReduced: Boolean,
) {
    val totalBytes: Long
        get() = sourceCacheBytes + previewCacheBytes
}

object RamAccelerationPolicy {
    const val MIB: Long = 1024L * 1024L
    const val MIN_CUSTOM_MIB: Int = 16
    const val MAX_CUSTOM_MIB: Int = 512

    private const val LOW_RAM_TOTAL_MIB = 32L
    private const val DEFAULT_MIN_TOTAL_MIB = 48L
    private const val DEFAULT_MAX_TOTAL_MIB = 256L
    private const val SOURCE_SHARE_PERCENT = 75L
    private const val PRESSURE_NUMERATOR = 1L
    private const val PRESSURE_DENOMINATOR = 2L

    /**
     * Conservative recommendation derived from both physical RAM and Android's per-process memory
     * class. The smaller limit wins so devices with generous physical RAM but a tight app heap do
     * not receive an unsafe recommendation.
     */
    fun recommendedTotalBytes(profile: RamAccelerationDeviceProfile): Long {
        if (profile.totalRamBytes <= 0L || profile.memoryClassMb <= 0) {
            return DEFAULT_MIN_TOTAL_MIB * MIB
        }
        if (profile.lowRamDevice) {
            return LOW_RAM_TOTAL_MIB * MIB
        }

        val physicalBudget = profile.totalRamBytes / 12L
        val heapBudget = profile.memoryClassMb.toLong() * MIB / 3L
        val bounded = min(physicalBudget, heapBudget)
        return bounded.coerceIn(DEFAULT_MIN_TOTAL_MIB * MIB, DEFAULT_MAX_TOTAL_MIB * MIB)
    }

    fun resolve(
        mode: RamAccelerationMode,
        profile: RamAccelerationDeviceProfile,
        customTotalMiB: Int,
        underMemoryPressure: Boolean,
    ): RamAccelerationBudget {
        val recommended = recommendedTotalBytes(profile)
        val requested = when (mode) {
            RamAccelerationMode.Off -> 0L
            RamAccelerationMode.Automatic -> recommended
            RamAccelerationMode.Custom -> customTotalMiB
                .coerceIn(MIN_CUSTOM_MIB, MAX_CUSTOM_MIB)
                .toLong() * MIB
        }
        val pressureAdjusted = if (underMemoryPressure && requested > 0L) {
            requested * PRESSURE_NUMERATOR / PRESSURE_DENOMINATOR
        } else {
            requested
        }
        val source = pressureAdjusted * SOURCE_SHARE_PERCENT / 100L
        val preview = pressureAdjusted - source
        return RamAccelerationBudget(
            mode = mode,
            sourceCacheBytes = source,
            previewCacheBytes = preview,
            recommendedTotalBytes = recommended,
            pressureReduced = underMemoryPressure && requested > 0L,
        )
    }
}
