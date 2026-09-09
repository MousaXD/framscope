package com.framescope.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.framescope.app.data.AndroidMicroscopeSimilarityRepository
import com.framescope.app.data.SimilarityStoreDisposition

@Composable
internal fun MicroscopeSimilarityInspectorPanel(
    state: MicroscopeUiState,
    onJumpFrame: (Long) -> Unit,
) {
    val applicationContext = LocalContext.current.applicationContext
    val cacheRoot = remember(applicationContext) {
        applicationContext.cacheDir.resolve("framescope").absolutePath
    }
    val repository = remember(cacheRoot) {
        AndroidMicroscopeSimilarityRepository(cacheRoot = cacheRoot)
    }
    val factory = remember(repository) {
        MicroscopeSimilarityViewModelFactory(repository)
    }
    val viewModel: MicroscopeSimilarityViewModel = viewModel(factory = factory)
    val similarityState by viewModel.state.collectAsStateWithLifecycle()
    val ready = state as? MicroscopeUiState.Ready
    val sessionId = when (state) {
        is MicroscopeUiState.LoadingFrame -> state.session.sessionId
        is MicroscopeUiState.Navigating -> state.session.sessionId
        is MicroscopeUiState.Ready -> state.session.sessionId
        is MicroscopeUiState.Empty -> state.session.sessionId
        is MicroscopeUiState.Error -> state.session?.sessionId
        MicroscopeUiState.Idle, MicroscopeUiState.Opening -> null
    }
    val authoritativeTargetFrameId = ready?.session?.currentFrame?.frameId

    LaunchedEffect(sessionId, authoritativeTargetFrameId) {
        viewModel.onAuthoritativeTargetChanged(sessionId, authoritativeTargetFrameId)
    }
    DisposableEffect(viewModel) {
        onDispose { viewModel.cancelSearch() }
    }

    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("inspector_similarity_panel"),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                text = "Find similar frames",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                text = "Search the complete indexed video for non-contiguous visual matches. The authoritative FrameId timeline remains unchanged.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )

            val currentFrameId = ready?.session?.currentFrame?.frameId
            Button(
                onClick = {
                    if (ready != null && currentFrameId != null) {
                        viewModel.findSimilarFrames(ready.session.sessionId, currentFrameId)
                    }
                },
                enabled = ready != null && currentFrameId != null &&
                    similarityState !is MicroscopeSimilarityUiState.Searching,
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag("find_similar_frames"),
            ) {
                Text(
                    currentFrameId?.let { "Find similar to frame ${it + 1}" }
                        ?: "Find similar frames",
                )
            }

            when (val resultState = similarityState) {
                MicroscopeSimilarityUiState.Idle -> {
                    if (ready == null) {
                        Text(
                            text = "A source-quality microscope frame must be ready before searching.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }

                is MicroscopeSimilarityUiState.Searching -> {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        CircularProgressIndicator()
                        Column(modifier = Modifier.weight(1f)) {
                            Text("Analyzing indexed frames…")
                            Text(
                                text = "Target frame ${resultState.targetFrameId + 1}",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                        TextButton(onClick = viewModel::cancelSearch) {
                            Text("Cancel")
                        }
                    }
                }

                is MicroscopeSimilarityUiState.Ready -> {
                    val result = resultState.result
                    Text(
                        text = "Target frame ${result.targetFrameId + 1} · ${result.matchedCount} confirmed match${if (result.matchedCount == 1L) "" else "es"}",
                        style = MaterialTheme.typography.bodyMedium,
                        fontWeight = FontWeight.Medium,
                    )
                    Text(
                        text = buildString {
                            append("Confirmation floor ${result.minimumSimilarity} / 10000 · ")
                            append(result.candidateCount)
                            append(" indexed candidates · ")
                            append(
                                when (result.disposition) {
                                    SimilarityStoreDisposition.Reused -> "reused descriptor index"
                                    SimilarityStoreDisposition.Built -> "built descriptor index"
                                },
                            )
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    if (result.matches.isEmpty()) {
                        Text(
                            text = "No other frame met the source-quality confirmation floor.",
                            style = MaterialTheme.typography.bodyMedium,
                        )
                    } else {
                        result.matches.forEachIndexed { index, match ->
                            OutlinedButton(
                                onClick = { onJumpFrame(match.frameId) },
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .testTag("similar_frame_result_$index"),
                            ) {
                                Text("Frame ${match.frameId + 1} · score ${match.similarity} / 10000")
                            }
                        }
                    }
                    if (result.truncated) {
                        Text(
                            text = "Showing ${result.matches.size} highest-scoring matches of ${result.matchedCount} confirmed matches.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    TextButton(onClick = viewModel::dismissStatus) {
                        Text("Clear results")
                    }
                }

                is MicroscopeSimilarityUiState.Error -> {
                    Text(
                        text = resultState.message,
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.error,
                    )
                    resultState.code?.let { code ->
                        Text(
                            text = code,
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Button(
                            onClick = {
                                val current = ready?.session?.currentFrame?.frameId
                                if (ready != null && current != null) {
                                    viewModel.findSimilarFrames(ready.session.sessionId, current)
                                }
                            },
                            enabled = ready?.session?.currentFrame != null,
                        ) {
                            Text("Retry")
                        }
                        TextButton(onClick = viewModel::dismissStatus) {
                            Text("Dismiss")
                        }
                    }
                }
            }
        }
    }
}
