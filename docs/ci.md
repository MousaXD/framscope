# Continuous integration

FrameScope uses GitHub-hosted CI as the primary verification environment for Rust, Android/NDK builds, the pinned Android FFmpeg toolchain, and generated real-media decoder fixtures. Pull-request builds require no repository secrets.

Feature branches are verified by `pull_request`. Direct pushes trigger CI only for `main`, `release/**`, and tags so the same feature commit is not built twice as both a branch push and a PR.

## Workflow layout

`.github/workflows/ci.yml` uses concurrency cancellation so a newer run for the same PR or branch cancels its obsolete predecessor.

### `rust-fast`

Runs the inexpensive host-side Rust gate:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`

This job pins Rust `1.85.1` to exercise the workspace's declared Rust 1.85 minimum. It caches the Cargo registry, Cargo git checkout data, and host `rust/target` outputs. The cache namespace includes the host compiler version and Rust workspace manifests; Cargo's own fingerprints remain authoritative for source changes.

### `native-android`

Builds the Rust JNI library for `aarch64-linux-android` through the repository-owned native scripts. `scripts/build-rust.sh` first creates or validates the pinned FFmpeg source build and then links it statically into `libframescope_ffi.so`. `scripts/verify-native-android.sh` verifies:

- an arm64-v8a `.so` was produced;
- the ELF machine is AArch64;
- the required version, file-descriptor inspection, and cancellation JNI exports are present.

The job installs NDK `27.3.13750724`, Rust `1.86.0`, and `cargo-ndk` `4.1.2`. Rust 1.86.0 is intentionally newer than the workspace's 1.85 host MSRV because `cargo-ndk 4.1.2` itself requires Rust 1.86 or newer. This is a build-tool requirement, not a change to the workspace's declared MSRV.

Android Cargo cache keys include the exact Android Rust toolchain, NDK version, cargo-ndk version, workspace manifests, FFmpeg build/link shim inputs, and repository native build/verification scripts. The FFmpeg prefix itself is not uploaded as a routine CI artifact. Cargo also tracks the verified external FFmpeg archives and build metadata so a restored target cache cannot silently retain an older statically linked prefix.

### `android`

Runs after `native-android` succeeds. It installs Java 17, Gradle 8.13, Android Platform 36 / Build Tools 36.0.0, the same Rust 1.86.0/NDK toolchain, restores the Android Cargo cache, then runs:

- `gradle --no-daemon testDebugUnitTest`
- `gradle --no-daemon lintDebug`
- `gradle --no-daemon assembleDebug`
- `bash scripts/verify-debug-apk.sh`

The packaging verifier checks that `lib/arm64-v8a/libframescope_ffi.so` is actually in the APK, rejects unsupported packaged FrameScope ABIs, and preserves the no-broad-storage-permission and no-INTERNET-permission invariants.

Debug APKs are not uploaded for ordinary PR/feature-branch builds. They are uploaded only for `main`, `release/*`, tags, and manual workflow dispatch, with seven-day retention.

### `video-fixtures`

Generates the synthetic fixture set described by `fixtures/video/manifest.json`, verifies every fixture contract with `ffprobe`, enforces a 1 MiB generated-fixture budget, and then runs the real Rust decoder integration suite against system FFmpeg development libraries:

- `cargo clippy -p framescope-video --features system-ffmpeg --all-targets -- -D warnings`
- `cargo test -p framescope-video --features system-ffmpeg`

The decoder tests assert real behavior, not merely that decoding did not crash. Coverage includes H.264 CFR and VFR timestamps, HEVC, VP9, AV1, audio-plus-video stream discovery, multiple video-stream selection, rotation, unusual dimensions, one-frame EOF, malformed/truncated input, cancellation, file-descriptor ownership, seek flushing, and stable EOF.

The generated media files are not committed and are not uploaded by routine CI.

## Fixture strategy

Fixtures are generated from FFmpeg synthetic sources with deterministic/bitexact settings where supported and constrained encoder threading. The corpus intentionally contains known-good and known-bad media. `scripts/verify-video-fixtures.py` validates codec, dimensions, stream counts, duration tolerance, frame counts, rotation, actual presentation-timestamp cadence, and that the truncated fixture is rejected by `ffprobe`.

The host `system-ffmpeg` feature exists only for decoder integration tests. Normal host builds do not acquire an FFmpeg dependency, and Android never links against the host FFmpeg packages.

## Caching rules

- Cargo host and Android-target caches use separate namespaces.
- Host target-output cache keys include Rust `1.85.1`.
- Android target-output cache keys include Rust `1.86.0`, the NDK version, cargo-ndk version, and correctness-relevant FFmpeg/native build inputs.
- The Android FFmpeg prefix records the exact source version/hash, NDK/API/ABI, enabled components, license mode, and the SHA-256 of the repository build recipe. A changed recipe invalidates prefix reuse.
- Gradle caching is handled by `gradle/actions/setup-gradle`; feature/PR refs are read-only.
- No secrets belong in cached paths.

## Action security and artifacts

Third-party and GitHub Actions used by the workflow are pinned to immutable commit SHAs with the human-readable release version kept in a comment. The workflow uses `pull_request`, not `pull_request_target`, and requests only `contents: read` permission.

Routine PR builds do not upload APKs or generated media. Concurrency cancellation prevents obsolete runs for the same PR from continuing after a newer commit arrives.

## Local reproduction

Fast Rust checks at the declared host MSRV:

```bash
cd rust
cargo +1.85.1 fmt --all --check
cargo +1.85.1 clippy --workspace --all-targets -- -D warnings
cargo +1.85.1 test --workspace
```

Native Android verification after installing the documented NDK/Rust target/cargo-ndk prerequisites:

```bash
rustup toolchain install 1.86.0 --target aarch64-linux-android
cargo +1.86.0 install cargo-ndk --locked --version 4.1.2
rustup default 1.86.0
bash ./scripts/verify-native-android.sh
```

Android checks:

```bash
cd android
gradle --no-daemon testDebugUnitTest
gradle --no-daemon lintDebug
gradle --no-daemon assembleDebug
cd ..
bash ./scripts/verify-debug-apk.sh
```

Fixture plus real-decoder checks on a host with FFmpeg development packages:

```bash
bash ./scripts/generate-video-fixtures.sh
python3 ./scripts/verify-video-fixtures.py
cd rust
cargo clippy -p framescope-video --features system-ffmpeg --all-targets -- -D warnings
cargo test -p framescope-video --features system-ffmpeg
```
