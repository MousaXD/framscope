package com.framescope.app

import com.framescope.app.data.BatchExportProgress
import com.framescope.app.data.BatchExportRequest
import com.framescope.app.data.BatchExportResult
import com.framescope.app.data.BatchExportSelection
import com.framescope.app.data.ExportedBatchDocument
import com.framescope.app.data.FrameExportFormat
import com.framescope.app.data.FrameScopeRepository
import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.InspectionProgress
import com.framescope.app.ui.BatchExportUiState
import com.framescope.app.ui.BatchExportViewModel
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
class BatchExportViewModelTest {
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
    fun matchingDestinationPublishesProgressAndSuccess() = runTest(dispatcher) {
        val request = request(BatchExportSelection.AllFrames)
        val repository = FakeRepository(
            exportBlock = { _, _, progress ->
                progress(BatchExportProgress(frameId = 3, ordinal = 2, total = 4))
                Result.success(document(sessionId = 7, format = FrameExportFormat.Png, frames = 4))
            },
        )
        val viewModel = BatchExportViewModel(repository)

        viewModel.beginBatchExport(sessionId = 7, currentFrameId = 3, request = request)
        viewModel.onDestinationSelected(
            treeUri = "content://test/tree",
            currentSessionId = 7,
            currentFrameId = 3,
        )
        dispatcher.scheduler.advanceUntilIdle()

        assertEquals(1, repository.exportCalls)
        val state = viewModel.state.value
        assertTrue(state is BatchExportUiState.Success)
        state as BatchExportUiState.Success
        assertEquals(4, state.document.export.committedFrames)
    }

    @Test
    fun changedSessionWhilePickerOpenIsRejectedBeforeRepositoryCall() = runTest(dispatcher) {
        val repository = FakeRepository()
        val viewModel = BatchExportViewModel(repository)
        val request = request(BatchExportSelection.AllFrames)

        viewModel.beginBatchExport(sessionId = 7, currentFrameId = 3, request = request)
        viewModel.onDestinationSelected(
            treeUri = "content://test/tree",
            currentSessionId = 8,
            currentFrameId = 3,
        )
        dispatcher.scheduler.advanceUntilIdle()

        assertEquals(0, repository.exportCalls)
        val state = viewModel.state.value
        assertTrue(state is BatchExportUiState.Error)
        state as BatchExportUiState.Error
        assertEquals("stale_result", state.code)
    }

    @Test
    fun currentFrameSelectionRejectsPickerRaceToAnotherFrame() = runTest(dispatcher) {
        val repository = FakeRepository()
        val viewModel = BatchExportViewModel(repository)
        val request = request(BatchExportSelection.CurrentFrame(3))

        viewModel.beginBatchExport(sessionId = 7, currentFrameId = 3, request = request)
        viewModel.onDestinationSelected(
            treeUri = "content://test/tree",
            currentSessionId = 7,
            currentFrameId = 4,
        )
        dispatcher.scheduler.advanceUntilIdle()

        assertEquals(0, repository.exportCalls)
        val state = viewModel.state.value
        assertTrue(state is BatchExportUiState.Error)
        state as BatchExportUiState.Error
        assertEquals("stale_result", state.code)
    }

    @Test
    fun cancellingActiveBatchRequestsNativeCancellationAndSuppressesWork() = runTest(dispatcher) {
        var coroutineCancelled = false
        val repository = FakeRepository(
            exportBlock = { _, _, _ ->
                try {
                    awaitCancellation()
                } finally {
                    coroutineCancelled = true
                }
            },
        )
        val viewModel = BatchExportViewModel(repository)
        val request = request(BatchExportSelection.AllFrames)

        viewModel.beginBatchExport(sessionId = 7, currentFrameId = 3, request = request)
        viewModel.onDestinationSelected(
            treeUri = "content://test/tree",
            currentSessionId = 7,
            currentFrameId = 3,
        )
        dispatcher.scheduler.runCurrent()
        assertTrue(viewModel.state.value is BatchExportUiState.Exporting)

        viewModel.cancelForMicroscopeChange()
        dispatcher.scheduler.advanceUntilIdle()

        assertTrue(repository.cancelCalls > 0)
        assertTrue(coroutineCancelled)
        assertEquals(BatchExportUiState.Idle, viewModel.state.value)
    }

    @Test
    fun mismatchedNativeSessionIsNotPublishedAsSuccess() = runTest(dispatcher) {
        val repository = FakeRepository(
            exportBlock = { _, _, _ ->
                Result.success(document(sessionId = 99, format = FrameExportFormat.Png, frames = 2))
            },
        )
        val viewModel = BatchExportViewModel(repository)
        val request = request(BatchExportSelection.AllFrames)

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
        assertEquals("export_identity_mismatch", state.code)
    }

    private fun request(selection: BatchExportSelection) = BatchExportRequest(
        selection = selection,
        everyNFrames = 1,
        format = FrameExportFormat.Png,
    )

    private fun document(
        sessionId: Long,
        format: FrameExportFormat,
        frames: Long,
    ) = ExportedBatchDocument(
        manifestUri = "content://test/manifest",
        manifestDisplayName = "framescope_manifest_7_1.jsonl",
        export = BatchExportResult(
            sessionId = sessionId,
            expectedFrames = frames,
            committedFrames = frames,
            encodedBytes = frames * 100,
            decodedFrames = frames,
            usedKeyframeSeek = true,
            fellBackToStreamStart = false,
            format = format,
            mimeType = format.mimeType,
        ),
    )

    private class FakeRepository(
        private val exportBlock: suspend (
            String,
            BatchExportRequest,
            (BatchExportProgress) -> Unit,
        ) -> Result<ExportedBatchDocument> = { _, _, _ -> Result.failure(IllegalStateException("unused")) },
    ) : FrameScopeRepository {
        var exportCalls = 0
        var cancelCalls = 0

        override suspend fun engineVersion(): Result<String> = Result.success("framescope-rust/test")

        override suspend fun inspect(
            uri: String,
            onProgress: (InspectionProgress) -> Unit,
        ): Result<InspectedVideo> = Result.failure(IllegalStateException("unused"))

        override suspend fun exportFrames(
            treeUri: String,
            request: BatchExportRequest,
            onProgress: (BatchExportProgress) -> Unit,
        ): Result<ExportedBatchDocument> {
            exportCalls += 1
            return exportBlock(treeUri, request, onProgress)
        }

        override fun cancelActiveNativeOperation() {
            cancelCalls += 1
        }
    }
}
