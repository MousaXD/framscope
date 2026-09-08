package com.framescope.app.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import com.framescope.app.data.FrameDetails
import com.framescope.app.data.MicroscopeSessionSnapshot
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

class MicroscopeTimelineControlsTest {
    @get:Rule
    val composeRule = createComposeRule()

    @Test
    fun indexedTimelineExposesScrubberAndInclusiveRangeSemantics() {
        composeRule.setContent {
            MaterialTheme {
                MicroscopeTimelineControls(
                    session = session(frameId = 1L),
                    timelineBounds = IndexedTimelineBounds(
                        sessionId = SESSION_ID,
                        startUs = 100_000L,
                        endUs = 500_000L,
                    ),
                    rangeSelection = null,
                    enabled = true,
                    onJumpFrame = {},
                    onJumpTimestampUs = {},
                    onCommitRange = { _, _ -> },
                    onClearRange = {},
                )
            }
        }

        composeRule.onNodeWithText("Indexed presentation timeline").assertExists()
        composeRule.onNodeWithTag(TIMELINE_SLIDER_TAG)
            .assertExists()
            .assert(hasContentDescription("Video timeline scrubber"))

        composeRule.onNodeWithText("Select range").performClick()
        composeRule.onNodeWithTag(TIMELINE_RANGE_SLIDER_TAG)
            .assertExists()
            .assert(hasContentDescription("Selected timeline range"))
        composeRule.onNodeWithText(
            "Extraction uses inclusive indexed timestamps: start ≤ frame timestamp ≤ end.",
        ).assertExists()
    }

    @Test
    fun committedRangeIsVisibleAndClearActionIsWired() {
        var cleared = false
        composeRule.setContent {
            MaterialTheme {
                MicroscopeTimelineControls(
                    session = session(frameId = 2L),
                    timelineBounds = IndexedTimelineBounds(
                        sessionId = SESSION_ID,
                        startUs = 100_000L,
                        endUs = 500_000L,
                    ),
                    rangeSelection = TimelineRangeSelection(
                        sessionId = SESSION_ID,
                        startUs = 200_000L,
                        endUs = 400_000L,
                    ),
                    enabled = true,
                    onJumpFrame = {},
                    onJumpTimestampUs = {},
                    onCommitRange = { _, _ -> },
                    onClearRange = { cleared = true },
                )
            }
        }

        composeRule.onNodeWithTag(TIMELINE_RANGE_SLIDER_TAG).assertExists()
        composeRule.onNodeWithText("Clear range").performClick()
        composeRule.runOnIdle { assertTrue(cleared) }
    }

    @Test
    fun missingTimestampBoundsFallsBackToFrameOrderWithoutRangeSelector() {
        composeRule.setContent {
            MaterialTheme {
                MicroscopeTimelineControls(
                    session = session(frameId = 1L),
                    timelineBounds = null,
                    rangeSelection = null,
                    enabled = true,
                    onJumpFrame = {},
                    onJumpTimestampUs = {},
                    onCommitRange = { _, _ -> },
                    onClearRange = {},
                )
            }
        }

        composeRule.onNodeWithText("Presentation-order scrub").assertExists()
        composeRule.onNodeWithTag(TIMELINE_SLIDER_TAG).assertExists()
        composeRule.onNodeWithText("Select range").assertDoesNotExist()
        composeRule.onNodeWithTag(TIMELINE_RANGE_SLIDER_TAG).assertDoesNotExist()
    }

    private fun session(frameId: Long) = MicroscopeSessionSnapshot(
        sessionId = SESSION_ID,
        frameCount = 5L,
        currentFrame = FrameDetails(
            frameId = frameId,
            timestampTicks = frameId * 100L,
            timestampUs = 100_000L + frameId * 100_000L,
            timeBaseNumerator = 1,
            timeBaseDenominator = 1_000,
            durationTicks = 100L,
            keyframe = frameId == 0L,
            corrupt = false,
        ),
        canStepPrevious = frameId > 0L,
        canStepNext = frameId < 4L,
    )

    private companion object {
        const val SESSION_ID = 501L
    }
}
