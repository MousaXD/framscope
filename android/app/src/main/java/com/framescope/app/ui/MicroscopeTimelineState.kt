package com.framescope.app.ui

/**
 * Authoritative presentation-time bounds resolved from the complete persistent frame index.
 *
 * These are not inferred from FPS or container duration. They are the indexed timestamps of the
 * first and last presentation frames, so non-zero timeline origins and VFR sources remain honest.
 */
data class IndexedTimelineBounds(
    val sessionId: Long,
    val startUs: Long,
    val endUs: Long,
) {
    fun isSane(): Boolean =
        sessionId > 0L &&
            endUs >= startUs &&
            MicroscopeTimelineMath.durationUs(startUs, endUs) != null
}

/**
 * One committed user-selected inclusive presentation-time interval.
 *
 * High-frequency handle motion stays local to Compose. Only the final committed interval is
 * hoisted here so extraction and other screen-level actions can consume it without recomposing the
 * whole screen on every pointer event.
 */
data class TimelineRangeSelection(
    val sessionId: Long,
    val startUs: Long,
    val endUs: Long,
) {
    fun isSaneFor(bounds: IndexedTimelineBounds): Boolean =
        bounds.isSane() &&
            sessionId == bounds.sessionId &&
            startUs in bounds.startUs..bounds.endUs &&
            endUs in bounds.startUs..bounds.endUs &&
            endUs >= startUs &&
            MicroscopeTimelineMath.durationUs(startUs, endUs) != null
}
