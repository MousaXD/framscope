#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LIB="$ROOT/android/app/build/generated/jniLibs/arm64-v8a/libframescope_ffi.so"

"$ROOT/scripts/build-rust.sh"

test -f "$LIB"
file "$LIB" | grep -Eq 'ELF 64-bit.*(ARM aarch64|aarch64|ARM64)'
readelf -h "$LIB" | grep -Eq 'Machine:[[:space:]]+AArch64'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_RustBridge_nativeVersion'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_RustBridge_nativeInspectVideoFd'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_RustBridge_nativeCancelInspection'

printf 'Verified Android arm64 Rust JNI library: %s\n' "$LIB"
