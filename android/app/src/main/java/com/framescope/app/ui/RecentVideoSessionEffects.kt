package com.framescope.app.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import com.framescope.app.data.RecentVideoHistory
import com.framescope.app.data.VideoUriPermissionStatus

data class RecentVideoOpenTarget(
    val contentUri: String,
    val permissionStatus: VideoUriPermissionStatus,
    val resumeTimestampUs: Long? = null,
    val replacesRecordId: String? = null,
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

    LaunchedEffect(microscopeState, target) {
        val source = target ?: return@LaunchedEffect
        val ready = microscopeState as? MicroscopeUiState.Ready ?: return@LaunchedEffect
        val frame = ready.session.currentFrame ?: return@LaunchedEffect

        runCatching {
            history.updatePosition(
                contentUri = source.contentUri,
                frameId = frame.frameId,
                timestampUs = frame.timestampUs,
            )
        }

        val resumeTimestampUs = source.resumeTimestampUs ?: return@LaunchedEffect
        if (resumedSessionId == ready.session.sessionId) return@LaunchedEffect

        // Mark before requesting navigation so recomposition cannot issue the same resume twice.
        resumedSessionId = ready.session.sessionId
        onResumeTimestampUs(resumeTimestampUs)
    }

    LaunchedEffect(microscopeState, target?.contentUri) {
        if (microscopeState is MicroscopeUiState.Idle || microscopeState is MicroscopeUiState.Error) {
            resumedSessionId = NO_SESSION
        }
    }
}

private const val NO_SESSION = 0L
