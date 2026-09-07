# FrameScope

FrameScope is an open-source Android application for precise, local-first video-frame inspection. Videos stay on the device. The normal app path has no backend, telemetry, analytics, ads, accounts, or network requirement.

The repository has completed the Phase 2 video engine and implemented the Phase 3 frame-index/navigation/cache stack. Phase 3 is currently undergoing its final integration acceptance review before Phase 4 begins.

## Implemented

- Kotlin + Jetpack Compose Android foundation.
- `Compose -> ViewModel -> repository -> JNI -> Rust -> FFmpeg` dependency flow.
- Local video selection through Android Storage Access Framework.
- No broad storage permission and no `INTERNET` permission.
- Blocking descriptor/native work kept off the Android main thread.
- Borrowed Android `ParcelFileDescriptor` with immediate native `dup`; Android and Rust/FFmpeg close only what they own.
- Pinned source-built FFmpeg 9.0.1 for Android API 26+ / `arm64-v8a`.
- H.264, HEVC/H.265, VP9, and AV1 software video decoding.
- MP4/MOV, Matroska/WebM, and AVI demuxing in the Android FFmpeg build.
- Deterministic video-stream selection.
- Sequential decoding with FFmpeg presentation timestamps and exact stream time bases.
- Variable-frame-rate-safe timing. Frame time is never derived from `frame_index / fps`.
- Cancellation, stable EOF, malformed-media errors, seek/decoder flush foundations, and explicit FFmpeg ownership.
- Persistent SQLite frame index with schema versioning and explicit incomplete/complete lifecycle.
- Persistent global `FrameId` separate from decoder-local frame counters and presentation time.
- Path-independent source identity with bounded BLAKE3 sampled content fingerprinting for reusable derived state.
- Exact frame/timestamp lookup and persisted safe earlier keyframe anchors.
- Indexed random navigation using seek -> reconcile -> decode-forward semantics.
- Rust-owned full-resolution RGBA frame boundary; reusable FFmpeg `AVFrame` memory never escapes native lifetime.
- Byte-bounded RAM hot-frame cache weighted by actual pixel bytes.
- Byte-bounded compressed JPEG/WebP disk proxy cache.
- Typed separation between full source-quality RGBA and lossy preview proxies.
- Preview hierarchy: RAM full frame -> disk proxy -> authoritative indexed source decode.
- Source invalidation, corrupt-cache recovery, atomic proxy writes, and cache-deletion/I/O fallback behavior.
- Deterministic generated media fixtures, real FFmpeg decoder tests, Phase 3 stress contracts, Android lint/tests, native linking checks, and APK verification in GitHub Actions.

## Not implemented yet

These are intentionally deferred to later phases:

- perceptual similarity / SSIM and duplicate or near-duplicate grouping;
- frame-microscope navigation UI and timeline experience;
- PNG/JPEG/WebP frame extraction/export workflows;
- hardware MediaCodec acceleration;
- production signing and public GitHub Release automation.

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
framescope-video ───────── framescope-cache
        ↓                         ↑
framescope-ffmpeg                 │
        ↓                 index + bounded caches
FFmpeg
```

Android owns UI/lifecycle/SAF and the original descriptor. Rust owns timing semantics, stream selection, persistent frame identity, index/navigation/cache policy, and FFmpeg resources.

See [`docs/architecture.md`](docs/architecture.md), [`docs/video-engine.md`](docs/video-engine.md), [`docs/frame-index.md`](docs/frame-index.md), and [`docs/ffmpeg-android.md`](docs/ffmpeg-android.md).

## Timing model

Presentation timestamps are the media clock.

Each decoded frame can carry:

- FFmpeg `best_effort_timestamp` with PTS fallback;
- exact selected-stream rational time base;
- checked integer microsecond conversion;
- optional frame duration;
- decoder-local index scoped to a decode epoch.

Phase 3 separately assigns persistent `FrameId`s in presentation order. Neither identity is used to derive time. Average/nominal FPS values are informational only.

## Phase 3 navigation and caching

Random access uses persisted keyframe anchors:

```text
FrameId
  ↓
index lookup
  ↓
safe earlier keyframe
  ↓
FFmpeg seek + decoder flush
  ↓
reconcile actual presentation metadata
  ↓
decode forward
  ↓
requested frame
```

The first decoded frame after seek is never assumed to be the requested frame. If a seek cannot be reconciled precisely, FrameScope falls back to a fresh verified decode path rather than returning the wrong frame.

Full-quality requests use only RAM or authoritative source decode. Lossy disk proxies can never satisfy extraction/source-quality APIs.

## Large-video invariant

FrameScope is designed so video duration does not imply unbounded memory usage:

- the source is streamed rather than read in full;
- indexing persists bounded metadata batches;
- full-resolution cache is bounded by bytes;
- disk proxy cache is bounded by bytes;
- caches are disposable and the source remains authoritative;
- random access uses keyframe anchors instead of routinely decoding from frame zero.

## Android / native baseline

- **minSdk:** 26
- **compileSdk / targetSdk:** 36
- **ABI:** `arm64-v8a`
- **NDK:** `27.3.13750724`
- **Java:** 17
- **Android Rust:** 1.86.0
- **Host Rust MSRV:** 1.85
- **FFmpeg:** 9.0.1, official source archive pinned by SHA-256

## Build

Native Android library:

```bash
./scripts/build-rust.sh
```

Android tests/lint/debug APK:

```bash
cd android
gradle --no-daemon testDebugUnitTest lintDebug assembleDebug
```

Debug APK output:

```text
android/app/build/outputs/apk/debug/app-debug.apk
```

Complete local verification:

```bash
./scripts/verify.sh
```

GitHub Actions is the canonical heavy verifier and additionally runs deterministic real-media FFmpeg/Phase 3 contracts.

## Privacy and security

FrameScope:

- processes selected media locally;
- requests no broad filesystem permission;
- declares no Android network permission;
- never converts `content://` URIs into fake filesystem paths;
- does not load an entire selected video into RAM;
- treats media, indexes, and cache files as untrusted/rebuildable input;
- catches panics at the JNI boundary and maps ordinary media failures to typed errors;
- keeps FFmpeg/native generated build state out of version control.

See [`SECURITY.md`](SECURITY.md).

## Roadmap

1. Phase 1: Foundation. **Complete.**
2. Phase 2: Video engine. **Complete.**
3. Phase 3: Frame indexing and caching. **Implemented, final acceptance in progress.**
4. Phase 4: Visual similarity and duplicate grouping.
5. Phase 5: Frame microscope UI.
6. Phase 6: Frame extraction.
7. Phase 7: Production hardening and GitHub releases.

See [`docs/roadmap.md`](docs/roadmap.md).

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md). Phase boundaries are intentional: later-phase features should consume the accepted timing/index/navigation contracts rather than replacing them.

## License

FrameScope is licensed under **GPL-3.0-only**. See [`LICENSE`](LICENSE).
