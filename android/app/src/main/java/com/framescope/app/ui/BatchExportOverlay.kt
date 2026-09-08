package com.framescope.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardOptions
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.framescope.app.data.BatchExportRequest
import com.framescope.app.data.BatchExportSelection
import com.framescope.app.data.FrameExportFormat

private enum class BatchSelectionMode {
    CurrentFrame,
    FrameRange,
    TimestampRange,
    AllFrames,
}

@Composable
fun BatchExportOverlay(
    microscopeState: MicroscopeUiState,
    exportState: BatchExportUiState,
    onRequestExport: (BatchExportRequest) -> Unit,
    onCancelExport: () -> Unit,
    onDismissStatus: () -> Unit,
) {
    val ready = microscopeState as? MicroscopeUiState.Ready ?: return
    var showDialog by remember(ready.session.sessionId) { mutableStateOf(false) }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .padding(20.dp),
        contentAlignment = Alignment.BottomStart,
    ) {
        when (exportState) {
            BatchExportUiState.Idle -> ExtendedFloatingActionButton(
                onClick = { showDialog = true },
            ) {
                Text("Export frames")
            }

            is BatchExportUiState.AwaitingDestination -> BatchExportStatusCard(
                title = "Choose batch export folder",
                detail = selectionLabel(exportState.pending.request.selection),
                onCancel = onCancelExport,
            )

            is BatchExportUiState.Exporting -> {
                val progress = exportState.progress
                BatchExportStatusCard(
                    title = if (progress == null) {
                        "Preparing batch export"
                    } else {
                        "Exporting ${progress.ordinal} / ${progress.total}"
                    },
                    detail = if (progress == null) {
                        "Opening the indexed source-quality extraction pipeline…"
                    } else {
                        "Frame ${progress.frameId} · ${formatLabel(exportState.pending.request.format)}"
                    },
                    busy = true,
                    onCancel = onCancelExport,
                )
            }

            is BatchExportUiState.Success -> BatchExportStatusCard(
                title = "Batch export complete",
                detail = buildString {
                    append("${exportState.document.export.committedFrames} frames")
                    append(" · ${formatBytes(exportState.document.export.encodedBytes)}")
                    append(" · ${exportState.document.manifestDisplayName}")
                },
                onDismiss = onDismissStatus,
            )

            is BatchExportUiState.Error -> BatchExportStatusCard(
                title = "Batch export failed",
                detail = buildString {
                    append(exportState.message)
                    exportState.code?.let { append(" ($it)") }
                },
                onDismiss = onDismissStatus,
            )
        }
    }

    if (showDialog) {
        BatchExportDialog(
            currentFrameId = ready.session.currentFrame?.frameId,
            frameCount = ready.session.frameCount,
            onDismiss = { showDialog = false },
            onConfirm = { request ->
                showDialog = false
                onRequestExport(request)
            },
        )
    }
}

@Composable
private fun BatchExportDialog(
    currentFrameId: Long?,
    frameCount: Long,
    onDismiss: () -> Unit,
    onConfirm: (BatchExportRequest) -> Unit,
) {
    var mode by remember { mutableStateOf(BatchSelectionMode.AllFrames) }
    var startFrame by remember { mutableStateOf(currentFrameId?.toString() ?: "0") }
    var endFrame by remember {
        mutableStateOf(
            when {
                frameCount > 0L -> (frameCount - 1L).toString()
                else -> "0"
            },
        )
    }
    var startUs by remember { mutableStateOf("0") }
    var endUs by remember { mutableStateOf("0") }
    var everyN by remember { mutableStateOf("1") }
    var format by remember { mutableStateOf(FrameExportFormat.Png) }

    val request = buildRequest(
        mode = mode,
        currentFrameId = currentFrameId,
        startFrame = startFrame,
        endFrame = endFrame,
        startUs = startUs,
        endUs = endUs,
        everyN = everyN,
        format = format,
    )

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Export frames") },
        text = {
            Column(
                verticalArrangement = Arrangement.spacedBy(12.dp),
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text(
                    "Choose an indexed selection. Frame ranges use exact FrameIds and time ranges use indexed presentation timestamps.",
                    style = MaterialTheme.typography.bodySmall,
                )

                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    ModeButton("Current", mode == BatchSelectionMode.CurrentFrame) {
                        mode = BatchSelectionMode.CurrentFrame
                    }
                    ModeButton("Range", mode == BatchSelectionMode.FrameRange) {
                        mode = BatchSelectionMode.FrameRange
                    }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    ModeButton("Time µs", mode == BatchSelectionMode.TimestampRange) {
                        mode = BatchSelectionMode.TimestampRange
                    }
                    ModeButton("All", mode == BatchSelectionMode.AllFrames) {
                        mode = BatchSelectionMode.AllFrames
                    }
                }

                when (mode) {
                    BatchSelectionMode.CurrentFrame -> Text(
                        text = currentFrameId?.let { "Frame $it" } ?: "No current frame",
                        style = MaterialTheme.typography.bodyMedium,
                    )

                    BatchSelectionMode.FrameRange -> Row(
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        NumberField(
                            value = startFrame,
                            onValueChange = { startFrame = it },
                            label = "Start frame",
                            modifier = Modifier.weight(1f),
                        )
                        NumberField(
                            value = endFrame,
                            onValueChange = { endFrame = it },
                            label = "End frame",
                            modifier = Modifier.weight(1f),
                        )
                    }

                    BatchSelectionMode.TimestampRange -> Row(
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        SignedNumberField(
                            value = startUs,
                            onValueChange = { startUs = it },
                            label = "Start µs",
                            modifier = Modifier.weight(1f),
                        )
                        SignedNumberField(
                            value = endUs,
                            onValueChange = { endUs = it },
                            label = "End µs",
                            modifier = Modifier.weight(1f),
                        )
                    }

                    BatchSelectionMode.AllFrames -> Text(
                        text = "All $frameCount indexed frames",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }

                NumberField(
                    value = everyN,
                    onValueChange = { everyN = it },
                    label = "Every N frames",
                    modifier = Modifier.fillMaxWidth(),
                )

                Text("Format", style = MaterialTheme.typography.labelLarge)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    FormatChoice("PNG", format == FrameExportFormat.Png) {
                        format = FrameExportFormat.Png
                    }
                    FormatChoice("JPEG", format == FrameExportFormat.Jpeg) {
                        format = FrameExportFormat.Jpeg
                    }
                    FormatChoice("WebP", format == FrameExportFormat.WebPLossless) {
                        format = FrameExportFormat.WebPLossless
                    }
                }
                if (format == FrameExportFormat.Jpeg) {
                    Text(
                        "JPEG uses quality 92.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (request == null) {
                    Text(
                        "Enter a valid selection and a positive frame interval.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }
        },
        confirmButton = {
            Button(
                enabled = request != null,
                onClick = { request?.let(onConfirm) },
            ) {
                Text("Choose folder")
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text("Cancel")
            }
        },
    )
}

@Composable
private fun ModeButton(
    label: String,
    selected: Boolean,
    onClick: () -> Unit,
) {
    if (selected) {
        Button(onClick = onClick) { Text(label) }
    } else {
        OutlinedButton(onClick = onClick) { Text(label) }
    }
}

@Composable
private fun FormatChoice(
    label: String,
    selected: Boolean,
    onClick: () -> Unit,
) {
    ModeButton(label = label, selected = selected, onClick = onClick)
}

@Composable
private fun NumberField(
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
private fun SignedNumberField(
    value: String,
    onValueChange: (String) -> Unit,
    label: String,
    modifier: Modifier,
) {
    OutlinedTextField(
        value = value,
        onValueChange = { candidate ->
            val body = candidate.removePrefix("-")
            if (body.all(Char::isDigit) && candidate.count { it == '-' } <= 1 && !candidate.drop(1).contains('-')) {
                onValueChange(candidate)
            }
        },
        label = { Text(label) },
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
        modifier = modifier,
    )
}

private fun buildRequest(
    mode: BatchSelectionMode,
    currentFrameId: Long?,
    startFrame: String,
    endFrame: String,
    startUs: String,
    endUs: String,
    everyN: String,
    format: FrameExportFormat,
): BatchExportRequest? {
    val stride = everyN.toLongOrNull()?.takeIf { it > 0L } ?: return null
    val selection = when (mode) {
        BatchSelectionMode.CurrentFrame -> BatchExportSelection.CurrentFrame(
            currentFrameId ?: return null,
        )

        BatchSelectionMode.FrameRange -> {
            val start = startFrame.toLongOrNull()?.takeIf { it >= 0L } ?: return null
            val end = endFrame.toLongOrNull()?.takeIf { it >= start } ?: return null
            BatchExportSelection.FrameRangeInclusive(start, end)
        }

        BatchSelectionMode.TimestampRange -> {
            val start = startUs.toLongOrNull() ?: return null
            val end = endUs.toLongOrNull()?.takeIf { it >= start } ?: return null
            BatchExportSelection.TimestampRangeUsInclusive(start, end)
        }

        BatchSelectionMode.AllFrames -> BatchExportSelection.AllFrames
    }
    return BatchExportRequest(
        selection = selection,
        everyNFrames = stride,
        format = format,
    ).takeIf(BatchExportRequest::isSane)
}

@Composable
private fun BatchExportStatusCard(
    title: String,
    detail: String,
    busy: Boolean = false,
    onCancel: (() -> Unit)? = null,
    onDismiss: (() -> Unit)? = null,
) {
    Card(
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
                if (busy) {
                    CircularProgressIndicator(
                        modifier = Modifier.size(20.dp),
                        strokeWidth = 2.dp,
                    )
                }
                Text(
                    text = title,
                    style = MaterialTheme.typography.titleSmall,
                    fontWeight = FontWeight.SemiBold,
                )
            }
            Text(
                text = detail,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            onCancel?.let { cancel ->
                TextButton(onClick = cancel) {
                    Text("Cancel export")
                }
            }
            onDismiss?.let { dismiss ->
                TextButton(onClick = dismiss) {
                    Text("Dismiss")
                }
            }
        }
    }
}

private fun selectionLabel(selection: BatchExportSelection): String = when (selection) {
    is BatchExportSelection.CurrentFrame -> "Frame ${selection.frameId}"
    is BatchExportSelection.FrameRangeInclusive ->
        "Frames ${selection.startFrameId}–${selection.endFrameId}"
    is BatchExportSelection.TimestampRangeUsInclusive ->
        "${selection.startUs}–${selection.endUs} µs"
    BatchExportSelection.AllFrames -> "All indexed frames"
}

private fun formatLabel(format: FrameExportFormat): String = when (format) {
    FrameExportFormat.Png -> "PNG"
    FrameExportFormat.Jpeg -> "JPEG"
    FrameExportFormat.WebPLossless -> "lossless WebP"
}

private fun formatBytes(bytes: Long): String = when {
    bytes >= 1024L * 1024L -> "%.1f MiB".format(bytes.toDouble() / (1024.0 * 1024.0))
    bytes >= 1024L -> "%.1f KiB".format(bytes.toDouble() / 1024.0)
    else -> "$bytes B"
}
