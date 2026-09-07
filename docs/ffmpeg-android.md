# FFmpeg + Android native build

FrameScope Phase 2 uses a pinned, source-built FFmpeg foundation for Android. This document covers only build/linkage availability. It does **not** claim the Phase 2 decoder API, frame extraction, indexing, caching, similarity, or UI work is implemented.

## Selected strategy

- **FFmpeg:** 9.0.1 (`Lei`), official source archive from `ffmpeg.org`.
- **Source SHA-256:** `cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635`.
- **Android ABI:** `arm64-v8a` / Rust target `aarch64-linux-android` only.
- **Android API floor:** 26, matching FrameScope `minSdk`.
- **NDK:** r27d, `27.3.13750724`.
- **Link mode:** FFmpeg is built as static PIC archives and linked into `libframescope_ffi.so`. The APK therefore does not need separate `libav*.so` runtime files.
- **Rust boundary:** `framescope-ffmpeg` owns Android FFmpeg discovery/link flags and a tiny version/link probe. `framescope-video` is the higher-level media boundary. FFmpeg build hacks must not leak into UI or application crates.

The project does not download third-party precompiled FFmpeg binaries. `scripts/build-ffmpeg-android.sh` downloads the official source tarball, verifies its pinned SHA-256, and cross-compiles it with the pinned Android NDK.

## FFmpeg configuration

The Android build intentionally enables only the libraries/components needed as the Phase 2 decoder foundation.

Libraries built and linked:

- `libavcodec`
- `libavformat`
- `libavutil`
- `libswscale`

Software video decoders explicitly enabled:

- H.264 (`h264`)
- H.265 / HEVC (`hevc`)
- VP9 (`vp9`)
- AV1 (`av1`)

Demuxers explicitly enabled:

- MP4/MOV (`mov`)
- Matroska/WebM (`matroska`)
- AVI (`avi`)

Parsers explicitly enabled: H.264, HEVC, VP9, and AV1. File and pipe protocols are enabled. Networking, programs (`ffmpeg`, `ffprobe`), encoders, muxers, filters, device APIs, `libavfilter`, `libavdevice`, `libswresample`, and external codec autodetection are disabled.

The build does not enable `libx264`, `libx265`, `libaom`, `libdav1d`, or other external codec libraries. The listed codecs are FFmpeg's built-in software decoders. Hardware MediaCodec acceleration is **not** part of this foundation and must not be reported as implemented.

The build script copies FFmpeg's generated `config_components.h` into the installed prefix. `scripts/verify-ffmpeg-android.sh` checks that the expected decoder/demuxer macros are actually enabled rather than trusting documentation alone.

## Licensing

FrameScope is GPL-3.0-only. The pinned FFmpeg build is configured with `--enable-gpl --enable-version3`, so the resulting FFmpeg libraries are distributed under GPLv3-or-later terms. This is compatible with distributing the combined FrameScope work under GPLv3.

No nonfree FFmpeg option is enabled, and no external GPL codec library is bundled. If a future change enables an external codec library or changes FFmpeg configure flags, licensing must be reviewed again and the build metadata/documentation must be updated.

FFmpeg copyright/license notices remain part of the FFmpeg source release. Binary redistributors must continue to satisfy both FrameScope's GPL-3.0-only license and FFmpeg's applicable GPL terms, including corresponding-source obligations.

## Directory layout and caching

Generated native state lives under the ignored `.native/` directory:

```text
.native/
  downloads/ffmpeg-9.0.1.tar.xz
  ffmpeg/arm64-v8a/
    include/
    lib/
    share/framescope/build-info.env
    share/framescope/config_components.h
```

The installed prefix is intentionally stable (`.native/ffmpeg/arm64-v8a`) while `build-info.env` records the FFmpeg version, source hash, ABI, API level, NDK version, libraries, and codecs. If those pinned inputs no longer match, the build script rebuilds the prefix.

This makes the prefix a suitable CI cache payload. Agent 4 can key the cache with, at minimum, the hash of `scripts/build-ffmpeg-android.sh`, FFmpeg version/hash, NDK version, Android API, and ABI. Do not cache or commit the extracted FFmpeg source/build tree.

Set `FRAMESCOPE_FFMPEG_ROOT` to use a different verified prefix location. CI can restore a cached prefix and the build script will validate/reuse it.

## Build commands

Prerequisites are the existing FrameScope Android/Rust toolchain plus `curl`, `sha256sum`, `tar`, `make`, and the NDK toolchain. On the current Linux CI host:

```bash
rustup target add aarch64-linux-android
cargo install cargo-ndk --locked
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.3.13750724"
```

Build/verify FFmpeg only:

```bash
./scripts/build-ffmpeg-android.sh
./scripts/verify-ffmpeg-android.sh
```

Build the Android Rust JNI library, automatically reusing or producing the verified FFmpeg prefix:

```bash
./scripts/build-rust.sh
```

Build the debug APK:

```bash
cd android
gradle --no-daemon assembleDebug
```

The Gradle `buildRustArm64` task depends on `prepareFfmpegArm64`, so normal APK builds use the same source-build script and prefix instead of maintaining a second native toolchain path.

## Packaging model

FFmpeg's four static archives are linked into:

```text
android/app/build/generated/jniLibs/arm64-v8a/libframescope_ffi.so
```

Gradle already packages that generated JNI directory. The debug APK must contain:

```text
lib/arm64-v8a/libframescope_ffi.so
```

It should **not** contain separate `libavcodec.so`, `libavformat.so`, `libavutil.so`, or `libswscale.so` files. `scripts/verify.sh` checks the exported `framescope_ffmpeg_link_probe` symbol and rejects accidental dynamic `libav*.so` dependencies.

## Interface for the decoder agent

Agent 2 should build the decoder abstraction in `framescope-video`, not in Kotlin and not directly in `framescope-ffi`.

`framescope-ffmpeg` is intentionally small today. It proves that the pinned headers/libraries exist and that the final Android JNI library resolves symbols from `avcodec`, `avformat`, `avutil`, and `swscale`. Agent 2 may extend this crate with a pinned low-level binding crate or a carefully scoped generated binding layer. If that happens, keep target-specific FFmpeg configuration centralized here and preserve host builds that do not require Android FFmpeg.

No Rust FFmpeg binding crate is selected in this change. That avoids committing Agent 2 to a high-level API before the decoder design exists while still giving it a reproducible, verified native prefix.

## CI integration points

Agent 4 can split the current monolithic workflow around these commands:

1. restore/cache `.native/ffmpeg/arm64-v8a`;
2. run `./scripts/build-ffmpeg-android.sh` (fast no-op on a valid cache);
3. run `./scripts/verify-ffmpeg-android.sh`;
4. run `./scripts/build-rust.sh` for the `aarch64-linux-android` native library;
5. run Gradle tests/lint/`assembleDebug`;
6. inspect the APK/native library using the checks already present in `scripts/verify.sh`.

The branch does not rewrite `.github/workflows/ci.yml`; workflow ownership remains with Agent 4.
