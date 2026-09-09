package com.framescope.app.data

import android.graphics.Bitmap
import android.os.Trace
import java.nio.ByteBuffer
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
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
 * Production previews own one reference to a pooled Bitmap. Compose must acquire a draw lease before
 * reading that Bitmap. The bridge may then release producer ownership on supersession/cancellation
 * without ever returning pixels to the pool while the UI can still draw them. The RGBA constructor
 * remains only for deterministic JVM fixtures and parser tests.
 */
class MicroscopeScrubPreview private constructor(
    val descriptor: MicroscopePreviewDescriptor,
    internal val rgba: ByteBuffer?,
    private val bitmapHandle: SharedPreviewBitmapHandle?,
) {
    constructor(
        descriptor: MicroscopePreviewDescriptor,
        rgba: ByteBuffer,
    ) : this(descriptor = descriptor, rgba = rgba, bitmapHandle = null)

    /** Raw access is retained for compatibility checks; production display must use a draw lease. */
    internal val bitmap: Bitmap?
        get() = bitmapHandle?.bitmap

    internal fun acquireBitmapLease(): MicroscopePreviewBitmapLease? = bitmapHandle?.acquire()

    internal fun releaseBitmapOwner() {
        bitmapHandle?.releaseOwner()
    }

    internal fun bitmapOwnerReleasedForTest(): Boolean =
        bitmapHandle?.ownerReleasedForTest() ?: true

    companion object {
        internal fun fromBitmapLease(
            descriptor: MicroscopePreviewDescriptor,
            bitmapLease: PreviewBitmapPoolLease,
        ): MicroscopeScrubPreview = MicroscopeScrubPreview(
            descriptor = descriptor,
            rgba = null,
            bitmapHandle = SharedPreviewBitmapHandle(bitmapLease),
        )
    }
}

internal class MicroscopePreviewBitmapLease private constructor(
    val bitmap: Bitmap,
    private val handle: SharedPreviewBitmapHandle,
) : AutoCloseable {
    private val closed = AtomicBoolean(false)

    override fun close() {
        if (closed.compareAndSet(false, true)) {
            handle.releaseReference()
        }
    }

    companion object {
        fun acquire(handle: SharedPreviewBitmapHandle): MicroscopePreviewBitmapLease =
            MicroscopePreviewBitmapLease(handle.bitmap, handle)
    }
}

internal class SharedPreviewBitmapHandle(
    private val bitmapLease: PreviewBitmapPoolLease,
) {
    private val references = AtomicInteger(1)
    private val ownerReleased = AtomicBoolean(false)

    val bitmap: Bitmap
        get() = bitmapLease.bitmap

    fun acquire(): MicroscopePreviewBitmapLease? {
        while (true) {
            val current = references.get()
            if (current <= 0) return null
            if (references.compareAndSet(current, current + 1)) {
                return MicroscopePreviewBitmapLease.acquire(this)
            }
        }
    }

    fun releaseOwner() {
        if (ownerReleased.compareAndSet(false, true)) {
            releaseReference()
        }
    }

    fun releaseReference() {
        val remaining = references.decrementAndGet()
        check(remaining >= 0) { "Preview bitmap reference count underflow" }
        if (remaining == 0) {
            bitmapLease.close()
        }
    }

    internal fun ownerReleasedForTest(): Boolean = ownerReleased.get()
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
    private val bitmapPool = PreviewBitmapPool(maxRetainedBitmaps = 2)
    private val previewOwnersLock = Any()
    private val latestPreviewOwnerBySession = HashMap<Long, MicroscopeScrubPreview>()

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
        if (sessionId <= 0L) return false
        val releasedPreview = releaseLatestPreviewOwner(sessionId)
        if (loadFailure != null) return releasedPreview
        return runCatching { nativeCancelMicroscopePreviewSession(sessionId) }
            .getOrDefault(false) || releasedPreview
    }

    override fun forgetSession(sessionId: Long): Boolean {
        if (sessionId <= 0L) return false
        val releasedPreview = releaseLatestPreviewOwner(sessionId)
        if (loadFailure != null) return releasedPreview
        return runCatching { nativeForgetMicroscopePreviewSession(sessionId) }
            .getOrDefault(false) || releasedPreview
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
        val bitmapLease = try {
            bitmapPool.borrow(descriptor.width, descriptor.height)
        } catch (_: OutOfMemoryError) {
            return NativeMicroscopePreview.Failure(
                code = "bitmap_allocation_failed",
                message = "Android could not allocate the bounded live preview bitmap.",
                engine = parsed.engine,
            )
        } catch (error: RuntimeException) {
            return NativeMicroscopePreview.Failure(
                code = "bitmap_allocation_failed",
                message = "Android could not prepare the bounded live preview bitmap: ${error.message ?: error::class.java.simpleName}",
                engine = parsed.engine,
            )
        }
        val bitmap = bitmapLease.bitmap
        val bitmapStarted = System.nanoTime()
        val traceStarted = runCatching {
            Trace.beginSection(TRACE_PREVIEW_BITMAP)
            true
        }.getOrDefault(false)
        return try {
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
                bitmapAllocated = bitmapLease.allocated,
                bitmapAllocationBytes = if (bitmapLease.allocated) {
                    bitmap.allocationByteCount.toLong()
                } else {
                    0L
                },
                bitmapConversionUs = bitmapUs,
            )
            val preview = MicroscopeScrubPreview.fromBitmapLease(descriptor, bitmapLease)
            replaceLatestPreviewOwner(descriptor.sessionId, preview)
            NativeMicroscopePreview.Success(
                preview = preview,
                engine = parsed.engine,
            )
        } catch (_: OutOfMemoryError) {
            bitmapLease.close()
            NativeMicroscopePreview.Failure(
                code = "bitmap_allocation_failed",
                message = "Android could not allocate the bounded live preview bitmap.",
                engine = parsed.engine,
            )
        } catch (error: RuntimeException) {
            bitmapLease.close()
            NativeMicroscopePreview.Failure(
                code = "bitmap_copy_failed",
                message = "Android could not materialize the live preview bitmap: ${error.message ?: error::class.java.simpleName}",
                engine = parsed.engine,
            )
        } finally {
            if (traceStarted) runCatching { Trace.endSection() }
        }
    }

    private fun replaceLatestPreviewOwner(sessionId: Long, preview: MicroscopeScrubPreview) {
        val previous = synchronized(previewOwnersLock) {
            latestPreviewOwnerBySession.put(sessionId, preview)
        }
        if (previous !== preview) {
            previous?.releaseBitmapOwner()
        }
    }

    private fun releaseLatestPreviewOwner(sessionId: Long): Boolean {
        val previous = synchronized(previewOwnersLock) {
            latestPreviewOwnerBySession.remove(sessionId)
        } ?: return false
        previous.releaseBitmapOwner()
        return true
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

/**
 * Bounded reusable ARGB bitmap pool for live scrub previews.
 *
 * A bitmap returns here only when producer ownership and every active UI draw lease have both been
 * released. Reconfiguration therefore cannot mutate pixels still reachable by Compose. The pool
 * keeps the largest useful allocations, allowing smaller previews to reuse them without unbounded
 * managed/native bitmap growth.
 */
internal class PreviewBitmapPool(
    private val maxRetainedBitmaps: Int = 2,
) {
    private val retained = ArrayList<Bitmap>(maxRetainedBitmaps)

    init {
        require(maxRetainedBitmaps >= 0)
    }

    @Synchronized
    fun borrow(width: Int, height: Int): PreviewBitmapPoolLease {
        require(width > 0 && height > 0)
        val requiredBytes = Math.multiplyExact(Math.multiplyExact(width, height), 4)
        var bestIndex = -1
        var bestCapacity = Int.MAX_VALUE
        retained.forEachIndexed { index, candidate ->
            if (
                !candidate.isRecycled &&
                candidate.isMutable &&
                candidate.allocationByteCount >= requiredBytes &&
                candidate.allocationByteCount < bestCapacity
            ) {
                bestIndex = index
                bestCapacity = candidate.allocationByteCount
            }
        }

        if (bestIndex >= 0) {
            val bitmap = retained.removeAt(bestIndex)
            try {
                bitmap.reconfigure(width, height, Bitmap.Config.ARGB_8888)
                return PreviewBitmapPoolLease(
                    bitmap = bitmap,
                    allocated = false,
                    onRelease = ::release,
                )
            } catch (_: RuntimeException) {
                bitmap.recycle()
            }
        }

        return PreviewBitmapPoolLease(
            bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888),
            allocated = true,
            onRelease = ::release,
        )
    }

    @Synchronized
    private fun release(bitmap: Bitmap) {
        if (bitmap.isRecycled) return
        if (!bitmap.isMutable || maxRetainedBitmaps == 0) {
            bitmap.recycle()
            return
        }
        if (retained.size < maxRetainedBitmaps) {
            retained += bitmap
            return
        }
        val smallestIndex = retained.indices.minByOrNull { retained[it].allocationByteCount }
        if (smallestIndex == null) {
            bitmap.recycle()
            return
        }
        val smallest = retained[smallestIndex]
        if (smallest.allocationByteCount < bitmap.allocationByteCount) {
            retained[smallestIndex] = bitmap
            smallest.recycle()
        } else {
            bitmap.recycle()
        }
    }

    @Synchronized
    internal fun retainedCountForTest(): Int = retained.size
}

internal class PreviewBitmapPoolLease(
    val bitmap: Bitmap,
    val allocated: Boolean,
    private val onRelease: (Bitmap) -> Unit,
) : AutoCloseable {
    private val closed = AtomicBoolean(false)

    override fun close() {
        if (closed.compareAndSet(false, true)) {
            onRelease(bitmap)
        }
    }

    internal fun isClosedForTest(): Boolean = closed.get()
}
