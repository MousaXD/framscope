package com.framescope.app.data

import org.json.JSONObject

enum class MicroscopeIndexingStage(
    val wireName: String,
) {
    ProbingMedia("probing_media"),
    CheckingExistingIndex("checking_existing_index"),
    ReusingExistingIndex("reusing_existing_index"),
    ValidatingExistingIndex("validating_existing_index"),
    RebuildingIndex("rebuilding_index"),
    Indexing("indexing"),
    Finalizing("finalizing"),
    ;

    companion object {
        fun fromWireName(value: String): MicroscopeIndexingStage? =
            values().firstOrNull { stage -> stage.wireName == value }
    }
}

data class MicroscopeIndexingProgress(
    val operationId: Long,
    val stage: MicroscopeIndexingStage,
    val indexedFrames: Long,
    val reusedFrames: Long,
    val expectedReuseFrames: Long,
    val firstTimestampUs: Long?,
    val currentTimestampUs: Long?,
    /** Compatibility alias for operation age. Throughput estimation must not use this field. */
    val elapsedMs: Long,
    /** Monotonic identity of the native progress event. Re-reading one event preserves this value. */
    val sequence: Long = elapsedMs,
    /** Native operation age captured at the instant this progress event was emitted. */
    val sampleElapsedMs: Long = elapsedMs,
    /** Current operation age at the time Android read this snapshot. */
    val operationElapsedMs: Long = elapsedMs,
    /** Native operation age of the most recent genuine work advancement. */
    val lastWorkAdvanceElapsedMs: Long = sampleElapsedMs,
    /** Monotonic presentation coverage, distinct from the exact latest frame timestamp. */
    val maxPresentationTimestampUs: Long? = currentTimestampUs,
    /** False means the values are the last known sample after progress telemetry became unavailable. */
    val telemetryAvailable: Boolean = true,
) {
    fun isSane(): Boolean =
        operationId > 0L &&
            sequence >= 0L &&
            indexedFrames >= 0L &&
            reusedFrames >= 0L &&
            expectedReuseFrames >= 0L &&
            reusedFrames <= maxOf(indexedFrames, expectedReuseFrames) &&
            elapsedMs >= 0L &&
            sampleElapsedMs >= 0L &&
            operationElapsedMs >= sampleElapsedMs &&
            lastWorkAdvanceElapsedMs in 0L..operationElapsedMs &&
            (firstTimestampUs == null || maxPresentationTimestampUs == null ||
                maxPresentationTimestampUs >= firstTimestampUs) &&
            (currentTimestampUs == null || maxPresentationTimestampUs == null ||
                maxPresentationTimestampUs >= currentTimestampUs)
}

interface MicroscopeIndexingProgressSource {
    fun read(operationId: Long): Result<MicroscopeIndexingProgress?>
}

/**
 * Keeps at most one operation's last native sample so telemetry loss can be represented explicitly
 * without letting a prior source leak into a replacement operation.
 */
internal class MicroscopeIndexingProgressFreshness {
    private var latest: MicroscopeIndexingProgress? = null

    fun beginOperation(operationId: Long) {
        if (latest?.operationId != operationId) {
            latest = null
        }
    }

    fun onSuccess(
        operationId: Long,
        progress: MicroscopeIndexingProgress?,
    ): MicroscopeIndexingProgress? {
        require(progress == null || progress.operationId == operationId) {
            "Indexing progress operation identity changed."
        }
        latest = progress?.copy(telemetryAvailable = true)
        return latest
    }

    fun onFailure(operationId: Long): MicroscopeIndexingProgress? {
        val current = latest?.takeIf { it.operationId == operationId } ?: return null
        return current.copy(telemetryAvailable = false).also { latest = it }
    }
}

object RustMicroscopeIndexingProgressSource : MicroscopeIndexingProgressSource {
    private val loadFailure: Throwable? = runCatching {
        System.loadLibrary("framescope_ffi")
    }.exceptionOrNull()
    private val freshness = MicroscopeIndexingProgressFreshness()

    @JvmStatic
    private external fun nativeMicroscopeIndexingProgress(operationId: Long): String?

    @Synchronized
    override fun read(operationId: Long): Result<MicroscopeIndexingProgress?> {
        if (operationId <= 0L) {
            return Result.failure(IllegalArgumentException("Indexing progress operation id must be positive."))
        }
        freshness.beginOperation(operationId)
        loadFailure?.let { error ->
            val stale = freshness.onFailure(operationId)
            return if (stale != null) Result.success(stale) else Result.failure(error)
        }

        return try {
            val raw = requireNotNull(nativeMicroscopeIndexingProgress(operationId)) {
                "Rust engine returned a null indexing progress response."
            }
            val parsed = parseResponse(raw, operationId)
            Result.success(freshness.onSuccess(operationId, parsed))
        } catch (error: Throwable) {
            val stale = freshness.onFailure(operationId)
            if (stale != null) Result.success(stale) else Result.failure(error)
        }
    }

    internal fun parseResponse(
        raw: String,
        expectedOperationId: Long,
    ): MicroscopeIndexingProgress? {
        val root = JSONObject(raw)
        return when (root.optString("status")) {
            "idle" -> null
            "ok" -> {
                val progress = root.getJSONObject("progress")
                val operationId = progress.getLong("operation_id")
                require(operationId == expectedOperationId) {
                    "Rust indexing progress operation identity changed."
                }
                val operationElapsedMs = progress.getLong("operation_elapsed_ms")
                val parsed = MicroscopeIndexingProgress(
                    operationId = operationId,
                    stage = requireNotNull(
                        MicroscopeIndexingStage.fromWireName(progress.getString("stage")),
                    ) { "Rust indexing progress returned an unknown stage." },
                    indexedFrames = progress.getLong("indexed_frames"),
                    reusedFrames = progress.getLong("reused_frames"),
                    expectedReuseFrames = progress.getLong("expected_reuse_frames"),
                    firstTimestampUs = progress.optionalLong("first_timestamp_us"),
                    currentTimestampUs = progress.optionalLong("current_timestamp_us"),
                    elapsedMs = operationElapsedMs,
                    sequence = progress.getLong("sequence"),
                    sampleElapsedMs = progress.getLong("sample_elapsed_ms"),
                    operationElapsedMs = operationElapsedMs,
                    lastWorkAdvanceElapsedMs = progress.getLong("last_work_advance_elapsed_ms"),
                    maxPresentationTimestampUs = progress.optionalLong("max_presentation_timestamp_us"),
                    telemetryAvailable = true,
                )
                require(parsed.isSane()) { "Rust indexing progress values are invalid." }
                parsed
            }
            else -> error("Rust indexing progress returned an unrecognized response.")
        }
    }
}

private fun JSONObject.optionalLong(name: String): Long? =
    if (!has(name) || isNull(name)) null else getLong(name)
