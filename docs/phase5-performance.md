# Phase 5 Performance Contract

**Status: Accepted.**

Phase 5 performance acceptance is correctness-first and based on explicit boundedness contracts. It does not claim a universal frame-latency number across Android devices, codecs, resolutions, or storage providers; device-specific profiling remains Phase 7 work.

## Boundedness rules and evidence

### Preview allocation

`MicroscopePreviewMath.kt` caps the Compose preview to a 1,920 x 1,080-equivalent pixel budget (`2,073,600` pixels). Larger authoritative source frames are sampled directly into that bounded preview plan instead of creating another full-resolution staging bitmap.

`MicroscopePreviewMathTest.kt` verifies the preview sizing and coordinate mapping behavior.

### Source-quality frame handoff

`MicroscopeFrameBridge.kt` rejects prepared RGBA payloads above 256 MiB before Android direct-buffer allocation. The JNI handoff copies into caller-owned memory and requires the exact expected byte count.

`MicroscopeFrameHandoffTest.kt` and the Rust presentation handoff tests cover descriptor validation, direct-buffer ownership, stale generations, and copy-length correctness.

### Navigation request pressure

Swipe navigation resolves one direction decision per completed gesture and cannot encode an unbounded step count. Timeline drag state is kept on the UI side and commits through the authoritative indexed navigation path rather than decoding every intermediate slider position.

`MicroscopeSwipeMathTest.kt` and `MicroscopeTimelineMathTest.kt` cover thresholds, boundaries, very large frame counts, and malformed/extreme input.

### Cache and source behavior

Phase 5 inherits the accepted Phase 3 byte-bounded RAM and disk cache contracts. Source-quality requests cannot be satisfied by lossy disk proxies, and weak/unverifiable source identities skip reusable cache state rather than risking aliasing.

### Similarity/group navigation

Similarity rebuilds stream fresh source-quality RGBA frames through the accepted grouping pipeline rather than accumulating a video-wide pixel set. Persisted groups use the bounded Phase 4 SQLite store, and navigation locates group targets without loading all group metadata into a process-wide vector.

The Phase 4 workflow exercises the bounded long-stream grouping contract. `framescope-group-navigation-video` preserves stream identity, timing metadata, cancellation, and sequential-frame reconciliation when source decode is required.

### Concurrency and lifecycle

Microscope session work uses explicit session ownership and stale-result suppression. Expensive native work is kept outside global registry locks, and replacement/close operations invalidate obsolete asynchronous results instead of allowing work to accumulate into authoritative UI state.

## CI acceptance

Phase 5 completion requires the canonical GitHub Actions regression set to pass:

- Rust format, clippy, and workspace tests.
- Real FFmpeg/video fixture integration.
- Native Android arm64 verification.
- Android unit tests, lint, debug APK build, and native packaging verification.
- Phase 3 index/cache contracts.
- Phase 4 similarity/grouping and bounded long-stream contracts.

These structural limits are the Phase 5 performance evidence. Wall-clock latency, device matrices, thermal behavior, and memory profiling under representative production workloads are intentionally reserved for Phase 7 and must not replace correctness gates.
