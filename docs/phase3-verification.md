# Phase 3 verification contracts

This document describes the Phase 3 CI, fixture, stress, and benchmark contracts. It intentionally does not define or implement the production frame index, seek engine, RAM cache, or disk proxy cache.

## Goals

Phase 3 verification must catch implementations that appear fast while violating presentation-time correctness, source invalidation, cache semantics, or bounded-memory behavior.

The canonical rules are:

- presentation timestamps come from decoded media timestamps, never `frame / fps`;
- keyframe anchors point to a real keyframe at or before the target frame;
- VFR and B-frame reorder cases are first-class fixtures;
- stress media is generated in CI rather than committed;
- memory checks use lifecycle/instrumentation bounds, not a fragile hosted-runner RSS number;
- performance values are reported, while hard timing gates are deferred until stable baselines exist.

## Generated fixture set

`scripts/generate-phase3-video-fixtures.sh` creates four files under `build/phase3-video-fixtures`.

| Fixture | Contract |
| --- | --- |
| `h264-dense-keyframes.mp4` | 24 frames, keyframes at presentation-frame indices 0, 3, 6, 9, 12, 15, 18, 21 |
| `h264-long-gop-bframes.mp4` | 36 frames, keyframes at 0 and 24, B-frame reordering enabled |
| `h264-vfr-transitions.mp4` | 11 frames across 4 fps, 12 fps, then 6 fps timestamp regimes |
| `h264-stress-long.mp4` | 24 fps, 45 seconds by default, 1,080 frames, low resolution/bitrate |

The stress duration is controlled by `PHASE3_STRESS_SECONDS`. The media remain build artifacts and are not committed.

The existing Phase 2 fixture generator remains unchanged and continues to cover codecs, rotation metadata, audio/video combinations, multiple video streams, unusual dimensions, one-frame media, and truncation.

## Independent frame truth

`scripts/phase3-frame-truth.py` uses an external `ffprobe` process as the fixture oracle. For each deterministic fixture it records:

- selected video stream index;
- exact stream time base numerator/denominator;
- frame count;
- presentation timestamp ticks;
- integer microsecond conversion;
- duration ticks when available;
- keyframe state;
- picture type;
- nearest earlier keyframe frame index;
- exact/between/before-first/after-last lookup cases.

The generated truth lives at `build/phase3-frame-truth.json` in CI. Production index tests should compare their persisted rows against this oracle rather than merely asserting that indexing completed.

### VFR proof

The VFR fixture must contain more than one positive presentation-timestamp delta. Tests must compare the actual PTS sequence from the persisted index with the oracle sequence. Average or nominal FPS is not sufficient evidence.

## Timestamp lookup semantics

`framescope-test-support::TimestampSelection` and `fixtures/video/phase3-manifest.json` define the shared contract.

- `AtOrBefore`: greatest presentation timestamp `<= target`; before the first frame returns no frame. With repeated equal timestamps, choose the last equal frame.
- `AtOrAfter`: smallest presentation timestamp `>= target`; after the final frame returns no frame. With repeated equal timestamps, choose the first equal frame.
- `Nearest`: minimum presentation-time distance; equal-distance ties choose the earlier timestamp. An exact repeated timestamp chooses its first frame.
- exact sequential frame lookup is zero-based and does not clamp an out-of-range frame index.

This contract applies to tests even if the production API exposes separate methods rather than one policy enum.

## Keyframe anchors

Every indexed frame must resolve to a keyframe anchor at or before the frame. The dense-keyframe fixture catches off-by-one anchor updates. The long-GOP/B-frame fixture catches implementations that confuse packet/decode order with presentation order.

Byte offsets are not part of this verification contract. Timestamp/keyframe anchors are the portable default.

## Bounded-memory and persistence instrumentation

`framescope-test-support::StreamingCounters` is a test-facing observation shape:

- `decoded_frames`;
- `max_live_decoded_frames`;
- `persisted_batches`;
- `peak_buffered_metadata_entries`.

`verify_bounded_streaming` compares those observations with the implementation's configured live-frame and metadata-batch limits. This is deliberate: a hosted-runner RSS ceiling would be noisy and would not prove that frame retention is independent of video duration.

For the default 1,080-frame stress fixture, integration tests should require more than one persistence batch. A production test adapter should fail if either peak live frames or buffered metadata exceeds its configured bound, regardless of total decoded frames.

## Cache/access instrumentation

`framescope-test-support::AccessCounters` supports deltas for:

- decoded frames produced;
- cache hits;
- cache misses;
- disk proxy reads;
- evictions;
- source invalidations.

The helpers encode these reusable contracts:

- cold access: a miss followed by decode fallback or disk-proxy read;
- warm access: at least one cache hit and zero redundant decoded frames;
- evicted access: miss followed by fallback;
- stale source: explicit invalidation and zero stale cache hits.

Agents should expose these counters through test doubles or scoped instrumentation. Do not add process-global test switches to the production API.

## Persistence/recovery integration cases

Agent 1's production index tests should plug into the generated truth and stress contracts for:

- interrupted index followed by reopen;
- incomplete database never reported complete;
- source identity mismatch with the same display name;
- schema migration;
- corrupted database recreation/recovery;
- write failure/low-storage simulation where practical;
- exact frame, timestamp, and keyframe-anchor lookup;
- stable EOF and cancellation;
- incremental persistence on the stress fixture.

Correct recovery is more important than resuming from the last numeric frame. A resume test must restore decoder state from a safe earlier anchor and reconcile already persisted entries, or rebuild safely.

## Seek/cache integration cases

Agents 2 and 3 should use the same truth/counters for:

- exact indexed frame access;
- forward and backward seek;
- repeated same-frame access;
- distant jumps;
- targets around keyframes;
- VFR targets;
- targets between presentation timestamps;
- access after EOF and cancellation;
- warm cache hit without redundant decode;
- eviction followed by correct fallback;
- stale-source invalidation.

Failure output should name the fixture, requested frame/timestamp, expected and actual anchor, plus relevant access-counter deltas.

## Benchmark report contract

`scripts/phase3-benchmark-report.py` validates and prints a JSON metrics payload. Timing values are reported but not hard-gated. Structural invariants are gated.

Required measurement names are:

- `frame_number_lookup`;
- `timestamp_lookup`;
- `random_seek_setup`;
- `cold_access`;
- `warm_access`.

The `index` section records frame count, elapsed nanoseconds, database bytes, persisted batches, peak live decoded frames, configured live-frame limit, peak buffered metadata entries, and configured metadata-batch limit.

The reporter prints throughput, bytes per frame, and nanoseconds per operation. It rejects invalid structural metrics such as a one-batch long index or a measured peak above the implementation's configured bound. Once Phase 3 implementation branches expose real measurements, CI should feed their metrics JSON to this reporter rather than inventing wall-clock pass/fail thresholds.

## CI

`.github/workflows/phase3.yml` adds the `phase3-index-cache` job. It:

1. installs FFmpeg;
2. validates the Rust support crate;
3. syntax/self-tests the Python harnesses;
4. generates deterministic Phase 3 fixtures;
5. captures exact frame truth;
6. logs oracle timing and stress media statistics;
7. enforces a small generated-media/truth size budget;
8. uploads the compact truth JSON only on failure.

The existing Phase 2 workflow remains unchanged. Existing decoder, Android/JNI, FFmpeg provenance, and corrupted-media tests must continue to pass.

## Integration ownership

This branch provides verification machinery only.

- Agent 1 owns the persistent index, source identity, migrations, interruption/recovery, and production index instrumentation points.
- Agent 2 owns indexed navigation/seek behavior and seek instrumentation.
- Agent 3 owns cache/proxy implementation and cache instrumentation.
- Agent 4 owns these shared oracles, generated fixtures, structural counter contracts, benchmark reporting, and CI gates.

No raw frames, thumbnails, proxies, or production cache implementation belong in this verification layer.
