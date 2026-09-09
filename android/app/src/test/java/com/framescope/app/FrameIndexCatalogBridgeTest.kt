package com.framescope.app

import com.framescope.app.data.NativeFrameIndexCatalogResponse
import com.framescope.app.data.NativeSessionIndexBindingResponse
import com.framescope.app.data.PersistentFrameIndexStatus
import com.framescope.app.data.parseFrameIndexCatalogResponse
import com.framescope.app.data.parseSessionIndexBindingResponse
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FrameIndexCatalogBridgeTest {
    @Test
    fun catalogParsesIndexedInProgressAndStaleDescriptors() {
        val response = parseFrameIndexCatalogResponse(
            """
            {"status":"ok","engine":"framescope-rust/test","indexes":[
              {"source_key":"source_a","stream_index":0,"relative_path":"frame-index/v1/source_a/stream-0.sqlite3","status":"indexed","indexed_frames":100,"frame_count":100,"last_modified_epoch_ms":10},
              {"source_key":"source_b","stream_index":1,"relative_path":"frame-index/v1/source_b/stream-1.sqlite3","status":"in_progress","indexed_frames":12,"frame_count":null,"last_modified_epoch_ms":20},
              {"source_key":"source_c","stream_index":0,"relative_path":"frame-index/v1/source_c/stream-0.sqlite3","status":"stale","indexed_frames":0,"frame_count":null,"last_modified_epoch_ms":30}
            ]}
            """.trimIndent(),
        )

        val success = response as NativeFrameIndexCatalogResponse.Success
        assertEquals(
            listOf(
                PersistentFrameIndexStatus.Indexed,
                PersistentFrameIndexStatus.InProgress,
                PersistentFrameIndexStatus.Stale,
            ),
            success.indexes.map { it.status },
        )
        assertEquals(100L, success.indexes.first().frameCount)
    }

    @Test
    fun sessionBindingParsesExactSourceAndIndexIdentity() {
        val response = parseSessionIndexBindingResponse(
            """
            {"status":"ok","engine":"framescope-rust/test","binding":{"source_key":"source_a","stream_index":2,"relative_path":"frame-index/v1/source_a/stream-2.sqlite3","status":"indexed","indexed_frames":44,"frame_count":44}}
            """.trimIndent(),
        )

        val binding = (response as NativeSessionIndexBindingResponse.Success).binding
        assertEquals("source_a", binding.sourceKey)
        assertEquals(2, binding.streamIndex)
        assertEquals(44L, binding.frameCount)
    }

    @Test
    fun catalogRejectsPathTraversalFromNativePayload() {
        val error = runCatching {
            parseFrameIndexCatalogResponse(
                """
                {"status":"ok","engine":"framescope-rust/test","indexes":[{"source_key":"source_a","stream_index":0,"relative_path":"frame-index/v1/../outside.sqlite3","status":"indexed","indexed_frames":1,"frame_count":1,"last_modified_epoch_ms":1}]}
                """.trimIndent(),
            )
        }.exceptionOrNull()

        assertTrue(error is IllegalArgumentException)
    }

    @Test
    fun nativeErrorsRemainExplicit() {
        val response = parseFrameIndexCatalogResponse(
            """{"status":"error","engine":"framescope-rust/test","code":"storage_io","message":"nope"}""",
        )

        val failure = response as NativeFrameIndexCatalogResponse.Failure
        assertEquals("storage_io", failure.code)
        assertEquals("nope", failure.message)
    }
}
