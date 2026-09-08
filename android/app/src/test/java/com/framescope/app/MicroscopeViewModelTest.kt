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
import com.framescope.app.ui.IndexedTimelineBounds
import com.framescope.app.ui.MainViewModel
import com.framescope.app.ui.MicroscopeUiState
import com.framescope.app.ui.TimelineRangeSelection
import java.nio.ByteBuffer
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class MicroscopeViewModelTest {
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
    fun successfulInspectionResolvesIndexedTimelineBeforePublishingSourceQualityFrame() = runTest(dispatcher) {
        val session = session(sessionId = 7L, frameId = 0L, frameCount = 2L)
        val frame = frame(session)
        val repository = FakeRepository(
            openBlock = { Result.success(session) },
            loadBlock = { Result.success(frame) },
        )
        val viewModel = MainViewModel(repository)

        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/first")
        dispatcher.scheduler.advanceUntilIdle()

        val state = viewModel.uiState.value.microscopeState as MicroscopeUiState.Ready
        assertEquals(session, state.session)
        assertSame(frame, state.frame)
        assertEquals(
            IndexedTimelineBounds(
                sessionId = 7L,
                startUs = 0L,
                endUs = 33_367L,
            ),
            viewModel.uiState.value.timelineBounds,
        )
        assertEquals(
            listOf(
                "close",
                "open:content://test/first",
                "jump:1",
                "jump:0",
                "load",
            ),
            repository.microscopeCalls,
        )
    }

    @Test
    fun navigationPublishesOnlyPixelsForUpdatedIndexedFrame() = runTest(dispatcher) {
        val first = session(sessionId = 11L, frameId = 0L, frameCount = 3L)
        val second = session(sessionId = 11L, frameId = 1L, frameCount = 3L)
        var current = first
        val repository = FakeRepository(
            openBlock = {
                current = first
                Result.success(first)
            },
            stepBlock = { delta ->
                assertEquals(1, delta)
                current = second
                Result.success(second)
            },
            loadBlock = { Result.success(frame(current)) },
        )
        val viewModel = MainViewModel(repository)

        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/nav")
        dispatcher.scheduler.advanceUntilIdle()
        viewModel.stepMicroscope(1)
        dispatcher.scheduler.advanceUntilIdle()

        val state = viewModel.uiState.value.microscopeState as MicroscopeUiState.Ready
        assertEquals(1L, state.session.currentFrame?.frameId)
        assertEquals(1L, state.frame.descriptor.frameId)
        assertEquals(2, repository.microscopeCalls.count { it == "load" })
    }

    @Test
    fun committedInclusiveTimelineRangeSurvivesFrameStepping() = runTest(dispatcher) {
        val first = session(sessionId = 12L, frameId = 0L, frameCount = 4L)
        val second = session(sessionId = 12L, frameId = 1L, frameCount = 4L)
        var current = first
        val repository = FakeRepository(
            openBlock = { Result.success(first) },
            stepBlock = {
                current = second
                Result.success(second)
            },
            loadBlock = { Result.success(frame(current)) },
        )
        val viewModel = MainViewModel(repository)

        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/range")
        dispatcher.scheduler.advanceUntilIdle()

        viewModel.commitTimelineRange(startUs = 33_367L, endUs = 66_734L)
        assertEquals(
            TimelineRangeSelection(12L, 33_367L, 66_734L),
            viewModel.uiState.value.timelineRange,
        )

        viewModel.stepMicroscope(1)
        dispatcher.scheduler.advanceUntilIdle()

        assertEquals(
            TimelineRangeSelection(12L, 33_367L, 66_734L),
            viewModel.uiState.value.timelineRange,
        )
        assertEquals(1L, (viewModel.uiState.value.microscopeState as MicroscopeUiState.Ready).frame.descriptor.frameId)
    }

    @Test
    fun crossingOrOutOfBoundsTimelineRangeIsRejected() = runTest(dispatcher) {
        val session = session(sessionId = 13L, frameId = 0L, frameCount = 4L)
        val repository = FakeRepository(
            openBlock = { Result.success(session) },
            loadBlock = { Result.success(frame(session)) },
        )
        val viewModel = MainViewModel(repository)

        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/range-invalid")
        dispatcher.scheduler.advanceUntilIdle()

        viewModel.commitTimelineRange(startUs = 90_000L, endUs = 20_000L)
        assertNull(viewModel.uiState.value.timelineRange)

        viewModel.commitTimelineRange(startUs = -1L, endUs = 20_000L)
        assertNull(viewModel.uiState.value.timelineRange)
    }

    @Test
    fun rapidTimestampRequestsLetNewestAuthoritativeFrameWin() = runTest(dispatcher) {
        val first = session(sessionId = 14L, frameId = 0L, frameCount = 4L)
        val stale = session(sessionId = 14L, frameId = 1L, frameCount = 4L)
        val newest = session(sessionId = 14L, frameId = 2L, frameCount = 4L)
        val firstSeekEntered = CompletableDeferred<Unit>()
        val releaseStaleSeek = CompletableDeferred<Unit>()
        var current = first
        val repository = FakeRepository(
            openBlock = {
                current = first
                Result.success(first)
            },
            timestampJumpBlock = { timestampUs, _ ->
                if (timestampUs == 40_000L) {
                    firstSeekEntered.complete(Unit)
                    try {
                        releaseStaleSeek.await()
                    } catch (_: CancellationException) {
                        // Model native work that raced past coroutine cancellation.
                    }
                    current = stale
                    Result.success(stale)
                } else {
                    assertEquals(70_000L, timestampUs)
                    current = newest
                    Result.success(newest)
                }
            },
            loadBlock = { Result.success(frame(current)) },
        )
        val viewModel = MainViewModel(repository)

        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/rapid-seek")
        dispatcher.scheduler.advanceUntilIdle()

        viewModel.jumpMicroscopeTimestampUs(40_000L)
        dispatcher.scheduler.runCurrent()
        assertTrue(firstSeekEntered.isCompleted)

        viewModel.jumpMicroscopeTimestampUs(70_000L)
        dispatcher.scheduler.advanceUntilIdle()
        releaseStaleSeek.complete(Unit)
        dispatcher.scheduler.advanceUntilIdle()

        val state = viewModel.uiState.value.microscopeState as MicroscopeUiState.Ready
        assertEquals(2L, state.session.currentFrame?.frameId)
        assertEquals(2L, state.frame.descriptor.frameId)
    }

    @Test
    fun lateNavigationFromReplacedVideoCannotOverwriteNewSource() = runTest(dispatcher) {
        val first = session(sessionId = 21L, frameId = 0L, frameCount = 2L)
        val second = session(sessionId = 22L, frameId = 0L, frameCount = 2L)
        val navigationEntered = CompletableDeferred<Unit>()
        val releaseNavigation = CompletableDeferred<Unit>()
        var active = first
        val repository = FakeRepository(
            openBlock = { uri ->
                active = if (uri.endsWith("second")) second else first
                Result.success(active)
            },
            stepBlock = {
                navigationEntered.complete(Unit)
                try {
                    releaseNavigation.await()
                } catch (_: CancellationException) {
                    // Simulate a native call that completed despite coroutine cancellation.
                }
                Result.success(first.copy(currentFrame = first.currentFrame?.copy(frameId = 1L)))
            },
            loadBlock = { Result.success(frame(active)) },
        )
        val viewModel = MainViewModel(repository)

        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/first")
        dispatcher.scheduler.advanceUntilIdle()
        viewModel.stepMicroscope(1)
        dispatcher.scheduler.runCurrent()
        assertTrue(navigationEntered.isCompleted)

        viewModel.onVideoSelected("content://test/second")
        dispatcher.scheduler.advanceUntilIdle()
        releaseNavigation.complete(Unit)
        dispatcher.scheduler.advanceUntilIdle()

        val state = viewModel.uiState.value.microscopeState as MicroscopeUiState.Ready
        assertEquals(22L, state.session.sessionId)
        assertEquals(22L, state.frame.descriptor.sessionId)
    }

    @Test
    fun mismatchedPresentationIdentityIsRejectedBeforeUiPublication() = runTest(dispatcher) {
        val session = session(sessionId = 31L, frameId = 0L, frameCount = 1L)
        val wrongFrame = frame(session).let { value ->
            MicroscopeFrame(
                descriptor = value.descriptor.copy(sessionId = 999L),
                rgba = value.rgba,
            )
        }
        val viewModel = MainViewModel(
            FakeRepository(
                openBlock = { Result.success(session) },
                loadBlock = { Result.success(wrongFrame) },
            ),
        )

        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/mismatch")
        dispatcher.scheduler.advanceUntilIdle()

        val state = viewModel.uiState.value.microscopeState as MicroscopeUiState.Error
        assertEquals("presentation_identity_mismatch", state.code)
        assertEquals(session, state.session)
    }

    @Test
    fun pickerStartInvalidatesMicroscopeRangeAndClosesSessionOffTheUiStatePath() = runTest(dispatcher) {
        val session = session(sessionId = 41L, frameId = 0L, frameCount = 2L)
        var closeCount = 0
        val viewModel = MainViewModel(
            FakeRepository(
                openBlock = { Result.success(session) },
                loadBlock = { Result.success(frame(session)) },
                closeBlock = {
                    closeCount += 1
                    true
                },
            ),
        )

        dispatcher.scheduler.advanceUntilIdle()
        viewModel.onVideoSelected("content://test/close")
        dispatcher.scheduler.advanceUntilIdle()
        viewModel.commitTimelineRange(0L, 33_367L)
        assertTrue(viewModel.uiState.value.timelineRange != null)
        val closesAfterOpen = closeCount

        viewModel.onPickerStarted()
        assertEquals(MicroscopeUiState.Idle, viewModel.uiState.value.microscopeState)
        assertNull(viewModel.uiState.value.timelineBounds)
        assertNull(viewModel.uiState.value.timelineRange)
        dispatcher.scheduler.advanceUntilIdle()

        assertEquals(closesAfterOpen + 1, closeCount)
    }

    private fun session(
        sessionId: Long,
        frameId: Long,
        frameCount: Long,
    ) = MicroscopeSessionSnapshot(
        sessionId = sessionId,
        frameCount = frameCount,
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
        canStepNext = frameId < frameCount - 1L,
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

    private class FakeRepository(
        private val openBlock: suspend (String) -> Result<MicroscopeSessionSnapshot>,
        private val stepBlock: suspend (Int) -> Result<MicroscopeSessionSnapshot> = {
            Result.failure(IllegalStateException("unused step"))
        },
        private val timestampJumpBlock: (suspend (
            Long,
            TimestampSelectionPolicy,
        ) -> Result<MicroscopeSessionSnapshot>)? = null,
        private val loadBlock: suspend () -> Result<MicroscopeFrame>,
        private val closeBlock: suspend () -> Boolean = { true },
    ) : FrameScopeRepository {
        val microscopeCalls = mutableListOf<String>()
        private var activeSession: MicroscopeSessionSnapshot? = null

        override suspend fun engineVersion(): Result<String> = Result.success("framescope-rust/0.1.0")

        override suspend fun inspect(
            uri: String,
            onProgress: (InspectionProgress) -> Unit,
        ): Result<InspectedVideo> = Result.success(inspectedVideoStatic())

        override suspend fun openMicroscope(uri: String): Result<MicroscopeSessionSnapshot> {
            microscopeCalls += "open:$uri"
            return openBlock(uri).also { result ->
                result.getOrNull()?.let { activeSession = it }
            }
        }

        override suspend fun stepMicroscope(delta: Int): Result<MicroscopeSessionSnapshot> {
            microscopeCalls += "step:$delta"
            return stepBlock(delta).also { result ->
                result.getOrNull()?.let { activeSession = it }
            }
        }

        override suspend fun jumpMicroscopeFrame(frameId: Long): Result<MicroscopeSessionSnapshot> {
            microscopeCalls += "jump:$frameId"
            val current = activeSession
                ?: return Result.failure(IllegalStateException("no active microscope"))
            if (frameId !in 0L until current.frameCount) {
                return Result.failure(IllegalArgumentException("frame out of range"))
            }
            val descriptor = requireNotNull(current.currentFrame).copy(
                frameId = frameId,
                timestampTicks = frameId * 1_001L,
                timestampUs = frameId * 33_367L,
                keyframe = frameId == 0L,
            )
            val updated = current.copy(
                currentFrame = descriptor,
                canStepPrevious = frameId > 0L,
                canStepNext = frameId < current.frameCount - 1L,
            )
            activeSession = updated
            return Result.success(updated)
        }

        override suspend fun jumpMicroscopeTimestampUs(
            timestampUs: Long,
            selection: TimestampSelectionPolicy,
        ): Result<MicroscopeSessionSnapshot> {
            microscopeCalls += "jump-ts:$timestampUs"
            val block = timestampJumpBlock
                ?: return Result.failure(IllegalStateException("unused timestamp jump"))
            return block(timestampUs, selection).also { result ->
                result.getOrNull()?.let { activeSession = it }
            }
        }

        override suspend fun loadMicroscopeFrame(): Result<MicroscopeFrame> {
            microscopeCalls += "load"
            return loadBlock()
        }

        override suspend fun closeMicroscope(): Boolean {
            microscopeCalls += "close"
            activeSession = null
            return closeBlock()
        }

        companion object {
            private fun inspectedVideoStatic() = InspectedVideo(
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
        }
    }
}
