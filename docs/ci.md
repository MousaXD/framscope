# Continuous integration

FrameScope uses GitHub-hosted CI as the primary verification environment for Rust, Android/NDK builds, and generated video fixtures. PR builds require no repository secrets.

Feature branches are verified by `pull_request`. Direct pushes trigger CI only for `main`, `release/**`, and tags so the same feature commit is not built twice as both a branch push and a PR.

## Workflow layout

`.github/workflows/ci.yml` uses concurrency cancellation so a newer run for the same PR or branch cancels its obsolete predecessor.

### `rust-fast`

Runs the inexpensive host-side Rust gate:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`

This job pins Rust `1.85.1` to exercise the workspace's declared Rust 1.85 minimum. It caches the Cargo registry, Cargo git checkout data, and host `rust/target` outputs. The cache key includes the exact host compiler version and Rust workspace manifests.

### `native-android`

Builds the existing Rust JNI library for `aarch64-linux-android` using the repository-owned `scripts/build-rust.sh`, then `scripts/verify-native-android.sh` verifies:

- an arm64-v8a `.so` was produced;
- the ELF machine is AArch64;
- required JNI exports are present.

The job installs NDK `27.3.13750724`, Rust `1.86.0`, and `cargo-ndk` `4.1.2`. Rust 1.86.0 is intentionally newer than the workspace's 1.85 host MSRV because `cargo-ndk 4.1.2` itself requires Rust 1.86 or newer. This is a build-tool requirement, not a change to the workspace's declared MSRV.

The job caches Cargo registry/git data, the `cargo-ndk` install metadata/binary, and Android Rust target outputs. The cache key includes the exact Android Rust toolchain, NDK version, cargo-ndk version, Rust manifests, and `scripts/build-rust.sh`.

**Agent 1 integration point:** FFmpeg/NDK setup belongs in this job or in repository scripts called by this job. When Agent 1 introduces a reproducible FFmpeg artifact/cache, its version, build configuration, and checksum/build-script inputs must be part of that cache key. Do not create a second Android toolchain in another job.

### `android`

Runs after `native-android` succeeds. It installs Java 17, Gradle 8.13, Android Platform 36 / Build Tools 36.0.0, the same Rust 1.86.0/NDK toolchain, restores the Android Cargo cache, then runs:

- `gradle --no-daemon testDebugUnitTest`
- `gradle --no-daemon lintDebug`
- `gradle --no-daemon assembleDebug`
- `scripts/verify-debug-apk.sh`

The packaging verifier checks that `lib/arm64-v8a/libframescope_ffi.so` is actually in the APK and preserves the existing privacy-permission invariants.

Debug APKs are not uploaded for ordinary PR/feature-branch builds. They are uploaded for `main`, `release/*`, tags, and manual workflow dispatch only, with seven-day retention.

### `video-fixtures`

Generates the synthetic fixture set described by `fixtures/video/manifest.json`, verifies every metadata contract with `ffprobe`, and enforces a 1 MiB generated-fixture budget. The generated video files are not committed or uploaded by routine CI.

This job is deliberately **not** a decoder integration test. It verifies the fixture corpus only.

## Wave B / Agent 2 integration

Once the Rust decoder exists, extend CI with a `video-integration` gate that consumes the generated fixture directory and runs real decoder assertions. That job should depend on the native build prerequisite and must not run when prerequisite native/build jobs fail.

The decoder tests should use the manifest as known truth for dimensions, duration/timestamp behavior, stream counts, rotation, codecs, and expected corruption failure. Do not replace these assertions with a simple "did not crash" smoke test.

## Caching rules

- Cargo host and Android-target caches use separate keys.
- Host target-output cache keys include Rust `1.85.1`.
- Android target-output cache keys include Rust `1.86.0`, the NDK version, and cargo-ndk version.
- Gradle caching is handled by `gradle/actions/setup-gradle`; feature/PR refs are read-only.
- No secrets belong in cached paths.
- FFmpeg caches added by Agent 1 must include every correctness-relevant build input, not merely a branch name.

## Action security

Third-party and GitHub Actions used by the workflow are pinned to immutable commit SHAs with the human-readable release version kept in a comment. The workflow uses `pull_request`, not `pull_request_target`, and only requests `contents: read` permission.

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
./scripts/verify-native-android.sh
```

Android checks:

```bash
cd android
gradle --no-daemon testDebugUnitTest
gradle --no-daemon lintDebug
gradle --no-daemon assembleDebug
cd ..
./scripts/verify-debug-apk.sh
```

Fixture checks:

```bash
./scripts/generate-video-fixtures.sh
./scripts/verify-video-fixtures.py
```
