# Roadmap

The phases are intentionally ordered. Later work should not be pulled into an earlier phase merely because a stub could be added cheaply.

## Phase 1: Foundation

**Complete.**

## Phase 2: Video engine

**Complete.**

## Phase 3: Frame indexing and caching

**Complete.**

- Persistent frame identity and timestamp contracts are established.
- Indexed navigation and bounded cache behavior are accepted foundations.
- CI contracts verify media correctness and bounded behavior.

## Phase 4: Visual similarity and duplicate grouping

**Complete.**

- Similarity data remains derived from authoritative frame indexes.
- Group navigation never replaces frame identity.
- Similarity stores are rebuildable derived data.

## Phase 5: Frame microscope UI

**Complete.**

Accepted capabilities:

- Exact previous/next-frame navigation.
- Timeline and timestamp navigation.
- Zoom/pan microscope interaction.
- Similarity/group navigation backed by the accepted Phase 4 store and fresh source-quality video adapter.
- Bounded frame presentation and lifecycle handling.
- Stale-result suppression and explicit native session cleanup.
- Android, Rust, FFmpeg fixture, Phase 3, and Phase 4 acceptance gates passing on the final integration path.

Performance acceptance is structural rather than a device-specific latency promise: frame presentation, preview conversion, caches, grouping, and navigation remain explicitly bounded. See `docs/phase5-performance.md`.

## Phase 6: Frame extraction

**Complete.**

Accepted capabilities:

- Current-frame source-quality export.
- Inclusive persistent FrameId-range export.
- Inclusive indexed timestamp-range export.
- All-frame export.
- Exact every-N sampling for ordinary batch selections.
- One source-quality representative per validated similarity group.
- PNG, JPEG, and lossless WebP output.
- Streaming forward decode and bounded per-frame encoding.
- Android SAF destinations with isolated rollback-safe batch workspaces.
- Append-only JSONL manifests.
- Per-frame progress and cooperative cancellation.
- Stale-session/coroutine revalidation before destination commit.
- Transactional storage/provider failure handling without inventing successful artifacts.
- Canonical Rust, FFmpeg, Android, native arm64, APK packaging, Phase 3, and Phase 4 gates passing on the final functional integration path.

See `docs/phase6-acceptance.md` for the full extraction invariants and deferred production-hardening work.

## Phase 7: Production hardening and releases

**Next phase.**

- Physical-device and codec/document-provider compatibility review.
- Performance and peak-memory profiling on representative hardware and large media.
- Malformed-media and adversarial low-storage/provider-failure hardening.
- Dependency/security/license review.
- Accessibility and UI polish.
- Reproducible release build/signing path where credentials are available.
- Semantic-version release workflow.
- Durable GitHub Release with APK, SHA-256 checksum, changelog, and source/tag linkage.
