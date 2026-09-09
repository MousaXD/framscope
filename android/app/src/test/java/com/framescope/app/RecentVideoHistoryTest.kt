package com.framescope.app

import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.PersistentFrameIndexCatalog
import com.framescope.app.data.PersistentFrameIndexDescriptor
import com.framescope.app.data.PersistentFrameIndexStatus
import com.framescope.app.data.RecentVideoAccessChecker
import com.framescope.app.data.RecentVideoAvailability
import com.framescope.app.data.RecentVideoHistoryRepository
import com.framescope.app.data.RecentVideoIndexStatus
import com.framescope.app.data.RecentVideoJsonCodec
import com.framescope.app.data.RecentVideoRecord
import com.framescope.app.data.RecentVideoStore
import com.framescope.app.data.SessionIndexBinding
import com.framescope.app.data.VideoMetadata
import com.framescope.app.data.VideoUriPermissionStatus
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class RecentVideoHistoryTest {
    @Test
    fun threeIndexesWithZeroHistoryAreStillVisible() = runTest {
        val catalog = FakeCatalog(
            descriptors = listOf(
                descriptor("source-a", 0, 10),
                descriptor("source-b", 0, 20),
                descriptor("source-c", 1, 30),
            ),
        )
        val history = repository(MemoryStore(), catalog = catalog)

        val entries = history.entries(refreshAccess = false)

        assertEquals(3, entries.size)
        assertEquals(setOf(10L, 20L, 30L), entries.mapNotNull { it.indexedFrameCount }.toSet())
        assertTrue(entries.all { it.contentUri == null })
        assertTrue(entries.all { it.availability == RecentVideoAvailability.SourceUnlinked })
        assertTrue(entries.all { it.indexStatus == RecentVideoIndexStatus.Available })
        assertTrue(entries.none(RecentVideoRecord::canOpen))
    }

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
    fun duplicateUriReopenRetainsResumeButRequiresFreshIndexProof() = runTest {
        var now = 100L
        val store = MemoryStore()
        val history = repository(store, now = { now })
        val first = history.recordOpened(
            "content://videos/a",
            video("old.mp4"),
            VideoUriPermissionStatus.Persisted,
        )
        history.updatePosition("content://videos/a", frameId = 47L, timestampUs = 1_500_000L)
        history.updateIndexBinding("content://videos/a", binding("source-a", 0, 100))
        now = 500L

        val reopened = history.recordOpened(
            "content://videos/a",
            video("renamed.mp4", codec = "hevc"),
            VideoUriPermissionStatus.Persisted,
        )

        assertEquals(first.id, reopened.id)
        assertEquals(47L, reopened.lastViewedFrameId)
        assertEquals(1_500_000L, reopened.lastViewedTimestampUs)
        assertEquals(RecentVideoIndexStatus.Unknown, reopened.indexStatus)
        assertNull(reopened.sourceIdentityKey)
        assertNull(reopened.indexRelativePath)
    }

    @Test
    fun permissionLossDoesNotFabricateSourceAccessOrLoseKnownIndex() = runTest {
        val store = MemoryStore()
        val access = FakeAccessChecker()
        val catalog = FakeCatalog(listOf(descriptor("source-a", 0, 120)))
        val history = repository(store, access = access, catalog = catalog)
        history.recordOpened(
            "content://videos/a",
            video("a.mp4"),
            VideoUriPermissionStatus.Persisted,
        )
        history.updateIndexBinding("content://videos/a", binding("source-a", 0, 120))
        access.availability["content://videos/a"] = RecentVideoAvailability.PermissionLost

        val refreshed = history.entries(refreshAccess = true).single()

        assertEquals(RecentVideoAvailability.PermissionLost, refreshed.availability)
        assertEquals(VideoUriPermissionStatus.Lost, refreshed.permissionStatus)
        assertEquals(RecentVideoIndexStatus.Available, refreshed.indexStatus)
        assertFalse(refreshed.canOpen())
    }

    @Test
    fun missingDocumentRemainsVisibleForReselect() = runTest {
        val store = MemoryStore()
        val access = FakeAccessChecker()
        val history = repository(store, access = access)
        val record = history.recordOpened(
            "content://videos/missing",
            video("missing.mp4"),
            VideoUriPermissionStatus.Persisted,
        )
        access.availability[requireNotNull(record.contentUri)] = RecentVideoAvailability.MissingDocument

        val refreshed = history.entries(refreshAccess = true).single()

        assertEquals(record.id, refreshed.id)
        assertEquals(RecentVideoAvailability.MissingDocument, refreshed.availability)
        assertFalse(refreshed.canOpen())
    }

    @Test
    fun restartReconcilesStoredSourceWithPersistentIndexAndResumePosition() = runTest {
        val store = MemoryStore()
        val catalog = FakeCatalog(listOf(descriptor("source-a", 0, 321)))
        val firstProcess = repository(store, catalog = catalog)
        val opened = firstProcess.recordOpened(
            "content://videos/a",
            video("a.mp4"),
            VideoUriPermissionStatus.Persisted,
        )
        firstProcess.updatePosition(
            requireNotNull(opened.contentUri),
            frameId = 320L,
            timestampUs = 12_345_678L,
        )
        firstProcess.updateIndexBinding(
            requireNotNull(opened.contentUri),
            binding("source-a", 0, 321),
        )

        val recreatedProcess = repository(store, catalog = catalog)
        val entries = recreatedProcess.entries(refreshAccess = false)

        assertEquals(1, entries.size)
        val restored = entries.single()
        assertEquals(opened.id, restored.id)
        assertEquals("content://videos/a", restored.contentUri)
        assertEquals(320L, restored.lastViewedFrameId)
        assertEquals(12_345_678L, restored.lastViewedTimestampUs)
        assertEquals("source-a", restored.sourceIdentityKey)
        assertEquals(321L, restored.indexedFrameCount)
        assertEquals(RecentVideoIndexStatus.Available, restored.indexStatus)
    }

    @Test
    fun clearingRecentActivityLeavesPersistentIndexesDiscoverable() = runTest {
        val store = MemoryStore()
        val catalog = FakeCatalog(listOf(descriptor("source-a", 0, 42)))
        val history = repository(store, catalog = catalog)
        history.recordOpened("content://videos/a", video("a.mp4"), VideoUriPermissionStatus.Persisted)
        history.updateIndexBinding("content://videos/a", binding("source-a", 0, 42))

        history.clear()
        val entries = history.entries(refreshAccess = false)

        assertEquals(1, entries.size)
        assertNull(entries.single().contentUri)
        assertEquals(RecentVideoIndexStatus.Available, entries.single().indexStatus)
    }

    @Test
    fun indexOnlyReselectUsesIndependentRecordIdUntilVerifiedBinding() = runTest {
        val catalog = FakeCatalog(listOf(descriptor("source-a", 0, 42)))
        val history = repository(MemoryStore(), catalog = catalog)
        val indexOnly = history.entries(refreshAccess = false).single()

        val reselected = history.recordReselected(
            recordId = indexOnly.id,
            contentUri = "content://provider/video",
            video = video("clip.mp4"),
            permissionStatus = VideoUriPermissionStatus.Persisted,
        )

        assertTrue(indexOnly.id != reselected.id)
        assertEquals(RecentVideoIndexStatus.Unknown, reselected.indexStatus)
        assertNull(reselected.sourceIdentityKey)
        val beforeProof = history.entries(refreshAccess = false)
        assertEquals(2, beforeProof.size)
        assertTrue(beforeProof.any { it.contentUri == null && it.sourceIdentityKey == "source-a" })

        history.updateIndexBinding("content://provider/video", binding("source-a", 0, 42))
        val afterProof = history.entries(refreshAccess = false)
        assertEquals(1, afterProof.size)
        assertEquals(reselected.id, afterProof.single().id)
        assertEquals("content://provider/video", afterProof.single().contentUri)
        assertEquals("source-a", afterProof.single().sourceIdentityKey)
    }

    @Test
    fun catalogMarksPreviouslyLinkedMissingIndexWithoutDiscardingSourceRecord() = runTest {
        val store = MemoryStore()
        val catalog = FakeCatalog(listOf(descriptor("source-a", 0, 12)))
        val history = repository(store, catalog = catalog)
        history.recordOpened("content://videos/a", video("a.mp4"), VideoUriPermissionStatus.Persisted)
        history.updateIndexBinding("content://videos/a", binding("source-a", 0, 12))
        catalog.descriptors = emptyList()

        val refreshed = history.entries(refreshAccess = false).single()

        assertEquals("content://videos/a", refreshed.contentUri)
        assertEquals(RecentVideoIndexStatus.Missing, refreshed.indexStatus)
        assertNull(refreshed.indexRelativePath)
    }

    @Test
    fun v1PersistenceMigratesWithoutInventingIndexIdentity() {
        val encoded = """
            {"version":1,"items":[{"id":"legacy","contentUri":"content://videos/a","permissionStatus":"Persisted","availability":"Available","displayName":"a.mp4","durationUs":1000000,"width":640,"height":360,"codec":"h264","container":"mp4","lastOpenedEpochMs":5,"lastViewedFrameId":2,"lastViewedTimestampUs":66666,"indexStatus":"Unknown","thumbnailUri":null,"extractionCount":null}]}
        """.trimIndent()

        val decoded = RecentVideoJsonCodec.decode(encoded).single()

        assertEquals("legacy", decoded.id)
        assertEquals("content://videos/a", decoded.contentUri)
        assertNull(decoded.sourceIdentityKey)
        assertNull(decoded.indexRelativePath)
    }

    @Test
    fun jsonRoundTripPreservesLibraryAndResumeFields() {
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
            indexStatus = RecentVideoIndexStatus.Available,
            sourceIdentityKey = "source-a",
            indexStreamIndex = 0,
            indexId = "index:source-a:0",
            indexRelativePath = "frame-index/v1/source-a/stream-0.sqlite3",
            indexedFrameCount = 150L,
            indexLastModifiedEpochMs = 1200L,
        )

        val decoded = RecentVideoJsonCodec.decode(RecentVideoJsonCodec.encode(listOf(record)))

        assertEquals(listOf(record), decoded)
    }

    @Test
    fun malformedPersistenceFailsClosedToEmptyLibraryMetadata() {
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
        val uri = requireNotNull(opened.contentUri)
        history.updatePosition(uri, 7L, 700_000L)
        history.updatePosition(uri, -1L, 900_000L)

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
        catalog: PersistentFrameIndexCatalog = FakeCatalog(),
        now: () -> Long = { 1_000L },
    ): RecentVideoHistoryRepository = RecentVideoHistoryRepository(
        store = store,
        accessChecker = access,
        indexCatalog = catalog,
        ioDispatcher = UnconfinedTestDispatcher(),
        clockEpochMs = now,
        idFactory = uniqueIdFactory(),
    )

    private fun uniqueIdFactory(): () -> String {
        var nextId = 0
        return { "generated-${++nextId}" }
    }

    private fun descriptor(
        sourceKey: String,
        streamIndex: Int,
        frameCount: Long,
        status: PersistentFrameIndexStatus = PersistentFrameIndexStatus.Indexed,
    ) = PersistentFrameIndexDescriptor(
        sourceKey = sourceKey,
        streamIndex = streamIndex,
        relativePath = "frame-index/v1/$sourceKey/stream-$streamIndex.sqlite3",
        status = status,
        indexedFrames = frameCount,
        frameCount = if (status == PersistentFrameIndexStatus.Indexed) frameCount else null,
        lastModifiedEpochMs = 2_000L + frameCount,
    )

    private fun binding(
        sourceKey: String,
        streamIndex: Int,
        frameCount: Long,
    ) = SessionIndexBinding(
        sourceKey = sourceKey,
        streamIndex = streamIndex,
        relativePath = "frame-index/v1/$sourceKey/stream-$streamIndex.sqlite3",
        status = PersistentFrameIndexStatus.Indexed,
        indexedFrames = frameCount,
        frameCount = frameCount,
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

    private class FakeCatalog(
        var descriptors: List<PersistentFrameIndexDescriptor> = emptyList(),
        private var sessionBinding: SessionIndexBinding? = null,
    ) : PersistentFrameIndexCatalog {
        override fun entries(): List<PersistentFrameIndexDescriptor> = descriptors.toList()
        override fun bindingForSession(sessionId: Long): SessionIndexBinding? = sessionBinding
    }
}
