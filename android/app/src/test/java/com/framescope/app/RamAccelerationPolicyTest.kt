package com.framescope.app.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class RamAccelerationPolicyTest {
    private val normalDevice = RamAccelerationDeviceProfile(
        totalRamBytes = 8L * 1024L * RamAccelerationPolicy.MIB,
        availableRamBytes = 4L * 1024L * RamAccelerationPolicy.MIB,
        memoryClassMb = 256,
        lowRamDevice = false,
        systemLowMemory = false,
    )

    @Test
    fun automaticRecommendationUsesTheSmallerPhysicalHeapOrHeadroomLimit() {
        val recommended = RamAccelerationPolicy.recommendedTotalBytes(normalDevice)

        assertEquals(85L * RamAccelerationPolicy.MIB + 349_525L, recommended)
    }

    @Test
    fun lowCurrentHeadroomReducesAutomaticRecommendation() {
        val recommended = RamAccelerationPolicy.recommendedTotalBytes(
            normalDevice.copy(availableRamBytes = 128L * RamAccelerationPolicy.MIB),
        )

        assertEquals(16L * RamAccelerationPolicy.MIB, recommended)
    }

    @Test
    fun lowRamDevicesReceiveAConservativeRecommendation() {
        val recommended = RamAccelerationPolicy.recommendedTotalBytes(
            RamAccelerationDeviceProfile(
                totalRamBytes = 3L * 1024L * RamAccelerationPolicy.MIB,
                availableRamBytes = 1L * 1024L * RamAccelerationPolicy.MIB,
                memoryClassMb = 256,
                lowRamDevice = true,
                systemLowMemory = false,
            ),
        )

        assertEquals(32L * RamAccelerationPolicy.MIB, recommended)
    }

    @Test
    fun offImmediatelyResolvesToZeroResidentBudget() {
        val budget = RamAccelerationPolicy.resolve(
            mode = RamAccelerationMode.Off,
            profile = normalDevice,
            customTotalMiB = 128,
            underMemoryPressure = false,
        )

        assertEquals(0L, budget.sourceCacheBytes)
        assertEquals(0L, budget.previewCacheBytes)
    }

    @Test
    fun customBudgetIsClampedAndPartitionedWithoutExceedingTotal() {
        val budget = RamAccelerationPolicy.resolve(
            mode = RamAccelerationMode.Custom,
            profile = normalDevice,
            customTotalMiB = 10_000,
            underMemoryPressure = false,
        )

        assertEquals(512L * RamAccelerationPolicy.MIB, budget.totalBytes)
        assertEquals(budget.totalBytes * 75L / 100L, budget.sourceCacheBytes)
        assertEquals(budget.totalBytes - budget.sourceCacheBytes, budget.previewCacheBytes)
    }

    @Test
    fun memoryPressureHalvesTheActiveBudget() {
        val normal = RamAccelerationPolicy.resolve(
            mode = RamAccelerationMode.Custom,
            profile = normalDevice,
            customTotalMiB = 128,
            underMemoryPressure = false,
        )
        val pressured = RamAccelerationPolicy.resolve(
            mode = RamAccelerationMode.Custom,
            profile = normalDevice,
            customTotalMiB = 128,
            underMemoryPressure = true,
        )

        assertEquals(normal.totalBytes / 2L, pressured.totalBytes)
        assertTrue(pressured.pressureReduced)
    }

    @Test
    fun systemLowMemoryAtStartupAlsoActivatesPressureReduction() {
        val budget = RamAccelerationPolicy.resolve(
            mode = RamAccelerationMode.Custom,
            profile = normalDevice.copy(systemLowMemory = true),
            customTotalMiB = 128,
            underMemoryPressure = false,
        )

        assertEquals(64L * RamAccelerationPolicy.MIB, budget.totalBytes)
        assertTrue(budget.pressureReduced)
    }
}
