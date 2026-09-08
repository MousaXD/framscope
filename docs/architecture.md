# FrameScope Architecture

## Current architecture through Phase 6

```text
Android Compose UI
        ↓
MainViewModel + export ViewModels
        ↓
FrameScopeRepository
        ↓
JNI control calls + SAF output callbacks
        ↓
framescope-ffi
        ↓
video / index / cache / similarity / extraction layers
        ↓
FFmpeg + bounded derived state + streaming encoders/manifests
```

Dependencies remain directional. Android owns platform UI, lifecycle, Storage Access Framework access, cache-directory selection, and the original source/output descriptors. Rust owns media timing, stream selection, persistent frame identity, indexing, navigation semantics, cache policy, similarity validation, source-quality extraction orchestration, image encoding, manifest semantics, and FFmpeg resource ownership. Core Rust crates do not depend on Android APIs.

FrameScope is local-first. Normal application behavior requires no backend, and the Android manifest has no broad storage or network permission.

## Source and descriptor ownership

The Android source path is:

```text
content:// URI
    ↓
ContentResolver
    ↓
ParcelFileDescriptor owned by Android
    ↓ borrowed integer fd through JNI
Rust / framescope-ffi
    ↓ immediate dup(fd)
framescope-ffmpeg / FFmpeg owns duplicate only
```

The URI is never converted into a pretend filesystem path and the whole video is never copied into a `ByteArray` or application-private source file. Android closes its original descriptor after the synchronous native ownership handoff/call boundary. Native code closes only the duplicate it owns.

Seekable descriptor reads use a private logical offset so normal decoder work does not mutate the caller's shared file position.

Export destinations follow the same ownership rule in the opposite direction. Android creates SAF documents and owns their `ParcelFileDescriptor`s. Rust receives borrowed writable descriptor numbers, duplicates them for the synchronous encode/write operation, flushes and closes only its duplicates, then reports success/failure back to Android. Android decides whether the newly created document/workspace commits or is deleted.

## Timestamp and frame identity model

Presentation timestamps are authoritative. The decoder uses FFmpeg `best_effort_timestamp`, with frame PTS fallback, interpreted in the selected stream's exact rational time base.

FrameScope deliberately has two different identities:

- the decoder-local sequential index, scoped to a decode epoch;
- Phase 3 persistent `FrameId`, the zero-based presentation-order identity stored in the frame index.

Neither is a clock. FrameScope never computes `timestamp = frame_index / fps`. Average and nominal FPS are informational metadata only, so CFR and VFR media share the same timing model.

The persistent `FrameIndexEntry` stores the exact optional presentation timestamp, optional duration, keyframe/corrupt state, and nearest safe earlier keyframe anchor.

## Persistent frame index

`framescope-cache` owns the Android-independent SQLite frame index.

The database contains:

- versioned schema state via `PRAGMA user_version`;
- `index_meta`, binding source identity, selected stream identity, lifecycle, committed row count, optional completed frame count, and recoverable error information;
- `frame_index`, storing compact per-frame timing/keyframe metadata;
- indexes for exact timestamp/timestamp-microsecond lookup.

Index lifecycle is explicit: building, incomplete, complete, or failed-recoverable. A partial database can never masquerade as complete.

Writes use bounded `IMMEDIATE` transactions. The indexer retains only a bounded metadata batch, 256 entries by default, and never stores decoded pixel planes in SQLite.

Persistent-state validation rejects inconsistent row counts, non-contiguous frame IDs, invalid time bases/durations, malformed booleans, impossible anchors, or anchors that do not refer to clean persisted keyframes. Because the index is derived data, supported corruption/schema failures trigger safe recreation rather than a media-triggered panic.

## Source identity and invalidation

A persistent index, cache entry, or reusable similarity result is never keyed only by filename, display name, filesystem path, or URI text.

`SourceIdentity` may include size, modification metadata, provider/document identity, and content-derived evidence. Reusable persistent state requires content-derived evidence plus source size.

For seekable sources, the built-in identity sampler reads at most three 64 KiB windows from the beginning, middle, and end, hashes them with BLAKE3, and restores the caller's seek position. This avoids hashing a multi-gigabyte source in full before work can begin.

The sampled fingerprint is documented as bounded evidence rather than mathematical proof that every unsampled byte is unchanged. Callers may provide a stronger content-derived tag. When reusable identity cannot be established, FrameScope rebuilds or refuses persistence of derived state instead of risking stale reuse.

## Index build and recovery

`framescope-video::build_or_resume_frame_index` streams decoded metadata into the persistent index.

For a partial index, recovery is correctness-first: a fresh decoder replays from stream start and reconciles already committed entries before appending new rows. If the persistent prefix no longer matches, FrameScope discards it and performs one clean rebuild. It does not pretend that `last_frame + 1` reconstructs codec state.

Cancellation may commit only already validated batches, then leaves the lifecycle incomplete. Decode/storage failures never set the complete marker.

## Indexed navigation

Random frame access uses the persistent index instead of an FPS estimate:

```text
target FrameId
    ↓
FrameIndex lookup
    ↓
nearest safe earlier keyframe anchor
    ↓
FFmpeg timestamp seek
    ↓
decoder flush/reset
    ↓
reconcile actual decoded presentation metadata with anchor
    ↓
decode forward
    ↓
requested persistent FrameId
```

The first decoded frame after a seek is never assumed to be the target. If a demuxer seek cannot be reconciled precisely, the navigation path reopens a fresh decoder and verifies from stream start rather than returning a potentially wrong frame.

Timestamp navigation exposes explicit at-or-before, at-or-after, and nearest policies. VFR timing remains PTS-driven.

## Owned pixel boundary

The reusable FFmpeg `AVFrame` never escapes the native decoder session.

After a successful decode, `framescope-ffmpeg` can copy the current frame into a caller-owned RGBA buffer through libswscale. The native boundary validates dimensions, stride arithmetic, output capacity, source frame validity, and complete conversion. The local swscale context is freed before return.

Rust exposes the result as owned RGBA bytes. Subsequent decoder advancement or seeking cannot invalidate an already returned Rust-owned frame.

The metadata-only decode API remains available so indexing does not pay full-resolution RGBA conversion/allocation cost.

## Cache hierarchy

The accepted pixel hierarchy has two bounded tiers:

```text
source-quality request:
RAM full-resolution RGBA
        ↓ miss
authoritative indexed source decode

preview request:
RAM full-resolution RGBA
        ↓ miss
compressed disk proxy
        ↓ miss/error
authoritative indexed source decode
```

### RAM hot cache

The RAM tier stores `OwnedRgbaFrame` payloads keyed by strong source identity, selected stream, and persistent `FrameId`.

It is bounded by actual resident bytes, not frame count. A 4K frame therefore consumes more budget than a small frame. Entries own their memory through reference-counted byte storage, so cache hits do not duplicate the full pixel buffer.

Eviction is recency-based and source invalidation removes only matching entries.

### Disk proxy cache

The disk tier stores compressed JPEG/WebP navigation proxies in a versioned source/stream/frame namespace. Proxy payloads are typed separately from source-quality RGBA so microscope presentation and Phase 6 extraction cannot accidentally consume a lossy preview.

Writes use a temporary file in the destination directory, flush it, and rename it into place. Cache scans ignore symlinks and temporary files. Corrupt/oversized entries are rejected and removed. A global byte budget drives eviction.

Disk proxy storage is disposable. Read/write failure must not make the source frame unavailable. Preview navigation degrades to authoritative indexed decode and can return a non-fatal cache diagnostic instead of failing the media request.

## Similarity and group navigation

Phase 4 similarity is a derived view over the Phase 3 timeline. It never deletes source frames or changes persistent `FrameId` identity.

The hybrid grouping path uses deterministic dHash as a rejection prefilter and confirmed full-pixel luma similarity for acceptance. Candidates must satisfy the configured threshold against both the previous frame and the canonical group representative so transitive visual drift cannot silently merge a long chain.

Similarity groups are persisted as versioned derived SQLite state bound to strong source identity, selected stream identity, and the full grouping policy. Writes are bounded and transactional; an interrupted rebuild cannot replace the previous complete result.

Group navigation performs constant-memory reads over validated persisted groups. Rebuilding, when required, uses fresh source-quality RGBA and a sequential decoder, never compressed preview proxies.

## Microscope session model

Phase 5 keeps one authoritative native microscope session per open source operation. A session owns the duplicated source descriptor, complete frame index, bounded presentation/cache state, current position, and presentation generation.

Android receives source-quality RGBA through an explicit prepared-frame handoff with bounded capacity checks. Compose converts that caller-owned RGBA into a bounded display preview. Zoom, pan, scrub, frame stepping, timestamp jumps, and group navigation never redefine the underlying frame identity.

Global native registry locks are released before expensive decoder work; per-session locking protects one session's mutable source/index/navigation state.

## Phase 6 extraction architecture

Phase 6 is layered so selection, decoding, image encoding, manifest semantics, JNI, and Android storage remain separable:

```text
BatchExportRequest / current microscope frame
        ↓
FrameIndex-backed selection planner
        ↓
source-quality sequential decoder
        ↓
one OwnedRgbaFrame at a time
        ↓
PNG / JPEG / lossless WebP encoder
        ↓
FrameOutputSink + JSONL manifest
        ↓
JNI callback
        ↓
Android SAF document/workspace transaction
```

### Selection planning

`framescope-extraction` resolves current frame, inclusive `FrameId` range, inclusive timestamp range, or all frames against a complete authoritative frame index. Optional every-N sampling uses persistent frame identity with checked arithmetic.

Plans expose exact counts and constant-memory iterators. They do not materialize a video-wide selection vector.

### Sequential source-quality batch execution

`framescope-extraction-video` executes a plan with one fresh source-quality decoder across the required span.

The first selected frame may use its persisted safe keyframe anchor. Seek landing is reconciled against the authoritative index before output. Stream-start fallback is permitted only before any visitor/output side effect. Once output begins, timeline divergence is an error rather than a retry that could duplicate files.

Every accepted decoded frame is checked against indexed presentation timestamp, duration, keyframe, corrupt state, and selected stream identity. Intermediate frames required for forward reconciliation are discarded immediately.

### Image encoding

`framescope-extraction-image` encodes one frame at a time:

- PNG preserves RGBA losslessly;
- lossless WebP preserves RGBA;
- JPEG validates quality in `1..=100`, drops alpha, and converts visible pixels to RGB;
- padded source rows are handled correctly and padding bytes are never encoded;
- encoded bytes are streamed to a caller-provided writer rather than accumulated for the batch.

Stable filenames are derived from persistent `FrameId` identity.

### Streaming manifest

Batch exports use an append-only JSON Lines manifest. The header records request/output identity and expected count. Frame records are appended only after the corresponding artifact commits and preserve persistent frame/timing metadata. Cancellation/failure terminals describe partial results when the manifest remains usable. A complete terminal record cannot be written until all expected outputs are committed.

This avoids retaining a video-wide manifest-record collection.

### Android SAF transaction

For a batch export Android creates an isolated `framescope_export_*` SAF workspace and creates the manifest first. Frame documents created while that manifest is active are routed into the same workspace.

The JNI callback owns at most one pending frame document. Rust receives one writable FD, duplicates it, encodes/flushes the frame, then asks Android to commit that document. Uncommitted frame documents are deleted on abort/close.

The manifest is the workspace transaction boundary. The workspace commits only after native success, native response identity validation, coroutine cancellation recheck, and current microscope-session identity recheck. If that final boundary is not reached, closing the uncommitted manifest removes the whole workspace.

Mid-export failures may preserve an auditable partial manifest only when output has materially begun. Preflight failures do not leave an empty committed workspace.

SAF providers do not expose one universal trustworthy predictive free-space API. Phase 6 therefore guarantees transactional storage/provider error handling rather than a speculative capacity estimate. Device/provider low-storage stress belongs to Phase 7.

## Unique/group-representative export

Unique export composes the accepted Phase 4 group store/navigation with the Phase 6 sequential source-quality pipeline.

A constant-memory cursor walks persisted groups in ordinal order. The batch decoder scans once from the first representative through the final representative, verifies intermediate timeline frames, immediately discards non-representatives, and forwards exactly one source-quality RGBA frame per group.

The production policy is explicit: maximum dHash distance `8` and minimum confirmed luma similarity `9700` on the documented `0..=10000` scale. The score is not a literal changed-pixel percentage.

Unique export cannot be combined with every-N sampling because its semantic contract is exactly one representative per validated group.

## Cancellation and lifecycle

Operation-scoped cancellation propagates from ViewModel/repository through JNI to a Rust `CancellationToken`, FFmpeg interrupt callback, decoder loops, grouping, and extraction loops. Stale Android operation generations cannot publish results over newer operations.

Navigation or source replacement cancels active export work before microscope state changes. After a long native export returns, Android rechecks coroutine cancellation and current session identity before committing the SAF workspace.

Native handles remain RAII-owned and seek resets packet/frame/flush/EOF state.

## FFmpeg ownership

The C shim owns a format context, codec context, reusable packet, reusable frame, optional custom AVIO state, and duplicated source descriptor. A deterministic destroy path releases them.

The packet/decode loop retains a compressed packet when `avcodec_send_packet` returns `EAGAIN`, drains decoder output, flushes at demux EOF, drains delayed frames, and reaches stable EOF. Seeking flushes decoder buffers and clears pending packet/frame/EOF state.

The Rust session is `Send` under its explicit unique-ownership contract and is not `Sync`.

## Large-video invariant

FrameScope must remain suitable for multi-gigabyte videos:

1. never read the complete video into RAM;
2. never retain all decoded full-resolution frames;
3. index metadata incrementally in bounded batches;
4. bound RAM cache by bytes;
5. bound disk proxy cache by bytes;
6. seek from persisted keyframe anchors for random access;
7. stream similarity groups rather than collect video-wide group state in RAM;
8. stream extraction selections, decoded frames, encoded output, and manifest records;
9. own at most one pending SAF frame document per batch sink;
10. keep the original source authoritative and all caches/derived similarity state disposable.

CI uses deterministic generated stress fixtures and structural counters rather than committing giant media or relying on flaky absolute hosted-runner RSS thresholds.

## Android / native baseline

- minSdk 26
- compileSdk / targetSdk 36
- current ABI: `arm64-v8a`
- NDK `27.3.13750724`
- Java 17 bytecode
- Android Rust toolchain 1.86.0
- host Rust MSRV 1.85
- FFmpeg 9.0.1, source-built from a pinned official archive

## Phase boundaries

Phases 1 through 6 are accepted foundations: project structure, video decode/timing, persistent frame identity/cache/navigation, visual similarity/grouping, microscope interaction, and source-quality extraction/export.

Phase 7 may harden compatibility, performance, malformed-media behavior, low-storage/provider behavior, accessibility, dependency/security posture, and release automation. It must not replace the accepted frame identity, PTS timing, source-quality extraction, bounded-memory, or transaction/rollback contracts merely to make a device or release test pass.
