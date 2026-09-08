# Phase 7 Acceptance

**Overall status: pending physical-device evidence.**

The repository-side production-hardening implementation is accepted through the merged large-media stress integration on `main`. Phase 7 itself is intentionally **not** marked complete because a GitHub-hosted runner cannot prove Android device, Storage Access Framework, UI, thermal, or physical-memory behavior.

## Repository-side acceptance

The following areas are covered by deterministic code/tests/workflows and must remain green:

### Media robustness

- healthy CFR/VFR and codec fixture contracts;
- audio-only/no-video rejection;
- empty and deterministic garbage input rejection;
- header-truncated MP4 rejection;
- fast-start MP4 with truncation inside `mdat`, preserving metadata while damaging the media payload;
- typed decoder/source failures rather than panic or false complete decode.

### Storage and export resilience

- source-quality extraction only;
- constant-memory selection and forward batch decode;
- per-frame PNG/JPEG/lossless-WebP output;
- isolated SAF workspaces and append-only JSONL manifests;
- cancellation and stale-session suppression;
- storage-full/quota/provider error classification;
- one pending SAF document at a time;
- rollback attempts for uncommitted document/workspace artifacts;
- provider failures never converted into successful manifest/output claims.

### Large-media boundedness

The Phase 7 large-media job runs the production extraction coordinator against 100,000 indexed frames with every-10th selection. The deterministic contract requires:

- metadata persistence in batches of at most 256 entries;
- one generated source-quality RGBA frame at a time;
- one decoder open and one initial seek;
- 10,000 selected/committed frames;
- decoding stops at the final selected FrameId 99,990, for exactly 99,991 decoded frames;
- no selected-frame, decoded-frame, encoded-output, or manifest-record video-wide collection;
- bounded individual manifest writes.

Hosted-runner wall time and max RSS are recorded as evidence only. They are not correctness thresholds. See `docs/phase7-performance.md`.

### Supply chain and native provenance

- committed and synchronized `rust/Cargo.lock`;
- crates.io-only third-party Rust sources;
- reviewed dependency license expressions and lockfile checksums;
- checksum-verified RustSec `cargo-audit` and advisory rejection;
- immutable-SHA GitHub Actions policy;
- dynamic Gradle/plugin versions and unreviewed custom repositories rejected;
- pinned FFmpeg source/hash/configuration with external-library/autodetection restrictions;
- arm64 JNI hardening/package verification.

### Release engineering

- semantic version source in `version.properties`;
- release-mode APK verification on ordinary Phase 7 CI;
- signing secrets required as a complete set;
- signed release APK certificate/package verification;
- durable GitHub Release assets with APK and SHA-256 checksum;
- release tag/version linkage;
- physical-device report required before signed release publication.

## Physical acceptance still required

The exact procedure is `docs/phase7-device-provider-acceptance.md`.

The physical report must prove at least:

- non-emulator arm64 Android hardware in the supported API 26–36 range;
- a local-only SAF/DocumentsProvider destination with create/write/stable-name/delete behavior required by FrameScope;
- deterministic H.264 CFR/VFR, HEVC, VP9, AV1, rotation, audio/video, multi-stream, unusual-dimension, and short-media compatibility;
- exact frame/timestamp/group navigation and microscope interactions;
- current-frame and batch export modes, cancellation, manifest naming, and rollback behavior;
- background/foreground and source replacement coherence;
- force-stop mid-export does not become a false successful export;
- representative >=1 GiB local media with observed open/navigation/export time and peak PSS.

The report file is:

`acceptance/phase7/device-report.json`

and must pass:

```text
python3 scripts/verify-phase7-device-report.py acceptance/phase7/device-report.json
```

## Final completion rule

Phase 7 can be marked **Complete** only after:

1. an accepted physical report is committed;
2. the report commit passes canonical CI, Phase 3, Phase 4, Phase 7 production/release contracts, and the device-report contract;
3. no APK-affecting source/build input changed after the tested physical commit;
4. the release workflow's physical-evidence preflight passes for the intended release tag.

An emulator, hosted Linux benchmark, or unit-test fake provider is useful regression evidence but cannot substitute for this physical acceptance requirement.
