package com.framescope.app

import com.framescope.app.data.RecentVideoAvailability
import com.framescope.app.data.RecentVideoRecord
import com.framescope.app.data.VideoUriPermissionStatus
import com.framescope.app.ui.toOpenTarget
import com.framescope.app.ui.toReselectTarget
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class RecentVideoOpenTargetTest {
    @Test
    fun availableRecordCarriesSavedTimestampIntoResumeTarget() {
        val target = record().toOpenTarget()

        assertEquals("content://videos/clip", target?.contentUri)
        assertEquals(VideoUriPermissionStatus.Persisted, target?.permissionStatus)
        assertEquals(9_876_543L, target?.resumeTimestampUs)
        assertNull(target?.replacesRecordId)
    }

    @Test
    fun unavailableRecordCannotProduceDirectOpenTarget() {
        val target = record(
            availability = RecentVideoAvailability.PermissionLost,
            permissionStatus = VideoUriPermissionStatus.Lost,
        ).toOpenTarget()

        assertNull(target)
    }

    @Test
    fun reselectTargetKeepsRecordIdentityAndResumeTimestamp() {
        val target = record().toReselectTarget(
            newContentUri = "content://provider/new-clip",
            permissionStatus = VideoUriPermissionStatus.Persisted,
        )

        assertEquals("content://provider/new-clip", target.contentUri)
        assertEquals("record-1", target.replacesRecordId)
        assertEquals(9_876_543L, target.resumeTimestampUs)
    }

    private fun record(
        availability: RecentVideoAvailability = RecentVideoAvailability.Available,
        permissionStatus: VideoUriPermissionStatus = VideoUriPermissionStatus.Persisted,
    ) = RecentVideoRecord(
        id = "record-1",
        contentUri = "content://videos/clip",
        permissionStatus = permissionStatus,
        availability = availability,
        displayName = "clip.mp4",
        durationUs = 20_000_000L,
        width = 1920,
        height = 1080,
        codec = "h264",
        container = "mp4",
        lastOpenedEpochMs = 1_000L,
        lastViewedFrameId = 296L,
        lastViewedTimestampUs = 9_876_543L,
    )
}
