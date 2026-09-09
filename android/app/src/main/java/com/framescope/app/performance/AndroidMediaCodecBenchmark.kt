package com.framescope.app.performance

import android.content.Context
import android.media.MediaCodec
import android.media.MediaExtractor
import android.media.MediaFormat
import android.os.Build
import android.os.PowerManager
import android.os.Process
import android.os.SystemClock
import java.io.FileDescriptor

internal data class MediaCodecDecodeBenchmarkResult(
    val codecName: String,
    val mimeType: String,
    val framesDecoded: Long,
    val wallTimeMs: Double,
    val processCpuTimeMs: Long,
    val timeToFirstFrameMs: Double?,
    val framesPerSecond: Double,
    val firstPresentationTimeUs: Long?,
    val lastPresentationTimeUs: Long?,
    val presentationTimestampDigest: String,
    val outputColorFormat: Int?,
    val thermalStatusBefore: Int?,
    val thermalStatusAfter: Int?,
    val cancelled: Boolean,
)

/**
 * Physical-device benchmark candidate for Android's direct MediaCodec API.
 *
 * This code is intentionally not wired into FrameScope's authoritative indexer or navigation path.
 * MediaCodec exposes presentation timestamps, but it does not expose all of the frame metadata that
 * currently contributes to FrameScope's persistent FrameIndexEntry contract. Device equivalence
 * evidence must exist before a production integration can be considered.
 */
internal object AndroidMediaCodecBenchmark {
    private const val CODEC_TIMEOUT_US = 10_000L

    fun decodeByteBuffer(
        context: Context,
        fileDescriptor: FileDescriptor,
        maxFrames: Long? = null,
        isCancelled: () -> Boolean = { false },
    ): MediaCodecDecodeBenchmarkResult {
        require(maxFrames == null || maxFrames > 0) { "maxFrames must be positive when provided" }

        val extractor = MediaExtractor()
        var codec: MediaCodec? = null
        val thermalBefore = currentThermalStatus(context)
        val wallStartNs = SystemClock.elapsedRealtimeNanos()
        val processCpuStartMs = Process.getElapsedCpuTime()

        try {
            extractor.setDataSource(fileDescriptor)
            val trackIndex = firstVideoTrack(extractor)
            require(trackIndex >= 0) { "source contains no video track" }
            extractor.selectTrack(trackIndex)
            val format = extractor.getTrackFormat(trackIndex)
            val mimeType = requireNotNull(format.getString(MediaFormat.KEY_MIME)) {
                "selected video track has no MIME type"
            }.lowercase()
            val capability = AndroidHardwareDecodeCapabilities.preferredHardwareDecoder(
                mimeType = mimeType,
                requireByteBufferOutput = true,
            ) ?: error("no explicitly hardware-accelerated ByteBuffer decoder is available for $mimeType")

            codec = MediaCodec.createByCodecName(capability.codecName)
            codec.configure(format, null, null, 0)
            codec.start()

            val bufferInfo = MediaCodec.BufferInfo()
            var inputEnded = false
            var outputEnded = false
            var framesDecoded = 0L
            var firstPresentationTimeUs: Long? = null
            var lastPresentationTimeUs: Long? = null
            var firstFrameNs: Long? = null
            var outputColorFormat: Int? = null
            var timestampDigest = Fnv1a64()
            var cancelled = false

            while (!outputEnded) {
                if (isCancelled()) {
                    cancelled = true
                    break
                }
                if (maxFrames != null && framesDecoded >= maxFrames) {
                    break
                }

                if (!inputEnded) {
                    val inputIndex = codec.dequeueInputBuffer(CODEC_TIMEOUT_US)
                    if (inputIndex >= 0) {
                        val inputBuffer = requireNotNull(codec.getInputBuffer(inputIndex)) {
                            "MediaCodec returned a null input buffer"
                        }
                        inputBuffer.clear()
                        val sampleSize = extractor.readSampleData(inputBuffer, 0)
                        if (sampleSize < 0) {
                            codec.queueInputBuffer(
                                inputIndex,
                                0,
                                0,
                                0L,
                                MediaCodec.BUFFER_FLAG_END_OF_STREAM,
                            )
                            inputEnded = true
                        } else {
                            val sampleTimeUs = extractor.sampleTime
                            val inputFlags = if (
                                extractor.sampleFlags and MediaExtractor.SAMPLE_FLAG_SYNC != 0
                            ) {
                                MediaCodec.BUFFER_FLAG_KEY_FRAME
                            } else {
                                0
                            }
                            codec.queueInputBuffer(inputIndex, 0, sampleSize, sampleTimeUs, inputFlags)
                            extractor.advance()
                        }
                    }
                }

                when (val outputIndex = codec.dequeueOutputBuffer(bufferInfo, CODEC_TIMEOUT_US)) {
                    MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                        val outputFormat = codec.outputFormat
                        outputColorFormat = if (outputFormat.containsKey(MediaFormat.KEY_COLOR_FORMAT)) {
                            outputFormat.getInteger(MediaFormat.KEY_COLOR_FORMAT)
                        } else {
                            null
                        }
                    }
                    MediaCodec.INFO_TRY_AGAIN_LATER,
                    MediaCodec.INFO_OUTPUT_BUFFERS_CHANGED,
                    -> Unit
                    else -> if (outputIndex >= 0) {
                        val isCodecConfig = bufferInfo.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0
                        val isEndOfStream = bufferInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0
                        if (!isCodecConfig && bufferInfo.size > 0) {
                            val ptsUs = bufferInfo.presentationTimeUs
                            if (firstFrameNs == null) {
                                firstFrameNs = SystemClock.elapsedRealtimeNanos()
                            }
                            if (firstPresentationTimeUs == null) {
                                firstPresentationTimeUs = ptsUs
                            }
                            lastPresentationTimeUs = ptsUs
                            timestampDigest.update(ptsUs)
                            framesDecoded += 1
                        }
                        codec.releaseOutputBuffer(outputIndex, false)
                        outputEnded = isEndOfStream
                    }
                }
            }

            val wallEndNs = SystemClock.elapsedRealtimeNanos()
            val wallTimeMs = nanosToMillis(wallEndNs - wallStartNs)
            val framesPerSecond = if (wallTimeMs > 0.0) {
                framesDecoded * 1_000.0 / wallTimeMs
            } else {
                0.0
            }
            return MediaCodecDecodeBenchmarkResult(
                codecName = capability.codecName,
                mimeType = mimeType,
                framesDecoded = framesDecoded,
                wallTimeMs = wallTimeMs,
                processCpuTimeMs = Process.getElapsedCpuTime() - processCpuStartMs,
                timeToFirstFrameMs = firstFrameNs?.let { nanosToMillis(it - wallStartNs) },
                framesPerSecond = framesPerSecond,
                firstPresentationTimeUs = firstPresentationTimeUs,
                lastPresentationTimeUs = lastPresentationTimeUs,
                presentationTimestampDigest = timestampDigest.hex(),
                outputColorFormat = outputColorFormat,
                thermalStatusBefore = thermalBefore,
                thermalStatusAfter = currentThermalStatus(context),
                cancelled = cancelled,
            )
        } finally {
            runCatching { codec?.stop() }
            runCatching { codec?.release() }
            extractor.release()
        }
    }

    private fun firstVideoTrack(extractor: MediaExtractor): Int {
        for (index in 0 until extractor.trackCount) {
            val mime = extractor.getTrackFormat(index).getString(MediaFormat.KEY_MIME)
            if (mime?.startsWith("video/") == true) {
                return index
            }
        }
        return -1
    }

    private fun currentThermalStatus(context: Context): Int? {
        if (Build.VERSION.SDK_INT < 29) {
            return null
        }
        return context.getSystemService(PowerManager::class.java)?.currentThermalStatus
    }

    private fun nanosToMillis(nanos: Long): Double = nanos / 1_000_000.0

    private class Fnv1a64 {
        private var value: Long = -3750763034362895579L

        fun update(input: Long) {
            var remaining = input
            repeat(Long.SIZE_BYTES) {
                value = value xor (remaining and 0xffL)
                value *= 1_099_511_628_211L
                remaining = remaining ushr 8
            }
        }

        fun hex(): String = java.lang.Long.toUnsignedString(value, 16).padStart(16, '0')
    }
}
