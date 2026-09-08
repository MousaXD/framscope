package com.framescope.app.ui

import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.List
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationRail
import androidx.compose.material3.NavigationRailItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp

internal enum class FrameScopeDestination(
    val route: String,
    val label: String,
    val title: String,
    val testTag: String,
) {
    Home("home", "Home", "FrameScope", "nav_home"),
    Workspace("workspace", "Video", "Video workspace", "nav_workspace"),
    Inspector("inspector", "Inspector", "Inspector", "nav_inspector"),
    History("history", "History", "History", "nav_history"),
    Storage("storage", "Storage", "Storage & settings", "nav_storage"),
    ;

    companion object {
        fun fromRoute(route: String?): FrameScopeDestination =
            entries.firstOrNull { destination -> destination.route == route } ?: Home
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun FrameScopeAppShell(
    currentDestination: FrameScopeDestination,
    onDestinationSelected: (FrameScopeDestination) -> Unit,
    modifier: Modifier = Modifier,
    content: @Composable (PaddingValues) -> Unit,
) {
    BoxWithConstraints(modifier = modifier.fillMaxSize()) {
        val useNavigationRail = maxWidth >= 700.dp
        if (useNavigationRail) {
            Row(modifier = Modifier.fillMaxSize()) {
                NavigationRail(
                    modifier = Modifier
                        .fillMaxHeight()
                        .testTag("framescope_navigation_rail"),
                ) {
                    Column(modifier = Modifier.padding(top = 12.dp)) {
                        FrameScopeDestination.entries.forEach { destination ->
                            NavigationRailItem(
                                selected = destination == currentDestination,
                                onClick = { onDestinationSelected(destination) },
                                icon = {
                                    Icon(
                                        imageVector = destination.icon(),
                                        contentDescription = destination.label,
                                    )
                                },
                                label = { Text(destination.label) },
                                modifier = Modifier.testTag(destination.testTag),
                            )
                        }
                    }
                }
                FrameScopeScaffold(
                    currentDestination = currentDestination,
                    bottomBar = {},
                    modifier = Modifier.weight(1f),
                    content = content,
                )
            }
        } else {
            FrameScopeScaffold(
                currentDestination = currentDestination,
                bottomBar = {
                    NavigationBar(
                        modifier = Modifier.testTag("framescope_bottom_navigation"),
                    ) {
                        FrameScopeDestination.entries.forEach { destination ->
                            NavigationBarItem(
                                selected = destination == currentDestination,
                                onClick = { onDestinationSelected(destination) },
                                icon = {
                                    Icon(
                                        imageVector = destination.icon(),
                                        contentDescription = destination.label,
                                    )
                                },
                                label = { Text(destination.label) },
                                modifier = Modifier.testTag(destination.testTag),
                            )
                        }
                    }
                },
                modifier = Modifier.fillMaxSize(),
                content = content,
            )
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun FrameScopeScaffold(
    currentDestination: FrameScopeDestination,
    bottomBar: @Composable () -> Unit,
    modifier: Modifier,
    content: @Composable (PaddingValues) -> Unit,
) {
    Scaffold(
        modifier = modifier,
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        text = currentDestination.title,
                        style = MaterialTheme.typography.titleLarge,
                    )
                },
            )
        },
        bottomBar = bottomBar,
        content = content,
    )
}

private fun FrameScopeDestination.icon(): ImageVector = when (this) {
    FrameScopeDestination.Home -> Icons.Filled.Home
    FrameScopeDestination.Workspace -> Icons.Filled.PlayArrow
    FrameScopeDestination.Inspector -> Icons.Filled.Info
    FrameScopeDestination.History -> Icons.Filled.List
    FrameScopeDestination.Storage -> Icons.Filled.Settings
}
