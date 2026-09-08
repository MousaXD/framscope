package com.framescope.app

import com.framescope.app.data.BatchExportProgress
import com.framescope.app.data.BatchExportRequest
import com.framescope.app.data.BatchExportSelection
import com.framescope.app.data.ExportedBatchDocument
import com.framescope.app.data.ExportedFrameDocument
import com.framescope.app.data.FrameExportException
import com.framescope.app.data.FrameExportFormat
import com.framescope.app.data.FrameScopeRepository
import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.InspectionProgress
import com.framescope.app.ui.BatchExportUiState
import com.framescope.app.ui.BatchExportViewModel
import com.framescope.app.ui.FrameExportUiState
import com.framescope.app.ui.FrameExportViewModel
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
class ExportStorageUiStateTest {
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
    fun `current frame native ENOSPC becomes actionable storage error`() = runTest(dispatcher) {
        val repository = FailingRepository(
            currentFailure = FrameExportException(
                code = "io_error",
                message = "No space left on device (os error 28)",
            ),
        )
        val viewModel = FrameExportViewModel(repository)

        viewModel.beginCurrentFrameExport(
            sessionId = 7,
            frameId = 3,
            format = FrameExportFormat.Png,
        )
        viewModel.onDestinationSelected(
            treeUri = "content://test/tree",
            currentSessionId = 7,
            currentFrameId = 3,
        )
        dispatcher.scheduler.advanceUntilIdle()

        val state = viewModel.state.value
        assertTrue(state is FrameExportUiState.Error)
        state as FrameExportUiState.Error
        assertEquals("storage_full", state.code)
        assertTrue(state.message.contains("out of space"))
    }

    @Test
    fun `batch capacity failure becomes actionable storage error`() = runTest(dispatcher) {
        val repository = FailingRepository(
            batchFailure = FrameExportException(
                code = "storage_full",
                message = "The export destination is out of space or has reached its storage quota.",
            ),
        )
        val viewModel = BatchExportViewModel(repository)
        val request = BatchExportRequest(
            selection = BatchExportSelection.AllFrames,
            everyNFrames = 1,
            format = FrameExportFormat.Png,
        )

        viewModel.beginBatchExport(sessionId = 7, currentFrameId = 3, request = request)
        viewModel.onDestinationSelected(
            treeUri = "content://test/tree",
            currentSessionId = 7,
            currentFrameId = 3,
        )
        dispatcher.scheduler.advanceUntilIdle()

        val state = viewModel.state.value
        assertTrue(state is BatchExportUiState.Error)
        state as BatchExportUiState.Error
        assertEquals("storage_full", state.code)
        assertTrue(state.message.contains("out of space"))
    }

    @Test
    fun `decode error mentioning space is not mislabeled`() = runTest(dispatcher) {
        val repository = FailingRepository(
            batchFailure = FrameExportException(
                code = "decode_error",
                message = "decoder said no space left on device",
            ),
        )
        val viewModel = BatchExportViewModel(repository)
        val request = BatchExportRequest(
            selection = BatchExportSelection.AllFrames,
            everyNFrames = 1,
            format = FrameExportFormat.Png,
        )

        viewModel.beginBatchExport(sessionId = 7, currentFrameId = 3, request = request)
        viewModel.onDestinationSelected(
            treeUri = "content://test/tree",
            currentSessionId = 7,
            currentFrameId = 3,
        )
        dispatcher.scheduler.advanceUntilIdle()

        val state = viewModel.state.value
        assertTrue(state is BatchExportUiState.Error)
        state as BatchExportUiState.Error
        assertEquals("decode_error", state.code)
    }

    private class FailingRepository(
        private val currentFailure: Throwable = IllegalStateException("unused"),
        private val batchFailure: Throwable = IllegalStateException("unused"),
    ) : FrameScopeRepository {
        override suspend fun engineVersion(): Result<String> = Result.success("framescope-rust/test")

        override suspend fun inspect(
            uri: String,
            onProgress: (InspectionProgress) -> Unit,
        ): Result<InspectedVideo> = Result.failure(IllegalStateException("unused"))

        override suspend fun exportCurrentFrame(
            treeUri: String,
            format: FrameExportFormat,
            jpegQuality: Int,
        ): Result<ExportedFrameDocument> = Result.failure(currentFailure)

        override suspend fun exportFrames(
            treeUri: String,
            request: BatchExportRequest,
            onProgress: (BatchExportProgress) -> Unit,
        ): Result<ExportedBatchDocument> = Result.failure(batchFailure)
    }
}
