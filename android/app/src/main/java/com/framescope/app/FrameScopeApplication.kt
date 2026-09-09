package com.framescope.app

import android.app.Application
import com.framescope.app.data.RamAccelerationRuntime

class FrameScopeApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        // Keep RAM policy on the exact same namespace used by microscope/scrub/storage. A separate
        // filesDir root makes live source-cache resize/trim target a cache that navigation never uses.
        RamAccelerationRuntime.initialize(
            context = this,
            cacheRoot = cacheDir.resolve("framescope").absolutePath,
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
