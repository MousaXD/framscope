package com.framescope.app.data

import java.nio.ByteBuffer
import org.json.JSONObject

private const val MAX_PRESENTATION_RGBA_BYTES = 256L * 1024L * 1024L

/** Operation counters emitted by the authoritative exact-navigation engine. */
data class ExactNavigationDiagnostics(
    val requestedExactFrames: Long,
    val randomSeeks: Long,
    val forwardDecodes: Long,
    val decoderReopenCount: Long,
    val decodedFrames: Long,
    val warmNavigationHits: Long,
    val ramNavigationHits: Long,
    val lastSeekDistanceFrames: Long,
    val totalSeekDistanceFrames: Long,
    val lastFramesDecoded: Long,
    val lastDecoderReopens: Long,
    val lastRandomSeek: Boolean,
    val lastWarmNavigationHit: Boolean,
) {
    fun isSane(): Boolean =
        requestedExactFrames >= 0L &&
            randomSeeks >= 0L &&
            forwardDecodes >= 0L &&
            decoderReopenCount >= 0L &&
            decodedFrames >= 0L &&
            warmNavigationHits >= 0L &&
            ramNavigationHits >= 0L &&
            lastSeekDistanceFrames >= 0L &&
            totalSeekDistanceFrames >= 0L &&
            lastFramesDecoded >= 0L &&
            lastDecoderReopens >= 0L &&
            randomSeeks <= requestedExactFrames &&
            warmNavigationHits <= requestedExactFrames &&
            ramNavigationHits <= requestedExactFrames

    /** Frames decoded per successful authoritative exact-frame request. */
    fun framesDecodedPerExactFrame(): Double? =
        if (requestedExactFrames == 0L) null else decodedFrames.toDouble() / requestedExactFrames
}

/** Metadata-only description of one prepared source-quality RGBA frame. */
data class PreparedMicroscopeFrame(
    val sessionId: Long,
    val frameId: Long,
    val generation: Long,
    val width: Int,
    val height: Int,
    val strideBytes: Long,
    val byteLen: Int,
    val navigation: ExactNavigationDiagnostics? = null,
) {
    fun isSane(): Boolean {
        if (sessionId <= 0L || frameId < 0L || generation <= 0L) return false
        if (width !in 1..65_535 || height !in 1..65_535) return false
        if (navigation?.isSane() == false) return false
        val minimumStride = width.toLong() * RGBA_BYTES_PER_PIXEL
        if (strideBytes < minimumStride) return false
        val expectedBytes = runCatching { Math.multiplyExact(strideBytes, height.toLong()) }
            .getOrNull() ?: return false
        return expectedBytes == byteLen.toLong() &&
            expectedBytes in 1..MAX_PRESENTATION_RGBA_BYTES &&
            expectedBytes <= Int.MAX_VALUE.toLong()
    }

    private companion object {
        const val RGBA_BYTES_PER_PIXEL = 4L
    }
}

sealed interface NativeFramePreparation {
    data class Success(
        val frame: PreparedMicroscopeFrame,
        val engine: String,
    ) : NativeFramePreparation

    data class Failure(
        val code: String,
        val message: String,
        val engine: String?,
    ) : NativeFramePreparation
}

sealed interface NativeFrameCopy {
    data class Success(
        val frame: PreparedMicroscopeFrame,
        val rgba: ByteBuffer,
    ) : NativeFrameCopy

    data class Failure(
        val code: String,
        val message: String,
    ) : NativeFrameCopy
}

interface NativeMicroscopeFrameBridge {
    fun prepare(sessionId: Long): NativeFramePreparation
    fun copy(frame: PreparedMicroscopeFrame): NativeFrameCopy
}

/**
 * Narrow Android/JNI handoff for a single microscope frame.
 *
 * JSON carries metadata only. Pixel bytes are copied once into a caller-owned direct ByteBuffer;
 * no Rust pointer is retained by Java/Kotlin and no base64 representation is created.
 */
object MicroscopeFrameBridge : NativeMicroscopeFrameBridge {
    private val loadFailure: Throwable? = runCatching {
        System.loadLibrary("framescope_ffi")
    }.exceptionOrNull()

    @JvmStatic
    private external fun nativePrepareMicroscopeFrame(sessionId: Long): String?

    @JvmStatic
    private external fun nativeCopyMicroscopeFrameRgba(
        sessionId: Long,
        generation: Long,
        destination: ByteBuffer,
    ): Long

    override fun prepare(sessionId: Long): NativeFramePreparation {
        if (sessionId <= 0L) {
            return NativeFramePreparation.Failure(
                code = "invalid_request",
                message = "Microscope frame preparation requires a positive session id.",
                engine = null,
            )
        }
        loadFailure?.let {
            return NativeFramePreparation.Failure(
                code = "native_library_unavailable",
                message = "Rust engine could not be loaded: ${it.message ?: it::class.java.simpleName}",
                engine = null,
            )
        }
        val raw = runCatching { nativePrepareMicroscopeFrame(sessionId) }.getOrElse {
            return NativeFramePreparation.Failure(
                code = "jni_error",
                message = "Rust frame preparation failed: ${it.message ?: it::class.java.simpleName}",
                engine = null,
            )
        } ?: return NativeFramePreparation.Failure(
            code = "jni_error",
            message = "Rust engine returned a null frame preparation response.",
            engine = null,
        )
        return parsePreparationResponse(raw)
    }

    override fun copy(frame: PreparedMicroscopeFrame): NativeFrameCopy {
        if (!frame.isSane()) {
            return NativeFrameCopy.Failure(
                code = "invalid_request",
                message = "Prepared microscope frame metadata is outside safety bounds.",
            )
        }
        loadFailure?.let {
            return NativeFrameCopy.Failure(
                code = "native_library_unavailable",
                message = "Rust engine could not be loaded: ${it.message ?: it::class.java.simpleName}",
            )
        }
        val destination = try {
            ByteBuffer.allocateDirect(frame.byteLen)
        } catch (error: OutOfMemoryError) {
            return NativeFrameCopy.Failure(
                code = "buffer_allocation_failed",
                message = "Android could not allocate memory for this frame.",
            )
        }
        val copied = runCatching {
            nativeCopyMicroscopeFrameRgba(frame.sessionId, frame.generation, destination)
        }.getOrElse {
            return NativeFrameCopy.Failure(
                code = "jni_error",
                message = "Rust frame copy failed: ${it.message ?: it::class.java.simpleName}",
            )
        }
        return interpretCopyResult(frame, destination, copied)
    }

    internal fun parsePreparationResponse(raw: String): NativeFramePreparation = try {
        val json = JSONObject(raw)
        val engine = json.optionalString("engine")
        when (json.optString("status")) {
            "ok" -> {
                if (engine == null) {
                    NativeFramePreparation.Failure(
                        code = "malformed_response",
                        message = "Rust frame preparation did not identify the engine.",
                        engine = null,
                    )
                } else {
                    val value = json.getJSONObject("frame")
                    val byteLenLong = value.getLong("byte_len")
                    val frame = if (byteLenLong in 1..Int.MAX_VALUE.toLong()) {
                        PreparedMicroscopeFrame(
                            sessionId = value.getLong("session_id"),
                            frameId = value.getLong("frame_id"),
                            generation = value.getLong("generation"),
                            width = value.getInt("width"),
                            height = value.getInt("height"),
                            strideBytes = value.getLong("stride_bytes"),
                            byteLen = byteLenLong.toInt(),
                            navigation = value.optJSONObject("navigation")
                                ?.let(::parseNavigationDiagnostics),
                        )
                    } else {
                        null
                    }
                    if (frame != null && frame.isSane()) {
                        NativeFramePreparation.Success(frame = frame, engine = engine)
                    } else {
                        NativeFramePreparation.Failure(
                            code = "malformed_frame_descriptor",
                            message = "Rust returned frame buffer metadata outside safety bounds.",
                            engine = engine,
                        )
                    }
                }
            }

            "error" -> NativeFramePreparation.Failure(
                code = json.optString("code", "rust_error"),
                message = json.optString("message", "Rust frame preparation failed."),
                engine = engine,
            )

            else -> NativeFramePreparation.Failure(
                code = "malformed_response",
                message = "Rust returned an unrecognized frame preparation response.",
                engine = engine,
            )
        }
    } catch (error: Exception) {
        NativeFramePreparation.Failure(
            code = "malformed_response",
            message = "Could not decode Rust frame preparation: ${error.message ?: error::class.java.simpleName}",
            engine = null,
        )
    }

    internal fun interpretCopyResult(
        frame: PreparedMicroscopeFrame,
        destination: ByteBuffer,
        copied: Long,
    ): NativeFrameCopy {
        if (copied < 0L) return copyFailure(copied)
        if (copied != frame.byteLen.toLong()) {
            return NativeFrameCopy.Failure(
                code = "copy_length_mismatch",
                message = "Rust copied $copied bytes but ${frame.byteLen} were required.",
            )
        }
        destination.position(0)
        destination.limit(frame.byteLen)
        return NativeFrameCopy.Success(
            frame = frame,
            rgba = destination.asReadOnlyBuffer(),
        )
    }

    private fun parseNavigationDiagnostics(value: JSONObject): ExactNavigationDiagnostics? =
        runCatching {
            ExactNavigationDiagnostics(
                requestedExactFrames = value.getLong("requested_exact_frames"),
                randomSeeks = value.getLong("random_seeks"),
                forwardDecodes = value.getLong("forward_decodes"),
                decoderReopenCount = value.getLong("decoder_reopen_count"),
                decodedFrames = value.getLong("decoded_frames"),
                warmNavigationHits = value.getLong("warm_navigation_hits"),
                ramNavigationHits = value.getLong("ram_navigation_hits"),
                lastSeekDistanceFrames = value.getLong("last_seek_distance_frames"),
                totalSeekDistanceFrames = value.getLong("total_seek_distance_frames"),
                lastFramesDecoded = value.getLong("last_frames_decoded"),
                lastDecoderReopens = value.getLong("last_decoder_reopens"),
                lastRandomSeek = value.getBoolean("last_random_seek"),
                lastWarmNavigationHit = value.getBoolean("last_warm_navigation_hit"),
            )
        }.getOrNull()?.takeIf(ExactNavigationDiagnostics::isSane)

    private fun copyFailure(code: Long): NativeFrameCopy.Failure = when (code) {
        -1L -> NativeFrameCopy.Failure("invalid_request", "Rust rejected the frame copy request.")
        -2L -> NativeFrameCopy.Failure("session_not_found", "Microscope session is no longer open.")
        -3L -> NativeFrameCopy.Failure(
            "stale_generation",
            "The prepared frame became stale after microscope navigation.",
        )
        -4L -> NativeFrameCopy.Failure(
            "no_prepared_frame",
            "No source-quality frame is prepared for the current microscope position.",
        )
        -5L -> NativeFrameCopy.Failure(
            "buffer_too_small",
            "Android's direct frame buffer is smaller than the prepared RGBA payload.",
        )
        -7L -> NativeFrameCopy.Failure(
            "invalid_buffer",
            "Android did not provide a valid direct frame buffer.",
        )
        else -> NativeFrameCopy.Failure("bridge_error", "Native frame copy failed safely.")
    }

    private fun JSONObject.optionalString(key: String): String? =
        if (!has(key) || isNull(key)) null else getString(key).trim().takeIf(String::isNotEmpty)
}
