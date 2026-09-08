package com.framescope.app

import com.framescope.app.data.FrameScopeRepository
import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.InspectionProgress
import com.framescope.app.data.MicroscopeIndexingProgress
import com.framescope.app.data.MicroscopeIndexingStage
import com.framescope.app.data.MicroscopeSessionSnapshot
import com.framescope.app.data.VideoMetadata
import com.framescope.app.ui.MainViewModel
import com.framescope.app.ui.MicroscopeUiState
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class IndexingProgressViewModelTest {
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
    fun sourceReplacementRejectsLateProgressAndCancellationClearsVisibleProgress() = runTest(dispatcher) {
        val repository = ProgressRepository()
        val viewModel = MainViewModel(repository)
        dispatcher.scheduler.advanceUntilIdle()

        viewModel.onVideoSelected("content://test/first")
        dispatcher.scheduler.runCurrent()

        assertEquals(MicroscopeUiState.Opening, viewModel.uiState.value.microscopeState)
        assertEquals(100L, viewModel.uiState.value.indexingProgress?.indexedFrames)
        val staleObserver = repository.progressObservers.first()

        viewModel.onVideoSelected("content://test/second")
        dispatcher.scheduler.runCurrent()

        assertEquals(MicroscopeUiState.Opening, viewModel.uiState.value.microscopeState)
        assertEquals(1L, viewModel.uiState.value.indexingProgress?.indexedFrames)
        assertTrue(repository.progressObservers.size >= 2)

        staleObserver(progress(operationId = 1L, frames = 999L, elapsedMs = 5_000L))
        assertEquals(1L, viewModel.uiState.value.indexingProgress?.indexedFrames)

        viewModel.cancelInspection()
        dispatcher.scheduler.advanceUntilIdle()

        assertEquals(MicroscopeUiState.Idle, viewModel.uiState.value.microscopeState)
        assertNull(viewModel.uiState.value.indexingProgress)
    }

    private class ProgressRepository : FrameScopeRepository {
        val progressObservers = mutableListOf<(MicroscopeIndexingProgress) -> Unit>()
        private var openCount = 0L

        override suspend fun engineVersion(): Result<String> = Result.success("framescope-rust/0.1.0")

        override suspend fun inspect(
            uri: String,
            onProgress: (InspectionProgress) -> Unit,
        ): Result<InspectedVideo> {
            onProgress(InspectionProgress.Opening)
            onProgress(InspectionProgress.Inspecting)
            return Result.success(inspectedVideo(uri.substringAfterLast('/')))
        }

        override suspend fun openMicroscope(
            uri: String,
            onIndexingProgress: (MicroscopeIndexingProgress) -> Unit,
        ): Result<MicroscopeSessionSnapshot> {
            openCount += 1L
            progressObservers += onIndexingProgress
            onIndexingProgress(
                progress(
                    operationId = openCount,
                    frames = if (openCount == 1L) 100L else 1L,
                    elapsedMs = if (openCount == 1L) 1_500L else 100L,
                ),
            )
            awaitCancellation()
        }

        override suspend fun closeMicroscope(): Boolean = true
    }

    companion object {
        private fun progress(
            operationId: Long,
            frames: Long,
            elapsedMs: Long,
        ) = MicroscopeIndexingProgress(
            operationId = operationId,
            stage = MicroscopeIndexingStage.Indexing,
            indexedFrames = frames,
            reusedFrames = 0L,
            expectedReuseFrames = 0L,
            firstTimestampUs = 0L,
            currentTimestampUs = frames * 100_000L,
            elapsedMs = elapsedMs,
        )

        private fun inspectedVideo(name: String) = InspectedVideo(
            displayName = "$name.mp4",
            metadata = VideoMetadata(
                durationUs = 60_000_000L,
                width = 1280,
                height = 720,
                estimatedFrameRate = 30.0,
                rotationDegrees = 0,
                container = "mp4",
                codec = "h264",
                videoStreamIndex = 0,
                videoStreamCount = 1,
                audioStreamCount = 0,
                pixelFormat = "yuv420p",
                variableFrameRate = true,
            ),
            engine = "framescope-rust/0.1.0",
        )
    }
}
