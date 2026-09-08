package com.framescope.app.data

import android.system.ErrnoException
import android.system.OsConstants
import java.io.IOException

internal data class ExportStorageFailure(
    val code: String,
    val message: String,
)

/**
 * Converts storage-capacity failures from SAF/Java and native Rust writes into one stable UI error.
 *
 * Android document providers may surface capacity failures as an ErrnoException wrapped by one or
 * more framework exceptions. Rust's std::io::Error crosses JNI as a typed native failure string, so
 * the native path is deliberately restricted to output-related codes before inspecting its message.
 */
internal object ExportStorageFailureClassifier {
    private const val MAX_CAUSE_DEPTH = 16
    private const val STORAGE_FULL_CODE = "storage_full"
    private const val STORAGE_FULL_MESSAGE =
        "The export destination is out of space or has reached its storage quota. Free space or choose another folder and try again."

    private val nativeOutputCodes = setOf(
        "io_error",
        "manifest_error",
        "invalid_destination",
        "image_encode_error",
        "output_callback_error",
        "output_commit_error",
    )

    private val capacityMarkers = listOf(
        "no space left on device",
        "disk quota exceeded",
        "quota exceeded",
        "storage full",
        "disk full",
        "enospc",
        "edquot",
        "os error 28",
        "os error 122",
    )

    fun classify(error: Throwable): ExportStorageFailure? {
        var current: Throwable? = error
        repeat(MAX_CAUSE_DEPTH) {
            val cause = current ?: return null
            if (cause is ErrnoException && isCapacityErrno(cause.errno)) {
                return storageFull()
            }
            if (cause is IOException && isCapacityMessage(cause.message)) {
                return storageFull()
            }
            current = cause.cause.takeUnless { it === cause }
        }
        return null
    }

    fun classifyNative(code: String, message: String): ExportStorageFailure? {
        if (code !in nativeOutputCodes || !isCapacityMessage(message)) return null
        return storageFull()
    }

    private fun isCapacityErrno(errno: Int): Boolean =
        errno == OsConstants.ENOSPC || errno == OsConstants.EDQUOT

    private fun isCapacityMessage(message: String?): Boolean {
        val normalized = message?.lowercase() ?: return false
        return capacityMarkers.any(normalized::contains)
    }

    private fun storageFull() = ExportStorageFailure(
        code = STORAGE_FULL_CODE,
        message = STORAGE_FULL_MESSAGE,
    )
}
