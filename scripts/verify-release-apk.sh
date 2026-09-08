#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APK="${1:-$ROOT/android/app/build/outputs/apk/release/app-release-unsigned.apk}"
VERSION_FILE="$ROOT/version.properties"

command -v unzip >/dev/null || { echo "error: unzip is required" >&2; exit 1; }
command -v apkanalyzer >/dev/null || { echo "error: apkanalyzer is required" >&2; exit 1; }
test -f "$APK" || { echo "error: release APK not found: $APK" >&2; exit 1; }
test -f "$VERSION_FILE" || { echo "error: version.properties is missing" >&2; exit 1; }

expected_version_name="$(awk -F= '$1 == "versionName" { print $2 }' "$VERSION_FILE" | tr -d '\r')"
expected_version_code="$(awk -F= '$1 == "versionCode" { print $2 }' "$VERSION_FILE" | tr -d '\r')"
[[ "$expected_version_name" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  echo "error: invalid semantic versionName in version.properties" >&2
  exit 1
}
[[ "$expected_version_code" =~ ^[1-9][0-9]*$ ]] || {
  echo "error: invalid positive versionCode in version.properties" >&2
  exit 1
}

entries="$(unzip -Z1 "$APK")"
grep -qx 'lib/arm64-v8a/libframescope_ffi.so' <<<"$entries" || {
  echo "error: arm64-v8a Rust JNI library is missing from the release APK" >&2
  exit 1
}
if grep -Eq '^lib/(armeabi-v7a|x86|x86_64)/libframescope_ffi\.so$' <<<"$entries"; then
  echo "error: an unsupported FrameScope ABI was packaged in the release APK" >&2
  exit 1
fi

application_id="$(apkanalyzer manifest application-id "$APK")"
version_name="$(apkanalyzer manifest version-name "$APK")"
version_code="$(apkanalyzer manifest version-code "$APK")"
debuggable="$(apkanalyzer manifest debuggable "$APK")"
permissions="$(apkanalyzer manifest permissions "$APK")"

[[ "$application_id" == "com.framescope.app" ]] || {
  echo "error: unexpected release application id: $application_id" >&2
  exit 1
}
[[ "$version_name" == "$expected_version_name" ]] || {
  echo "error: release versionName $version_name does not match $expected_version_name" >&2
  exit 1
}
[[ "$version_code" == "$expected_version_code" ]] || {
  echo "error: release versionCode $version_code does not match $expected_version_code" >&2
  exit 1
}
[[ "$debuggable" == "false" ]] || {
  echo "error: release APK is debuggable" >&2
  exit 1
}

if grep -Eq 'android\.permission\.(INTERNET|READ_EXTERNAL_STORAGE|WRITE_EXTERNAL_STORAGE|MANAGE_EXTERNAL_STORAGE)' <<<"$permissions"; then
  echo "error: release APK contains a forbidden network or broad-storage permission" >&2
  printf '%s\n' "$permissions" >&2
  exit 1
fi

printf 'Verified release APK identity, version, native packaging, and privacy invariants: %s\n' "$APK"
