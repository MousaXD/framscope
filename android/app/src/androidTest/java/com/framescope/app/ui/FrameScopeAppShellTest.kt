package com.framescope.app.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.VideoMetadata
import org.junit.Rule
import org.junit.Test

class FrameScopeAppShellTest {
    @get:Rule
    val composeRule = createComposeRule()

    @Test
    fun topLevelNavigationChangesDestinationWithoutOneGiantPage() {
        composeRule.setContent {
            MaterialTheme {
                var destination by remember { mutableStateOf(FrameScopeDestination.Home) }
                FrameScopeAppShell(
                    currentDestination = destination,
                    onDestinationSelected = { destination = it },
                ) {
                    androidx.compose.material3.Text("content_${destination.route}")
                }
            }
        }

        composeRule.onNodeWithText("content_home").assertExists()
        composeRule.onNodeWithTag("nav_history").performClick()
        composeRule.onNodeWithText("content_home").assertDoesNotExist()
        composeRule.onNodeWithText("content_history").assertExists()
        composeRule.onNodeWithTag("nav_storage").performClick()
        composeRule.onNodeWithText("content_storage").assertExists()
    }

    @Test
    fun technicalVideoMetadataLivesInInspectorNotHome() {
        composeRule.setContent {
            MaterialTheme {
                FrameScopeScreen(
                    state = readyVideoState(),
                    onOpenVideo = {},
                    onCancelInspection = {},
                    onDismissError = {},
                    onStepMicroscope = {},
                    onJumpMicroscopeFrame = {},
                    onJumpMicroscopeTimestampUs = {},
                    onCommitMicroscopeRange = { _, _ -> },
                    onClearMicroscopeRange = {},
                )
            }
        }

        composeRule.onNodeWithTag("home_destination").assertExists()
        composeRule.onNodeWithText("h264").assertDoesNotExist()
        composeRule.onNodeWithText("Frame timing is timestamp-driven. FPS is informational only.")
            .assertDoesNotExist()

        composeRule.onNodeWithTag("nav_inspector").performClick()
        composeRule.onNodeWithTag("inspector_destination").assertExists()
        composeRule.onNodeWithText("h264").assertExists()
        composeRule.onNodeWithText("Frame timing is timestamp-driven. FPS is informational only.")
            .assertExists()
    }

    @Test
    fun historyAndStorageDestinationsExposeIntegrationSurfaces() {
        composeRule.setContent {
            MaterialTheme {
                FrameScopeScreen(
                    state = FrameScopeUiState(engineStatus = EngineStatus.Ready("test-engine")),
                    onOpenVideo = {},
                    onCancelInspection = {},
                    onDismissError = {},
                    onStepMicroscope = {},
                    onJumpMicroscopeFrame = {},
                    onJumpMicroscopeTimestampUs = {},
                    onCommitMicroscopeRange = { _, _ -> },
                    onClearMicroscopeRange = {},
                )
            }
        }

        composeRule.onNodeWithTag("nav_history").performClick()
        composeRule.onNodeWithTag("history_integration_point").assertExists()
        composeRule.onNodeWithTag("nav_storage").performClick()
        composeRule.onNodeWithTag("storage_integration_point").assertExists()
    }

    private fun readyVideoState() = FrameScopeUiState(
        engineStatus = EngineStatus.Ready("framescope-test"),
        videoState = VideoInspectionState.Ready(
            InspectedVideo(
                displayName = "sample-vfr.mp4",
                metadata = VideoMetadata(
                    durationUs = 2_000_000L,
                    width = 1920,
                    height = 1080,
                    estimatedFrameRate = 29.97,
                    rotationDegrees = 0,
                    container = "mov,mp4",
                    codec = "h264",
                    videoStreamIndex = 0,
                    videoStreamCount = 1,
                    audioStreamCount = 1,
                    pixelFormat = "yuv420p",
                    variableFrameRate = true,
                ),
                engine = "framescope-test",
            ),
        ),
    )
}
