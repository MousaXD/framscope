package com.framescope.app.data

import org.json.JSONObject

enum class PersistentFrameIndexStatus {
    Indexed,
    InProgress,
    Stale,
}

data class PersistentFrameIndexDescriptor(
    val sourceKey: String,
    val streamIndex: Int,
    val relativePath: String,
    val status: PersistentFrameIndexStatus,
    val indexedFrames: Long,
    val frameCount: Long?,
    val lastModifiedEpochMs: Long?,
)

data class SessionIndexBinding(
    val sourceKey: String,
    val streamIndex: Int,
    val relativePath: String,
    val status: PersistentFrameIndexStatus,
    val indexedFrames: Long,
    val frameCount: Long?,
)

interface PersistentFrameIndexCatalog {
    fun entries(): List<PersistentFrameIndexDescriptor>
    fun bindingForSession(sessionId: Long): SessionIndexBinding?
}

object NoOpPersistentFrameIndexCatalog : PersistentFrameIndexCatalog {
    override fun entries(): List<PersistentFrameIndexDescriptor> = emptyList()
    override fun bindingForSession(sessionId: Long): SessionIndexBinding? = null
}

internal sealed interface NativeFrameIndexCatalogResponse {
    data class Success(val indexes: List<PersistentFrameIndexDescriptor>) : NativeFrameIndexCatalogResponse
    data class Failure(val code: String, val message: String) : NativeFrameIndexCatalogResponse
}

internal sealed interface NativeSessionIndexBindingResponse {
    data class Success(val binding: SessionIndexBinding) : NativeSessionIndexBindingResponse
    data class Failure(val code: String, val message: String) : NativeSessionIndexBindingResponse
}

internal interface NativeFrameIndexCatalogBridge {
    fun catalog(cacheRoot: String): NativeFrameIndexCatalogResponse
    fun sessionBinding(cacheRoot: String, sessionId: Long): NativeSessionIndexBindingResponse
}

internal object FrameIndexCatalogBridge : NativeFrameIndexCatalogBridge {
    private val loadFailure: Throwable? = runCatching {
        System.loadLibrary("framescope_ffi")
    }.exceptionOrNull()

    @JvmStatic
    private external fun nativeIndexCatalog(cacheRoot: String): String?

    @JvmStatic
    private external fun nativeSessionIndexBinding(cacheRoot: String, sessionId: Long): String?

    override fun catalog(cacheRoot: String): NativeFrameIndexCatalogResponse {
        loadFailure?.let { failure ->
            return NativeFrameIndexCatalogResponse.Failure(
                code = "native_load_failed",
                message = failure.message ?: "FrameScope native library could not be loaded.",
            )
        }
        return runCatching {
            parseFrameIndexCatalogResponse(nativeIndexCatalog(cacheRoot) ?: error("Native index catalog returned no response."))
        }.getOrElse { error ->
            NativeFrameIndexCatalogResponse.Failure(
                code = "bridge_error",
                message = error.message ?: "Native index catalog could not be decoded.",
            )
        }
    }

    override fun sessionBinding(
        cacheRoot: String,
        sessionId: Long,
    ): NativeSessionIndexBindingResponse {
        loadFailure?.let { failure ->
            return NativeSessionIndexBindingResponse.Failure(
                code = "native_load_failed",
                message = failure.message ?: "FrameScope native library could not be loaded.",
            )
        }
        return runCatching {
            parseSessionIndexBindingResponse(
                nativeSessionIndexBinding(cacheRoot, sessionId)
                    ?: error("Native session index binding returned no response."),
            )
        }.getOrElse { error ->
            NativeSessionIndexBindingResponse.Failure(
                code = "bridge_error",
                message = error.message ?: "Native session index binding could not be decoded.",
            )
        }
    }
}

class NativePersistentFrameIndexCatalog(
    private val cacheRoot: String,
    private val bridge: NativeFrameIndexCatalogBridge = FrameIndexCatalogBridge,
) : PersistentFrameIndexCatalog {
    override fun entries(): List<PersistentFrameIndexDescriptor> = when (val response = bridge.catalog(cacheRoot)) {
        is NativeFrameIndexCatalogResponse.Success -> response.indexes
        is NativeFrameIndexCatalogResponse.Failure -> throw IllegalStateException(
            "${response.code}: ${response.message}",
        )
    }

    override fun bindingForSession(sessionId: Long): SessionIndexBinding? =
        when (val response = bridge.sessionBinding(cacheRoot, sessionId)) {
            is NativeSessionIndexBindingResponse.Success -> response.binding
            is NativeSessionIndexBindingResponse.Failure -> when (response.code) {
                "non_persistent_index" -> null
                else -> throw IllegalStateException("${response.code}: ${response.message}")
            }
        }
}

internal fun parseFrameIndexCatalogResponse(payload: String): NativeFrameIndexCatalogResponse {
    val root = JSONObject(payload)
    return when (root.getString("status")) {
        "ok" -> {
            val indexes = root.getJSONArray("indexes")
            NativeFrameIndexCatalogResponse.Success(
                buildList {
                    for (index in 0 until indexes.length()) {
                        add(indexes.getJSONObject(index).toPersistentFrameIndexDescriptor())
                    }
                },
            )
        }
        "error" -> NativeFrameIndexCatalogResponse.Failure(
            code = root.optString("code", "index_catalog_error"),
            message = root.optString("message", "FrameScope could not read persistent indexes."),
        )
        else -> error("Native index catalog returned an unknown status.")
    }
}

internal fun parseSessionIndexBindingResponse(payload: String): NativeSessionIndexBindingResponse {
    val root = JSONObject(payload)
    return when (root.getString("status")) {
        "ok" -> NativeSessionIndexBindingResponse.Success(
            binding = root.getJSONObject("binding").toSessionIndexBinding(),
        )
        "error" -> NativeSessionIndexBindingResponse.Failure(
            code = root.optString("code", "index_binding_error"),
            message = root.optString("message", "FrameScope could not identify the current persistent index."),
        )
        else -> error("Native session index binding returned an unknown status.")
    }
}

private fun JSONObject.toPersistentFrameIndexDescriptor(): PersistentFrameIndexDescriptor =
    PersistentFrameIndexDescriptor(
        sourceKey = requiredSourceKey(),
        streamIndex = requiredStreamIndex(),
        relativePath = requiredRelativeIndexPath(),
        status = requiredIndexStatus(),
        indexedFrames = nonNegativeLong("indexed_frames"),
        frameCount = optionalNonNegativeLong("frame_count"),
        lastModifiedEpochMs = optionalNonNegativeLong("last_modified_epoch_ms"),
    )

private fun JSONObject.toSessionIndexBinding(): SessionIndexBinding = SessionIndexBinding(
    sourceKey = requiredSourceKey(),
    streamIndex = requiredStreamIndex(),
    relativePath = requiredRelativeIndexPath(),
    status = requiredIndexStatus(),
    indexedFrames = nonNegativeLong("indexed_frames"),
    frameCount = optionalNonNegativeLong("frame_count"),
)

private fun JSONObject.requiredSourceKey(): String = getString("source_key").also { value ->
    require(value.length in 1..256 && value.all { it.isLetterOrDigit() || it == '-' || it == '_' }) {
        "Native frame-index source key is invalid."
    }
}

private fun JSONObject.requiredStreamIndex(): Int = getInt("stream_index").also { value ->
    require(value >= 0) { "Native frame-index stream index must not be negative." }
}

private fun JSONObject.requiredRelativeIndexPath(): String = getString("relative_path").also { value ->
    require(
        value.startsWith("frame-index/") &&
            !value.startsWith("/") &&
            value.split('/').none { it == ".." },
    ) { "Native frame-index path escaped its owned cache namespace." }
}

private fun JSONObject.requiredIndexStatus(): PersistentFrameIndexStatus = when (getString("status")) {
    "indexed" -> PersistentFrameIndexStatus.Indexed
    "in_progress" -> PersistentFrameIndexStatus.InProgress
    "stale" -> PersistentFrameIndexStatus.Stale
    else -> error("Native frame-index status is unknown.")
}

private fun JSONObject.nonNegativeLong(name: String): Long = getLong(name).also { value ->
    require(value >= 0L) { "Native frame-index field $name must not be negative." }
}

private fun JSONObject.optionalNonNegativeLong(name: String): Long? =
    if (!has(name) || isNull(name)) {
        null
    } else {
        nonNegativeLong(name)
    }
