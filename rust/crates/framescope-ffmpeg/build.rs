use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=FRAMESCOPE_FFMPEG_ROOT");
    println!("cargo:rerun-if-changed=native/framescope_ffmpeg_shim.c");
    println!("cargo:rustc-check-cfg=cfg(framescope_ffmpeg_native)");

    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS is set by Cargo");
    let use_system_ffmpeg = env::var_os("CARGO_FEATURE_SYSTEM_FFMPEG").is_some();

    if target_os == "android" {
        build_android();
        return;
    }

    if use_system_ffmpeg {
        build_system();
    }
}

fn build_android() {
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
    require(&root, "share/framescope/build-info.env");
    require(&root, "share/framescope/config_components.h");

    // These files live outside the Cargo package and can be rebuilt at the same stable prefix.
    // Explicitly track them so a restored Cargo target cache cannot silently retain an older
    // statically linked FFmpeg build after the verified prefix changes.
    track(&root, "lib/libavcodec.a");
    track(&root, "lib/libavformat.a");
    track(&root, "lib/libavutil.a");
    track(&root, "lib/libswscale.a");
    track(&root, "share/framescope/build-info.env");
    track(&root, "share/framescope/config_components.h");

    compile_shim(Some(&root.join("include")));

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
    println!("cargo:rustc-cfg=framescope_ffmpeg_native");
}

fn build_system() {
    compile_shim(None);
    println!("cargo:rustc-link-lib=dylib=avformat");
    println!("cargo:rustc-link-lib=dylib=avcodec");
    println!("cargo:rustc-link-lib=dylib=swscale");
    println!("cargo:rustc-link-lib=dylib=avutil");
    println!("cargo:rustc-link-lib=dylib=m");
    println!("cargo:rustc-link-lib=dylib=dl");
    println!("cargo:rustc-cfg=framescope_ffmpeg_native");
}

fn compile_shim(include: Option<&Path>) {
    let mut build = cc::Build::new();
    build
        .file("native/framescope_ffmpeg_shim.c")
        .warnings(true)
        .flag_if_supported("-Werror=implicit-function-declaration")
        .flag_if_supported("-Werror=incompatible-pointer-types");
    if let Some(include) = include {
        build.include(include);
    }
    build.compile("framescope_ffmpeg_shim");
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

fn track(root: &Path, relative: &str) {
    println!("cargo:rerun-if-changed={}", root.join(relative).display());
}
