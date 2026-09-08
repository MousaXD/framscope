package com.framescope.app.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.test.assertDoesNotExist
import androidx.compose.ui.test.assertExists
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import com.framescope.app.data.MicroscopeIndexingStage
import org.junit.Rule
import org.junit.Test

class MicroscopeIndexingProgressCardTest {
    @get:Rule
    val composeRule = createComposeRule()

    @Test
    fun validationShowsExactStagePercentageAndVerifiedCounts() {
        composeRule.setContent {
            MaterialTheme {
                MicroscopeIndexingCard(
                    progress = ui(
                        stage = MicroscopeIndexingStage.ValidatingExistingIndex,
                        indexedFrames = 8_000L,
                        reusedFrames = 4_000L,
                        expectedReuseFrames = 8_000L,
                        stageProgressFraction = 0.5,
                    ),
                )
            }
        }

        composeRule.onNodeWithText("Validating saved index: 50%").assertExists()
        composeRule.onNodeWithText("4000 / 8000 saved frames verified").assertExists()
        composeRule.onNodeWithText("Media timeline covered: 50%").assertDoesNotExist()
    }

    @Test
    fun indexingLabelsTimelineCoverageAndNeverRoundsNinetyNinePointSixToComplete() {
        composeRule.setContent {
            MaterialTheme {
                MicroscopeIndexingCard(
                    progress = ui(
                        stage = MicroscopeIndexingStage.Indexing,
                        indexedFrames = 18_240L,
                        currentTimestampUs = 9_960_000L,
                        maxPresentationTimestampUs = 9_960_000L,
                        mediaTimelineFraction = 0.996,
                    ),
                )
            }
        }

        composeRule.onNodeWithText("Indexing frames").assertExists()
        composeRule.onNodeWithText("18240 frames indexed").assertExists()
        composeRule.onNodeWithText("Media timeline covered: 99%").assertExists()
        composeRule.onNodeWithText("Media timeline covered: 100%").assertDoesNotExist()
        composeRule.onNodeWithText("Estimating time remaining…").assertExists()
    }

    @Test
    fun confidentEtaUsesCoarseRangeLanguage() {
        composeRule.setContent {
            MaterialTheme {
                MicroscopeIndexingCard(
                    progress = ui(
                        stage = MicroscopeIndexingStage.Indexing,
                        indexedFrames = 18_240L,
                        mediaTimelineFraction = 0.63,
                        estimatedRemainingRange = IndexingEtaRange(120L, 180L),
                    ),
                )
            }
        }

        composeRule.onNodeWithText("Media timeline covered: 63%").assertExists()
        composeRule.onNodeWithText("About 2–3 min remaining").assertExists()
    }

    @Test
    fun finalizingIsIndeterminateAndNeverShowsTimelinePercentOrEta() {
        composeRule.setContent {
            MaterialTheme {
                MicroscopeIndexingCard(
                    progress = ui(
                        stage = MicroscopeIndexingStage.Finalizing,
                        indexedFrames = 20_000L,
                    ),
                )
            }
        }

        composeRule.onNodeWithText("Finalizing frame index…").assertExists()
        composeRule.onNodeWithText("Media timeline covered: 100%").assertDoesNotExist()
        composeRule.onNodeWithText("Estimating time remaining…").assertDoesNotExist()
    }

    @Test
    fun telemetryLossIsExplicitAndHidesFreshnessDependentEta() {
        composeRule.setContent {
            MaterialTheme {
                MicroscopeIndexingCard(
                    progress = ui(
                        stage = MicroscopeIndexingStage.Indexing,
                        indexedFrames = 10_000L,
                        mediaTimelineFraction = 0.63,
                        telemetryAvailable = false,
                    ),
                )
            }
        }

        composeRule.onNodeWithText("Progress details temporarily unavailable").assertExists()
        composeRule.onNodeWithText("Estimating time remaining…").assertDoesNotExist()
    }

    private fun ui(
        stage: MicroscopeIndexingStage,
        indexedFrames: Long = 0L,
        reusedFrames: Long = 0L,
        expectedReuseFrames: Long = 0L,
        currentTimestampUs: Long? = null,
        maxPresentationTimestampUs: Long? = currentTimestampUs,
        stageProgressFraction: Double? = null,
        mediaTimelineFraction: Double? = null,
        estimatedRemainingRange: IndexingEtaRange? = null,
        telemetryAvailable: Boolean = true,
    ) = IndexingProgressUi(
        stage = stage,
        indexedFrames = indexedFrames,
        reusedFrames = reusedFrames,
        expectedReuseFrames = expectedReuseFrames,
        currentTimestampUs = currentTimestampUs,
        maxPresentationTimestampUs = maxPresentationTimestampUs,
        elapsedMs = 1_000L,
        framesPerSecond = null,
        stageProgressFraction = stageProgressFraction,
        mediaTimelineFraction = mediaTimelineFraction,
        estimatedRemainingRange = estimatedRemainingRange,
        telemetryAvailable = telemetryAvailable,
    )
}
