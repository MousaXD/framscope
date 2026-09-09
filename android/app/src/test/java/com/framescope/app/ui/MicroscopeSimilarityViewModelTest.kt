package com.framescope.app.ui

import com.framescope.app.data.MicroscopeSimilarityMatch
import com.framescope.app.data.MicroscopeSimilarityRepository
import com.framescope.app.data.MicroscopeSimilarityResult
import com.framescope.app.data.SimilarityStoreDisposition
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class MicroscopeSimilarityViewModelTest {
    private val dispatcher = StandardTestDispatcher()

    @Before
    fun setUp() {
        Dispatchers.setMain(dispatcher)
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
    }

    @Test
    fun sameSessionNavigationRetainsResultsUntilNewAuthoritativeFrameThenClearsThem() =
        runTest(dispatcher) {
            val repository = FakeSimilarityRepository(
                search = { sessionId, targetFrameId ->
                    Result.success(result(sessionId, targetFrameId))
                },
            )
            val viewModel = MicroscopeSimilarityViewModel(repository)

            viewModel.findSimilarFrames(sessionId = 7L, targetFrameId = 3L)
            dispatcher.scheduler.advanceUntilIdle()
            assertTrue(viewModel.state.value is MicroscopeSimilarityUiState.Ready)

            // Navigating intentionally retains the previous authoritative frame. Null target means
            // the replacement source-quality presentation is not authoritative yet.
            viewModel.onAuthoritativeTargetChanged(sessionId = 7L, targetFrameId = null)
            assertTrue(viewModel.state.value is MicroscopeSimilarityUiState.Ready)

            viewModel.onAuthoritativeTargetChanged(sessionId = 7L, targetFrameId = 4L)

            assertEquals(MicroscopeSimilarityUiState.Idle, viewModel.state.value)
            assertTrue(repository.cancelCalls >= 2)
        }

    @Test
    fun changedAuthoritativeFrameCancelsAnActiveSearch() = runTest(dispatcher) {
        var coroutineCancelled = false
        val repository = FakeSimilarityRepository(
            search = { _, _ ->
                try {
                    awaitCancellation()
                } finally {
                    coroutineCancelled = true
                }
            },
        )
        val viewModel = MicroscopeSimilarityViewModel(repository)

        viewModel.findSimilarFrames(sessionId = 7L, targetFrameId = 3L)
        dispatcher.scheduler.runCurrent()
        assertTrue(viewModel.state.value is MicroscopeSimilarityUiState.Searching)

        viewModel.onAuthoritativeTargetChanged(sessionId = 7L, targetFrameId = 5L)
        dispatcher.scheduler.advanceUntilIdle()

        assertEquals(MicroscopeSimilarityUiState.Idle, viewModel.state.value)
        assertTrue(repository.cancelCalls >= 2)
        assertTrue(coroutineCancelled)
    }

    @Test
    fun sourceReplacementClearsResultsEvenBeforeANewFrameIsReady() = runTest(dispatcher) {
        val repository = FakeSimilarityRepository(
            search = { sessionId, targetFrameId ->
                Result.success(result(sessionId, targetFrameId))
            },
        )
        val viewModel = MicroscopeSimilarityViewModel(repository)

        viewModel.findSimilarFrames(sessionId = 7L, targetFrameId = 3L)
        dispatcher.scheduler.advanceUntilIdle()
        assertTrue(viewModel.state.value is MicroscopeSimilarityUiState.Ready)

        viewModel.onAuthoritativeTargetChanged(sessionId = null, targetFrameId = null)

        assertEquals(MicroscopeSimilarityUiState.Idle, viewModel.state.value)
    }

    private fun result(sessionId: Long, targetFrameId: Long) = MicroscopeSimilarityResult(
        sessionId = sessionId,
        targetFrameId = targetFrameId,
        descriptorCount = 10L,
        candidateCount = 9L,
        matchedCount = 1L,
        truncated = false,
        minimumSimilarity = 9_300,
        disposition = SimilarityStoreDisposition.Reused,
        matches = listOf(
            MicroscopeSimilarityMatch(
                frameId = if (targetFrameId == 4L) 5L else 4L,
                similarity = 9_800,
            ),
        ),
    )

    private class FakeSimilarityRepository(
        private val search: suspend (Long, Long) -> Result<MicroscopeSimilarityResult>,
    ) : MicroscopeSimilarityRepository {
        var cancelCalls = 0

        override suspend fun findSimilarFrames(
            sessionId: Long,
            targetFrameId: Long,
        ): Result<MicroscopeSimilarityResult> = search(sessionId, targetFrameId)

        override fun cancelActiveSearch() {
            cancelCalls += 1
        }
    }
}
