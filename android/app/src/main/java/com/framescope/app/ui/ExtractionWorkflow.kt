package com.framescope.app.ui

import android.net.Uri
import android.os.SystemClock
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilterChip
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.framescope.app.data.BatchExportProgress
import com.framescope.app.data.BatchExportRequest
import com.framescope.app.data.BatchExportSelection
import com.framescope.app.data.FrameExportFormat
import kotlinx.coroutines.delay

@Composable
fun ExtractionWorkflow(
    microscopeState: MicroscopeUiState,
    selectedTimelineRange: TimelineRangeSelection?,
    currentFrameState: FrameExportUiState,
    batchState: BatchExportUiState,
    onRequestCurrentFrame: (FrameExportFormat) -> Unit,
    onRequestBatch: (BatchExportRequest) -> Unit,
    onCancelCurrentFrame: () -> Unit,
    onCancelBatch: () -> Unit,
    onDismissCurrentFrameStatus: () -> Unit,
    onDismissBatchStatus: () -> Unit,
) {
    val ready = microscopeState as? MicroscopeUiState.Ready ?: return
    val selectedRange = selectedTimelineRange?.takeIf {
        it.sessionId == ready.session.sessionId && it.endUs >= it.startUs
    }
    var showSheet by rememberSaveable(ready.session.sessionId) { mutableStateOf(false) }

    when {
        currentFrameState !is FrameExportUiState.Idle -> CurrentFrameStatus(
            state = currentFrameState,
            onCancel = onCancelCurrentFrame,
            onDismiss = onDismissCurrentFrameStatus,
        )
        batchState !is BatchExportUiState.Idle -> BatchStatus(
            state = batchState,
            onCancel = onCancelBatch,
            onDismiss = onDismissBatchStatus,
        )
        else -> Button(
            onClick = { showSheet = true },
            modifier = Modifier
                .fillMaxWidth()
                .testTag("extract_action"),
        ) {
            Text(if (selectedRange == null) "Extract" else "Extract selected range")
        }
    }

    if (showSheet) {
        ExtractionSheet(
            sessionId = ready.session.sessionId,
            currentFrameId = ready.session.currentFrame?.frameId,
            frameCount = ready.session.frameCount,
            selectedTimelineRange = selectedRange,
            onDismiss = { showSheet = false },
            onCurrentFrame = { format ->
                showSheet = false
                onRequestCurrentFrame(format)
            },
            onBatch = { request ->
                showSheet = false
                onRequestBatch(request)
            },
        )
    }
}

@Composable
private fun ExtractionSheet(
    sessionId: Long,
    currentFrameId: Long?,
    frameCount: Long,
    selectedTimelineRange: TimelineRangeSelection?,
    onDismiss: () -> Unit,
    onCurrentFrame: (FrameExportFormat) -> Unit,
    onBatch: (BatchExportRequest) -> Unit,
) {
    val defaultMode = if (selectedTimelineRange != null) {
        ExtractionMode.SelectedTimeline
    } else {
        ExtractionMode.CurrentFrame
    }
    var modeName by rememberSaveable(sessionId) { mutableStateOf(defaultMode.name) }
    val mode = remember(modeName) {
        ExtractionMode.entries.firstOrNull { it.name == modeName } ?: defaultMode
    }
    var startFrame by rememberSaveable(sessionId) { mutableStateOf(currentFrameId?.toString() ?: "0") }
    var endFrame by rememberSaveable(sessionId) {
        mutableStateOf(if (frameCount > 0L) (frameCount - 1L).toString() else "0")
    }
    var startSeconds by rememberSaveable(sessionId) { mutableStateOf("0") }
    var endSeconds by rememberSaveable(sessionId) { mutableStateOf("1") }
    var everyN by rememberSaveable(sessionId) { mutableStateOf("1") }
    var formatName by rememberSaveable(sessionId) { mutableStateOf(FrameExportFormat.Png.name) }
    val format = remember(formatName) {
        FrameExportFormat.entries.firstOrNull { it.name == formatName } ?: FrameExportFormat.Png
    }

    val draft = ExtractionDraft(
        mode = mode,
        selectedTimelineRange = selectedTimelineRange,
        startFrame = startFrame,
        endFrame = endFrame,
        startSeconds = startSeconds,
        endSeconds = endSeconds,
        everyNFrames = everyN,
        format = format,
    )
    val batchRequest = ExtractionRequestFactory.buildBatchRequest(draft)
    val canContinue = when (mode) {
        ExtractionMode.CurrentFrame -> currentFrameId != null
        else -> batchRequest != null
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        modifier = Modifier.testTag("extraction_sheet"),
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 20.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Text(
                text = "Extract frames",
                style = MaterialTheme.typography.headlineSmall,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                text = "Choose what to save. Frame and time ranges stay tied to the indexed presentation timeline.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )

            if (selectedTimelineRange != null) {
                ModeRow(
                    first = ExtractionMode.SelectedTimeline,
                    firstLabel = "Selected range",
                    second = ExtractionMode.CurrentFrame,
                    secondLabel = "Current frame",
                    selected = mode,
                    onSelected = { modeName = it.name },
                )
            } else {
                ModeRow(
                    first = ExtractionMode.CurrentFrame,
                    firstLabel = "Current frame",
                    second = ExtractionMode.AllFrames,
                    secondLabel = "All frames",
                    selected = mode,
                    onSelected = { modeName = it.name },
                )
            }
            ModeRow(
                first = ExtractionMode.FrameRange,
                firstLabel = "Frame range",
                second = ExtractionMode.TimestampRange,
                secondLabel = "Time range",
                selected = mode,
                onSelected = { modeName = it.name },
            )
            if (selectedTimelineRange != null) {
                ModeRow(
                    first = ExtractionMode.AllFrames,
                    firstLabel = "All frames",
                    second = ExtractionMode.UniqueGroups,
                    secondLabel = "Unique groups",
                    selected = mode,
                    onSelected = { modeName = it.name },
                )
            } else {
                SingleModeChip(
                    mode = ExtractionMode.UniqueGroups,
                    label = "Unique groups",
                    selected = mode,
                    onSelected = { modeName = it.name },
                )
            }

            when (mode) {
                ExtractionMode.SelectedTimeline -> selectedTimelineRange?.let { range ->
                    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Text("Selected timeline range", style = MaterialTheme.typography.labelLarge)
                        Text(
                            "${MicroscopePreviewMath.formatTimestampUs(range.startUs)} to " +
                                MicroscopePreviewMath.formatTimestampUs(range.endUs),
                        )
                        Text(
                            "Frames whose indexed presentation timestamp falls inside this range are included.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
                ExtractionMode.CurrentFrame -> Text(
                    currentFrameId?.let { "Current frame ${it + 1}" } ?: "No current frame is available.",
                    style = MaterialTheme.typography.bodyMedium,
                )
                ExtractionMode.FrameRange -> Row(
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    WholeNumberField(
                        value = startFrame,
                        onValueChange = { startFrame = it },
                        label = "Start frame ID",
                        modifier = Modifier.weight(1f),
                    )
                    WholeNumberField(
                        value = endFrame,
                        onValueChange = { endFrame = it },
                        label = "End frame ID",
                        modifier = Modifier.weight(1f),
                    )
                }
                ExtractionMode.TimestampRange -> Row(
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    SecondsField(
                        value = startSeconds,
                        onValueChange = { startSeconds = it },
                        label = "Start (seconds)",
                        modifier = Modifier.weight(1f),
                    )
                    SecondsField(
                        value = endSeconds,
                        onValueChange = { endSeconds = it },
                        label = "End (seconds)",
                        modifier = Modifier.weight(1f),
                    )
                }
                ExtractionMode.AllFrames -> Text(
                    "All $frameCount indexed frames",
                    style = MaterialTheme.typography.bodyMedium,
                )
                ExtractionMode.UniqueGroups -> Text(
                    "One source-quality representative from each validated similarity group.",
                    style = MaterialTheme.typography.bodyMedium,
                )
            }

            if (mode !in setOf(ExtractionMode.CurrentFrame, ExtractionMode.UniqueGroups)) {
                WholeNumberField(
                    value = everyN,
                    onValueChange = { everyN = it },
                    label = "Every Nth matching frame",
                    modifier = Modifier.fillMaxWidth(),
                )
            }

            Text("Format", style = MaterialTheme.typography.labelLarge)
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                FormatChip("PNG", FrameExportFormat.Png, format, Modifier.weight(1f)) {
                    formatName = it.name
                }
                FormatChip("JPEG", FrameExportFormat.Jpeg, format, Modifier.weight(1f)) {
                    formatName = it.name
                }
                FormatChip("WebP", FrameExportFormat.WebPLossless, format, Modifier.weight(1f)) {
                    formatName = it.name
                }
            }
            if (format == FrameExportFormat.Jpeg) {
                Text(
                    "JPEG quality: 92",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }

            if (!canContinue) {
                Text(
                    "Enter a valid range and interval.",
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            Button(
                enabled = canContinue,
                onClick = {
                    if (mode == ExtractionMode.CurrentFrame) {
                        onCurrentFrame(format)
                    } else {
                        batchRequest?.let(onBatch)
                    }
                },
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag("extraction_choose_destination"),
            ) {
                Text("Choose destination")
            }
            TextButton(
                onClick = onDismiss,
                modifier = Modifier.fillMaxWidth(),
            ) { Text("Cancel") }
        }
    }
}

@Composable
private fun ModeRow(
    first: ExtractionMode,
    firstLabel: String,
    second: ExtractionMode,
    secondLabel: String,
    selected: ExtractionMode,
    onSelected: (ExtractionMode) -> Unit,
) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        FilterChip(
            selected = selected == first,
            onClick = { onSelected(first) },
            label = { Text(firstLabel) },
            modifier = Modifier
                .weight(1f)
                .testTag("extract_mode_${first.name}"),
        )
        FilterChip(
            selected = selected == second,
            onClick = { onSelected(second) },
            label = { Text(secondLabel) },
            modifier = Modifier
                .weight(1f)
                .testTag("extract_mode_${second.name}"),
        )
    }
}

@Composable
private fun SingleModeChip(
    mode: ExtractionMode,
    label: String,
    selected: ExtractionMode,
    onSelected: (ExtractionMode) -> Unit,
) {
    FilterChip(
        selected = selected == mode,
        onClick = { onSelected(mode) },
        label = { Text(label) },
        modifier = Modifier
            .fillMaxWidth()
            .testTag("extract_mode_${mode.name}"),
    )
}

@Composable
private fun FormatChip(
    label: String,
    value: FrameExportFormat,
    selected: FrameExportFormat,
    modifier: Modifier,
    onSelected: (FrameExportFormat) -> Unit,
) {
    FilterChip(
        selected = selected == value,
        onClick = { onSelected(value) },
        label = { Text(label) },
        modifier = modifier,
    )
}

@Composable
private fun WholeNumberField(
    value: String,
    onValueChange: (String) -> Unit,
    label: String,
    modifier: Modifier,
) {
    OutlinedTextField(
        value = value,
        onValueChange = { candidate ->
            if (candidate.all(Char::isDigit)) onValueChange(candidate)
        },
        label = { Text(label) },
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
        modifier = modifier,
    )
}

@Composable
private fun SecondsField(
    value: String,
    onValueChange: (String) -> Unit,
    label: String,
    modifier: Modifier,
) {
    OutlinedTextField(
        value = value,
        onValueChange = { candidate ->
            if (candidate.matches(Regex("-?\\d*(\\.\\d{0,6})?"))) onValueChange(candidate)
        },
        label = { Text(label) },
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal),
        modifier = modifier,
    )
}

@Composable
private fun CurrentFrameStatus(
    state: FrameExportUiState,
    onCancel: () -> Unit,
    onDismiss: () -> Unit,
) {
    when (state) {
        FrameExportUiState.Idle -> Unit
        is FrameExportUiState.AwaitingDestination -> ExtractionStatusCard(
            title = "Choose destination",
            detail = "Pick a folder for frame ${state.request.frameId + 1}.",
            onCancel = onCancel,
        )
        is FrameExportUiState.Exporting -> {
            val elapsedMs = rememberElapsedMillis(state.request)
            ExtractionStatusCard(
                title = "Exporting current frame",
                detail = "Frame ${state.request.frameId + 1} · ${formatLabel(state.request.format)} · " +
                    "${formatElapsed(elapsedMs)} elapsed",
                indeterminate = true,
                onCancel = onCancel,
            )
        }
        is FrameExportUiState.Success -> ExtractionStatusCard(
            title = "Extraction complete",
            detail = buildString {
                append("1 frame · ${formatLabel(state.document.export.format)} · ")
                append(formatBytes(state.document.export.byteLength))
                append("\n${state.document.displayName}")
                append(" · ${destinationLabel(state.document.uri)}")
            },
            onDismiss = onDismiss,
        )
        is FrameExportUiState.Error -> ExtractionStatusCard(
            title = "Extraction failed",
            detail = state.code?.let { "${state.message} ($it)" } ?: state.message,
            onDismiss = onDismiss,
        )
    }
}

@Composable
private fun BatchStatus(
    state: BatchExportUiState,
    onCancel: () -> Unit,
    onDismiss: () -> Unit,
) {
    when (state) {
        BatchExportUiState.Idle -> Unit
        is BatchExportUiState.AwaitingDestination -> ExtractionStatusCard(
            title = "Choose destination",
            detail = selectionLabel(state.pending.request.selection),
            onCancel = onCancel,
        )
        is BatchExportUiState.Exporting -> {
            val elapsedMs = rememberElapsedMillis(state.pending)
            val progress = state.progress?.takeIf(BatchExportProgress::isSane)
            val detail = if (progress == null) {
                "Preparing ${selectionLabel(state.pending.request.selection).lowercase()} · ${formatElapsed(elapsedMs)} elapsed"
            } else {
                val percent = (progress.ordinal.toDouble() / progress.total.toDouble()).coerceIn(0.0, 1.0)
                "${progress.ordinal} / ${progress.total} · ${(percent * 100).toInt()}% · " +
                    "frame ${progress.frameId + 1} · ${formatElapsed(elapsedMs)} elapsed"
            }
            ExtractionStatusCard(
                title = "Extracting frames",
                detail = detail,
                progress = progress,
                indeterminate = progress == null,
                onCancel = onCancel,
            )
        }
        is BatchExportUiState.Success -> ExtractionStatusCard(
            title = "Extraction complete",
            detail = buildString {
                append("${state.document.export.committedFrames} frames · ")
                append("${formatLabel(state.document.export.format)} · ")
                append(formatBytes(state.document.export.encodedBytes))
                append("\nManifest: ${state.document.manifestDisplayName}")
                append(" · ${destinationLabel(state.document.workspaceUri ?: state.document.manifestUri)}")
            },
            onDismiss = onDismiss,
        )
        is BatchExportUiState.Error -> ExtractionStatusCard(
            title = "Extraction failed",
            detail = state.code?.let { "${state.message} ($it)" } ?: state.message,
            onDismiss = onDismiss,
        )
    }
}

@Composable
private fun rememberElapsedMillis(key: Any): Long {
    val startedAt = remember(key) { SystemClock.elapsedRealtime() }
    val elapsed by produceState(initialValue = 0L, key1 = key) {
        while (true) {
            value = (SystemClock.elapsedRealtime() - startedAt).coerceAtLeast(0L)
            delay(500L)
        }
    }
    return elapsed
}

@Composable
private fun ExtractionStatusCard(
    title: String,
    detail: String,
    progress: BatchExportProgress? = null,
    indeterminate: Boolean = false,
    onCancel: (() -> Unit)? = null,
    onDismiss: (() -> Unit)? = null,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("extraction_status")
            .semantics {
                liveRegion = LiveRegionMode.Polite
                progress?.takeIf(BatchExportProgress::isSane)?.let { value ->
                    progressBarRangeInfo = ProgressBarRangeInfo(
                        current = value.ordinal.toFloat(),
                        range = 0f..value.total.toFloat(),
                        steps = (value.total - 1L).coerceIn(0L, Int.MAX_VALUE.toLong()).toInt(),
                    )
                }
            },
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerHigh),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Row(
                horizontalArrangement = Arrangement.spacedBy(10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (indeterminate) {
                    CircularProgressIndicator(strokeWidth = 2.dp)
                }
                Text(title, style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold)
            }
            progress?.takeIf(BatchExportProgress::isSane)?.let { value ->
                LinearProgressIndicator(
                    progress = { value.ordinal.toFloat() / value.total.toFloat() },
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            Text(
                detail,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            onCancel?.let { cancel ->
                TextButton(onClick = cancel) { Text("Cancel extraction") }
            }
            onDismiss?.let { dismiss ->
                TextButton(onClick = dismiss) { Text("Done") }
            }
        }
    }
}

private fun selectionLabel(selection: BatchExportSelection): String = when (selection) {
    is BatchExportSelection.CurrentFrame -> "Current frame"
    is BatchExportSelection.FrameRangeInclusive -> "Frame range ${selection.startFrameId + 1}-${selection.endFrameId + 1}"
    is BatchExportSelection.TimestampRangeUsInclusive -> "Selected time range"
    BatchExportSelection.AllFrames -> "All frames"
    BatchExportSelection.UniqueGroups -> "Unique groups"
}

private fun formatLabel(format: FrameExportFormat): String = when (format) {
    FrameExportFormat.Png -> "PNG"
    FrameExportFormat.Jpeg -> "JPEG"
    FrameExportFormat.WebPLossless -> "WebP"
}

private fun formatBytes(bytes: Long): String = when {
    bytes >= 1024L * 1024L -> "%.1f MiB".format(bytes.toDouble() / (1024.0 * 1024.0))
    bytes >= 1024L -> "%.1f KiB".format(bytes.toDouble() / 1024.0)
    else -> "$bytes B"
}

private fun formatElapsed(elapsedMs: Long): String {
    val totalSeconds = elapsedMs / 1_000L
    val minutes = totalSeconds / 60L
    val seconds = totalSeconds % 60L
    return if (minutes > 0L) "${minutes}m ${seconds}s" else "${seconds}s"
}

private fun destinationLabel(uri: String): String {
    val parsed = runCatching { Uri.parse(uri) }.getOrNull()
    val segment = parsed?.lastPathSegment?.let(Uri::decode)?.takeIf { it.isNotBlank() }
    return segment?.let { "Destination: $it" } ?: "Saved to selected destination"
}
