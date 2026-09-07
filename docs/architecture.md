# FrameScope Architecture

## Current architecture through Phase 3

```text
Android Compose UI
        ↓
MainViewModel
        ↓
FrameScopeRepository
        ↓
NativeBridge / JNI
        ↓
framescope-ffi
        ↓
framescope-video ─────────────── framescope-cache
        ↓                               ↑
framescope-ffmpeg                       │
        ↓                               │
FFmpeg                          frame index + caches
```

Dependencies remain directional. Android owns platform UI, lifecycle, Storage Access Framework access, cache-directory selection, and the original file descriptor. Rust owns media timing, stream selection, persistent frame identity, indexing, navigation semantics, cache policy, and FFmpeg resource ownership. Core Rust crates do not depend on Android APIs.

FrameScope is local-first. Normal application behavior requires no backend and the Android manifest has no broad storage or network permission.

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

## Timestamp and frame identity model

Presentation timestamps are authoritative. The decoder uses FFmpeg `best_effort_timestamp`, with frame PTS fallback, interpreted in the selected stream's exact rational time base.

FrameScope deliberately has two different identities:

- the Phase 2 decoder-local sequential index, scoped to a decode epoch;
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

A persistent index or cache entry is never keyed only by filename, display name, filesystem path, or URI text.

`SourceIdentity` may include size, modification metadata, provider/document identity, and content-derived evidence. Reusable persistent state requires content-derived evidence plus source size.

For seekable sources, the built-in identity sampler reads at most three 64 KiB windows from the beginning, middle, and end, hashes them with BLAKE3, and restores the caller's seek position. This avoids hashing a multi-gigabyte source in full before work can begin.

The sampled fingerprint is deliberately documented as bounded evidence rather than mathematical proof that every unsampled byte is unchanged. Callers may provide a stronger content-derived tag. When reusable identity cannot be established, FrameScope rebuilds derived state instead of risking stale reuse.

## Index build and recovery

`framescope-video::build_or_resume_frame_index` streams Phase 2 decoded metadata into the persistent index.

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

Phase 3 adds two bounded pixel tiers:

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

The disk tier stores compressed JPEG/WebP navigation proxies in a versioned source/stream/frame namespace. Proxy payloads are typed separately from source-quality RGBA so extraction or other original-quality paths cannot accidentally consume a lossy preview.

Writes use a temporary file in the destination directory, flush it, and rename it into place. Cache scans ignore symlinks and temporary files. Corrupt/oversized entries are rejected and removed. A global byte budget drives eviction.

Disk proxy storage is disposable. Read/write failure must not make the source frame unavailable. Preview navigation degrades to authoritative indexed decode and can return a non-fatal cache diagnostic instead of failing the media request.

## Cancellation and lifecycle

Operation-scoped cancellation propagates from ViewModel/repository through JNI to a Rust `CancellationToken`, FFmpeg interrupt callback, and decoder loop. Stale Android operation generations cannot publish results over newer operations.

Phase 3 index/navigation work reuses the same cancellation/error model. Native handles remain RAII-owned and seek resets packet/frame/flush/EOF state.

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
7. keep the original source authoritative and all caches disposable.

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

Phase 3 provides truthful indexed navigation and bounded caching. It deliberately does not implement visual-similarity grouping, the frame-microscope Compose UI, extraction/export, hardware MediaCodec acceleration, or public release automation.

Those are Phase 4 through Phase 7 work and must consume the authoritative Phase 2/3 timing, identity, navigation, and source-quality boundaries rather than replacing them.
