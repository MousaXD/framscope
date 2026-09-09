package com.framescope.app.data

import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext

interface MicroscopeSimilarityRepository {
    suspend fun findSimilarFrames(
        sessionId: Long,
        targetFrameId: Long,
    ): Result<MicroscopeSimilarityResult>

    fun cancelActiveSearch()
}

internal class AndroidMicroscopeSimilarityRepository(
    private val cacheRoot: String,
    private val nativeBridge: NativeMicroscopeSimilarityBridge = MicroscopeSimilarityBridge,
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
) : MicroscopeSimilarityRepository {
    private val nextOperationId = AtomicLong(1L)
    private val activeOperationId = AtomicLong(NO_OPERATION)

    override suspend fun findSimilarFrames(
        sessionId: Long,
        targetFrameId: Long,
    ): Result<MicroscopeSimilarityResult> = try {
        Result.success(
            withContext(ioDispatcher) {
                currentCoroutineContext().ensureActive()
                if (sessionId <= 0L || targetFrameId < 0L || cacheRoot.isBlank()) {
                    throw MicroscopeSimilarityException(
                        code = "invalid_request",
                        message = "Similarity requires a live microscope frame and cache root.",
                    )
                }
                val operationId = allocateOperationId()
                if (!activeOperationId.compareAndSet(NO_OPERATION, operationId)) {
                    throw MicroscopeSimilarityException(
                        code = "operation_busy",
                        message = "Another similarity search is already active.",
                    )
                }
                try {
                    currentCoroutineContext().ensureActive()
                    when (
                        val native = nativeBridge.findSimilarFrames(
                            sessionId = sessionId,
                            targetFrameId = targetFrameId,
                            operationId = operationId,
                            cacheRoot = cacheRoot,
                        )
                    ) {
                        is NativeMicroscopeSimilarity.Success -> {
                            currentCoroutineContext().ensureActive()
                            val result = native.result
                            if (
                                result.sessionId != sessionId ||
                                result.targetFrameId != targetFrameId
                            ) {
                                throw MicroscopeSimilarityException(
                                    code = "similarity_identity_mismatch",
                                    message = "Similarity results no longer match the requested microscope frame.",
                                )
                            }
                            result
                        }
                        is NativeMicroscopeSimilarity.Failure -> {
                            if (native.code == "cancelled") {
                                throw CancellationException(native.message)
                            }
                            throw MicroscopeSimilarityException(native.code, native.message)
                        }
                    }
                } finally {
                    activeOperationId.compareAndSet(operationId, NO_OPERATION)
                }
            },
        )
    } catch (cancelled: CancellationException) {
        throw cancelled
    } catch (error: MicroscopeSimilarityException) {
        Result.failure(error)
    } catch (error: Exception) {
        Result.failure(
            MicroscopeSimilarityException(
                code = "similarity_error",
                message = error.message ?: "FrameScope could not find similar frames.",
            ),
        )
    }

    override fun cancelActiveSearch() {
        val operationId = activeOperationId.get()
        if (operationId != NO_OPERATION) {
            nativeBridge.cancel(operationId)
        }
    }

    private fun allocateOperationId(): Long {
        while (true) {
            val current = nextOperationId.getAndUpdate { previous ->
                if (previous == Long.MAX_VALUE) 1L else previous + 1L
            }
            if (current > 0L) return current
        }
    }

    private companion object {
        const val NO_OPERATION = 0L
    }
}
