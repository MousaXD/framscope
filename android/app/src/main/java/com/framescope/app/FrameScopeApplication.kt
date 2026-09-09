package com.framescope.app

import android.app.Application
import com.framescope.app.data.RamAccelerationRuntime
import com.framescope.app.performance.FrameScopePerformanceRuntime

class FrameScopeApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        RamAccelerationRuntime.initialize(
            context = this,
            cacheRoot = cacheDir.resolve("ram-acceleration"),
        )
        FrameScopePerformanceRuntime.initialize(this)
    }

    override fun onTrimMemory(level: Int) {
        super.onTrimMemory(level)
        RamAccelerationRuntime.onTrimMemory(level)
        FrameScopePerformanceRuntime.onTrimMemory(level)
    }

    override fun onLowMemory() {
        RamAccelerationRuntime.onLowMemory()
        FrameScopePerformanceRuntime.onLowMemory()
        super.onLowMemory()
    }
}
