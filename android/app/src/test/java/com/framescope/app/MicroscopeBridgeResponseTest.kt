package com.framescope.app

import com.framescope.app.data.NativeMicroscope
import com.framescope.app.data.RustBridge
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MicroscopeBridgeResponseTest {
    @Test
    fun parsesExactFrameIdentityAndSignedTimestamp() {
        val response = RustBridge.parseMicroscopeResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "session":{
                "session_id":7,
                "frame_count":12,
                "current_frame":{
                  "frame_id":4,
                  "timestamp_ticks":-90,
                  "timestamp_us":-1000,
                  "time_base_numerator":1,
                  "time_base_denominator":90000,
                  "duration_ticks":3003,
                  "keyframe":false,
                  "corrupt":false
                },
                "can_step_previous":true,
                "can_step_next":true
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeMicroscope.Success)
        response as NativeMicroscope.Success
        assertEquals(7L, response.session.sessionId)
        assertEquals(12L, response.session.frameCount)
        assertEquals(4L, response.session.currentFrame?.frameId)
        assertEquals(-90L, response.session.currentFrame?.timestampTicks)
        assertEquals(-1000L, response.session.currentFrame?.timestampUs)
        assertEquals(1, response.session.currentFrame?.timeBaseNumerator)
        assertEquals(90000, response.session.currentFrame?.timeBaseDenominator)
        assertEquals(3003L, response.session.currentFrame?.durationTicks)
    }

    @Test
    fun parsesEmptyCompleteIndexWithoutInventingFrameZero() {
        val response = RustBridge.parseMicroscopeResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "session":{
                "session_id":8,
                "frame_count":0,
                "current_frame":null,
                "can_step_previous":false,
                "can_step_next":false
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeMicroscope.Success)
        response as NativeMicroscope.Success
        assertNull(response.session.currentFrame)
        assertNull(response.session.openDiagnostics)
    }

    @Test
    fun parsesIndexingOpenDiagnosticsWithoutChangingFrameState() {
        val response = RustBridge.parseMicroscopeResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "session":{
                "session_id":11,
                "frame_count":100,
                "current_frame":{
                  "frame_id":0,
                  "timestamp_ticks":0,
                  "timestamp_us":0,
                  "time_base_numerator":1,
                  "time_base_denominator":1000,
                  "duration_ticks":40,
                  "keyframe":true,
                  "corrupt":false
                },
                "can_step_previous":false,
                "can_step_next":true,
                "open_diagnostics":{
                  "source_seekable":true,
                  "source_size_bytes":1048576,
                  "source_identity_bytes_read":1048576,
                  "source_identity_read_calls":4,
                  "source_identity_seek_calls":3,
                  "source_identity_io_elapsed_us":1200,
                  "source_identity_elapsed_us":2400,
                  "source_reuse_safe":true,
                  "persistent_index":true,
                  "probe_open_elapsed_us":700,
                  "index_open_elapsed_us":350,
                  "index_open_disposition":"reused",
                  "database_bytes":65536,
                  "wal_bytes":0,
                  "total_open_elapsed_us":9200,
                  "indexing":{
                    "reused_existing_frames":80,
                    "newly_indexed_frames":20,
                    "restarted_after_partial_mismatch":false,
                    "max_pending_entries":20,
                    "total_elapsed_us":5000,
                    "index_status_elapsed_us":20,
                    "decoder_open_elapsed_us":400,
                    "decoder_open_count":1,
                    "frames_decoded":52,
                    "validation_frames_replayed":32,
                    "reconciliation_sqlite_elapsed_us":100,
                    "reconciliation_range_queries":1,
                    "sqlite_batch_elapsed_us":300,
                    "batch_commits":1,
                    "bounded_resume_attempted":true,
                    "bounded_resume_succeeded":true,
                    "bounded_resume_fell_back":false,
                    "resume_checkpoint_frame_id":48,
                    "resume_seek_scan_frames":1
                  }
                }
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeMicroscope.Success)
        response as NativeMicroscope.Success
        val diagnostics = requireNotNull(response.session.openDiagnostics)
        assertEquals(1_048_576L, diagnostics.sourceIdentityBytesRead)
        assertEquals("reused", diagnostics.indexOpenDisposition)
        assertEquals(80L, diagnostics.indexing.reusedExistingFrames)
        assertEquals(20L, diagnostics.indexing.newlyIndexedFrames)
        assertEquals(48L, diagnostics.indexing.resumeCheckpointFrameId)
        assertEquals(0L, response.session.currentFrame?.frameId)
    }

    @Test
    fun rejectsImpossibleBoundedResumeDiagnostics() {
        val response = RustBridge.parseMicroscopeResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "session":{
                "session_id":12,
                "frame_count":0,
                "current_frame":null,
                "can_step_previous":false,
                "can_step_next":false,
                "open_diagnostics":{
                  "source_seekable":false,
                  "source_size_bytes":null,
                  "source_identity_bytes_read":0,
                  "source_identity_read_calls":0,
                  "source_identity_seek_calls":0,
                  "source_identity_io_elapsed_us":0,
                  "source_identity_elapsed_us":10,
                  "source_reuse_safe":false,
                  "persistent_index":false,
                  "probe_open_elapsed_us":20,
                  "index_open_elapsed_us":30,
                  "index_open_disposition":"created",
                  "database_bytes":4096,
                  "wal_bytes":0,
                  "total_open_elapsed_us":100,
                  "indexing":{
                    "reused_existing_frames":0,
                    "newly_indexed_frames":0,
                    "restarted_after_partial_mismatch":false,
                    "max_pending_entries":0,
                    "total_elapsed_us":40,
                    "index_status_elapsed_us":1,
                    "decoder_open_elapsed_us":2,
                    "decoder_open_count":1,
                    "frames_decoded":0,
                    "validation_frames_replayed":0,
                    "reconciliation_sqlite_elapsed_us":0,
                    "reconciliation_range_queries":0,
                    "sqlite_batch_elapsed_us":0,
                    "batch_commits":0,
                    "bounded_resume_attempted":false,
                    "bounded_resume_succeeded":true,
                    "bounded_resume_fell_back":false,
                    "resume_checkpoint_frame_id":null,
                    "resume_seek_scan_frames":0
                  }
                }
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeMicroscope.Failure)
        response as NativeMicroscope.Failure
        assertEquals("malformed_microscope_state", response.code)
    }

    @Test
    fun rejectsInconsistentStepFlags() {
        val response = RustBridge.parseMicroscopeResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "session":{
                "session_id":9,
                "frame_count":3,
                "current_frame":{
                  "frame_id":0,
                  "timestamp_ticks":0,
                  "timestamp_us":0,
                  "time_base_numerator":1,
                  "time_base_denominator":1000,
                  "duration_ticks":40,
                  "keyframe":true,
                  "corrupt":false
                },
                "can_step_previous":true,
                "can_step_next":true
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeMicroscope.Failure)
        response as NativeMicroscope.Failure
        assertEquals("malformed_microscope_state", response.code)
    }

    @Test
    fun rejectsFrameOutsideReportedIndex() {
        val response = RustBridge.parseMicroscopeResponse(
            """
            {
              "status":"ok",
              "engine":"framescope-rust/0.1.0",
              "session":{
                "session_id":10,
                "frame_count":2,
                "current_frame":{
                  "frame_id":2,
                  "timestamp_ticks":80,
                  "timestamp_us":80000,
                  "time_base_numerator":1,
                  "time_base_denominator":1000,
                  "duration_ticks":40,
                  "keyframe":false,
                  "corrupt":false
                },
                "can_step_previous":true,
                "can_step_next":false
              }
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeMicroscope.Failure)
        response as NativeMicroscope.Failure
        assertEquals("malformed_microscope_state", response.code)
    }

    @Test
    fun preservesNativeNavigationErrorCode() {
        val response = RustBridge.parseMicroscopeResponse(
            """
            {
              "status":"error",
              "engine":"framescope-rust/0.1.0",
              "code":"frame_boundary",
              "message":"cannot step beyond the indexed frame range"
            }
            """.trimIndent(),
        )

        assertTrue(response is NativeMicroscope.Failure)
        response as NativeMicroscope.Failure
        assertEquals("frame_boundary", response.code)
    }
}
