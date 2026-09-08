package com.framescope.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.framescope.app.data.FrameExportFormat

@Composable
fun CurrentFrameExportOverlay(
    microscopeState: MicroscopeUiState,
    exportState: FrameExportUiState,
    onRequestExport: (FrameExportFormat) -> Unit,
    onCancelExport: () -> Unit,
    onDismissStatus: () -> Unit,
) {
    val ready = microscopeState as? MicroscopeUiState.Ready ?: return
    var showFormatDialog by remember { mutableStateOf(false) }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .padding(20.dp),
        contentAlignment = Alignment.BottomEnd,
    ) {
        when (exportState) {
            FrameExportUiState.Idle -> ExtendedFloatingActionButton(
                onClick = { showFormatDialog = true },
            ) {
                Text("Export frame ${ready.session.currentFrame?.frameId ?: ""}")
            }

            is FrameExportUiState.AwaitingDestination -> ExportStatusCard(
                title = "Choose export folder",
                detail = "Android's folder picker is open for frame ${exportState.request.frameId}.",
                onCancel = onCancelExport,
            )

            is FrameExportUiState.Exporting -> ExportStatusCard(
                title = "Exporting frame ${exportState.request.frameId}",
                detail = "Encoding ${formatLabel(exportState.request.format)} at source resolution…",
                busy = true,
                onCancel = onCancelExport,
            )

            is FrameExportUiState.Success -> ExportStatusCard(
                title = "Frame exported",
                detail = "${exportState.document.displayName} · ${formatBytes(exportState.document.export.byteLength)}",
                onDismiss = onDismissStatus,
            )

            is FrameExportUiState.Error -> ExportStatusCard(
                title = "Export failed",
                detail = buildString {
                    append(exportState.message)
                    exportState.code?.let { append(" ($it)") }
                },
                onDismiss = onDismissStatus,
            )
        }
    }

    if (showFormatDialog) {
        AlertDialog(
            onDismissRequest = { showFormatDialog = false },
            title = { Text("Export current frame") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("Choose the image format. Export uses the authoritative source-quality frame.")
                    FormatButton("PNG · lossless RGBA") {
                        showFormatDialog = false
                        onRequestExport(FrameExportFormat.Png)
                    }
                    FormatButton("JPEG · quality 92") {
                        showFormatDialog = false
                        onRequestExport(FrameExportFormat.Jpeg)
                    }
                    FormatButton("WebP · lossless RGBA") {
                        showFormatDialog = false
                        onRequestExport(FrameExportFormat.WebPLossless)
                    }
                }
            },
            confirmButton = {},
            dismissButton = {
                TextButton(onClick = { showFormatDialog = false }) {
                    Text("Cancel")
                }
            },
        )
    }
}

@Composable
private fun FormatButton(
    label: String,
    onClick: () -> Unit,
) {
    OutlinedButton(onClick = onClick) {
        Text(label)
    }
}

@Composable
private fun ExportStatusCard(
    title: String,
    detail: String,
    busy: Boolean = false,
    onCancel: (() -> Unit)? = null,
    onDismiss: (() -> Unit)? = null,
) {
    Card(
        modifier = Modifier.semantics {
            liveRegion = LiveRegionMode.Polite
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
