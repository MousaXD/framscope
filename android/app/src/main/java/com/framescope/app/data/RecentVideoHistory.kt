package com.framescope.app.data

import android.content.ContentResolver
import android.content.Context
import android.net.Uri
import java.io.FileNotFoundException
import java.util.UUID
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

enum class VideoUriPermissionStatus {
    Persisted,
    Transient,
    Lost,
}

enum class RecentVideoAvailability {
    Available,
    PermissionLost,
    MissingDocument,
    SourceUnlinked,
}

enum class RecentVideoIndexStatus {
    Unknown,
    Available,
    InProgress,
    Missing,
    Stale,
}

enum class MediaLibraryCategory {
    Recent,
    Indexed,
    InProgress,
    PermissionLost,
    Missing,
    Stale,
}

data class RecentVideoRecord(
    val id: String,
    val contentUri: String?,
    val permissionStatus: VideoUriPermissionStatus,
    val availability: RecentVideoAvailability,
    val displayName: String,
    val durationUs: Long?,
    val width: Int,
    val height: Int,
    val codec: String?,
    val container: String?,
    val lastOpenedEpochMs: Long,
    val lastViewedFrameId: Long?,
    val lastViewedTimestampUs: Long?,
    val indexStatus: RecentVideoIndexStatus = RecentVideoIndexStatus.Unknown,
    val thumbnailUri: String? = null,
    val extractionCount: Int? = null,
    val sourceIdentityKey: String? = null,
    val indexStreamIndex: Int? = null,
    val indexId: String? = null,
    val indexRelativePath: String? = null,
    val indexedFrameCount: Long? = null,
    val indexLastModifiedEpochMs: Long? = null,
) {
    fun canOpen(): Boolean =
        contentUri != null && availability == RecentVideoAvailability.Available

    fun category(): MediaLibraryCategory = when {
        availability == RecentVideoAvailability.PermissionLost -> MediaLibraryCategory.PermissionLost
        availability == RecentVideoAvailability.MissingDocument -> MediaLibraryCategory.Missing
        indexStatus == RecentVideoIndexStatus.Stale -> MediaLibraryCategory.Stale
        indexStatus == RecentVideoIndexStatus.InProgress -> MediaLibraryCategory.InProgress
        indexStatus == RecentVideoIndexStatus.Available -> MediaLibraryCategory.Indexed
        else -> MediaLibraryCategory.Recent
    }
}

interface RecentVideoHistory {
    suspend fun entries(refreshAccess: Boolean = true): List<RecentVideoRecord>

    suspend fun findById(id: String): RecentVideoRecord?

    suspend fun recordOpened(
        contentUri: String,
        video: InspectedVideo,
        permissionStatus: VideoUriPermissionStatus,
    ): RecentVideoRecord

    suspend fun recordReselected(
        recordId: String,
        contentUri: String,
        video: InspectedVideo,
        permissionStatus: VideoUriPermissionStatus,
    ): RecentVideoRecord

    suspend fun updatePosition(
        contentUri: String,
        frameId: Long?,
        timestampUs: Long?,
    )

    suspend fun updateIndexBinding(
        contentUri: String,
        binding: SessionIndexBinding,
    )

    suspend fun remove(recordId: String)

    suspend fun clear()
}

object NoOpRecentVideoHistory : RecentVideoHistory {
    override suspend fun entries(refreshAccess: Boolean): List<RecentVideoRecord> = emptyList()

    override suspend fun findById(id: String): RecentVideoRecord? = null

    override suspend fun recordOpened(
        contentUri: String,
        video: InspectedVideo,
        permissionStatus: VideoUriPermissionStatus,
    ): RecentVideoRecord = recentVideoRecord(
        id = "noop",
        contentUri = contentUri,
        video = video,
        permissionStatus = permissionStatus,
        openedAtEpochMs = 0L,
    )

    override suspend fun recordReselected(
        recordId: String,
        contentUri: String,
        video: InspectedVideo,
        permissionStatus: VideoUriPermissionStatus,
    ): RecentVideoRecord = recordOpened(contentUri, video, permissionStatus)

    override suspend fun updatePosition(contentUri: String, frameId: Long?, timestampUs: Long?) = Unit

    override suspend fun updateIndexBinding(contentUri: String, binding: SessionIndexBinding) = Unit

    override suspend fun remove(recordId: String) = Unit

    override suspend fun clear() = Unit
}

interface RecentVideoStore {
    fun load(): List<RecentVideoRecord>
    fun save(entries: List<RecentVideoRecord>)
}

interface RecentVideoAccessChecker {
    fun availability(contentUri: String): RecentVideoAvailability
    fun hasPersistedReadPermission(contentUri: String): Boolean
}

class AndroidRecentVideoAccessChecker(
    private val contentResolver: ContentResolver,
) : RecentVideoAccessChecker {
    override fun availability(contentUri: String): RecentVideoAvailability {
        val uri = Uri.parse(contentUri)
        if (uri.scheme != ContentResolver.SCHEME_CONTENT) {
            return RecentVideoAvailability.MissingDocument
        }
        return try {
            val descriptor = contentResolver.openFileDescriptor(uri, "r")
            if (descriptor == null) {
                RecentVideoAvailability.MissingDocument
            } else {
                descriptor.close()
                RecentVideoAvailability.Available
            }
        } catch (_: SecurityException) {
            RecentVideoAvailability.PermissionLost
        } catch (_: FileNotFoundException) {
            RecentVideoAvailability.MissingDocument
        } catch (_: IllegalArgumentException) {
            RecentVideoAvailability.MissingDocument
        }
    }

    override fun hasPersistedReadPermission(contentUri: String): Boolean {
        val uri = Uri.parse(contentUri)
        return contentResolver.persistedUriPermissions.any { permission ->
            permission.uri == uri && permission.isReadPermission
        }
    }
}

class SharedPreferencesRecentVideoStore(
    context: Context,
) : RecentVideoStore {
    private val preferences = context.getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)

    override fun load(): List<RecentVideoRecord> {
        val encoded = preferences.getString(KEY_HISTORY, null) ?: return emptyList()
        return RecentVideoJsonCodec.decode(encoded)
    }

    override fun save(entries: List<RecentVideoRecord>) {
        check(preferences.edit().putString(KEY_HISTORY, RecentVideoJsonCodec.encode(entries)).commit()) {
            "Android could not persist FrameScope media-library metadata."
        }
    }

    private companion object {
        const val PREFERENCES_NAME = "framescope_recent_videos"
        const val KEY_HISTORY = "history_v1"
    }
}

class RecentVideoHistoryRepository(
    private val store: RecentVideoStore,
    private val accessChecker: RecentVideoAccessChecker,
    private val indexCatalog: PersistentFrameIndexCatalog = NoOpPersistentFrameIndexCatalog,
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
    private val clockEpochMs: () -> Long = System::currentTimeMillis,
    private val idFactory: () -> String = { UUID.randomUUID().toString() },
) : RecentVideoHistory {
    private val mutex = Mutex()

    override suspend fun entries(refreshAccess: Boolean): List<RecentVideoRecord> = serialized {
        libraryEntries(refreshAccess)
    }

    override suspend fun findById(id: String): RecentVideoRecord? = serialized {
        libraryEntries(refreshAccess = false).firstOrNull { it.id == id }
    }

    override suspend fun recordOpened(
        contentUri: String,
        video: InspectedVideo,
        permissionStatus: VideoUriPermissionStatus,
    ): RecentVideoRecord = serialized {
        requireContentUri(contentUri)
        val existing = store.load().firstOrNull { it.contentUri == contentUri }
        val record = recentVideoRecord(
            id = existing?.id ?: idFactory(),
            contentUri = contentUri,
            video = video,
            permissionStatus = permissionStatus,
            openedAtEpochMs = clockEpochMs(),
            previous = existing,
        )
        saveUpsert(record)
        record
    }

    override suspend fun recordReselected(
        recordId: String,
        contentUri: String,
        video: InspectedVideo,
        permissionStatus: VideoUriPermissionStatus,
    ): RecentVideoRecord = serialized {
        require(recordId.isNotBlank()) { "Media-library record id must not be blank." }
        requireContentUri(contentUri)
        val current = store.load()
        val previous = current.firstOrNull { it.id == recordId }
            ?: currentCatalogOrNull()
                ?.firstOrNull { indexRecordId(it) == recordId }
                ?.let(::indexOnlyRecord)
            ?: throw IllegalArgumentException("Media-library record no longer exists.")
        val record = recentVideoRecord(
            id = previous.id,
            contentUri = contentUri,
            video = video,
            permissionStatus = permissionStatus,
            openedAtEpochMs = clockEpochMs(),
            previous = previous,
        )
        val deduplicated = current.filterNot { it.id == recordId || it.contentUri == contentUri }
        store.save(normalizedStored(deduplicated + record))
        record
    }

    override suspend fun updatePosition(
        contentUri: String,
        frameId: Long?,
        timestampUs: Long?,
    ) = serialized {
        if (frameId != null && frameId < 0L) return@serialized
        val current = store.load()
        var changed = false
        val updated = current.map { record ->
            if (record.contentUri != contentUri) return@map record
            changed = true
            record.copy(
                lastViewedFrameId = frameId,
                lastViewedTimestampUs = timestampUs,
            )
        }
        if (changed) store.save(normalizedStored(updated))
    }

    override suspend fun updateIndexBinding(
        contentUri: String,
        binding: SessionIndexBinding,
    ) = serialized {
        requireContentUri(contentUri)
        val current = store.load()
        val target = current.firstOrNull { it.contentUri == contentUri } ?: return@serialized
        val updated = target.copy(
            sourceIdentityKey = binding.sourceKey,
            indexStreamIndex = binding.streamIndex,
            indexId = indexRecordId(binding.sourceKey, binding.streamIndex),
            indexRelativePath = binding.relativePath,
            indexStatus = binding.status.toRecentStatus(),
            indexedFrameCount = binding.frameCount ?: binding.indexedFrames,
        )
        val deduplicated = current.filterNot { record ->
            record.id == target.id ||
                (record.sourceIdentityKey == binding.sourceKey &&
                    record.indexStreamIndex == binding.streamIndex)
        }
        store.save(normalizedStored(deduplicated + updated))
    }

    override suspend fun remove(recordId: String) = serialized {
        val current = store.load()
        val updated = current.filterNot { it.id == recordId }
        if (updated.size != current.size) store.save(normalizedStored(updated))
    }

    override suspend fun clear() = serialized {
        if (store.load().isNotEmpty()) store.save(emptyList())
    }

    private fun libraryEntries(refreshAccess: Boolean): List<RecentVideoRecord> {
        var stored = normalizedStored(store.load())
        if (refreshAccess) {
            var changed = false
            stored = stored.map { record ->
                val contentUri = record.contentUri ?: return@map record
                val availability = accessChecker.availability(contentUri)
                val permissionStatus = when {
                    availability == RecentVideoAvailability.PermissionLost -> VideoUriPermissionStatus.Lost
                    accessChecker.hasPersistedReadPermission(contentUri) -> VideoUriPermissionStatus.Persisted
                    else -> VideoUriPermissionStatus.Transient
                }
                val updated = record.copy(
                    availability = availability,
                    permissionStatus = permissionStatus,
                )
                if (updated != record) changed = true
                updated
            }
            if (changed) store.save(stored)
        }

        val catalog = currentCatalogOrNull() ?: return sortLibrary(stored)
        val descriptorsByKey = catalog.associateBy { it.sourceKey to it.streamIndex }
        var storedChanged = false
        val reconciledStored = stored.map { record ->
            val sourceKey = record.sourceIdentityKey ?: return@map record
            val streamIndex = record.indexStreamIndex ?: return@map record
            val descriptor = descriptorsByKey[sourceKey to streamIndex]
            val updated = if (descriptor == null) {
                record.copy(
                    indexStatus = RecentVideoIndexStatus.Missing,
                    indexRelativePath = null,
                    indexedFrameCount = null,
                    indexLastModifiedEpochMs = null,
                )
            } else {
                record.copy(
                    indexId = indexRecordId(descriptor),
                    indexRelativePath = descriptor.relativePath,
                    indexStatus = descriptor.status.toRecentStatus(),
                    indexedFrameCount = descriptor.frameCount ?: descriptor.indexedFrames,
                    indexLastModifiedEpochMs = descriptor.lastModifiedEpochMs,
                )
            }
            if (updated != record) storedChanged = true
            updated
        }
        if (storedChanged) store.save(normalizedStored(reconciledStored))

        val represented = reconciledStored.mapNotNull { record ->
            val sourceKey = record.sourceIdentityKey ?: return@mapNotNull null
            val streamIndex = record.indexStreamIndex ?: return@mapNotNull null
            sourceKey to streamIndex
        }.toSet()
        val indexOnly = catalog
            .filterNot { descriptor -> (descriptor.sourceKey to descriptor.streamIndex) in represented }
            .map(::indexOnlyRecord)
        return sortLibrary(reconciledStored + indexOnly)
    }

    private fun currentCatalogOrNull(): List<PersistentFrameIndexDescriptor>? =
        runCatching(indexCatalog::entries).getOrNull()

    private fun saveUpsert(record: RecentVideoRecord) {
        val current = store.load()
        val updated = current.filterNot { it.id == record.id || it.contentUri == record.contentUri } + record
        store.save(normalizedStored(updated))
    }

    private fun normalizedStored(entries: List<RecentVideoRecord>): List<RecentVideoRecord> =
        entries
            .filter { it.contentUri != null }
            .distinctBy { it.id }
            .sortedByDescending { it.lastOpenedEpochMs }
            .take(MAX_RECENT_ITEMS)

    private fun sortLibrary(entries: List<RecentVideoRecord>): List<RecentVideoRecord> =
        entries.sortedWith(
            compareByDescending<RecentVideoRecord> {
                maxOf(it.lastOpenedEpochMs, it.indexLastModifiedEpochMs ?: 0L)
            }.thenBy { it.id },
        )

    private suspend fun <T> serialized(block: () -> T): T = withContext(ioDispatcher) {
        mutex.withLock { block() }
    }

    private fun requireContentUri(contentUri: String) {
        require(isContentUri(contentUri)) {
            "Media-library source records accept only content:// URIs."
        }
    }

    private companion object {
        const val MAX_RECENT_ITEMS = 100
    }
}

private fun indexOnlyRecord(descriptor: PersistentFrameIndexDescriptor): RecentVideoRecord =
    RecentVideoRecord(
        id = indexRecordId(descriptor),
        contentUri = null,
        permissionStatus = VideoUriPermissionStatus.Lost,
        availability = RecentVideoAvailability.SourceUnlinked,
        displayName = "Indexed video ${descriptor.sourceKey.take(8)}",
        durationUs = null,
        width = 0,
        height = 0,
        codec = null,
        container = null,
        lastOpenedEpochMs = 0L,
        lastViewedFrameId = null,
        lastViewedTimestampUs = null,
        indexStatus = descriptor.status.toRecentStatus(),
        sourceIdentityKey = descriptor.sourceKey,
        indexStreamIndex = descriptor.streamIndex,
        indexId = indexRecordId(descriptor),
        indexRelativePath = descriptor.relativePath,
        indexedFrameCount = descriptor.frameCount ?: descriptor.indexedFrames,
        indexLastModifiedEpochMs = descriptor.lastModifiedEpochMs,
    )

private fun PersistentFrameIndexStatus.toRecentStatus(): RecentVideoIndexStatus = when (this) {
    PersistentFrameIndexStatus.Indexed -> RecentVideoIndexStatus.Available
    PersistentFrameIndexStatus.InProgress -> RecentVideoIndexStatus.InProgress
    PersistentFrameIndexStatus.Stale -> RecentVideoIndexStatus.Stale
}

private fun indexRecordId(descriptor: PersistentFrameIndexDescriptor): String =
    indexRecordId(descriptor.sourceKey, descriptor.streamIndex)

private fun indexRecordId(sourceKey: String, streamIndex: Int): String =
    "index:$sourceKey:$streamIndex"

internal fun recentVideoRecord(
    id: String,
    contentUri: String,
    video: InspectedVideo,
    permissionStatus: VideoUriPermissionStatus,
    openedAtEpochMs: Long,
    previous: RecentVideoRecord? = null,
): RecentVideoRecord = RecentVideoRecord(
    id = id,
    contentUri = contentUri,
    permissionStatus = permissionStatus,
    availability = RecentVideoAvailability.Available,
    displayName = video.displayName,
    durationUs = video.metadata.durationUs,
    width = video.metadata.width,
    height = video.metadata.height,
    codec = video.metadata.codec,
    container = video.metadata.container,
    lastOpenedEpochMs = openedAtEpochMs,
    lastViewedFrameId = previous?.lastViewedFrameId,
    lastViewedTimestampUs = previous?.lastViewedTimestampUs,
    // URI text, display name, and old history state are not source-identity proof. A fresh open must
    // remain unbound until the native microscope session exposes its verified persistent index.
    indexStatus = RecentVideoIndexStatus.Unknown,
    thumbnailUri = previous?.thumbnailUri,
    extractionCount = previous?.extractionCount,
    sourceIdentityKey = null,
    indexStreamIndex = null,
    indexId = null,
    indexRelativePath = null,
    indexedFrameCount = null,
    indexLastModifiedEpochMs = null,
)

internal object RecentVideoJsonCodec {
    private const val VERSION = 2

    fun encode(entries: List<RecentVideoRecord>): String {
        val items = JSONArray()
        entries.forEach { record -> items.put(encodeRecord(record)) }
        return JSONObject()
            .put("version", VERSION)
            .put("items", items)
            .toString()
    }

    fun decode(encoded: String): List<RecentVideoRecord> = runCatching {
        val root = JSONObject(encoded)
        val version = root.optInt("version", -1)
        if (version !in setOf(1, VERSION)) return@runCatching emptyList()
        val items = root.optJSONArray("items") ?: return@runCatching emptyList()
        buildList {
            for (index in 0 until items.length()) {
                val item = items.optJSONObject(index) ?: continue
                decodeRecord(item, version)?.let(::add)
            }
        }
    }.getOrElse { emptyList() }

    private fun encodeRecord(record: RecentVideoRecord): JSONObject = JSONObject()
        .put("id", record.id)
        .putNullable("contentUri", record.contentUri)
        .put("permissionStatus", record.permissionStatus.name)
        .put("availability", record.availability.name)
        .put("displayName", record.displayName)
        .putNullable("durationUs", record.durationUs)
        .put("width", record.width)
        .put("height", record.height)
        .putNullable("codec", record.codec)
        .putNullable("container", record.container)
        .put("lastOpenedEpochMs", record.lastOpenedEpochMs)
        .putNullable("lastViewedFrameId", record.lastViewedFrameId)
        .putNullable("lastViewedTimestampUs", record.lastViewedTimestampUs)
        .put("indexStatus", record.indexStatus.name)
        .putNullable("thumbnailUri", record.thumbnailUri)
        .putNullable("extractionCount", record.extractionCount)
        .putNullable("sourceIdentityKey", record.sourceIdentityKey)
        .putNullable("indexStreamIndex", record.indexStreamIndex)
        .putNullable("indexId", record.indexId)
        .putNullable("indexRelativePath", record.indexRelativePath)
        .putNullable("indexedFrameCount", record.indexedFrameCount)
        .putNullable("indexLastModifiedEpochMs", record.indexLastModifiedEpochMs)

    private fun decodeRecord(json: JSONObject, version: Int): RecentVideoRecord? = runCatching {
        val id = json.getString("id").takeIf { it.isNotBlank() } ?: return@runCatching null
        val contentUri = if (version == 1) {
            json.getString("contentUri")
        } else {
            json.optNullableString("contentUri")
        }
        if (contentUri != null && !isContentUri(contentUri)) return@runCatching null
        if (version == 1 && contentUri == null) return@runCatching null
        val displayName = json.getString("displayName").takeIf { it.isNotBlank() } ?: "Selected video"
        RecentVideoRecord(
            id = id,
            contentUri = contentUri,
            permissionStatus = enumValueOrDefault(
                json.optString("permissionStatus"),
                VideoUriPermissionStatus.Transient,
            ),
            availability = enumValueOrDefault(
                json.optString("availability"),
                if (contentUri == null) RecentVideoAvailability.SourceUnlinked else RecentVideoAvailability.Available,
            ),
            displayName = displayName,
            durationUs = json.optNullableLong("durationUs"),
            width = json.getInt("width"),
            height = json.getInt("height"),
            codec = json.optNullableString("codec"),
            container = json.optNullableString("container"),
            lastOpenedEpochMs = json.getLong("lastOpenedEpochMs"),
            lastViewedFrameId = json.optNullableLong("lastViewedFrameId"),
            lastViewedTimestampUs = json.optNullableLong("lastViewedTimestampUs"),
            indexStatus = enumValueOrDefault(
                json.optString("indexStatus"),
                RecentVideoIndexStatus.Unknown,
            ),
            thumbnailUri = json.optNullableString("thumbnailUri"),
            extractionCount = json.optNullableInt("extractionCount"),
            sourceIdentityKey = json.optNullableString("sourceIdentityKey"),
            indexStreamIndex = json.optNullableInt("indexStreamIndex"),
            indexId = json.optNullableString("indexId"),
            indexRelativePath = json.optNullableString("indexRelativePath"),
            indexedFrameCount = json.optNullableLong("indexedFrameCount"),
            indexLastModifiedEpochMs = json.optNullableLong("indexLastModifiedEpochMs"),
        )
    }.getOrNull()

    private inline fun <reified T : Enum<T>> enumValueOrDefault(value: String, default: T): T =
        enumValues<T>().firstOrNull { it.name == value } ?: default

    private fun JSONObject.putNullable(name: String, value: Any?): JSONObject =
        put(name, value ?: JSONObject.NULL)

    private fun JSONObject.optNullableLong(name: String): Long? =
        if (!has(name) || isNull(name)) null else getLong(name)

    private fun JSONObject.optNullableInt(name: String): Int? =
        if (!has(name) || isNull(name)) null else getInt(name)

    private fun JSONObject.optNullableString(name: String): String? =
        if (!has(name) || isNull(name)) null else getString(name)
}

private fun isContentUri(value: String): Boolean =
    value.startsWith("content://") && value.length > "content://".length
