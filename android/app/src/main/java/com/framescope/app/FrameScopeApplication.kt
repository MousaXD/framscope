package com.framescope.app

import android.app.Application
import com.framescope.app.data.RamAccelerationRuntime
import com.framescope.app.performance.FrameScopePerformanceRuntime
import java.io.File

class FrameScopeApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        FrameScopePerformanceRuntime.initialize(this)
        RamAccelerationRuntime.initialize(
            context = this,
            cacheRoot = File(filesDir, "framescope-cache").absolutePath,
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
