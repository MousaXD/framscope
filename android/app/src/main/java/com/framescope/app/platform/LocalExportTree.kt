package com.framescope.app.platform

import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.activity.result.contract.ActivityResultContracts

/** System folder picker restricted to storage exposed locally by Android document providers. */
class LocalExportTree : ActivityResultContracts.OpenDocumentTree() {
    override fun createIntent(context: Context, input: Uri?): Intent =
        super.createIntent(context, input)
            .putExtra(Intent.EXTRA_LOCAL_ONLY, true)
}
