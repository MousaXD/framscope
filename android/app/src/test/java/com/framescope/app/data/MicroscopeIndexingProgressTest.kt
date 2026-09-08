package com.framescope.app.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test

class MicroscopeIndexingProgressTest {
    @Test
    fun parsesOperationScopedVfrProgress() {
        val progress = RustMicroscopeIndexingProgressSource.parseResponse(
            raw = """{"status":"ok","progress":{"operation_id":17,"stage":"indexing","indexed_frames":420,"reused_frames":64,"expected_reuse_frames":64,"first_timestamp_us":100000,"current_timestamp_us":9700000,"elapsed_ms":1450}}""",
            expectedOperationId = 17L,
        )

        requireNotNull(progress)
        assertEquals(MicroscopeIndexingStage.Indexing, progress.stage)
        assertEquals(420L, progress.indexedFrames)
        assertEquals(64L, progress.reusedFrames)
        assertEquals(100_000L, progress.firstTimestampUs)
        assertEquals(9_700_000L, progress.currentTimestampUs)
    }

    @Test
    fun idleMeansOperationHasNoPublishedProgress() {
        assertNull(
            RustMicroscopeIndexingProgressSource.parseResponse(
                raw = """{"status":"idle"}""",
                expectedOperationId = 99L,
            ),
        )
    }

    @Test
    fun rejectsProgressFromDifferentOperation() {
        assertThrows(IllegalArgumentException::class.java) {
            RustMicroscopeIndexingProgressSource.parseResponse(
                raw = """{"status":"ok","progress":{"operation_id":18,"stage":"indexing","indexed_frames":1,"reused_frames":0,"expected_reuse_frames":0,"first_timestamp_us":0,"current_timestamp_us":0,"elapsed_ms":1}}""",
                expectedOperationId = 17L,
            )
        }
    }

    @Test
    fun rejectsUnknownStageRatherThanGuessing() {
        assertThrows(IllegalArgumentException::class.java) {
            RustMicroscopeIndexingProgressSource.parseResponse(
                raw = """{"status":"ok","progress":{"operation_id":17,"stage":"almost_ready","indexed_frames":1,"reused_frames":0,"expected_reuse_frames":0,"first_timestamp_us":0,"current_timestamp_us":0,"elapsed_ms":1}}""",
                expectedOperationId = 17L,
            )
        }
    }
}
