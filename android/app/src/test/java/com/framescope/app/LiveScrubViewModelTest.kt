package com.framescope.app

import com.framescope.app.data.FrameDetails
import com.framescope.app.data.FrameScopeRepository
import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.InspectionProgress
import com.framescope.app.data.MicroscopeFrame
import com.framescope.app.data.MicroscopePreviewDescriptor
import com.framescope.app.data.MicroscopeScrubPreview
import com.framescope.app.data.MicroscopeScrubPreviewSource
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
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class LiveScrubViewModelTest {
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
    fun latePreviewCannotPublishAfterSourceReplacement() = runTest(dispatcher) {
        val repository = ScrubRepository()
        val previewSource = BlockingPreviewSource()
        val viewModel = MainViewModel(repository, previewSource)

        advanceUntilIdle()
        viewModel.onVideoSelected("content://test/first")
        advanceUntilIdle()
        assertEquals(21L, ready(viewModel).session.sessionId)

        viewModel.previewMicroscopeTimestampUs(40_000L)
        runCurrent()
        assertTrue(previewSource.started.isCompleted)

        viewModel.onVideoSelected("content://test/second")
        runCurrent()
        advanceUntilIdle()
        assertEquals(22L, ready(viewModel).session.sessionId)
        assertNull(viewModel.uiState.value.scrubPreview)

        previewSource.release.complete(
            Result.success(preview(sessionId = 21L, frameId = 1L, timestampUs = 40_000L)),
        )
        advanceUntilIdle()

        assertEquals(22L, ready(viewModel).session.sessionId)
        assertNull(viewModel.uiState.value.scrubPreview)
        assertTrue(21L in previewSource.forgottenSessions)
    }

    @Test
    fun releaseReplacesPreviewOnlyAfterAuthoritativeIndexedFrameArrives() = runTest(dispatcher) {
        val repository = ScrubRepository()
        val previewSource = ImmediatePreviewSource()
        val viewModel = MainViewModel(repository, previewSource)

        advanceUntilIdle()
        viewModel.onVideoSelected("content://test/first")
        advanceUntilIdle()
        assertEquals(0L, ready(viewModel).session.currentFrame?.frameId)

        viewModel.previewMicroscopeTimestampUs(40_000L)
        advanceUntilIdle()

        assertEquals(0L, ready(viewModel).session.currentFrame?.frameId)
        assertEquals(1L, viewModel.uiState.value.scrubPreview?.descriptor?.frameId)

        viewModel.finishMicroscopeScrubTimestampUs(40_000L)
        advanceUntilIdle()

        assertEquals(1L, ready(viewModel).session.currentFrame?.frameId)
        assertNull(viewModel.uiState.value.scrubPreview)
        assertEquals(listOf(40_000L), repository.authoritativeTimestampJumps)
    }

    private fun ready(viewModel: MainViewModel): MicroscopeUiState.Ready =
        viewModel.uiState.value.microscopeState as MicroscopeUiState.Ready

    private class BlockingPreviewSource : MicroscopeScrubPreviewSource {
        val started = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Result<MicroscopeScrubPreview>>()
        val forgottenSessions = mutableListOf<Long>()

        override suspend fun renderTimestamp(
            sessionId: Long,
            timestampUs: Long,
            selection: TimestampSelectionPolicy,
        ): Result<MicroscopeScrubPreview> {
            started.complete(Unit)
            return release.await()
        }

        override suspend fun renderFrame(
            sessionId: Long,
            frameId: Long,
        ): Result<MicroscopeScrubPreview> {
            started.complete(Unit)
            return release.await()
        }

        // This fixture exercises demand publication only; speculative work stays deliberately inert.
        override suspend fun prefetchFrame(sessionId: Long, frameId: Long): Boolean = false

        override fun cancelSession(sessionId: Long): Boolean = false

        override suspend fun forgetSession(sessionId: Long) {
            forgottenSessions += sessionId
        }
    }

    private class ImmediatePreviewSource : MicroscopeScrubPreviewSource {
        override suspend fun renderTimestamp(
            sessionId: Long,
            timestampUs: Long,
            selection: TimestampSelectionPolicy,
        ): Result<MicroscopeScrubPreview> = Result.success(
            preview(sessionId = sessionId, frameId = 1L, timestampUs = timestampUs),
        )

        override suspend fun renderFrame(
            sessionId: Long,
            frameId: Long,
        ): Result<MicroscopeScrubPreview> = Result.success(
            preview(sessionId = sessionId, frameId = frameId, timestampUs = frameId * 40_000L),
        )

        // This fixture exercises demand publication only; speculative work stays deliberately inert.
        override suspend fun prefetchFrame(sessionId: Long, frameId: Long): Boolean = false

        override fun cancelSession(sessionId: Long): Boolean = false

        override suspend fun forgetSession(sessionId: Long) = Unit
    }

    private class ScrubRepository : FrameScopeRepository {
        private var sessionId = 0L
        private var frameId = 0L
        val authoritativeTimestampJumps = mutableListOf<Long>()

        override suspend fun engineVersion(): Result<String> = Result.success("test-engine")

        override suspend fun inspect(
            uri: String,
            onProgress: (InspectionProgress) -> Unit,
        ): Result<InspectedVideo> {
            onProgress(InspectionProgress.Opening)
            onProgress(InspectionProgress.Inspecting)
            return Result.success(
                InspectedVideo(
                    displayName = if (uri.endsWith("second")) "second.mp4" else "first.mp4",
                    metadata = VideoMetadata(
                        durationUs = 80_000L,
                        width = 16,
                        height = 16,
                        estimatedFrameRate = null,
                        rotationDegrees = 0,
                    ),
                    engine = "test-engine",
                ),
            )
        }

        override suspend fun openMicroscope(uri: String): Result<MicroscopeSessionSnapshot> {
            sessionId = if (uri.endsWith("second")) 22L else 21L
            frameId = 0L
            return Result.success(snapshot())
        }

        override suspend fun jumpMicroscopeFrame(frameId: Long): Result<MicroscopeSessionSnapshot> {
            if (frameId !in 0L..1L) {
                return Result.failure(IllegalArgumentException("frame out of range"))
            }
            this.frameId = frameId
            return Result.success(snapshot())
        }

        override suspend fun jumpMicroscopeTimestampUs(
            timestampUs: Long,
            selection: TimestampSelectionPolicy,
        ): Result<MicroscopeSessionSnapshot> {
            authoritativeTimestampJumps += timestampUs
            frameId = if (timestampUs >= 20_000L) 1L else 0L
            return Result.success(snapshot())
        }

        override suspend fun loadMicroscopeFrame(): Result<MicroscopeFrame> = Result.success(
            MicroscopeFrame(
                descriptor = PreparedMicroscopeFrame(
                    sessionId = sessionId,
                    frameId = frameId,
                    generation = frameId + 1L,
                    width = 1,
                    height = 1,
                    strideBytes = 4L,
                    byteLen = 4,
                ),
                rgba = ByteBuffer.allocateDirect(4),
            ),
        )

        override suspend fun closeMicroscope(): Boolean = true

        private fun snapshot(): MicroscopeSessionSnapshot = MicroscopeSessionSnapshot(
            sessionId = sessionId,
            frameCount = 2L,
            currentFrame = FrameDetails(
                frameId = frameId,
                timestampTicks = frameId * 40L,
                timestampUs = frameId * 40_000L,
                timeBaseNumerator = 1,
                timeBaseDenominator = 1_000,
                durationTicks = 40L,
                keyframe = frameId == 0L,
                corrupt = false,
            ),
            canStepPrevious = frameId > 0L,
            canStepNext = frameId < 1L,
        )
    }

    private companion object {
        fun preview(
            sessionId: Long,
            frameId: Long,
            timestampUs: Long,
        ) = MicroscopeScrubPreview(
            descriptor = MicroscopePreviewDescriptor(
                sessionId = sessionId,
                frameId = frameId,
                timestampUs = timestampUs,
                width = 1,
                height = 1,
                strideBytes = 4L,
                byteLen = 4,
                source = "decoded",
                decodedFrames = 1L,
            ),
            rgba = ByteBuffer.allocateDirect(4),
        )
    }
}
