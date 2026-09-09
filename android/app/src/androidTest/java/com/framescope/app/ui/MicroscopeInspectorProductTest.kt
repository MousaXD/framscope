package com.framescope.app.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import com.framescope.app.data.FrameDetails
import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.MicroscopeFrame
import com.framescope.app.data.MicroscopeSessionSnapshot
import com.framescope.app.data.PreparedMicroscopeFrame
import com.framescope.app.data.VideoMetadata
import java.nio.ByteBuffer
import org.junit.Rule
import org.junit.Test

class MicroscopeInspectorProductTest {
    @get:Rule
    val composeRule = createComposeRule()

    @Test
    fun workspaceActionOpensVisibleAuthoritativeFrameInspectorAndFullscreen() {
        val details = FrameDetails(
            frameId = 0L,
            timestampTicks = 120L,
            timestampUs = 5_000_000L,
            timeBaseNumerator = 1,
            timeBaseDenominator = 24,
            durationTicks = 1L,
            keyframe = true,
            corrupt = false,
        )
        val session = MicroscopeSessionSnapshot(
            sessionId = 11L,
            frameCount = 2L,
            currentFrame = details,
            canStepPrevious = false,
            canStepNext = true,
        )
        val descriptor = PreparedMicroscopeFrame(
            sessionId = 11L,
            frameId = 0L,
            generation = 1L,
            width = 2,
            height = 2,
            strideBytes = 8L,
            byteLen = 16,
        )
        val rgba = ByteBuffer.allocateDirect(16).apply {
            repeat(4) {
                put(0xff.toByte())
                put(0x40.toByte())
                put(0x20.toByte())
                put(0xff.toByte())
            }
            position(0)
        }.asReadOnlyBuffer()
        val metadata = VideoMetadata(
            durationUs = 10_000_000L,
            width = 2,
            height = 2,
            estimatedFrameRate = 24.0,
            rotationDegrees = 0,
            container = "matroska",
            codec = "h264",
            pixelFormat = "yuv420p",
        )
        val state = FrameScopeUiState(
            engineStatus = EngineStatus.Ready("inspector-test"),
            videoState = VideoInspectionState.Ready(
                InspectedVideo(
                    displayName = "inspector.mkv",
                    metadata = metadata,
                    engine = "inspector-test",
                ),
            ),
            microscopeState = MicroscopeUiState.Ready(
                session = session,
                frame = MicroscopeFrame(descriptor, rgba),
            ),
        )

        composeRule.setContent {
            MaterialTheme {
                FrameScopeScreen(
                    state = state,
                    onOpenVideo = {},
                    onCancelInspection = {},
                    onDismissError = {},
                    onStepMicroscope = {},
                    onJumpMicroscopeFrame = {},
                    onJumpMicroscopeTimestampUs = {},
                    onPreviewMicroscopeFrame = {},
                    onPreviewMicroscopeTimestampUs = {},
                    onFinishMicroscopeScrubFrame = {},
                    onFinishMicroscopeScrubTimestampUs = {},
                    onCommitMicroscopeRange = { _, _ -> },
                    onClearMicroscopeRange = {},
                    recentVideosContent = { Box(Modifier.testTag("recent")) },
                    storageSummaryContent = { Box(Modifier.testTag("storage_summary")) },
                    historyContent = { Box(Modifier.testTag("history")) },
                    storageContent = { Box(Modifier.testTag("storage")) },
                )
            }
        }

        composeRule.onNodeWithTag("nav_workspace").performClick()
        composeRule.onNodeWithTag("inspect_current_frame_action").assertExists().performClick()
        composeRule.onNodeWithTag("inspector_destination").assertExists()
        composeRule.onNodeWithTag("microscope_inspector_workspace").assertExists()
        composeRule.onNodeWithTag("inspector_frame_surface").assertExists()
        composeRule.onNodeWithText("AUTHORITATIVE DECODED FRAME · FULL RESOLUTION").assertExists()
        composeRule.onNodeWithText("Codec h264").assertExists()
        composeRule.onNodeWithText("Source pixel format yuv420p").assertExists()
        composeRule.onNodeWithTag("inspector_one_to_one").assertExists()

        composeRule.onNodeWithTag("inspector_fullscreen_action").performClick()
        composeRule.onNodeWithTag("inspector_fullscreen_surface").assertExists()
    }
}
