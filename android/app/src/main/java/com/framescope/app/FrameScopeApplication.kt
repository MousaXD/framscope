package com.framescope.app

import android.app.Application
import com.framescope.app.data.RamAccelerationRuntime
import java.io.File

class FrameScopeApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        RamAccelerationRuntime.initialize(
            context = this,
            cacheRoot = File(filesDir, "framescope-cache").absolutePath,
        )
    }

    override fun onTrimMemory(level: Int) {
        super.onTrimMemory(level)
        RamAccelerationRuntime.onTrimMemory(level)
    }

    override fun onLowMemory() {
        super.onLowMemory()
        RamAccelerationRuntime.onLowMemory()
    }
}
