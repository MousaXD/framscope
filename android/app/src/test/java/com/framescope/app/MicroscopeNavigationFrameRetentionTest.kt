package com.framescope.app

import com.framescope.app.data.FrameDetails
import com.framescope.app.data.FrameScopeRepository
import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.InspectionProgress
import com.framescope.app.data.MicroscopeFrame
import com.framescope.app.data.MicroscopeSessionSnapshot
import com.framescope.app.data.PreparedMicroscopeFrame
import com.framescope.app.data.TimestampSelectionPolicy
import com.framescope.app.data.VideoMetadata
import com.framescope.app.ui.MainViewModel
import com.framescope.app.ui.MicroscopeUiState
import java.nio.ByteBuffer
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class MicroscopeNavigationFrameRetentionTest {
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
    fun previousFrameRemainsPublishedUntilNavigatedPixelsArrive() = runTest(dispatcher) {
        val firstSession = session(frameId = 0L)
        val secondSession = session(frameId = 1L)
        val firstFrame = frame(firstSession)
        val secondFrame = frame(secondSession)
        val secondLoadEntered = CompletableDeferred<Unit>()
        val releaseSecondLoad = CompletableDeferred<Unit>()
        var loadCount = 0

        val repository = object : FrameScopeRepository {
            override suspend fun engineVersion(): Result<String> =
                Result.success("framescope-rust/0.1.0")

            override suspend fun inspect(
                uri: String,
                onProgress: (InspectionProgress) -> Unit,
            ): Result<InspectedVideo> = Result.success(inspectedVideo())

            override suspend fun openMicroscope(uri: String): Result<MicroscopeSessionSnapshot> =
                Result.success(firstSession)

            override suspend fun stepMicroscope(delta: Int): Result<MicroscopeSessionSnapshot> {
                assertEquals(1, delta)
                return Result.success(secondSession)
            }

            override suspend fun jumpMicroscopeFrame(
                frameId: Long,
            ): Result<MicroscopeSessionSnapshot> =
                Result.failure(IllegalStateException("unused frame jump"))

            override suspend fun jumpMicroscopeTimestampUs(
                timestampUs: Long,
                selection: TimestampSelectionPolicy,
            ): Result<MicroscopeSessionSnapshot> =
                Result.failure(IllegalStateException("unused timestamp jump"))

            override suspend fun loadMicroscopeFrame(): Result<MicroscopeFrame> {
                loadCount += 1
                return if (loadCount == 1) {
                    Result.success(firstFrame)
                } else {
                    secondLoadEntered.complete(Unit)
                    releaseSecondLoad.await()
                    Result.success(secondFrame)
                }
            }

            override suspend fun closeMicroscope(): Boolean = true
        }

        val viewModel = MainViewModel(repository)
        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/navigation-retention")
        dispatcher.scheduler.advanceUntilIdle()

        val ready = viewModel.uiState.value.microscopeState as MicroscopeUiState.Ready
        assertSame(firstFrame, ready.frame)

        viewModel.stepMicroscope(1)
        dispatcher.scheduler.runCurrent()
        assertEquals(true, secondLoadEntered.isCompleted)

        val navigating = viewModel.uiState.value.microscopeState as MicroscopeUiState.Navigating
        assertSame(firstFrame, navigating.previousFrame)
        assertEquals(0L, navigating.session.currentFrame?.frameId)

        releaseSecondLoad.complete(Unit)
        dispatcher.scheduler.advanceUntilIdle()

        val replaced = viewModel.uiState.value.microscopeState as MicroscopeUiState.Ready
        assertEquals(1L, replaced.session.currentFrame?.frameId)
        assertSame(secondFrame, replaced.frame)
    }

    private fun session(frameId: Long) = MicroscopeSessionSnapshot(
        sessionId = 71L,
        frameCount = 2L,
        currentFrame = FrameDetails(
            frameId = frameId,
            timestampTicks = frameId * 1_001L,
            timestampUs = frameId * 33_367L,
            timeBaseNumerator = 1,
            timeBaseDenominator = 30_000,
            durationTicks = 1_001L,
            keyframe = frameId == 0L,
            corrupt = false,
        ),
        canStepPrevious = frameId > 0L,
        canStepNext = frameId == 0L,
    )

    private fun frame(session: MicroscopeSessionSnapshot): MicroscopeFrame {
        val frameId = requireNotNull(session.currentFrame).frameId
        return MicroscopeFrame(
            descriptor = PreparedMicroscopeFrame(
                sessionId = session.sessionId,
                frameId = frameId,
                generation = frameId + 1L,
                width = 1,
                height = 1,
                strideBytes = 4L,
                byteLen = 4,
            ),
            rgba = ByteBuffer.allocateDirect(4),
        )
    }

    private fun inspectedVideo() = InspectedVideo(
        displayName = "clip.mp4",
        metadata = VideoMetadata(
            durationUs = 2_000_000L,
            width = 1280,
            height = 720,
            estimatedFrameRate = 30.0,
            rotationDegrees = 0,
        ),
        engine = "framescope-rust/0.1.0",
    )
}
