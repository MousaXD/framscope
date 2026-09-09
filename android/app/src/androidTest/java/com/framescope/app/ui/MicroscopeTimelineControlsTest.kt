package com.framescope.app.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertDoesNotExist
import androidx.compose.ui.test.assertExists
import androidx.compose.ui.test.fetchSemanticsNode
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTouchInput
import com.framescope.app.data.FrameDetails
import com.framescope.app.data.MicroscopeSessionSnapshot
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

class MicroscopeTimelineControlsTest {
    @get:Rule
    val composeRule = createComposeRule()

    @Test
    fun indexedTimelineExposesConsumerScrubberAndRangeAction() {
        composeRule.setContent {
            MaterialTheme {
                controls(session = session(frameId = 1L))
            }
        }

        composeRule.onNodeWithTag(TIMELINE_SLIDER_TAG)
            .assertExists()
            .assert(hasContentDescription("Video timeline scrubber"))
        composeRule.onNodeWithText("Frame 2 / 5").assertExists()
        composeRule.onNodeWithText("‹ Frame").assertExists()
        composeRule.onNodeWithText("Frame ›").assertExists()

        composeRule.onNodeWithText("Range").performClick()
        composeRule.onNodeWithTag(TIMELINE_RANGE_SLIDER_TAG)
            .assertExists()
            .assert(hasContentDescription("Selected timeline range"))
        composeRule.onNodeWithText("Extraction uses inclusive indexed timestamps: start ≤ frame timestamp ≤ end.")
            .assertDoesNotExist()
    }

    @Test
    fun realDragMovesThumbAndDispatchesLivePreview() {
        val previewTimestamps = mutableListOf<Long>()
        composeRule.setContent {
            MaterialTheme {
                controls(
                    session = session(frameId = 0L),
                    onPreviewTimestampUs = { previewTimestamps += it },
                )
            }
        }

        dragTimeline(fromFraction = 0.15f, toFraction = 0.78f)

        composeRule.runOnIdle {
            assertTrue("real pointer drag should dispatch a live preview", previewTimestamps.isNotEmpty())
            assertTrue(
                "drag should move well beyond the stale authoritative start",
                sliderProgress() > 0.60f,
            )
        }
    }

    @Test
    fun realPointerReleaseDispatchesExactTimestampSettle() {
        val finishedTimestamps = mutableListOf<Long>()
        composeRule.setContent {
            MaterialTheme {
                controls(
                    session = session(frameId = 0L),
                    onFinishScrubTimestampUs = { finishedTimestamps += it },
                )
            }
        }

        dragTimeline(fromFraction = 0.20f, toFraction = 0.72f)

        composeRule.runOnIdle {
            assertEquals(1, finishedTimestamps.size)
            assertTrue(finishedTimestamps.single() in 300_000L..450_000L)
        }
    }

    @Test
    fun releaseHoldsThumbWhileAuthoritativeFrameIsStaleThenSynchronizesAfterSettle() {
        var activeSession by mutableStateOf(session(frameId = 0L))
        var exactSettleInProgress by mutableStateOf(false)
        val finishedTimestamps = mutableListOf<Long>()

        composeRule.setContent {
            MaterialTheme {
                controls(
                    session = activeSession,
                    exactSettleInProgress = exactSettleInProgress,
                    onFinishScrubTimestampUs = { finishedTimestamps += it },
                )
            }
        }

        dragTimeline(fromFraction = 0.18f, toFraction = 0.88f)
        val releasedProgress = sliderProgress()
        assertTrue(releasedProgress > 0.70f)
        composeRule.onNodeWithTag(TIMELINE_SETTLING_TAG).assertExists()

        // The parent still exposes the old authoritative frame for a composition. The release
        // position must not be overwritten by that stale fraction.
        composeRule.waitForIdle()
        assertEquals(releasedProgress, sliderProgress(), 0.02f)
        assertEquals(1, finishedTimestamps.size)

        composeRule.runOnIdle { exactSettleInProgress = true }
        composeRule.waitForIdle()
        assertEquals(releasedProgress, sliderProgress(), 0.02f)
        composeRule.onNodeWithTag(TIMELINE_SETTLING_TAG).assertExists()

        // Exact indexed navigation resolves frame 4 at 400 ms, fraction 0.75 in these bounds.
        composeRule.runOnIdle {
            activeSession = session(frameId = 3L)
            exactSettleInProgress = false
        }
        composeRule.waitForIdle()

        assertEquals(0.75f, sliderProgress(), 0.02f)
        composeRule.onNodeWithTag(TIMELINE_SETTLING_TAG).assertDoesNotExist()
        composeRule.onNodeWithText("Frame 4 / 5").assertExists()
    }

    @Test
    fun settlingStateDoesNotPresentOldFramePositionAsCurrentTarget() {
        var exactSettleInProgress by mutableStateOf(false)
        composeRule.setContent {
            MaterialTheme {
                controls(
                    session = session(frameId = 0L),
                    exactSettleInProgress = exactSettleInProgress,
                )
            }
        }

        dragTimeline(fromFraction = 0.20f, toFraction = 0.80f)
        composeRule.onNodeWithTag(TIMELINE_SETTLING_TAG).assertExists()
        composeRule.onNodeWithText("Frame 1 / 5").assertDoesNotExist()

        composeRule.runOnIdle { exactSettleInProgress = true }
        composeRule.waitForIdle()
        composeRule.onNodeWithTag(TIMELINE_SETTLING_TAG).assertExists()
        composeRule.onNodeWithText("Frame 1 / 5").assertDoesNotExist()
    }

    @Test
    fun committedRangeIsVisibleAndClearActionIsWired() {
        var cleared = false
        composeRule.setContent {
            MaterialTheme {
                controls(
                    session = session(frameId = 2L),
                    rangeSelection = TimelineRangeSelection(
                        sessionId = SESSION_ID,
                        startUs = 200_000L,
                        endUs = 400_000L,
                    ),
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
                controls(
                    session = session(frameId = 1L),
                    timelineBounds = null,
                )
            }
        }

        composeRule.onNodeWithTag(TIMELINE_SLIDER_TAG).assertExists()
        composeRule.onNodeWithText("Frame 2 / 5").assertExists()
        composeRule.onNodeWithText("Range").assertDoesNotExist()
        composeRule.onNodeWithTag(TIMELINE_RANGE_SLIDER_TAG).assertDoesNotExist()
    }

    @Composable
    private fun controls(
        session: MicroscopeSessionSnapshot,
        timelineBounds: IndexedTimelineBounds? = IndexedTimelineBounds(
            sessionId = SESSION_ID,
            startUs = 100_000L,
            endUs = 500_000L,
        ),
        rangeSelection: TimelineRangeSelection? = null,
        exactSettleInProgress: Boolean = false,
        onPreviewTimestampUs: (Long) -> Unit = {},
        onFinishScrubTimestampUs: (Long) -> Unit = {},
        onClearRange: () -> Unit = {},
    ) {
        MicroscopeTimelineControls(
            session = session,
            timelineBounds = timelineBounds,
            rangeSelection = rangeSelection,
            enabled = true,
            exactSettleInProgress = exactSettleInProgress,
            onStep = {},
            onPreviewFrame = {},
            onPreviewTimestampUs = onPreviewTimestampUs,
            onFinishScrubFrame = {},
            onFinishScrubTimestampUs = onFinishScrubTimestampUs,
            onCommitRange = { _, _ -> },
            onClearRange = onClearRange,
        )
    }

    private fun dragTimeline(fromFraction: Float, toFraction: Float) {
        composeRule.onNodeWithTag(TIMELINE_SLIDER_TAG).performTouchInput {
            val start = Offset(width * fromFraction, center.y)
            val end = Offset(width * toFraction, center.y)
            down(start)
            moveTo(end)
            up()
        }
        composeRule.waitForIdle()
    }

    private fun sliderProgress(): Float =
        composeRule.onNodeWithTag(TIMELINE_SLIDER_TAG)
            .fetchSemanticsNode()
            .config[SemanticsProperties.ProgressBarRangeInfo]
            .current

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
