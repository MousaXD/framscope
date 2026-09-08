package com.framescope.app.ui

import com.framescope.app.data.BatchExportRequest
import com.framescope.app.data.BatchExportSelection
import com.framescope.app.data.FrameExportFormat
import java.math.BigDecimal
import java.math.RoundingMode

internal enum class ExtractionMode {
    CurrentFrame,
    SelectedTimeline,
    FrameRange,
    TimestampRange,
    AllFrames,
    UniqueGroups,
}

internal data class ExtractionDraft(
    val mode: ExtractionMode,
    val selectedTimelineRange: TimelineRangeSelection?,
    val startFrame: String,
    val endFrame: String,
    val startSeconds: String,
    val endSeconds: String,
    val everyNFrames: String,
    val format: FrameExportFormat,
)

internal object ExtractionRequestFactory {
    fun buildBatchRequest(draft: ExtractionDraft): BatchExportRequest? {
        if (draft.mode == ExtractionMode.CurrentFrame) return null

        val stride = if (draft.mode == ExtractionMode.UniqueGroups) {
            1L
        } else {
            draft.everyNFrames.toLongOrNull()?.takeIf { it > 0L } ?: return null
        }

        val selection = when (draft.mode) {
            ExtractionMode.CurrentFrame -> return null
            ExtractionMode.SelectedTimeline -> {
                val range = draft.selectedTimelineRange ?: return null
                if (range.sessionId <= 0L || range.endUs < range.startUs) return null
                BatchExportSelection.TimestampRangeUsInclusive(range.startUs, range.endUs)
            }
            ExtractionMode.FrameRange -> {
                val start = draft.startFrame.toLongOrNull()?.takeIf { it >= 0L } ?: return null
                val end = draft.endFrame.toLongOrNull()?.takeIf { it >= start } ?: return null
                BatchExportSelection.FrameRangeInclusive(start, end)
            }
            ExtractionMode.TimestampRange -> {
                val start = secondsToMicros(draft.startSeconds) ?: return null
                val end = secondsToMicros(draft.endSeconds)?.takeIf { it >= start } ?: return null
                BatchExportSelection.TimestampRangeUsInclusive(start, end)
            }
            ExtractionMode.AllFrames -> BatchExportSelection.AllFrames
            ExtractionMode.UniqueGroups -> BatchExportSelection.UniqueGroups
        }

        return BatchExportRequest(
            selection = selection,
            everyNFrames = stride,
            format = draft.format,
        ).takeIf(BatchExportRequest::isSane)
    }

    fun secondsToMicros(value: String): Long? = runCatching {
        val seconds = value.trim().takeIf { it.isNotEmpty() }?.let(::BigDecimal) ?: return null
        seconds
            .multiply(MICROS_PER_SECOND)
            .setScale(0, RoundingMode.UNNECESSARY)
            .longValueExact()
    }.getOrNull()

    private val MICROS_PER_SECOND = BigDecimal(1_000_000L)
}
