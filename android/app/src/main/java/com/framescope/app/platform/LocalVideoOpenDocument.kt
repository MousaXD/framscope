package com.framescope.app.platform

import android.content.Context
import android.content.Intent
import androidx.activity.result.contract.ActivityResultContracts

/** System document picker contract restricted to media already available on the device. */
class LocalVideoOpenDocument : ActivityResultContracts.OpenDocument() {
    override fun createIntent(context: Context, input: Array<String>): Intent =
        super.createIntent(context, input)
            .putExtra(Intent.EXTRA_LOCAL_ONLY, true)
}
