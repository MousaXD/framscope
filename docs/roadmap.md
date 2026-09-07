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

**Implementation complete, final acceptance in progress.**

Completed capabilities:

- Exact previous/next-frame navigation.
- Timeline and timestamp navigation.
- Zoom/pan microscope interaction.
- Similarity/group navigation.
- Bounded frame presentation and lifecycle handling.

Remaining acceptance:

- PR #47 integration verification.
- Final GitHub Actions validation.
- Performance evidence collection.

## Phase 6: Frame extraction

**Next phase.**

- Current-frame export.
- Selected frame/time-range export.
- All-frame export.
- Interval sampling.
- Unique/group-representative export.
- PNG/JPEG/WebP output.
- Streaming output, cancellation, progress, manifest, and storage-space/error handling.

## Phase 7: Production hardening and releases

- Device/codec compatibility review.
- Performance and memory profiling.
- Malformed-media and low-storage hardening.
- Dependency/security/license review.
- Accessibility and UI polish.
- Reproducible release build/signing path where credentials are available.
- Semantic-version release workflow.
- Durable GitHub Release with APK, SHA-256 checksum, changelog, and source/tag linkage.
