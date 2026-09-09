package com.framescope.app.data

import android.graphics.Bitmap
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotSame
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class PreviewBitmapPoolTest {
    @Test
    fun releasedBitmapIsReusedAndReconfiguredForSmallerPreview() {
        val pool = PreviewBitmapPool(maxRetainedBitmaps = 2)
        val first = pool.borrow(width = 96, height = 64)
        val original = first.bitmap

        assertTrue(first.allocated)
        first.close()
        assertEquals(1, pool.retainedCountForTest())

        val second = pool.borrow(width = 48, height = 32)
        assertFalse(second.allocated)
        assertSame(original, second.bitmap)
        assertEquals(48, second.bitmap.width)
        assertEquals(32, second.bitmap.height)
        second.close()
    }

    @Test
    fun activeDrawLeasePreventsBitmapReuseUntilUiReleasesIt() {
        val pool = PreviewBitmapPool(maxRetainedBitmaps = 2)
        val producerLease = pool.borrow(width = 64, height = 64)
        val producerBitmap = producerLease.bitmap
        val preview = MicroscopeScrubPreview.fromBitmapLease(
            descriptor = descriptor(width = 64, height = 64),
            bitmapLease = producerLease,
        )
        val drawLease = requireNotNull(preview.acquireBitmapLease())

        preview.releaseBitmapOwner()
        assertTrue(preview.bitmapOwnerReleasedForTest())
        assertEquals(0, pool.retainedCountForTest())

        val concurrent = pool.borrow(width = 64, height = 64)
        assertTrue(concurrent.allocated)
        assertNotSame(producerBitmap, concurrent.bitmap)

        drawLease.close()
        assertEquals(1, pool.retainedCountForTest())
        concurrent.close()
        assertEquals(2, pool.retainedCountForTest())
    }

    @Test
    fun producerAndDrawLeaseReleaseAreIdempotent() {
        val pool = PreviewBitmapPool(maxRetainedBitmaps = 1)
        val producerLease = pool.borrow(width = 32, height = 32)
        val preview = MicroscopeScrubPreview.fromBitmapLease(
            descriptor = descriptor(width = 32, height = 32),
            bitmapLease = producerLease,
        )
        val drawLease = requireNotNull(preview.acquireBitmapLease())

        preview.releaseBitmapOwner()
        preview.releaseBitmapOwner()
        drawLease.close()
        drawLease.close()

        assertEquals(1, pool.retainedCountForTest())
        val reused = pool.borrow(width = 32, height = 32)
        assertFalse(reused.allocated)
        reused.close()
    }

    @Test
    fun undersizedBitmapIsNotReusedForLargerPreview() {
        val pool = PreviewBitmapPool(maxRetainedBitmaps = 2)
        val small = pool.borrow(width = 32, height = 32)
        val smallBitmap = small.bitmap
        small.close()

        val large = pool.borrow(width = 128, height = 128)
        assertTrue(large.allocated)
        assertNotSame(smallBitmap, large.bitmap)
        large.close()
    }

    @Test
    fun zeroCapacityPoolRecyclesOnReleaseInsteadOfRetaining() {
        val pool = PreviewBitmapPool(maxRetainedBitmaps = 0)
        val lease = pool.borrow(width = 32, height = 32)
        val bitmap: Bitmap = lease.bitmap

        lease.close()

        assertEquals(0, pool.retainedCountForTest())
        assertTrue(bitmap.isRecycled)
    }

    private fun descriptor(width: Int, height: Int) = MicroscopePreviewDescriptor(
        sessionId = 77L,
        frameId = 3L,
        timestampUs = 120_000L,
        width = width,
        height = height,
        strideBytes = width.toLong() * 4L,
        byteLen = width * height * 4,
        source = "decoded",
        decodedFrames = 1L,
    )
}
