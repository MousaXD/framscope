# FrameScope

FrameScope is an open-source Android application for precise video-frame inspection. It is intentionally local-first: videos stay on the device, the app has no backend, and the normal application path makes no network requests.

This repository is **Phase 1: Foundation**. It establishes the Android/Rust architecture, a real JNI bridge, safe file selection through Android's Storage Access Framework, bounded Rust-backed MP4/MOV metadata inspection, tests, documentation, and CI. It does **not** claim to implement frame decoding or the later microscope workflow yet.

## Implemented in Phase 1

- Android application written in Kotlin and Jetpack Compose.
- Dark, focused home/inspection UI.
- System document picker based on `ActivityResultContracts.OpenDocument` with `video/*` and `EXTRA_LOCAL_ONLY`.
- No broad storage permission and no `INTERNET` permission.
- `ViewModel -> repository -> NativeBridge -> Rust JNI` application boundary.
- Rust workspace with `framescope-core`, `framescope-video`, `framescope-cache`, and `framescope-ffi`.
- Real Rust native calls for engine version and video inspection.
- Bounded, seek-based ISO BMFF metadata parser for ordinary MP4/MOV files with a `moov` box.
- Width, height, duration, rotation, and estimated FPS when an `stts` sample table is available.
- Safe FFI error envelope instead of panics or exceptions crossing the JNI boundary.
- Initial cache source-identity contract and a real no-op cache store.
- Rust unit tests and Android/JVM ViewModel/formatting tests.
- GitHub Actions configuration for future public-repository CI.

## Planned, not implemented

- FFmpeg-backed decoding and broad container/codec support.
- Exact frame indexing and variable-frame-rate navigation.
- Keyframe-aware seeking and decoder fallback.
- RAM hot cache and compressed disk cache.
- Perceptual hashes, SSIM, duplicate/near-duplicate grouping.
- Frame microscope timeline UI.
- Individual, range, all-frame, and unique-frame extraction.
- Production signing, release automation, and store distribution.

FrameScope follows one product rule:

> A feature belongs in FrameScope when it improves inspecting, navigating, comparing, or extracting video frames.

## Architecture

```text
Jetpack Compose UI
        ↓
MainViewModel
        ↓
FrameScopeRepository
        ↓
NativeBridge / JNI
        ↓
framescope-ffi
        ↓
framescope-video + framescope-core
        ↓
future decoder/index/cache layers
```

`framescope-cache` is currently an independent boundary because payload caching does not belong in Phase 1.

The Android layer owns lifecycle, UI, the Storage Access Framework, file-descriptor lifetime, and presentation. Rust owns media-domain validation and inspection. The selected SAF `ParcelFileDescriptor` is passed to JNI; Rust duplicates the descriptor before wrapping it in `File`, so Android retains ownership of the original descriptor.

See [`docs/architecture.md`](docs/architecture.md) for the detailed design and the FFmpeg decision.

## Platform baseline

- **minSdk 26**: keeps the project focused on a modern Android baseline without tying the architecture to recent-only APIs.
- **compileSdk 36 / targetSdk 36**: Android 16 baseline for the Phase 1 build.
- **ABI:** `arm64-v8a` only in Phase 1.
- **NDK:** r27d LTS (`27.3.13750724`).
- **JVM toolchain:** Java 17 bytecode.

Additional ABIs can be added later by extending the `abiFilters` and cargo-ndk targets; no media-domain code is ABI-specific.

## Requirements

For a local build:

- JDK 17 or newer capable of targeting Java 17.
- Gradle 8.13.
- Android SDK Platform 36 and Build Tools 36.x.
- Android NDK r27d (`27.3.13750724`).
- Rust stable with the `aarch64-linux-android` target.
- `cargo-ndk` available on `PATH`.

Typical Rust setup:

```bash
rustup target add aarch64-linux-android
cargo install cargo-ndk --locked
```

Set `ANDROID_HOME`/`ANDROID_SDK_ROOT` normally and make the NDK discoverable to cargo-ndk (for example with `ANDROID_NDK_HOME`). Android Studio users can install the required SDK/NDK packages from SDK Manager.

## Build Rust

From the repository root:

```bash
./scripts/build-rust.sh
```

The Gradle Android build also has a `buildRustArm64` task wired into `preBuild`, so normal APK builds compile the JNI library automatically.

## Build Android

```bash
cd android
gradle testDebugUnitTest lintDebug assembleDebug
```

The debug APK is produced under:

```text
android/app/build/outputs/apk/debug/app-debug.apk
```

The APK must contain:

```text
lib/arm64-v8a/libframescope_ffi.so
```

## Run the complete verification gate

```bash
./scripts/verify.sh
```

It runs Rust formatting, clippy, Rust tests, Android unit tests, Android lint, the debug APK build, and checks that the Rust `.so` is packaged inside the APK.

## Rust/Android bridge

Phase 1 uses direct JNI rather than UniFFI. There are only two narrow calls today:

- `framescope_version()` equivalent through `RustBridge.nativeVersion()`.
- `inspect_video(fd)` equivalent through `RustBridge.nativeInspectVideoFd(fd)`.

The JNI implementation is isolated in `framescope-ffi`; UI code never declares native methods. Responses are serialized as a small JSON envelope so Rust errors have stable codes and cross the boundary without unwinding.

Direct JNI was chosen because the surface is tiny, file descriptors are naturally represented as integers, and it avoids introducing code generation for two functions. If the cross-language model expands enough to justify generated bindings, the Android-facing `NativeBridge` interface lets that implementation change without rewriting the UI or application layer.

## Phase 1 media scope

The Rust parser handles metadata from conventional ISO BMFF MP4/MOV files. It deliberately does not decode frames. Estimated FPS is derived from the video track sample count and media duration when the non-fragmented `stts` table is available. Fragmented MP4, Matroska/WebM, AVI, unusual edit lists, and full codec/container coverage are deferred to the FFmpeg-backed Phase 2 engine.

No fallback to `MediaMetadataRetriever` is used, so the project is not architecturally locked to Android's media stack.

## Privacy and security

FrameScope Phase 1:

- processes selected videos locally;
- has no backend, authentication, account system, analytics, ads, telemetry, or remote API;
- declares no Android network permission;
- requests no broad filesystem permission;
- reads user-selected, device-local content through Android's Storage Access Framework;
- duplicates native file descriptors to keep ownership boundaries correct;
- validates Rust metadata on both sides of JNI before showing it.

See [`SECURITY.md`](SECURITY.md) for reporting guidance and [`docs/architecture.md`](docs/architecture.md) for trust boundaries.

## Roadmap

1. Phase 1: Foundation.
2. Phase 2: Video engine.
3. Phase 3: Frame indexing and caching.
4. Phase 4: Visual similarity and duplicate grouping.
5. Phase 5: Frame microscope UI.
6. Phase 6: Frame extraction.
7. Phase 7: Production hardening and releases.

See [`docs/roadmap.md`](docs/roadmap.md).

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) before contributing. Phase boundaries are intentional: avoid adding later-phase features before their architecture is ready.

## License

FrameScope is licensed under **GPL-3.0**. See [`LICENSE`](LICENSE).
