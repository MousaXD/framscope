# FrameScope

FrameScope is an open-source Android application for precise, local-first video-frame inspection and source-quality frame extraction. Videos stay on the device. The normal app path has no backend, telemetry, analytics, ads, accounts, or network requirement.

The repository has completed the Phase 2 video engine, Phase 3 frame-index/navigation/cache stack, Phase 4 similarity/grouping foundation, Phase 5 frame microscope UI/integration, and Phase 6 frame extraction/export pipeline. Phase 7 production hardening and releases is next.

## Phase status

- Phase 1: Foundation. **Complete.**
- Phase 2: Video engine. **Complete.**
- Phase 3: Frame indexing and caching. **Complete.**
- Phase 4: Visual similarity and duplicate grouping. **Complete.**
- Phase 5: Frame microscope UI and integration. **Complete.**
- Phase 6: Frame extraction and export. **Complete.**
- Phase 7: Production hardening and releases. **Next.**

## Extraction/export

Phase 6 supports:

- current-frame export;
- inclusive frame and timestamp ranges;
- all-frame export;
- exact every-N sampling;
- one representative per validated similarity group;
- PNG, JPEG, and lossless WebP;
- Android Storage Access Framework destinations;
- streaming JSONL manifests, progress, cancellation, and rollback-safe partial-failure handling.

Exports use authoritative persistent `FrameId`/PTS metadata and source-quality RGBA. Lossy navigation proxies are never accepted as extraction source pixels.

## Acceptance policy

Phase completion requires correctness validation through GitHub Actions. Heavy Android, FFmpeg, Rust, and media fixture checks run in CI rather than depending on a developer workstation.

Existing architecture contracts remain fixed:

- FrameId is the authoritative frame identity.
- Presentation timestamps remain the media clock.
- FPS is never used to reconstruct frame timing.
- Similarity groups are derived data and never replace the source frame index.
- Source-quality extraction never consumes lossy disk proxies.
- Memory usage must remain bounded for large videos.

## Architecture

```text
Jetpack Compose UI
        ↓
MainViewModel / export ViewModels
        ↓
FrameScopeRepository
        ↓
JNI + SAF output callbacks
        ↓
framescope-ffi
        ↓
video/index/similarity/extraction crates
        ↓
FFmpeg + bounded caches + streaming encoders/manifests
```

Android owns UI/lifecycle/SAF and the original source/output descriptors. Rust owns timing semantics, stream selection, persistent frame identity, index/navigation/cache policy, similarity validation, source-quality extraction orchestration, encoding, manifests, and FFmpeg resources.

See `docs/architecture.md`, `docs/video-engine.md`, `docs/frame-index.md`, `docs/phase6-acceptance.md`, and `docs/roadmap.md`.

## Timing model

Presentation timestamps are the media clock.

Each decoded frame can carry:

- FFmpeg best effort timestamp with PTS fallback;
- exact selected-stream rational time base;
- checked integer microsecond conversion;
- optional frame duration;
- decoder-local index scoped to a decode epoch.

Persistent FrameIds are separate from timestamps and decoder-local counters.

## Large-video invariant

FrameScope is designed so video duration does not imply unbounded memory usage:

- the source is streamed rather than loaded fully;
- indexes persist bounded metadata;
- full-resolution cache is byte bounded;
- disk proxies are disposable and bounded;
- random access uses indexed seeking;
- extraction selections and similarity representatives are streamed rather than collected video-wide;
- batch output owns one selected frame/document at a time and writes manifests incrementally.

## GitHub Actions

GitHub Actions is the canonical heavy verifier for Rust, Android, FFmpeg, media fixtures, native JNI exports, APK packaging, and the Phase 3/4 regression contracts.

See `docs/phase5-acceptance.md`, `docs/phase5-performance.md`, and `docs/phase6-acceptance.md`.

## License

FrameScope is licensed under **GPL-3.0-only**. See `LICENSE`.
