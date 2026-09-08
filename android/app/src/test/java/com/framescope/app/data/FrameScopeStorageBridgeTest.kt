package com.framescope.app.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FrameScopeStorageBridgeTest {
    @Test
    fun parsesTruthfulStorageSummaryAndClearReceipt() {
        val response = parseStorageResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/test",
              "storage":{
                "total_bytes":7168,
                "persistent_indexes":{"bytes":6144,"files":3,"items":3},
                "preview_proxy":{"bytes":0,"files":0,"items":0},
                "disposable":{"bytes":1024,"files":2,"items":1},
                "indexed_sources":2,
                "preview_proxy_enabled":false
              },
              "cleared":{
                "scope":"persistent_indexes",
                "cleared_bytes":4096,
                "cleared_files":2,
                "cleared_items":1
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeStorageResponse.Success)
        response as NativeStorageResponse.Success
        assertEquals(7168L, response.storage.totalBytes)
        assertEquals(6144L, response.storage.persistentIndexes.bytes)
        assertEquals(2L, response.storage.indexedSources)
        assertFalse(response.storage.previewProxyEnabled)
        assertEquals(StorageClearScope.PersistentIndexes, response.cleared?.scope)
        assertEquals(4096L, response.cleared?.clearedBytes)
    }

    @Test
    fun parsesTypedNativeFailure() {
        val response = parseStorageResponse(
            """{"status":"error","engine":"framescope-rust/test","code":"storage_io","message":"disk failed"}""",
        )

        assertEquals(
            NativeStorageResponse.Failure(
                code = "storage_io",
                message = "disk failed",
                engine = "framescope-rust/test",
            ),
            response,
        )
    }

    @Test
    fun negativeNativeCountersAreRejectedInsteadOfDisplayed() {
        val failure = runCatching {
            parseStorageResponse(
                """
                {
                  "status":"ok",
                  "storage":{
                    "total_bytes":-1,
                    "persistent_indexes":{"bytes":0,"files":0,"items":0},
                    "preview_proxy":{"bytes":0,"files":0,"items":0},
                    "disposable":{"bytes":0,"files":0,"items":0},
                    "indexed_sources":0,
                    "preview_proxy_enabled":false
                  }
                }
                """.trimIndent(),
            )
        }

        assertTrue(failure.isFailure)
    }

    @Test
    fun unknownNativeClearScopeIsRejected() {
        val failure = runCatching {
            parseStorageResponse(
                """
                {
                  "status":"ok",
                  "storage":{
                    "total_bytes":0,
                    "persistent_indexes":{"bytes":0,"files":0,"items":0},
                    "preview_proxy":{"bytes":0,"files":0,"items":0},
                    "disposable":{"bytes":0,"files":0,"items":0},
                    "indexed_sources":0,
                    "preview_proxy_enabled":false
                  },
                  "cleared":{
                    "scope":"outside_namespace",
                    "cleared_bytes":0,
                    "cleared_files":0,
                    "cleared_items":0
                  }
                }
                """.trimIndent(),
            )
        }

        assertTrue(failure.isFailure)
    }
}
