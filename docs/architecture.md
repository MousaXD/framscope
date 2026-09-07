# FrameScope Architecture

## Phase 1 shape

```text
Android Compose UI
        ↓
MainViewModel
        ↓
FrameScopeRepository
        ↓
NativeBridge
        ↓ JNI
framescope-ffi
        ↓
framescope-video ─── framescope-core
        ↓
future decoder/index services

framescope-cache (independent cache contracts in Phase 1)
```

The architecture separates platform concerns from media concerns. Kotlin is responsible for Android lifecycle, the Storage Access Framework, UI state, and presentation. Rust is responsible for media-domain inspection and will own heavy frame-processing logic.

## Android application boundary

`MainActivity` is a composition root. It launches a small `LocalVideoOpenDocument` extension of `ActivityResultContracts.OpenDocument` with `video/*`, adds Android's `EXTRA_LOCAL_ONLY` hint, and forwards the selected URI string to `MainViewModel`. It does not request storage permissions.

`MainViewModel` owns observable state and cancellation of superseded inspection jobs. It depends only on `FrameScopeRepository`, making application behavior unit-testable.

`AndroidFrameScopeRepository` resolves the SAF URI, obtains a display name, opens a read-only `ParcelFileDescriptor`, and calls `NativeBridge` on an IO dispatcher. The descriptor stays open for the synchronous native call and is then closed by Kotlin.

`RustBridge` owns library loading, native declarations, and decoding of the JNI response. No Composable or Activity contains native declarations.

## JNI choice

Phase 1 uses direct JNI in a dedicated `framescope-ffi` crate.

Why:

- the boundary contains only version inspection and file-descriptor-based metadata inspection;
- Android SAF already exposes a file descriptor, which maps cleanly to JNI `jint`;
- direct JNI avoids a code-generation dependency for a two-function interface;
- the Kotlin `NativeBridge` interface and Rust `framescope-ffi` crate isolate the choice, so a future move to generated bindings does not affect UI/application code.

The native side calls `dup(2)` before wrapping the descriptor in Rust `File`. Rust owns and closes only its duplicate. Panics are caught at the JNI entry point, and all ordinary errors are returned as a serialized result envelope with stable error codes.

## Rust crates

### `framescope-core`

Platform-neutral domain types and errors. Android-specific concepts must not enter this crate.

Current responsibilities:

- `VideoMetadata`;
- metadata safety validation;
- stable `FrameScopeError` categories/codes.

### `framescope-video`

Media inspection abstractions and the current Phase 1 ISO BMFF inspector.

The parser is seek-based and bounded. It walks box headers and reads only the small payload ranges required for metadata. It does **not** load the complete video into RAM. For conventional MP4/MOV it reads:

- `mvhd` / `mdhd` for timescales and duration;
- `hdlr` to identify the video track;
- `tkhd` for dimensions and common rotation matrices;
- `stts` sample counts for estimated FPS when available.

The crate has no Android dependency.

### `framescope-cache`

Phase 1 defines only boundaries that are already useful:

- path-independent `SourceVideoIdentity` suitable for SAF sources;
- deterministic cache namespace keys;
- versioned, path-safe `CacheDirectoryLayout` names beneath an Android-provided cache root;
- a small `CacheStore` invalidation/clear contract shared by future bounded RAM/disk stores;
- `NoopCacheStore` as a real, tested implementation while payload caching is absent.

It intentionally does not define frame blobs, eviction policy, or disk format yet.

### `framescope-ffi`

The only Rust crate allowed to depend on JNI. It converts Android file descriptors into owned duplicate descriptors, calls `framescope-video`, and serializes success/failure envelopes.

## Phase 1 video-stack decision

**FFmpeg is deferred to Phase 2.**

Bundling FFmpeg now would add a large native build, codec configuration, licensing/build-surface decisions, and ABI complexity before FrameScope decodes a single frame. Instead, Phase 1 proves the Rust/Android ownership path with a legitimate native MP4/MOV metadata parser.

This is not a permanent home-grown decoder strategy. `framescope-video` is the boundary where the Phase 2 decoder backend will be introduced. The current metadata parser can remain as lightweight format probing or be narrowed later without changing Android architecture.

FrameScope does not use `MediaMetadataRetriever` as its media engine or fallback. This avoids locking future frame semantics to Android framework decoding.

## Android/NDK baseline

- `minSdk 26`.
- `compileSdk 36`.
- `targetSdk 36`.
- `arm64-v8a` Phase 1 ABI.
- NDK r27d LTS.
- Gradle 8.13 + Android Gradle Plugin 8.13.2.
- Kotlin 2.4.10.
- Jetpack Compose BOM 2026.06.00.
- Java 17 bytecode.

Phase 1 deliberately stays on the stable Android 16/API 36 baseline instead of adopting the newest Compose train that requires compileSdk 37. The UI does not need Android 17 preview APIs, so that dependency would add churn without product value.

The ABI is deliberately explicit. Adding another architecture is a build configuration change plus another Rust target, not a media-engine rewrite.

## Large-video invariant

FrameScope must remain safe for multi-gigabyte sources.

Permanent rules:

1. Never read an entire source video into RAM.
2. Never pre-decode an entire video into raw PNG files for navigation.
3. Index metadata and compressed-source positions incrementally.
4. Keep frame payloads behind bounded caches.
5. Prefer source file descriptors/seekable streams over copied temporary source files when Android grants safe access.

Future navigation hierarchy:

```text
RAM hot cache
        ↓ miss
compressed disk cache
        ↓ miss
indexed source-video decoder fallback
```

## Future video engine: Phase 2

`framescope-video` will gain a decoder abstraction around an FFmpeg-based backend. The API must model:

- presentation timestamps rather than assuming constant FPS;
- variable-frame-rate tracks;
- stream/track selection;
- keyframes and seek points;
- decoder flush/restart behavior;
- pixel format/color-space metadata;
- cancellation and bounded decode work.

The application should request frame/timestamp operations, not manipulate FFmpeg handles directly.

## Future indexing and caching: Phase 3

A frame index will map logical inspection positions to source timestamps and decoder seek hints. It should be persisted compactly and invalidated by `SourceVideoIdentity`.

Caching will separate:

- **RAM hot cache:** a small bounded set of recently/adjacently inspected frames;
- **compressed disk cache:** efficient previews or future frame-cache representation, not raw unbounded PNG dumps;
- **decoder fallback:** source video remains the authority.

Eviction policy, on-disk schema/versioning, and cache-directory lifecycle belong in Phase 3.

## Future visual similarity: Phase 4

Similarity is a derived index, not a replacement for source timing. Planned layers:

1. inexpensive perceptual hash / coarse signal to find candidates;
2. optional SSIM or equivalent comparison for close candidates;
3. consecutive duplicate/near-duplicate groups represented as ranges with thresholds recorded in metadata.

A grouped range must retain original frame/timestamp mappings so extraction remains exact and reversible.

## Future microscope UI: Phase 5

The UI will consume indexed timestamps and cache state rather than control decoder internals. Planned interactions include exact previous/next frame, jump to meaningful visual change, precise timeline range selection, and adjacent-frame comparison.

FrameScope is an inspector, not a nonlinear video editor.

## Future extraction: Phase 6

Extraction will operate from the frame index and decoder backend. Planned modes:

- one frame;
- all frames;
- selected time/timestamp range;
- unique/group representatives.

The extraction pipeline must stream outputs and apply backpressure. It must never require all decoded frames to exist simultaneously in memory.

## Security/privacy boundary

Normal app operation has no network dependency and the manifest declares no `INTERNET` permission. There are no keys, accounts, analytics SDKs, telemetry SDKs, ad SDKs, or cloud services.

Untrusted input is the selected video container. Rust parsing therefore uses parent-bounded box ranges, overflow checks, entry-count limits, metadata validation, and non-panicking error propagation across JNI.
