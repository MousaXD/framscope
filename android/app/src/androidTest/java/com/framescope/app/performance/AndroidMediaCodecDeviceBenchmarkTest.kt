package com.framescope.app.performance

import android.os.ParcelFileDescriptor
import android.util.Log
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import java.io.File

/**
 * Opt-in physical-device benchmark. CI compiles this test but does not fabricate performance data.
 *
 * Run with:
 *   -e framescope.hwdecode.video /sdcard/Movies/fixture.mp4
 *   -e framescope.hwdecode.maxFrames 600
 *   -e framescope.hwdecode.seekTargetUs 10000000
 */
class AndroidMediaCodecDeviceBenchmarkTest {
    @Test
    fun logRuntimeCapabilityMatrix() {
        val capabilities = AndroidHardwareDecodeCapabilities.discover()
        val rows = JSONArray()
        capabilities.forEach { capability ->
            rows.put(
                JSONObject()
                    .put("codec", capability.codecName)
                    .put("mime", capability.mimeType)
                    .put("acceleration", capability.acceleration.name)
                    .put("vendor", capability.vendor)
                    .put("alias", capability.alias)
                    .put("surface", capability.supportsSurfaceOutput)
                    .put("byte_buffer", capability.supportsByteBufferOutput)
                    .put("flexible_yuv420", capability.supportsFlexibleYuv420),
            )
        }
        Log.i(
            "FrameScopeHwDecode",
            JSONObject()
                .put("event", "codec_capability_matrix")
                .put("codecs", rows)
                .toString(),
        )
    }

    @Test
    fun benchmarkExplicitHardwareByteBufferDecoderWhenFixtureProvided() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val arguments = InstrumentationRegistry.getArguments()
        val videoPath = arguments.getString("framescope.hwdecode.video")
        assumeTrue("physical benchmark fixture was not provided", !videoPath.isNullOrBlank())

        val file = File(requireNotNull(videoPath))
        assumeTrue("benchmark fixture does not exist: $file", file.isFile)
        val maxFrames = arguments.getString("framescope.hwdecode.maxFrames")
            ?.toLongOrNull()
            ?.takeIf { it > 0 }
        val seekTargetUs = arguments.getString("framescope.hwdecode.seekTargetUs")
            ?.toLongOrNull()
            ?.takeIf { it >= 0 }

        ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY).use { descriptor ->
            val result = AndroidMediaCodecBenchmark.decodeByteBuffer(
                context = instrumentation.targetContext,
                fileDescriptor = descriptor.fileDescriptor,
                maxFrames = maxFrames,
                seekTargetUs = seekTargetUs,
            )
            assertTrue("MediaCodec benchmark produced no frames", result.framesDecoded > 0)
            Log.i(
                "FrameScopeHwDecode",
                JSONObject()
                    .put("event", "decode_benchmark")
                    .put("backend", "android-mediacodec")
                    .put("codec", result.codecName)
                    .put("mime", result.mimeType)
                    .put("frames", result.framesDecoded)
                    .put("wall_ms", result.wallTimeMs)
                    .put("process_cpu_ms", result.processCpuTimeMs)
                    .put("ttff_ms", result.timeToFirstFrameMs)
                    .put("seek_target_us", seekTargetUs)
                    .put("seek_settle_ms", result.seekSettleMs)
                    .put("fps", result.framesPerSecond)
                    .put("first_pts_us", result.firstPresentationTimeUs)
                    .put("last_pts_us", result.lastPresentationTimeUs)
                    .put("pts_digest", result.presentationTimestampDigest)
                    .put("output_color_format", result.outputColorFormat)
                    .put("thermal_before", result.thermalStatusBefore)
                    .put("thermal_after", result.thermalStatusAfter)
                    .put("cancelled", result.cancelled)
                    .toString(),
            )
        }
    }
}
