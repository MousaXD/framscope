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
    val elapsedMs: Long,
) {
    fun isSane(): Boolean =
        operationId > 0L &&
            indexedFrames >= 0L &&
            reusedFrames >= 0L &&
            expectedReuseFrames >= 0L &&
            reusedFrames <= maxOf(indexedFrames, expectedReuseFrames) &&
            elapsedMs >= 0L &&
            (firstTimestampUs == null || currentTimestampUs == null || currentTimestampUs >= firstTimestampUs)
}

interface MicroscopeIndexingProgressSource {
    fun read(operationId: Long): Result<MicroscopeIndexingProgress?>
}

object RustMicroscopeIndexingProgressSource : MicroscopeIndexingProgressSource {
    private val loadFailure: Throwable? = runCatching {
        System.loadLibrary("framescope_ffi")
    }.exceptionOrNull()

    @JvmStatic
    private external fun nativeMicroscopeIndexingProgress(operationId: Long): String?

    override fun read(operationId: Long): Result<MicroscopeIndexingProgress?> {
        if (operationId <= 0L) {
            return Result.failure(IllegalArgumentException("Indexing progress operation id must be positive."))
        }
        loadFailure?.let { return Result.failure(it) }
        return runCatching {
            val raw = requireNotNull(nativeMicroscopeIndexingProgress(operationId)) {
                "Rust engine returned a null indexing progress response."
            }
            parseResponse(raw, operationId)
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
                    elapsedMs = progress.getLong("elapsed_ms"),
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
