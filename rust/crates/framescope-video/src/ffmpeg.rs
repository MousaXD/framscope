//! FFmpeg backend availability boundary.
//!
//! Agent 1 establishes Android linkage only. Decoder APIs belong to the Phase 2 video-engine
//! implementation and must remain above this low-level probe.

/// Force a link-time reference to the pinned Android FFmpeg libraries.
pub fn link_probe() -> u32 {
    #[cfg(target_os = "android")]
    {
        framescope_ffmpeg::link_probe()
    }

    #[cfg(not(target_os = "android"))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_os = "android"))]
    #[test]
    fn host_probe_is_explicitly_unavailable() {
        assert_eq!(super::link_probe(), 0);
    }
}
