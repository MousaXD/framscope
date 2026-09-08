# Phase 6 Acceptance

**Status: Complete after the final acceptance branch passes the canonical exact-head gates.**

## Scope

Phase 6 adds source-quality frame extraction and export on top of the accepted Phase 2 video engine, Phase 3 frame index/cache contracts, Phase 4 similarity groups, and Phase 5 microscope session model.

The phase does not redefine frame identity, timing, similarity semantics, or Android source ownership. Export remains a consumer of those accepted layers.

## Accepted capabilities

- Export the current authoritative microscope frame.
- Export an inclusive persistent `FrameId` range.
- Export an inclusive indexed presentation-timestamp range.
- Export all indexed frames.
- Apply exact every-N-frame sampling to ordinary batch selections.
- Export exactly one representative frame per validated Phase 4 similarity group.
- Encode PNG, JPEG, or lossless WebP.
- Write through Android Storage Access Framework destinations without filesystem-path conversion or broad storage permission.
- Report per-frame progress and support cooperative cancellation.
- Produce append-only JSON Lines manifests for batch and unique-group exports.
- Roll back uncommitted SAF documents/workspaces and retain auditable partial manifests only after export output has materially begun.
- Surface storage/provider/encoding/decode failures without inventing successful artifacts.

## Correctness invariants

### Frame identity and timing

- Persistent Phase 3 `FrameId` is the authoritative export identity.
- Timestamp selections resolve against indexed presentation timestamps.
- FPS is informational only and is never used to reconstruct frame timing.
- Extraction requires a complete authoritative frame index.
- Fresh decoders are validated against the indexed stream identity.
- Decoded PTS, duration, keyframe, and corrupt metadata are reconciled against the authoritative index before output is accepted.

### Source-quality pixels

- Export consumes fresh/source-quality RGBA frames or the accepted microscope source-quality handoff.
- Lossy Phase 3 disk proxies can never satisfy current-frame, batch, or unique-group extraction.
- The FFmpeg reusable frame never escapes its decoder lifetime.

### Bounded execution

- Selection planning uses constant-memory iterators rather than a video-wide selected-frame vector.
- Ordinary batch export uses one forward decoder for the resolved span instead of one random seek per selected frame.
- Intermediate frames needed for timeline reconciliation are discarded immediately.
- The image encoder owns at most one frame's bounded format-specific scratch storage.
- Android's batch SAF sink owns at most one pending frame document at a time.
- JSONL manifests are written incrementally and do not accumulate all frame records in memory.
- Unique-group export streams persisted group representatives without materializing the representative set or performing one random seek per group.

### Seek and retry safety

- A persisted safe keyframe anchor may be used for the first selected frame.
- Stream-start fallback is allowed only before any visitor/output side effect.
- After output begins, timeline divergence is an error rather than a retry that could duplicate exported files.

### Manifest and destination transactions

- Stable output names are derived from persistent `FrameId` identity.
- A manifest frame record is appended only after the corresponding output artifact is committed.
- Complete terminal records are impossible until the expected number of outputs has committed.
- Cancellation and recoverable mid-export failures produce explicit partial terminal state when the manifest remains writable.
- Each Android batch uses an isolated SAF workspace. The workspace commits only after native success, response identity validation, coroutine cancellation recheck, and microscope-session identity recheck.
- Pre-output/preflight failures do not leave an empty committed workspace.
- Android/provider storage failures stop the export and roll back any still-uncommitted document.

Predictive free-space reporting is not treated as a portable SAF invariant because document providers do not expose one universal trustworthy capacity API. Phase 6 accepts transactional failure handling; adversarial low-storage/device/provider testing remains a Phase 7 hardening task.

## Unique/group-representative contract

Unique export is a derived view over the authoritative timeline. It never removes or renumbers source frames.

The production hybrid grouping policy is explicit and versioned by the existing Phase 4 persistence identity:

- maximum dHash distance: `8`;
- minimum confirmed luma similarity: `9700` on the documented `0..=10000` similarity scale.

That score is a FrameScope similarity metric, not a literal percentage of changed pixels.

Unique export means exactly one representative per validated group, so every-N sampling is intentionally rejected for this mode.

## Android lifecycle and cancellation

- Source videos continue to enter through `content://` and a caller-owned `ParcelFileDescriptor`.
- Native code owns only duplicated descriptors.
- Export work runs off the main thread.
- Operation ids reuse the existing native cancellation registry.
- Navigation, source replacement, lifecycle cancellation, or explicit export cancellation invalidate stale work.
- A successful long-running native call is rechecked against coroutine and microscope-session state before the SAF workspace can commit.

## Verification gates

The Phase 6 completion path requires all of the following on the exact final acceptance head:

- Rust format.
- Rust clippy with warnings denied.
- Rust workspace tests.
- Real FFmpeg/video fixture integration tests.
- Native Android arm64 build verification.
- JNI export verification, including the unique-group export symbol.
- Android unit tests.
- Android lint.
- Android debug APK build.
- APK native packaging verification.
- Phase 3 index/cache contract workflow.
- Phase 4 similarity/grouping contract workflow.

The final functional integration PR (#63) passed all of those gates before merging to `main` as `c8b521ebe27fbf846734e8cb99b4b3eab51e2736`. This acceptance/documentation PR must pass the same gates against that merged base before Phase 6 is considered closed.

## Evidence map

- Selection planning and every-N sampling: `framescope-extraction`.
- Sequential source-quality execution and timeline reconciliation: `framescope-extraction-video`.
- PNG/JPEG/lossless-WebP encoding: `framescope-extraction-image`.
- Group representative traversal: `framescope-extraction-groups`.
- Group representative sequential decode: `framescope-extraction-groups-video`.
- Streaming JSONL manifest protocol: `framescope-extraction-manifest`.
- Ordinary streaming output orchestration: `framescope-extraction-output`.
- Unique-group encoded output orchestration: `framescope-extraction-groups-output`.
- Current-frame and batch JNI boundaries: `framescope-ffi`.
- Rollback-safe SAF destinations and bounded frame sink: `FrameExportDestination.kt` and `BatchExportDestination.kt`.
- Repository/session ordering and native cancellation: `FrameScopeRepository.kt` and `MicroscopeSessionController.kt`.
- Current-frame and indexed/unique batch export UI: `CurrentFrameExportOverlay.kt`, `FrameExportViewModel.kt`, `BatchExportOverlay.kt`, and `BatchExportViewModel.kt`.
- Native arm64 JNI-symbol verification: `scripts/verify-native-android.sh`.

## Deferred to Phase 7

Phase 7 owns production hardening rather than changing the Phase 6 extraction semantics:

- physical-device and codec/provider compatibility matrix;
- performance and peak-memory profiling on representative phones and large media;
- adversarial low-storage and provider-failure testing;
- malformed-media hardening beyond the existing decoder/index regression suite;
- dependency, security, and license review;
- accessibility and UI polish;
- release signing, reproducible release workflow, versioning, and durable GitHub Release artifacts.

No acceptance step may weaken an existing correctness invariant or remove a regression test merely to make CI pass.
