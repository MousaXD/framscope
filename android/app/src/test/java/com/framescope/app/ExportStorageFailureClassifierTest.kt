package com.framescope.app

import com.framescope.app.data.ExportStorageFailureClassifier
import java.io.IOException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ExportStorageFailureClassifierTest {
    @Test
    fun `classifies direct no-space IOException`() {
        val failure = ExportStorageFailureClassifier.classify(
            IOException("No space left on device"),
        )

        assertEquals("storage_full", failure?.code)
    }

    @Test
    fun `classifies nested quota failure`() {
        val failure = ExportStorageFailureClassifier.classify(
            IllegalStateException(
                "provider failed",
                IOException("Disk quota exceeded"),
            ),
        )

        assertEquals("storage_full", failure?.code)
    }

    @Test
    fun `native classification is restricted to output errors`() {
        assertEquals(
            "storage_full",
            ExportStorageFailureClassifier.classifyNative(
                "io_error",
                "No space left on device (os error 28)",
            )?.code,
        )
        assertNull(
            ExportStorageFailureClassifier.classifyNative(
                "decode_error",
                "No space left on device",
            ),
        )
    }

    @Test
    fun `ordinary IO failure remains generic`() {
        assertNull(
            ExportStorageFailureClassifier.classify(
                IOException("Document provider disconnected"),
            ),
        )
    }
}
