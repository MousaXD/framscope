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

    suspend fun closeMicroscope(): Boolean = true

    fun cancelActiveInspection() {}
}

class AndroidFrameScopeRepository(
    private val contentResolver: ContentResolver,
    private val cacheRoot: String,
    private val nativeBridge: NativeBridge = RustBridge,
    private val frameBridge: NativeMicroscopeFrameBridge = MicroscopeFrameBridge,
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
    private val microscopeController: MicroscopeSessionController =
        MicroscopeSessionController(nativeBridge, frameBridge),
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

    override suspend fun closeMicroscope(): Boolean = withContext(ioDispatcher) {
        microscopeController.closeCurrent()
    }

    override fun cancelActiveInspection() {
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
