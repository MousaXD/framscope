# FFmpeg + Android native build

FrameScope Phase 2 uses a pinned, source-built FFmpeg foundation for Android and links the Rust video engine against it. This document describes the production Android build, linkage, codec surface, provenance checks, and licensing.

## Selected strategy

- **FFmpeg:** 9.0.1 (`Lei`), official source archive from `ffmpeg.org`.
- **Source SHA-256:** `cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635`.
- **Android ABI:** `arm64-v8a` / Rust target `aarch64-linux-android` only.
- **Android API floor:** 26, matching FrameScope `minSdk`.
- **NDK:** r27d, `27.3.13750724`.
- **Link mode:** FFmpeg is built as static PIC archives and linked into `libframescope_ffi.so`. The APK does not need separate `libav*.so` runtime files.
- **Rust boundary:** `framescope-ffmpeg` owns the narrow C/Rust FFmpeg lifetime and link boundary; `framescope-video` owns platform-neutral stream/timestamp/decoder policy; Android reaches it only through `framescope-ffi`.

The project does not download third-party precompiled FFmpeg binaries. `scripts/build-ffmpeg-android.sh` downloads the official source tarball, verifies its pinned SHA-256, and cross-compiles it with the pinned Android NDK.

## FFmpeg configuration

The Android build intentionally enables only the libraries/components needed by the Phase 2 decoder.

Libraries built and statically linked:

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

The Android build does not enable `libx264`, `libx265`, `libaom`, `libdav1d`, or other external codec libraries. The listed codecs use FFmpeg's built-in software decoders. Hardware MediaCodec acceleration is not part of Phase 2 and must not be reported as implemented.

Audio streams are discoverable in container metadata, but Phase 2 does not decode audio. For example, AAC may appear as an audio stream in an MP4 fixture while no AAC decoder is enabled in the Android FFmpeg build.

## Provenance and reproducibility

The build script writes `share/framescope/build-info.env` into the generated prefix. It records:

- FFmpeg version and official source URL;
- pinned source SHA-256;
- SHA-256 of `scripts/build-ffmpeg-android.sh` itself;
- Android ABI/architecture/API;
- NDK version;
- license mode;
- linked FFmpeg libraries;
- enabled decoders, demuxers, parsers, and protocols.

Prefix reuse is allowed only when the recorded version, source hash, build-recipe hash, ABI, API, and NDK still match. Changing the configure recipe therefore invalidates an old local/cache prefix even when the FFmpeg release number stays the same.

The build also saves FFmpeg's generated `config_components.h`. `scripts/verify-ffmpeg-android.sh` checks the pinned provenance fields and verifies that the expected decoder/demuxer macros are actually enabled. It rejects unexpected external codec-library enablement instead of trusting documentation alone.

Cargo's `framescope-ffmpeg/build.rs` explicitly tracks the external static archives plus the generated build metadata/configuration files. If the stable prefix is rebuilt, Cargo must rerun the native build/link step rather than silently reusing an older `rust/target` artifact.

## Licensing

FrameScope is GPL-3.0-only. The pinned FFmpeg build is configured with `--enable-gpl --enable-version3`, so the resulting FFmpeg build is distributed under GPLv3-or-later terms. This is compatible with distributing the combined FrameScope work under GPLv3.

No nonfree FFmpeg option is enabled, and no external codec library is bundled. Any future change to enabled external libraries or FFmpeg configure flags requires a fresh licensing review and corresponding documentation/build-metadata update.

FFmpeg copyright/license notices remain part of the FFmpeg source release. Binary redistributors must continue to satisfy FrameScope's GPL-3.0-only license and FFmpeg's applicable GPL terms, including corresponding-source obligations.

## Generated directory layout

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

No generated FFmpeg archive, extracted source tree, or prebuilt binary is committed to the repository.

Set `FRAMESCOPE_FFMPEG_ROOT` to use a different prefix location. The same verification rules still apply before Rust links it.

## Build commands

Prerequisites are the FrameScope Android/Rust toolchain plus `curl`, `sha256sum`, `tar`, `make`, and the NDK toolchain. On the Linux CI host:

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

Build the Android Rust JNI library, automatically producing or reusing the verified FFmpeg prefix:

```bash
./scripts/build-rust.sh
```

Build the debug APK:

```bash
cd android
gradle --no-daemon assembleDebug
```

The Gradle `buildRustArm64` task depends on `prepareFfmpegArm64`, so normal APK builds use the same source-build script and native toolchain instead of maintaining a second FFmpeg path.

## Packaging model

FFmpeg's four static archives are linked into:

```text
android/app/build/generated/jniLibs/arm64-v8a/libframescope_ffi.so
```

Gradle packages that generated JNI directory. The debug APK must contain:

```text
lib/arm64-v8a/libframescope_ffi.so
```

It must not contain separate `libavcodec.so`, `libavformat.so`, `libavutil.so`, or `libswscale.so` files. Repository verification checks the JNI exports, AArch64 ELF identity, APK ABI layout, and rejects accidental dynamic `libav*.so` dependencies.

## Host test separation

The production Android build never discovers or links host FFmpeg libraries. Android always requires the verified cross-compiled prefix.

For Linux decoder integration tests only, `framescope-video` exposes the `system-ffmpeg` feature. CI installs Ubuntu FFmpeg development packages and exercises the same C/Rust decoder API against generated fixtures. Normal host workspace builds do not enable that feature and therefore do not acquire an FFmpeg dependency.

## Current Phase 2 ownership

- `framescope-ffmpeg`: native FFmpeg ownership, custom AVIO, cancellation callback, packet/frame lifecycle, link configuration.
- `framescope-video`: stream selection, exact timestamp/time-base semantics, sequential decode, foundational seeking, typed errors.
- `framescope-ffi`: Android JNI metadata/control projection and operation-scoped cancellation.
- Android repository/ViewModel: SAF descriptor acquisition, off-main-thread invocation, lifecycle/stale-result handling.

Frame caching, timeline indexing, perceptual similarity, duplicate grouping, and extraction remain outside this Phase 2 foundation.
