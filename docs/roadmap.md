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

**In progress: repository/CI hardening complete; physical-device acceptance pending.**

Completed repository-side work:

- Deterministic malformed/adversarial media coverage, including no-video, garbage, header truncation, and media-payload truncation.
- Low-storage/provider-failure classification and rollback behavior.
- Accessibility and UI-state polish.
- Committed/synchronized Cargo lockfile, reviewed Rust dependency source/license/checksum policy, RustSec advisory scanning, and immutable build-dependency declarations.
- Reproducible arm64 Android debug/release candidate paths and fail-closed signing configuration.
- Semantic-version release workflow and durable GitHub Release packaging with APK, SHA-256 checksum, generated notes, and immutable tag/source linkage.
- Deterministic 100,000-frame large-stream extraction stress plus hosted-runner wall-time/max-RSS evidence without flaky timing thresholds.
- Physical-device/local-provider report schema, CI validator, GitHub-built test kit, and a signed-release lock that requires accepted physical evidence.

Still required before Phase 7 can be marked complete:

- Run the physical arm64 Android/local-SAF provider compatibility matrix from `docs/phase7-device-provider-acceptance.md`.
- Exercise the deterministic codec/navigation/export cases on real hardware.
- Record representative >=1 GiB media timing and peak-PSS evidence.
- Commit `acceptance/phase7/device-report.json` with every required capability/scenario passing.
- Re-run final acceptance/release gates on the evidence commit.

Cloud-only document providers are outside the current contract because both source and destination pickers explicitly use Android `EXTRA_LOCAL_ONLY`.
