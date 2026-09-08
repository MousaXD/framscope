package com.framescope.app.data

sealed interface BatchExportSelection {
    val nativeKind: Int
    val start: Long
    val end: Long

    data class CurrentFrame(
        val frameId: Long,
    ) : BatchExportSelection {
        override val nativeKind: Int = 0
        override val start: Long = frameId
        override val end: Long = 0L
    }

    data class FrameRangeInclusive(
        val startFrameId: Long,
        val endFrameId: Long,
    ) : BatchExportSelection {
        override val nativeKind: Int = 1
        override val start: Long = startFrameId
        override val end: Long = endFrameId
    }

    data class TimestampRangeUsInclusive(
        val startUs: Long,
        val endUs: Long,
    ) : BatchExportSelection {
        override val nativeKind: Int = 2
        override val start: Long = startUs
        override val end: Long = endUs
    }

    data object AllFrames : BatchExportSelection {
        override val nativeKind: Int = 3
        override val start: Long = 0L
        override val end: Long = 0L
    }

    fun isSane(): Boolean = when (this) {
        is CurrentFrame -> frameId >= 0L
        is FrameRangeInclusive -> startFrameId >= 0L && endFrameId >= startFrameId
        is TimestampRangeUsInclusive -> endUs >= startUs
        AllFrames -> true
    }
}

data class BatchExportRequest(
    val selection: BatchExportSelection,
    val everyNFrames: Long = 1L,
    val format: FrameExportFormat,
    val jpegQuality: Int = FrameScopeRepository.DEFAULT_JPEG_QUALITY,
) {
    fun isSane(): Boolean =
        selection.isSane() &&
            everyNFrames > 0L &&
            (format != FrameExportFormat.Jpeg || jpegQuality in 1..100)
}

data class BatchExportResult(
    val sessionId: Long,
    val expectedFrames: Long,
    val committedFrames: Long,
    val encodedBytes: Long,
    val decodedFrames: Long,
    val usedKeyframeSeek: Boolean,
    val fellBackToStreamStart: Boolean,
    val format: FrameExportFormat,
    val mimeType: String,
) {
    fun isSane(): Boolean =
        sessionId > 0L &&
            expectedFrames > 0L &&
            committedFrames == expectedFrames &&
            encodedBytes > 0L &&
            decodedFrames >= committedFrames &&
            mimeType == format.mimeType
}

sealed interface NativeBatchExport {
    data class Success(
        val export: BatchExportResult,
        val engine: String,
    ) : NativeBatchExport

    data class Failure(
        val code: String,
        val message: String,
        val engine: String?,
    ) : NativeBatchExport
}

/**
 * Synchronous callback ABI invoked by Rust while one batch extraction call is active.
 * Implementations must own at most one pending output document at a time.
 */
interface NativeBatchFrameSink {
    fun openFrame(
        fileName: String,
        mimeType: String,
        frameId: Long,
        ordinal: Long,
        total: Long,
    ): Int

    fun commitFrame(
        fileName: String,
        frameId: Long,
        ordinal: Long,
        total: Long,
        byteLength: Long,
    ): Boolean

    fun abortFrame(fileName: String)
}

data class BatchExportProgress(
    val frameId: Long,
    val ordinal: Long,
    val total: Long,
) {
    fun isSane(): Boolean =
        frameId >= 0L &&
            total > 0L &&
            ordinal >= 1L &&
            ordinal <= total
}

data class ExportedBatchDocument(
    val manifestUri: String,
    val manifestDisplayName: String,
    val export: BatchExportResult,
    val workspaceUri: String? = null,
)
