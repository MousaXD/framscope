package com.framescope.app

import com.framescope.app.data.FrameDetails
import com.framescope.app.data.FrameScopeRepository
import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.InspectionProgress
import com.framescope.app.data.MicroscopeFrame
import com.framescope.app.data.MicroscopeOperationException
import com.framescope.app.data.MicroscopeSessionSnapshot
import com.framescope.app.data.PreparedMicroscopeFrame
import com.framescope.app.data.VideoMetadata
import com.framescope.app.ui.MainViewModel
import com.framescope.app.ui.MicroscopeUiState
import java.nio.ByteBuffer
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
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
class MicroscopeViewModelErrorCleanupTest {
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
    fun dismissingNavigationErrorClosesSessionBeforeDroppingUiOwnership() = runTest(dispatcher) {
        val session = session()
        val repository = FakeRepository(session)
        val viewModel = MainViewModel(repository)

        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/video")
        dispatcher.scheduler.advanceUntilIdle()

        assertTrue(viewModel.uiState.value.microscopeState is MicroscopeUiState.Ready)
        assertEquals(1, repository.closeCalls)

        viewModel.stepMicroscope(1)
        dispatcher.scheduler.advanceUntilIdle()

        val error = viewModel.uiState.value.microscopeState as MicroscopeUiState.Error
        assertEquals("decoder_failure", error.code)
        assertEquals(session, error.session)

        viewModel.clearError()
        dispatcher.scheduler.advanceUntilIdle()

        assertEquals(MicroscopeUiState.Idle, viewModel.uiState.value.microscopeState)
        assertEquals(2, repository.closeCalls)
    }

    @Test
    fun invalidStepDeltaNeverReachesRepository() = runTest(dispatcher) {
        val repository = FakeRepository(session())
        val viewModel = MainViewModel(repository)

        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/video")
        dispatcher.scheduler.advanceUntilIdle()

        val ready = viewModel.uiState.value.microscopeState
        assertTrue(ready is MicroscopeUiState.Ready)

        viewModel.stepMicroscope(2)
        dispatcher.scheduler.advanceUntilIdle()

        assertEquals(0, repository.stepCalls)
        assertTrue(viewModel.uiState.value.microscopeState is MicroscopeUiState.Ready)
    }

    private fun session() = MicroscopeSessionSnapshot(
        sessionId = 7L,
        frameCount = 2L,
        currentFrame = FrameDetails(
            frameId = 0L,
            timestampTicks = 0L,
            timestampUs = 0L,
            timeBaseNumerator = 1,
            timeBaseDenominator = 1_000,
            durationTicks = 40L,
            keyframe = true,
            corrupt = false,
        ),
        canStepPrevious = false,
        canStepNext = true,
    )

    private class FakeRepository(
        private val session: MicroscopeSessionSnapshot,
    ) : FrameScopeRepository {
        var closeCalls = 0
        var stepCalls = 0

        override suspend fun engineVersion(): Result<String> = Result.success("test-engine")

        override suspend fun inspect(
            uri: String,
            onProgress: (InspectionProgress) -> Unit,
        ): Result<InspectedVideo> {
            onProgress(InspectionProgress.Opening)
            onProgress(InspectionProgress.Inspecting)
            return Result.success(
                InspectedVideo(
                    displayName = "video.mp4",
                    metadata = VideoMetadata(
                        durationUs = 1_000_000L,
                        width = 16,
                        height = 16,
                        estimatedFrameRate = null,
                        rotationDegrees = 0,
                    ),
                    engine = "test-engine",
                ),
            )
        }

        override suspend fun openMicroscope(uri: String): Result<MicroscopeSessionSnapshot> =
            Result.success(session)

        override suspend fun jumpMicroscopeFrame(frameId: Long): Result<MicroscopeSessionSnapshot> = when (frameId) {
            0L -> Result.success(session)
            1L -> Result.success(
                session.copy(
                    currentFrame = session.currentFrame!!.copy(
                        frameId = 1L,
                        timestampTicks = 40L,
                        timestampUs = 40_000L,
                        keyframe = false,
                    ),
                    canStepPrevious = true,
                    canStepNext = false,
                ),
            )
            else -> Result.failure(IllegalArgumentException("frame out of range"))
        }

        override suspend fun loadMicroscopeFrame(): Result<MicroscopeFrame> =
            Result.success(
                MicroscopeFrame(
                    descriptor = PreparedMicroscopeFrame(
                        sessionId = session.sessionId,
                        frameId = session.currentFrame!!.frameId,
                        generation = 1L,
                        width = 1,
                        height = 1,
                        strideBytes = 4L,
                        byteLen = 4,
                    ),
                    rgba = ByteBuffer.allocateDirect(4),
                ),
            )

        override suspend fun stepMicroscope(delta: Int): Result<MicroscopeSessionSnapshot> {
            stepCalls += 1
            return Result.failure(
                MicroscopeOperationException(
                    code = "decoder_failure",
                    message = "navigation failed",
                ),
            )
        }

        override suspend fun closeMicroscope(): Boolean {
            closeCalls += 1
            return true
        }
    }
}
