package com.framescope.app

import com.framescope.app.data.FrameDetails
import com.framescope.app.data.FrameScopeRepository
import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.InspectionProgress
import com.framescope.app.data.MicroscopeFrame
import com.framescope.app.data.MicroscopeSessionSnapshot
import com.framescope.app.data.PreparedMicroscopeFrame
import com.framescope.app.data.VideoMetadata
import com.framescope.app.ui.MainViewModel
import com.framescope.app.ui.MicroscopeUiState
import com.framescope.app.ui.VideoInspectionState
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
class MicroscopeViewModelSourceReplacementTest {
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
    fun failedReplacementInspectionStillClosesPreviousMicroscopeSession() = runTest(dispatcher) {
        val repository = ReplacementRepository()
        val viewModel = MainViewModel(repository)

        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/first")
        dispatcher.scheduler.advanceUntilIdle()

        assertTrue(viewModel.uiState.value.microscopeState is MicroscopeUiState.Ready)
        assertEquals(1, repository.closeCalls)

        viewModel.onPickerStarted()
        repository.failInspection = true
        viewModel.onVideoSelected("content://test/bad-replacement")
        dispatcher.scheduler.advanceUntilIdle()

        assertTrue(viewModel.uiState.value.videoState is VideoInspectionState.Error)
        assertEquals(MicroscopeUiState.Idle, viewModel.uiState.value.microscopeState)
        assertEquals(2, repository.closeCalls)
    }

    private class ReplacementRepository : FrameScopeRepository {
        private val session = MicroscopeSessionSnapshot(
            sessionId = 21L,
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

        var closeCalls = 0
        var failInspection = false

        override suspend fun engineVersion(): Result<String> = Result.success("test-engine")

        override suspend fun inspect(
            uri: String,
            onProgress: (InspectionProgress) -> Unit,
        ): Result<InspectedVideo> {
            onProgress(InspectionProgress.Opening)
            if (failInspection) {
                return Result.failure(IllegalStateException("replacement inspection failed"))
            }
            onProgress(InspectionProgress.Inspecting)
            return Result.success(
                InspectedVideo(
                    displayName = "first.mp4",
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

        override suspend fun closeMicroscope(): Boolean {
            closeCalls += 1
            return true
        }
    }
}
