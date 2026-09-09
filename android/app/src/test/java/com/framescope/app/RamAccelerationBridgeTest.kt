package com.framescope.app.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class RamAccelerationBridgeTest {
    @Test
    fun parsesNativeResidentAndEffectivenessCounters() {
        val metrics = RamAccelerationBridge.parseStats(
            """
            {
              "status":"ok",
              "engine":"test",
              "ram":{
                "source":{
                  "budget_bytes":314572800,
                  "resident_bytes":16777216,
                  "resident_frames":2,
                  "hits":11,
                  "misses":5,
                  "insertions":4,
                  "evictions":2
                },
                "preview":{
                  "budget_bytes":734003200,
                  "resident_bytes":8388608,
                  "resident_frames":9,
                  "hits":31,
                  "misses":7,
                  "insertions":12,
                  "evictions":3
                },
                "retained_preview_sessions":1
              }
            }
            """.trimIndent(),
        )

        requireNotNull(metrics)
        assertEquals(25_165_824L, metrics.residentBytes)
        assertEquals(42L, metrics.hits)
        assertEquals(12L, metrics.misses)
        assertEquals(2L, metrics.source.residentFrames)
        assertEquals(9L, metrics.preview.residentFrames)
        assertEquals(1, metrics.retainedPreviewSessions)
    }

    @Test
    fun rejectsNegativeOrErrorStatsInsteadOfDisplayingInventedResidency() {
        val negative = """
            {
              "status":"ok",
              "ram":{
                "source":{"budget_bytes":1,"resident_bytes":-1,"resident_frames":0,"hits":0,"misses":0,"insertions":0,"evictions":0},
                "preview":{"budget_bytes":1,"resident_bytes":0,"resident_frames":0,"hits":0,"misses":0,"insertions":0,"evictions":0},
                "retained_preview_sessions":0
              }
            }
        """.trimIndent()

        assertNull(RamAccelerationBridge.parseStats(negative))
        assertNull(RamAccelerationBridge.parseStats("{\"status\":\"error\"}"))
    }
}
