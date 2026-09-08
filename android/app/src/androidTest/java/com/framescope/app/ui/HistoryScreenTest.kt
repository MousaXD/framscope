package com.framescope.app.ui

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.junit4.createComposeRule
import com.framescope.app.data.RecentVideoAvailability
import com.framescope.app.data.RecentVideoRecord
import com.framescope.app.data.VideoUriPermissionStatus
import com.framescope.app.ui.theme.FrameScopeTheme
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

class HistoryScreenTest {
    @get:Rule
    val composeRule = createComposeRule()

    @Test
    fun availableRecentVideoShowsResumeAndInvokesOpen() {
        var openedId: String? = null
        composeRule.setContent {
            FrameScopeTheme {
                HistoryScreen(
                    state = HistoryUiState.Ready(
                        listOf(record(lastViewedTimestampUs = 5_000_000L)),
                    ),
                    onRefresh = {},
                    onOpen = { openedId = it.id },
                    onReselect = {},
                    onRemove = {},
                    onClear = {},
                )
            }
        }

        composeRule.onNodeWithText("clip.mp4").assertIsDisplayed()
        composeRule.onNodeWithText("Resume at 0:05").assertIsDisplayed()
        composeRule.onNodeWithText("Resume").assertIsDisplayed().performClick()

        composeRule.runOnIdle {
            assertEquals("record-1", openedId)
        }
    }

    @Test
    fun permissionLostEntryOffersReselectAndRemoveInsteadOfOpen() {
        var reselected = false
        var removedId: String? = null
        val unavailable = record(
            availability = RecentVideoAvailability.PermissionLost,
            permissionStatus = VideoUriPermissionStatus.Lost,
        )
        composeRule.setContent {
            FrameScopeTheme {
                HistoryScreen(
                    state = HistoryUiState.Ready(listOf(unavailable)),
                    onRefresh = {},
                    onOpen = { error("Unavailable entries must not be opened directly") },
                    onReselect = { reselected = true },
                    onRemove = { removedId = it },
                    onClear = {},
                )
            }
        }

        composeRule.onNodeWithText(
            "FrameScope no longer has permission to read this video. Reselect it to restore access.",
        ).assertIsDisplayed()
        composeRule.onNodeWithText("Reselect").performClick()
        composeRule.onNodeWithText("Remove").performClick()

        composeRule.runOnIdle {
            assertTrue(reselected)
            assertEquals("record-1", removedId)
        }
    }

    @Test
    fun emptyHistoryShowsDedicatedEmptyState() {
        composeRule.setContent {
            FrameScopeTheme {
                HistoryScreen(
                    state = HistoryUiState.Ready(emptyList()),
                    onRefresh = {},
                    onOpen = {},
                    onReselect = {},
                    onRemove = {},
                    onClear = {},
                )
            }
        }

        composeRule.onNodeWithText("No recent videos yet").assertIsDisplayed()
    }

    private fun record(
        availability: RecentVideoAvailability = RecentVideoAvailability.Available,
        permissionStatus: VideoUriPermissionStatus = VideoUriPermissionStatus.Persisted,
        lastViewedTimestampUs: Long? = null,
    ) = RecentVideoRecord(
        id = "record-1",
        contentUri = "content://videos/clip",
        permissionStatus = permissionStatus,
        availability = availability,
        displayName = "clip.mp4",
        durationUs = 65_000_000L,
        width = 1920,
        height = 1080,
        codec = "h264",
        container = "mp4",
        lastOpenedEpochMs = 1_000L,
        lastViewedFrameId = if (lastViewedTimestampUs == null) null else 150L,
        lastViewedTimestampUs = lastViewedTimestampUs,
    )
}
