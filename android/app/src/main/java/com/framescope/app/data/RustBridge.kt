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

    fun exportCurrentMicroscopeFrame(
        sessionId: Long,
        outputFd: Int,
        operationId: Long,
        format: FrameExportFormat,
        jpegQuality: Int,
    ): NativeFrameExport = NativeFrameExport.Failure(
        code = "not_supported",
        message = "Current-frame export is not supported by this native bridge.",
        engine = null,
    )

    fun exportMicroscopeBatch(
        sessionId: Long,
        manifestFd: Int,
        operationId: Long,
        request: BatchExportRequest,
        sink: NativeBatchFrameSink,
    ): NativeBatchExport = NativeBatchExport.Failure(
        code = "not_supported",
        message = "Batch export is not supported by this native bridge.",
        engine = null,
    )

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
    private external fun nativeExportCurrentMicroscopeFrameFd(
        sessionId: Long,
        outputFd: Int,
        operationId: Long,
        format: Int,
        jpegQuality: Int,
    ): String?

    @JvmStatic
    private external fun nativeExportMicroscopeBatchFd(
        sessionId: Long,
        manifestFd: Int,
        operationId: Long,
        selectionKind: Int,
        start: Long,
        end: Long,
        everyN: Long,
        format: Int,
        jpegQuality: Int,
        sink: NativeBatchFrameSink,
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
    ): NativeMicroscope {
        if (fd < 0 || operationId <= 0L || cacheRoot.isBlank()) {
            return NativeMicroscope.Failure(
                code = "invalid_request",
                message = "Microscope open requires a readable descriptor, positive operation id, and cache root.",
                engine = null,
            )
        }
        return microscopeCall { nativeOpenMicroscopeSession(fd, operationId, cacheRoot) }
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

    override fun exportCurrentMicroscopeFrame(
        sessionId: Long,
        outputFd: Int,
        operationId: Long,
        format: FrameExportFormat,
        jpegQuality: Int,
    ): NativeFrameExport {
        if (sessionId <= 0L || outputFd < 0 || operationId <= 0L) {
            return NativeFrameExport.Failure(
                code = "invalid_request",
                message = "Frame export requires a positive session id, writable descriptor, and operation id.",
                engine = null,
            )
        }
        if (format == FrameExportFormat.Jpeg && jpegQuality !in 1..100) {
            return NativeFrameExport.Failure(
                code = "invalid_request",
                message = "JPEG quality must be between 1 and 100.",
                engine = null,
            )
        }
        return frameExportCall {
            nativeExportCurrentMicroscopeFrameFd(
                sessionId,
                outputFd,
                operationId,
                format.nativeValue,
                jpegQuality,
            )
        }
    }

    override fun exportMicroscopeBatch(
        sessionId: Long,
        manifestFd: Int,
        operationId: Long,
        request: BatchExportRequest,
        sink: NativeBatchFrameSink,
    ): NativeBatchExport {
        if (sessionId <= 0L || manifestFd < 0 || operationId <= 0L || !request.isSane()) {
            return NativeBatchExport.Failure(
                code = "invalid_request",
                message = "Batch export requires a valid session, manifest destination, operation id, selection, and format.",
                engine = null,
            )
        }
        return batchExportCall {
            nativeExportMicroscopeBatchFd(
                sessionId = sessionId,
                manifestFd = manifestFd,
                operationId = operationId,
                selectionKind = request.selection.nativeKind,
                start = request.selection.start,
                end = request.selection.end,
                everyN = request.everyNFrames,
                format = request.format.nativeValue,
                jpegQuality = request.jpegQuality,
                sink = sink,
            )
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

    internal fun parseFrameExportResponse(raw: String): NativeFrameExport = try {
        val json = JSONObject(raw)
        val engine = json.optionalString("engine")
        when (json.optString("status")) {
            "ok" -> parseFrameExportSuccess(json, engine)
            "error" -> NativeFrameExport.Failure(
                code = json.optString("code", "rust_error"),
                message = json.optString("message", "Rust frame export failed."),
                engine = engine,
            )

            else -> NativeFrameExport.Failure(
                code = "malformed_response",
                message = "Rust returned an unrecognized frame export response.",
                engine = engine,
            )
        }
    } catch (error: Exception) {
        NativeFrameExport.Failure(
            code = "malformed_response",
            message = "Could not decode Rust frame export response: ${error.message ?: error::class.java.simpleName}",
            engine = null,
        )
    }

    internal fun parseBatchExportResponse(raw: String): NativeBatchExport = try {
        val json = JSONObject(raw)
        val engine = json.optionalString("engine")
        when (json.optString("status")) {
            "ok" -> parseBatchExportSuccess(json, engine)
            "error" -> NativeBatchExport.Failure(
                code = json.optString("code", "rust_error"),
                message = json.optString("message", "Rust batch export failed."),
                engine = engine,
            )

            else -> NativeBatchExport.Failure(
                code = "malformed_response",
                message = "Rust returned an unrecognized batch export response.",
                engine = engine,
            )
        }
    } catch (error: Exception) {
        NativeBatchExport.Failure(
            code = "malformed_response",
            message = "Could not decode Rust batch export response: ${error.message ?: error::class.java.simpleName}",
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

    private fun frameExportCall(call: () -> String?): NativeFrameExport {
        loadFailure?.let {
            return NativeFrameExport.Failure(
                code = "native_library_unavailable",
                message = "Rust engine could not be loaded: ${it.message ?: it::class.java.simpleName}",
                engine = null,
            )
        }
        val raw = runCatching(call).getOrElse {
            return NativeFrameExport.Failure(
                code = "jni_error",
                message = "Rust frame export call failed: ${it.message ?: it::class.java.simpleName}",
                engine = null,
            )
        } ?: return NativeFrameExport.Failure(
            code = "jni_error",
            message = "Rust engine returned a null frame export response.",
            engine = null,
        )
        return parseFrameExportResponse(raw)
    }

    private fun batchExportCall(call: () -> String?): NativeBatchExport {
        loadFailure?.let {
            return NativeBatchExport.Failure(
                code = "native_library_unavailable",
                message = "Rust engine could not be loaded: ${it.message ?: it::class.java.simpleName}",
                engine = null,
            )
        }
        val raw = runCatching(call).getOrElse {
            return NativeBatchExport.Failure(
                code = "jni_error",
                message = "Rust batch export call failed: ${it.message ?: it::class.java.simpleName}",
                engine = null,
            )
        } ?: return NativeBatchExport.Failure(
            code = "jni_error",
            message = "Rust engine returned a null batch export response.",
            engine = null,
        )
        return parseBatchExportResponse(raw)
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
        val diagnostics = sessionJson.optJSONObject("open_diagnostics")?.let(::parseMicroscopeOpenDiagnostics)
        val session = MicroscopeSessionSnapshot(
            sessionId = sessionJson.getLong("session_id"),
            frameCount = sessionJson.getLong("frame_count"),
            currentFrame = frame,
            canStepPrevious = sessionJson.getBoolean("can_step_previous"),
            canStepNext = sessionJson.getBoolean("can_step_next"),
            openDiagnostics = diagnostics,
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

    private fun parseMicroscopeOpenDiagnostics(json: JSONObject): MicroscopeOpenDiagnostics {
        val indexingJson = json.getJSONObject("indexing")
        val indexing = IndexingRuntimeDiagnostics(
            reusedExistingFrames = indexingJson.getLong("reused_existing_frames"),
            newlyIndexedFrames = indexingJson.getLong("newly_indexed_frames"),
            restartedAfterPartialMismatch = indexingJson.getBoolean("restarted_after_partial_mismatch"),
            maxPendingEntries = indexingJson.getLong("max_pending_entries"),
            totalElapsedUs = indexingJson.getLong("total_elapsed_us"),
            indexStatusElapsedUs = indexingJson.getLong("index_status_elapsed_us"),
            decoderOpenElapsedUs = indexingJson.getLong("decoder_open_elapsed_us"),
            decoderOpenCount = indexingJson.getLong("decoder_open_count"),
            framesDecoded = indexingJson.getLong("frames_decoded"),
            validationFramesReplayed = indexingJson.getLong("validation_frames_replayed"),
            reconciliationSqliteElapsedUs = indexingJson.getLong("reconciliation_sqlite_elapsed_us"),
            reconciliationRangeQueries = indexingJson.getLong("reconciliation_range_queries"),
            sqliteBatchElapsedUs = indexingJson.getLong("sqlite_batch_elapsed_us"),
            batchCommits = indexingJson.getLong("batch_commits"),
            boundedResumeAttempted = indexingJson.getBoolean("bounded_resume_attempted"),
            boundedResumeSucceeded = indexingJson.getBoolean("bounded_resume_succeeded"),
            boundedResumeFellBack = indexingJson.getBoolean("bounded_resume_fell_back"),
            resumeCheckpointFrameId = indexingJson.optionalLong("resume_checkpoint_frame_id"),
            resumeSeekScanFrames = indexingJson.getLong("resume_seek_scan_frames"),
        )
        return MicroscopeOpenDiagnostics(
            sourceSeekable = json.getBoolean("source_seekable"),
            sourceSizeBytes = json.optionalLong("source_size_bytes"),
            sourceIdentityBytesRead = json.getLong("source_identity_bytes_read"),
            sourceIdentityReadCalls = json.getLong("source_identity_read_calls"),
            sourceIdentitySeekCalls = json.getLong("source_identity_seek_calls"),
            sourceIdentityIoElapsedUs = json.getLong("source_identity_io_elapsed_us"),
            sourceIdentityElapsedUs = json.getLong("source_identity_elapsed_us"),
            sourceReuseSafe = json.getBoolean("source_reuse_safe"),
            persistentIndex = json.getBoolean("persistent_index"),
            probeOpenElapsedUs = json.getLong("probe_open_elapsed_us"),
            indexOpenElapsedUs = json.getLong("index_open_elapsed_us"),
            indexOpenDisposition = json.getString("index_open_disposition"),
            databaseBytes = json.getLong("database_bytes"),
            walBytes = json.getLong("wal_bytes"),
            totalOpenElapsedUs = json.getLong("total_open_elapsed_us"),
            indexing = indexing,
        )
    }

    private fun parseFrameExportSuccess(json: JSONObject, engine: String?): NativeFrameExport {
        if (engine == null) {
            return NativeFrameExport.Failure(
                code = "malformed_response",
                message = "Rust frame export success response did not identify the engine.",
                engine = null,
            )
        }
        val exportJson = json.getJSONObject("export")
        val wireFormat = exportJson.getString("format")
        val format = FrameExportFormat.fromWireName(wireFormat)
            ?: return NativeFrameExport.Failure(
                code = "malformed_export_state",
                message = "Rust returned an unknown frame export format.",
                engine = engine,
            )
        val export = FrameExportResult(
            sessionId = exportJson.getLong("session_id"),
            frameId = exportJson.getLong("frame_id"),
            width = exportJson.getInt("width"),
            height = exportJson.getInt("height"),
            format = format,
            mimeType = exportJson.getString("mime_type"),
            byteLength = exportJson.getLong("byte_len"),
        )
        return if (export.isSane()) {
            NativeFrameExport.Success(export = export, engine = engine)
        } else {
            NativeFrameExport.Failure(
                code = "malformed_export_state",
                message = "Rust returned frame export metadata outside expected safety bounds.",
                engine = engine,
            )
        }
    }

    private fun parseBatchExportSuccess(json: JSONObject, engine: String?): NativeBatchExport {
        if (engine == null) {
            return NativeBatchExport.Failure(
                code = "malformed_response",
                message = "Rust batch export success response did not identify the engine.",
                engine = null,
            )
        }
        val exportJson = json.getJSONObject("export")
        val format = FrameExportFormat.fromWireName(exportJson.getString("format"))
            ?: return NativeBatchExport.Failure(
                code = "malformed_export_state",
                message = "Rust returned an unknown batch export format.",
                engine = engine,
            )
        val export = BatchExportResult(
            sessionId = exportJson.getLong("session_id"),
            expectedFrames = exportJson.getLong("expected_frames"),
            committedFrames = exportJson.getLong("committed_frames"),
            encodedBytes = exportJson.getLong("encoded_bytes"),
            decodedFrames = exportJson.getLong("decoded_frames"),
            usedKeyframeSeek = exportJson.getBoolean("used_keyframe_seek"),
            fellBackToStreamStart = exportJson.getBoolean("fell_back_to_stream_start"),
            format = format,
            mimeType = exportJson.getString("mime_type"),
        )
        return if (export.isSane()) {
            NativeBatchExport.Success(export = export, engine = engine)
        } else {
            NativeBatchExport.Failure(
                code = "malformed_export_state",
                message = "Rust returned batch export metadata outside expected safety bounds.",
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
