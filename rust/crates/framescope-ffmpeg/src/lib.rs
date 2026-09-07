//! Narrow FFmpeg linkage boundary for Android.
//!
//! This crate intentionally exposes only a tiny version/link probe in Phase 2 Agent 1 work.
//! Decoder ownership remains in `framescope-video` and will be implemented separately.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkedVersions {
    pub avcodec: u32,
    pub avformat: u32,
    pub avutil: u32,
    pub swscale: u32,
}

#[cfg(target_os = "android")]
unsafe extern "C" {
    fn avcodec_version() -> u32;
    fn avformat_version() -> u32;
    fn avutil_version() -> u32;
    fn swscale_version() -> u32;
}

/// Return linked FFmpeg library version integers on Android.
///
/// Host builds intentionally return `None`; host tests must not accidentally depend on an
/// Android FFmpeg installation.
pub fn linked_versions() -> Option<LinkedVersions> {
    #[cfg(target_os = "android")]
    {
        // SAFETY: these are argument-free version functions from the statically linked, pinned
        // FFmpeg libraries. Successful linking proves the symbols are present.
        Some(unsafe {
            LinkedVersions {
                avcodec: avcodec_version(),
                avformat: avformat_version(),
                avutil: avutil_version(),
                swscale: swscale_version(),
            }
        })
    }

    #[cfg(not(target_os = "android"))]
    {
        None
    }
}

/// Force the final Android JNI library to retain references to all required FFmpeg libraries.
pub fn link_probe() -> u32 {
    linked_versions()
        .map(|versions| versions.avcodec ^ versions.avformat ^ versions.avutil ^ versions.swscale)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "android"))]
    #[test]
    fn host_build_does_not_require_android_ffmpeg() {
        assert_eq!(linked_versions(), None);
        assert_eq!(link_probe(), 0);
    }
}
