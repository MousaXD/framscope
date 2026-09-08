package com.framescope.app

import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.RecentVideoAvailability
import com.framescope.app.data.RecentVideoHistory
import com.framescope.app.data.RecentVideoRecord
import com.framescope.app.data.VideoMetadata
import com.framescope.app.data.VideoUriPermissionStatus
import com.framescope.app.ui.HistoryUiState
import com.framescope.app.ui.HistoryViewModel
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
class HistoryViewModelTest {
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
    fun initialLoadRefreshesAccessAndPublishesEntries() = runTest(dispatcher) {
        val entry = record("one")
        val history = FakeHistory(mutableListOf(entry))
        val viewModel = HistoryViewModel(history)

        dispatcher.scheduler.advanceUntilIdle()

        assertTrue(history.refreshAccessRequested)
        assertEquals(HistoryUiState.Ready(listOf(entry)), viewModel.state.value)
    }

    @Test
    fun removeDeletesOnlyRequestedRecordAndReloads() = runTest(dispatcher) {
        val first = record("one")
        val second = record("two")
        val history = FakeHistory(mutableListOf(first, second))
        val viewModel = HistoryViewModel(history)
        dispatcher.scheduler.advanceUntilIdle()

        viewModel.remove(first.id)
        dispatcher.scheduler.advanceUntilIdle()

        assertEquals(HistoryUiState.Ready(listOf(second)), viewModel.state.value)
    }

    @Test
    fun clearPublishesEmptyHistory() = runTest(dispatcher) {
        val history = FakeHistory(mutableListOf(record("one")))
        val viewModel = HistoryViewModel(history)
        dispatcher.scheduler.advanceUntilIdle()

        viewModel.clear()
        dispatcher.scheduler.advanceUntilIdle()

        assertEquals(HistoryUiState.Ready(emptyList()), viewModel.state.value)
    }

    @Test
    fun loadFailureIsVisibleInsteadOfCrashing() = runTest(dispatcher) {
        val viewModel = HistoryViewModel(
            FakeHistory(
                records = mutableListOf(),
                loadFailure = IllegalStateException("history storage unavailable"),
            ),
        )

        dispatcher.scheduler.advanceUntilIdle()

        assertEquals(
            HistoryUiState.Error("history storage unavailable"),
            viewModel.state.value,
        )
    }

    private fun record(id: String) = RecentVideoRecord(
        id = id,
        contentUri = "content://videos/$id",
        permissionStatus = VideoUriPermissionStatus.Persisted,
        availability = RecentVideoAvailability.Available,
        displayName = "$id.mp4",
        durationUs = 1_000_000L,
        width = 1280,
        height = 720,
        codec = "h264",
        container = "mp4",
        lastOpenedEpochMs = 1_000L,
        lastViewedFrameId = null,
        lastViewedTimestampUs = null,
    )

    private class FakeHistory(
        val records: MutableList<RecentVideoRecord>,
        private val loadFailure: Throwable? = null,
    ) : RecentVideoHistory {
        var refreshAccessRequested = false

        override suspend fun entries(refreshAccess: Boolean): List<RecentVideoRecord> {
            refreshAccessRequested = refreshAccess
            loadFailure?.let { throw it }
            return records.toList()
        }

        override suspend fun findById(id: String): RecentVideoRecord? =
            records.firstOrNull { it.id == id }

        override suspend fun recordOpened(
            contentUri: String,
            video: InspectedVideo,
            permissionStatus: VideoUriPermissionStatus,
        ): RecentVideoRecord = error("unused")

        override suspend fun recordReselected(
            recordId: String,
            contentUri: String,
            video: InspectedVideo,
            permissionStatus: VideoUriPermissionStatus,
        ): RecentVideoRecord = error("unused")

        override suspend fun updatePosition(contentUri: String, frameId: Long?, timestampUs: Long?) = Unit

        override suspend fun remove(recordId: String) {
            records.removeAll { it.id == recordId }
        }

        override suspend fun clear() {
            records.clear()
        }
    }
}
