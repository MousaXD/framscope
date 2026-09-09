package com.framescope.app.data

import android.graphics.Bitmap
import android.os.Trace
import java.nio.ByteBuffer
import org.json.JSONObject

const val DEFAULT_SCRUB_PREVIEW_MAX_EDGE = 640
private const val MIN_SCRUB_PREVIEW_MAX_EDGE = 64
private const val MAX_SCRUB_PREVIEW_MAX_EDGE = 1_024
private const val RGBA_BYTES_PER_PIXEL = 4L
private const val TRACE_PREVIEW_JNI = "FrameScope.pixels.preview_jni"
private const val TRACE_PREVIEW_BITMAP = "FrameScope.pixels.preview_bitmap"

data class MicroscopePreviewDescriptor(
    val sessionId: Long,
    val frameId: Long,
    val timestampUs: Long?,
    val width: Int,
    val height: Int,
    val strideBytes: Long,
    val byteLen: Int,
    val source: String,
    val decodedFrames: Long,
) {
    fun isSane(maxEdge: Int): Boolean {
        if (sessionId <= 0L || frameId < 0L || decodedFrames < 0L) return false
        if (maxEdge !in MIN_SCRUB_PREVIEW_MAX_EDGE..MAX_SCRUB_PREVIEW_MAX_EDGE) return false
        if (width !in 1..maxEdge || height !in 1..maxEdge) return false
        if (source !in VALID_SOURCES) return false
        val minimumStride = width.toLong() * RGBA_BYTES_PER_PIXEL
        if (strideBytes != minimumStride) return false
        val expectedBytes = runCatching { Math.multiplyExact(strideBytes, height.toLong()) }
            .getOrNull() ?: return false
        val maxBytes = maxEdge.toLong() * maxEdge.toLong() * RGBA_BYTES_PER_PIXEL
        return expectedBytes == byteLen.toLong() && expectedBytes in 1..maxBytes
    }

    private companion object {
        val VALID_SOURCES = setOf("preview_ram", "source_ram", "decoded")
    }
}

/**
 * One non-authoritative, bounded image used only while the timeline is being scrubbed.
 *
 * Production previews are materialized into a Bitmap before the JNI scratch-buffer lease is
 * released. The RGBA constructor is retained only for deterministic JVM fixtures and parser tests.
 */
class MicroscopeScrubPreview private constructor(
    val descriptor: MicroscopePreviewDescriptor,
    internal val rgba: ByteBuffer?,
    internal val bitmap: Bitmap?,
) {
    constructor(
        descriptor: MicroscopePreviewDescriptor,
        rgba: ByteBuffer,
    ) : this(descriptor = descriptor, rgba = rgba, bitmap = null)

    companion object {
        internal fun fromBitmap(
            descriptor: MicroscopePreviewDescriptor,
            bitmap: Bitmap,
        ): MicroscopeScrubPreview = MicroscopeScrubPreview(
            descriptor = descriptor,
            rgba = null,
            bitmap = bitmap,
        )
    }
}

sealed interface NativeMicroscopePreview {
    data class Success(
        val preview: MicroscopeScrubPreview,
        val engine: String,
    ) : NativeMicroscopePreview

    data class Failure(
        val code: String,
        val message: String,
        val engine: String?,
    ) : NativeMicroscopePreview
}

interface NativeMicroscopePreviewBridge {
    fun renderTimestamp(
        sessionId: Long,
        timestampUs: Long,
        selection: TimestampSelectionPolicy,
        cacheRoot: String,
        maxEdge: Int = DEFAULT_SCRUB_PREVIEW_MAX_EDGE,
    ): NativeMicroscopePreview

    fun renderFrame(
        sessionId: Long,
        frameId: Long,
        cacheRoot: String,
        maxEdge: Int = DEFAULT_SCRUB_PREVIEW_MAX_EDGE,
    ): NativeMicroscopePreview

    fun prefetchFrame(
        sessionId: Long,
        frameId: Long,
        cacheRoot: String,
    ): Boolean

    fun cancelSession(sessionId: Long): Boolean

    fun forgetSession(sessionId: Long): Boolean
}

/** JNI handoff for bounded live-scrub previews. Authoritative frame state is never mutated here. */
object MicroscopePreviewBridge : NativeMicroscopePreviewBridge {
    private val loadFailure: Throwable? = runCatching {
        System.loadLibrary("framescope_ffi")
    }.exceptionOrNull()
    private val directBufferPool = PreviewDirectBufferPool(maxRetainedBuffers = 2)

    @JvmStatic
    private external fun nativeRenderMicroscopePreviewTimestampUs(
        sessionId: Long,
        timestampUs: Long,
        selection: Int,
        maxEdge: Int,
        cacheRoot: String,
        destination: ByteBuffer,
    ): String?

    @JvmStatic
    private external fun nativeRenderMicroscopePreviewFrame(
        sessionId: Long,
        frameId: Long,
        maxEdge: Int,
        cacheRoot: String,
        destination: ByteBuffer,
    ): String?

    @JvmStatic
    private external fun nativePrefetchMicroscopePreviewFrame(
        sessionId: Long,
        frameId: Long,
        cacheRoot: String,
    ): Boolean

    @JvmStatic
    private external fun nativeCancelMicroscopePreviewSession(sessionId: Long): Boolean

    @JvmStatic
    private external fun nativeForgetMicroscopePreviewSession(sessionId: Long): Boolean

    override fun renderTimestamp(
        sessionId: Long,
        timestampUs: Long,
        selection: TimestampSelectionPolicy,
        cacheRoot: String,
        maxEdge: Int,
    ): NativeMicroscopePreview = render(sessionId, maxEdge) { destination ->
        nativeRenderMicroscopePreviewTimestampUs(
            sessionId,
            timestampUs,
            selection.nativeValue,
            maxEdge,
            cacheRoot,
            destination,
        )
    }

    override fun renderFrame(
        sessionId: Long,
        frameId: Long,
        cacheRoot: String,
        maxEdge: Int,
    ): NativeMicroscopePreview {
        if (frameId < 0L) {
            return NativeMicroscopePreview.Failure(
                code = "invalid_request",
                message = "Live preview frame id must be non-negative.",
                engine = null,
            )
        }
        return render(sessionId, maxEdge) { destination ->
            nativeRenderMicroscopePreviewFrame(
                sessionId,
                frameId,
                maxEdge,
                cacheRoot,
                destination,
            )
        }
    }

    override fun prefetchFrame(
        sessionId: Long,
        frameId: Long,
        cacheRoot: String,
    ): Boolean {
        if (sessionId <= 0L || frameId < 0L || cacheRoot.isBlank() || loadFailure != null) return false
        return runCatching {
            nativePrefetchMicroscopePreviewFrame(sessionId, frameId, cacheRoot)
        }.getOrDefault(false)
    }

    override fun cancelSession(sessionId: Long): Boolean {
        if (sessionId <= 0L || loadFailure != null) return false
        return runCatching { nativeCancelMicroscopePreviewSession(sessionId) }.getOrDefault(false)
    }

    override fun forgetSession(sessionId: Long): Boolean {
        if (sessionId <= 0L || loadFailure != null) return false
        return runCatching { nativeForgetMicroscopePreviewSession(sessionId) }.getOrDefault(false)
    }

    private fun render(
        sessionId: Long,
        maxEdge: Int,
        nativeCall: (ByteBuffer) -> String?,
    ): NativeMicroscopePreview {
        if (sessionId <= 0L || maxEdge !in MIN_SCRUB_PREVIEW_MAX_EDGE..MAX_SCRUB_PREVIEW_MAX_EDGE) {
            return NativeMicroscopePreview.Failure(
                code = "invalid_request",
                message = "Live preview request is outside safety bounds.",
                engine = null,
            )
        }
        loadFailure?.let {
            return NativeMicroscopePreview.Failure(
                code = "native_library_unavailable",
                message = "Rust engine could not be loaded: ${it.message ?: it::class.java.simpleName}",
                engine = null,
            )
        }

        val capacity = runCatching {
            Math.toIntExact(
                Math.multiplyExact(
                    Math.multiplyExact(maxEdge.toLong(), maxEdge.toLong()),
                    RGBA_BYTES_PER_PIXEL,
                ),
            )
        }.getOrElse {
            return NativeMicroscopePreview.Failure(
                code = "invalid_request",
                message = "Live preview buffer size overflowed.",
                engine = null,
            )
        }
        val lease = try {
            directBufferPool.borrow(capacity)
        } catch (_: OutOfMemoryError) {
            return NativeMicroscopePreview.Failure(
                code = "buffer_allocation_failed",
                message = "Android could not allocate the bounded live preview buffer.",
                engine = null,
            )
        }

        lease.use {
            val destination = lease.buffer
            val jniStarted = System.nanoTime()
            val traceStarted = runCatching {
                Trace.beginSection(TRACE_PREVIEW_JNI)
                true
            }.getOrDefault(false)
            val raw = try {
                runCatching { nativeCall(destination) }.getOrElse {
                    return NativeMicroscopePreview.Failure(
                        code = "jni_error",
                        message = "Rust live preview failed: ${it.message ?: it::class.java.simpleName}",
                        engine = null,
                    )
                } ?: return NativeMicroscopePreview.Failure(
                    code = "jni_error",
                    message = "Rust engine returned a null live preview response.",
                    engine = null,
                )
            } finally {
                if (traceStarted) runCatching { Trace.endSection() }
            }
            val jniUs = elapsedUs(jniStarted)
            val parsed = parseResponse(raw, destination, sessionId, maxEdge)
            if (parsed !is NativeMicroscopePreview.Success) return parsed
            return materializeBitmapPreview(
                parsed = parsed,
                directBufferAllocated = lease.allocated,
                directBufferCapacity = destination.capacity(),
                jniUs = jniUs,
            )
        }
    }

    private fun materializeBitmapPreview(
        parsed: NativeMicroscopePreview.Success,
        directBufferAllocated: Boolean,
        directBufferCapacity: Int,
        jniUs: Long,
    ): NativeMicroscopePreview {
        val descriptor = parsed.preview.descriptor
        val rgba = parsed.preview.rgba ?: return NativeMicroscopePreview.Failure(
            code = "bridge_error",
            message = "Parsed live preview did not retain its synchronous RGBA transport buffer.",
            engine = parsed.engine,
        )
        var bitmap: Bitmap? = null
        val bitmapStarted = System.nanoTime()
        val traceStarted = runCatching {
            Trace.beginSection(TRACE_PREVIEW_BITMAP)
            true
        }.getOrDefault(false)
        return try {
            bitmap = Bitmap.createBitmap(descriptor.width, descriptor.height, Bitmap.Config.ARGB_8888)
            val source = rgba.duplicate().apply {
                position(0)
                limit(descriptor.byteLen)
            }
            bitmap.copyPixelsFromBuffer(source)
            val bitmapUs = elapsedUs(bitmapStarted)
            PixelTransportTelemetry.recordPreview(
                descriptor = descriptor,
                directBufferAllocated = directBufferAllocated,
                directBufferCapacity = directBufferCapacity,
                jniUs = jniUs,
                bitmapAllocationBytes = bitmap.allocationByteCount.toLong(),
                bitmapConversionUs = bitmapUs,
            )
            NativeMicroscopePreview.Success(
                preview = MicroscopeScrubPreview.fromBitmap(descriptor, bitmap),
                engine = parsed.engine,
            )
        } catch (_: OutOfMemoryError) {
            bitmap?.recycle()
            NativeMicroscopePreview.Failure(
                code = "bitmap_allocation_failed",
                message = "Android could not allocate the bounded live preview bitmap.",
                engine = parsed.engine,
            )
        } catch (error: RuntimeException) {
            bitmap?.recycle()
            NativeMicroscopePreview.Failure(
                code = "bitmap_copy_failed",
                message = "Android could not materialize the live preview bitmap: ${error.message ?: error::class.java.simpleName}",
                engine = parsed.engine,
            )
        } finally {
            if (traceStarted) runCatching { Trace.endSection() }
        }
    }

    internal fun parseResponse(
        raw: String,
        destination: ByteBuffer,
        expectedSessionId: Long,
        maxEdge: Int,
    ): NativeMicroscopePreview = try {
        val json = JSONObject(raw)
        val engine = json.optionalString("engine")
        when (json.optString("status")) {
            "ok" -> {
                if (engine == null) {
                    NativeMicroscopePreview.Failure(
                        code = "malformed_response",
                        message = "Rust live preview did not identify the engine.",
                        engine = null,
                    )
                } else {
                    val value = json.getJSONObject("preview")
                    val byteLenLong = value.getLong("byte_len")
                    val descriptor = if (byteLenLong in 1..Int.MAX_VALUE.toLong()) {
                        MicroscopePreviewDescriptor(
                            sessionId = value.getLong("session_id"),
                            frameId = value.getLong("frame_id"),
                            timestampUs = if (value.has("timestamp_us") && !value.isNull("timestamp_us")) {
                                value.getLong("timestamp_us")
                            } else {
                                null
                            },
                            width = value.getInt("width"),
                            height = value.getInt("height"),
                            strideBytes = value.getLong("stride_bytes"),
                            byteLen = byteLenLong.toInt(),
                            source = value.getString("source"),
                            decodedFrames = value.getLong("decoded_frames"),
                        )
                    } else {
                        null
                    }
                    if (
                        descriptor == null ||
                        descriptor.sessionId != expectedSessionId ||
                        !descriptor.isSane(maxEdge) ||
                        descriptor.byteLen > destination.capacity()
                    ) {
                        NativeMicroscopePreview.Failure(
                            code = "malformed_preview_descriptor",
                            message = "Rust returned live preview metadata outside safety or session bounds.",
                            engine = engine,
                        )
                    } else {
                        destination.position(0)
                        destination.limit(descriptor.byteLen)
                        NativeMicroscopePreview.Success(
                            preview = MicroscopeScrubPreview(
                                descriptor = descriptor,
                                rgba = destination.asReadOnlyBuffer(),
                            ),
                            engine = engine,
                        )
                    }
                }
            }

            "error" -> NativeMicroscopePreview.Failure(
                code = json.optString("code", "rust_error"),
                message = json.optString("message", "Rust live preview failed."),
                engine = engine,
            )

            else -> NativeMicroscopePreview.Failure(
                code = "malformed_response",
                message = "Rust returned an unrecognized live preview response.",
                engine = engine,
            )
        }
    } catch (error: Exception) {
        NativeMicroscopePreview.Failure(
            code = "malformed_response",
            message = "Could not decode Rust live preview: ${error.message ?: error::class.java.simpleName}",
            engine = null,
        )
    }

    private fun elapsedUs(startedNanos: Long): Long =
        ((System.nanoTime() - startedNanos).coerceAtLeast(0L)) / 1_000L

    private fun JSONObject.optionalString(key: String): String? =
        if (!has(key) || isNull(key)) null else getString(key).trim().takeIf(String::isNotEmpty)
}
