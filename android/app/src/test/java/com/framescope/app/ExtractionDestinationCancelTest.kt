package com.framescope.app

import com.framescope.app.data.BatchExportProgress
import com.framescope.app.data.BatchExportRequest
import com.framescope.app.data.BatchExportSelection
import com.framescope.app.data.ExportedBatchDocument
import com.framescope.app.data.ExportedFrameDocument
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
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ExtractionDestinationCancelTest {
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
    fun currentFrameDestinationCancelReturnsToIdleWithoutExport() = runTest(dispatcher) {
        val repository = CountingRepository()
        val viewModel = FrameExportViewModel(repository)

        viewModel.beginCurrentFrameExport(
            sessionId = 7,
            frameId = 3,
            format = FrameExportFormat.Png,
        )
        viewModel.onDestinationPickerCancelled()

        assertEquals(FrameExportUiState.Idle, viewModel.state.value)
        assertEquals(0, repository.currentExportCalls)
    }

    @Test
    fun batchDestinationCancelReturnsToIdleWithoutExport() = runTest(dispatcher) {
        val repository = CountingRepository()
        val viewModel = BatchExportViewModel(repository)

        viewModel.beginBatchExport(
            sessionId = 7,
            currentFrameId = 3,
            request = BatchExportRequest(
                selection = BatchExportSelection.AllFrames,
                everyNFrames = 5,
                format = FrameExportFormat.Png,
            ),
        )
        viewModel.onDestinationPickerCancelled()

        assertEquals(BatchExportUiState.Idle, viewModel.state.value)
        assertEquals(0, repository.batchExportCalls)
    }

    private class CountingRepository : FrameScopeRepository {
        var currentExportCalls = 0
        var batchExportCalls = 0

        override suspend fun engineVersion(): Result<String> = Result.success("framescope-rust/test")

        override suspend fun inspect(
            uri: String,
            onProgress: (InspectionProgress) -> Unit,
        ): Result<InspectedVideo> = Result.failure(IllegalStateException("unused"))

        override suspend fun exportCurrentFrame(
            treeUri: String,
            format: FrameExportFormat,
            jpegQuality: Int,
        ): Result<ExportedFrameDocument> {
            currentExportCalls += 1
            return Result.failure(AssertionError("Export must not run after destination cancellation."))
        }

        override suspend fun exportFrames(
            treeUri: String,
            request: BatchExportRequest,
            onProgress: (BatchExportProgress) -> Unit,
        ): Result<ExportedBatchDocument> {
            batchExportCalls += 1
            return Result.failure(AssertionError("Export must not run after destination cancellation."))
        }
    }
}
