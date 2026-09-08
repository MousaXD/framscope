# Phase 7 Performance and Large-Media Evidence

**Status: in progress.**

Phase 7 adds representative stress and profiling evidence on top of the structural boundedness contracts accepted in Phases 3–6. Performance evidence must never replace correctness invariants or make CI depend on noisy hosted-runner timing.

## What CI proves

The `large-media-stress` job in `.github/workflows/phase7.yml` runs the production extraction coordinator through a deterministic 100,000-frame synthetic index.

The stress harness deliberately avoids test-only whole-video allocations:

- frame-index entries are persisted in batches of at most 256 entries;
- the decoder generates one 2x1 RGBA frame on demand instead of storing a frame vector or queue;
- one decoder instance serves the complete extraction span;
- every tenth frame is selected, while all 100,000 authoritative timeline entries are reconciled sequentially;
- the output sink records counters only and retains no encoded-frame collection;
- the manifest writer counts writes and bytes without retaining the JSONL document;
- the test requires exact decoded/selected/committed counts, strict progress ordering, one decoder open, one initial seek, and bounded manifest write size.

These are deterministic pass/fail contracts. A regression that materializes a video-wide decoded-frame list, selection list, or output list should require an explicit design review rather than increasing a CI memory allowance.

## Hosted-runner measurements

After precompiling the stress harness, CI runs it under `/usr/bin/time -v` and preserves:

- the test's structural counter report;
- wall-clock/process accounting from the hosted runner;
- maximum resident set size reported by the operating system.

The files are uploaded as the `phase7-large-media-stress-<commit>` artifact for seven days.

Timing and RSS from GitHub-hosted runners are **observational evidence only**. CPU model, host load, kernel state, caches, and runner image changes are outside FrameScope's control, so no fixed latency or RSS threshold is used as a correctness gate.

## What this does not prove

A Linux hosted runner is not an Android device. This job does not establish:

- touch/navigation latency on a phone;
- Android Java/Kotlin heap pressure;
- GPU/Compose rendering cost;
- thermal throttling or sustained battery behavior;
- SAF provider latency;
- performance differences across Android versions, SoCs, storage providers, codecs, or high-resolution source media.

Those claims require physical-device profiling. Device evidence must be reported separately with the device model, Android version, source codec/resolution/duration, provider/storage path, operation, and observed measurements.

## Acceptance rule

Phase 7 performance acceptance therefore has two layers:

1. **Required deterministic CI:** bounded-state contracts, exact frame/output accounting, canonical regression suites, and release gates must pass.
2. **Physical-device evidence:** representative device/provider profiling is recorded before the first production release. Device measurements may identify optimizations or release blockers, but they must not be converted into fake hosted-runner equivalence.

Correctness, frame identity, timestamp truth, source-quality guarantees, cancellation semantics, and output atomicity remain higher priority than benchmark numbers.
