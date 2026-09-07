package com.framescope.app.data

import android.content.ContentResolver
import android.database.Cursor
import android.net.Uri
import android.provider.OpenableColumns
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

interface FrameScopeRepository {
    suspend fun engineVersion(): Result<String>
    suspend fun inspect(uri: String): Result<InspectedVideo>
}

class AndroidFrameScopeRepository(
    private val contentResolver: ContentResolver,
    private val nativeBridge: NativeBridge = RustBridge,
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
) : FrameScopeRepository {

    override suspend fun engineVersion(): Result<String> = withContext(ioDispatcher) {
        nativeBridge.version()
    }

    override suspend fun inspect(uri: String): Result<InspectedVideo> = withContext(ioDispatcher) {
        runCatching {
            val parsedUri = Uri.parse(uri)
            require(parsedUri.scheme != null) { "Selected video URI is invalid." }

            val displayName = queryDisplayName(parsedUri)
                ?.takeIf { it.isNotBlank() }
                ?: "Selected video"
            val descriptor = contentResolver.openFileDescriptor(parsedUri, "r")
                ?: error("Android could not open the selected video.")

            descriptor.use { pfd ->
                when (val result = nativeBridge.inspectVideoFd(pfd.fd)) {
                    is NativeInspection.Success -> InspectedVideo(
                        displayName = displayName,
                        metadata = result.metadata,
                        engine = result.engine,
                    )

                    is NativeInspection.Failure -> error(userMessage(result))
                }
            }
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

    private fun userMessage(failure: NativeInspection.Failure): String = when (failure.code) {
        "unsupported_format" -> "This file is not supported by the Phase 1 Rust inspector. MP4/MOV is supported now; broader codec/container support is planned for Phase 2."
        "no_video_track" -> "The selected file does not contain a readable video track."
        "malformed_container", "invalid_metadata", "malformed_metadata" -> "The video container metadata appears malformed or unsupported."
        "io_error" -> "FrameScope could not read the selected video."
        else -> failure.message.ifBlank { "The Rust engine could not inspect this video." }
    }
}
