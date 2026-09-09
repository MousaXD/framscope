package com.framescope.app.performance

import org.junit.Assert.assertEquals
import org.junit.Test

class AndroidHardwareDecodePolicyTest {
    @Test
    fun preAndroid10DoesNotGuessHardwareFromUnavailableFlags() {
        assertEquals(
            DecoderAccelerationClass.Unknown,
            AndroidHardwareDecodeCapabilities.classifyDecoder(
                sdkInt = 28,
                hardwareAccelerated = null,
                softwareOnly = null,
            ),
        )
    }

    @Test
    fun explicitHardwareFlagIsRequiredForHardwareClassification() {
        assertEquals(
            DecoderAccelerationClass.Hardware,
            AndroidHardwareDecodeCapabilities.classifyDecoder(
                sdkInt = 29,
                hardwareAccelerated = true,
                softwareOnly = false,
            ),
        )
        assertEquals(
            DecoderAccelerationClass.Unknown,
            AndroidHardwareDecodeCapabilities.classifyDecoder(
                sdkInt = 29,
                hardwareAccelerated = false,
                softwareOnly = false,
            ),
        )
    }

    @Test
    fun softwareOnlyFlagWinsOverHardwareFlag() {
        assertEquals(
            DecoderAccelerationClass.Software,
            AndroidHardwareDecodeCapabilities.classifyDecoder(
                sdkInt = 36,
                hardwareAccelerated = true,
                softwareOnly = true,
            ),
        )
    }
}
