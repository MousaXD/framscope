# Phase 5 Acceptance

**Status: Complete.**

## Scope

Phase 5 integrates the accepted indexing, navigation, similarity, and cache contracts into the frame microscope experience without replacing the Phase 2/3/4 correctness model.

## Acceptance requirements

- Previous/next frame navigation uses authoritative persistent frame identity.
- Timeline and timestamp operations use presentation timestamps, never FPS-derived estimates.
- Similarity grouping remains a navigation aid and never removes source timeline entries.
- Group rebuilds consume fresh source-quality RGBA frames and validate selected-stream identity against the authoritative frame index.
- UI operations remain bounded by the Phase 3 cache and Phase 5 presentation limits.
- Large videos do not require loading the full source or all group metadata into memory.
- Late asynchronous results cannot replace newer microscope/session state.
- Native sessions and Android-owned frame buffers have explicit ownership and cleanup.
- CI remains the canonical verification environment for heavy builds and media tests.

## Verification gates

The Phase 5 completion path requires all of the following on the current integration head:

- Rust format.
- Rust clippy with warnings denied.
- Rust workspace tests.
- Real FFmpeg/video fixture integration tests.
- Native Android arm64 build verification.
- Android unit tests and lint.
- Android debug APK build and native packaging verification.
- Phase 3 index/cache contracts.
- Phase 4 similarity/grouping contracts, including the bounded long-stream grouping check.

The final group-navigation video adapter was merged only after these canonical checks were green. The Phase 5 acceptance documentation is merged only after the same regression gates pass against the updated base.

## Evidence map

- Exact frame/timestamp navigation: `framescope-video` microscope/navigation modules plus `MicroscopeTimelineMathTest.kt` and `MicroscopeSwipeMathTest.kt`.
- Source-quality frame handoff and ownership: `MicroscopeFrameBridge.kt`, `MicroscopeFrameHandoffTest.kt`, and the Rust presentation handoff tests.
- Stale-result/lifecycle safety: `MicroscopeSessionControllerTest.kt`, `MicroscopeViewModelSourceReplacementTest.kt`, and related ViewModel cleanup tests.
- Bounded preview and zoom/pan behavior: `MicroscopePreviewMathTest.kt` and `MicroscopeTransformMathTest.kt`.
- Similarity/group navigation: `framescope-group-navigation` and `framescope-group-navigation-video`, with Phase 4 persistence/grouping contracts remaining authoritative.

No acceptance step may weaken an existing correctness invariant or remove a regression test merely to make CI pass.
