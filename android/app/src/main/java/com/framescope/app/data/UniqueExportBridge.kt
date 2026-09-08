package com.framescope.app.data

interface NativeUniqueExportBridge {
    fun exportMicroscopeUniqueGroups(
        sessionId: Long,
        manifestFd: Int,
        operationId: Long,
        cacheRoot: String,
        request: BatchExportRequest,
        sink: NativeBatchFrameSink,
    ): NativeBatchExport
}

object RustUniqueExportBridge : NativeUniqueExportBridge {
    private val loadFailure: Throwable? = runCatching {
        System.loadLibrary("framescope_ffi")
    }.exceptionOrNull()

    @JvmStatic
    private external fun nativeExportMicroscopeUniqueGroupsFd(
        sessionId: Long,
        manifestFd: Int,
        operationId: Long,
        cacheRoot: String,
        format: Int,
        jpegQuality: Int,
        sink: NativeBatchFrameSink,
    ): String?

    override fun exportMicroscopeUniqueGroups(
        sessionId: Long,
        manifestFd: Int,
        operationId: Long,
        cacheRoot: String,
        request: BatchExportRequest,
        sink: NativeBatchFrameSink,
    ): NativeBatchExport {
        if (
            sessionId <= 0L ||
            manifestFd < 0 ||
            operationId <= 0L ||
            cacheRoot.isBlank() ||
            cacheRoot.length > MAX_CACHE_ROOT_LENGTH ||
            request.selection != BatchExportSelection.UniqueGroups ||
            !request.isSane()
        ) {
            return NativeBatchExport.Failure(
                code = "invalid_request",
                message = "Unique export requires a valid session, destination, operation, cache root, and unique-group request.",
                engine = null,
            )
        }
        loadFailure?.let {
            return NativeBatchExport.Failure(
                code = "native_library_unavailable",
                message = "Rust engine could not be loaded: ${it.message ?: it::class.java.simpleName}",
                engine = null,
            )
        }

        val raw = runCatching {
            nativeExportMicroscopeUniqueGroupsFd(
                sessionId = sessionId,
                manifestFd = manifestFd,
                operationId = operationId,
                cacheRoot = cacheRoot,
                format = request.format.nativeValue,
                jpegQuality = request.jpegQuality,
                sink = sink,
            )
        }.getOrElse {
            return NativeBatchExport.Failure(
                code = "jni_error",
                message = "Rust unique export call failed: ${it.message ?: it::class.java.simpleName}",
                engine = null,
            )
        } ?: return NativeBatchExport.Failure(
            code = "jni_error",
            message = "Rust engine returned a null unique export response.",
            engine = null,
        )

        return RustBridge.parseBatchExportResponse(raw)
    }

    private const val MAX_CACHE_ROOT_LENGTH = 4_096
}
