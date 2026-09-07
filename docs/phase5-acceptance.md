# Phase 5 Acceptance

## Scope

Phase 5 integrates the accepted indexing, navigation, similarity, and cache contracts into the frame microscope experience.

## Acceptance requirements

- Previous/next frame navigation uses authoritative frame identity.
- Timeline operations use presentation timestamps, not FPS-derived estimates.
- Similarity grouping remains a navigation aid and never removes source timeline entries.
- UI operations remain bounded by Phase 3 cache limits.
- Large videos must not require loading the full source into memory.
- CI remains the canonical verification environment for heavy builds and media tests.

## Verification

Required checks:

- Rust format and clippy.
- Workspace tests.
- Android unit tests and lint.
- Native Android verification.
- Phase 3 and Phase 4 contract tests.

No acceptance step may weaken existing correctness invariants.