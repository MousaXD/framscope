package com.framescope.app.data

import org.json.JSONObject

data class RamCacheTierMetrics(
    val budgetBytes: Long,
    val residentBytes: Long,
    val residentFrames: Long,
    val hits: Long,
    val misses: Long,
    val insertions: Long,
    val evictions: Long,
) {
    val requests: Long
        get() = hits + misses
}

data class RamAccelerationMetrics(
    val source: RamCacheTierMetrics,
    val preview: RamCacheTierMetrics,
    val retainedPreviewSessions: Int,
) {
    val residentBytes: Long
        get() = source.residentBytes + preview.residentBytes

    val hits: Long
        get() = source.hits + preview.hits

    val misses: Long
        get() = source.misses + preview.misses
}

internal interface NativeRamAccelerationBridge {
    fun configure(
        cacheRoot: String,
        sourceCacheBytes: Long,
        previewCacheBytes: Long,
    ): Boolean

    fun stats(cacheRoot: String): RamAccelerationMetrics?
}

internal object RamAccelerationBridge : NativeRamAccelerationBridge {
    private val loadFailure: Throwable? = runCatching {
        System.loadLibrary("framescope_ffi")
    }.exceptionOrNull()

    @JvmStatic
    private external fun nativeConfigureRamAcceleration(
        cacheRoot: String,
        sourceCacheBytes: Long,
        previewCacheBytes: Long,
    ): Boolean

    @JvmStatic
    private external fun nativeRamAccelerationStats(cacheRoot: String): String?

    override fun configure(
        cacheRoot: String,
        sourceCacheBytes: Long,
        previewCacheBytes: Long,
    ): Boolean {
        if (loadFailure != null) return false
        return runCatching {
            nativeConfigureRamAcceleration(
                cacheRoot,
                sourceCacheBytes,
                previewCacheBytes,
            )
        }.getOrDefault(false)
    }

    override fun stats(cacheRoot: String): RamAccelerationMetrics? {
        if (cacheRoot.isBlank() || loadFailure != null) return null
        val raw = runCatching { nativeRamAccelerationStats(cacheRoot) }.getOrNull() ?: return null
        return parseStats(raw)
    }

    internal fun parseStats(raw: String): RamAccelerationMetrics? = runCatching {
        val response = JSONObject(raw)
        if (response.optString("status") != "ok") return@runCatching null
        val ram = response.getJSONObject("ram")
        RamAccelerationMetrics(
            source = parseTier(ram.getJSONObject("source")),
            preview = parseTier(ram.getJSONObject("preview")),
            retainedPreviewSessions = ram.getInt("retained_preview_sessions")
                .takeIf { it >= 0 } ?: return@runCatching null,
        )
    }.getOrNull()

    private fun parseTier(value: JSONObject): RamCacheTierMetrics {
        fun nonNegative(name: String): Long = value.getLong(name).also { parsed ->
            require(parsed >= 0L) { "$name must be non-negative" }
        }
        return RamCacheTierMetrics(
            budgetBytes = nonNegative("budget_bytes"),
            residentBytes = nonNegative("resident_bytes"),
            residentFrames = nonNegative("resident_frames"),
            hits = nonNegative("hits"),
            misses = nonNegative("misses"),
            insertions = nonNegative("insertions"),
            evictions = nonNegative("evictions"),
        )
    }
}
