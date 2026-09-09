package com.framescope.app.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MicroscopeFrameBridgeTest {
    @Test
    fun parsesExactNavigationCountersForDeviceBenchmarks() {
        val parsed = MicroscopeFrameBridge.parsePreparationResponse(
            raw = responseWithNavigation(
                requestedExactFrames = 4,
                randomSeeks = 1,
                forwardDecodes = 3,
                decoderReopenCount = 2,
                decodedFrames = 10,
                warmNavigationHits = 2,
                ramNavigationHits = 1,
                lastSeekDistanceFrames = 0,
                totalSeekDistanceFrames = 7,
                lastFramesDecoded = 1,
                lastDecoderReopens = 0,
                lastRandomSeek = false,
                lastWarmNavigationHit = true,
            ),
        )

        val success = parsed as NativeFramePreparation.Success
        val navigation = requireNotNull(success.frame.navigation)
        assertEquals(4L, navigation.requestedExactFrames)
        assertEquals(1L, navigation.randomSeeks)
        assertEquals(3L, navigation.forwardDecodes)
        assertEquals(2L, navigation.decoderReopenCount)
        assertEquals(10L, navigation.decodedFrames)
        assertEquals(2L, navigation.warmNavigationHits)
        assertEquals(1L, navigation.ramNavigationHits)
        assertEquals(7L, navigation.totalSeekDistanceFrames)
        assertEquals(1L, navigation.lastFramesDecoded)
        assertEquals(0L, navigation.lastDecoderReopens)
        assertFalse(navigation.lastRandomSeek)
        assertTrue(navigation.lastWarmNavigationHit)
        assertEquals(2.5, navigation.framesDecodedPerExactFrame()!!, 0.0)
    }

    @Test
    fun malformedOptionalDiagnosticsDoNotInvalidateAuthoritativeFrameDescriptor() {
        val raw = responseWithNavigation(
            requestedExactFrames = -1,
            randomSeeks = 0,
            forwardDecodes = 0,
            decoderReopenCount = 0,
            decodedFrames = 0,
            warmNavigationHits = 0,
            ramNavigationHits = 0,
            lastSeekDistanceFrames = 0,
            totalSeekDistanceFrames = 0,
            lastFramesDecoded = 0,
            lastDecoderReopens = 0,
            lastRandomSeek = false,
            lastWarmNavigationHit = false,
        )

        val parsed = MicroscopeFrameBridge.parsePreparationResponse(raw)
        val success = parsed as NativeFramePreparation.Success
        assertNull(success.frame.navigation)
        assertTrue(success.frame.isSane())
    }

    private fun responseWithNavigation(
        requestedExactFrames: Long,
        randomSeeks: Long,
        forwardDecodes: Long,
        decoderReopenCount: Long,
        decodedFrames: Long,
        warmNavigationHits: Long,
        ramNavigationHits: Long,
        lastSeekDistanceFrames: Long,
        totalSeekDistanceFrames: Long,
        lastFramesDecoded: Long,
        lastDecoderReopens: Long,
        lastRandomSeek: Boolean,
        lastWarmNavigationHit: Boolean,
    ): String =
        """
        {
          "status":"ok",
          "engine":"framescope-rust/test",
          "frame":{
            "session_id":7,
            "frame_id":11,
            "generation":3,
            "width":2,
            "height":2,
            "stride_bytes":8,
            "byte_len":16,
            "navigation":{
              "requested_exact_frames":$requestedExactFrames,
              "random_seeks":$randomSeeks,
              "forward_decodes":$forwardDecodes,
              "decoder_reopen_count":$decoderReopenCount,
              "decoded_frames":$decodedFrames,
              "warm_navigation_hits":$warmNavigationHits,
              "ram_navigation_hits":$ramNavigationHits,
              "last_seek_distance_frames":$lastSeekDistanceFrames,
              "total_seek_distance_frames":$totalSeekDistanceFrames,
              "last_frames_decoded":$lastFramesDecoded,
              "last_decoder_reopens":$lastDecoderReopens,
              "last_random_seek":$lastRandomSeek,
              "last_warm_navigation_hit":$lastWarmNavigationHit
            }
          }
        }
        """.trimIndent()
}
