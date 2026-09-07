# Roadmap

The phases are intentionally ordered. Later work should not be pulled into an earlier phase merely because a stub could be added cheaply.

## Phase 1: Foundation

- Kotlin/Compose Android app.
- Rust workspace and domain boundaries.
- Real JNI bridge.
- SAF video selection.
- Real Rust-backed metadata inspection for a narrow, documented format scope.
- Tests, docs, CI foundation, privacy model.

## Phase 2: Video engine

- Introduce the maintained FFmpeg build/integration strategy for Android.
- Decoder abstraction in `framescope-video`.
- Track selection, timestamps, keyframes, variable-frame-rate semantics.
- Exact single-frame decode proof.
- Cancellation and decoder resource lifecycle.

## Phase 3: Frame indexing and caching

- Persistent frame/timestamp index.
- Keyframe-aware seek hints.
- RAM hot cache.
- Compressed disk cache.
- Source identity, invalidation, disk schema/versioning, cache limits.
- Large-video performance tests.

## Phase 4: Visual similarity and duplicate grouping

- Perceptual hash/candidate stage.
- SSIM or equivalent verification stage.
- Configurable similarity thresholds.
- Consecutive duplicate/near-duplicate groups.
- Jump-to-next-meaningful-change primitive.

## Phase 5: Frame microscope UI

- Exact previous/next-frame navigation.
- Responsive scrub/timeline model backed by the index/cache.
- Timestamp/frame details.
- Range selection.
- Comparison and duplicate-group navigation.

## Phase 6: Frame extraction

- Single-frame export.
- Selected-range export.
- All-frame export.
- Unique/group-representative export.
- Streaming output, cancellation, progress, storage-space/error handling.

## Phase 7: Production hardening and releases

- Device/codec compatibility matrix.
- Performance and memory profiling.
- Fuzzing/hardening of untrusted container paths where appropriate.
- Accessibility and UI polish.
- Reproducible signing/release process.
- Public release automation and distribution.
