package com.framescope.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.viewModels
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.framescope.app.data.AndroidFrameScopeRepository
import com.framescope.app.platform.LocalVideoOpenDocument
import com.framescope.app.ui.FrameScopeScreen
import com.framescope.app.ui.MainViewModel
import com.framescope.app.ui.MainViewModelFactory
import com.framescope.app.ui.theme.FrameScopeTheme

class MainActivity : ComponentActivity() {
    private val viewModel: MainViewModel by viewModels {
        MainViewModelFactory(
            AndroidFrameScopeRepository(
                contentResolver = applicationContext.contentResolver,
                cacheRoot = applicationContext.cacheDir.resolve("framescope").absolutePath,
            ),
        )
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            FrameScopeTheme {
                val state by viewModel.uiState.collectAsStateWithLifecycle()
                val picker = rememberLauncherForActivityResult(
                    contract = LocalVideoOpenDocument(),
                ) { uri ->
                    if (uri == null) {
                        viewModel.onPickerCancelled()
                    } else {
                        viewModel.onVideoSelected(uri.toString())
                    }
                }

                FrameScopeScreen(
                    state = state,
                    onOpenVideo = {
                        viewModel.onPickerStarted()
                        picker.launch(arrayOf("video/*"))
                    },
                    onCancelInspection = viewModel::cancelInspection,
                    onDismissError = viewModel::clearError,
                    onStepMicroscope = viewModel::stepMicroscope,
                    onJumpMicroscopeFrame = viewModel::jumpMicroscopeFrame,
                    onJumpMicroscopeTimestampUs = { timestampUs ->
                        viewModel.jumpMicroscopeTimestampUs(timestampUs)
                    },
                )
            }
        }
    }
}
