package com.framescope.app.data

data class ExportedFrameDocument(
    val uri: String,
    val displayName: String,
    val export: FrameExportResult,
)

class FrameExportException(
    val code: String,
    message: String,
    cause: Throwable? = null,
) : IllegalStateException(message, cause)
