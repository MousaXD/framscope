# Phase 7 Performance and Large-Media Evidence

**Status: deterministic CI layer accepted; physical-device layer pending.**

Phase 7 adds representative stress and profiling evidence on top of the structural boundedness contracts accepted in Phases 3–6. Performance evidence must never replace correctness invariants or make CI depend on noisy hosted-runner timing.

## What CI proves

The `large-media-stress` job in `.github/workflows/phase7.yml` runs the production extraction coordinator through a deterministic 100,000-frame synthetic index.

The stress harness deliberately avoids test-only whole-video allocations:

- frame-index entries are persisted in batches of at most 256 entries;
- the decoder generates one 2x1 RGBA frame on demand instead of storing a frame vector or queue;
- one decoder instance serves the complete extraction span;
- every tenth frame is selected;
- the sequential decoder reconciles frames only through the final selected frame and does not waste work on an unselected tail;
- with the default 100,000-frame corpus, 10,000 frames are selected and the decoder must stop after exactly 99,991 decoded frames, at selected FrameId 99,990;
- the output sink records counters only and retains no encoded-frame collection;
- the manifest writer counts writes and bytes without retaining the JSONL document;
- the test requires exact decoded/selected/committed counts, strict progress ordering, one decoder open, one initial seek, and bounded manifest write size.

These are deterministic pass/fail contracts. A regression that materializes a video-wide decoded-frame list, selection list, or output list should require an explicit design review rather than increasing a CI memory allowance.

## Accepted hosted-runner evidence

The final pre-merge Phase 7 stress run for the accepted large-media integration reported:

- indexed frames: 100,000;
- selected/committed frames: 10,000;
- final selected FrameId: 99,990;
- decoded frames: 99,991;
- decoder opens: 1;
- decoder seeks: 1;
- index batch limit: 256;
- manifest bytes: 3,849,189;
- manifest write calls: 1,140,108;
- largest individual manifest write: 30 bytes;
- observed wall time: approximately 2.43 seconds;
- observed maximum resident set size: 34,092 KiB;
- major page faults: 0;
- swaps: 0.

These timing/RSS numbers describe one GitHub-hosted Linux runner and are **not** thresholds. CPU model, host load, kernel state, caches, and runner image changes are outside FrameScope's control.

The job precompiles the stress harness before measurement, runs it under `/usr/bin/time -v`, and uploads the structural log plus process-accounting report as a short-lived `phase7-large-media-stress-<commit>` artifact.

## What this does not prove

A Linux hosted runner is not an Android device. This job does not establish:

- touch/navigation latency on a phone;
- Android Java/Kotlin heap pressure;
- GPU/Compose rendering cost;
- thermal throttling or sustained battery behavior;
- SAF provider latency;
- performance differences across Android versions, SoCs, local document providers, codecs, or high-resolution source media.

Those claims require physical-device profiling. The mandatory baseline is defined in `docs/phase7-device-provider-acceptance.md` and recorded through `acceptance/phase7/device-report.json`.

## Acceptance rule

Phase 7 performance acceptance therefore has two layers:

1. **Accepted deterministic CI:** bounded-state contracts, exact frame/output accounting, canonical regression suites, and release gates pass without timing/RSS thresholds.
2. **Pending physical-device evidence:** representative >=1 GiB local media is profiled on non-emulator arm64 Android hardware, including open/navigation/export elapsed time and observed peak PSS.

Correctness, frame identity, timestamp truth, source-quality guarantees, cancellation semantics, and output atomicity remain higher priority than benchmark numbers.
