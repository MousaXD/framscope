package com.framescope.app.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.test.assertExists
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import com.framescope.app.data.BatchExportProgress
import com.framescope.app.data.BatchExportRequest
import com.framescope.app.data.BatchExportSelection
import com.framescope.app.data.FrameDetails
import com.framescope.app.data.FrameExportFormat
import com.framescope.app.data.MicroscopeFrame
import com.framescope.app.data.MicroscopeSessionSnapshot
import com.framescope.app.data.PreparedMicroscopeFrame
import java.nio.ByteBuffer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Rule
import org.junit.Test

class ExtractionWorkflowTest {
    @get:Rule
    val composeRule = createComposeRule()

    @Test
    fun selectedTimelineOpensOneSheetAndSubmitsExactIndexedRange() {
        var submitted: BatchExportRequest? = null
        val selectedRange = TimelineRangeSelection(
            sessionId = 7,
            startUs = 1_250_001,
            endUs = 4_750_009,
        )

        composeRule.setContent {
            MaterialTheme {
                ExtractionWorkflow(
                    microscopeState = readyState(),
                    selectedTimelineRange = selectedRange,
                    currentFrameState = FrameExportUiState.Idle,
                    batchState = BatchExportUiState.Idle,
                    onRequestCurrentFrame = {},
                    onRequestBatch = { submitted = it },
                    onCancelCurrentFrame = {},
                    onCancelBatch = {},
                    onDismissCurrentFrameStatus = {},
                    onDismissBatchStatus = {},
                )
            }
        }

        composeRule.onNodeWithTag("extract_action").performClick()
        composeRule.onNodeWithTag("extraction_sheet").assertExists()
        composeRule.onNodeWithText("Selected timeline range").assertExists()
        composeRule.onNodeWithTag("extraction_choose_destination").performClick()

        composeRule.runOnIdle {
            val request = submitted
            assertNotNull(request)
            assertEquals(
                BatchExportSelection.TimestampRangeUsInclusive(1_250_001, 4_750_009),
                request?.selection,
            )
        }
    }

    @Test
    fun currentFrameUsesDedicatedCurrentFramePath() {
        var currentFormat: FrameExportFormat? = null
        var batchRequest: BatchExportRequest? = null

        composeRule.setContent {
            MaterialTheme {
                ExtractionWorkflow(
                    microscopeState = readyState(),
                    selectedTimelineRange = null,
                    currentFrameState = FrameExportUiState.Idle,
                    batchState = BatchExportUiState.Idle,
                    onRequestCurrentFrame = { currentFormat = it },
                    onRequestBatch = { batchRequest = it },
                    onCancelCurrentFrame = {},
                    onCancelBatch = {},
                    onDismissCurrentFrameStatus = {},
                    onDismissBatchStatus = {},
                )
            }
        }

        composeRule.onNodeWithTag("extract_action").performClick()
        composeRule.onNodeWithText("Current frame 4").assertExists()
        composeRule.onNodeWithTag("extraction_choose_destination").performClick()

        composeRule.runOnIdle {
            assertEquals(FrameExportFormat.Png, currentFormat)
            assertEquals(null, batchRequest)
        }
    }

    @Test
    fun batchProgressIsInlineAndReportsCurrentTotalPercentAndFrame() {
        val pending = PendingBatchExport(
            sessionId = 7,
            currentFrameIdAtRequest = 3,
            request = BatchExportRequest(
                selection = BatchExportSelection.AllFrames,
                everyNFrames = 1,
                format = FrameExportFormat.Png,
            ),
        )

        composeRule.setContent {
            MaterialTheme {
                ExtractionWorkflow(
                    microscopeState = readyState(),
                    selectedTimelineRange = null,
                    currentFrameState = FrameExportUiState.Idle,
                    batchState = BatchExportUiState.Exporting(
                        pending = pending,
                        progress = BatchExportProgress(frameId = 8, ordinal = 2, total = 4),
                    ),
                    onRequestCurrentFrame = {},
                    onRequestBatch = {},
                    onCancelCurrentFrame = {},
                    onCancelBatch = {},
                    onDismissCurrentFrameStatus = {},
                    onDismissBatchStatus = {},
                )
            }
        }

        composeRule.onNodeWithTag("extraction_status").assertExists()
        composeRule.onNodeWithText("Extracting frames").assertExists()
        composeRule.onNodeWithText("2 / 4 · 50% · frame 9", substring = true).assertExists()
    }

    private fun readyState(): MicroscopeUiState.Ready {
        val details = FrameDetails(
            frameId = 3,
            timestampTicks = 3,
            timestampUs = 1_500_000,
            timeBaseNumerator = 1,
            timeBaseDenominator = 1_000_000,
            durationTicks = 1,
            keyframe = false,
            corrupt = false,
        )
        val session = MicroscopeSessionSnapshot(
            sessionId = 7,
            frameCount = 10,
            currentFrame = details,
            canStepPrevious = true,
            canStepNext = true,
        )
        val descriptor = PreparedMicroscopeFrame(
            sessionId = 7,
            frameId = 3,
            generation = 1,
            width = 1,
            height = 1,
            strideBytes = 4,
            byteLen = 4,
        )
        return MicroscopeUiState.Ready(
            session = session,
            frame = MicroscopeFrame(
                descriptor = descriptor,
                rgba = ByteBuffer.allocateDirect(4),
            ),
        )
    }
}
