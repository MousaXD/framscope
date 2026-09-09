package com.framescope.app

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.setContent
import androidx.activity.viewModels
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.framescope.app.data.AndroidFrameScopeRepository
import com.framescope.app.data.AndroidMicroscopeScrubPreviewSource
import com.framescope.app.data.AndroidRecentVideoAccessChecker
import com.framescope.app.data.NativePersistentFrameIndexCatalog
import com.framescope.app.data.RecentVideoHistoryRepository
import com.framescope.app.data.RecentVideoRecord
import com.framescope.app.data.SharedPreferencesRecentVideoStore
import com.framescope.app.platform.LocalExportTree
import com.framescope.app.platform.LocalVideoOpenDocument
import com.framescope.app.platform.VideoUriPermissionManager
import com.framescope.app.ui.BatchExportViewModel
import com.framescope.app.ui.BatchExportViewModelFactory
import com.framescope.app.ui.ExtractionWorkflow
import com.framescope.app.ui.FrameExportViewModel
import com.framescope.app.ui.FrameExportViewModelFactory
import com.framescope.app.ui.FrameScopeScreen
import com.framescope.app.ui.HistoryScreen
import com.framescope.app.ui.HistoryViewModel
import com.framescope.app.ui.HistoryViewModelFactory
import com.framescope.app.ui.MainViewModel
import com.framescope.app.ui.MainViewModelFactory
import com.framescope.app.ui.MicroscopeUiState
import com.framescope.app.ui.RecentVideoOpenTarget
import com.framescope.app.ui.RecentVideoSessionEffects
import com.framescope.app.ui.RecentVideosHomeContent
import com.framescope.app.ui.StorageDestinationContent
import com.framescope.app.ui.StorageSummaryContent
import com.framescope.app.ui.toOpenTarget
import com.framescope.app.ui.toReselectTarget
import com.framescope.app.ui.theme.FrameScopeTheme

class MainActivity : ComponentActivity() {
    private val frameScopeCacheRoot by lazy {
        applicationContext.cacheDir.resolve("framescope").absolutePath
    }

    private val repository by lazy {
        AndroidFrameScopeRepository(
            contentResolver = applicationContext.contentResolver,
            cacheRoot = frameScopeCacheRoot,
        )
    }

    private val scrubPreviewSource by lazy {
        AndroidMicroscopeScrubPreviewSource(cacheRoot = frameScopeCacheRoot)
    }

    private val persistentFrameIndexCatalog by lazy {
        NativePersistentFrameIndexCatalog(cacheRoot = frameScopeCacheRoot)
    }

    private val recentVideoHistory by lazy {
        RecentVideoHistoryRepository(
            store = SharedPreferencesRecentVideoStore(applicationContext),
            accessChecker = AndroidRecentVideoAccessChecker(applicationContext.contentResolver),
            indexCatalog = persistentFrameIndexCatalog,
        )
    }

    private val videoUriPermissionManager by lazy {
        VideoUriPermissionManager(applicationContext.contentResolver)
    }

    private val viewModel: MainViewModel by viewModels {
        MainViewModelFactory(repository, scrubPreviewSource)
    }

    private val historyViewModel: HistoryViewModel by viewModels {
        HistoryViewModelFactory(recentVideoHistory)
    }

    private val exportViewModel: FrameExportViewModel by viewModels {
        FrameExportViewModelFactory(repository)
    }

    private val batchExportViewModel: BatchExportViewModel by viewModels {
        BatchExportViewModelFactory(repository)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            FrameScopeTheme {
                val state by viewModel.uiState.collectAsStateWithLifecycle()
                val historyState by historyViewModel.state.collectAsStateWithLifecycle()
                val exportState by exportViewModel.state.collectAsStateWithLifecycle()
                val batchExportState by batchExportViewModel.state.collectAsStateWithLifecycle()
                var selectedSource by remember { mutableStateOf<RecentVideoOpenTarget?>(null) }
                var pendingReselect by remember { mutableStateOf<RecentVideoRecord?>(null) }

                fun openTarget(target: RecentVideoOpenTarget) {
                    exportViewModel.cancelForMicroscopeChange()
                    batchExportViewModel.cancelForMicroscopeChange()
                    selectedSource = target
                    viewModel.onVideoSelected(target.contentUri)
                }

                val videoPicker = rememberLauncherForActivityResult(
                    contract = LocalVideoOpenDocument(),
                ) { uri ->
                    if (uri == null) {
                        viewModel.onPickerCancelled()
                    } else {
                        val permissionStatus = videoUriPermissionManager.persistReadAccess(uri)
                        openTarget(
                            RecentVideoOpenTarget(
                                contentUri = uri.toString(),
                                permissionStatus = permissionStatus,
                            ),
                        )
                    }
                }
                val reselectVideoPicker = rememberLauncherForActivityResult(
                    contract = LocalVideoOpenDocument(),
                ) { uri ->
                    val record = pendingReselect
                    pendingReselect = null
                    if (uri != null && record != null) {
                        val permissionStatus = videoUriPermissionManager.persistReadAccess(uri)
                        openTarget(record.toReselectTarget(uri.toString(), permissionStatus))
                    }
                }
                val exportTreePicker = rememberLauncherForActivityResult(
                    contract = LocalExportTree(),
                ) { uri ->
                    if (uri == null) {
                        exportViewModel.onDestinationPickerCancelled()
                    } else {
                        runCatching {
                            contentResolver.takePersistableUriPermission(
                                uri,
                                Intent.FLAG_GRANT_READ_URI_PERMISSION or
                                    Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
                            )
                        }
                        val ready = state.microscopeState as? MicroscopeUiState.Ready
                        exportViewModel.onDestinationSelected(
                            treeUri = uri.toString(),
                            currentSessionId = ready?.session?.sessionId,
                            currentFrameId = ready?.session?.currentFrame?.frameId,
                        )
                    }
                }
                val batchExportTreePicker = rememberLauncherForActivityResult(
                    contract = LocalExportTree(),
                ) { uri ->
                    if (uri == null) {
                        batchExportViewModel.onDestinationPickerCancelled()
                    } else {
                        runCatching {
                            contentResolver.takePersistableUriPermission(
                                uri,
                                Intent.FLAG_GRANT_READ_URI_PERMISSION or
                                    Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
                            )
                        }
                        val ready = state.microscopeState as? MicroscopeUiState.Ready
                        batchExportViewModel.onDestinationSelected(
                            treeUri = uri.toString(),
                            currentSessionId = ready?.session?.sessionId,
                            currentFrameId = ready?.session?.currentFrame?.frameId,
                        )
                    }
                }

                RecentVideoSessionEffects(
                    target = selectedSource,
                    videoState = state.videoState,
                    microscopeState = state.microscopeState,
                    history = recentVideoHistory,
                    indexCatalog = persistentFrameIndexCatalog,
                    onResumeTimestampUs = viewModel::jumpMicroscopeTimestampUs,
                    onLibraryChanged = historyViewModel::refresh,
                )

                FrameScopeScreen(
                    state = state,
                    onOpenVideo = {
                        exportViewModel.cancelForMicroscopeChange()
                        batchExportViewModel.cancelForMicroscopeChange()
                        viewModel.onPickerStarted()
                        videoPicker.launch(arrayOf("video/*"))
                    },
                    onCancelInspection = {
                        exportViewModel.cancelForMicroscopeChange()
                        batchExportViewModel.cancelForMicroscopeChange()
                        viewModel.cancelInspection()
                    },
                    onDismissError = viewModel::clearError,
                    onStepMicroscope = { delta ->
                        exportViewModel.cancelForMicroscopeChange()
                        batchExportViewModel.cancelForMicroscopeChange()
                        viewModel.stepMicroscope(delta)
                    },
                    onJumpMicroscopeFrame = { frameId ->
                        exportViewModel.cancelForMicroscopeChange()
                        batchExportViewModel.cancelForMicroscopeChange()
                        viewModel.jumpMicroscopeFrame(frameId)
                    },
                    onJumpMicroscopeTimestampUs = { timestampUs ->
                        exportViewModel.cancelForMicroscopeChange()
                        batchExportViewModel.cancelForMicroscopeChange()
                        viewModel.jumpMicroscopeTimestampUs(timestampUs)
                    },
                    onPreviewMicroscopeFrame = viewModel::previewMicroscopeFrame,
                    onPreviewMicroscopeTimestampUs = viewModel::previewMicroscopeTimestampUs,
                    onFinishMicroscopeScrubFrame = { frameId ->
                        exportViewModel.cancelForMicroscopeChange()
                        batchExportViewModel.cancelForMicroscopeChange()
                        viewModel.finishMicroscopeScrubFrame(frameId)
                    },
                    onFinishMicroscopeScrubTimestampUs = { timestampUs ->
                        exportViewModel.cancelForMicroscopeChange()
                        batchExportViewModel.cancelForMicroscopeChange()
                        viewModel.finishMicroscopeScrubTimestampUs(timestampUs)
                    },
                    onCommitMicroscopeRange = viewModel::commitTimelineRange,
                    onClearMicroscopeRange = viewModel::clearTimelineRange,
                    recentVideosContent = {
                        RecentVideosHomeContent(
                            state = historyState,
                            onOpen = { record -> record.toOpenTarget()?.let(::openTarget) },
                        )
                    },
                    historyContent = {
                        HistoryScreen(
                            state = historyState,
                            onRefresh = historyViewModel::refresh,
                            onOpen = { record -> record.toOpenTarget()?.let(::openTarget) },
                            onReselect = { record ->
                                pendingReselect = record
                                reselectVideoPicker.launch(arrayOf("video/*"))
                            },
                            onRemove = historyViewModel::remove,
                            onClear = historyViewModel::clear,
                        )
                    },
                    storageSummaryContent = {
                        StorageSummaryContent(cacheRoot = frameScopeCacheRoot)
                    },
                    storageContent = {
                        StorageDestinationContent(cacheRoot = frameScopeCacheRoot)
                    },
                    workspaceExtractionContent = {
                        ExtractionWorkflow(
                            microscopeState = state.microscopeState,
                            selectedTimelineRange = state.timelineRange,
                            currentFrameState = exportState,
                            batchState = batchExportState,
                            onRequestCurrentFrame = { format ->
                                val ready = state.microscopeState as? MicroscopeUiState.Ready
                                    ?: return@ExtractionWorkflow
                                val frameId = ready.session.currentFrame?.frameId
                                    ?: return@ExtractionWorkflow
                                batchExportViewModel.cancelForMicroscopeChange()
                                exportViewModel.beginCurrentFrameExport(
                                    sessionId = ready.session.sessionId,
                                    frameId = frameId,
                                    format = format,
                                )
                                exportTreePicker.launch(null)
                            },
                            onRequestBatch = { request ->
                                val ready = state.microscopeState as? MicroscopeUiState.Ready
                                    ?: return@ExtractionWorkflow
                                exportViewModel.cancelForMicroscopeChange()
                                batchExportViewModel.beginBatchExport(
                                    sessionId = ready.session.sessionId,
                                    currentFrameId = ready.session.currentFrame?.frameId,
                                    request = request,
                                )
                                batchExportTreePicker.launch(null)
                            },
                            onCancelCurrentFrame = exportViewModel::cancelForMicroscopeChange,
                            onCancelBatch = batchExportViewModel::cancelForMicroscopeChange,
                            onDismissCurrentFrameStatus = exportViewModel::dismissStatus,
                            onDismissBatchStatus = batchExportViewModel::dismissStatus,
                        )
                    },
                )
            }
        }
    }
}
