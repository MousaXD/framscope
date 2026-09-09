package com.framescope.app.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import com.framescope.app.data.InspectedVideo
import com.framescope.app.data.NoOpPersistentFrameIndexCatalog
import com.framescope.app.data.PersistentFrameIndexCatalog
import com.framescope.app.data.RecentVideoHistory
import com.framescope.app.data.RecentVideoRecord
import com.framescope.app.data.VideoUriPermissionStatus

data class RecentVideoSourceSignature(
    val displayName: String,
    val durationUs: Long?,
    val width: Int,
    val height: Int,
)

data class RecentVideoOpenTarget(
    val contentUri: String,
    val permissionStatus: VideoUriPermissionStatus,
    val resumeTimestampUs: Long? = null,
    val replacesRecordId: String? = null,
    val resumeSourceSignature: RecentVideoSourceSignature? = null,
)

fun RecentVideoRecord.toOpenTarget(): RecentVideoOpenTarget? {
    val uri = contentUri ?: return null
    if (!canOpen()) return null
    return RecentVideoOpenTarget(
        contentUri = uri,
        permissionStatus = permissionStatus,
        resumeTimestampUs = lastViewedTimestampUs,
        resumeSourceSignature = resumeSourceSignature(),
    )
}

fun RecentVideoRecord.toReselectTarget(
    newContentUri: String,
    permissionStatus: VideoUriPermissionStatus,
): RecentVideoOpenTarget = RecentVideoOpenTarget(
    contentUri = newContentUri,
    permissionStatus = permissionStatus,
    resumeTimestampUs = lastViewedTimestampUs,
    replacesRecordId = id,
    resumeSourceSignature = resumeSourceSignature().takeUnless {
        contentUri == null && width == 0 && height == 0 && durationUs == null
    },
)

internal fun RecentVideoOpenTarget.canResume(inspectedVideo: InspectedVideo?): Boolean {
    if (resumeTimestampUs == null) return false
    val expected = resumeSourceSignature ?: return true
    val actual = inspectedVideo ?: return false
    return expected.displayName == actual.displayName &&
        expected.durationUs == actual.metadata.durationUs &&
        expected.width == actual.metadata.width &&
        expected.height == actual.metadata.height
}

private fun RecentVideoRecord.resumeSourceSignature() = RecentVideoSourceSignature(
    displayName = displayName,
    durationUs = durationUs,
    width = width,
    height = height,
)

@Composable
fun RecentVideoSessionEffects(
    target: RecentVideoOpenTarget?,
    videoState: VideoInspectionState,
    microscopeState: MicroscopeUiState,
    history: RecentVideoHistory,
    indexCatalog: PersistentFrameIndexCatalog = NoOpPersistentFrameIndexCatalog,
    onResumeTimestampUs: (Long) -> Unit,
    onLibraryChanged: () -> Unit = {},
) {
    var resumedSessionId by remember(target?.contentUri, target?.resumeTimestampUs) {
        mutableLongStateOf(NO_SESSION)
    }
    var boundSessionId by remember(target?.contentUri) {
        mutableLongStateOf(NO_SESSION)
    }

    LaunchedEffect(videoState, target) {
        val source = target ?: return@LaunchedEffect
        val ready = videoState as? VideoInspectionState.Ready ?: return@LaunchedEffect
        val persisted = runCatching {
            if (source.replacesRecordId == null) {
                history.recordOpened(
                    contentUri = source.contentUri,
                    video = ready.video,
                    permissionStatus = source.permissionStatus,
                )
            } else {
                history.recordReselected(
                    recordId = source.replacesRecordId,
                    contentUri = source.contentUri,
                    video = ready.video,
                    permissionStatus = source.permissionStatus,
                )
            }
        }.isSuccess
        if (persisted) onLibraryChanged()
    }

    LaunchedEffect(microscopeState, videoState, target) {
        val source = target ?: return@LaunchedEffect
        val ready = microscopeState as? MicroscopeUiState.Ready ?: return@LaunchedEffect
        val frame = ready.session.currentFrame ?: return@LaunchedEffect
        val resumeTimestampUs = source.resumeTimestampUs
        val inspectedVideo = (videoState as? VideoInspectionState.Ready)?.video

        if (boundSessionId != ready.session.sessionId && inspectedVideo != null) {
            val binding = runCatching {
                indexCatalog.bindingForSession(ready.session.sessionId)
            }.getOrNull()
            if (binding != null) {
                val bound = runCatching {
                    history.updateIndexBinding(source.contentUri, binding)
                }.isSuccess
                if (bound) onLibraryChanged()
            }
            boundSessionId = ready.session.sessionId
        }

        if (
            resumeTimestampUs != null &&
            source.canResume(inspectedVideo) &&
            resumedSessionId != ready.session.sessionId
        ) {
            // Preserve the saved point until the existing generation-safe microscope path completes
            // its authoritative VFR-aware timestamp jump. A changed document with the same URI fails
            // the source signature check and starts at the newly indexed first frame instead.
            resumedSessionId = ready.session.sessionId
            onResumeTimestampUs(resumeTimestampUs)
            return@LaunchedEffect
        }

        runCatching {
            history.updatePosition(
                contentUri = source.contentUri,
                frameId = frame.frameId,
                timestampUs = frame.timestampUs,
            )
        }
    }

    LaunchedEffect(microscopeState, target?.contentUri) {
        if (microscopeState is MicroscopeUiState.Idle || microscopeState is MicroscopeUiState.Error) {
            resumedSessionId = NO_SESSION
            boundSessionId = NO_SESSION
        }
    }
}

private const val NO_SESSION = 0L
