# Development Guide

## Prerequisites

- JDK 17+
- Gradle 8.13.
- Android SDK Platform 36
- Android Build Tools 36.x
- Android NDK r27d (`27.3.13750724`)
- Rust stable
- Rust target `aarch64-linux-android`
- `cargo-ndk`

```bash
rustup target add aarch64-linux-android
cargo install cargo-ndk --locked
```

## Rust workflow

```bash
cd rust
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The Rust crates must remain Android-independent except `framescope-ffi`.

## Native Android library

From the repository root:

```bash
./scripts/build-rust.sh
```

Gradle also invokes cargo-ndk through the `:app:buildRustArm64` task before Android `preBuild`. The generated `.so` lives under `android/app/build/generated/jniLibs/arm64-v8a/` and is not committed.

## Android workflow

```bash
cd android
gradle testDebugUnitTest
gradle lintDebug
gradle assembleDebug
```

The application must continue to build without an `INTERNET` permission or broad storage permissions.

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
- `framescope-cache` must remain bounded by design when payload caching is introduced.
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
