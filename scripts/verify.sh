#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "== Rust format =="
(cd "$ROOT/rust" && cargo fmt --all --check)

echo "== Rust clippy =="
(cd "$ROOT/rust" && cargo clippy --workspace --all-targets -- -D warnings)

echo "== Rust tests =="
(cd "$ROOT/rust" && cargo test --workspace)

echo "== Android unit tests =="
(cd "$ROOT/android" && gradle --no-daemon testDebugUnitTest)

echo "== Android lint =="
(cd "$ROOT/android" && gradle --no-daemon lintDebug)

echo "== Android debug build (includes Rust arm64 build) =="
(cd "$ROOT/android" && gradle --no-daemon assembleDebug)

APK="$ROOT/android/app/build/outputs/apk/debug/app-debug.apk"
test -f "$APK"

if ! unzip -l "$APK" | grep -q 'lib/arm64-v8a/libframescope_ffi.so'; then
  echo "error: Rust JNI library is not packaged in the debug APK" >&2
  exit 1
fi

NATIVE_LIB="$ROOT/android/app/build/generated/jniLibs/arm64-v8a/libframescope_ffi.so"
test -f "$NATIVE_LIB"
if command -v nm >/dev/null 2>&1; then
  nm -D "$NATIVE_LIB" | grep -q 'Java_com_framescope_app_data_RustBridge_nativeVersion'
  nm -D "$NATIVE_LIB" | grep -q 'Java_com_framescope_app_data_RustBridge_nativeInspectVideoFd'
fi

if grep -R --line-number --fixed-string 'android.permission.INTERNET' "$ROOT/android/app/src/main"; then
  echo "error: INTERNET permission must not be declared" >&2
  exit 1
fi

if grep -R --line-number -E 'READ_EXTERNAL_STORAGE|WRITE_EXTERNAL_STORAGE|MANAGE_EXTERNAL_STORAGE' "$ROOT/android/app/src/main/AndroidManifest.xml"; then
  echo "error: broad storage permissions must not be declared" >&2
  exit 1
fi

echo "Verification passed."
