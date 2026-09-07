package com.framescope.app

import com.framescope.app.data.FrameScopeRepository
import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.VideoMetadata
import com.framescope.app.ui.MainViewModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
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
        val expected = InspectedVideo(
            displayName = "clip.mp4",
            metadata = VideoMetadata(
                durationUs = 2_000_000,
                width = 1280,
                height = 720,
                estimatedFrameRate = 30.0,
                rotationDegrees = 0,
            ),
            engine = "framescope-rust/0.1.0",
        )
        val viewModel = MainViewModel(FakeRepository(inspectResult = Result.success(expected)))

        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/video")
        dispatcher.scheduler.advanceUntilIdle()

        assertEquals(expected, viewModel.uiState.value.video)
        assertFalse(viewModel.uiState.value.isInspecting)
        assertNull(viewModel.uiState.value.errorMessage)
    }

    @Test
    fun inspectionFailureBecomesUserVisibleState() = runTest(dispatcher) {
        val viewModel = MainViewModel(
            FakeRepository(inspectResult = Result.failure(IllegalStateException("Unsupported video"))),
        )

        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/bad")
        dispatcher.scheduler.advanceUntilIdle()

        assertEquals("Unsupported video", viewModel.uiState.value.errorMessage)
        assertFalse(viewModel.uiState.value.isInspecting)
    }

    @Test
    fun engineLoadFailureIsVisibleWithoutPretendingReady() = runTest(dispatcher) {
        val viewModel = MainViewModel(
            FakeRepository(
                engineResult = Result.failure(IllegalStateException("native library missing")),
            ),
        )

        dispatcher.scheduler.advanceUntilIdle()

        assertNull(viewModel.uiState.value.engineVersion)
        assertEquals("native library missing", viewModel.uiState.value.engineError)
    }

    @Test
    fun pickerCancellationDoesNotCreateAnError() = runTest(dispatcher) {
        val viewModel = MainViewModel(FakeRepository())
        dispatcher.scheduler.advanceUntilIdle()

        viewModel.onPickerCancelled()

        assertNull(viewModel.uiState.value.errorMessage)
        assertFalse(viewModel.uiState.value.isInspecting)
    }

    private class FakeRepository(
        private val inspectResult: Result<InspectedVideo> = Result.failure(IllegalStateException("unused")),
        private val engineResult: Result<String> = Result.success("framescope-rust/0.1.0"),
    ) : FrameScopeRepository {
        override suspend fun engineVersion(): Result<String> = engineResult

        override suspend fun inspect(uri: String): Result<InspectedVideo> = inspectResult
    }
}
