package com.framescope.app.data

import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FrameScopeStorageRepositoryTest {
    @Test
    fun nativeActiveSessionFailureIsPreservedAsTypedStorageError() = runTest {
        val bridge = FakeStorageBridge(
            clearResponse = NativeStorageResponse.Failure(
                code = "active_session",
                message = "Close the current video before clearing FrameScope index or cache data",
                engine = "framescope-rust/test",
            ),
        )
        val repository = AndroidFrameScopeStorageRepository(
            cacheRoot = "/cache/framescope",
            nativeBridge = bridge,
            ioDispatcher = UnconfinedTestDispatcher(testScheduler),
        )

        val result = repository.clear(StorageClearScope.All)

        assertTrue(result.isFailure)
        val failure = result.exceptionOrNull() as StorageOperationException
        assertEquals("active_session", failure.code)
        assertEquals(1, bridge.clearCalls)
    }

    @Test
    fun nativeClearReceiptMustMatchRequestedScope() = runTest {
        val storage = emptyStorage()
        val bridge = FakeStorageBridge(
            clearResponse = NativeStorageResponse.Success(
                storage = storage,
                cleared = StorageClearReceipt(
                    scope = StorageClearScope.PreviewProxy,
                    clearedBytes = 0L,
                    clearedFiles = 0L,
                    clearedItems = 0L,
                ),
            ),
        )
        val repository = AndroidFrameScopeStorageRepository(
            cacheRoot = "/cache/framescope",
            nativeBridge = bridge,
            ioDispatcher = UnconfinedTestDispatcher(testScheduler),
        )

        val result = repository.clear(StorageClearScope.All)

        assertTrue(result.isFailure)
        val failure = result.exceptionOrNull() as StorageOperationException
        assertEquals("bridge_error", failure.code)
    }

    private class FakeStorageBridge(
        private val clearResponse: NativeStorageResponse,
    ) : NativeFrameScopeStorageBridge {
        var clearCalls = 0

        override fun stats(cacheRoot: String): NativeStorageResponse =
            NativeStorageResponse.Success(storage = emptyStorage(), cleared = null)

        override fun clear(
            cacheRoot: String,
            scope: StorageClearScope,
        ): NativeStorageResponse {
            clearCalls += 1
            return clearResponse
        }

        override fun clearSourceIndexes(
            cacheRoot: String,
            sourceKey: String,
        ): NativeStorageResponse = clearResponse
    }

    companion object {
        private fun emptyStorage(): FrameScopeStorageStats = FrameScopeStorageStats(
            totalBytes = 0L,
            persistentIndexes = StorageCategoryStats(0L, 0L, 0L),
            previewProxy = StorageCategoryStats(0L, 0L, 0L),
            disposable = StorageCategoryStats(0L, 0L, 0L),
            indexedSources = 0L,
            previewProxyEnabled = false,
        )
    }
}
