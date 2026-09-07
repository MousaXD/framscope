package com.framescope.app

import com.framescope.app.data.FrameScopeRepository
import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.InspectionProgress
import com.framescope.app.data.VideoMetadata
import com.framescope.app.data.VideoOpenErrorKind
import com.framescope.app.data.VideoOpenException
import com.framescope.app.ui.EngineStatus
import com.framescope.app.ui.MainViewModel
import com.framescope.app.ui.VideoInspectionState
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
class MainViewModelTest {
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
    fun successfulInspectionPublishesRealMetadata() = runTest(dispatcher) {
        val expected = inspectedVideo()
        val viewModel = MainViewModel(
            FakeRepository(
                inspectBlock = { _, progress ->
                    progress(InspectionProgress.Opening)
                    progress(InspectionProgress.Inspecting)
                    Result.success(expected)
                },
            ),
        )

        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/video")
        dispatcher.scheduler.advanceUntilIdle()

        assertEquals(VideoInspectionState.Ready(expected), viewModel.uiState.value.videoState)
        assertEquals(EngineStatus.Ready(expected.engine), viewModel.uiState.value.engineStatus)
    }

    @Test
    fun inspectionFailureBecomesTypedUserVisibleState() = runTest(dispatcher) {
        val error = VideoOpenException(
            kind = VideoOpenErrorKind.UnsupportedVideo,
            message = "This video format is unsupported.",
            diagnostic = "unsupported_codec: demo",
        )
        val viewModel = MainViewModel(
            FakeRepository(inspectBlock = { _, _ -> Result.failure(error) }),
        )

        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/bad")
        dispatcher.scheduler.advanceUntilIdle()

        assertEquals(
            VideoInspectionState.Error(
                message = "This video format is unsupported.",
                diagnostic = "unsupported_codec: demo",
            ),
            viewModel.uiState.value.videoState,
        )
    }

    @Test
    fun engineLoadFailureIsVisibleWithoutPretendingReady() = runTest(dispatcher) {
        val viewModel = MainViewModel(
            FakeRepository(
                engineResult = Result.failure(IllegalStateException("native library missing")),
            ),
        )

        dispatcher.scheduler.advanceUntilIdle()

        assertEquals(
            EngineStatus.Unavailable("native library missing"),
            viewModel.uiState.value.engineStatus,
        )
    }

    @Test
    fun pickerCancellationHasFirstClassCancelledState() = runTest(dispatcher) {
        val viewModel = MainViewModel(FakeRepository())
        dispatcher.scheduler.advanceUntilIdle()

        viewModel.onPickerStarted()
        assertEquals(VideoInspectionState.Picking, viewModel.uiState.value.videoState)

        viewModel.onPickerCancelled()
        assertEquals(VideoInspectionState.Cancelled, viewModel.uiState.value.videoState)
    }

    @Test
    fun cancellingLongInspectionCancelsRepositoryAndSuppressesLateState() = runTest(dispatcher) {
        var repositoryWasCancelled = false
        val viewModel = MainViewModel(
            FakeRepository(
                inspectBlock = { _, progress ->
                    progress(InspectionProgress.Opening)
                    progress(InspectionProgress.Inspecting)
                    try {
                        awaitCancellation()
                    } finally {
                        repositoryWasCancelled = true
                    }
                },
            ),
        )

        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/slow")
        dispatcher.scheduler.runCurrent()
        assertEquals(VideoInspectionState.Inspecting, viewModel.uiState.value.videoState)

        viewModel.cancelInspection()
        dispatcher.scheduler.advanceUntilIdle()

        assertTrue(repositoryWasCancelled)
        assertEquals(VideoInspectionState.Cancelled, viewModel.uiState.value.videoState)
    }

    private fun inspectedVideo() = InspectedVideo(
        displayName = "clip.mp4",
        metadata = VideoMetadata(
            durationUs = 2_000_000,
            width = 1280,
            height = 720,
            estimatedFrameRate = 30.0,
            rotationDegrees = 0,
            container = "mov,mp4,m4a,3gp,3g2,mj2",
            codec = "h264",
            videoStreamIndex = 0,
            videoStreamCount = 1,
            audioStreamCount = 1,
            pixelFormat = "yuv420p",
            variableFrameRate = false,
        ),
        engine = "framescope-rust/0.1.0",
    )

    private class FakeRepository(
        private val inspectBlock: suspend (
            String,
            (InspectionProgress) -> Unit,
        ) -> Result<InspectedVideo> = { _, _ -> Result.failure(IllegalStateException("unused")) },
        private val engineResult: Result<String> = Result.success("framescope-rust/0.1.0"),
    ) : FrameScopeRepository {
        override suspend fun engineVersion(): Result<String> = engineResult

        override suspend fun inspect(
            uri: String,
            onProgress: (InspectionProgress) -> Unit,
        ): Result<InspectedVideo> = inspectBlock(uri, onProgress)
    }
}
