package com.framescope.app

import com.framescope.app.data.BatchSafFrameSink
import com.framescope.app.data.ExportDocument
import com.framescope.app.data.ExportDocumentFactory
import java.io.IOException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class BatchSafFrameSinkFailureTest {
    @Test
    fun `document creation capacity failure is reported without leaking`() {
        val factory = FailureFactory(createFailure = IOException("No space left on device"))
        BatchSafFrameSink("content://tree/export", factory).use { sink ->
            assertEquals(-1, sink.openFrame("frame_1.png", "image/png", 1L, 1L, 1L))
            assertEquals("storage_full", sink.failureCode())
        }
        assertTrue(factory.documents.isEmpty())
    }

    @Test
    fun `fd capacity failure aborts pending document`() {
        val factory = FailureFactory(fdFailure = IOException("Disk quota exceeded"))
        BatchSafFrameSink("content://tree/export", factory).use { sink ->
            assertEquals(-1, sink.openFrame("frame_1.png", "image/png", 1L, 1L, 1L))
            assertEquals("storage_full", sink.failureCode())
        }
        assertTrue(factory.documents.single().closed)
        assertFalse(factory.documents.single().committed)
    }

    @Test
    fun `commit capacity failure clears pending slot and preserves first failure`() {
        val factory = FailureFactory(commitFailure = IOException("No space left on device"))
        BatchSafFrameSink("content://tree/export", factory).use { sink ->
            assertEquals(41, sink.openFrame("frame_1.png", "image/png", 1L, 1L, 2L))
            assertFalse(sink.commitFrame("frame_1.png", 1L, 1L, 2L, 128L))
            assertEquals("storage_full", sink.failureCode())

            factory.commitFailure = null
            assertEquals(42, sink.openFrame("frame_2.png", "image/png", 2L, 2L, 2L))
        }
        assertTrue(factory.documents.all { it.closed })
    }

    @Test
    fun `ordinary callback failure is not mislabeled as storage full`() {
        val factory = FailureFactory()
        BatchSafFrameSink(
            treeUri = "content://tree/export",
            documentFactory = factory,
            onProgress = { error("progress listener failed") },
        ).use { sink ->
            assertEquals(-1, sink.openFrame("frame_1.png", "image/png", 1L, 1L, 1L))
            assertNull(sink.failureCode())
        }
        assertTrue(factory.documents.single().closed)
    }

    @Test
    fun `close aborts one pending document`() {
        val factory = FailureFactory()
        val sink = BatchSafFrameSink("content://tree/export", factory)
        assertEquals(41, sink.openFrame("frame_1.webp", "image/webp", 1L, 1L, 1L))
        sink.close()

        assertEquals(1, factory.documents.size)
        assertTrue(factory.documents.single().closed)
        assertFalse(factory.documents.single().committed)
    }

    private class FailureFactory(
        private val createFailure: Throwable? = null,
        private val fdFailure: Throwable? = null,
        var commitFailure: Throwable? = null,
    ) : ExportDocumentFactory {
        val documents = mutableListOf<FailureDocument>()

        override fun create(
            treeUri: String,
            displayName: String,
            mimeType: String,
        ): ExportDocument {
            createFailure?.let { throw it }
            return FailureDocument(
                fdValue = 41 + documents.size,
                fdFailure = fdFailure,
                commitFailure = { commitFailure },
            ).also(documents::add)
        }
    }

    private class FailureDocument(
        private val fdValue: Int,
        private val fdFailure: Throwable?,
        private val commitFailure: () -> Throwable?,
    ) : ExportDocument {
        override val uri: String = "content://document/$fdValue"
        var committed = false
        var closed = false

        override val fd: Int
            get() {
                fdFailure?.let { throw it }
                return fdValue
            }

        override fun commit() {
            commitFailure()?.let { throw it }
            committed = true
        }

        override fun close() {
            closed = true
        }
    }
}
