# Development Guide

## Prerequisites

- JDK 17+
- Gradle 8.13.
- Android SDK Platform 36
- Android Build Tools 36.x
- Android NDK r27d (`27.3.13750724`)
- Rust 1.85+ for the workspace host MSRV; CI tests it with `1.85.1`
- Rust 1.86+ for the current Android build tooling; CI pins `1.86.0`
- Rust target `aarch64-linux-android`
- `cargo-ndk` (CI pins `4.1.2`)

```bash
rustup toolchain install 1.85.1 --component rustfmt --component clippy
rustup toolchain install 1.86.0 --target aarch64-linux-android
cargo +1.86.0 install cargo-ndk --locked --version 4.1.2
```

`cargo-ndk 4.1.2` requires Rust 1.86 or newer. That Android build-tool constraint does not raise the Rust workspace's declared 1.85 host MSRV.

Keep local tooling aligned with the CI pins when reproducing native CI failures.

## Rust workflow

```bash
cd rust
cargo +1.85.1 fmt --all --check
cargo +1.85.1 clippy --workspace --all-targets -- -D warnings
cargo +1.85.1 test --workspace
```

The Rust crates must remain Android-independent except `framescope-ffi`.

## Phase 3 frame-index workflow

The persistent metadata index lives in `framescope-cache`; the streaming decoder-to-index pipeline
lives in `framescope-video`. Run the normal workspace checks plus the real-media fixture tests:

```bash
bash ./scripts/generate-video-fixtures.sh
cd rust
cargo +1.85.1 test -p framescope-cache
cargo +1.85.1 test -p framescope-video --features system-ffmpeg --test frame_index_fixtures
```

When changing the schema, source identity, lifecycle, or resume logic, verify at minimum:

- schema creation/reopen and version behavior;
- stale and unverifiable source recreation;
- corrupt/not-a-database recovery;
- interrupted indexing and fresh-decoder reconciliation;
- exact frame and timestamp lookup;
- keyframe anchors;
- VFR PTS persistence against real decoded timestamps;
- bounded pending batch size.

Do not resume by assuming Phase 2's decoder-epoch-local frame number identifies persistent codec
state. Reopen a decoder and reconcile existing rows, or rebuild safely. See `docs/frame-index.md`.

## Native Android library

From the repository root, with Rust 1.86.0 selected for the Android build tooling:

```bash
rustup default 1.86.0
./scripts/build-rust.sh
```

Gradle also invokes cargo-ndk through the `:app:buildRustArm64` task before Android `preBuild`. The generated `.so` lives under `android/app/build/generated/jniLibs/arm64-v8a/` and is not committed.

For the same native checks used by CI:

```bash
bash ./scripts/verify-native-android.sh
```

## Android workflow

```bash
cd android
gradle testDebugUnitTest
gradle lintDebug
gradle assembleDebug
cd ..
bash ./scripts/verify-debug-apk.sh
```

The application must continue to build without an `INTERNET` permission or broad storage permissions.

## Video fixture workflow

Phase 2 media fixtures are generated, not committed:

```bash
bash ./scripts/generate-video-fixtures.sh
python3 ./scripts/verify-video-fixtures.py
```

See `fixtures/video/README.md` for the fixture contract. Fixture codec coverage does not by itself claim application decoder support.

## Continuous integration

CI is split into host Rust, Android native, Android APK, and fixture-contract jobs with concurrency cancellation and correctness-keyed caches. See `docs/ci.md` for job responsibilities, cache rules, artifact policy, and Agent 1 / Agent 2 integration points.

## Full verification

```bash
./scripts/verify.sh
```

Do not weaken an assertion, lint rule, parser bound, or error check simply to make verification green. Fix the actual defect or document a real platform limitation.

## Module rules

- UI does not call JNI directly.
- Android `ContentResolver`/`Uri` types do not enter Rust.
- `framescope-core` stays platform-neutral.
- `framescope-video` owns media semantics, not UI strings.
- `framescope-cache` owns persistent metadata/source identity and must not become an unbounded pixel store.
- FFI functions must not allow unwinding across JNI.

## Adding another ABI later

1. Install its Rust Android target.
2. Add the ABI to Android `abiFilters`.
3. Add the cargo-ndk target to the native build task/script.
4. Verify the corresponding `.so` exists in the APK.

Do not add obsolete ABIs without a concrete compatibility reason.

## Media parser changes

Treat all video bytes as untrusted. New parsing code must:

- obey parent/container bounds;
- use checked arithmetic for offsets and sizes;
- bound attacker-controlled loop counts;
- avoid full-file reads;
- return `FrameScopeError` instead of panicking on malformed input;
- include tests for valid and malformed examples.
