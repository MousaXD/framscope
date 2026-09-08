package com.framescope.app.platform

import android.content.ContentResolver
import android.content.Intent
import android.net.Uri
import com.framescope.app.data.VideoUriPermissionStatus

class VideoUriPermissionManager(
    private val contentResolver: ContentResolver,
) {
    fun persistReadAccess(uri: Uri): VideoUriPermissionStatus {
        if (uri.scheme != ContentResolver.SCHEME_CONTENT) {
            return VideoUriPermissionStatus.Lost
        }

        runCatching {
            contentResolver.takePersistableUriPermission(
                uri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION,
            )
        }

        return if (
            contentResolver.persistedUriPermissions.any { permission ->
                permission.uri == uri && permission.isReadPermission
            }
        ) {
            VideoUriPermissionStatus.Persisted
        } else {
            VideoUriPermissionStatus.Transient
        }
    }
}
