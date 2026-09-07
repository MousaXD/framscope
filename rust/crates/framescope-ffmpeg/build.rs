use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=FRAMESCOPE_FFMPEG_ROOT");

    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS is set by Cargo");
    if target_os != "android" {
        return;
    }

    let target_arch =
        env::var("CARGO_CFG_TARGET_ARCH").expect("CARGO_CFG_TARGET_ARCH is set by Cargo");
    if target_arch != "aarch64" {
        panic!("FrameScope Phase 2 currently supports FFmpeg only for Android aarch64/arm64-v8a");
    }

    let root = env::var_os("FRAMESCOPE_FFMPEG_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "FRAMESCOPE_FFMPEG_ROOT is required for Android builds; run scripts/build-ffmpeg-android.sh or scripts/build-rust.sh"
            )
        });

    require(&root, "include/libavcodec/avcodec.h");
    require(&root, "include/libavformat/avformat.h");
    require(&root, "include/libavutil/avutil.h");
    require(&root, "include/libswscale/swscale.h");
    require(&root, "lib/libavcodec.a");
    require(&root, "lib/libavformat.a");
    require(&root, "lib/libavutil.a");
    require(&root, "lib/libswscale.a");

    println!(
        "cargo:rustc-link-search=native={}",
        root.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=avformat");
    println!("cargo:rustc-link-lib=static=avcodec");
    println!("cargo:rustc-link-lib=static=swscale");
    println!("cargo:rustc-link-lib=static=avutil");
    println!("cargo:rustc-link-lib=dylib=m");
    println!("cargo:rustc-link-lib=dylib=dl");
    println!("cargo:rustc-link-lib=dylib=log");
}

fn require(root: &Path, relative: &str) {
    let path = root.join(relative);
    if !path.is_file() {
        panic!(
            "required FFmpeg Android file is missing: {}. Rebuild the pinned prefix with scripts/build-ffmpeg-android.sh",
            path.display()
        );
    }
}
