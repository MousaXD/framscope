package com.framescope.app.data

import org.json.JSONObject

data class StorageCategoryStats(
    val bytes: Long,
    val files: Long,
    val items: Long,
)

data class FrameScopeStorageStats(
    val totalBytes: Long,
    val persistentIndexes: StorageCategoryStats,
    val previewProxy: StorageCategoryStats,
    val disposable: StorageCategoryStats,
    val indexedSources: Long,
    val previewProxyEnabled: Boolean,
)

enum class StorageClearScope(internal val nativeCode: Int) {
    PreviewProxy(0),
    PersistentIndexes(1),
    Disposable(2),
    All(3),
}

data class StorageClearReceipt(
    val scope: StorageClearScope,
    val clearedBytes: Long,
    val clearedFiles: Long,
    val clearedItems: Long,
)

sealed interface NativeStorageResponse {
    data class Success(
        val storage: FrameScopeStorageStats,
        val cleared: StorageClearReceipt?,
    ) : NativeStorageResponse

    data class Failure(
        val code: String,
        val message: String,
        val engine: String?,
    ) : NativeStorageResponse
}

internal interface NativeFrameScopeStorageBridge {
    fun stats(cacheRoot: String): NativeStorageResponse
    fun clear(cacheRoot: String, scope: StorageClearScope): NativeStorageResponse
    fun clearSourceIndexes(cacheRoot: String, sourceKey: String): NativeStorageResponse
}

internal object FrameScopeStorageBridge : NativeFrameScopeStorageBridge {
    private val loadFailure: Throwable? = runCatching {
        System.loadLibrary("framescope_ffi")
    }.exceptionOrNull()

    @JvmStatic
    private external fun nativeStorageStats(cacheRoot: String): String?

    @JvmStatic
    private external fun nativeClearStorage(cacheRoot: String, scope: Int): String?

    @JvmStatic
    private external fun nativeClearSourceIndexes(cacheRoot: String, sourceKey: String): String?

    override fun stats(cacheRoot: String): NativeStorageResponse = invokeNative {
        nativeStorageStats(cacheRoot)
    }

    override fun clear(
        cacheRoot: String,
        scope: StorageClearScope,
    ): NativeStorageResponse = invokeNative {
        nativeClearStorage(cacheRoot, scope.nativeCode)
    }

    override fun clearSourceIndexes(
        cacheRoot: String,
        sourceKey: String,
    ): NativeStorageResponse = invokeNative {
        nativeClearSourceIndexes(cacheRoot, sourceKey)
    }

    private fun invokeNative(call: () -> String?): NativeStorageResponse {
        loadFailure?.let { failure ->
            return NativeStorageResponse.Failure(
                code = "native_load_failed",
                message = failure.message ?: "FrameScope native library could not be loaded.",
                engine = null,
            )
        }
        return runCatching {
            val payload = call()
                ?: return NativeStorageResponse.Failure(
                    code = "bridge_error",
                    message = "Native storage administration returned no response.",
                    engine = null,
                )
            parseStorageResponse(payload)
        }.getOrElse { error ->
            NativeStorageResponse.Failure(
                code = "bridge_error",
                message = error.message ?: "Native storage response could not be decoded.",
                engine = null,
            )
        }
    }
}

internal fun parseStorageResponse(payload: String): NativeStorageResponse {
    val root = JSONObject(payload)
    val engine = root.optString("engine").takeIf { it.isNotBlank() }
    return when (root.getString("status")) {
        "ok" -> NativeStorageResponse.Success(
            storage = root.getJSONObject("storage").toStorageStats(),
            cleared = root.optJSONObject("cleared")?.toClearReceipt(),
        )
        "error" -> NativeStorageResponse.Failure(
            code = root.optString("code", "storage_error"),
            message = root.optString("message", "FrameScope storage operation failed."),
            engine = engine,
        )
        else -> NativeStorageResponse.Failure(
            code = "bridge_error",
            message = "Native storage response had an unknown status.",
            engine = engine,
        )
    }
}

private fun JSONObject.toStorageStats(): FrameScopeStorageStats = FrameScopeStorageStats(
    totalBytes = nonNegativeLong("total_bytes"),
    persistentIndexes = getJSONObject("persistent_indexes").toCategoryStats(),
    previewProxy = getJSONObject("preview_proxy").toCategoryStats(),
    disposable = getJSONObject("disposable").toCategoryStats(),
    indexedSources = nonNegativeLong("indexed_sources"),
    previewProxyEnabled = getBoolean("preview_proxy_enabled"),
)

private fun JSONObject.toCategoryStats(): StorageCategoryStats = StorageCategoryStats(
    bytes = nonNegativeLong("bytes"),
    files = nonNegativeLong("files"),
    items = nonNegativeLong("items"),
)

private fun JSONObject.toClearReceipt(): StorageClearReceipt {
    val scope = when (getString("scope")) {
        "preview_proxy" -> StorageClearScope.PreviewProxy
        "persistent_indexes" -> StorageClearScope.PersistentIndexes
        "disposable" -> StorageClearScope.Disposable
        "all" -> StorageClearScope.All
        else -> error("Native storage response contained an unknown clear scope.")
    }
    return StorageClearReceipt(
        scope = scope,
        clearedBytes = nonNegativeLong("cleared_bytes"),
        clearedFiles = nonNegativeLong("cleared_files"),
        clearedItems = nonNegativeLong("cleared_items"),
    )
}

private fun JSONObject.nonNegativeLong(name: String): Long = getLong(name).also { value ->
    require(value >= 0L) { "Native storage field $name must not be negative." }
}
