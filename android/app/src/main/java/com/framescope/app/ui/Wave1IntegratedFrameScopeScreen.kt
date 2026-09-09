package com.framescope.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
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
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
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

/**
 * Wave 1 production composition. The extra live-scrub callbacks intentionally distinguish this
 * overload from the original shell contract so the #83 shell remains testable while production
 * combines #82, #83, #84 and #85 without replacing any destination wholesale.
 */
@Composable
fun FrameScopeScreen(
    state: FrameScopeUiState,
    onOpenVideo: () -> Unit,
    onCancelInspection: () -> Unit,
    onDismissError: () -> Unit,
    onStepMicroscope: (Int) -> Unit,
    onJumpMicroscopeFrame: (Long) -> Unit,
    onJumpMicroscopeTimestampUs: (Long) -> Unit,
    onPreviewMicroscopeFrame: (Long) -> Unit,
    onPreviewMicroscopeTimestampUs: (Long) -> Unit,
    onFinishMicroscopeScrubFrame: (Long) -> Unit,
    onFinishMicroscopeScrubTimestampUs: (Long) -> Unit,
    onCommitMicroscopeRange: (Long, Long) -> Unit,
    onClearMicroscopeRange: () -> Unit,
    workspaceExtractionContent: @Composable () -> Unit = {},
    workspaceOverlay: @Composable () -> Unit = {},
    recentVideosContent: @Composable () -> Unit,
    storageSummaryContent: @Composable () -> Unit,
    historyContent: @Composable () -> Unit,
    storageContent: @Composable () -> Unit,
) {
    val navController = rememberNavController()
    val backStackEntry by navController.currentBackStackEntryAsState()
    val currentDestination = FrameScopeDestination.fromRoute(backStackEntry?.destination?.route)
    var videoWasReady by rememberSaveable {
        mutableStateOf(state.videoState is VideoInspectionState.Ready)
    }

    LaunchedEffect(state.videoState) {
        val ready = state.videoState is VideoInspectionState.Ready
        if (ready && !videoWasReady) {
            navController.navigateWave1TopLevel(FrameScopeDestination.Workspace)
        }
        videoWasReady = ready
    }

    FrameScopeAppShell(
        currentDestination = currentDestination,
        onDestinationSelected = navController::navigateWave1TopLevel,
    ) { innerPadding ->
        NavHost(
            navController = navController,
            startDestination = FrameScopeDestination.Home.route,
            modifier = Modifier
                .fillMaxSize()
                .padding(innerPadding),
        ) {
            composable(FrameScopeDestination.Home.route) {
                Wave1HomeScreen(
                    state = state,
                    onOpenVideo = onOpenVideo,
                    onOpenWorkspace = {
                        navController.navigateWave1TopLevel(FrameScopeDestination.Workspace)
                    },
                    recentVideosContent = recentVideosContent,
                    storageSummaryContent = storageSummaryContent,
                )
            }
            composable(FrameScopeDestination.Workspace.route) {
                Wave1WorkspaceScreen(
                    state = state,
                    onOpenVideo = onOpenVideo,
                    onCancelInspection = onCancelInspection,
                    onDismissError = onDismissError,
                    onStep = onStepMicroscope,
                    onJumpFrame = onJumpMicroscopeFrame,
                    onJumpTimestampUs = onJumpMicroscopeTimestampUs,
                    onPreviewFrame = onPreviewMicroscopeFrame,
                    onPreviewTimestampUs = onPreviewMicroscopeTimestampUs,
                    onFinishScrubFrame = onFinishMicroscopeScrubFrame,
                    onFinishScrubTimestampUs = onFinishMicroscopeScrubTimestampUs,
                    onCommitRange = onCommitMicroscopeRange,
                    onClearRange = onClearMicroscopeRange,
                    onOpenInspector = {
                        navController.navigateWave1TopLevel(FrameScopeDestination.Inspector)
                    },
                    workspaceExtractionContent = workspaceExtractionContent,
                    workspaceOverlay = workspaceOverlay,
                )
            }
            composable(FrameScopeDestination.Inspector.route) {
                Wave1InspectorScreen(
                    state = state,
                    onOpenVideo = onOpenVideo,
                    onStep = onStepMicroscope,
                    onJumpFrame = onJumpMicroscopeFrame,
                    onJumpTimestampUs = onJumpMicroscopeTimestampUs,
                )
            }
            composable(FrameScopeDestination.History.route) {
                Box(
                    modifier = Modifier
                        .fillMaxSize()
                        .padding(20.dp)
                        .testTag("history_destination"),
                ) { historyContent() }
            }
            composable(FrameScopeDestination.Storage.route) {
                Box(
                    modifier = Modifier
                        .fillMaxSize()
                        .padding(20.dp)
                        .testTag("storage_destination"),
                ) { storageContent() }
            }
        }
    }
}

private fun NavHostController.navigateWave1TopLevel(destination: FrameScopeDestination) {
    navigate(destination.route) {
        launchSingleTop = true
        restoreState = true
        popUpTo(FrameScopeDestination.Home.route) { saveState = true }
    }
}

@Composable
private fun Wave1HomeScreen(
    state: FrameScopeUiState,
    onOpenVideo: () -> Unit,
    onOpenWorkspace: () -> Unit,
    recentVideosContent: @Composable () -> Unit,
    storageSummaryContent: @Composable () -> Unit,
) {
    LazyColumn(
        modifier = Modifier
            .fillMaxSize()
            .testTag("home_destination"),
        contentPadding = PaddingValues(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        item {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(
                    text = "Inspect video, frame by frame.",
                    style = MaterialTheme.typography.headlineMedium,
                    fontWeight = FontWeight.SemiBold,
                )
                Text(
                    text = "Open a local video or resume a recent session.",
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
            ) { Text("Open video") }
        }
        val ready = state.videoState as? VideoInspectionState.Ready
        if (ready != null) {
            item {
                Card(
                    modifier = Modifier.fillMaxWidth(),
                    colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainer),
                ) {
                    Column(
                        modifier = Modifier.padding(16.dp),
                        verticalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        Text("Continue", style = MaterialTheme.typography.labelLarge)
                        Text(
                            ready.video.displayName,
                            style = MaterialTheme.typography.titleMedium,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        TextButton(onClick = onOpenWorkspace) { Text("Open workspace") }
                    }
                }
            }
        }
        item { recentVideosContent() }
        item { storageSummaryContent() }
        item {
            Text(
                text = "Local files stay on this device and use Android document access.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun Wave1WorkspaceScreen(
    state: FrameScopeUiState,
    onOpenVideo: () -> Unit,
    onCancelInspection: () -> Unit,
    onDismissError: () -> Unit,
    onStep: (Int) -> Unit,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
    onPreviewFrame: (Long) -> Unit,
    onPreviewTimestampUs: (Long) -> Unit,
    onFinishScrubFrame: (Long) -> Unit,
    onFinishScrubTimestampUs: (Long) -> Unit,
    onCommitRange: (Long, Long) -> Unit,
    onClearRange: () -> Unit,
    onOpenInspector: () -> Unit,
    workspaceExtractionContent: @Composable () -> Unit,
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
            when (val video = state.videoState) {
                is VideoInspectionState.Ready -> {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            text = video.video.displayName,
                            modifier = Modifier.weight(1f),
                            style = MaterialTheme.typography.titleMedium,
                            fontWeight = FontWeight.SemiBold,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Spacer(Modifier.size(8.dp))
                        TextButton(onClick = onOpenVideo) { Text("Open another") }
                    }
                    MicroscopePanel(
                        state = state.microscopeState,
                        indexingProgress = state.indexingProgress,
                        timelineBounds = state.timelineBounds,
                        rangeSelection = state.timelineRange,
                        scrubPreview = state.scrubPreview,
                        onStep = onStep,
                        onJumpFrame = onJumpFrame,
                        onJumpTimestampUs = onJumpTimestampUs,
                        onPreviewFrame = onPreviewFrame,
                        onPreviewTimestampUs = onPreviewTimestampUs,
                        onFinishScrubFrame = onFinishScrubFrame,
                        onFinishScrubTimestampUs = onFinishScrubTimestampUs,
                        onCommitRange = onCommitRange,
                        onClearRange = onClearRange,
                        onDismissError = onDismissError,
                    )
                    workspaceExtractionContent()
                    OutlinedButton(
                        onClick = onOpenInspector,
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("inspect_current_frame_action"),
                    ) { Text("Inspect current frame") }
                }
                VideoInspectionState.Opening,
                VideoInspectionState.Inspecting,
                -> {
                    Text("Preparing video workspace…")
                    OutlinedButton(onClick = onCancelInspection) { Text("Cancel") }
                }
                is VideoInspectionState.Error -> {
                    Text(video.message, color = MaterialTheme.colorScheme.error)
                    Button(onClick = onOpenVideo) { Text("Choose another video") }
                }
                else -> {
                    Text("No video open", style = MaterialTheme.typography.titleLarge)
                    Button(onClick = onOpenVideo) { Text("Open video") }
                }
            }
        }
        workspaceOverlay()
    }
}

@Composable
private fun Wave1InspectorScreen(
    state: FrameScopeUiState,
    onOpenVideo: () -> Unit,
    onStep: (Int) -> Unit,
    onJumpFrame: (Long) -> Unit,
    onJumpTimestampUs: (Long) -> Unit,
) {
    LazyColumn(
        modifier = Modifier
            .fillMaxSize()
            .testTag("inspector_destination"),
        contentPadding = PaddingValues(16.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        val ready = state.videoState as? VideoInspectionState.Ready
        if (ready == null) {
            item {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text("Nothing to inspect yet", style = MaterialTheme.typography.titleLarge)
                    Text("Open a video to inspect technical metadata and exact frame identity.")
                    Button(onClick = onOpenVideo) { Text("Open video") }
                }
            }
        } else {
            item {
                Card(modifier = Modifier.fillMaxWidth()) {
                    Column(
                        modifier = Modifier.padding(16.dp),
                        verticalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        Text(
                            ready.video.displayName,
                            style = MaterialTheme.typography.titleMedium,
                            fontWeight = FontWeight.SemiBold,
                        )
                        Text("${ready.video.metadata.width} × ${ready.video.metadata.height}")
                        ready.video.metadata.durationUs?.let {
                            Text("Duration ${VideoMetadataFormatter.duration(it)}")
                        }
                    }
                }
            }
            item {
                MicroscopeInspectorPanel(
                    state = state.microscopeState,
                    metadata = ready.video.metadata,
                    onStep = onStep,
                    onJumpFrame = onJumpFrame,
                    onJumpTimestampUs = onJumpTimestampUs,
                )
            }
        }
    }
}
