package com.framescope.app.data

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext

interface MicroscopeScrubPreviewSource {
    suspend fun renderTimestamp(
        sessionId: Long,
        timestampUs: Long,
        selection: TimestampSelectionPolicy = TimestampSelectionPolicy.Nearest,
    ): Result<MicroscopeScrubPreview>

    suspend fun renderFrame(
        sessionId: Long,
        frameId: Long,
    ): Result<MicroscopeScrubPreview>

    fun cancelSession(sessionId: Long): Boolean

    suspend fun forgetSession(sessionId: Long)
}

object UnsupportedMicroscopeScrubPreviewSource : MicroscopeScrubPreviewSource {
    private fun unsupported(): Result<MicroscopeScrubPreview> = Result.failure(
        MicroscopeOperationException(
            code = "preview_unavailable",
            message = "Live scrub preview is not configured for this FrameScope instance.",
        ),
    )

    override suspend fun renderTimestamp(
        sessionId: Long,
        timestampUs: Long,
        selection: TimestampSelectionPolicy,
    ): Result<MicroscopeScrubPreview> = unsupported()

    override suspend fun renderFrame(
        sessionId: Long,
        frameId: Long,
    ): Result<MicroscopeScrubPreview> = unsupported()

    override fun cancelSession(sessionId: Long): Boolean = false

    override suspend fun forgetSession(sessionId: Long) = Unit
}

class AndroidMicroscopeScrubPreviewSource(
    private val cacheRoot: String,
    private val bridge: NativeMicroscopePreviewBridge = MicroscopePreviewBridge,
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
) : MicroscopeScrubPreviewSource {
    init {
        require(cacheRoot.isNotBlank()) { "Live scrub preview cache root must not be blank." }
    }

    override suspend fun renderTimestamp(
        sessionId: Long,
        timestampUs: Long,
        selection: TimestampSelectionPolicy,
    ): Result<MicroscopeScrubPreview> = withContext(ioDispatcher) {
        currentCoroutineContext().ensureActive()
        val nativeResult = ScrubPerformanceTelemetry.measureRender {
            bridge.renderTimestamp(
                sessionId = sessionId,
                timestampUs = timestampUs,
                selection = selection,
                cacheRoot = cacheRoot,
            )
        }
        currentCoroutineContext().ensureActive()
        bridgeResult(nativeResult, expectedSessionId = sessionId)
    }

    override suspend fun renderFrame(
        sessionId: Long,
        frameId: Long,
    ): Result<MicroscopeScrubPreview> = withContext(ioDispatcher) {
        currentCoroutineContext().ensureActive()
        val nativeResult = ScrubPerformanceTelemetry.measureRender {
            bridge.renderFrame(
                sessionId = sessionId,
                frameId = frameId,
                cacheRoot = cacheRoot,
            )
        }
        currentCoroutineContext().ensureActive()
        bridgeResult(nativeResult, expectedSessionId = sessionId)
    }

    override fun cancelSession(sessionId: Long): Boolean =
        sessionId > 0L && bridge.cancelSession(sessionId)

    override suspend fun forgetSession(sessionId: Long) {
        if (sessionId <= 0L) return
        withContext(ioDispatcher) {
            bridge.forgetSession(sessionId)
        }
    }

    private fun bridgeResult(
        result: NativeMicroscopePreview,
        expectedSessionId: Long,
    ): Result<MicroscopeScrubPreview> = when (result) {
        is NativeMicroscopePreview.Failure -> Result.failure(
            MicroscopeOperationException(result.code, result.message),
        )
        is NativeMicroscopePreview.Success -> {
            if (result.preview.descriptor.sessionId != expectedSessionId) {
                Result.failure(
                    MicroscopeOperationException(
                        code = "preview_identity_mismatch",
                        message = "Live scrub preview belongs to a different microscope session.",
                    ),
                )
            } else {
                Result.success(result.preview)
            }
        }
    }
}
