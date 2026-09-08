package com.framescope.app.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import com.framescope.app.data.InspectedVideo
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

fun RecentVideoRecord.toOpenTarget(): RecentVideoOpenTarget? =
    takeIf(RecentVideoRecord::canOpen)?.let { record ->
        RecentVideoOpenTarget(
            contentUri = record.contentUri,
            permissionStatus = record.permissionStatus,
            resumeTimestampUs = record.lastViewedTimestampUs,
            resumeSourceSignature = record.resumeSourceSignature(),
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
    resumeSourceSignature = resumeSourceSignature(),
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
    onResumeTimestampUs: (Long) -> Unit,
) {
    var resumedSessionId by remember(target?.contentUri, target?.resumeTimestampUs) {
        mutableLongStateOf(NO_SESSION)
    }

    LaunchedEffect(videoState, target) {
        val source = target ?: return@LaunchedEffect
        val ready = videoState as? VideoInspectionState.Ready ?: return@LaunchedEffect
        runCatching {
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
        }
    }

    LaunchedEffect(microscopeState, videoState, target) {
        val source = target ?: return@LaunchedEffect
        val ready = microscopeState as? MicroscopeUiState.Ready ?: return@LaunchedEffect
        val frame = ready.session.currentFrame ?: return@LaunchedEffect
        val resumeTimestampUs = source.resumeTimestampUs
        val inspectedVideo = (videoState as? VideoInspectionState.Ready)?.video

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
        }
    }
}

private const val NO_SESSION = 0L
