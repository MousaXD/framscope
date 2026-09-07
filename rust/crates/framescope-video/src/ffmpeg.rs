//! FFmpeg backend availability boundary.

/// Force a link-time reference to the configured FFmpeg libraries.
pub fn link_probe() -> u32 {
    framescope_ffmpeg::link_probe()
}

/// Whether this build contains the native decoder backend.
pub const fn backend_available() -> bool {
    framescope_ffmpeg::backend_available()
}

#[cfg(test)]
mod tests {
    #[cfg(not(feature = "system-ffmpeg"))]
    #[test]
    fn default_host_build_is_explicitly_unavailable() {
        #[cfg(not(target_os = "android"))]
        {
            assert_eq!(super::link_probe(), 0);
            assert!(!super::backend_available());
        }
    }
}
