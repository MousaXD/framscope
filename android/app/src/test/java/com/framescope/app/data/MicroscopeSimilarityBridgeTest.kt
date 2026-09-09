package com.framescope.app.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class MicroscopeSimilarityBridgeTest {
    @Test
    fun parsesStrictlyOrderedBoundedSimilarityResults() {
        val parsed = parseSimilarityResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/test",
              "result":{
                "session_id":7,
                "target_frame_id":4,
                "descriptor_count":12,
                "candidate_count":5,
                "matched_count":2,
                "truncated":false,
                "minimum_similarity":9300,
                "disposition":"reused",
                "matches":[
                  {"frame_id":9,"similarity":9800},
                  {"frame_id":2,"similarity":9400}
                ]
              }
            }
            """.trimIndent(),
        )

        assertTrue(parsed is NativeMicroscopeSimilarity.Success)
        val success = parsed as NativeMicroscopeSimilarity.Success
        assertEquals(7L, success.result.sessionId)
        assertEquals(4L, success.result.targetFrameId)
        assertEquals(SimilarityStoreDisposition.Reused, success.result.disposition)
        assertEquals(listOf(9L, 2L), success.result.matches.map { it.frameId })
    }

    @Test
    fun rejectsResultThatIncludesTargetFrameAsMatch() {
        val parsed = parseSimilarityResponse(
            successPayload(
                matches = "[{\"frame_id\":4,\"similarity\":9800}]",
                matchedCount = 1,
            ),
        )

        assertMalformed(parsed)
    }

    @Test
    fun rejectsNonDeterministicMatchOrdering() {
        val parsed = parseSimilarityResponse(
            successPayload(
                matches = "[" +
                    "{\"frame_id\":2,\"similarity\":9400}," +
                    "{\"frame_id\":9,\"similarity\":9800}" +
                    "]",
                matchedCount = 2,
            ),
        )

        assertMalformed(parsed)
    }

    @Test
    fun truncatedResultMustActuallyOmitConfirmedMatches() {
        val parsed = parseSimilarityResponse(
            successPayload(
                matches = "[{\"frame_id\":9,\"similarity\":9800}]",
                matchedCount = 1,
                truncated = true,
            ),
        )

        assertMalformed(parsed)
    }

    private fun successPayload(
        matches: String,
        matchedCount: Int,
        truncated: Boolean = false,
    ): String =
        """
        {
          "status":"ok",
          "engine":"framescope-rust/test",
          "result":{
            "session_id":7,
            "target_frame_id":4,
            "descriptor_count":12,
            "candidate_count":5,
            "matched_count":$matchedCount,
            "truncated":$truncated,
            "minimum_similarity":9300,
            "disposition":"built",
            "matches":$matches
          }
        }
        """.trimIndent()

    private fun assertMalformed(parsed: NativeMicroscopeSimilarity) {
        assertTrue(parsed is NativeMicroscopeSimilarity.Failure)
        assertEquals(
            "malformed_similarity_state",
            (parsed as NativeMicroscopeSimilarity.Failure).code,
        )
    }
}
