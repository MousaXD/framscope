package com.framescope.app.performance

import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.os.Build

internal enum class DecoderAccelerationClass {
    Hardware,
    Software,
    Unknown,
}

internal data class AndroidVideoDecoderCapability(
    val codecName: String,
    val mimeType: String,
    val acceleration: DecoderAccelerationClass,
    val vendor: Boolean?,
    val alias: Boolean?,
    val supportsSurfaceOutput: Boolean,
    val supportsByteBufferOutput: Boolean,
    val supportsFlexibleYuv420: Boolean,
)

/**
 * App-level MediaCodec discovery for the codecs FrameScope currently indexes.
 *
 * API 29 added authoritative platform flags for hardware/software/vendor/alias classification.
 * On API 26-28 we intentionally report acceleration as Unknown instead of guessing from codec names.
 */
internal object AndroidHardwareDecodeCapabilities {
    private val targetMimeTypes = setOf(
        "video/avc",
        "video/hevc",
        "video/x-vnd.on2.vp9",
        "video/av01",
    )

    fun discover(): List<AndroidVideoDecoderCapability> =
        MediaCodecList(MediaCodecList.ALL_CODECS)
            .codecInfos
            .asSequence()
            .filter { codecInfo -> !codecInfo.isEncoder }
            .flatMap { codecInfo ->
                codecInfo.supportedTypes
                    .asSequence()
                    .map { type -> type.lowercase() }
                    .filter { type -> targetMimeTypes.contains(type) }
                    .mapNotNull { mime -> capability(codecInfo, mime) }
            }
            .sortedWith(compareBy(AndroidVideoDecoderCapability::mimeType, AndroidVideoDecoderCapability::codecName))
            .toList()

    /**
     * Fail-closed production-candidate selection. Pre-29 devices remain fully supported by the
     * existing software decoder, but are not auto-promoted to a hardware path from name heuristics.
     */
    fun preferredHardwareDecoder(
        mimeType: String,
        requireByteBufferOutput: Boolean,
    ): AndroidVideoDecoderCapability? = selectPreferredHardwareDecoder(
        candidates = discover(),
        mimeType = mimeType,
        requireByteBufferOutput = requireByteBufferOutput,
    )

    internal fun selectPreferredHardwareDecoder(
        candidates: List<AndroidVideoDecoderCapability>,
        mimeType: String,
        requireByteBufferOutput: Boolean,
    ): AndroidVideoDecoderCapability? =
        candidates
            .asSequence()
            .filter { capability -> capability.mimeType == mimeType.lowercase() }
            .filter { capability -> capability.acceleration == DecoderAccelerationClass.Hardware }
            .filter { capability -> capability.alias != true }
            .filter { capability -> !requireByteBufferOutput || capability.supportsByteBufferOutput }
            .firstOrNull()

    private fun capability(
        codecInfo: MediaCodecInfo,
        mimeType: String,
    ): AndroidVideoDecoderCapability? {
        val codecCapabilities = runCatching { codecInfo.getCapabilitiesForType(mimeType) }.getOrNull()
            ?: return null
        val colorFormats = codecCapabilities.colorFormats.orEmpty()
        val classification = classifyDecoder(
            sdkInt = Build.VERSION.SDK_INT,
            hardwareAccelerated = if (Build.VERSION.SDK_INT >= 29) codecInfo.isHardwareAccelerated else null,
            softwareOnly = if (Build.VERSION.SDK_INT >= 29) codecInfo.isSoftwareOnly else null,
        )

        return AndroidVideoDecoderCapability(
            codecName = codecInfo.name,
            mimeType = mimeType,
            acceleration = classification,
            vendor = if (Build.VERSION.SDK_INT >= 29) codecInfo.isVendor else null,
            alias = if (Build.VERSION.SDK_INT >= 29) codecInfo.isAlias else null,
            supportsSurfaceOutput = colorFormats.contains(MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface),
            supportsByteBufferOutput = colorFormats.any { format ->
                format != MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface
            },
            supportsFlexibleYuv420 = colorFormats.contains(MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible),
        )
    }

    internal fun classifyDecoder(
        sdkInt: Int,
        hardwareAccelerated: Boolean?,
        softwareOnly: Boolean?,
    ): DecoderAccelerationClass {
        if (sdkInt < 29) {
            return DecoderAccelerationClass.Unknown
        }
        return when {
            softwareOnly == true -> DecoderAccelerationClass.Software
            hardwareAccelerated == true -> DecoderAccelerationClass.Hardware
            else -> DecoderAccelerationClass.Unknown
        }
    }
}
