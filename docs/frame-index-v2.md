# FrameScope Index Architecture V2

## Status and scope

This document defines the persistent-index architecture owned by `agent-06/index-v2`.
It is an additive evolution of the existing authoritative frame index. It does not change decoder
ordering, `FrameId` assignment, timestamp selection, cancellation, extraction quality, or similarity
thresholds.

The implementation in this branch deliberately does **not** claim a physical-device performance
improvement. It establishes versioned persistence and correctness boundaries that navigation,
index-throughput, media-library, and benchmark work can consume.

## Audit findings verified on current `main`

Current production already has important correctness hardening from the 2026-09-09 performance wave:

- reusable source identity is bound to complete-content evidence rather than the older sparse sample;
- the authoritative index persists fail-closed timestamp-seek safety;
- partial-index reconciliation uses bounded ordered SQLite traversal and can use the proven seek-safety
  contract for bounded resume;
- progress/ETA and scrub/cache work from the recent integration wave are present.

The persistent index is nevertheless still primarily an exact decoded-frame metadata table:

- SQLite schema version 1 has `index_meta` and `frame_index`;
- each row is an authoritative display-order `FrameId` with presentation timestamp, duration,
  keyframe/corruption state, and nearest decoded clean-keyframe anchor;
- the indexer still obtains authoritative rows from sequential presentation-frame decode;
- there is no persisted packet/sync-point table, GOP catalog, preview pyramid catalog, or progressive
  per-layer lifecycle;
- the current decoded-frame domain model exposes presentation timestamp and keyframe state, but not
  trustworthy packet ordinal, byte position, packet PTS, or packet DTS.

That last point is a correctness boundary. V2 must not invent packet metadata from nominal FPS,
`FrameId`, decoded PTS, or decoder-local indexes.

## Design goals

V2 turns the index into a layered navigation acceleration structure while preserving the existing
exact index as the authority.

1. A caller can know which layers exist, their generation, lifecycle, coverage, and errors.
2. Structural navigation metadata can exist before a complete exact frame table.
3. Preview/thumbnail/similarity-derived metadata has an explicit source/stream/generation binding.
4. Provisional layers can make the UI useful without being presented as authoritative `FrameId`
   truth.
5. Existing completed indexes are never rewritten merely to adopt V2 metadata.
6. Any V2 corruption, unsupported schema, stale source binding, or incompatible authority contract
   can be recovered by discarding only derived V2 state.

## Layer model

### Level 1: structural / navigation

Persisted by the V2 companion catalog as `navigation_anchor` plus the structural layer status.

An anchor can carry:

- anchor ordinal;
- anchor kind (`decoded_clean_keyframe` or future `packet_sync_point`);
- authoritative `FrameId` when reconciliation has proved one;
- exact presentation timestamp when known;
- GOP end `FrameId` / timestamp when known;
- packet ordinal when a trustworthy demux producer supplies one;
- byte offset when the container/demuxer supplies a trustworthy one;
- packet PTS and packet DTS independently, when known.

Packet fields are intentionally nullable. The current compatibility producer projects decoded clean
keyframes from the existing exact index and leaves packet ordinal, byte offset, packet PTS, and packet
DTS `NULL`.

This matches FFmpeg's own contracts: `AVPacket.pts` and `AVPacket.dts` may be unknown, and packet
`pos` may be `-1` when unavailable. `AVIndexEntry.timestamp` is also demuxer-dependent and is not a
universal substitute for exact presentation-frame identity.

References:

- https://ffmpeg.org/doxygen/trunk/structAVPacket.html
- https://ffmpeg.org/doxygen/trunk/structAVIndexEntry.html
- https://ffmpeg.org/doxygen/trunk/group__lavc__packet.html

A structural anchor is a navigation hint. It becomes `FrameId` authority only where an exact
reconciliation explicitly populated `frame_id`. A timestamp or byte position alone never proves
presentation-frame identity.

### Level 2: visual acceleration

Persisted by `visual_artifact`, the visual layer status, and `similarity_fingerprint_status`.

`visual_artifact` records metadata only:

- thumbnail or scrub-preview kind;
- optional authoritative `FrameId`;
- optional exact stream timestamp;
- pyramid/profile tier;
- producer generation;
- confined relative storage key;
- dimensions and stored byte size.

The companion database does not own image bytes and never substitutes previews for source-quality
extraction. A timestamp-only visual artifact is explicitly provisional and must not be surfaced as an
exact `FrameId` result.

Similarity fingerprint progress is versioned separately so Agent 10 can invalidate/rebuild derived
fingerprints without changing frame identity or the adjacent-grouping correctness contract.

### Level 3: authoritative exact frame index

The existing `FrameIndex` remains the authority for:

- zero-based sequential display `FrameId`;
- exact decoded presentation timestamp in the selected stream time base;
- exact decoded duration when known;
- keyframe/corruption flags;
- safe decoded keyframe anchor;
- completion frame count;
- persisted timestamp-seek safety.

V2 mirrors the exact layer's lifecycle and coverage into its companion status table but does not
reinterpret or duplicate exact frame rows.

Packet PTS/DTS are not added to `FrameIndexEntry` by this branch because the current decoder contract
does not expose a trustworthy mapping. Adding those fields belongs with the producer that can prove
packet-to-presentation-frame association.

## Persistence layout

The exact index remains the existing SQLite database.

`ProgressiveFrameIndex::companion_path()` creates a sibling file by appending `.v2.sqlite3` to the
exact-index path. Both files therefore remain inside the existing per-source FrameScope index
namespace and are included in existing storage accounting/clear operations.

The companion database uses WAL, `synchronous=NORMAL`, a busy timeout, and transactional writes. Its
schema generation is independent from the authoritative exact-index schema generation.

Tables:

- `progressive_meta`
  - strong source identity JSON and stable key;
  - selected-stream identity JSON;
  - authoritative exact-index schema version;
  - exact timeline-contract generation.
- `layer_status`
  - one persisted row for structural, visual, and exact layers;
  - lifecycle, generation, completed/total units, optional contiguous exact-prefix coverage, exact
    timeline coverage, last recoverable error.
- `navigation_anchor`
  - structural/GOP/sync metadata with optional trustworthy packet fields.
- `visual_artifact`
  - thumbnail/scrub-preview location and profile metadata.
- `similarity_fingerprint_status`
  - versioned fingerprint lifecycle and authoritative-prefix coverage.

## Lifecycle and progressive truth

Every layer persists one of:

- `not_started`;
- `building`;
- `incomplete`;
- `complete`;
- `failed_recoverable`.

`complete` requires `completed_units == total_units`. Unknown totals remain `NULL`; they are not
fabricated from duration multiplied by nominal/average FPS.

The exact layer mirrors the authoritative `FrameIndexLifecycle`. Structural metadata can be useful
while the exact layer is still building or incomplete, but UI and navigation code must label/use that
state as provisional. Level 1 or Level 2 completion is not equivalent to Level 3 completion.

## Compatibility and migration strategy

### Why V2 is a companion instead of an in-place V1 migration

The current exact database is already production authority and recent fixes strengthened its source
identity and timeline contract. Rewriting that database solely to add optional acceleration metadata
would enlarge the corruption and rollback surface without improving exact-frame correctness.

Therefore V2 migration is additive:

1. open the existing exact `FrameIndex` normally;
2. open/create the companion using the exact index's already-validated source and stream binding;
3. optionally bootstrap structural/exact status from the committed exact prefix;
4. leave all existing exact rows untouched.

Existing completed V1 indexes remain readable even if V2 creation fails.

If the companion has an unsupported schema, malformed state, stale source identity, a different
stream, or an incompatible exact-index/timeline generation, FrameScope may delete and recreate only
the companion because every row in it is derived data. The exact database is not deleted by that
recovery path.

This is the backward-compatibility guarantee for the first V2 deployment.

## Crash safety and resumability

Companion schema creation and structural snapshot replacement are explicit transactions. A crash
cannot turn a partially replaced structural snapshot into a committed `complete` layer.

Layer lifecycle and coverage are persistent. Producers can resume/rebuild their own layer from the
last committed status without changing exact index authority.

The current compatibility bootstrap deliberately rebuilds the small structural projection from the
committed exact rows. It is a migration/stage-boundary operation, not a per-frame/per-batch hot-path
API. Agent 7 should populate Level 1 incrementally from a future demux-first structural pass rather
than repeatedly rescanning the exact table during indexing.

## Source identity and stale-index detection

The companion copies and verifies the same `SourceIdentity` and `FrameIndexStreamIdentity` already
validated by the authoritative index, plus the authoritative exact-index schema version and timeline
contract generation.

A source that is not safe for persistent exact-index reuse is not allowed to make a reusable V2
companion authoritative. Reopen resets derived companion state instead of trusting it.

Changing source bytes, selected stream, exact schema, or timeline semantics therefore invalidates V2
without weakening the existing fail-closed exact-index behavior.

## API safety rules for consumers

- `ProgressiveIndexLayer::StructuralNavigation` and `VisualAcceleration` are never exact-frame
  authority.
- A structural timestamp/packet/byte anchor is a decode-start hint, not proof of `FrameId` alignment.
- Agent 2 navigation must continue to honor `FrameIndex::timestamp_seek_safety()` and exact
  reconciliation before publishing an authoritative frame.
- A preview with only a timestamp can be displayed as provisional scrub imagery, but release/settle
  must resolve through the authoritative exact index.
- Source-quality extraction continues to use authoritative source decode.
- No consumer may derive a frame number from timestamp × FPS.

## Current implementation boundary

This branch supplies the persistence contract and a correctness-safe compatibility bootstrap. It does
not yet replace the authoritative sequential indexing loop with a demux-first progressive producer.
That is intentional because current `DecodedFrame` does not expose the packet identity needed to
populate Level 1 packet fields faithfully.

The schema is ready for a producer to add trustworthy packet anchors later without a semantic
reinterpretation of existing nullable fields.

## Dependencies and handoff

### Agent 2: decoder/navigation

Consume Level 1 anchors only as navigation hints. Timestamp-seek shortcuts must remain gated by
`TimestampSeekSafety`; packet/byte seeking additionally needs proof that the demuxer/source supports
the persisted anchor semantics. Exact release/settle still reconciles to `FrameId`.

### Agent 7: index throughput

Own the producer-side evolution from decoded-frame-only indexing toward a progressive structural
pass. Populate packet ordinal/position/PTS/DTS only from trustworthy demux metadata, persist Level 1
status incrementally, and keep Level 3 generation exactly presentation-order/VFR-safe.

### Agent 8: media library

The library may use persisted layer/lifecycle metadata to show that an indexed source exists across
restart. It must distinguish structural/visual availability from exact-index completion and must not
use recent-history presence as the source of truth for index existence.

### Agent 12: integration benchmarks

Measure, rather than assume:

- companion open/bootstrap overhead;
- time until Level 1 becomes useful once Agent 7 supplies a real structural producer;
- cold versus warm exact seek;
- adjacent step locality;
- scrub-preview latency;
- source/provider behavior and storage growth.

Physical-device acceptance remains outstanding for performance claims.

## Tests in this branch

Focused Rust tests cover:

- bootstrapping a completed exact index into V2 without changing exact rows;
- partial/building lifecycle remaining visibly non-complete;
- VFR-safe signed/repeated presentation timestamps in the inherited exact index;
- stale source rebinding clearing only derived companion metadata;
- unsupported companion schema recreation preserving the completed exact index;
- confined visual storage keys;
- versioned similarity fingerprint progress;
- packet fields remaining absent when the current producer cannot prove them.
