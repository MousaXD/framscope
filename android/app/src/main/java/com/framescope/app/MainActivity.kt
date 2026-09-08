package com.framescope.app

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.viewModels
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.framescope.app.data.AndroidFrameScopeRepository
import com.framescope.app.platform.LocalExportTree
import com.framescope.app.platform.LocalVideoOpenDocument
import com.framescope.app.ui.BatchExportOverlay
import com.framescope.app.ui.BatchExportViewModel
import com.framescope.app.ui.BatchExportViewModelFactory
import com.framescope.app.ui.CurrentFrameExportOverlay
import com.framescope.app.ui.FrameExportViewModel
import com.framescope.app.ui.FrameExportViewModelFactory
import com.framescope.app.ui.FrameScopeScreen
import com.framescope.app.ui.MainViewModel
import com.framescope.app.ui.MainViewModelFactory
import com.framescope.app.ui.MicroscopeUiState
import com.framescope.app.ui.theme.FrameScopeTheme

class MainActivity : ComponentActivity() {
    private val repository by lazy {
        AndroidFrameScopeRepository(
            contentResolver = applicationContext.contentResolver,
            cacheRoot = applicationContext.cacheDir.resolve("framescope").absolutePath,
        )
    }

    private val viewModel: MainViewModel by viewModels {
        MainViewModelFactory(repository)
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
                val exportState by exportViewModel.state.collectAsStateWithLifecycle()
                val batchExportState by batchExportViewModel.state.collectAsStateWithLifecycle()
                val videoPicker = rememberLauncherForActivityResult(
                    contract = LocalVideoOpenDocument(),
                ) { uri ->
                    if (uri == null) {
                        viewModel.onPickerCancelled()
                    } else {
                        viewModel.onVideoSelected(uri.toString())
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

                Box {
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
                    )

                    CurrentFrameExportOverlay(
                        microscopeState = state.microscopeState,
                        exportState = exportState,
                        onRequestExport = { format ->
                            val ready = state.microscopeState as? MicroscopeUiState.Ready
                                ?: return@CurrentFrameExportOverlay
                            val frameId = ready.session.currentFrame?.frameId
                                ?: return@CurrentFrameExportOverlay
                            batchExportViewModel.cancelForMicroscopeChange()
                            exportViewModel.beginCurrentFrameExport(
                                sessionId = ready.session.sessionId,
                                frameId = frameId,
                                format = format,
                            )
                            exportTreePicker.launch(null)
                        },
                        onCancelExport = exportViewModel::cancelForMicroscopeChange,
                        onDismissStatus = exportViewModel::dismissStatus,
                    )

                    BatchExportOverlay(
                        microscopeState = state.microscopeState,
                        exportState = batchExportState,
                        onRequestExport = { request ->
                            val ready = state.microscopeState as? MicroscopeUiState.Ready
                                ?: return@BatchExportOverlay
                            exportViewModel.cancelForMicroscopeChange()
                            batchExportViewModel.beginBatchExport(
                                sessionId = ready.session.sessionId,
                                currentFrameId = ready.session.currentFrame?.frameId,
                                request = request,
                            )
                            batchExportTreePicker.launch(null)
                        },
                        onCancelExport = batchExportViewModel::cancelForMicroscopeChange,
                        onDismissStatus = batchExportViewModel::dismissStatus,
                    )
                }
            }
        }
    }
}
