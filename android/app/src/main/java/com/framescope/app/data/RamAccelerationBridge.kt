package com.framescope.app.data

internal interface NativeRamAccelerationBridge {
    fun configure(
        cacheRoot: String,
        sourceCacheBytes: Long,
        previewCacheBytes: Long,
    ): Boolean
}

internal object RamAccelerationBridge : NativeRamAccelerationBridge {
    private val loadFailure: Throwable? = runCatching {
        System.loadLibrary("framescope_ffi")
    }.exceptionOrNull()

    @JvmStatic
    private external fun nativeConfigureRamAcceleration(
        cacheRoot: String,
        sourceCacheBytes: Long,
        previewCacheBytes: Long,
    ): Boolean

    override fun configure(
        cacheRoot: String,
        sourceCacheBytes: Long,
        previewCacheBytes: Long,
    ): Boolean {
        if (loadFailure != null) return false
        return runCatching {
            nativeConfigureRamAcceleration(
                cacheRoot,
                sourceCacheBytes,
                previewCacheBytes,
            )
        }.getOrDefault(false)
    }
}
