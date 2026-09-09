package com.framescope.app.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RamAccelerationPolicyTest {
    private val normalDevice = RamAccelerationDeviceProfile(
        totalRamBytes = 8L * 1024L * RamAccelerationPolicy.MIB,
        availableRamBytes = 4L * 1024L * RamAccelerationPolicy.MIB,
        lowMemoryThresholdBytes = 512L * RamAccelerationPolicy.MIB,
        memoryClassMb = 256,
        lowRamDevice = false,
        systemLowMemory = false,
    )

    @Test
    fun automaticUsesNativeHeadroomInsteadOfTreatingJavaHeapClassAsNativeCeiling() {
        val recommended = RamAccelerationPolicy.recommendedTotalBytes(normalDevice)
        val tinyManagedHeap = RamAccelerationPolicy.recommendedTotalBytes(
            normalDevice.copy(memoryClassMb = 128),
        )

        assertEquals(512L * RamAccelerationPolicy.MIB, recommended)
        assertEquals(recommended, tinyManagedHeap)
    }

    @Test
    fun twelveGibDeviceCanUseMoreThanLegacyFiveHundredTwelveMibCeiling() {
        val profile = RamAccelerationDeviceProfile(
            totalRamBytes = 12L * 1024L * RamAccelerationPolicy.MIB,
            availableRamBytes = 6L * 1024L * RamAccelerationPolicy.MIB,
            lowMemoryThresholdBytes = 512L * RamAccelerationPolicy.MIB,
            memoryClassMb = 512,
            lowRamDevice = false,
            systemLowMemory = false,
        )

        assertEquals(768L * RamAccelerationPolicy.MIB, RamAccelerationPolicy.recommendedTotalBytes(profile))
        assertEquals(1024L * RamAccelerationPolicy.MIB, RamAccelerationPolicy.aggressiveTotalBytes(profile))
        assertEquals(1024L * RamAccelerationPolicy.MIB, RamAccelerationPolicy.customMaximumTotalBytes(profile))
    }

    @Test
    fun lowCurrentHeadroomCanReduceAutomaticRecommendationToZero() {
        val recommended = RamAccelerationPolicy.recommendedTotalBytes(
            normalDevice.copy(availableRamBytes = 128L * RamAccelerationPolicy.MIB),
        )

        assertEquals(0L, recommended)
    }

    @Test
    fun lowRamDevicesRemainConservative() {
        val profile = RamAccelerationDeviceProfile(
            totalRamBytes = 3L * 1024L * RamAccelerationPolicy.MIB,
            availableRamBytes = 1L * 1024L * RamAccelerationPolicy.MIB,
            lowMemoryThresholdBytes = 256L * RamAccelerationPolicy.MIB,
            memoryClassMb = 256,
            lowRamDevice = true,
            systemLowMemory = false,
        )

        assertEquals(32L * RamAccelerationPolicy.MIB, RamAccelerationPolicy.recommendedTotalBytes(profile))
        assertEquals(64L * RamAccelerationPolicy.MIB, RamAccelerationPolicy.aggressiveTotalBytes(profile))
        assertEquals(128L * RamAccelerationPolicy.MIB, RamAccelerationPolicy.customMaximumTotalBytes(profile))
    }

    @Test
    fun offImmediatelyResolvesToZeroResidentBudget() {
        val budget = RamAccelerationPolicy.resolve(
            mode = RamAccelerationMode.Off,
            profile = normalDevice,
            customTotalMiB = 128,
        )

        assertEquals(0L, budget.requestedTotalBytes)
        assertEquals(0L, budget.sourceCacheBytes)
        assertEquals(0L, budget.previewCacheBytes)
    }

    @Test
    fun customBudgetCanReachOneGibAndPrioritizesPreviewCoverage() {
        val budget = RamAccelerationPolicy.resolve(
            mode = RamAccelerationMode.Custom,
            profile = normalDevice,
            customTotalMiB = 10_000,
        )

        assertEquals(1024L * RamAccelerationPolicy.MIB, budget.requestedTotalBytes)
        assertEquals(budget.totalBytes * 30L / 100L, budget.sourceCacheBytes)
        assertEquals(budget.totalBytes - budget.sourceCacheBytes, budget.previewCacheBytes)
        assertFalse(budget.headroomLimited)
    }

    @Test
    fun customPreferenceIsClampedByCurrentLiveHeadroomWithoutChangingDeviceMaximum() {
        val constrained = normalDevice.copy(
            availableRamBytes = 1280L * RamAccelerationPolicy.MIB,
        )
        val budget = RamAccelerationPolicy.resolve(
            mode = RamAccelerationMode.Custom,
            profile = constrained,
            customTotalMiB = 1024,
        )

        assertEquals(1024L * RamAccelerationPolicy.MIB, budget.requestedTotalBytes)
        assertEquals(128L * RamAccelerationPolicy.MIB, budget.totalBytes)
        assertTrue(budget.headroomLimited)
        assertEquals(1024L * RamAccelerationPolicy.MIB, budget.customMaximumTotalBytes)
    }

    @Test
    fun memoryPressureShrinksActiveBudgetAndKeepsRequestedBudgetVisible() {
        val normal = RamAccelerationPolicy.resolve(
            mode = RamAccelerationMode.Custom,
            profile = normalDevice,
            customTotalMiB = 256,
        )
        val pressured = RamAccelerationPolicy.resolve(
            mode = RamAccelerationMode.Custom,
            profile = normalDevice,
            customTotalMiB = 256,
            memoryPressureScalePercent = 50,
        )

        assertEquals(normal.requestedTotalBytes, pressured.requestedTotalBytes)
        assertEquals(normal.totalBytes / 2L, pressured.totalBytes)
        assertEquals(50, pressured.pressureReductionPercent)
    }

    @Test
    fun systemLowMemoryForcesCriticalTwentyFivePercentScale() {
        val budget = RamAccelerationPolicy.resolve(
            mode = RamAccelerationMode.Custom,
            profile = normalDevice.copy(systemLowMemory = true),
            customTotalMiB = 256,
        )

        assertEquals(64L * RamAccelerationPolicy.MIB, budget.totalBytes)
        assertEquals(75, budget.pressureReductionPercent)
    }
}
