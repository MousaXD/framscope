package com.framescope.app.data

import org.json.JSONObject

private const val SIMILARITY_SCALE = 10_000

data class MicroscopeSimilarityMatch(
    val frameId: Long,
    val similarity: Int,
)

data class MicroscopeSimilarityResult(
    val sessionId: Long,
    val targetFrameId: Long,
    val descriptorCount: Long,
    val candidateCount: Long,
    val matchedCount: Long,
    val truncated: Boolean,
    val minimumSimilarity: Int,
    val disposition: SimilarityStoreDisposition,
    val matches: List<MicroscopeSimilarityMatch>,
) {
    fun isSane(): Boolean =
        sessionId > 0L &&
            targetFrameId >= 0L &&
            descriptorCount > 0L &&
            targetFrameId < descriptorCount &&
            candidateCount in 0L until descriptorCount &&
            matchedCount in 0L..candidateCount &&
            minimumSimilarity in 0..SIMILARITY_SCALE &&
            matches.size.toLong() <= matchedCount &&
            matches.all { match ->
                match.frameId in 0L until descriptorCount &&
                    match.frameId != targetFrameId &&
                    match.similarity in minimumSimilarity..SIMILARITY_SCALE
            } &&
            matches.zipWithNext().all { (left, right) ->
                left.similarity > right.similarity ||
                    (left.similarity == right.similarity && left.frameId < right.frameId)
            } &&
            (!truncated || matchedCount > matches.size.toLong())
}

enum class SimilarityStoreDisposition {
    Reused,
    Built,
}

sealed interface NativeMicroscopeSimilarity {
    data class Success(
        val result: MicroscopeSimilarityResult,
        val engine: String,
    ) : NativeMicroscopeSimilarity

    data class Failure(
        val code: String,
        val message: String,
        val engine: String?,
    ) : NativeMicroscopeSimilarity
}

internal interface NativeMicroscopeSimilarityBridge {
    fun findSimilarFrames(
        sessionId: Long,
        targetFrameId: Long,
        operationId: Long,
        cacheRoot: String,
    ): NativeMicroscopeSimilarity

    fun cancel(operationId: Long): Boolean
}

internal object MicroscopeSimilarityBridge : NativeMicroscopeSimilarityBridge {
    private val loadFailure: Throwable? = runCatching {
        System.loadLibrary("framescope_ffi")
    }.exceptionOrNull()

    @JvmStatic
    private external fun nativeFindSimilarFrames(
        sessionId: Long,
        targetFrameId: Long,
        operationId: Long,
        cacheRoot: String,
    ): String?

    @JvmStatic
    private external fun nativeCancelSimilarity(operationId: Long): Boolean

    override fun findSimilarFrames(
        sessionId: Long,
        targetFrameId: Long,
        operationId: Long,
        cacheRoot: String,
    ): NativeMicroscopeSimilarity {
        if (sessionId <= 0L || targetFrameId < 0L || operationId <= 0L || cacheRoot.isBlank()) {
            return NativeMicroscopeSimilarity.Failure(
                code = "invalid_request",
                message = "Similarity requires a live session, target frame, operation id, and cache root.",
                engine = null,
            )
        }
        loadFailure?.let { error ->
            return NativeMicroscopeSimilarity.Failure(
                code = "native_library_unavailable",
                message = "Rust engine could not be loaded: ${error.message ?: error::class.java.simpleName}",
                engine = null,
            )
        }

        val raw = runCatching {
            nativeFindSimilarFrames(sessionId, targetFrameId, operationId, cacheRoot)
        }.getOrElse { error ->
            return NativeMicroscopeSimilarity.Failure(
                code = "jni_error",
                message = "Rust similarity call failed: ${error.message ?: error::class.java.simpleName}",
                engine = null,
            )
        } ?: return NativeMicroscopeSimilarity.Failure(
            code = "jni_error",
            message = "Rust engine returned a null similarity response.",
            engine = null,
        )
        return parseSimilarityResponse(raw)
    }

    override fun cancel(operationId: Long): Boolean {
        if (operationId <= 0L || loadFailure != null) return false
        return runCatching { nativeCancelSimilarity(operationId) }.getOrDefault(false)
    }
}

internal fun parseSimilarityResponse(raw: String): NativeMicroscopeSimilarity = try {
    val root = JSONObject(raw)
    val engine = root.optString("engine").trim().takeIf(String::isNotEmpty)
    when (root.optString("status")) {
        "ok" -> {
            if (engine == null) {
                NativeMicroscopeSimilarity.Failure(
                    code = "malformed_response",
                    message = "Rust similarity success did not identify the engine.",
                    engine = null,
                )
            } else {
                parseSimilaritySuccess(root.getJSONObject("result"), engine)
            }
        }
        "error" -> NativeMicroscopeSimilarity.Failure(
            code = root.optString("code", "rust_error"),
            message = root.optString("message", "Rust similarity query failed."),
            engine = engine,
        )
        else -> NativeMicroscopeSimilarity.Failure(
            code = "malformed_response",
            message = "Rust returned an unrecognized similarity response.",
            engine = engine,
        )
    }
} catch (error: Exception) {
    NativeMicroscopeSimilarity.Failure(
        code = "malformed_response",
        message = "Could not decode Rust similarity response: ${error.message ?: error::class.java.simpleName}",
        engine = null,
    )
}

private fun parseSimilaritySuccess(
    json: JSONObject,
    engine: String,
): NativeMicroscopeSimilarity {
    val disposition = when (json.getString("disposition")) {
        "reused" -> SimilarityStoreDisposition.Reused
        "built" -> SimilarityStoreDisposition.Built
        else -> return NativeMicroscopeSimilarity.Failure(
            code = "malformed_similarity_state",
            message = "Rust returned an unknown similarity-store disposition.",
            engine = engine,
        )
    }
    val matchesJson = json.getJSONArray("matches")
    val matches = buildList(matchesJson.length()) {
        for (index in 0 until matchesJson.length()) {
            val match = matchesJson.getJSONObject(index)
            add(
                MicroscopeSimilarityMatch(
                    frameId = match.getLong("frame_id"),
                    similarity = match.getInt("similarity"),
                ),
            )
        }
    }
    val result = MicroscopeSimilarityResult(
        sessionId = json.getLong("session_id"),
        targetFrameId = json.getLong("target_frame_id"),
        descriptorCount = json.getLong("descriptor_count"),
        candidateCount = json.getLong("candidate_count"),
        matchedCount = json.getLong("matched_count"),
        truncated = json.getBoolean("truncated"),
        minimumSimilarity = json.getInt("minimum_similarity"),
        disposition = disposition,
        matches = matches,
    )
    return if (result.isSane()) {
        NativeMicroscopeSimilarity.Success(result = result, engine = engine)
    } else {
        NativeMicroscopeSimilarity.Failure(
            code = "malformed_similarity_state",
            message = "Rust returned similarity data outside expected identity or score bounds.",
            engine = engine,
        )
    }
}

class MicroscopeSimilarityException(
    val code: String,
    message: String,
) : IllegalStateException(message)
