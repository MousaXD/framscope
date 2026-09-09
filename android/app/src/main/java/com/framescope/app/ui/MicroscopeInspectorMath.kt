package com.framescope.app.ui

import com.framescope.app.data.PreparedMicroscopeFrame
import java.nio.ByteBuffer
import kotlin.math.max
import kotlin.math.min

internal data class InspectorViewportPlan(
    val fitScale: Float,
    val oneToOneScale: Float,
    val minimumScale: Float,
    val maximumScale: Float,
)

internal data class InspectorRgbaSample(
    val x: Int,
    val y: Int,
    val red: Int,
    val green: Int,
    val blue: Int,
    val alpha: Int,
)

internal object MicroscopeInspectorMath {
    private const val DEFAULT_MAX_RELATIVE_ZOOM = 8f
    private const val ONE_TO_ONE_OVERSCAN_MULTIPLIER = 4f

    fun viewportPlan(
        imageWidth: Int,
        imageHeight: Int,
        viewportWidth: Int,
        viewportHeight: Int,
    ): InspectorViewportPlan? {
        if (imageWidth <= 0 || imageHeight <= 0 || viewportWidth <= 0 || viewportHeight <= 0) {
            return null
        }
        val fitScale = min(
            viewportWidth.toFloat() / imageWidth.toFloat(),
            viewportHeight.toFloat() / imageHeight.toFloat(),
        )
        if (!fitScale.isFinite() || fitScale <= 0f) return null
        val oneToOneScale = 1f / fitScale
        if (!oneToOneScale.isFinite() || oneToOneScale <= 0f) return null
        return InspectorViewportPlan(
            fitScale = fitScale,
            oneToOneScale = oneToOneScale,
            minimumScale = min(1f, oneToOneScale),
            maximumScale = max(
                DEFAULT_MAX_RELATIVE_ZOOM,
                oneToOneScale * ONE_TO_ONE_OVERSCAN_MULTIPLIER,
            ),
        )
    }

    fun clampTranslation(
        translationX: Float,
        translationY: Float,
        relativeScale: Float,
        imageWidth: Int,
        imageHeight: Int,
        viewportWidth: Int,
        viewportHeight: Int,
    ): Pair<Float, Float> {
        val plan = viewportPlan(imageWidth, imageHeight, viewportWidth, viewportHeight)
            ?: return 0f to 0f
        val safeScale = relativeScale
            .takeIf { it.isFinite() }
            ?.coerceIn(plan.minimumScale, plan.maximumScale)
            ?: 1f.coerceIn(plan.minimumScale, plan.maximumScale)
        val renderedWidth = imageWidth.toFloat() * plan.fitScale * safeScale
        val renderedHeight = imageHeight.toFloat() * plan.fitScale * safeScale
        val maxX = max(0f, (renderedWidth - viewportWidth.toFloat()) / 2f)
        val maxY = max(0f, (renderedHeight - viewportHeight.toFloat()) / 2f)
        val safeX = translationX.takeIf(Float::isFinite) ?: 0f
        val safeY = translationY.takeIf(Float::isFinite) ?: 0f
        return safeX.coerceIn(-maxX, maxX) to safeY.coerceIn(-maxY, maxY)
    }

    fun sampleRgba(
        descriptor: PreparedMicroscopeFrame,
        rgba: ByteBuffer,
        x: Int,
        y: Int,
    ): InspectorRgbaSample? {
        if (!descriptor.isSane() || x !in 0 until descriptor.width || y !in 0 until descriptor.height) {
            return null
        }
        val offsetLong = runCatching {
            Math.addExact(
                Math.multiplyExact(y.toLong(), descriptor.strideBytes),
                Math.multiplyExact(x.toLong(), 4L),
            )
        }.getOrNull() ?: return null
        if (offsetLong < 0L || offsetLong + 3L >= rgba.capacity().toLong()) return null
        val offset = runCatching { Math.toIntExact(offsetLong) }.getOrNull() ?: return null
        return InspectorRgbaSample(
            x = x,
            y = y,
            red = rgba.get(offset).toInt() and 0xff,
            green = rgba.get(offset + 1).toInt() and 0xff,
            blue = rgba.get(offset + 2).toInt() and 0xff,
            alpha = rgba.get(offset + 3).toInt() and 0xff,
        )
    }
}
