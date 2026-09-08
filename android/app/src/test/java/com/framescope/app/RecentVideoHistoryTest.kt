package com.framescope.app

import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.RecentVideoAccessChecker
import com.framescope.app.data.RecentVideoAvailability
import com.framescope.app.data.RecentVideoHistoryRepository
import com.framescope.app.data.RecentVideoJsonCodec
import com.framescope.app.data.RecentVideoRecord
import com.framescope.app.data.RecentVideoStore
import com.framescope.app.data.VideoMetadata
import com.framescope.app.data.VideoUriPermissionStatus
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class RecentVideoHistoryTest {
    @Test
    fun insertAndOrderingUseMostRecentOpenTime() = runTest {
        var now = 100L
        val store = MemoryStore()
        val history = repository(store, now = { now })

        history.recordOpened("content://videos/a", video("a.mp4"), VideoUriPermissionStatus.Persisted)
        now = 200L
        history.recordOpened("content://videos/b", video("b.mp4"), VideoUriPermissionStatus.Persisted)

        assertEquals(
            listOf("b.mp4", "a.mp4"),
            history.entries(refreshAccess = false).map(RecentVideoRecord::displayName),
        )
    }

    @Test
    fun duplicateUriReopenUpdatesOneStableRecordAndRetainsPosition() = runTest {
        var now = 100L
        var idCalls = 0
        val store = MemoryStore()
        val history = repository(
            store = store,
            now = { now },
            id = { "id-${++idCalls}" },
        )

        val first = history.recordOpened(
            "content://videos/a",
            video("old.mp4"),
            VideoUriPermissionStatus.Transient,
        )
        history.updatePosition("content://videos/a", frameId = 47L, timestampUs = 1_500_000L)
        now = 500L
        val reopened = history.recordOpened(
            "content://videos/a",
            video("renamed.mp4", codec = "hevc"),
            VideoUriPermissionStatus.Persisted,
        )

        val entries = history.entries(refreshAccess = false)
        assertEquals(1, entries.size)
        assertEquals(first.id, reopened.id)
        assertEquals(first.id, entries.single().id)
        assertEquals("renamed.mp4", entries.single().displayName)
        assertEquals("hevc", entries.single().codec)
        assertEquals(47L, entries.single().lastViewedFrameId)
        assertEquals(1_500_000L, entries.single().lastViewedTimestampUs)
        assertEquals(1, idCalls)
    }

    @Test
    fun refreshMarksPersistedPermissionWhenDocumentIsReadable() = runTest {
        val store = MemoryStore()
        val access = FakeAccessChecker().apply {
            persisted["content://videos/a"] = true
        }
        val history = repository(store, access = access)
        history.recordOpened(
            "content://videos/a",
            video("a.mp4"),
            VideoUriPermissionStatus.Transient,
        )

        val refreshed = history.entries(refreshAccess = true).single()

        assertEquals(RecentVideoAvailability.Available, refreshed.availability)
        assertEquals(VideoUriPermissionStatus.Persisted, refreshed.permissionStatus)
        assertTrue(refreshed.canOpen())
    }

    @Test
    fun permissionLossMarksOnlyAffectedEntryUnavailable() = runTest {
        var now = 100L
        val store = MemoryStore()
        val access = FakeAccessChecker()
        val history = repository(store, access = access, now = { now })
        history.recordOpened("content://videos/a", video("a.mp4"), VideoUriPermissionStatus.Persisted)
        now = 200L
        history.recordOpened("content://videos/b", video("b.mp4"), VideoUriPermissionStatus.Persisted)
        access.availability["content://videos/a"] = RecentVideoAvailability.PermissionLost
        access.persisted["content://videos/b"] = true

        val refreshed = history.entries(refreshAccess = true)
        val lost = refreshed.single { it.contentUri == "content://videos/a" }
        val available = refreshed.single { it.contentUri == "content://videos/b" }

        assertEquals(2, refreshed.size)
        assertEquals(RecentVideoAvailability.PermissionLost, lost.availability)
        assertEquals(VideoUriPermissionStatus.Lost, lost.permissionStatus)
        assertFalse(lost.canOpen())
        assertEquals(RecentVideoAvailability.Available, available.availability)
        assertTrue(available.canOpen())
    }

    @Test
    fun missingDocumentRemainsInHistoryForReselectOrRemoval() = runTest {
        val store = MemoryStore()
        val access = FakeAccessChecker()
        val history = repository(store, access = access)
        val record = history.recordOpened(
            "content://videos/missing",
            video("missing.mp4"),
            VideoUriPermissionStatus.Persisted,
        )
        access.availability[record.contentUri] = RecentVideoAvailability.MissingDocument

        val refreshed = history.entries(refreshAccess = true).single()

        assertEquals(record.id, refreshed.id)
        assertEquals(RecentVideoAvailability.MissingDocument, refreshed.availability)
        assertFalse(refreshed.canOpen())
    }

    @Test
    fun removeOneDoesNotRemoveOtherHistory() = runTest {
        var now = 100L
        val store = MemoryStore()
        val history = repository(store, now = { now })
        val first = history.recordOpened(
            "content://videos/a",
            video("a.mp4"),
            VideoUriPermissionStatus.Persisted,
        )
        now = 200L
        val second = history.recordOpened(
            "content://videos/b",
            video("b.mp4"),
            VideoUriPermissionStatus.Persisted,
        )

        history.remove(first.id)

        assertEquals(listOf(second.id), history.entries(refreshAccess = false).map(RecentVideoRecord::id))
    }

    @Test
    fun clearRemovesAllHistoryRecords() = runTest {
        val store = MemoryStore()
        val history = repository(store)
        history.recordOpened("content://videos/a", video("a.mp4"), VideoUriPermissionStatus.Persisted)
        history.recordOpened("content://videos/b", video("b.mp4"), VideoUriPermissionStatus.Persisted)

        history.clear()

        assertTrue(history.entries(refreshAccess = false).isEmpty())
    }

    @Test
    fun lastPositionSurvivesRepositoryRecreation() = runTest {
        val store = MemoryStore()
        val firstProcess = repository(store)
        val opened = firstProcess.recordOpened(
            "content://videos/a",
            video("a.mp4"),
            VideoUriPermissionStatus.Persisted,
        )
        firstProcess.updatePosition(opened.contentUri, frameId = 321L, timestampUs = 12_345_678L)

        val recreatedProcess = repository(store)
        val restored = recreatedProcess.findById(opened.id)

        assertEquals(321L, restored?.lastViewedFrameId)
        assertEquals(12_345_678L, restored?.lastViewedTimestampUs)
    }

    @Test
    fun reselectPreservesStableIdAndResumePositionAndDeduplicatesUri() = runTest {
        var now = 100L
        val store = MemoryStore()
        val history = repository(store, now = { now })
        val original = history.recordOpened(
            "content://old-provider/video",
            video("clip.mp4"),
            VideoUriPermissionStatus.Persisted,
        )
        history.updatePosition(original.contentUri, 88L, 8_800_000L)
        now = 200L
        history.recordOpened(
            "content://new-provider/video",
            video("other.mp4"),
            VideoUriPermissionStatus.Persisted,
        )
        now = 300L

        val reselected = history.recordReselected(
            recordId = original.id,
            contentUri = "content://new-provider/video",
            video = video("clip.mp4"),
            permissionStatus = VideoUriPermissionStatus.Persisted,
        )

        val entries = history.entries(refreshAccess = false)
        assertEquals(1, entries.size)
        assertEquals(original.id, reselected.id)
        assertEquals(original.id, entries.single().id)
        assertEquals("content://new-provider/video", entries.single().contentUri)
        assertEquals(88L, entries.single().lastViewedFrameId)
        assertEquals(8_800_000L, entries.single().lastViewedTimestampUs)
    }

    @Test
    fun jsonRoundTripPreservesOptionalIntegrationFieldsAndResumeState() {
        val record = RecentVideoRecord(
            id = "stable-id",
            contentUri = "content://videos/a",
            permissionStatus = VideoUriPermissionStatus.Persisted,
            availability = RecentVideoAvailability.Available,
            displayName = "a.mp4",
            durationUs = 5_000_000L,
            width = 1920,
            height = 1080,
            codec = "h264",
            container = "mp4",
            lastOpenedEpochMs = 1234L,
            lastViewedFrameId = 17L,
            lastViewedTimestampUs = 566_667L,
            thumbnailUri = "content://framescope/thumbnail/a",
            extractionCount = 3,
        )

        val decoded = RecentVideoJsonCodec.decode(RecentVideoJsonCodec.encode(listOf(record)))

        assertEquals(listOf(record), decoded)
    }

    @Test
    fun malformedPersistenceFailsClosedToEmptyHistory() {
        assertTrue(RecentVideoJsonCodec.decode("not-json").isEmpty())
    }

    @Test
    fun invalidNegativeFramePositionDoesNotOverwriteGoodResumePoint() = runTest {
        val store = MemoryStore()
        val history = repository(store)
        val opened = history.recordOpened(
            "content://videos/a",
            video("a.mp4"),
            VideoUriPermissionStatus.Persisted,
        )
        history.updatePosition(opened.contentUri, 7L, 700_000L)

        history.updatePosition(opened.contentUri, -1L, 900_000L)

        val record = history.findById(opened.id)
        assertEquals(7L, record?.lastViewedFrameId)
        assertEquals(700_000L, record?.lastViewedTimestampUs)
    }

    @Test
    fun unknownRecordLookupReturnsNull() = runTest {
        assertNull(repository(MemoryStore()).findById("missing"))
    }

    private fun repository(
        store: RecentVideoStore,
        access: RecentVideoAccessChecker = FakeAccessChecker(),
        now: () -> Long = { 1_000L },
        id: () -> String = { "generated-id" },
    ): RecentVideoHistoryRepository = RecentVideoHistoryRepository(
        store = store,
        accessChecker = access,
        ioDispatcher = UnconfinedTestDispatcher(),
        clockEpochMs = now,
        idFactory = id,
    )

    private fun video(
        name: String,
        codec: String = "h264",
    ) = InspectedVideo(
        displayName = name,
        metadata = VideoMetadata(
            durationUs = 10_000_000L,
            width = 1920,
            height = 1080,
            estimatedFrameRate = 30.0,
            rotationDegrees = 0,
            container = "mp4",
            codec = codec,
        ),
        engine = "framescope-rust/test",
    )

    private class MemoryStore(
        initial: List<RecentVideoRecord> = emptyList(),
    ) : RecentVideoStore {
        private var records = initial.toList()

        override fun load(): List<RecentVideoRecord> = records.toList()

        override fun save(entries: List<RecentVideoRecord>) {
            records = entries.toList()
        }
    }

    private class FakeAccessChecker : RecentVideoAccessChecker {
        val availability = mutableMapOf<String, RecentVideoAvailability>()
        val persisted = mutableMapOf<String, Boolean>()

        override fun availability(contentUri: String): RecentVideoAvailability =
            availability[contentUri] ?: RecentVideoAvailability.Available

        override fun hasPersistedReadPermission(contentUri: String): Boolean =
            persisted[contentUri] ?: false
    }
}
