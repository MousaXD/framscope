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
        val engine = json.optString("engine").takeIf { it.isNotBlank() }
        when (json.optString("status")) {
            "ok" -> {
                val metadataJson = json.getJSONObject("metadata")
                val fps = if (metadataJson.isNull("estimated_frame_rate")) {
                    null
                } else {
                    metadataJson.getDouble("estimated_frame_rate")
                }
                val metadata = VideoMetadata(
                    durationUs = metadataJson.getLong("duration_us"),
                    width = metadataJson.getInt("width"),
                    height = metadataJson.getInt("height"),
                    estimatedFrameRate = fps,
                    rotationDegrees = metadataJson.getInt("rotation_degrees"),
                )
                when {
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
}
