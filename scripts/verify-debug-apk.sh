#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APK="${1:-$ROOT/android/app/build/outputs/apk/debug/app-debug.apk}"

command -v unzip >/dev/null || { echo "error: unzip is required" >&2; exit 1; }
test -f "$APK" || { echo "error: APK not found: $APK" >&2; exit 1; }

entries="$(unzip -Z1 "$APK")"
grep -qx 'lib/arm64-v8a/libframescope_ffi.so' <<<"$entries" || {
  echo "error: arm64-v8a Rust JNI library is missing from the APK" >&2
  exit 1
}

if grep -Eq '^lib/(armeabi-v7a|x86|x86_64)/libframescope_ffi\.so$' <<<"$entries"; then
  echo "error: an unsupported FrameScope ABI was packaged" >&2
  exit 1
fi

if grep -R --line-number --fixed-string 'android.permission.INTERNET' "$ROOT/android/app/src/main"; then
  echo "error: INTERNET permission must not be declared" >&2
  exit 1
fi

if grep -R --line-number -E 'READ_EXTERNAL_STORAGE|WRITE_EXTERNAL_STORAGE|MANAGE_EXTERNAL_STORAGE' \
  "$ROOT/android/app/src/main/AndroidManifest.xml"; then
  echo "error: broad storage permissions must not be declared" >&2
  exit 1
fi

printf 'Verified debug APK native packaging and privacy invariants: %s\n' "$APK"
