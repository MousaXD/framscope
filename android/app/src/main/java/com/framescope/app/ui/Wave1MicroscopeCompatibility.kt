package com.framescope.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.runtime.Composable
import androidx.compose.ui.unit.dp
import com.framescope.app.data.MicroscopeSessionSnapshot
import com.framescope.app.data.VideoMetadata

/** Compatibility overload for shell tests and non-live call sites retained from the Wave 1 shell PR. */
@Composable
internal fun MicroscopePanel(
    state: MicroscopeUiState,
    timelineBounds: IndexedTimelineBounds?,
    rangeSelection: TimelineRangeSelection?,
    onStep: (Int) -> Unit,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
    onCommitRange: (Long, Long) -> Unit,
    onClearRange: () -> Unit,
    onDismissError: () -> Unit,
    compactWorkspace: Boolean = false,
) {
    MicroscopePanel(
        state = state,
        timelineBounds = timelineBounds,
        rangeSelection = rangeSelection,
        scrubPreview = null,
        onStep = onStep,
        onJumpFrame = onJumpFrame,
        onJumpTimestampUs = onJumpTimestampUs,
        onPreviewFrame = {},
        onPreviewTimestampUs = {},
        onFinishScrubFrame = onJumpFrame,
        onFinishScrubTimestampUs = onJumpTimestampUs,
        onCommitRange = onCommitRange,
        onClearRange = onClearRange,
        onDismissError = onDismissError,
    )
}

/** #83's shell tests call the pre-live timeline signature; keep it as a thin adapter. */
@Composable
internal fun MicroscopeTimelineControls(
    session: MicroscopeSessionSnapshot,
    timelineBounds: IndexedTimelineBounds?,
    rangeSelection: TimelineRangeSelection?,
    enabled: Boolean,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
    onCommitRange: (Long, Long) -> Unit,
    onClearRange: () -> Unit,
    showImplementationGuidance: Boolean = true,
) {
    MicroscopeTimelineControls(
        session = session,
        timelineBounds = timelineBounds,
        rangeSelection = rangeSelection,
        enabled = enabled,
        onJumpFrame = onJumpFrame,
        onJumpTimestampUs = onJumpTimestampUs,
        onPreviewFrame = {},
        onPreviewTimestampUs = {},
        onFinishScrubFrame = onJumpFrame,
        onFinishScrubTimestampUs = onJumpTimestampUs,
        onCommitRange = onCommitRange,
        onClearRange = onClearRange,
    )
}

@Composable
internal fun MicroscopeInspectorPanel(
    state: MicroscopeUiState,
    metadata: VideoMetadata? = null,
    onStep: (Int) -> Unit,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        MicroscopeInspectorWorkspace(
            state = state,
            metadata = metadata,
            onStep = onStep,
            onJumpFrame = onJumpFrame,
            onJumpTimestampUs = onJumpTimestampUs,
        )
        MicroscopeSimilarityInspectorPanel(
            state = state,
            onJumpFrame = onJumpFrame,
        )
    }
}
