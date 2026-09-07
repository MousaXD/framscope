package com.framescope.app.data

import org.json.JSONObject

interface NativeBridge {
    fun version(): Result<String>
    fun inspectVideoFd(fd: Int): NativeInspection
}

object RustBridge : NativeBridge {
    private val loadFailure: Throwable? = runCatching {
        System.loadLibrary("framescope_ffi")
    }.exceptionOrNull()

    @JvmStatic
    private external fun nativeVersion(): String?

    @JvmStatic
    private external fun nativeInspectVideoFd(fd: Int): String?

    override fun version(): Result<String> {
        loadFailure?.let { return Result.failure(it) }
        return runCatching {
            requireNotNull(nativeVersion()) { "Rust engine returned a null version string." }
        }
    }

    override fun inspectVideoFd(fd: Int): NativeInspection {
        loadFailure?.let {
            return NativeInspection.Failure(
                code = "native_library_unavailable",
                message = "Rust engine could not be loaded: ${it.message ?: it::class.java.simpleName}",
                engine = null,
            )
        }

        val raw = runCatching { nativeInspectVideoFd(fd) }.getOrElse {
            return NativeInspection.Failure(
                code = "jni_error",
                message = "Rust engine call failed: ${it.message ?: it::class.java.simpleName}",
                engine = null,
            )
        } ?: return NativeInspection.Failure(
            code = "jni_error",
            message = "Rust engine returned a null inspection response.",
            engine = null,
        )

        return parseResponse(raw)
    }

    internal fun parseResponse(raw: String): NativeInspection = try {
        val json = JSONObject(raw)
        val engine = json.optionalString("engine")
        when (json.optString("status")) {
            "ok" -> parseSuccess(json, engine)
            "error" -> NativeInspection.Failure(
                code = json.optString("code", "rust_error"),
                message = json.optString("message", "Rust inspection failed."),
                engine = engine,
            )

            else -> NativeInspection.Failure(
                code = "malformed_response",
                message = "Rust returned an unrecognized response.",
                engine = engine,
            )
        }
    } catch (error: Exception) {
        NativeInspection.Failure(
            code = "malformed_response",
            message = "Could not decode Rust response: ${error.message ?: error::class.java.simpleName}",
            engine = null,
        )
    }

    private fun parseSuccess(json: JSONObject, engine: String?): NativeInspection {
        val metadataJson = json.getJSONObject("metadata")
        val metadata = VideoMetadata(
            durationUs = metadataJson.optionalLong("duration_us"),
            width = metadataJson.getInt("width"),
            height = metadataJson.getInt("height"),
            estimatedFrameRate = metadataJson.optionalDouble("estimated_frame_rate", "nominal_frame_rate"),
            rotationDegrees = metadataJson.optInt("rotation_degrees", 0),
            container = metadataJson.optionalString("container", "container_name", "container_format"),
            codec = metadataJson.optionalString("codec", "codec_name"),
            videoStreamIndex = metadataJson.optionalInt("video_stream_index", "stream_index"),
            videoStreamCount = metadataJson.optionalInt("video_stream_count"),
            audioStreamCount = metadataJson.optionalInt("audio_stream_count"),
            pixelFormat = metadataJson.optionalString("pixel_format", "pixel_format_name"),
            variableFrameRate = metadataJson.optionalBoolean("variable_frame_rate", "is_variable_frame_rate"),
        )

        return when {
            engine == null -> NativeInspection.Failure(
                code = "malformed_response",
                message = "Rust success response did not identify the engine.",
                engine = null,
            )

            !metadata.isSane() -> NativeInspection.Failure(
                code = "malformed_metadata",
                message = "Rust returned metadata outside expected safety bounds.",
                engine = engine,
            )

            else -> NativeInspection.Success(
                metadata = metadata,
                engine = engine,
            )
        }
    }

    private fun JSONObject.optionalString(vararg keys: String): String? {
        for (key in keys) {
            if (has(key) && !isNull(key)) {
                return getString(key).trim().takeIf { it.isNotEmpty() }
            }
        }
        return null
    }

    private fun JSONObject.optionalLong(key: String): Long? =
        if (!has(key) || isNull(key)) null else getLong(key)

    private fun JSONObject.optionalInt(vararg keys: String): Int? {
        for (key in keys) {
            if (has(key) && !isNull(key)) return getInt(key)
        }
        return null
    }

    private fun JSONObject.optionalDouble(vararg keys: String): Double? {
        for (key in keys) {
            if (has(key) && !isNull(key)) return getDouble(key)
        }
        return null
    }

    private fun JSONObject.optionalBoolean(vararg keys: String): Boolean? {
        for (key in keys) {
            if (has(key) && !isNull(key)) return getBoolean(key)
        }
        return null
    }
}
