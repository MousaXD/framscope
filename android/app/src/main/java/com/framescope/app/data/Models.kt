package com.framescope.app.data

data class VideoMetadata(
    val durationUs: Long,
    val width: Int,
    val height: Int,
    val estimatedFrameRate: Double?,
    val rotationDegrees: Int,
) {
    fun isSane(): Boolean =
        durationUs > 0 &&
            width in 1..65_535 &&
            height in 1..65_535 &&
            rotationDegrees in setOf(0, 90, 180, 270) &&
            (estimatedFrameRate == null ||
                (estimatedFrameRate.isFinite() && estimatedFrameRate > 0.0 && estimatedFrameRate <= 1_000.0))
}

data class InspectedVideo(
    val displayName: String,
    val metadata: VideoMetadata,
    val engine: String,
)

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
