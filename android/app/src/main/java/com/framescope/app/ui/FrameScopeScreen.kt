package com.framescope.app.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
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
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
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
    onCommitMicroscopeRange: (Long, Long) -> Unit,
    onClearMicroscopeRange: () -> Unit,
    workspaceOverlay: @Composable () -> Unit = {},
    recentVideosContent: @Composable () -> Unit = { RecentVideosIntegrationPoint() },
    storageSummaryContent: @Composable () -> Unit = { StorageSummaryIntegrationPoint() },
    historyContent: @Composable () -> Unit = { HistoryIntegrationPoint() },
    storageContent: @Composable () -> Unit = { StorageIntegrationPoint() },
) {
    val navController = rememberNavController()
    val backStackEntry by navController.currentBackStackEntryAsState()
    val currentDestination = FrameScopeDestination.fromRoute(backStackEntry?.destination?.route)
    var videoWasReady by rememberSaveable {
        mutableStateOf(state.videoState is VideoInspectionState.Ready)
    }

    LaunchedEffect(state.videoState) {
        val videoIsReady = state.videoState is VideoInspectionState.Ready
        if (videoIsReady && !videoWasReady) {
            navController.navigateTopLevel(FrameScopeDestination.Workspace)
        }
        videoWasReady = videoIsReady
    }

    FrameScopeAppShell(
        currentDestination = currentDestination,
        onDestinationSelected = navController::navigateTopLevel,
    ) { innerPadding ->
        NavHost(
            navController = navController,
            startDestination = FrameScopeDestination.Home.route,
            modifier = Modifier
                .fillMaxSize()
                .padding(innerPadding),
        ) {
            composable(FrameScopeDestination.Home.route) {
                HomeScreen(
                    state = state,
                    onOpenVideo = onOpenVideo,
                    onOpenWorkspace = {
                        navController.navigateTopLevel(FrameScopeDestination.Workspace)
                    },
                    onDismissError = onDismissError,
                    recentVideosContent = recentVideosContent,
                    storageSummaryContent = storageSummaryContent,
                )
            }
            composable(FrameScopeDestination.Workspace.route) {
                VideoWorkspaceScreen(
                    state = state,
                    onOpenVideo = onOpenVideo,
                    onCancelInspection = onCancelInspection,
                    onDismissError = onDismissError,
                    onStepMicroscope = onStepMicroscope,
                    onJumpMicroscopeFrame = onJumpMicroscopeFrame,
                    onJumpMicroscopeTimestampUs = onJumpMicroscopeTimestampUs,
                    onCommitMicroscopeRange = onCommitMicroscopeRange,
                    onClearMicroscopeRange = onClearMicroscopeRange,
                    onOpenInspector = {
                        navController.navigateTopLevel(FrameScopeDestination.Inspector)
                    },
                    workspaceOverlay = workspaceOverlay,
                )
            }
            composable(FrameScopeDestination.Inspector.route) {
                InspectorScreen(
                    state = state,
                    onOpenVideo = onOpenVideo,
                    onDismissError = onDismissError,
                    onStepMicroscope = onStepMicroscope,
                    onJumpMicroscopeFrame = onJumpMicroscopeFrame,
                    onJumpMicroscopeTimestampUs = onJumpMicroscopeTimestampUs,
                )
            }
            composable(FrameScopeDestination.History.route) {
                IntegrationDestination(
                    modifier = Modifier.testTag("history_destination"),
                    content = historyContent,
                )
            }
            composable(FrameScopeDestination.Storage.route) {
                IntegrationDestination(
                    modifier = Modifier.testTag("storage_destination"),
                    content = storageContent,
                )
            }
        }
    }
}

private fun NavHostController.navigateTopLevel(destination: FrameScopeDestination) {
    navigate(destination.route) {
        launchSingleTop = true
        restoreState = true
        popUpTo(FrameScopeDestination.Home.route) {
            saveState = true
        }
    }
}

@Composable
private fun HomeScreen(
    state: FrameScopeUiState,
    onOpenVideo: () -> Unit,
    onOpenWorkspace: () -> Unit,
    onDismissError: () -> Unit,
    recentVideosContent: @Composable () -> Unit,
    storageSummaryContent: @Composable () -> Unit,
) {
    LazyColumn(
        modifier = Modifier
            .fillMaxSize()
            .testTag("home_destination"),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        item {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    text = "Inspect video, frame by frame.",
                    style = MaterialTheme.typography.headlineMedium,
                    fontWeight = FontWeight.SemiBold,
                )
                Text(
                    text = "Open a local video to enter the workspace.",
                    style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        item {
            Button(
                onClick = onOpenVideo,
                modifier = Modifier
                    .fillMaxWidth()
                    .height(56.dp)
                    .testTag("home_open_video"),
                shape = RoundedCornerShape(16.dp),
            ) {
                Text("Open video")
            }
        }
        when (val videoState = state.videoState) {
            is VideoInspectionState.Ready -> item {
                ContinueVideoCard(
                    video = videoState.video,
                    onOpenWorkspace = onOpenWorkspace,
                )
            }
            is VideoInspectionState.Error -> item {
                ErrorCard(
                    title = "Could not open video",
                    message = videoState.message,
                    diagnostic = videoState.diagnostic,
                    showDiagnostic = false,
                    onDismiss = onDismissError,
                )
            }
            VideoInspectionState.Picking -> item { StatusCard("Choose a video from the Android picker.") }
            VideoInspectionState.Opening -> item { BusyCard("Opening video…") }
            VideoInspectionState.Inspecting -> item { BusyCard("Reading video details…") }
            VideoInspectionState.Cancelled -> item { StatusCard("Video opening was cancelled.") }
            VideoInspectionState.Idle -> Unit
        }
        item { CompactEngineStatus(state.engineStatus) }
        item { recentVideosContent() }
        item { storageSummaryContent() }
        item {
            Text(
                text = "Local files stay on this device. FrameScope uses Android document access and does not require broad storage permission.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun ContinueVideoCard(
    video: InspectedVideo,
    onOpenWorkspace: () -> Unit,
) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainer),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                text = "Continue",
                style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.primary,
            )
            Text(
                text = video.displayName,
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                text = buildString {
                    append("${video.metadata.width} × ${video.metadata.height}")
                    video.metadata.durationUs?.let { duration ->
                        append(" · ${VideoMetadataFormatter.duration(duration)}")
                    }
                },
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            TextButton(onClick = onOpenWorkspace) {
                Text("Open workspace")
            }
        }
    }
}

@Composable
private fun VideoWorkspaceScreen(
    state: FrameScopeUiState,
    onOpenVideo: () -> Unit,
    onCancelInspection: () -> Unit,
    onDismissError: () -> Unit,
    onStepMicroscope: (Int) -> Unit,
    onJumpMicroscopeFrame: (Long) -> Unit,
    onJumpMicroscopeTimestampUs: (Long) -> Unit,
    onCommitMicroscopeRange: (Long, Long) -> Unit,
    onClearMicroscopeRange: () -> Unit,
    onOpenInspector: () -> Unit,
    workspaceOverlay: @Composable () -> Unit,
) {
    Box(
        modifier = Modifier
            .fillMaxSize()
            .testTag("workspace_destination"),
    ) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            when (val videoState = state.videoState) {
                is VideoInspectionState.Ready -> {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Column(modifier = Modifier.weight(1f)) {
                            Text(
                                text = videoState.video.displayName,
                                style = MaterialTheme.typography.titleMedium,
                                fontWeight = FontWeight.SemiBold,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            Text(
                                text = videoState.video.metadata.durationUs?.let(VideoMetadataFormatter::duration)
                                    ?: "Duration unavailable",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                        Spacer(Modifier.size(12.dp))
                        TextButton(onClick = onOpenVideo) {
                            Text("Open another")
                        }
                    }

                    MicroscopePanel(
                        state = state.microscopeState,
                        timelineBounds = state.timelineBounds,
                        rangeSelection = state.timelineRange,
                        onStep = onStepMicroscope,
                        onJumpFrame = onJumpMicroscopeFrame,
                        onJumpTimestampUs = onJumpMicroscopeTimestampUs,
                        onCommitRange = onCommitMicroscopeRange,
                        onClearRange = onClearMicroscopeRange,
                        onDismissError = onDismissError,
                        compactWorkspace = true,
                    )

                    OutlinedButton(
                        onClick = onOpenInspector,
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Text("Inspector & exact jumps")
                    }
                }
                is VideoInspectionState.Error -> {
                    WorkspaceEmptyState(
                        title = "Video unavailable",
                        message = videoState.message,
                        actionLabel = "Choose another video",
                        onAction = onOpenVideo,
                    )
                }
                VideoInspectionState.Opening,
                VideoInspectionState.Inspecting,
                -> {
                    BusyCard(
                        if (videoState == VideoInspectionState.Opening) {
                            "Opening video…"
                        } else {
                            "Preparing video workspace…"
                        },
                    )
                    OutlinedButton(
                        onClick = onCancelInspection,
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Text("Cancel")
                    }
                }
                VideoInspectionState.Picking -> StatusCard("Choose a video from the Android picker.")
                VideoInspectionState.Cancelled,
                VideoInspectionState.Idle,
                -> WorkspaceEmptyState(
                    title = "No video open",
                    message = "Choose a video to start frame-accurate inspection.",
                    actionLabel = "Open video",
                    onAction = onOpenVideo,
                )
            }
        }
        workspaceOverlay()
    }
}

@Composable
private fun WorkspaceEmptyState(
    title: String,
    message: String,
    actionLabel: String,
    onAction: () -> Unit,
) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainer),
    ) {
        Column(
            modifier = Modifier.padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(title, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
            Text(
                message,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Button(onClick = onAction) {
                Text(actionLabel)
            }
        }
    }
}

@Composable
private fun InspectorScreen(
    state: FrameScopeUiState,
    onOpenVideo: () -> Unit,
    onDismissError: () -> Unit,
    onStepMicroscope: (Int) -> Unit,
    onJumpMicroscopeFrame: (Long) -> Unit,
    onJumpMicroscopeTimestampUs: (Long) -> Unit,
) {
    LazyColumn(
        modifier = Modifier
            .fillMaxSize()
            .testTag("inspector_destination"),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(16.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        item { EngineStatusCard(state.engineStatus, showVersion = true) }

        when (val videoState = state.videoState) {
            is VideoInspectionState.Ready -> {
                item { VideoDetailsCard(videoState.video) }
                item {
                    MicroscopeInspectorPanel(
                        state = state.microscopeState,
                        onStep = onStepMicroscope,
                        onJumpFrame = onJumpMicroscopeFrame,
                        onJumpTimestampUs = onJumpMicroscopeTimestampUs,
                    )
                }
            }
            is VideoInspectionState.Error -> item {
                ErrorCard(
                    title = "Video diagnostic",
                    message = videoState.message,
                    diagnostic = videoState.diagnostic,
                    showDiagnostic = true,
                    onDismiss = onDismissError,
                )
            }
            else -> item {
                WorkspaceEmptyState(
                    title = "Nothing to inspect yet",
                    message = "Open a video to view codec, timing, frame flags, and exact jump tools.",
                    actionLabel = "Open video",
                    onAction = onOpenVideo,
                )
            }
        }
    }
}

@Composable
private fun IntegrationDestination(
    modifier: Modifier = Modifier,
    content: @Composable () -> Unit,
) {
    Box(
        modifier = modifier
            .fillMaxSize()
            .padding(20.dp),
    ) {
        content()
    }
}

@Composable
private fun RecentVideosIntegrationPoint() {
    IntegrationCard(
        title = "Recent videos",
        message = "Recent sessions will appear here once local history is connected.",
        modifier = Modifier.testTag("home_recent_videos_integration"),
    )
}

@Composable
private fun StorageSummaryIntegrationPoint() {
    IntegrationCard(
        title = "Storage",
        message = "Cache and index usage will appear here when storage management is connected.",
        modifier = Modifier.testTag("home_storage_summary_integration"),
    )
}

@Composable
private fun HistoryIntegrationPoint() {
    IntegrationCard(
        title = "Recent video history",
        message = "History persistence is not connected yet. This destination is ready for the local history module.",
        modifier = Modifier.testTag("history_integration_point"),
    )
}

@Composable
private fun StorageIntegrationPoint() {
    IntegrationCard(
        title = "Storage & cache",
        message = "Storage controls are not connected yet. This destination is ready for the cache-management module.",
        modifier = Modifier.testTag("storage_integration_point"),
    )
}

@Composable
private fun IntegrationCard(
    title: String,
    message: String,
    modifier: Modifier = Modifier,
) {
    Card(
        modifier = modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainer),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(title, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            Text(
                message,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun CompactEngineStatus(status: EngineStatus) {
    val (title, detail) = when (status) {
        EngineStatus.Checking -> "Engine starting" to "Native video engine is loading."
        is EngineStatus.Ready -> "Ready" to "Video analysis engine available."
        is EngineStatus.Unavailable -> "Engine unavailable" to status.message
    }
    IntegrationCard(
        title = title,
        message = detail,
        modifier = Modifier.testTag("home_engine_status"),
    )
}

@Composable
private fun EngineStatusCard(
    status: EngineStatus,
    showVersion: Boolean,
) {
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
                    title = "Native engine"
                    detail = "Loading"
                }
                is EngineStatus.Ready -> {
                    title = "Native engine"
                    detail = if (showVersion) status.version else "Ready"
                }
                is EngineStatus.Unavailable -> {
                    title = "Native engine unavailable"
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
                maxLines = if (showVersion) 4 else 2,
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
                text = "Frame timing is timestamp-driven. FPS is informational only.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
            Text(
                text = "Engine: ${video.engine}",
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
private fun ErrorCard(
    title: String,
    message: String,
    diagnostic: String?,
    showDiagnostic: Boolean,
    onDismiss: () -> Unit,
) {
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
                text = title,
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.onErrorContainer,
                fontWeight = FontWeight.Medium,
            )
            Text(
                text = message,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onErrorContainer,
            )
            if (showDiagnostic) {
                diagnostic?.let {
                    Text(
                        text = "Diagnostic: $it",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onErrorContainer,
                    )
                }
            }
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
