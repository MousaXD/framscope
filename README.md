# FrameScope

FrameScope is an open-source Android application for precise, local-first video-frame inspection. Videos stay on the device. The normal app path has no backend, telemetry, analytics, ads, accounts, or network requirement.

The repository has completed the Phase 2 video engine, Phase 3 frame-index/navigation/cache stack, and Phase 4 similarity/grouping foundation. Phase 5 microscope integration is in final acceptance.

## Phase status

- Phase 1: Foundation. **Complete.**
- Phase 2: Video engine. **Complete.**
- Phase 3: Frame indexing and caching. **Complete.**
- Phase 4: Visual similarity and duplicate grouping. **Complete.**
- Phase 5: Frame microscope UI and integration. **Final acceptance in progress.**
- Phase 6: Frame extraction. **Next.**

## Acceptance policy

Phase completion requires correctness validation through GitHub Actions. Heavy Android, FFmpeg, Rust, and media fixture checks run in CI rather than depending on a developer workstation.

Existing architecture contracts remain fixed:

- FrameId is the authoritative frame identity.
- Presentation timestamps remain the media clock.
- FPS is never used to reconstruct frame timing.
- Similarity groups are derived data and never replace the source frame index.
- Memory usage must remain bounded for large videos.

## Architecture

```text
Jetpack Compose UI
        ↓
MainViewModel
        ↓
FrameScopeRepository
        ↓
NativeBridge / JNI
        ↓
framescope-ffi
        ↓
framescope-video ───────── framescope-cache
        ↓                         ↑
framescope-ffmpeg                 │
        ↓                 index + bounded caches
FFmpeg
```

Android owns UI/lifecycle/SAF and the original descriptor. Rust owns timing semantics, stream selection, persistent frame identity, index/navigation/cache policy, and FFmpeg resources.

See `docs/architecture.md`, `docs/video-engine.md`, `docs/frame-index.md`, and `docs/roadmap.md`.

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
- random access uses indexed seeking.

## GitHub Actions

GitHub Actions is the canonical heavy verifier for Rust, Android, FFmpeg, and media fixture validation.

See `docs/phase5-acceptance.md` and `docs/phase5-performance.md`.

## License

FrameScope is licensed under **GPL-3.0-only**. See `LICENSE`.
