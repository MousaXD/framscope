package com.framescope.app

import com.framescope.app.ui.VideoMetadataFormatter
import org.junit.Assert.assertEquals
import org.junit.Test

class VideoMetadataFormatterTest {
    @Test
    fun formatsShortDuration() {
        assertEquals("04:32", VideoMetadataFormatter.duration(272_000_000))
    }

    @Test
    fun formatsLongDuration() {
        assertEquals("1:02:03", VideoMetadataFormatter.duration(3_723_000_000))
    }

    @Test
    fun formatsEstimatedFpsWithoutFakePrecision() {
        assertEquals("29.97", VideoMetadataFormatter.fps(29.97))
        assertEquals("30", VideoMetadataFormatter.fps(30.0))
        assertEquals("Unknown", VideoMetadataFormatter.fps(null))
    }
}
