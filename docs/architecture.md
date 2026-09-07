# FrameScope Architecture

## Phase 3 shape

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
        ↓                  ↑
framescope-ffmpeg          │
        ↓                  │
FFmpeg              framescope-cache
                    persistent metadata index
                    + source identity
```

Dependencies are directional. Android owns platform/lifecycle concerns; Rust owns media semantics;
FFmpeg is contained behind the native/video-engine boundary. Compose does not call JNI directly,
Kotlin does not parse video timing independently, and the platform-neutral engine/index layers do not
depend on Android APIs.

Phase 3 extends the accepted Phase 2 decoder with a persistent, metadata-only frame index. It does not
add a pixel cache. `framescope-video` streams decoded metadata into `framescope-cache`; future seek and
cache layers consume that index without replacing Phase 2's timestamp model.

## Android application boundary

`MainActivity` is the composition root. It launches `LocalVideoOpenDocument`, an
`ActivityResultContracts.OpenDocument` contract restricted to local `video/*` content, and forwards
the returned URI string to `MainViewModel`. The manifest requests neither broad storage access nor
`INTERNET`.

`MainViewModel` owns observable state, monotonically increasing inspection generations,
cancellation, and stale-result suppression. It depends only on `FrameScopeRepository`.

`AndroidFrameScopeRepository` accepts only `content://` sources, queries the display name, opens a
read-only `ParcelFileDescriptor`, and calls the native bridge on `Dispatchers.IO`. It stores an
application `ContentResolver`, not an `Activity` context.

Descriptor ownership is explicit:

1. Kotlin owns the original `ParcelFileDescriptor` and keeps it open for the synchronous JNI call.
2. `framescope-ffi` creates only a temporary borrowed Rust view of that integer descriptor.
3. the FFmpeg backend immediately duplicates the descriptor;
4. FFmpeg owns/closes only that duplicate;
5. Kotlin's `use` block closes the original after JNI returns.

No URI-to-filesystem-path conversion or whole-file `ByteArray` copy exists in the application path.
Phase 3's persistent index remains platform-neutral. Android integration may provide source metadata
or a separately owned seekable descriptor for bounded fingerprinting later; URI strings and display
names are never treated as source identity. If sufficiently strong identity evidence is unavailable,
the Rust index layer chooses rebuild over stale reuse.

## Cancellation and lifecycle

Every native inspection has an operation ID. The ViewModel asks the repository to cancel the active
native operation before cancelling the coroutine job. Rust maps operation IDs to cloneable
`CancellationToken`s; FFmpeg's interrupt callback and the decoder loop observe the same atomic
cancellation state.

Generation checks prevent progress or results from an older operation from replacing newer UI state.
`onCleared()` performs the same native-first cancellation sequence.

Phase 3 indexing uses the same decoder cancellation signal. A cancelled build may commit only already
validated metadata batches, then remains explicitly `incomplete`; it can never set the complete
marker.

## JNI boundary

Direct JNI is isolated in `framescope-ffi`. The Android-facing Phase 2 surface remains intentionally
narrow: engine version, inspect a selected video file descriptor plus operation ID, and cancel an
operation ID. The Phase 3 frame index does not add Android types or UI/JNI surface as part of Agent 1's
ownership.

The JNI entry point catches Rust panics before they can unwind across JNI and serializes stable
success/error envelopes. Kotlin validates bounds on the Rust response and presents it. It does not
derive timestamps, stream selection, codec/container facts, or rotation from a separate Android media
parser.

## Rust crates

### `framescope-core`

Platform-neutral media-domain types and stable errors: exact rational/time-base types, signed
presentation timestamps and durations, stream/container metadata, decoded-frame metadata, and stable
error categories. Timestamp-to-microsecond conversion uses checked integer arithmetic rather than
floating-point FPS math.

### `framescope-video`

`VideoDecoder` owns deterministic video-stream selection, stream discovery, sequential frame decoding,
exact FFmpeg PTS + stream-time-base semantics, decode epochs, foundational timestamp seeking,
cancellation, and typed errors. It never preloads or predecodes the whole source.

Phase 3 adds `build_or_resume_frame_index` above this decoder. It assigns persistent `FrameId`s in
display order, records exact decoded metadata, and writes bounded batches into `framescope-cache`.
Partial resume reopens a fresh decoder and reconciles the committed prefix before appending; it never
assumes decoder epoch-local index alone restores codec state.

Default stream selection prefers a supported default video stream, then the lowest supported video
stream index. `VideoStreamSelection::Index` provides explicit override. The legacy bounded ISO-BMFF
inspector remains available as Phase 1 compatibility code.

See [`video-engine.md`](video-engine.md) and [`frame-index.md`](frame-index.md).

### `framescope-ffmpeg`

The narrow native ownership/build boundary around FFmpeg. The C shim owns one `AVFormatContext`, one
`AVCodecContext`, one reusable `AVPacket`, one reusable `AVFrame`, optional custom `AVIOContext`
state, and the duplicated descriptor. One destroy path releases them on normal drop and errors.

The packet/decode loop retains packets across `EAGAIN`, drains frames before submitting more packets,
flushes at demux EOF, handles delayed frames/stable decoder EOF, and clears packet/frame/flush state
after seeking. The Rust wrapper is `Send` but not `Sync`; mutable operations require `&mut self`.

Android uses FFmpeg 9.0.1 pinned by SHA-256 and cross-compiled with NDK r27d for API 26 /
`arm64-v8a`. See [`ffmpeg-android.md`](ffmpeg-android.md).

### `framescope-ffi`

The only Rust crate allowed to depend on JNI. It borrows the Android descriptor long enough for the
video engine to duplicate it, opens `VideoDecoder`, projects Rust metadata into Android responses,
samples a bounded decoded prefix for early VFR evidence, and maps operation cancellation into the
video engine.

A bounded prefix can prove VFR when differing decoded PTS intervals are observed, but it cannot prove
whole-source CFR.

### `framescope-cache`

Phase 3 owns persistent source/index metadata here. The crate remains Android-independent and contains:

- layered, path-independent `SourceIdentity` plus bounded sampled fingerprinting;
- deterministic/versioned cache namespace derivation;
- `FrameId`, `FrameIndexEntry`, `FrameIndexStatus`, and `KeyframeAnchor` contracts;
- a versioned SQLite metadata index with migration/recreation policy;
- exact frame/timestamp/keyframe lookups and bounded range iteration;
- explicit building/incomplete/complete/failed-recoverable lifecycle state;
- source and selected-stream binding validation;
- corruption/stale-source recovery.

There is still no decoded-frame RAM cache, eviction policy, thumbnail/proxy cache, or disk pixel
format. The persistent index stores metadata rows only.

## Persistent frame-index model

Frame identity and time are separate concepts. `FrameId` is the zero-based persistent display-order
identity assigned by the indexer. `FrameIndexEntry` separately preserves the decoder's optional signed
presentation timestamp and exact stream time base, optional duration, keyframe/corrupt flags, and
nearest safe earlier anchor.

`FrameIndexStreamIdentity` binds the database to the selected stream using stream index, codec ID/name,
exact time base, and coded dimensions. Rotation is intentionally not an identity component because
display rotation does not alter the presentation timeline.

The SQLite schema is versioned with `PRAGMA user_version`. `index_meta` stores source/stream binding,
lifecycle, committed row count, optional complete count, and last recoverable error. `frame_index`
stores compact per-frame metadata. WAL journaling plus bounded `IMMEDIATE` transactions keeps each
persisted batch atomic; an explicit complete marker distinguishes a finished timeline from surviving
partial rows after process death.

Source mismatch, weak/unverifiable identity, invalid serialized state, impossible rows, SQLite
corruption/not-a-database errors, or unsupported schema versions cause safe recreation because this
database is derived state. See [`frame-index.md`](frame-index.md).

## Source identity

A video is never identified by filename, URI string, or display name. `SourceIdentity` layers source
size, modification metadata, provider/document ID, and content-derived evidence.

For seekable sources the built-in sampler hashes at most three 64 KiB windows at the beginning,
middle, and end with BLAKE3 and restores the caller's position. Metadata/provider identity without a
content-derived tag is deliberately not persisted for reuse; the index is rebuilt instead.

The sampled fingerprint is bounded and practical for multi-gigabyte files but is not proof that
unsampled bytes are unchanged. A caller with stronger content-derived evidence may supply it. When
identity cannot be established to the caller's required confidence, rebuilding is the safe fallback.

## Timestamp model

The media clock is FFmpeg presentation time, not FPS. For each decoded frame the native layer uses
`AVFrame.best_effort_timestamp`, falling back to `AVFrame.pts`, interpreted with the selected
`AVStream.time_base`. Rust preserves the original ticks/time base and offers checked integer
microsecond conversion.

Phase 3 persists those exact tick/time-base values. It never computes `timestamp = frame / fps`.
Average/nominal frame-rate values are informational only. VFR sources remain correct because frame
identity and presentation time are independent.

## Seeking and keyframe anchors

`seek_to_timestamp_us` remains a foundation, not an exact arbitrary-frame promise. A successful seek
rescales the timestamp, performs an FFmpeg seek, flushes decoder buffers, clears EOF state, increments
the decode epoch, and resets the epoch-local frame index.

For each persistent frame, Phase 3 stores the nearest earlier clean decoded keyframe as a
`KeyframeAnchor`, preferring exact timestamp information. Before any usable keyframe, the anchor is
`StreamStart`. Container packet byte offsets are deliberately not persisted because their semantics
are not reliably portable across custom AVIO/demuxers/media layouts.

Agent 2 can use the anchor as a safe earlier decode-start hint, then decode forward to the target
`FrameId`.

## Interruption and resume

Indexing can stop on cancellation, process death, storage failure, or decode failure without claiming
completion. Already committed transactional batches remain valid; lifecycle communicates whether the
whole timeline is complete.

Resume is correctness-first. The current implementation starts a fresh decoder from stream start,
replays the committed prefix, and compares every reconstructed entry with persistent state. Only after
that prefix matches are new rows appended. A mismatch or premature EOF discards the partial timeline
and performs one clean rebuild. This repeats some decoding but does not fake decoder state restoration
from `last_frame + 1`.

A future optimization may seek to an earlier persisted keyframe checkpoint, provided it keeps the same
reconciliation guarantee.

## Large-video invariant

FrameScope must remain safe for multi-gigabyte sources:

1. never read an entire source video into RAM;
2. never predecode an entire video into raw frame files for navigation;
3. inspect/decode/index incrementally from the source descriptor;
4. persist frame-index metadata in bounded transactions;
5. keep future frame payloads behind bounded caches;
6. prefer SAF file descriptors over copied temporary source files.

The Phase 2 decoder uses reusable packet/frame storage. The Phase 3 indexer retains only a bounded
metadata batch (256 entries by default) and never stores decoded pixel planes.

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

Adding another Android ABI requires a matching Rust target and verified FFmpeg prefix, not
media-engine architecture changes.

## CI trust boundary

GitHub CI verifies host Rust formatting/clippy/tests, deterministic real-media fixtures, real decoder
integration through `system-ffmpeg`, pinned Android FFmpeg provenance/build, arm64 Rust JNI linking,
Android unit/lint/APK checks, and Phase 3 frame-index unit/fixture tests including actual VFR PTS
comparison. Obsolete PR runs are cancelled.

See [`ci.md`](ci.md).

## Deferred to later phases

Phase 3 Agent 1 intentionally does not implement RAM hot-frame caching, compressed disk-frame/proxy
caching, perceptual hashing/SSIM/duplicate grouping, extraction/export, microscope UI, or Android
arbitrary-frame viewing. Those layers consume the timestamp-driven decoder and persistent metadata
index rather than replacing them with FPS-derived timing.

## Security/privacy boundary

Normal app operation has no network dependency and the manifest declares no `INTERNET` permission.
The selected media and persistent index are treated as untrusted/rebuildable input. The engine bounds
stream counts/dimensions, validates time bases, uses checked timestamp conversion, maps malformed
input to typed errors, contains panics at the JNI boundary, and validates persistent index
schema/source/row/anchor consistency before reuse.
