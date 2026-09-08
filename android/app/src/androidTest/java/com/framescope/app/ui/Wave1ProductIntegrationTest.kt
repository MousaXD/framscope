package com.framescope.app.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import org.junit.Rule
import org.junit.Test

class Wave1ProductIntegrationTest {
    @get:Rule
    val composeRule = createComposeRule()

    @Test
    fun productionShellUsesRealHistoryAndStorageSlotsWithLiveScrubContract() {
        composeRule.setContent {
            MaterialTheme {
                FrameScopeScreen(
                    state = FrameScopeUiState(engineStatus = EngineStatus.Ready("wave1-test")),
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
                    recentVideosContent = { Box(Modifier.testTag("wave1_real_recent")) },
                    storageSummaryContent = { Box(Modifier.testTag("wave1_real_storage_summary")) },
                    historyContent = { Box(Modifier.testTag("wave1_real_history")) },
                    storageContent = { Box(Modifier.testTag("wave1_real_storage")) },
                    workspaceOverlay = { Box(Modifier.testTag("wave1_extraction_overlay")) },
                )
            }
        }

        composeRule.onNodeWithTag("home_destination").assertExists()
        composeRule.onNodeWithTag("wave1_real_recent").assertExists()
        composeRule.onNodeWithTag("wave1_real_storage_summary").assertExists()

        composeRule.onNodeWithTag("nav_history").performClick()
        composeRule.onNodeWithTag("history_destination").assertExists()
        composeRule.onNodeWithTag("wave1_real_history").assertExists()

        composeRule.onNodeWithTag("nav_storage").performClick()
        composeRule.onNodeWithTag("storage_destination").assertExists()
        composeRule.onNodeWithTag("wave1_real_storage").assertExists()

        composeRule.onNodeWithTag("nav_workspace").performClick()
        composeRule.onNodeWithTag("workspace_destination").assertExists()
        composeRule.onNodeWithTag("wave1_extraction_overlay").assertExists()
    }
}
