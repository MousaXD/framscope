package com.framescope.app.ui

import kotlin.math.abs
import kotlin.math.floor
import kotlin.math.sqrt

internal const val MICROSCOPE_PREVIEW_PIXEL_BUDGET: Long = 1_920L * 1_080L

internal data class MicroscopePreviewPlan(
    val sourceWidth: Int,
    val sourceHeight: Int,
    val targetWidth: Int,
    val targetHeight: Int,
) {
    val isDownscaled: Boolean
        get() = sourceWidth != targetWidth || sourceHeight != targetHeight

    fun sourceX(outputX: Int): Int = sourceCoordinate(outputX, sourceWidth, targetWidth)

    fun sourceY(outputY: Int): Int = sourceCoordinate(outputY, sourceHeight, targetHeight)

    private fun sourceCoordinate(
        outputCoordinate: Int,
        sourceExtent: Int,
        targetExtent: Int,
    ): Int {
        require(outputCoordinate in 0 until targetExtent)
        val centeredNumerator = (2L * outputCoordinate + 1L) * sourceExtent.toLong()
        return (centeredNumerator / (2L * targetExtent.toLong()))
            .toInt()
            .coerceAtMost(sourceExtent - 1)
    }
}

internal object MicroscopePreviewMath {
    fun plan(
        sourceWidth: Int,
        sourceHeight: Int,
        maxPixels: Long = MICROSCOPE_PREVIEW_PIXEL_BUDGET,
    ): MicroscopePreviewPlan? {
        if (sourceWidth <= 0 || sourceHeight <= 0 || maxPixels <= 0L) return null
        val sourcePixels = sourceWidth.toLong() * sourceHeight.toLong()
        if (sourcePixels <= maxPixels) {
            return MicroscopePreviewPlan(
                sourceWidth = sourceWidth,
                sourceHeight = sourceHeight,
                targetWidth = sourceWidth,
                targetHeight = sourceHeight,
            )
        }

        val scale = sqrt(maxPixels.toDouble() / sourcePixels.toDouble())
        var targetWidth = floor(sourceWidth.toDouble() * scale).toInt().coerceAtLeast(1)
        var targetHeight = floor(sourceHeight.toDouble() * scale).toInt().coerceAtLeast(1)

        while (targetWidth.toLong() * targetHeight.toLong() > maxPixels) {
            if (targetWidth >= targetHeight && targetWidth > 1) {
                targetWidth -= 1
            } else if (targetHeight > 1) {
                targetHeight -= 1
            } else {
                return null
            }
        }

        return MicroscopePreviewPlan(
            sourceWidth = sourceWidth,
            sourceHeight = sourceHeight,
            targetWidth = targetWidth,
            targetHeight = targetHeight,
        )
    }

    fun formatTimestampUs(timestampUs: Long): String {
        val negative = timestampUs < 0L
        val wholeSeconds = abs(timestampUs / 1_000_000L)
        val micros = abs(timestampUs % 1_000_000L)
        val hours = wholeSeconds / 3_600L
        val minutes = (wholeSeconds % 3_600L) / 60L
        val seconds = wholeSeconds % 60L
        val prefix = if (negative) "-" else ""
        return if (hours > 0L) {
            "%s%d:%02d:%02d.%06d".format(prefix, hours, minutes, seconds, micros)
        } else {
            "%s%02d:%02d.%06d".format(prefix, minutes, seconds, micros)
        }
    }

    fun sanitizeSignedTimestampInput(value: String): String = buildString {
        value.forEachIndexed { index, character ->
            if (length >= 20) return@forEachIndexed
            when {
                character.isDigit() -> append(character)
                character == '-' && index == 0 && isEmpty() -> append(character)
            }
        }
    }
}
