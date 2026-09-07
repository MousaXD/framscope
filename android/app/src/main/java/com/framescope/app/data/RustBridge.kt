package com.framescope.app.data

import org.json.JSONObject

interface NativeBridge {
    fun version(): Result<String>
    fun inspectVideoFd(fd: Int, operationId: Long): NativeInspection
    fun cancelInspection(operationId: Long): Boolean = false

    fun openMicroscopeSession(
        fd: Int,
        operationId: Long,
        cacheRoot: String,
    ): NativeMicroscope = unsupportedMicroscope()

    fun stepMicroscope(sessionId: Long, delta: Int): NativeMicroscope = unsupportedMicroscope()

    fun jumpMicroscopeFrame(sessionId: Long, frameId: Long): NativeMicroscope = unsupportedMicroscope()

    fun jumpMicroscopeTimestampUs(
        sessionId: Long,
        timestampUs: Long,
        selection: TimestampSelectionPolicy,
    ): NativeMicroscope = unsupportedMicroscope()

    fun closeMicroscopeSession(sessionId: Long): Boolean = false

    private fun unsupportedMicroscope(): NativeMicroscope = NativeMicroscope.Failure(
        code = "not_supported",
        message = "Microscope navigation is not supported by this native bridge.",
        engine = null,
    )
}

object RustBridge : NativeBridge {
    private val loadFailure: Throwable? = runCatching {
        System.loadLibrary("framescope_ffi")
    }.exceptionOrNull()

    @JvmStatic
    private external fun nativeVersion(): String?

    @JvmStatic
    private external fun nativeInspectVideoFd(fd: Int, operationId: Long): String?

    @JvmStatic
    private external fun nativeOpenMicroscopeSession(
        fd: Int,
        operationId: Long,
        cacheRoot: String,
    ): String?

    @JvmStatic
    private external fun nativeStepMicroscope(sessionId: Long, delta: Int): String?

    @JvmStatic
    private external fun nativeJumpMicroscopeFrame(sessionId: Long, frameId: Long): String?

    @JvmStatic
    private external fun nativeJumpMicroscopeTimestampUs(
        sessionId: Long,
        timestampUs: Long,
        selection: Int,
    ): String?

    @JvmStatic
    private external fun nativeCloseMicroscopeSession(sessionId: Long): Boolean

    @JvmStatic
    private external fun nativeCancelInspection(operationId: Long): Boolean

    override fun version(): Result<String> {
        loadFailure?.let { return Result.failure(it) }
        return runCatching {
            requireNotNull(nativeVersion()) { "Rust engine returned a null version string." }
        }
    }

    override fun inspectVideoFd(fd: Int, operationId: Long): NativeInspection {
        loadFailure?.let {
            return NativeInspection.Failure(
                code = "native_library_unavailable",
                message = "Rust engine could not be loaded: ${it.message ?: it::class.java.simpleName}",
                engine = null,
            )
        }

        val raw = runCatching { nativeInspectVideoFd(fd, operationId) }.getOrElse {
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

    override fun openMicroscopeSession(
        fd: Int,
        operationId: Long,
        cacheRoot: String,
    ): NativeMicroscope = microscopeCall {
        nativeOpenMicroscopeSession(fd, operationId, cacheRoot)
    }

    override fun stepMicroscope(sessionId: Long, delta: Int): NativeMicroscope {
        if (sessionId <= 0L || delta !in setOf(-1, 1)) {
            return NativeMicroscope.Failure(
                code = "invalid_request",
                message = "Microscope step requires a positive session id and delta -1 or +1.",
                engine = null,
            )
        }
        return microscopeCall { nativeStepMicroscope(sessionId, delta) }
    }

    override fun jumpMicroscopeFrame(sessionId: Long, frameId: Long): NativeMicroscope {
        if (sessionId <= 0L || frameId < 0L) {
            return NativeMicroscope.Failure(
                code = "invalid_request",
                message = "Microscope frame jump requires a positive session id and non-negative frame id.",
                engine = null,
            )
        }
        return microscopeCall { nativeJumpMicroscopeFrame(sessionId, frameId) }
    }

    override fun jumpMicroscopeTimestampUs(
        sessionId: Long,
        timestampUs: Long,
        selection: TimestampSelectionPolicy,
    ): NativeMicroscope {
        if (sessionId <= 0L) {
            return NativeMicroscope.Failure(
                code = "invalid_request",
                message = "Microscope timestamp jump requires a positive session id.",
                engine = null,
            )
        }
        return microscopeCall {
            nativeJumpMicroscopeTimestampUs(sessionId, timestampUs, selection.nativeValue)
        }
    }

    override fun closeMicroscopeSession(sessionId: Long): Boolean {
        if (sessionId <= 0L || loadFailure != null) return false
        return runCatching { nativeCloseMicroscopeSession(sessionId) }.getOrDefault(false)
    }

    override fun cancelInspection(operationId: Long): Boolean {
        if (operationId <= 0L || loadFailure != null) return false
        return runCatching { nativeCancelInspection(operationId) }.getOrDefault(false)
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

    internal fun parseMicroscopeResponse(raw: String): NativeMicroscope = try {
        val json = JSONObject(raw)
        val engine = json.optionalString("engine")
        when (json.optString("status")) {
            "ok" -> parseMicroscopeSuccess(json, engine)
            "error" -> NativeMicroscope.Failure(
                code = json.optString("code", "rust_error"),
                message = json.optString("message", "Rust microscope operation failed."),
                engine = engine,
            )

            else -> NativeMicroscope.Failure(
                code = "malformed_response",
                message = "Rust returned an unrecognized microscope response.",
                engine = engine,
            )
        }
    } catch (error: Exception) {
        NativeMicroscope.Failure(
            code = "malformed_response",
            message = "Could not decode Rust microscope response: ${error.message ?: error::class.java.simpleName}",
            engine = null,
        )
    }

    private fun microscopeCall(call: () -> String?): NativeMicroscope {
        loadFailure?.let {
            return NativeMicroscope.Failure(
                code = "native_library_unavailable",
                message = "Rust engine could not be loaded: ${it.message ?: it::class.java.simpleName}",
                engine = null,
            )
        }
        val raw = runCatching(call).getOrElse {
            return NativeMicroscope.Failure(
                code = "jni_error",
                message = "Rust microscope call failed: ${it.message ?: it::class.java.simpleName}",
                engine = null,
            )
        } ?: return NativeMicroscope.Failure(
            code = "jni_error",
            message = "Rust engine returned a null microscope response.",
            engine = null,
        )
        return parseMicroscopeResponse(raw)
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

    private fun parseMicroscopeSuccess(json: JSONObject, engine: String?): NativeMicroscope {
        if (engine == null) {
            return NativeMicroscope.Failure(
                code = "malformed_response",
                message = "Rust microscope success response did not identify the engine.",
                engine = null,
            )
        }
        val sessionJson = json.getJSONObject("session")
        val frameJson = sessionJson.optJSONObject("current_frame")
        val frame = frameJson?.let {
            FrameDetails(
                frameId = it.getLong("frame_id"),
                timestampTicks = it.optionalLong("timestamp_ticks"),
                timestampUs = it.optionalLong("timestamp_us"),
                timeBaseNumerator = it.getInt("time_base_numerator"),
                timeBaseDenominator = it.getInt("time_base_denominator"),
                durationTicks = it.optionalLong("duration_ticks"),
                keyframe = it.getBoolean("keyframe"),
                corrupt = it.getBoolean("corrupt"),
            )
        }
        val session = MicroscopeSessionSnapshot(
            sessionId = sessionJson.getLong("session_id"),
            frameCount = sessionJson.getLong("frame_count"),
            currentFrame = frame,
            canStepPrevious = sessionJson.getBoolean("can_step_previous"),
            canStepNext = sessionJson.getBoolean("can_step_next"),
        )
        return if (session.isSane()) {
            NativeMicroscope.Success(session = session, engine = engine)
        } else {
            NativeMicroscope.Failure(
                code = "malformed_microscope_state",
                message = "Rust returned microscope state outside expected safety bounds.",
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
