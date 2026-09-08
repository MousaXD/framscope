package com.framescope.app.data

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

interface FrameScopeStorageRepository {
    suspend fun stats(): Result<FrameScopeStorageStats>

    suspend fun clear(scope: StorageClearScope): Result<StorageMutationResult>

    suspend fun clearSourceIndexes(sourceKey: String): Result<StorageMutationResult>
}

data class StorageMutationResult(
    val storage: FrameScopeStorageStats,
    val receipt: StorageClearReceipt,
)

class StorageOperationException(
    val code: String,
    message: String,
) : IllegalStateException(message)

internal class AndroidFrameScopeStorageRepository(
    private val cacheRoot: String,
    private val nativeBridge: NativeFrameScopeStorageBridge = FrameScopeStorageBridge,
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
) : FrameScopeStorageRepository {
    override suspend fun stats(): Result<FrameScopeStorageStats> = withContext(ioDispatcher) {
        when (val response = nativeBridge.stats(cacheRoot)) {
            is NativeStorageResponse.Success -> Result.success(response.storage)
            is NativeStorageResponse.Failure -> Result.failure(response.toException())
        }
    }

    override suspend fun clear(scope: StorageClearScope): Result<StorageMutationResult> =
        withContext(ioDispatcher) {
            nativeBridge.clear(cacheRoot, scope).toMutationResult(scope)
        }

    override suspend fun clearSourceIndexes(sourceKey: String): Result<StorageMutationResult> =
        withContext(ioDispatcher) {
            nativeBridge.clearSourceIndexes(cacheRoot, sourceKey)
                .toMutationResult(StorageClearScope.PersistentIndexes)
        }

    private fun NativeStorageResponse.toMutationResult(
        expectedScope: StorageClearScope,
    ): Result<StorageMutationResult> {
        return when (this) {
            is NativeStorageResponse.Failure -> Result.failure(toException())
            is NativeStorageResponse.Success -> {
                val receipt = cleared
                    ?: return Result.failure(
                        StorageOperationException(
                            code = "bridge_error",
                            message = "Native clear succeeded without a clear receipt.",
                        ),
                    )
                if (receipt.scope != expectedScope) {
                    return Result.failure(
                        StorageOperationException(
                            code = "bridge_error",
                            message = "Native clear response did not match the requested scope.",
                        ),
                    )
                }
                Result.success(StorageMutationResult(storage = storage, receipt = receipt))
            }
        }
    }

    private fun NativeStorageResponse.Failure.toException(): StorageOperationException =
        StorageOperationException(code = code, message = message)
}
