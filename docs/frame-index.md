# Phase 3 Persistent Frame Index

FrameScope Phase 3 adds a metadata-only persistent frame index above the Phase 2 streaming decoder.
The index never stores decoded pixels and never derives presentation time from FPS.

## Domain model

`FrameId` is a zero-based sequential display identity assigned while indexing. It is distinct from
presentation time and from Phase 2's decoder-epoch-local `DecodedFrame.index`.

Each `FrameIndexEntry` stores:

- `FrameId`;
- the decoded presentation timestamp as signed ticks plus the exact selected-stream `TimeBase`;
- optional decoded duration in the same exact time base;
- keyframe and corruption flags;
- the nearest safe earlier `KeyframeAnchor`.

Average and nominal frame rates remain informational. CFR and VFR files use exactly the same index
path.

`FrameIndexStreamIdentity` binds an index to the selected video stream by stream index, FFmpeg codec
ID/name, exact time base, and coded dimensions. Rotation is deliberately not part of frame identity;
display rotation does not change the presentation timeline.

## SQLite schema and lifecycle

`framescope-cache` owns SQLite schema version 1. SQLite is built through rusqlite's bundled feature so
the Rust layer does not depend on an Android platform SQLite version.

The database contains two logical tables:

- `index_meta`: one row containing lifecycle, source binding, selected-stream binding, committed row
  count, optional complete frame count, and last recoverable error;
- `frame_index`: compact per-frame metadata, exact PTS/duration fields, keyframe flags, and anchor
  metadata.

The database uses WAL journaling, `synchronous=NORMAL`, transactional batches, and an explicit
completion state. A process death can therefore leave committed batches behind, but those rows remain
`building`/`incomplete`; they are never advertised as a complete index.

Lifecycle values are `building`, `incomplete`, `complete`, and `failed_recoverable`. Before a database
exists the effective state is absent. Source mismatch, unverifiable identity, schema incompatibility,
or corrupt persistent state causes safe recreation because the index is derived data.

## Source identity

`SourceIdentity` is path-independent. It can carry source size, modification metadata,
provider/document identifier, and a content-derived tag. Filename, display name, and URI text are
never identity. Provider IDs and metadata are useful layers but are not sufficient by themselves for
persistent reuse.

For a seekable source, `SourceIdentity::from_seekable` computes a bounded BLAKE3 sample fingerprint
from the beginning, middle, and end, reading at most three 64 KiB windows and restoring the caller's
seek position. A source lacking content-derived evidence is deliberately treated as unverifiable and
its persisted index is rebuilt instead of reused.

A sampled fingerprint is a practical lightweight identity, not a proof that every byte of a
multi-gigabyte file is unchanged. An adversarial replacement that preserves size/metadata and all
sampled windows can evade it. Callers that already possess a stronger content-derived identifier may
supply it as `content_tag`. When the available identity cannot satisfy the caller's trust requirement,
the safe policy is rebuild.

## Keyframe anchors

A clean decoded keyframe anchors itself. Later frames retain the nearest prior clean keyframe. If no
usable keyframe has been observed, the anchor is `StreamStart`.

Anchors are timestamp-oriented. No container packet byte offset is persisted. This avoids assuming
that byte positions are stable or meaningful across custom AVIO, demuxers, or container layouts. A
keyframe can exist without a usable PTS; in that case it remains a display-order fact but is not a
direct timestamp seek target.

## Streaming indexing and bounded memory

`build_or_resume_frame_index` consumes one decoded frame at a time, converts only its metadata into a
`FrameIndexEntry`, and commits bounded batches. The default batch size is 256 rows. Decoded pixel
buffers remain owned/reused by the Phase 2 decoder and are not retained by the index.

Index disk usage therefore scales with frame count, not pixel dimensions or decoded image size.
Metadata indexing and future pixel/proxy caches are intentionally separate storage layers.

## Interruption and resume

Cancellation or decoder failure flushes only a valid pending metadata batch and leaves the database
non-complete. Cancellation records `incomplete`; decoder failure records `failed_recoverable` when the
status update itself can be persisted.

Partial resume is correctness-first. The current implementation opens a fresh decoder from stream
start, re-decodes the already committed prefix, and reconciles every existing row. It then continues
appending new rows. If the fresh timeline disagrees with the stored prefix, or reaches EOF too early,
the partial index is discarded and rebuilt once from a newly opened decoder.

It deliberately does not continue from `last_frame + 1` because Phase 2 frame indexes are local to a
decode epoch and codec state must be restored. A future optimization may resume from a persisted
keyframe anchor, but only if it preserves the same reconciliation guarantees.

## Public API for seek/cache layers

Agents building navigation/cache layers should use:

- `FrameIndex::entry(FrameId)` for exact sequential identity;
- `frame_at_or_before(MediaTimestamp)` / `frame_at_or_after(MediaTimestamp)` for exact-time-base
  lookup;
- `frame_at_or_before_us` / `frame_at_or_after_us` for convenience timestamp lookup;
- `nearest_keyframe_anchor(FrameId)` for a safe decode-start hint;
- `frame_count()` only when completion is known;
- `status()`, `source_identity()`, and `stream_identity()` for validity/progress;
- `visit_range(start, end, visitor)` for bounded-memory range iteration;
- `build_or_resume_frame_index` to stream metadata from a fresh `VideoDecoder` factory.

No Android type appears in these APIs.

## Corruption and migration

Open validates schema version, metadata/source binding, row-count continuity, timestamp/duration time
bases, lifecycle consistency, and keyframe-anchor references. Invalid serialized metadata, impossible
rows, SQLite corruption/not-a-database failures, and unsupported newer schema versions are treated as
rebuildable derived state rather than panics.

Schema changes must advance `FRAME_INDEX_SCHEMA_VERSION` and add an explicit migration or documented
safe-recreation path. Never silently reinterpret rows written by a different schema.

## Deferred work

This phase does not implement decoded-frame RAM caching, thumbnail/proxy disk caching, pHash/SSIM,
duplicate grouping, extraction/export, microscope UI, or Android arbitrary-frame viewing. Those
layers consume the persistent metadata index; they do not belong inside it.
