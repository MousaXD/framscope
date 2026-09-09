package com.framescope.app.data

import kotlin.math.max
import kotlin.math.min

enum class RamAccelerationMode {
    Off,
    Automatic,
    Aggressive,
    Custom,
}

data class RamAccelerationDeviceProfile(
    val totalRamBytes: Long,
    val availableRamBytes: Long,
    val lowMemoryThresholdBytes: Long,
    val memoryClassMb: Int,
    val lowRamDevice: Boolean,
    val systemLowMemory: Boolean,
)

data class RamAccelerationBudget(
    val mode: RamAccelerationMode,
    val requestedTotalBytes: Long,
    val sourceCacheBytes: Long,
    val previewCacheBytes: Long,
    val recommendedTotalBytes: Long,
    val aggressiveTotalBytes: Long,
    val customMaximumTotalBytes: Long,
    val headroomLimited: Boolean,
    val pressureReductionPercent: Int,
) {
    val totalBytes: Long
        get() = sourceCacheBytes + previewCacheBytes
}

/**
 * Policy for discretionary native navigation caches.
 *
 * FrameScope's source and scrub caches live in Rust/native memory, so ActivityManager.memoryClass
 * is not treated as a hard ceiling for the cache. It is instead part of the system-headroom reserve
 * alongside Android's low-memory threshold and a fraction of physical RAM. This avoids confusing a
 * managed-heap guideline with the process' native cache while still leaving room for the Java heap,
 * decoder state, Bitmaps, JNI buffers, and the rest of the system.
 *
 * Budgets are ceilings only. Native caches grow lazily from real navigation/scrub demand and trim
 * immediately when the active ceiling shrinks.
 */
object RamAccelerationPolicy {
    const val MIB: Long = 1024L * 1024L
    const val MIN_CUSTOM_MIB: Int = 16
    const val MAX_CUSTOM_MIB: Int = 1024

    private const val AUTOMATIC_MAX_TOTAL_MIB = 768L
    private const val AGGRESSIVE_MAX_TOTAL_MIB = 1024L
    private const val LOW_RAM_AUTOMATIC_MIB = 32L
    private const val LOW_RAM_AGGRESSIVE_MIB = 64L
    private const val LOW_RAM_CUSTOM_MAX_MIB = 128L
    private const val SOURCE_SHARE_PERCENT = 30L
    private const val MIN_PRESSURE_SCALE_PERCENT = 25
    private const val MAX_PRESSURE_SCALE_PERCENT = 100

    /** Recommended default for native post-index navigation caches. */
    fun recommendedTotalBytes(profile: RamAccelerationDeviceProfile): Long {
        val physicalTarget = when {
            profile.lowRamDevice -> LOW_RAM_AUTOMATIC_MIB * MIB
            profile.totalRamBytes > 0L -> min(
                profile.totalRamBytes / 16L,
                AUTOMATIC_MAX_TOTAL_MIB * MIB,
            )
            else -> MIN_CUSTOM_MIB.toLong() * MIB
        }
        return min(physicalTarget, availableHeadroomBudget(profile, divisor = 3L))
            .coerceAtLeast(0L)
    }

    /** Opt-in higher ceiling that still reserves substantial live system headroom. */
    fun aggressiveTotalBytes(profile: RamAccelerationDeviceProfile): Long {
        val physicalTarget = when {
            profile.lowRamDevice -> LOW_RAM_AGGRESSIVE_MIB * MIB
            profile.totalRamBytes > 0L -> min(
                profile.totalRamBytes / 12L,
                AGGRESSIVE_MAX_TOTAL_MIB * MIB,
            )
            else -> 2L * MIN_CUSTOM_MIB * MIB
        }
        return min(physicalTarget, availableHeadroomBudget(profile, divisor = 2L))
            .coerceAtLeast(0L)
    }

    /**
     * Device-stable Custom slider ceiling. Current headroom is applied separately to the active
     * budget so a temporary pressure event does not erase the user's configured preference.
     */
    fun customMaximumTotalBytes(profile: RamAccelerationDeviceProfile): Long {
        if (profile.lowRamDevice) return LOW_RAM_CUSTOM_MAX_MIB * MIB
        if (profile.totalRamBytes <= 0L) return 128L * MIB
        return min(
            profile.totalRamBytes / 8L,
            MAX_CUSTOM_MIB.toLong() * MIB,
        ).coerceAtLeast(MIN_CUSTOM_MIB.toLong() * MIB)
    }

    fun resolve(
        mode: RamAccelerationMode,
        profile: RamAccelerationDeviceProfile,
        customTotalMiB: Int,
        memoryPressureScalePercent: Int = 100,
    ): RamAccelerationBudget {
        val recommended = recommendedTotalBytes(profile)
        val aggressive = aggressiveTotalBytes(profile)
        val customMaximum = customMaximumTotalBytes(profile)
        val requested = when (mode) {
            RamAccelerationMode.Off -> 0L
            RamAccelerationMode.Automatic -> recommended
            RamAccelerationMode.Aggressive -> aggressive
            RamAccelerationMode.Custom -> min(
                customTotalMiB
                    .coerceIn(MIN_CUSTOM_MIB, MAX_CUSTOM_MIB)
                    .toLong() * MIB,
                customMaximum,
            )
        }

        val headroomCap = when (mode) {
            RamAccelerationMode.Off -> 0L
            RamAccelerationMode.Automatic,
            RamAccelerationMode.Aggressive,
            -> Long.MAX_VALUE // already headroom-limited above
            RamAccelerationMode.Custom -> availableHeadroomBudget(profile, divisor = 2L)
        }
        val headroomAdjusted = min(requested, headroomCap).coerceAtLeast(0L)
        val pressureScale = if (profile.systemLowMemory) {
            MIN_PRESSURE_SCALE_PERCENT
        } else {
            memoryPressureScalePercent.coerceIn(
                MIN_PRESSURE_SCALE_PERCENT,
                MAX_PRESSURE_SCALE_PERCENT,
            )
        }
        val pressureAdjusted = if (headroomAdjusted == 0L) {
            0L
        } else {
            headroomAdjusted * pressureScale / 100L
        }
        val source = pressureAdjusted * SOURCE_SHARE_PERCENT / 100L
        val preview = pressureAdjusted - source
        return RamAccelerationBudget(
            mode = mode,
            requestedTotalBytes = requested,
            sourceCacheBytes = source,
            previewCacheBytes = preview,
            recommendedTotalBytes = recommended,
            aggressiveTotalBytes = aggressive,
            customMaximumTotalBytes = customMaximum,
            headroomLimited = headroomAdjusted < requested,
            pressureReductionPercent = if (requested > 0L) 100 - pressureScale else 0,
        )
    }

    /**
     * Returns the portion of currently available RAM FrameScope may consume after preserving room
     * for Android's low-memory threshold, the managed heap, decoder/UI allocations, and the rest of
     * the process. Unknown available-memory data is treated as unbounded here; physical/hard caps
     * still apply at the caller.
     */
    private fun availableHeadroomBudget(
        profile: RamAccelerationDeviceProfile,
        divisor: Long,
    ): Long {
        if (profile.availableRamBytes <= 0L) return Long.MAX_VALUE
        val thresholdReserve = profile.lowMemoryThresholdBytes
            .coerceAtLeast(0L)
            .let { threshold -> saturatingMultiply(threshold, 2L) }
        val managedHeapReserve = profile.memoryClassMb
            .coerceAtLeast(0)
            .toLong()
            .let { memoryClass -> saturatingMultiply(memoryClass, 2L * MIB) }
        val physicalReserve = if (profile.totalRamBytes > 0L) {
            profile.totalRamBytes / 8L
        } else {
            0L
        }
        val reserve = max(physicalReserve, max(thresholdReserve, managedHeapReserve))
        return profile.availableRamBytes
            .saturatingSubtract(reserve)
            .coerceAtLeast(0L) / divisor
    }

    private fun saturatingMultiply(left: Long, right: Long): Long =
        runCatching { Math.multiplyExact(left, right) }.getOrDefault(Long.MAX_VALUE)

    private fun Long.saturatingSubtract(other: Long): Long =
        if (other >= this) 0L else this - other
}
