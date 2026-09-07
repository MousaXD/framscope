package com.framescope.app.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.framescope.app.data.InspectedVideo

@Composable
fun FrameScopeScreen(
    state: FrameScopeUiState,
    onOpenVideo: () -> Unit,
    onCancelInspection: () -> Unit,
    onDismissError: () -> Unit,
    onStepMicroscope: (Int) -> Unit,
    onJumpMicroscopeFrame: (Long) -> Unit,
    onJumpMicroscopeTimestampUs: (Long) -> Unit,
) {
    Surface(
        modifier = Modifier.fillMaxSize(),
        color = MaterialTheme.colorScheme.background,
    ) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .safeDrawingPadding()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 24.dp, vertical = 28.dp),
            verticalArrangement = Arrangement.spacedBy(18.dp),
        ) {
            Text(
                text = "FrameScope",
                fontSize = 34.sp,
                fontWeight = FontWeight.SemiBold,
                color = MaterialTheme.colorScheme.onBackground,
            )
            Text(
                text = "Open a video and inspect its authoritative presentation timeline frame by frame.",
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )

            EngineStatusCard(state.engineStatus)

            when (val videoState = state.videoState) {
                is VideoInspectionState.Error -> ErrorCard(
                    message = videoState.message,
                    onDismiss = onDismissError,
                )

                is VideoInspectionState.Ready -> VideoDetailsCard(videoState.video)
                VideoInspectionState.Cancelled -> StatusCard("Video inspection cancelled.")
                VideoInspectionState.Picking -> StatusCard("Waiting for Android's video picker…")
                VideoInspectionState.Opening -> BusyCard("Opening selected video…")
                VideoInspectionState.Inspecting -> BusyCard("Inspecting video in Rust…")
                VideoInspectionState.Idle -> Unit
            }

            MicroscopePanel(
                state = state.microscopeState,
                onStep = onStepMicroscope,
                onJumpFrame = onJumpMicroscopeFrame,
                onJumpTimestampUs = onJumpMicroscopeTimestampUs,
                onDismissError = onDismissError,
            )

            val inspectionActive = state.videoState == VideoInspectionState.Opening ||
                state.videoState == VideoInspectionState.Inspecting

            if (inspectionActive) {
                OutlinedButton(
                    onClick = onCancelInspection,
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(56.dp),
                    shape = RoundedCornerShape(16.dp),
                ) {
                    Text("Cancel Inspection")
                }
            } else if (state.videoState != VideoInspectionState.Picking) {
                Button(
                    onClick = onOpenVideo,
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(56.dp),
                    shape = RoundedCornerShape(16.dp),
                ) {
                    Text(
                        if (state.videoState is VideoInspectionState.Ready) {
                            "Open Another Video"
                        } else {
                            "Open Video"
                        },
                    )
                }
            }

            Text(
                text = "Local only · SAF access · No broad storage permission · No telemetry",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.align(Alignment.CenterHorizontally),
            )
        }
    }
}

@Composable
private fun EngineStatusCard(status: EngineStatus) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            val title: String
            val detail: String
            when (status) {
                EngineStatus.Checking -> {
                    title = "Rust engine: checking…"
                    detail = "Loading native engine…"
                }

                is EngineStatus.Ready -> {
                    title = "Rust engine: ready"
                    detail = status.version
                }

                is EngineStatus.Unavailable -> {
                    title = "Rust engine: unavailable"
                    detail = status.message
                }
            }

            Text(
                text = title,
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.Medium,
            )
            Text(
                text = detail,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

@Composable
private fun VideoDetailsCard(video: InspectedVideo) {
    val metadata = video.metadata
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                text = video.displayName,
                style = MaterialTheme.typography.titleLarge,
                fontWeight = FontWeight.SemiBold,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)

            metadata.container?.let { MetadataRow("Container", it) }
            metadata.codec?.let { MetadataRow("Codec", it) }
            MetadataRow("Resolution", "${metadata.width} × ${metadata.height}")
            MetadataRow("Duration", VideoMetadataFormatter.duration(metadata.durationUs))
            metadata.pixelFormat?.let { MetadataRow("Pixel format", it) }
            metadata.videoStreamIndex?.let { MetadataRow("Selected stream", "#$it") }
            metadata.videoStreamCount?.let { MetadataRow("Video streams", it.toString()) }
            metadata.audioStreamCount?.let { MetadataRow("Audio streams", it.toString()) }
            MetadataRow(
                "Nominal / estimated FPS",
                VideoMetadataFormatter.fps(metadata.estimatedFrameRate),
            )
            metadata.variableFrameRate?.let {
                MetadataRow("Frame-rate mode", if (it) "Variable" else "Constant")
            }
            if (metadata.rotationDegrees != 0) {
                MetadataRow("Rotation", "${metadata.rotationDegrees}°")
            }

            Text(
                text = "Frame timing remains timestamp-driven; FPS is informational only.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
            Text(
                text = "Inspected by ${video.engine}",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.secondary,
            )
        }
    }
}

@Composable
private fun MetadataRow(label: String, value: String) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = label,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodyMedium,
            modifier = Modifier.weight(1f),
        )
        Spacer(Modifier.size(12.dp))
        Text(
            text = value,
            style = MaterialTheme.typography.bodyMedium,
            fontWeight = FontWeight.Medium,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
private fun BusyCard(message: String) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        shape = RoundedCornerShape(18.dp),
    ) {
        Row(
            modifier = Modifier.padding(18.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            CircularProgressIndicator(
                modifier = Modifier.size(22.dp),
                strokeWidth = 2.dp,
            )
            Spacer(Modifier.size(12.dp))
            Text(message, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

@Composable
private fun StatusCard(message: String) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        shape = RoundedCornerShape(18.dp),
    ) {
        Text(
            text = message,
            modifier = Modifier.padding(18.dp),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun ErrorCard(message: String, onDismiss: () -> Unit) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                text = "Could not inspect video",
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.onErrorContainer,
                fontWeight = FontWeight.Medium,
            )
            Text(
                text = message,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onErrorContainer,
            )
            OutlinedButton(
                onClick = onDismiss,
                border = BorderStroke(1.dp, MaterialTheme.colorScheme.onErrorContainer),
                colors = ButtonDefaults.outlinedButtonColors(
                    contentColor = MaterialTheme.colorScheme.onErrorContainer,
                ),
            ) {
                Text("Dismiss")
            }
        }
    }
}
