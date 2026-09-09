package com.framescope.app

import com.framescope.app.data.MicroscopePreviewDescriptor
import com.framescope.app.data.PixelTransportTelemetry
import org.junit.Assert.assertEquals
import org.junit.Test

class PixelTransportTelemetryTest {
    @Test
    fun previewAccountingCountsOnlyMeasuredTransportWork() {
        PixelTransportTelemetry.resetForTest()
        val descriptor = MicroscopePreviewDescriptor(
            sessionId = 9L,
            frameId = 11L,
            timestampUs = 42_000L,
            width = 2,
            height = 2,
            strideBytes = 8L,
            byteLen = 16,
            source = "decoded",
            decodedFrames = 3L,
        )

        PixelTransportTelemetry.recordPreview(
            descriptor = descriptor,
            directBufferAllocated = true,
            directBufferCapacity = 64,
            jniUs = 120L,
            bitmapAllocationBytes = 16L,
            bitmapConversionUs = 30L,
        )
        PixelTransportTelemetry.recordPreview(
            descriptor = descriptor,
            directBufferAllocated = false,
            directBufferCapacity = 64,
            jniUs = 80L,
            bitmapAllocationBytes = 16L,
            bitmapConversionUs = 20L,
        )

        val snapshot = PixelTransportTelemetry.snapshot()
        assertEquals(1L, snapshot.directBufferAllocations)
        assertEquals(1L, snapshot.directBufferReuses)
        assertEquals(64L, snapshot.directBufferAllocatedBytes)
        assertEquals(32L, snapshot.nativeToJvmCopiedBytes)
        assertEquals(2L, snapshot.bitmapAllocations)
        assertEquals(32L, snapshot.bitmapAllocatedBytes)
        assertEquals(32L, snapshot.bitmapCopiedBytes)
        assertEquals(200L, snapshot.totalJniUs)
        assertEquals(50L, snapshot.totalBitmapConversionUs)
    }
}
