package com.framescope.app

import com.framescope.app.data.BatchExportProgress
import com.framescope.app.data.BatchSafFrameSink
import com.framescope.app.data.ExportDocument
import com.framescope.app.data.ExportDocumentFactory
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class BatchSafFrameSinkTest {
    @Test
    fun `sink owns only one pending document and commits matching identity`() {
        val factory = FakeDocumentFactory()
        val progress = mutableListOf<BatchExportProgress>()
        BatchSafFrameSink("content://tree/export", factory, progress::add).use { sink ->
            assertEquals(
                41,
                sink.openFrame("frame_0001.png", "image/png", 1L, 1L, 2L),
            )
            assertEquals(
                -1,
                sink.openFrame("frame_0002.png", "image/png", 2L, 2L, 2L),
            )
            assertEquals(1, factory.documents.size)
            assertTrue(sink.commitFrame("frame_0001.png", 1L, 1L, 2L, 128L))
        }

        val document = factory.documents.single()
        assertTrue(document.committed)
        assertTrue(document.closed)
        assertEquals(listOf(BatchExportProgress(1L, 1L, 2L)), progress)
    }

    @Test
    fun `abort closes an uncommitted document and frees the next slot`() {
        val factory = FakeDocumentFactory()
        BatchSafFrameSink("content://tree/export", factory).use { sink ->
            assertEquals(41, sink.openFrame("frame_1.webp", "image/webp", 1L, 1L, 2L))
            sink.abortFrame("frame_1.webp")
            assertEquals(42, sink.openFrame("frame_2.webp", "image/webp", 2L, 2L, 2L))
        }

        assertFalse(factory.documents[0].committed)
        assertTrue(factory.documents[0].closed)
        assertFalse(factory.documents[1].committed)
        assertTrue(factory.documents[1].closed)
    }

    @Test
    fun `identity mismatch cannot commit pending artifact`() {
        val factory = FakeDocumentFactory()
        BatchSafFrameSink("content://tree/export", factory).use { sink ->
            assertEquals(41, sink.openFrame("frame_1.jpg", "image/jpeg", 1L, 1L, 1L))
            assertFalse(sink.commitFrame("frame_2.jpg", 2L, 1L, 1L, 100L))
            sink.abortFrame("frame_1.jpg")
        }

        val document = factory.documents.single()
        assertFalse(document.committed)
        assertTrue(document.closed)
    }

    @Test
    fun `invalid metadata and storage failure return sentinel without leaking`() {
        val factory = FakeDocumentFactory()
        BatchSafFrameSink("content://tree/export", factory).use { sink ->
            assertEquals(-1, sink.openFrame("../bad.png", "image/png", 0L, 1L, 1L))
            assertEquals(-1, sink.openFrame("frame.png", "text/plain", 0L, 1L, 1L))
            assertEquals(-1, sink.openFrame("frame.png", "image/png", 0L, 0L, 1L))
            factory.fail = true
            assertEquals(-1, sink.openFrame("frame.png", "image/png", 0L, 1L, 1L))
        }
        assertTrue(factory.documents.isEmpty())
    }

    private class FakeDocumentFactory : ExportDocumentFactory {
        val documents = mutableListOf<FakeDocument>()
        var fail = false

        override fun create(
            treeUri: String,
            displayName: String,
            mimeType: String,
        ): ExportDocument {
            if (fail) error("forced storage failure")
            return FakeDocument(fd = 41 + documents.size).also(documents::add)
        }
    }

    private class FakeDocument(
        override val fd: Int,
    ) : ExportDocument {
        override val uri: String = "content://document/$fd"
        var committed = false
        var closed = false

        override fun commit() {
            check(!closed)
            committed = true
        }

        override fun close() {
            closed = true
        }
    }
}
