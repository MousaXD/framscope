package com.framescope.app.data

import android.util.Log
import java.nio.ByteBuffer
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

/**
 * Pixel-transport observability that never participates in frame identity, ordering, cache keys,
 * cancellation, or admission. Counters intentionally describe transport work only; decoder and
 * cache telemetry remain owned by their respective subsystems.
 */
internal object PixelTransportTelemetry {
    private const val TAG = "FrameScopePixels"

    private val directBufferAllocations = AtomicLong()
    private val directBufferReuses = AtomicLong()
    private val directBufferAllocatedBytes = AtomicLong()
    private val nativeToJvmCopiedBytes = AtomicLong()
    private val bitmapAllocations = AtomicLong()
    private val bitmapCopiedBytes = AtomicLong()
    private val totalJniUs = AtomicLong()
    private val totalBitmapConversionUs = AtomicLong()
    private val totalNativeConversionUs = AtomicLong()
    private val totalNativeCopyUs = AtomicLong()
    private val previewPixelAllocations = AtomicLong()

    fun recordPreview(
        descriptor: MicroscopePreviewDescriptor,
        directBufferAllocated: Boolean,
        directBufferCapacity: Int,
        jniUs: Long,
        bitmapAllocationBytes: Long,
        bitmapConversionUs: Long,
    ) {
        if (directBufferAllocated) {
            directBufferAllocations.incrementAndGet()
            directBufferAllocatedBytes.addAndGet(directBufferCapacity.toLong())
        } else {
            directBufferReuses.incrementAndGet()
        }
        nativeToJvmCopiedBytes.addAndGet(descriptor.nativeBytesCopied)
        bitmapAllocations.incrementAndGet()
        bitmapCopiedBytes.addAndGet(descriptor.byteLen.toLong())
        totalJniUs.addAndGet(jniUs.coerceAtLeast(0L))
        totalBitmapConversionUs.addAndGet(bitmapConversionUs.coerceAtLeast(0L))
        totalNativeConversionUs.addAndGet(descriptor.nativeConversionUs)
        totalNativeCopyUs.addAndGet(descriptor.nativeCopyUs)
        previewPixelAllocations.addAndGet(descriptor.previewPixelAllocations)
        log(
            "path=live_preview frame_id=${descriptor.frameId} source=${descriptor.source} " +
                "bytes=${descriptor.byteLen} direct_buffer=${if (directBufferAllocated) "allocated" else "reused"} " +
                "direct_capacity=$directBufferCapacity native_conversion_us=${descriptor.nativeConversionUs} " +
                "native_copy_us=${descriptor.nativeCopyUs} jni_us=$jniUs bitmap_us=$bitmapConversionUs " +
                "preview_pixel_allocations=${descriptor.previewPixelAllocations}",
        )
    }

    fun recordAuthoritativeBufferAllocation(bytes: Int) {
        directBufferAllocations.incrementAndGet()
        directBufferAllocatedBytes.addAndGet(bytes.toLong())
    }

    fun recordAuthoritativeJniCopy(bytes: Long, jniUs: Long) {
        if (bytes > 0L) nativeToJvmCopiedBytes.addAndGet(bytes)
        totalJniUs.addAndGet(jniUs.coerceAtLeast(0L))
        log("path=authoritative_frame native_to_jvm_bytes=$bytes jni_us=$jniUs")
    }

    fun recordAuthoritativeBitmap(
        allocationBytes: Long,
        copiedBytes: Long,
        conversionUs: Long,
    ) {
        bitmapAllocations.incrementAndGet()
        bitmapCopiedBytes.addAndGet(copiedBytes.coerceAtLeast(0L))
        totalBitmapConversionUs.addAndGet(conversionUs.coerceAtLeast(0L))
        log(
            "path=authoritative_bitmap allocation_bytes=$allocationBytes " +
                "copied_bytes=$copiedBytes conversion_us=$conversionUs",
        )
    }

    fun snapshot(): PixelTransportSnapshot = PixelTransportSnapshot(
        directBufferAllocations = directBufferAllocations.get(),
        directBufferReuses = directBufferReuses.get(),
        directBufferAllocatedBytes = directBufferAllocatedBytes.get(),
        nativeToJvmCopiedBytes = nativeToJvmCopiedBytes.get(),
        bitmapAllocations = bitmapAllocations.get(),
        bitmapCopiedBytes = bitmapCopiedBytes.get(),
        totalJniUs = totalJniUs.get(),
        totalBitmapConversionUs = totalBitmapConversionUs.get(),
        totalNativeConversionUs = totalNativeConversionUs.get(),
        totalNativeCopyUs = totalNativeCopyUs.get(),
        previewPixelAllocations = previewPixelAllocations.get(),
    )

    internal fun resetForTest() {
        listOf(
            directBufferAllocations,
            directBufferReuses,
            directBufferAllocatedBytes,
            nativeToJvmCopiedBytes,
            bitmapAllocations,
            bitmapCopiedBytes,
            totalJniUs,
            totalBitmapConversionUs,
            totalNativeConversionUs,
            totalNativeCopyUs,
            previewPixelAllocations,
        ).forEach { it.set(0L) }
    }

    private fun log(message: String) {
        runCatching {
            if (Log.isLoggable(TAG, Log.DEBUG)) Log.d(TAG, message)
        }
    }
}

internal data class PixelTransportSnapshot(
    val directBufferAllocations: Long,
    val directBufferReuses: Long,
    val directBufferAllocatedBytes: Long,
    val nativeToJvmCopiedBytes: Long,
    val bitmapAllocations: Long,
    val bitmapCopiedBytes: Long,
    val totalJniUs: Long,
    val totalBitmapConversionUs: Long,
    val totalNativeConversionUs: Long,
    val totalNativeCopyUs: Long,
    val previewPixelAllocations: Long,
)

/**
 * Bounded direct-buffer pool for synchronous JNI preview transport.
 *
 * A borrowed buffer is never returned to the pool until its lease closes. The production preview
 * bridge closes the lease only after pixels have been copied into a caller-owned Bitmap, so Java
 * never retains a view of memory that can be refilled by a later native request.
 */
internal class PreviewDirectBufferPool(
    private val maxRetainedBuffers: Int = 2,
) {
    private val retained = ArrayList<ByteBuffer>(maxRetainedBuffers)

    init {
        require(maxRetainedBuffers >= 0)
    }

    @Synchronized
    fun borrow(minCapacity: Int): PreviewDirectBufferLease {
        require(minCapacity > 0)
        var bestIndex = -1
        var bestCapacity = Int.MAX_VALUE
        retained.forEachIndexed { index, candidate ->
            val capacity = candidate.capacity()
            if (capacity >= minCapacity && capacity < bestCapacity) {
                bestIndex = index
                bestCapacity = capacity
            }
        }
        val allocated = bestIndex < 0
        val buffer = if (allocated) {
            ByteBuffer.allocateDirect(minCapacity)
        } else {
            retained.removeAt(bestIndex)
        }
        buffer.clear()
        return PreviewDirectBufferLease(
            buffer = buffer,
            allocated = allocated,
            onRelease = ::release,
        )
    }

    @Synchronized
    private fun release(buffer: ByteBuffer) {
        buffer.clear()
        if (retained.size < maxRetainedBuffers) {
            retained += buffer
        }
    }

    @Synchronized
    internal fun retainedCountForTest(): Int = retained.size
}

internal class PreviewDirectBufferLease(
    val buffer: ByteBuffer,
    val allocated: Boolean,
    private val onRelease: (ByteBuffer) -> Unit,
) : AutoCloseable {
    private val closed = AtomicBoolean(false)

    override fun close() {
        if (closed.compareAndSet(false, true)) {
            onRelease(buffer)
        }
    }

    internal fun isClosedForTest(): Boolean = closed.get()
}
