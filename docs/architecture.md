# FrameScope Architecture

## Phase 2 shape

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
framescope-ffmpeg
        ↓
FFmpeg

framescope-cache (source/cache identity contracts only; payload caching is Phase 3)
```

Dependencies are directional. Android owns platform/lifecycle concerns; Rust owns media semantics; FFmpeg is contained behind the native/video-engine boundary. Compose does not call JNI directly, Kotlin does not parse video timing independently, and the platform-neutral engine does not depend on Android APIs.

## Android application boundary

`MainActivity` is the composition root. It launches `LocalVideoOpenDocument`, an `ActivityResultContracts.OpenDocument` contract restricted to local `video/*` content, and forwards the returned URI string to `MainViewModel`. The manifest requests neither broad storage access nor `INTERNET`.

`MainViewModel` owns observable state, monotonically increasing inspection generations, cancellation, and stale-result suppression. It depends only on `FrameScopeRepository`.

`AndroidFrameScopeRepository` accepts only `content://` sources, queries the display name, opens a read-only `ParcelFileDescriptor`, and calls the native bridge on `Dispatchers.IO`. It stores an application `ContentResolver`, not an `Activity` context.

Descriptor ownership is explicit:

1. Kotlin owns the original `ParcelFileDescriptor` and keeps it open for the synchronous JNI call.
2. `framescope-ffi` creates only a temporary borrowed Rust view of that integer descriptor.
3. the FFmpeg backend immediately duplicates the descriptor;
4. FFmpeg owns/closes only that duplicate;
5. Kotlin's `use` block closes the original after JNI returns.

No URI-to-filesystem-path conversion or whole-file `ByteArray` copy exists in the application path.

## Cancellation and lifecycle

Every native inspection has an operation ID. The ViewModel asks the repository to cancel the active native operation before cancelling the coroutine job. Rust maps operation IDs to cloneable `CancellationToken`s; FFmpeg's interrupt callback and the decoder loop observe the same atomic cancellation state.

The token registry preserves a cancellation that races just before native startup and removes completed operations. The registry is bounded against accumulated cancellation tombstones.

Generation checks prevent progress or results from an older operation from replacing newer UI state. `onCleared()` performs the same native-first cancellation sequence.

## JNI boundary

Direct JNI is isolated in `framescope-ffi`.

The Android-facing surface is intentionally narrow:

- engine version;
- inspect a selected video file descriptor plus operation ID;
- cancel an operation ID.

The JNI entry point catches Rust panics before they can unwind across JNI and serializes stable success/error envelopes. User-controlled media failures are represented with typed `FrameScopeError` categories.

Kotlin validates bounds on the Rust response and presents it. It does not derive timestamps, stream selection, codec/container facts, or rotation from a separate Android media parser.

## Rust crates

### `framescope-core`

Platform-neutral media-domain types and stable errors:

- exact rational/time-base types;
- signed presentation timestamps and durations;
- stream/container metadata;
- decoded-frame metadata;
- stable error categories/codes.

Timestamp-to-microsecond conversion uses checked integer arithmetic rather than floating-point FPS math.

### `framescope-video`

The platform-neutral Phase 2 video engine. `VideoDecoder` owns:

- deterministic supported-video-stream selection;
- all-stream discovery metadata;
- sequential frame decoding;
- exact FFmpeg PTS + stream-time-base semantics;
- timestamp-derived observed cadence information;
- decode epochs and foundational seeking;
- cancellation propagation;
- typed error mapping.

The decoder is streaming. It does not preload the source, predecode all frames, or retain frame pixel buffers. Frame index is identity within a decode epoch and is never converted into presentation time.

Default stream selection prefers a supported video stream marked default, then the lowest supported video-stream index. `VideoStreamSelection::Index` provides explicit override.

The legacy bounded ISO-BMFF inspector remains available as Phase 1 code, but Android Phase 2 inspection uses `VideoDecoder`/FFmpeg rather than duplicating media metadata parsing in Kotlin.

See [`video-engine.md`](video-engine.md).

### `framescope-ffmpeg`

The narrow native ownership/build boundary around FFmpeg.

The C shim owns one `AVFormatContext`, one `AVCodecContext`, one reusable `AVPacket`, one reusable `AVFrame`, optional custom `AVIOContext` state, and the duplicated descriptor. One destroy path releases them on normal drop and open/decode failures.

The packet/decode loop:

- retains a packet when `avcodec_send_packet` returns `EAGAIN`;
- drains available frames before submitting more packets;
- sends the decoder flush packet at demux EOF;
- handles delayed frames and stable decoder EOF;
- clears packet/frame/flush state after seeking.

The Rust wrapper is `Send` but not `Sync`; mutable decoder operations require `&mut self`. Unsafe code is limited to the FFI/native lifetime boundary and the documented cancellation pointer.

Android uses FFmpeg 9.0.1 from the official source archive, pinned by SHA-256 and cross-compiled with NDK r27d for API 26 / `arm64-v8a`. The build enables built-in H.264, HEVC, VP9, and AV1 video decoders and MOV, Matroska, and AVI demuxers. FFmpeg archives are statically linked into `libframescope_ffi.so`.

See [`ffmpeg-android.md`](ffmpeg-android.md).

### `framescope-ffi`

The only Rust crate allowed to depend on JNI. It:

- borrows the Android descriptor long enough for the video engine to duplicate it;
- opens `VideoDecoder`;
- projects Rust-owned metadata into the Android response;
- samples at most a small bounded prefix of decoded frames for early VFR evidence;
- maps operation cancellation into the video engine.

A bounded sample can prove VFR when differing decoded PTS intervals are observed, but it cannot prove whole-source CFR. The Android bridge therefore reports `variable_frame_rate=true` only on observed variation and otherwise leaves the whole-source classification unknown.

### `framescope-cache`

Only Phase-3-ready identity/boundary primitives exist:

- path-independent `SourceVideoIdentity`;
- deterministic/versioned cache namespace derivation;
- invalidation/clear contract;
- `NoopCacheStore` for wiring/tests before payload caching exists.

There is no frame payload cache, eviction policy, or disk frame format in Phase 2.

## Timestamp model

The media clock is FFmpeg presentation time, not FPS.

For each decoded frame the native layer uses `AVFrame.best_effort_timestamp`, falling back to `AVFrame.pts` when needed, and interprets that value with the selected `AVStream.time_base`. Rust preserves the original ticks/time base and offers a checked integer microsecond convenience conversion.

Average/nominal frame-rate values are informational metadata only. VFR sources remain correct because no time is derived from `frame_index / fps`.

## Seeking

`seek_to_timestamp_us` is a foundation for later indexing, not an exact arbitrary-frame promise. The engine rescales the requested timestamp into the selected stream's time base, performs an FFmpeg seek, flushes decoder buffers, clears packet/frame EOF state, increments the decode epoch, and resets the epoch-local frame index.

Phase 3 can build precise navigation/indexing above this without changing Phase 2's timing contract.

## Large-video invariant

FrameScope must remain safe for multi-gigabyte sources.

Permanent rules:

1. never read an entire source video into RAM;
2. never predecode an entire video into raw frame files for navigation;
3. inspect/decode incrementally from the source descriptor;
4. keep future frame payloads behind bounded caches;
5. prefer SAF file descriptors over copied temporary source files.

The Phase 2 decoder follows these rules with reusable packet/frame storage and no cross-JNI pixel-plane copy.

## Android/NDK baseline

- `minSdk 26`
- `compileSdk 36`
- `targetSdk 36`
- `arm64-v8a` current ABI
- NDK r27d (`27.3.13750724`)
- Gradle 8.13 / Android Gradle Plugin 8.13.2
- Kotlin 2.4.10
- Jetpack Compose BOM 2026.06.00
- Java 17 bytecode

Adding another Android ABI requires a matching Rust target and verified FFmpeg prefix, not media-engine architecture changes.

## CI trust boundary

GitHub CI separately verifies:

- host Rust formatting, `clippy -D warnings`, and workspace tests;
- deterministic real-media fixture generation and ffprobe contracts;
- real decoder integration tests through the host `system-ffmpeg` feature;
- pinned Android FFmpeg provenance/build plus arm64 Rust JNI linking;
- Android unit tests and lint;
- APK assembly, ABI/native-library packaging, and permission invariants.

Routine PR builds do not upload generated media or debug APKs. Obsolete runs for the same PR are cancelled.

See [`ci.md`](ci.md).

## Deferred to Phase 3+

Phase 2 intentionally does not implement:

- persistent full frame/timestamp indexing;
- RAM hot-frame cache;
- compressed disk-frame cache;
- perceptual hashing / SSIM / duplicate grouping;
- exact indexed arbitrary-frame navigation UI;
- frame extraction/export.

Those layers must consume the timestamp-driven decoder contract rather than replacing it with FPS-derived timing.

## Security/privacy boundary

Normal app operation has no network dependency and the manifest declares no `INTERNET` permission. There are no keys, accounts, analytics SDKs, telemetry SDKs, ad SDKs, or cloud services.

The selected media is untrusted input. The engine bounds stream counts/dimensions, validates time bases and metadata, uses checked timestamp conversion, maps malformed input to typed errors, and contains panics at the JNI boundary. Native FFmpeg resources are released through deterministic ownership/drop paths.
