# PHASE 1 REPORT

## Architecture

FrameScope Phase 1 is split into a platform/application side and a media-domain side:

```text
Jetpack Compose UI
        ↓
MainViewModel
        ↓
FrameScopeRepository
        ↓
NativeBridge
        ↓ JNI
framescope-ffi
        ↓
framescope-video + framescope-core

framescope-cache: independent cache identity/directory/invalidation contracts
```

The Android layer owns lifecycle, local-only Storage Access Framework selection, URI/file-descriptor lifetime, and presentation. Rust owns metadata inspection, validation, error classification, and the future media-processing boundary. There is no backend or network runtime dependency.

## Implemented

- Kotlin + Jetpack Compose Android foundation with a polished dark inspection UI.
- System `ACTION_OPEN_DOCUMENT` picker through an `ActivityResultContracts.OpenDocument` extension, filtered to `video/*` and marked `EXTRA_LOCAL_ONLY`.
- No Android permissions declared. `allowBackup` is disabled.
- ViewModel/repository/native-bridge separation with superseded inspection-job cancellation.
- Real JNI engine-version and file-descriptor inspection entry points.
- Bounded, seek-based Rust ISO BMFF metadata inspection for conventional MP4/MOV sources containing a `moov` box.
- Real duration, dimensions, standard rotation, and estimated average FPS when `stts` is present.
- Safe duplicate ownership of the Android SAF file descriptor with `dup(2)`.
- Stable Rust error codes serialized across JNI and revalidated on Kotlin before display.
- Versioned source cache identity/directory namespace and invalidation/clear boundary, without implementing the future frame cache.
- GPL-3.0 license, README, architecture/development/roadmap docs, security/contribution/community files, verification scripts, and public GitHub-hosted CI configuration.

## Rust crates

`framescope-core` owns platform-neutral `VideoMetadata`, validation, and `FrameScopeError` codes.

`framescope-video` owns the Phase 1 bounded MP4/MOV inspector. It walks only required ISO BMFF box ranges and does not read or decode the whole video.

`framescope-cache` owns `SourceVideoIdentity`, deterministic BLAKE3 cache namespace keys, a versioned path-safe `CacheDirectoryLayout`, and the `CacheStore` invalidation/clear contract plus a tested no-op implementation.

`framescope-ffi` is the only JNI crate. It duplicates the supplied descriptor, calls `framescope-video`, catches native panics at the inspection entry point, and serializes success/error envelopes.

## Android architecture

`MainActivity` is only the composition root and system-picker launcher. `MainViewModel` owns UI state. `AndroidFrameScopeRepository` owns Android URI/file-descriptor access on an IO dispatcher. `RustBridge` owns native loading, JNI declarations, response decoding, and boundary validation. Composables contain presentation only.

The baseline is minSdk 26, compileSdk/targetSdk 36, Java 17 bytecode, NDK r27d (`27.3.13750724`), and `arm64-v8a` for Phase 1.

## FFI approach

Direct JNI was chosen for the current two-call boundary because the file descriptor maps naturally to `jint` and generated bindings would add machinery without buying useful safety for this tiny interface. JNI code is isolated behind Kotlin `NativeBridge` and Rust `framescope-ffi`, so a future binding strategy can change without rewriting the UI/application layers.

R8/ProGuard has a keep rule for the JNI owner class so later minification cannot silently rename the native entry points.

## Video metadata approach

FFmpeg is intentionally deferred to Phase 2 rather than bundling a large native media stack before FrameScope decodes frames. Phase 1 uses a legitimate Rust metadata inspector for conventional ISO BMFF MP4/MOV. It reads box headers and small metadata payloads through `Read + Seek`, bounds attacker-controlled `stts` entry counts, checks arithmetic and parent bounds, and validates all surfaced metadata.

This parser is not intended to become FrameScope's permanent decoder. `framescope-video` is the stable boundary for the planned FFmpeg decoder/index implementation.

## Tests

The source tree contains 16 Rust unit tests and 11 Android/JVM unit-test methods.

Rust tests cover metadata validation/error codes, bounded MP4/MOV parsing, malformed/oversized boxes, hostile `stts` counts, rotation handling, cache source identity/directory layout, cache invalidation behavior, and FFI error serialization.

Android/JVM tests cover ViewModel success/failure/cancellation states, engine-load failure state, metadata formatting, JNI response parsing, missing engine identity, malformed metadata, and stable Rust error codes.

## Build verification

**Full production-style quality gate: NOT VERIFIED in this execution environment.** The provided sandbox does not contain `cargo`, `rustc`, `gradle`, `sdkmanager`, or `adb`. Running `./scripts/verify.sh` was attempted and stops immediately at `cargo fmt --all --check` with `cargo: command not found` (exit 127). Therefore this report does not claim that Rust fmt/clippy/tests, Android Gradle unit tests, lint, APK assembly, packaged `.so` inspection, or runtime device JNI invocation passed here.

Checks that were actually run successfully here:

- complete source-tree structural inspection;
- Bash syntax validation for both scripts;
- TOML, Android XML, and GitHub Actions YAML parsing;
- manifest scan confirming zero declared permissions;
- scans for generated binaries/caches, signing material, obvious secrets, cloud/tracking dependencies, and `MediaMetadataRetriever` references;
- source-level pairing of Kotlin JNI declarations to Rust JNI symbol names;
- pure Kotlin `VideoMetadata`/formatter compilation and execution with the available Kotlin compiler;
- compiled ViewModel state/cancellation harness using lightweight local lifecycle stubs;
- compiled repository success/error/file-descriptor ownership harness using lightweight Android API stubs.

The repository contains `scripts/verify.sh` and `.github/workflows/ci.yml` to run the actual Rust + Android gate on a machine/runner with the documented toolchain. The verify script also checks the final APK for `lib/arm64-v8a/libframescope_ffi.so` and, when `nm` is available, both expected JNI symbols.

## Known limitations

Phase 1 metadata inspection is limited to conventional seekable ISO BMFF MP4/MOV sources with a `moov` box. Fragmented MP4 may not provide the Phase 1 sample table needed for FPS, and non-seekable document descriptors fail gracefully as I/O errors. Matroska/WebM, AVI, broad codec/container support, edit-list-perfect timing, and actual frame decoding are not implemented.

Superseded Android inspection coroutines are canceled logically, but the short bounded Phase 1 native metadata call is synchronous once JNI is entered. Native cooperative cancellation belongs in the Phase 2 decoder interface.

Only `arm64-v8a` is configured in Phase 1.

## Deferred intentionally to Phase 2

FFmpeg integration, decoder lifecycle, codec/track selection, presentation timestamps, variable-frame-rate semantics, keyframe-aware seeking, exact single-frame decoding, and native cooperative cancellation.

Frame indexing/caching, similarity/SSIM, duplicate grouping, microscope navigation, and extraction remain in their later roadmap phases and are not hidden behind placeholders in Phase 1.

## ZIP

`FrameScope-phase1.zip`
