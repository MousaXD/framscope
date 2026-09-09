package com.framescope.app

import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.PersistentFrameIndexCatalog
import com.framescope.app.data.PersistentFrameIndexDescriptor
import com.framescope.app.data.PersistentFrameIndexStatus
import com.framescope.app.data.RecentVideoAccessChecker
import com.framescope.app.data.RecentVideoAvailability
import com.framescope.app.data.RecentVideoHistoryRepository
import com.framescope.app.data.RecentVideoRecord
import com.framescope.app.data.RecentVideoStore
import com.framescope.app.data.SessionIndexBinding
import com.framescope.app.data.VideoMetadata
import com.framescope.app.data.VideoUriPermissionStatus
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class MediaLibraryReselectIdentityTest {
    @Test
    fun locatingIndexOnlySourceGetsIndependentRecordIdUntilNativeProof() = runTest {
        val catalog = FakeCatalog(mutableListOf(descriptor("source-a", 0, 42)))
        val history = repository(catalog)
        val indexOnly = history.entries(refreshAccess = false).single()

        val located = history.recordReselected(
            recordId = indexOnly.id,
            contentUri = "content://provider/located",
            video = video("located.mp4"),
            permissionStatus = VideoUriPermissionStatus.Persisted,
        )

        assertNotEquals(indexOnly.id, located.id)
        val beforeProof = history.entries(refreshAccess = false)
        assertEquals(2, beforeProof.size)
        assertTrue(beforeProof.any { it.id == indexOnly.id && it.contentUri == null })
        assertTrue(beforeProof.any { it.id == located.id && it.contentUri == "content://provider/located" })

        history.updateIndexBinding(
            "content://provider/located",
            binding("source-a", 0, 42),
        )

        val afterProof = history.entries(refreshAccess = false)
        assertEquals(1, afterProof.size)
        assertEquals(located.id, afterProof.single().id)
        assertEquals("source-a", afterProof.single().sourceIdentityKey)
    }

    @Test
    fun locatingDifferentSourceKeepsOriginalIndexAndDistinctLibraryIdentity() = runTest {
        val catalog = FakeCatalog(mutableListOf(descriptor("source-a", 0, 42)))
        val history = repository(catalog)
        val indexOnlyA = history.entries(refreshAccess = false).single()

        val locatedB = history.recordReselected(
            recordId = indexOnlyA.id,
            contentUri = "content://provider/different",
            video = video("different.mp4"),
            permissionStatus = VideoUriPermissionStatus.Persisted,
        )
        catalog.descriptors += descriptor("source-b", 0, 84)
        history.updateIndexBinding(
            "content://provider/different",
            binding("source-b", 0, 84),
        )

        val entries = history.entries(refreshAccess = false)

        assertEquals(2, entries.size)
        assertEquals(2, entries.map(RecentVideoRecord::id).toSet().size)
        assertTrue(entries.any { it.contentUri == null && it.sourceIdentityKey == "source-a" })
        assertTrue(
            entries.any {
                it.id == locatedB.id &&
                    it.contentUri == "content://provider/different" &&
                    it.sourceIdentityKey == "source-b"
            },
        )
    }

    private fun repository(catalog: PersistentFrameIndexCatalog): RecentVideoHistoryRepository =
        RecentVideoHistoryRepository(
            store = MemoryStore(),
            accessChecker = AlwaysReadable,
            indexCatalog = catalog,
            ioDispatcher = UnconfinedTestDispatcher(),
            clockEpochMs = { 1_000L },
            idFactory = { "located-source-record" },
        )

    private fun descriptor(
        sourceKey: String,
        streamIndex: Int,
        frameCount: Long,
    ) = PersistentFrameIndexDescriptor(
        sourceKey = sourceKey,
        streamIndex = streamIndex,
        relativePath = "frame-index/v1/$sourceKey/stream-$streamIndex.sqlite3",
        status = PersistentFrameIndexStatus.Indexed,
        indexedFrames = frameCount,
        frameCount = frameCount,
        lastModifiedEpochMs = 2_000L,
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

    private fun video(name: String) = InspectedVideo(
        displayName = name,
        metadata = VideoMetadata(
            durationUs = 10_000_000L,
            width = 1920,
            height = 1080,
            estimatedFrameRate = 30.0,
            rotationDegrees = 0,
            container = "mp4",
            codec = "h264",
        ),
        engine = "framescope-rust/test",
    )

    private class MemoryStore : RecentVideoStore {
        private var records = emptyList<RecentVideoRecord>()

        override fun load(): List<RecentVideoRecord> = records.toList()

        override fun save(entries: List<RecentVideoRecord>) {
            records = entries.toList()
        }
    }

    private object AlwaysReadable : RecentVideoAccessChecker {
        override fun availability(contentUri: String): RecentVideoAvailability =
            RecentVideoAvailability.Available

        override fun hasPersistedReadPermission(contentUri: String): Boolean = true
    }

    private class FakeCatalog(
        var descriptors: MutableList<PersistentFrameIndexDescriptor>,
    ) : PersistentFrameIndexCatalog {
        override fun entries(): List<PersistentFrameIndexDescriptor> = descriptors.toList()

        override fun bindingForSession(sessionId: Long): SessionIndexBinding? = null
    }
}
