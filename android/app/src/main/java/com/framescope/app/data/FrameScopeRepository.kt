package com.framescope.app.data

import android.content.ContentResolver
import android.database.Cursor
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Log
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
}

class AndroidFrameScopeRepository(
    private val contentResolver: ContentResolver,
    private val nativeBridge: NativeBridge = RustBridge,
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
) : FrameScopeRepository {

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

                val parsedUri = Uri.parse(uri)
                if (parsedUri.scheme != ContentResolver.SCHEME_CONTENT) {
                    throw VideoOpenException(
                        kind = VideoOpenErrorKind.InvalidUri,
                        message = "FrameScope can only open videos selected through Android's document picker.",
                        diagnostic = "Expected content:// URI but received scheme=${parsedUri.scheme}",
                    )
                }

                val displayName = queryDisplayName(parsedUri)
                    ?.takeIf { it.isNotBlank() }
                    ?: "Selected video"

                currentCoroutineContext().ensureActive()
                val descriptor = contentResolver.openFileDescriptor(parsedUri, "r")
                    ?: throw VideoOpenException(
                        kind = VideoOpenErrorKind.UnreadableUri,
                        message = "Android could not open the selected video.",
                        diagnostic = "ContentResolver.openFileDescriptor returned null for $parsedUri",
                    )

                descriptor.use { pfd ->
                    currentCoroutineContext().ensureActive()
                    onProgress(InspectionProgress.Inspecting)

                    val nativeResult = nativeBridge.inspectVideoFd(pfd.fd)
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
        Result.failure(
            VideoOpenException(
                kind = VideoOpenErrorKind.PermissionRevoked,
                message = "FrameScope no longer has permission to read this video. Please select it again.",
                diagnostic = error.message,
            ),
        )
    } catch (error: Exception) {
        Result.failure(
            VideoOpenException(
                kind = VideoOpenErrorKind.UnreadableUri,
                message = "FrameScope could not read the selected video.",
                diagnostic = error.message ?: error::class.java.simpleName,
            ),
        )
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

    private companion object {
        const val TAG = "FrameScopeRepository"
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
