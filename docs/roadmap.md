# Roadmap

The phases are intentionally ordered. Later work should not be pulled into an earlier phase merely because a stub could be added cheaply.

## Phase 1: Foundation

**Complete.**

- Kotlin/Compose Android app.
- Rust workspace and domain boundaries.
- Real JNI bridge.
- SAF video selection.
- Real Rust-backed metadata inspection for a narrow, documented format scope.
- Tests, docs, CI foundation, privacy model.

## Phase 2: Video engine

**Complete.**

- Source-built FFmpeg integration for Android.
- Decoder abstraction in `framescope-video`.
- Track selection, exact presentation timestamps, keyframes, and VFR semantics.
- Streaming decode without whole-video buffering.
- Cooperative cancellation and deterministic decoder resource lifecycle.
- SAF file-descriptor ownership with native duplication.

## Phase 3: Frame indexing and caching

**Complete pending final integration acceptance merge.**

- Persistent SQLite frame/timestamp index with explicit lifecycle and schema versioning.
- Path-independent source identity with bounded sampled content fingerprinting.
- Persistent global `FrameId` distinct from decoder-local counters and presentation time.
- Exact timestamp lookup policies and nearest safe earlier keyframe anchors.
- Indexed seek, decoder flush, presentation-timeline reconciliation, and decode-forward navigation.
- Rust-owned full-resolution RGBA frame boundary with no escaped `AVFrame` lifetime.
- Byte-bounded RAM hot-frame cache.
- Byte-bounded compressed JPEG/WebP disk proxy cache.
- Typed separation between source-quality RGBA and lossy preview proxies.
- RAM -> disk proxy -> authoritative source fallback for preview navigation.
- Cache invalidation, corruption recovery, atomic proxy writes, and disposable-cache fallback semantics.
- Deterministic Phase 3 fixtures, structural bounded-memory checks, and CI contracts.

## Phase 4: Visual similarity and duplicate grouping

- Perceptual hash/candidate stage.
- SSIM or equivalent verification stage where justified.
- Configurable, explicitly defined similarity thresholds.
- Consecutive duplicate/near-duplicate groups without deleting authoritative timeline entries.
- Representative frames and jump-to-next-meaningful-change primitives.
- Chain-drift-resistant grouping semantics.

## Phase 5: Frame microscope UI

- Exact previous/next-frame navigation.
- Responsive scrub/timeline model backed by the Phase 3 index/cache.
- Timestamp/frame details.
- Jump by frame and timestamp.
- Zoom/pan and rapid bounded stepping.
- Similarity/group navigation backed by Phase 4.

## Phase 6: Frame extraction

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
