package com.framescope.app.data

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Test

class PixelTransportTelemetryTest {
    @Before
    fun reset() {
        PixelTransportTelemetry.resetForTest()
    }

    @After
    fun cleanup() {
        PixelTransportTelemetry.resetForTest()
    }

    @Test
    fun livePreviewSeparatesBitmapAllocationFromReuse() {
        val descriptor = MicroscopePreviewDescriptor(
            sessionId = 9L,
            frameId = 4L,
            timestampUs = 200_000L,
            width = 16,
            height = 8,
            strideBytes = 64L,
            byteLen = 512,
            source = "preview_ram",
            decodedFrames = 0L,
        )

        PixelTransportTelemetry.recordPreview(
            descriptor = descriptor,
            directBufferAllocated = true,
            directBufferCapacity = 1_024,
            jniUs = 50L,
            bitmapAllocated = true,
            bitmapAllocationBytes = 512L,
            bitmapConversionUs = 20L,
        )
        PixelTransportTelemetry.recordPreview(
            descriptor = descriptor.copy(frameId = 5L),
            directBufferAllocated = false,
            directBufferCapacity = 1_024,
            jniUs = 30L,
            bitmapAllocated = false,
            bitmapAllocationBytes = 0L,
            bitmapConversionUs = 10L,
        )

        val snapshot = PixelTransportTelemetry.snapshot()
        assertEquals(1L, snapshot.directBufferAllocations)
        assertEquals(1L, snapshot.directBufferReuses)
        assertEquals(1_024L, snapshot.directBufferAllocatedBytes)
        assertEquals(1L, snapshot.bitmapAllocations)
        assertEquals(1L, snapshot.bitmapReuses)
        assertEquals(512L, snapshot.bitmapAllocatedBytes)
        assertEquals(1_024L, snapshot.nativeToJvmCopiedBytes)
        assertEquals(1_024L, snapshot.bitmapCopiedBytes)
        assertEquals(80L, snapshot.totalJniUs)
        assertEquals(30L, snapshot.totalBitmapConversionUs)
    }
}
