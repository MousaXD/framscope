package com.framescope.app.ui

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.junit4.createComposeRule
import com.framescope.app.data.FrameScopeStorageStats
import com.framescope.app.data.StorageCategoryStats
import com.framescope.app.data.StorageClearScope
import com.framescope.app.ui.theme.FrameScopeTheme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test

class StorageSettingsContentTest {
    @get:Rule
    val composeRule = createComposeRule()

    @Test
    fun storageSurfaceReportsDisabledDiskProxyAndConfirmsDestructiveClear() {
        var requestedScope: StorageClearScope? = null
        composeRule.setContent {
            FrameScopeTheme {
                StorageSettingsContent(
                    state = StorageUiState(
                        storage = FrameScopeStorageStats(
                            totalBytes = 12_288L,
                            persistentIndexes = StorageCategoryStats(
                                bytes = 12_288L,
                                files = 3L,
                                items = 1L,
                            ),
                            previewProxy = StorageCategoryStats(
                                bytes = 0L,
                                files = 0L,
                                items = 0L,
                            ),
                            disposable = StorageCategoryStats(
                                bytes = 0L,
                                files = 0L,
                                items = 0L,
                            ),
                            indexedSources = 1L,
                            previewProxyEnabled = false,
                        ),
                    ),
                    onRefresh = {},
                    onRequestClear = { requestedScope = it },
                    onConfirmClear = {},
                    onDismissClear = {},
                    onDismissStatus = {},
                )
            }
        }

        composeRule.onNodeWithTag("storage-settings").assertIsDisplayed()
        composeRule.onNodeWithText(
            "Disk previews disabled. The current native proxy budget is 0 B.",
        ).assertIsDisplayed()
        composeRule.onNodeWithText("1 indexed video.").assertIsDisplayed()

        composeRule.onNodeWithTag("clear-all-cache").performClick()
        composeRule.runOnIdle {
            assertEquals(StorageClearScope.All, requestedScope)
        }
    }

    @Test
    fun pendingClearShowsExplicitConfirmation() {
        composeRule.setContent {
            FrameScopeTheme {
                StorageSettingsContent(
                    state = StorageUiState(
                        storage = FrameScopeStorageStats(
                            totalBytes = 0L,
                            persistentIndexes = StorageCategoryStats(0L, 0L, 0L),
                            previewProxy = StorageCategoryStats(0L, 0L, 0L),
                            disposable = StorageCategoryStats(0L, 0L, 0L),
                            indexedSources = 0L,
                            previewProxyEnabled = false,
                        ),
                        pendingClear = StorageClearScope.All,
                    ),
                    onRefresh = {},
                    onRequestClear = {},
                    onConfirmClear = {},
                    onDismissClear = {},
                    onDismissStatus = {},
                )
            }
        }

        composeRule.onNodeWithText("Clear all FrameScope cache?").assertIsDisplayed()
        composeRule.onNodeWithTag("confirm-storage-clear").assertIsDisplayed()
        composeRule.onNodeWithText(
            "Persistent indexes, preview cache, and other disposable FrameScope cache data will be deleted. Video files and history records are not affected.",
        ).assertIsDisplayed()
    }
}
