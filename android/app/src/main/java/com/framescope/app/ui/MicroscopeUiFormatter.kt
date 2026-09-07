package com.framescope.app.ui

import com.framescope.app.data.FrameDetails
import com.framescope.app.data.MicroscopeSessionSnapshot

internal data class MicroscopeControlModel(
    val framePosition: String,
    val timestamp: String,
    val duration: String?,
    val canStepPrevious: Boolean,
    val canStepNext: Boolean,
)

internal object MicroscopeUiFormatter {
    fun controls(
        session: MicroscopeSessionSnapshot,
        busy: Boolean,
    ): MicroscopeControlModel? {
        val frame = session.currentFrame ?: return null
        return MicroscopeControlModel(
            framePosition = "#${frame.frameId + 1} of ${session.frameCount} · id ${frame.frameId}",
            timestamp = exactTimestamp(frame),
            duration = exactDuration(frame),
            canStepPrevious = !busy && session.canStepPrevious,
            canStepNext = !busy && session.canStepNext,
        )
    }

    fun exactTimestamp(frame: FrameDetails): String =
        frame.timestampTicks?.let { ticks ->
            "$ticks ticks @ ${frame.timeBaseNumerator}/${frame.timeBaseDenominator} s"
        } ?: "Unavailable"

    fun exactDuration(frame: FrameDetails): String? =
        frame.durationTicks?.let { ticks ->
            "$ticks ticks @ ${frame.timeBaseNumerator}/${frame.timeBaseDenominator} s"
        }
}
