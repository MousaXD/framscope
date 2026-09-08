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

        val persisted = runCatching {
            contentResolver.persistedUriPermissions.any { permission ->
                permission.uri == uri && permission.isReadPermission
            }
        }.getOrDefault(false)

        return if (persisted) {
            VideoUriPermissionStatus.Persisted
        } else {
            // The current Activity grant may still be usable for this session. History records this
            // honestly as transient and revalidates access before offering a future Resume action.
            VideoUriPermissionStatus.Transient
        }
    }
}
