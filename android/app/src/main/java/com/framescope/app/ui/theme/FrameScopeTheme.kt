package com.framescope.app.ui.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

private val FrameScopeColors = darkColorScheme(
    primary = Color(0xFFB8C7FF),
    onPrimary = Color(0xFF15234B),
    secondary = Color(0xFF91D7C3),
    background = Color(0xFF090A0C),
    surface = Color(0xFF111318),
    surfaceVariant = Color(0xFF1A1D24),
    onBackground = Color(0xFFF2F3F7),
    onSurface = Color(0xFFF2F3F7),
    onSurfaceVariant = Color(0xFFB9BEC9),
    error = Color(0xFFFFB4AB),
)

@Composable
fun FrameScopeTheme(content: @Composable () -> Unit) {
    // FrameScope intentionally uses one inspection-focused dark palette in Phase 1.
    MaterialTheme(
        colorScheme = FrameScopeColors,
        content = content,
    )
}
