package com.framescope.app.data

import android.util.Log
import java.nio.ByteBuffer
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

/**
 * Pixel-transport observability that never participates in frame identity, ordering, cache keys,
 * cancellation, or admission. Counters intentionally describe measured transport work only;
 * decoder, swscale, and cache telemetry remain owned by their respective subsystems.
 */
internal object PixelTransportTelemetry {
    private const val TAG = "FrameScopePixels"

    private val directBufferAllocations = AtomicLong()
    private val directBufferReuses = AtomicLong()
    private val directBufferAllocatedBytes = AtomicLong()
    private val nativeToJvmCopiedBytes = AtomicLong()
    private val bitmapAllocations = AtomicLong()
    private val bitmapReuses = AtomicLong()
    private val bitmapAllocatedBytes = AtomicLong()
    private val bitmapCopiedBytes = AtomicLong()
    private val totalJniUs = AtomicLong()
    private val totalBitmapConversionUs = AtomicLong()

    fun recordPreview(
        descriptor: MicroscopePreviewDescriptor,
        directBufferAllocated: Boolean,
        directBufferCapacity: Int,
        jniUs: Long,
        bitmapAllocated: Boolean,
        bitmapAllocationBytes: Long,
        bitmapConversionUs: Long,
    ) {
        if (directBufferAllocated) {
            directBufferAllocations.incrementAndGet()
            directBufferAllocatedBytes.addAndGet(directBufferCapacity.toLong())
        } else {
            directBufferReuses.incrementAndGet()
        }
        nativeToJvmCopiedBytes.addAndGet(descriptor.byteLen.toLong())
        if (bitmapAllocated) {
            bitmapAllocations.incrementAndGet()
            bitmapAllocatedBytes.addAndGet(bitmapAllocationBytes.coerceAtLeast(0L))
        } else {
            bitmapReuses.incrementAndGet()
        }
        bitmapCopiedBytes.addAndGet(descriptor.byteLen.toLong())
        totalJniUs.addAndGet(jniUs.coerceAtLeast(0L))
        totalBitmapConversionUs.addAndGet(bitmapConversionUs.coerceAtLeast(0L))
        val measuredTransportAllocations =
            (if (directBufferAllocated) 1 else 0) + (if (bitmapAllocated) 1 else 0)
        log(
            "path=live_preview frame_id=${descriptor.frameId} source=${descriptor.source} " +
                "payload_bytes=${descriptor.byteLen} native_to_jvm_bytes=${descriptor.byteLen} " +
                "direct_buffer=${if (directBufferAllocated) "allocated" else "reused"} " +
                "direct_capacity=$directBufferCapacity " +
                "bitmap=${if (bitmapAllocated) "allocated" else "reused"} " +
                "bitmap_allocation_bytes=${if (bitmapAllocated) bitmapAllocationBytes else 0L} " +
                "measured_transport_allocations=$measuredTransportAllocations " +
                "jni_us=$jniUs bitmap_conversion_us=$bitmapConversionUs",
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
        bitmapAllocatedBytes.addAndGet(allocationBytes.coerceAtLeast(0L))
        bitmapCopiedBytes.addAndGet(copiedBytes.coerceAtLeast(0L))
        totalBitmapConversionUs.addAndGet(conversionUs.coerceAtLeast(0L))
        log(
            "path=authoritative_bitmap bitmap_allocation_bytes=$allocationBytes " +
                "copied_bytes=$copiedBytes bitmap_conversion_us=$conversionUs",
        )
    }

    fun snapshot(): PixelTransportSnapshot = PixelTransportSnapshot(
        directBufferAllocations = directBufferAllocations.get(),
        directBufferReuses = directBufferReuses.get(),
        directBufferAllocatedBytes = directBufferAllocatedBytes.get(),
        nativeToJvmCopiedBytes = nativeToJvmCopiedBytes.get(),
        bitmapAllocations = bitmapAllocations.get(),
        bitmapReuses = bitmapReuses.get(),
        bitmapAllocatedBytes = bitmapAllocatedBytes.get(),
        bitmapCopiedBytes = bitmapCopiedBytes.get(),
        totalJniUs = totalJniUs.get(),
        totalBitmapConversionUs = totalBitmapConversionUs.get(),
    )

    internal fun resetForTest() {
        listOf(
            directBufferAllocations,
            directBufferReuses,
            directBufferAllocatedBytes,
            nativeToJvmCopiedBytes,
            bitmapAllocations,
            bitmapReuses,
            bitmapAllocatedBytes,
            bitmapCopiedBytes,
            totalJniUs,
            totalBitmapConversionUs,
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
    val bitmapReuses: Long,
    val bitmapAllocatedBytes: Long,
    val bitmapCopiedBytes: Long,
    val totalJniUs: Long,
    val totalBitmapConversionUs: Long,
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
        if (maxRetainedBuffers == 0) return
        if (retained.size < maxRetainedBuffers) {
            retained += buffer
            return
        }
        val smallestIndex = retained.indices.minByOrNull { retained[it].capacity() } ?: return
        if (retained[smallestIndex].capacity() < buffer.capacity()) {
            retained[smallestIndex] = buffer
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
