package com.framescope.app

import android.app.Application
import com.framescope.app.data.RamAccelerationRuntime
import com.framescope.app.performance.FrameScopePerformanceRuntime

class FrameScopeApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        FrameScopePerformanceRuntime.initialize(this)
        // Keep RAM policy on the same cache namespace used by microscope/scrub/storage.
        RamAccelerationRuntime.initialize(
            context = this,
            cacheRoot = cacheDir.resolve("framescope").absolutePath,
        )
    }

    override fun onTrimMemory(level: Int) {
        super.onTrimMemory(level)
        FrameScopePerformanceRuntime.onTrimMemory(level)
        RamAccelerationRuntime.onTrimMemory(level)
    }

    override fun onLowMemory() {
        super.onLowMemory()
        FrameScopePerformanceRuntime.onLowMemory()
        RamAccelerationRuntime.onLowMemory()
    }
}
