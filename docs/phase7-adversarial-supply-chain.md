# Phase 7 adversarial media and supply-chain hardening

This Phase 7 slice hardens two production boundaries without changing FrameScope frame identity, timing, cache, similarity, extraction, or output-order semantics.

## Deterministic hostile-media matrix

The generated fixture set now includes healthy codec coverage plus deterministic hostile inputs:

- a valid AAC-only source, which must be rejected as `NoVideoTrack` rather than treated as malformed video;
- a header-only truncated MP4, which must fail as a typed malformed-container error;
- a zero-byte source and deterministic non-container bytes, which must be rejected without panic;
- a fast-start H.264 MP4 with its media payload truncated, which may fail at open/decode or end early but must never be accepted as the complete healthy 12-frame source.

`fixtures/video/manifest.json` is the shared fixture contract. `scripts/verify-video-fixtures.py` verifies the generated corpus with FFprobe, while `framescope-video/tests/decoder_fixtures.rs` verifies FrameScope's own typed open/decode behavior against the same files.

The hostile corpus is intentionally deterministic. Phase 7 acceptance should not depend on unreproducible random bytes or a fuzz seed that is not retained.

## Rust dependency policy

`scripts/verify-rust-supply-chain.py` fails closed when a non-workspace Rust dependency:

- comes from a source other than the approved crates.io registry;
- lacks an SPDX license expression;
- contains a license atom that has not been explicitly reviewed in the script allowlist; or
- lacks a valid SHA-256 checksum in `rust/Cargo.lock`.

Workspace crates are excluded from the third-party license check because they inherit FrameScope's `GPL-3.0-only` workspace policy.

The Phase 7 workflow also runs RustSec `cargo-audit` against the locked dependency graph. The audit executable is not installed from a floating version: CI downloads upstream `cargo-audit 0.22.2`, verifies the published Linux archive SHA-256, and only then executes it.

## Build dependency immutability

`scripts/verify-build-supply-chain.py` enforces repository-level declarations:

- external GitHub Actions must use full 40-hex commit SHAs;
- Docker actions, if introduced, must use a SHA-256 digest;
- Android plugin and library declarations may not use dynamic versions such as `+`, `latest.*`, or `SNAPSHOT`;
- custom Maven/Ivy repositories require an explicit policy change rather than silently entering the build;
- Android dependency repositories remain centralized with `RepositoriesMode.FAIL_ON_PROJECT_REPOS` and the reviewed Google/Maven Central sources.

Dependabot remains responsible for proposing Cargo, Gradle, and GitHub Actions updates; these gates ensure an update still has to pass the repository's exact-head verification.

## Native FFmpeg provenance and license mode

The existing native build remains part of this supply-chain boundary:

- FFmpeg source version and tarball SHA-256 are pinned in `scripts/build-ffmpeg-android.sh`;
- Android NDK/API/ABI configuration is pinned;
- autodetection is disabled and the permitted decoders/demuxers/parsers/protocols are explicitly selected;
- unexpected external codec libraries are rejected by `scripts/verify-ffmpeg-android.sh`;
- generated build metadata must report `FFMPEG_LICENSE_MODE=GPLv3-or-later`, consistent with FrameScope's GPL-3.0-only distribution policy.

## Deliberate remaining work

This slice does not claim physical-device compatibility, peak-memory numbers, or complete Gradle artifact checksum locking. Those require device/profile evidence and, for Gradle verification metadata, a reviewed committed artifact hash set. They remain Phase 7 work rather than being papered over by a weak generated-at-build-time checksum list.
