package com.framescope.app.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.test.StateRestorationTester
import androidx.compose.ui.test.assertIsSelected
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.runComposeUiTest
import com.framescope.app.data.FrameDetails
import com.framescope.app.data.MicroscopeFrame
import com.framescope.app.data.MicroscopeSessionSnapshot
import com.framescope.app.data.PreparedMicroscopeFrame
import java.nio.ByteBuffer
import org.junit.Test

class ExtractionWorkflowRestorationTest {
    @Test
    fun openSheetAndSelectedModeRestoreAcrossSavedInstanceState() = runComposeUiTest {
        val restorationTester = StateRestorationTester(this)
        restorationTester.setContent {
            MaterialTheme {
                ExtractionWorkflow(
                    microscopeState = readyState(),
                    selectedTimelineRange = null,
                    currentFrameState = FrameExportUiState.Idle,
                    batchState = BatchExportUiState.Idle,
                    onRequestCurrentFrame = {},
                    onRequestBatch = {},
                    onCancelCurrentFrame = {},
                    onCancelBatch = {},
                    onDismissCurrentFrameStatus = {},
                    onDismissBatchStatus = {},
                )
            }
        }

        onNodeWithTag("extract_action").performClick()
        onNodeWithTag("extract_mode_FrameRange").performClick()
        onNodeWithTag("extract_mode_FrameRange").assertIsSelected()

        restorationTester.emulateSavedInstanceStateRestore()

        onNodeWithTag("extraction_sheet").assertExists()
        onNodeWithTag("extract_mode_FrameRange").assertIsSelected()
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
