package com.framescope.app

import com.framescope.app.data.BatchExportRequest
import com.framescope.app.data.BatchExportSelection
import com.framescope.app.data.FrameExportFormat
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class UniqueBatchExportRequestTest {
    @Test
    fun uniqueGroupsRequiresExactlyOneRepresentativePerGroup() {
        val valid = BatchExportRequest(
            selection = BatchExportSelection.UniqueGroups,
            everyNFrames = 1L,
            format = FrameExportFormat.Png,
        )
        val invalidSampling = valid.copy(everyNFrames = 2L)

        assertTrue(valid.isSane())
        assertFalse(invalidSampling.isSane())
    }

    @Test
    fun uniqueGroupsUsesDedicatedNativeSelectionIdentity() {
        assertTrue(BatchExportSelection.UniqueGroups.isSane())
        assertTrue(BatchExportSelection.UniqueGroups.nativeKind == 4)
        assertTrue(BatchExportSelection.UniqueGroups.start == 0L)
        assertTrue(BatchExportSelection.UniqueGroups.end == 0L)
    }
}
