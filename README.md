# FrameScope

FrameScope is an open-source Android application for precise video-frame inspection. It is intentionally local-first: videos stay on the device, the app has no backend, and the normal application path makes no network requests.

This repository has completed the **Phase 2 video-engine foundation**. Android can select a local video through the Storage Access Framework, pass its file descriptor through JNI, and inspect/decode it with a Rust-owned FFmpeg backend without copying the whole source into RAM.

## Implemented through Phase 2

- Kotlin + Jetpack Compose Android application.
- `Compose -> ViewModel -> repository -> JNI -> Rust video engine -> FFmpeg` dependency flow.
- Android `OpenDocument` picker restricted to local `video/*` content.
- No broad storage permission and no `INTERNET` permission.
- Application-scoped `ContentResolver`; blocking descriptor/native work runs on `Dispatchers.IO`.
- Stale-result suppression and operation-scoped native cancellation.
- Borrowed Android `ParcelFileDescriptor` ownership with an immediate native `dup`; Android closes the original and FFmpeg closes only its duplicate.
- Rust workspace with `framescope-core`, `framescope-video`, `framescope-cache`, `framescope-ffi`, and `framescope-ffmpeg`.
- Pinned source-built FFmpeg 9.0.1 for Android API 26+ / `arm64-v8a`.
- Built-in software decoders for H.264, HEVC/H.265, VP9, and AV1.
- MP4/MOV, Matroska/WebM, and AVI demuxer support in the Android FFmpeg build.
- Deterministic video-stream selection with explicit override support.
- Sequential frame decoding with exact FFmpeg presentation timestamps and stream time bases.
- Variable-frame-rate-safe timing. Frame time is never derived from `frame_index / fps`.
- Foundational timestamp/keyframe seeking with decoder flush and decode epochs.
- Rotation, dimensions, duration, codec/container, stream counts, and pixel-format metadata.
- Typed malformed/corrupt/unsupported/cancellation errors instead of media-triggered panics crossing JNI.
- Deterministic generated video fixtures plus real decoder integration tests in CI.
- Android APK verification for the Rust JNI library, ABI, and storage/network permission invariants.

## Not implemented yet

These belong to later phases and are intentionally absent from Phase 2:

- full persistent frame/timestamp indexing;
- RAM hot-frame cache and compressed disk-frame cache;
- perceptual hashing, SSIM, or duplicate grouping;
- microscope timeline/frame-navigation UI;
- frame extraction/export;
- hardware MediaCodec acceleration;
- production signing/store release automation.

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
framescope-ffmpeg
        ↓
FFmpeg
```

`framescope-cache` currently contains source/cache identity contracts only. Frame payload caching remains Phase 3 work.

Android owns lifecycle, UI, Storage Access Framework access, and the original descriptor. Rust owns video-engine metadata/timing semantics and FFmpeg resources. Kotlin validates and presents Rust metadata but does not independently parse media timing/container metadata.

See [`docs/architecture.md`](docs/architecture.md), [`docs/video-engine.md`](docs/video-engine.md), and [`docs/android-video-bridge.md`](docs/android-video-bridge.md).

## Timestamp model

Presentation timestamps are the authority.

Each decoded frame may expose:

- FFmpeg best-effort PTS;
- the exact selected-stream time base;
- a checked integer microsecond convenience conversion;
- frame duration when FFmpeg exposes it;
- a sequential frame `index` scoped to a `decode_epoch`.

The frame index is identity/navigation data, not a clock. Average/nominal FPS values are informational only.

The Android inspection bridge examines only a bounded prefix of decoded frames. If differing PTS intervals are observed it can report VFR. A constant prefix does not prove a whole file is CFR, so the bridge leaves whole-source CFR status unknown rather than making a false claim.

## Android / native baseline

- **minSdk:** 26
- **compileSdk / targetSdk:** 36
- **ABI:** `arm64-v8a` only
- **NDK:** r27d (`27.3.13750724`)
- **JVM:** Java 17 bytecode
- **Android Rust build toolchain:** Rust 1.86.0 + cargo-ndk 4.1.2
- **Workspace host MSRV:** Rust 1.85
- **FFmpeg:** 9.0.1, official source archive pinned by SHA-256

The Android FFmpeg build is static and source-built. The APK contains `lib/arm64-v8a/libframescope_ffi.so`; it does not ship separate dynamic `libav*.so` files.

See [`docs/ffmpeg-android.md`](docs/ffmpeg-android.md) for source provenance, configure scope, licensing, and reproduction details.

## Local requirements

- JDK 17+
- Gradle 8.13
- Android SDK Platform 36 / Build Tools 36.x
- Android NDK `27.3.13750724`
- Rust with `aarch64-linux-android`
- `cargo-ndk`
- `curl`, `sha256sum`, `tar`, and `make` for the FFmpeg source build

Typical Rust setup:

```bash
rustup toolchain install 1.86.0 --target aarch64-linux-android
cargo +1.86.0 install cargo-ndk --locked --version 4.1.2
```

Set `ANDROID_HOME`/`ANDROID_SDK_ROOT` normally and expose the pinned NDK through `ANDROID_NDK_HOME` or its standard SDK path.

## Build the Android native library

From the repository root:

```bash
./scripts/build-rust.sh
```

This creates or reuses a verified source-built FFmpeg prefix and builds the Rust JNI library with cargo-ndk.

## Build Android

```bash
cd android
gradle --no-daemon testDebugUnitTest lintDebug assembleDebug
```

The debug APK is produced at:

```text
android/app/build/outputs/apk/debug/app-debug.apk
```

The APK must contain:

```text
lib/arm64-v8a/libframescope_ffi.so
```

## Complete local verification

```bash
./scripts/verify.sh
```

For the real host decoder fixture suite, install FFmpeg development libraries and run:

```bash
./scripts/generate-video-fixtures.sh
./scripts/verify-video-fixtures.py
cd rust
cargo clippy -p framescope-video --features system-ffmpeg --all-targets -- -D warnings
cargo test -p framescope-video --features system-ffmpeg
```

GitHub CI runs the host Rust gate, real video fixtures/decoder tests, the pinned Android FFmpeg/Rust native build, Android unit tests/lint/APK assembly, and native packaging/privacy verification. See [`docs/ci.md`](docs/ci.md).

## Supported Phase 2 media surface

Android software video decoders:

- H.264
- HEVC/H.265
- VP9
- AV1

Android demuxers:

- MP4/MOV family
- Matroska/WebM family
- AVI

Audio streams are discoverable but audio decoding/playback is not a Phase 2 feature.

## Privacy and security

FrameScope:

- processes selected videos locally;
- has no backend, accounts, analytics, ads, telemetry, or remote API;
- declares no Android network permission;
- requests no broad filesystem permission;
- accepts selected content through Android SAF rather than filesystem-path conversion;
- does not load an entire source video into RAM;
- catches panics at the JNI boundary and maps ordinary media failures to typed errors;
- keeps generated FFmpeg/native build state out of version control.

See [`SECURITY.md`](SECURITY.md) for reporting guidance.

## Roadmap

1. Phase 1: Foundation. **Complete.**
2. Phase 2: Video engine. **Complete / integration acceptance.**
3. Phase 3: Frame indexing and caching.
4. Phase 4: Visual similarity and duplicate grouping.
5. Phase 5: Frame microscope UI.
6. Phase 6: Frame extraction.
7. Phase 7: Production hardening and releases.

See [`docs/roadmap.md`](docs/roadmap.md).

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) before contributing. Phase boundaries are intentional: avoid adding later-phase features before their architecture is ready.

## License

FrameScope is licensed under **GPL-3.0-only**. See [`LICENSE`](LICENSE).
