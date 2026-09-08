# Phase 7 Physical Device and Local Provider Acceptance

**Status: pending physical evidence.** Repository/CI hardening is complete, but Phase 7 must not be declared production-accepted until `acceptance/phase7/device-report.json` exists and passes `scripts/verify-phase7-device-report.py`.

## Scope

FrameScope currently ships only `arm64-v8a`, with `minSdk 26` and `targetSdk 36`.

Both source and export pickers deliberately set Android `Intent.EXTRA_LOCAL_ONLY`. Phase 7 therefore certifies **local Android Storage Access Framework / DocumentsProvider behavior**. Cloud-only providers are outside the current product contract and must not be listed as accepted evidence unless the picker contract changes first.

At least one non-emulator physical Android device is required. Additional devices and local providers are useful compatibility evidence but do not replace the required baseline report.

## Test artifact

Use an APK built from the exact `tested_commit` recorded in the report. Record the APK SHA-256 before installation. A debug APK is acceptable for physical feature/provider acceptance because release-mode identity, packaging, native hardening, and non-debuggable configuration are independently enforced by CI; a signed release build is preferable when available.

The manual `Phase 7 physical-device kit` GitHub Actions workflow builds an APK and bundles the deterministic media corpus, report template, and this procedure. It exists specifically so the physical test does not require a local Rust/Android build.

## Device metadata

Record the physical device manufacturer, model, Android API level, and primary ABI. The accepted report requires:

- `is_emulator: false`;
- ABI `arm64-v8a`;
- Android API 26 through 36 inclusive.

Useful ADB evidence:

```text
adb shell getprop ro.product.manufacturer
adb shell getprop ro.product.model
adb shell getprop ro.build.version.sdk
adb shell getprop ro.product.cpu.abi
adb shell getprop ro.kernel.qemu
```

Do not mark a report accepted if the target is an emulator or if the tested APK does not match the recorded SHA-256.

## Local provider capabilities

Use a folder visible through FrameScope's local-only system picker. Record the provider name and authority. The baseline provider must prove all of these behaviors:

- visible through the local-only picker;
- can create the isolated export workspace directory;
- can create frame and manifest documents;
- returns a writable `ParcelFileDescriptor` for the app's `wt` open mode;
- preserves the requested stable frame/manifest names inside the workspace;
- can delete an uncommitted document during rollback;
- can delete an uncommitted workspace during rollback.

FrameScope intentionally does not assume POSIX directory file descriptors or atomic filesystem rename semantics for SAF.

## Deterministic codec/media matrix

Run the media included in the physical-device kit and mark every case pass/fail:

- H.264 CFR;
- H.264 VFR;
- HEVC CFR;
- VP9 CFR;
- AV1 CFR;
- rotated portrait metadata;
- H.264 with audio;
- multiple streams;
- unusual dimensions;
- very short video.

A pass means the source opens without a crash, the selected video stream is sensible, and exact navigation/presentation remains usable. The deterministic CI oracle remains authoritative for exact metadata; the physical run is a compatibility check, not a replacement oracle.

## Required interaction/export scenarios

The report must mark every required scenario `pass`:

1. install and launch;
2. open local video through the system picker;
3. previous/next and first/last boundary behavior;
4. indexed VFR timestamp navigation;
5. zoom and pan;
6. similarity group navigation;
7. background then foreground with the microscope session still coherent;
8. source replacement without stale frame/export state winning;
9. current-frame PNG export;
10. current-frame JPEG export;
11. current-frame lossless WebP export;
12. batch FrameId range export;
13. batch indexed timestamp range export;
14. every-N export;
15. all-frames export on a small deterministic fixture;
16. unique/group-representative export;
17. cooperative batch cancellation;
18. workspace manifest and stable relative filenames;
19. abort rollback cleanup;
20. force-stop during export without a false success being presented after restart.

### Force-stop semantics

A process death can occur when no code remains alive to delete the active SAF workspace. A provider may therefore retain an incomplete `framescope_export_*` directory. This is not a successful export: its manifest cannot contain the normal terminal complete record. The physical acceptance requirement is that FrameScope never reports that interrupted run as successful after restart and that the provider's partial workspace remains user-removable.

Do **not** add broad startup deletion of `framescope_export_*` directories merely to hide this condition. Automatic deletion could destroy a valid export from another app instance or a directory copied/renamed by the user.

## Large-media physical evidence

The accepted report requires one local source of at least **1 GiB**. Record:

- source name, byte size, duration, codec, width, and height;
- open-to-ready elapsed milliseconds;
- elapsed milliseconds for ten exact navigation steps;
- the every-N stride used for a representative sampled export;
- sampled-export elapsed milliseconds;
- observed peak PSS in KiB.

For memory evidence, capture `adb shell dumpsys meminfo com.framescope.app` during or immediately after the representative operation and record the highest observed total PSS. Timing and PSS are observational: there is no universal pass/fail number because device, source, storage, and thermal conditions vary. Crashes, OOMs, corruption, stale output, or loss of exact frame semantics are failures regardless of speed.

## Completing the report

Copy:

`acceptance/phase7/device-report.template.json`

to:

`acceptance/phase7/device-report.json`

Fill the actual commit, APK hash, timestamp, device/provider metadata, all result states, large-media observations, and useful notes. Then run:

```text
python3 scripts/verify-phase7-device-report.py acceptance/phase7/device-report.json
```

The validator is deliberately fail-closed. `pending`, `blocked`, or `fail` results cannot unlock a production release.

## Release linkage

The signed release workflow requires an accepted physical report before it touches the release build/signing path. It also verifies that the tested commit is an ancestor of the release tag and that no APK-affecting Android/Rust/native-build inputs changed after the physical test. A report-only/documentation commit may follow the tested commit without invalidating the physical evidence.
