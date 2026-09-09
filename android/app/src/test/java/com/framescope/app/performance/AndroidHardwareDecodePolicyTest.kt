package com.framescope.app.performance

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
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

    @Test
    fun selectionRejectsUnknownSoftwareAliasesAndWrongMime() {
        val candidates = listOf(
            capability("legacy.unknown", "video/avc", DecoderAccelerationClass.Unknown),
            capability("platform.software", "video/avc", DecoderAccelerationClass.Software),
            capability("vendor.alias", "video/avc", DecoderAccelerationClass.Hardware, alias = true),
            capability("vendor.hevc", "video/hevc", DecoderAccelerationClass.Hardware),
        )

        assertNull(
            AndroidHardwareDecodeCapabilities.selectPreferredHardwareDecoder(
                candidates = candidates,
                mimeType = "video/avc",
                requireByteBufferOutput = false,
            ),
        )
    }

    @Test
    fun byteBufferRequirementFallsBackWhenHardwareIsSurfaceOnly() {
        val surfaceOnly = capability(
            codecName = "vendor.surface",
            mimeType = "video/avc",
            acceleration = DecoderAccelerationClass.Hardware,
            supportsByteBufferOutput = false,
        )

        assertNull(
            AndroidHardwareDecodeCapabilities.selectPreferredHardwareDecoder(
                candidates = listOf(surfaceOnly),
                mimeType = "video/avc",
                requireByteBufferOutput = true,
            ),
        )
        assertEquals(
            surfaceOnly,
            AndroidHardwareDecodeCapabilities.selectPreferredHardwareDecoder(
                candidates = listOf(surfaceOnly),
                mimeType = "video/avc",
                requireByteBufferOutput = false,
            ),
        )
    }

    @Test
    fun explicitHardwareByteBufferCandidateIsSelected() {
        val candidate = capability(
            codecName = "vendor.hw",
            mimeType = "video/avc",
            acceleration = DecoderAccelerationClass.Hardware,
        )

        assertEquals(
            candidate,
            AndroidHardwareDecodeCapabilities.selectPreferredHardwareDecoder(
                candidates = listOf(candidate),
                mimeType = "VIDEO/AVC",
                requireByteBufferOutput = true,
            ),
        )
    }

    private fun capability(
        codecName: String,
        mimeType: String,
        acceleration: DecoderAccelerationClass,
        alias: Boolean? = false,
        supportsByteBufferOutput: Boolean = true,
    ) = AndroidVideoDecoderCapability(
        codecName = codecName,
        mimeType = mimeType,
        acceleration = acceleration,
        vendor = true,
        alias = alias,
        supportsSurfaceOutput = true,
        supportsByteBufferOutput = supportsByteBufferOutput,
        supportsFlexibleYuv420 = supportsByteBufferOutput,
    )
}
