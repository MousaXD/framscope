package com.framescope.app.data

import android.content.ContentResolver
import android.database.Cursor
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Log
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext

interface FrameScopeRepository {
    suspend fun engineVersion(): Result<String>

    suspend fun inspect(
        uri: String,
        onProgress: (InspectionProgress) -> Unit = {},
    ): Result<InspectedVideo>

    suspend fun openMicroscope(uri: String): Result<MicroscopeSessionSnapshot> =
        Result.failure(UnsupportedOperationException("Microscope sessions are not supported."))

    suspend fun stepMicroscope(delta: Int): Result<MicroscopeSessionSnapshot> =
        Result.failure(UnsupportedOperationException("Microscope navigation is not supported."))

    suspend fun jumpMicroscopeFrame(frameId: Long): Result<MicroscopeSessionSnapshot> =
        Result.failure(UnsupportedOperationException("Microscope navigation is not supported."))

    suspend fun jumpMicroscopeTimestampUs(
        timestampUs: Long,
        selection: TimestampSelectionPolicy,
    ): Result<MicroscopeSessionSnapshot> =
        Result.failure(UnsupportedOperationException("Microscope navigation is not supported."))

    suspend fun loadMicroscopeFrame(): Result<MicroscopeFrame> =
        Result.failure(UnsupportedOperationException("Microscope frame presentation is not supported."))

    suspend fun exportCurrentFrame(
        treeUri: String,
        format: FrameExportFormat,
        jpegQuality: Int = DEFAULT_JPEG_QUALITY,
    ): Result<ExportedFrameDocument> =
        Result.failure(UnsupportedOperationException("Current-frame export is not supported."))

    suspend fun exportFrames(
        treeUri: String,
        request: BatchExportRequest,
        onProgress: (BatchExportProgress) -> Unit = {},
    ): Result<ExportedBatchDocument> =
        Result.failure(UnsupportedOperationException("Batch frame export is not supported."))

    suspend fun closeMicroscope(): Boolean = true

    fun cancelActiveInspection() {}

    fun cancelActiveNativeOperation() = cancelActiveInspection()

    companion object {
        const val DEFAULT_JPEG_QUALITY = 92
    }
}

class AndroidFrameScopeRepository(
    private val contentResolver: ContentResolver,
    private val cacheRoot: String,
    private val nativeBridge: NativeBridge = RustBridge,
    private val frameBridge: NativeMicroscopeFrameBridge = MicroscopeFrameBridge,
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
    private val microscopeController: MicroscopeSessionController =
        MicroscopeSessionController(nativeBridge, frameBridge),
    private val exportDestinationFactory: FrameExportDestinationFactory =
        AndroidFrameExportDestinationFactory(contentResolver),
    private val batchDocumentFactory: ExportDocumentFactory =
        AndroidExportDocumentFactory(contentResolver),
    private val nativeUniqueExportBridge: NativeUniqueExportBridge = RustUniqueExportBridge,
) : FrameScopeRepository {
    private val nextOperationId = AtomicLong(1L)
    private val activeNativeOperationId = AtomicLong(NO_OPERATION)

    override suspend fun engineVersion(): Result<String> = withContext(ioDispatcher) {
        nativeBridge.version()
    }

    override suspend fun inspect(
        uri: String,
        onProgress: (InspectionProgress) -> Unit,
    ): Result<InspectedVideo> = try {
        Result.success(
            withContext(ioDispatcher) {
                currentCoroutineContext().ensureActive()
                onProgress(InspectionProgress.Opening)

                val parsedUri = requireContentUri(uri)
                val displayName = queryDisplayName(parsedUri)
                    ?.takeIf { it.isNotBlank() }
                    ?: "Selected video"

                currentCoroutineContext().ensureActive()
                val descriptor = openReadDescriptor(parsedUri)

                descriptor.use { pfd ->
                    currentCoroutineContext().ensureActive()
                    onProgress(InspectionProgress.Inspecting)

                    val operationId = nextOperationId()
                    activeNativeOperationId.set(operationId)
                    val nativeResult = try {
                        currentCoroutineContext().ensureActive()
                        nativeBridge.inspectVideoFd(pfd.fd, operationId)
                    } finally {
                        activeNativeOperationId.compareAndSet(operationId, NO_OPERATION)
                    }
                    currentCoroutineContext().ensureActive()

                    when (nativeResult) {
                        is NativeInspection.Success -> InspectedVideo(
                            displayName = displayName,
                            metadata = nativeResult.metadata,
                            engine = nativeResult.engine,
                        )

                        is NativeInspection.Failure -> {
                            Log.w(
                                TAG,
                                "Native inspection failed code=${nativeResult.code}: ${nativeResult.message}",
                            )
                            throw NativeFailureMapper.toThrowable(nativeResult)
                        }
                    }
                }
            },
        )
    } catch (cancelled: CancellationException) {
        throw cancelled
    } catch (error: VideoOpenException) {
        Result.failure(error)
    } catch (error: SecurityException) {
        Result.failure(permissionRevoked(error))
    } catch (error: Exception) {
        Result.failure(unreadableUri(error))
    }

    override suspend fun openMicroscope(uri: String): Result<MicroscopeSessionSnapshot> = try {
        withContext(ioDispatcher) {
            currentCoroutineContext().ensureActive()
            val parsedUri = requireContentUri(uri)
            val descriptor = openReadDescriptor(parsedUri)
            descriptor.use { pfd ->
                currentCoroutineContext().ensureActive()
                val operationId = nextOperationId()
                activeNativeOperationId.set(operationId)
                val nativeResult = try {
                    microscopeController.open(
                        fd = pfd.fd,
                        operationId = operationId,
                        cacheRoot = cacheRoot,
                    )
                } finally {
                    activeNativeOperationId.compareAndSet(operationId, NO_OPERATION)
                }
                try {
                    currentCoroutineContext().ensureActive()
                    microscopeResult(nativeResult)
                } catch (cancelled: CancellationException) {
                    if (nativeResult is NativeMicroscope.Success) {
                        microscopeController.closeIfCurrent(nativeResult.session.sessionId)
                    }
                    throw cancelled
                }
            }
        }
    } catch (cancelled: CancellationException) {
        throw cancelled
    } catch (error: VideoOpenException) {
        Result.failure(error)
    } catch (error: SecurityException) {
        Result.failure(permissionRevoked(error))
    } catch (error: Exception) {
        Result.failure(unreadableUri(error))
    }

    override suspend fun stepMicroscope(delta: Int): Result<MicroscopeSessionSnapshot> =
        runMicroscopeNavigation { microscopeController.step(delta) }

    override suspend fun jumpMicroscopeFrame(frameId: Long): Result<MicroscopeSessionSnapshot> =
        runMicroscopeNavigation { microscopeController.jumpToFrame(frameId) }

    override suspend fun jumpMicroscopeTimestampUs(
        timestampUs: Long,
        selection: TimestampSelectionPolicy,
    ): Result<MicroscopeSessionSnapshot> =
        runMicroscopeNavigation { microscopeController.jumpToTimestamp(timestampUs, selection) }

    override suspend fun loadMicroscopeFrame(): Result<MicroscopeFrame> = withContext(ioDispatcher) {
        currentCoroutineContext().ensureActive()
        val result = microscopeController.loadCurrentFrame()
        currentCoroutineContext().ensureActive()
        when (result) {
            is NativeFrameCopy.Success -> Result.success(
                MicroscopeFrame(
                    descriptor = result.frame,
                    rgba = result.rgba,
                ),
            )
            is NativeFrameCopy.Failure -> Result.failure(
                MicroscopeOperationException(result.code, result.message),
            )
        }
    }

    override suspend fun exportCurrentFrame(
        treeUri: String,
        format: FrameExportFormat,
        jpegQuality: Int,
    ): Result<ExportedFrameDocument> = try {
        Result.success(
            withContext(ioDispatcher) {
                currentCoroutineContext().ensureActive()
                if (format == FrameExportFormat.Jpeg && jpegQuality !in 1..100) {
                    throw FrameExportException(
                        code = "invalid_request",
                        message = "JPEG quality must be between 1 and 100.",
                    )
                }
                val snapshot = microscopeController.currentSnapshot()
                    ?: throw FrameExportException(
                        code = "session_not_found",
                        message = "No microscope session is currently open.",
                    )
                val frameId = snapshot.currentFrame?.frameId
                    ?: throw FrameExportException(
                        code = "no_frames",
                        message = "The current microscope session contains no indexed frames.",
                    )
                val displayName = stableFrameFileName(frameId, format)
                val destination = try {
                    exportDestinationFactory.create(
                        treeUri = treeUri,
                        displayName = displayName,
                        mimeType = format.mimeType,
                    )
                } catch (error: SecurityException) {
                    throw error
                } catch (error: Exception) {
                    throw FrameExportException(
                        code = "destination_error",
                        message = "Android could not create the frame export destination.",
                        cause = error,
                    )
                }

                destination.use { output ->
                    currentCoroutineContext().ensureActive()
                    val operationId = nextExportOperationId()
                    if (!activeNativeOperationId.compareAndSet(NO_OPERATION, operationId)) {
                        throw FrameExportException(
                            code = "operation_busy",
                            message = "Another native FrameScope operation is already active.",
                        )
                    }
                    val nativeResult = try {
                        currentCoroutineContext().ensureActive()
                        microscopeController.exportCurrentFrame(
                            outputFd = output.fd,
                            operationId = operationId,
                            format = format,
                            jpegQuality = jpegQuality,
                        )
                    } finally {
                        activeNativeOperationId.compareAndSet(operationId, NO_OPERATION)
                    }
                    currentCoroutineContext().ensureActive()

                    val export = when (nativeResult) {
                        is NativeFrameExport.Success -> nativeResult.export
                        is NativeFrameExport.Failure -> {
                            if (nativeResult.code in CANCELLATION_CODES) {
                                throw CancellationException(nativeResult.message)
                            }
                            throw FrameExportException(
                                code = nativeResult.code,
                                message = nativeResult.message,
                            )
                        }
                    }
                    if (
                        export.sessionId != snapshot.sessionId ||
                        export.frameId != frameId ||
                        export.format != format
                    ) {
                        throw FrameExportException(
                            code = "export_identity_mismatch",
                            message = "Frame export no longer matches the selected microscope frame.",
                        )
                    }
                    currentCoroutineContext().ensureActive()
                    output.commit()
                    ExportedFrameDocument(
                        uri = output.uri,
                        displayName = displayName,
                        export = export,
                    )
                }
            },
        )
    } catch (cancelled: CancellationException) {
        throw cancelled
    } catch (error: FrameExportException) {
        Result.failure(error)
    } catch (error: SecurityException) {
        Result.failure(
            FrameExportException(
                code = "permission_revoked",
                message = "FrameScope no longer has permission to write to this export folder.",
                cause = error,
            ),
        )
    } catch (error: Exception) {
        Result.failure(
            FrameExportException(
                code = "destination_error",
                message = "FrameScope could not write the exported frame.",
                cause = error,
            ),
        )
    }

    override suspend fun exportFrames(
        treeUri: String,
        request: BatchExportRequest,
        onProgress: (BatchExportProgress) -> Unit,
    ): Result<ExportedBatchDocument> = try {
        Result.success(
            withContext(ioDispatcher) {
                currentCoroutineContext().ensureActive()
                if (!request.isSane()) {
                    throw FrameExportException(
                        code = "invalid_request",
                        message = "Batch export selection, interval, or image format is invalid.",
                    )
                }
                val snapshot = microscopeController.currentSnapshot()
                    ?: throw FrameExportException(
                        code = "session_not_found",
                        message = "No microscope session is currently open.",
                    )
                validateBatchRequest(snapshot, request)

                val operationId = nextExportOperationId()
                if (!activeNativeOperationId.compareAndSet(NO_OPERATION, operationId)) {
                    throw FrameExportException(
                        code = "operation_busy",
                        message = "Another native FrameScope operation is already active.",
                    )
                }
                val manifestDisplayName = stableManifestFileName(snapshot.sessionId, operationId)
                try {
                    val manifest = try {
                        batchDocumentFactory.create(
                            treeUri = treeUri,
                            displayName = manifestDisplayName,
                            mimeType = MANIFEST_MIME_TYPE,
                        )
                    } catch (error: SecurityException) {
                        throw error
                    } catch (error: Exception) {
                        throw FrameExportException(
                            code = "destination_error",
                            message = "Android could not create the batch export manifest.",
                            cause = error,
                        )
                    }

                    manifest.use { manifestOutput ->
                        BatchSafFrameSink(
                            treeUri = treeUri,
                            documentFactory = batchDocumentFactory,
                            onProgress = onProgress,
                        ).use { sink ->
                            currentCoroutineContext().ensureActive()
                            val nativeResult = if (request.selection == BatchExportSelection.UniqueGroups) {
                                nativeUniqueExportBridge.exportMicroscopeUniqueGroups(
                                    sessionId = snapshot.sessionId,
                                    manifestFd = manifestOutput.fd,
                                    operationId = operationId,
                                    cacheRoot = cacheRoot,
                                    request = request,
                                    sink = sink,
                                )
                            } else {
                                nativeBridge.exportMicroscopeBatch(
                                    sessionId = snapshot.sessionId,
                                    manifestFd = manifestOutput.fd,
                                    operationId = operationId,
                                    request = request,
                                    sink = sink,
                                )
                            }
                            currentCoroutineContext().ensureActive()
                            if (microscopeController.currentSnapshot()?.sessionId != snapshot.sessionId) {
                                throw FrameExportException(
                                    code = "stale_result",
                                    message = "Batch export completed after the microscope source was replaced.",
                                )
                            }

                            when (nativeResult) {
                                is NativeBatchExport.Success -> {
                                    val export = nativeResult.export
                                    if (
                                        export.sessionId != snapshot.sessionId ||
                                        export.format != request.format
                                    ) {
                                        throw FrameExportException(
                                            code = "export_identity_mismatch",
                                            message = "Batch export no longer matches the requested microscope session.",
                                        )
                                    }
                                    manifestOutput.commit()
                                    ExportedBatchDocument(
                                        manifestUri = manifestOutput.uri,
                                        manifestDisplayName = manifestDisplayName,
                                        export = export,
                                    )
                                }

                                is NativeBatchExport.Failure -> {
                                    if (shouldPreserveFailureManifest(nativeResult.code, request.selection)) {
                                        manifestOutput.commit()
                                    }
                                    if (nativeResult.code in CANCELLATION_CODES) {
                                        throw CancellationException(nativeResult.message)
                                    }
                                    throw FrameExportException(
                                        code = nativeResult.code,
                                        message = nativeResult.message,
                                    )
                                }
                            }
                        }
                    }
                } finally {
                    activeNativeOperationId.compareAndSet(operationId, NO_OPERATION)
                }
            },
        )
    } catch (cancelled: CancellationException) {
        throw cancelled
    } catch (error: FrameExportException) {
        Result.failure(error)
    } catch (error: SecurityException) {
        Result.failure(
            FrameExportException(
                code = "permission_revoked",
                message = "FrameScope no longer has permission to write to this export folder.",
                cause = error,
            ),
        )
    } catch (error: Exception) {
        Result.failure(
            FrameExportException(
                code = "destination_error",
                message = "FrameScope could not write the batch export.",
                cause = error,
            ),
        )
    }

    override suspend fun closeMicroscope(): Boolean = withContext(ioDispatcher) {
        microscopeController.closeCurrent()
    }

    override fun cancelActiveInspection() {
        cancelActiveNativeOperation()
    }

    override fun cancelActiveNativeOperation() {
        val operationId = activeNativeOperationId.get()
        if (operationId != NO_OPERATION) {
            nativeBridge.cancelInspection(operationId)
        }
    }

    private suspend fun runMicroscopeNavigation(
        call: () -> NativeMicroscope,
    ): Result<MicroscopeSessionSnapshot> = withContext(ioDispatcher) {
        currentCoroutineContext().ensureActive()
        val result = call()
        currentCoroutineContext().ensureActive()
        microscopeResult(result)
    }

    private fun microscopeResult(result: NativeMicroscope): Result<MicroscopeSessionSnapshot> =
        when (result) {
            is NativeMicroscope.Success -> Result.success(result.session)
            is NativeMicroscope.Failure -> Result.failure(
                MicroscopeOperationException(result.code, result.message),
            )
        }

    private fun validateBatchRequest(
        snapshot: MicroscopeSessionSnapshot,
        request: BatchExportRequest,
    ) {
        if (snapshot.frameCount <= 0L) {
            throw FrameExportException(
                code = "no_frames",
                message = "The current microscope session contains no indexed frames.",
            )
        }
        when (val selection = request.selection) {
            is BatchExportSelection.CurrentFrame -> {
                if (snapshot.currentFrame?.frameId != selection.frameId) {
                    throw FrameExportException(
                        code = "stale_result",
                        message = "Current-frame batch export no longer matches the microscope position.",
                    )
                }
            }

            is BatchExportSelection.FrameRangeInclusive -> {
                if (selection.endFrameId >= snapshot.frameCount) {
                    throw FrameExportException(
                        code = "frame_out_of_range",
                        message = "Batch frame range extends beyond the indexed video.",
                    )
                }
            }

            is BatchExportSelection.TimestampRangeUsInclusive -> Unit
            BatchExportSelection.AllFrames, BatchExportSelection.UniqueGroups -> Unit
        }
    }

    private fun shouldPreserveFailureManifest(
        code: String,
        selection: BatchExportSelection,
    ): Boolean {
        if (
            selection == BatchExportSelection.UniqueGroups &&
            code in UNIQUE_PRE_MANIFEST_FAILURE_CODES
        ) {
            return false
        }
        return code !in NON_PERSISTABLE_MANIFEST_FAILURE_CODES
    }

    private fun stableManifestFileName(sessionId: Long, operationId: Long): String {
        if (sessionId <= 0L || operationId <= 0L) {
            throw FrameExportException(
                code = "invalid_request",
                message = "Batch manifest identity must be positive.",
            )
        }
        return "framescope_manifest_${sessionId}_${operationId}.jsonl"
    }

    private fun requireContentUri(uri: String): Uri {
        val parsedUri = Uri.parse(uri)
        if (parsedUri.scheme != ContentResolver.SCHEME_CONTENT) {
            throw VideoOpenException(
                kind = VideoOpenErrorKind.InvalidUri,
                message = "FrameScope can only open videos selected through Android's document picker.",
                diagnostic = "Expected content:// URI but received scheme=${parsedUri.scheme}",
            )
        }
        return parsedUri
    }

    private fun openReadDescriptor(uri: Uri) =
        contentResolver.openFileDescriptor(uri, "r")
            ?: throw VideoOpenException(
                kind = VideoOpenErrorKind.UnreadableUri,
                message = "Android could not open the selected video.",
                diagnostic = "ContentResolver.openFileDescriptor returned null for $uri",
            )

    private fun nextOperationId(): Long {
        while (true) {
            val current = nextOperationId.get()
            if (current <= 0L || current == Long.MAX_VALUE) {
                throw VideoOpenException(
                    kind = VideoOpenErrorKind.NativeFailure,
                    message = "FrameScope cannot start another native video operation in this process.",
                    diagnostic = "Native operation id space is exhausted.",
                )
            }
            if (nextOperationId.compareAndSet(current, current + 1L)) return current
        }
    }

    private fun nextExportOperationId(): Long = try {
        nextOperationId()
    } catch (error: VideoOpenException) {
        throw FrameExportException(
            code = "operation_id_exhausted",
            message = "FrameScope cannot start another frame export in this process.",
            cause = error,
        )
    }

    private fun stableFrameFileName(
        frameId: Long,
        format: FrameExportFormat,
    ): String {
        if (frameId < 0L) {
            throw FrameExportException(
                code = "frame_out_of_range",
                message = "Frame id must be non-negative for export.",
            )
        }
        return "frame_${frameId.toString().padStart(FRAME_ID_WIDTH, '0')}.${format.extension}"
    }

    private fun queryDisplayName(uri: Uri): String? {
        val cursor: Cursor = contentResolver.query(
            uri,
            arrayOf(OpenableColumns.DISPLAY_NAME),
            null,
            null,
            null,
        ) ?: return null
        return cursor.use {
            if (!it.moveToFirst()) return@use null
            val index = it.getColumnIndex(OpenableColumns.DISPLAY_NAME)
            if (index < 0 || it.isNull(index)) null else it.getString(index)
        }
    }

    private fun permissionRevoked(error: SecurityException) = VideoOpenException(
        kind = VideoOpenErrorKind.PermissionRevoked,
        message = "FrameScope no longer has permission to read this video. Please select it again.",
        diagnostic = error.message,
    )

    private fun unreadableUri(error: Exception) = VideoOpenException(
        kind = VideoOpenErrorKind.UnreadableUri,
        message = "FrameScope could not read the selected video.",
        diagnostic = error.message ?: error::class.java.simpleName,
    )

    private companion object {
        const val TAG = "FrameScopeRepository"
        const val NO_OPERATION = 0L
        const val FRAME_ID_WIDTH = 20
        const val MANIFEST_MIME_TYPE = "application/json"
        val CANCELLATION_CODES = setOf("cancelled", "cancellation", "cancelled_preflight")
        val UNIQUE_PRE_MANIFEST_FAILURE_CODES = setOf(
            "cancelled_preflight",
            "group_preflight_error",
            "unsafe_source_identity",
            "invalid_destination_preflight",
            "bridge_error",
        )
        val NON_PERSISTABLE_MANIFEST_FAILURE_CODES = setOf(
            "invalid_request",
            "session_not_found",
            "selection_error",
            "manifest_error",
            "jni_error",
            "native_library_unavailable",
            "malformed_response",
        )
    }
}

internal object NativeFailureMapper {
    fun toThrowable(failure: NativeInspection.Failure): Throwable {
        val diagnostic = "${failure.code}: ${failure.message}"
        return when (failure.code) {
            "cancelled", "cancellation" -> CancellationException("Native video inspection was cancelled.")
            "unsupported_format", "unsupported_codec" -> VideoOpenException(
                kind = VideoOpenErrorKind.UnsupportedVideo,
                message = "This video format or codec is not supported by this FrameScope build.",
                diagnostic = diagnostic,
            )

            "no_video_track", "no_video_stream" -> VideoOpenException(
                kind = VideoOpenErrorKind.NoVideoTrack,
                message = "The selected file does not contain a readable video track.",
                diagnostic = diagnostic,
            )

            "malformed_container", "malformed_data", "invalid_metadata" -> VideoOpenException(
                kind = VideoOpenErrorKind.CorruptMedia,
                message = "The selected video appears to be corrupt or malformed.",
                diagnostic = diagnostic,
            )

            "decoder_failure", "decoder_error" -> VideoOpenException(
                kind = VideoOpenErrorKind.DecoderFailure,
                message = "FrameScope could not decode this video's selected video stream.",
                diagnostic = diagnostic,
            )

            "io_error", "invalid_source" -> VideoOpenException(
                kind = VideoOpenErrorKind.UnreadableUri,
                message = "FrameScope could not read the selected video.",
                diagnostic = diagnostic,
            )

            "permission_revoked" -> VideoOpenException(
                kind = VideoOpenErrorKind.PermissionRevoked,
                message = "FrameScope no longer has permission to read this video. Please select it again.",
                diagnostic = diagnostic,
            )

            else -> VideoOpenException(
                kind = VideoOpenErrorKind.NativeFailure,
                message = "The Rust video engine could not inspect this video.",
                diagnostic = diagnostic,
            )
        }
    }
}
