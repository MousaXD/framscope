package com.framescope.app.data

import java.nio.ByteBuffer

data class VideoMetadata(
    val durationUs: Long?,
    val width: Int,
    val height: Int,
    val estimatedFrameRate: Double?,
    val rotationDegrees: Int,
    val container: String? = null,
    val codec: String? = null,
    val videoStreamIndex: Int? = null,
    val videoStreamCount: Int? = null,
    val audioStreamCount: Int? = null,
    val pixelFormat: String? = null,
    val variableFrameRate: Boolean? = null,
) {
    fun isSane(): Boolean =
        (durationUs == null || durationUs > 0) &&
            width in 1..65_535 &&
            height in 1..65_535 &&
            rotationDegrees in setOf(0, 90, 180, 270) &&
            (estimatedFrameRate == null ||
                (estimatedFrameRate.isFinite() && estimatedFrameRate > 0.0 && estimatedFrameRate <= 1_000.0)) &&
            (videoStreamIndex == null || videoStreamIndex >= 0) &&
            (videoStreamCount == null || videoStreamCount > 0) &&
            (audioStreamCount == null || audioStreamCount >= 0) &&
            listOf(container, codec, pixelFormat).all { value -> value == null || value.length <= 256 }
}

data class InspectedVideo(
    val displayName: String,
    val metadata: VideoMetadata,
    val engine: String,
)

data class FrameDetails(
    val frameId: Long,
    val timestampTicks: Long?,
    val timestampUs: Long?,
    val timeBaseNumerator: Int,
    val timeBaseDenominator: Int,
    val durationTicks: Long?,
    val keyframe: Boolean,
    val corrupt: Boolean,
) {
    fun isSane(): Boolean =
        frameId >= 0L &&
            timeBaseNumerator > 0 &&
            timeBaseDenominator > 0 &&
            (durationTicks == null || durationTicks > 0L)
}

data class MicroscopeSessionSnapshot(
    val sessionId: Long,
    val frameCount: Long,
    val currentFrame: FrameDetails?,
    val canStepPrevious: Boolean,
    val canStepNext: Boolean,
) {
    fun isSane(): Boolean {
        if (sessionId <= 0L || frameCount < 0L) return false
        if ((currentFrame == null) != (frameCount == 0L)) return false
        val frame = currentFrame ?: return !canStepPrevious && !canStepNext
        if (!frame.isSane() || frame.frameId >= frameCount) return false
        val expectedPrevious = frame.frameId > 0L
        val expectedNext = frame.frameId < frameCount - 1L
        return canStepPrevious == expectedPrevious && canStepNext == expectedNext
    }
}

/**
 * One caller-owned, source-quality RGBA microscope frame.
 *
 * This deliberately is not a data class: ByteBuffer equality may compare remaining pixel content,
 * which would turn ordinary UI state comparisons into O(frame-size) work for large frames.
 */
class MicroscopeFrame(
    val descriptor: PreparedMicroscopeFrame,
    val rgba: ByteBuffer,
)

class MicroscopeOperationException(
    val code: String,
    message: String,
) : IllegalStateException(message)

enum class TimestampSelectionPolicy(
    val nativeValue: Int,
) {
    AtOrBefore(0),
    AtOrAfter(1),
    Nearest(2),
}

enum class FrameExportFormat(
    val nativeValue: Int,
    val wireName: String,
    val mimeType: String,
    val extension: String,
) {
    Png(0, "png", "image/png", "png"),
    Jpeg(1, "jpeg", "image/jpeg", "jpg"),
    WebPLossless(2, "webp_lossless", "image/webp", "webp"),
    ;

    companion object {
        fun fromWireName(value: String): FrameExportFormat? =
            values().firstOrNull { format -> format.wireName == value }
    }
}

data class FrameExportResult(
    val sessionId: Long,
    val frameId: Long,
    val width: Int,
    val height: Int,
    val format: FrameExportFormat,
    val mimeType: String,
    val byteLength: Long,
) {
    fun isSane(): Boolean =
        sessionId > 0L &&
            frameId >= 0L &&
            width in 1..65_535 &&
            height in 1..65_535 &&
            mimeType == format.mimeType &&
            byteLength > 0L
}

sealed interface NativeInspection {
    data class Success(
        val metadata: VideoMetadata,
        val engine: String,
    ) : NativeInspection

    data class Failure(
        val code: String,
        val message: String,
        val engine: String?,
    ) : NativeInspection
}

sealed interface NativeMicroscope {
    data class Success(
        val session: MicroscopeSessionSnapshot,
        val engine: String,
    ) : NativeMicroscope

    data class Failure(
        val code: String,
        val message: String,
        val engine: String?,
    ) : NativeMicroscope
}

sealed interface NativeFrameExport {
    data class Success(
        val export: FrameExportResult,
        val engine: String,
    ) : NativeFrameExport

    data class Failure(
        val code: String,
        val message: String,
        val engine: String?,
    ) : NativeFrameExport
}

enum class InspectionProgress {
    Opening,
    Inspecting,
}

enum class VideoOpenErrorKind {
    InvalidUri,
    UnreadableUri,
    PermissionRevoked,
    UnsupportedVideo,
    NoVideoTrack,
    CorruptMedia,
    DecoderFailure,
    NativeFailure,
}

class VideoOpenException(
    val kind: VideoOpenErrorKind,
    message: String,
    val diagnostic: String? = null,
) : IllegalStateException(message)
