package com.framescope.app.ui

import java.util.Locale

object VideoMetadataFormatter {
    fun duration(durationUs: Long): String {
        val totalSeconds = durationUs.coerceAtLeast(0) / 1_000_000
        val hours = totalSeconds / 3600
        val minutes = (totalSeconds % 3600) / 60
        val seconds = totalSeconds % 60
        return if (hours > 0) {
            String.format(Locale.US, "%d:%02d:%02d", hours, minutes, seconds)
        } else {
            String.format(Locale.US, "%02d:%02d", minutes, seconds)
        }
    }

    fun fps(value: Double?): String = value?.let {
        String.format(Locale.US, "%.2f", it).trimEnd('0').trimEnd('.')
    } ?: "Unknown"
}
