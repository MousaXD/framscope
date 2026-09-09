package com.framescope.app

import com.framescope.app.data.PreviewDirectBufferPool
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotSame
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class PreviewDirectBufferPoolTest {
    @Test
    fun activeLeaseIsNeverReusedBeforeClose() {
        val pool = PreviewDirectBufferPool(maxRetainedBuffers = 2)
        val first = pool.borrow(64)
        val second = pool.borrow(64)

        assertNotSame(first.buffer, second.buffer)
        assertTrue(first.allocated)
        assertTrue(second.allocated)
        assertFalse(first.isClosedForTest())
        assertEquals(0, pool.retainedCountForTest())

        first.close()
        second.close()
        assertEquals(2, pool.retainedCountForTest())
    }

    @Test
    fun closedLeaseCanBeReusedWithoutExposingOldOwner() {
        val pool = PreviewDirectBufferPool(maxRetainedBuffers = 1)
        val first = pool.borrow(128)
        val original = first.buffer
        first.close()
        first.close()

        assertTrue(first.isClosedForTest())
        assertEquals(1, pool.retainedCountForTest())

        val reused = pool.borrow(64)
        assertSame(original, reused.buffer)
        assertFalse(reused.allocated)
        assertEquals(0, reused.buffer.position())
        assertEquals(reused.buffer.capacity(), reused.buffer.limit())
        reused.close()
    }

    @Test
    fun undersizedRetainedBufferIsNotReturnedForLargerRequest() {
        val pool = PreviewDirectBufferPool(maxRetainedBuffers = 2)
        val small = pool.borrow(64)
        val smallBuffer = small.buffer
        small.close()

        val large = pool.borrow(256)
        assertNotSame(smallBuffer, large.buffer)
        assertTrue(large.allocated)
        large.close()
    }

    @Test
    fun largerReturnedBufferReplacesSmallerRetainedCapacity() {
        val pool = PreviewDirectBufferPool(maxRetainedBuffers = 1)
        val small = pool.borrow(64)
        small.close()

        val large = pool.borrow(256)
        val largeBuffer = large.buffer
        large.close()

        val reused = pool.borrow(128)
        assertSame(largeBuffer, reused.buffer)
        assertFalse(reused.allocated)
        reused.close()
    }

    @Test
    fun zeroRetentionPoolNeverPublishesAClosedLeaseForReuse() {
        val pool = PreviewDirectBufferPool(maxRetainedBuffers = 0)
        val first = pool.borrow(64)
        val firstBuffer = first.buffer
        first.close()

        assertEquals(0, pool.retainedCountForTest())
        val second = pool.borrow(64)
        assertNotSame(firstBuffer, second.buffer)
        assertTrue(second.allocated)
        second.close()
    }
}
