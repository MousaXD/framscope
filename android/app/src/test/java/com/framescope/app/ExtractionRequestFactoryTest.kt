package com.framescope.app

import com.framescope.app.data.BatchExportSelection
import com.framescope.app.data.FrameExportFormat
import com.framescope.app.ui.ExtractionDraft
import com.framescope.app.ui.ExtractionMode
import com.framescope.app.ui.ExtractionRequestFactory
import com.framescope.app.ui.TimelineRangeSelection
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ExtractionRequestFactoryTest {
    @Test
    fun selectedTimelineUsesCommittedPresentationTimestamps() {
        val request = ExtractionRequestFactory.buildBatchRequest(
            draft(
                mode = ExtractionMode.SelectedTimeline,
                selectedTimelineRange = TimelineRangeSelection(
                    sessionId = 7,
                    startUs = 1_250_001,
                    endUs = 4_750_009,
                ),
                everyNFrames = "3",
            ),
        )

        assertEquals(
            BatchExportSelection.TimestampRangeUsInclusive(1_250_001, 4_750_009),
            request?.selection,
        )
        assertEquals(3L, request?.everyNFrames)
    }

    @Test
    fun allFramesPreservesEveryNthFrameStride() {
        val request = ExtractionRequestFactory.buildBatchRequest(
            draft(mode = ExtractionMode.AllFrames, everyNFrames = "12"),
        )

        assertEquals(BatchExportSelection.AllFrames, request?.selection)
        assertEquals(12L, request?.everyNFrames)
    }

    @Test
    fun timestampRangeParsesExactDecimalSecondsToMicroseconds() {
        val request = ExtractionRequestFactory.buildBatchRequest(
            draft(
                mode = ExtractionMode.TimestampRange,
                startSeconds = "1.250001",
                endSeconds = "2.000009",
            ),
        )

        assertEquals(
            BatchExportSelection.TimestampRangeUsInclusive(1_250_001, 2_000_009),
            request?.selection,
        )
    }

    @Test
    fun timestampRangeRejectsPrecisionBelowOneMicrosecond() {
        assertNull(ExtractionRequestFactory.secondsToMicros("1.0000001"))
    }

    @Test
    fun frameRangeRequiresInclusiveOrderedIds() {
        assertNull(
            ExtractionRequestFactory.buildBatchRequest(
                draft(mode = ExtractionMode.FrameRange, startFrame = "8", endFrame = "7"),
            ),
        )
        val request = ExtractionRequestFactory.buildBatchRequest(
            draft(mode = ExtractionMode.FrameRange, startFrame = "8", endFrame = "8"),
        )
        assertEquals(BatchExportSelection.FrameRangeInclusive(8, 8), request?.selection)
    }

    @Test
    fun uniqueGroupsForcesStrideOne() {
        val request = ExtractionRequestFactory.buildBatchRequest(
            draft(mode = ExtractionMode.UniqueGroups, everyNFrames = "99"),
        )

        assertEquals(BatchExportSelection.UniqueGroups, request?.selection)
        assertEquals(1L, request?.everyNFrames)
        assertTrue(request?.isSane() == true)
    }

    @Test
    fun currentFrameUsesDedicatedSingleFramePath() {
        assertNull(
            ExtractionRequestFactory.buildBatchRequest(
                draft(mode = ExtractionMode.CurrentFrame),
            ),
        )
    }

    private fun draft(
        mode: ExtractionMode,
        selectedTimelineRange: TimelineRangeSelection? = null,
        startFrame: String = "0",
        endFrame: String = "0",
        startSeconds: String = "0",
        endSeconds: String = "1",
        everyNFrames: String = "1",
        format: FrameExportFormat = FrameExportFormat.Png,
    ) = ExtractionDraft(
        mode = mode,
        selectedTimelineRange = selectedTimelineRange,
        startFrame = startFrame,
        endFrame = endFrame,
        startSeconds = startSeconds,
        endSeconds = endSeconds,
        everyNFrames = everyNFrames,
        format = format,
    )
}
