#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LIB="$ROOT/android/app/build/generated/jniLibs/arm64-v8a/libframescope_ffi.so"

"$ROOT/scripts/build-rust.sh"

test -f "$LIB"
file "$LIB" | grep -Eq 'ELF 64-bit.*(ARM aarch64|aarch64|ARM64)'
readelf -h "$LIB" | grep -Eq 'Machine:[[:space:]]+AArch64'

# Production native hardening. The shared object must use read-only relocation metadata, eager
# binding, and a non-executable stack. Embedded runtime search paths and text relocations are not
# accepted because the APK ships all native code in its own ABI directory.
readelf -W -l "$LIB" | grep -q 'GNU_RELRO'
stack_flags="$(readelf -W -l "$LIB" | awk '$1 == "GNU_STACK" { print $(NF - 1) }')"
if [[ -z "$stack_flags" || "$stack_flags" == *E* ]]; then
  echo "error: native library does not have a non-executable GNU_STACK" >&2
  exit 1
fi
readelf -W -d "$LIB" | grep -Eq '\(FLAGS\).*BIND_NOW|\(FLAGS_1\).*NOW'
if readelf -W -d "$LIB" | grep -Eq '\(TEXTREL\)|\bTEXTREL\b'; then
  echo "error: native library contains text relocations" >&2
  exit 1
fi
if readelf -W -d "$LIB" | grep -Eq '\(RPATH\)|\(RUNPATH\)'; then
  echo "error: native library contains a runtime search path" >&2
  exit 1
fi

# Keep the shipped Kotlin/native surface link-checked. A Kotlin `external` declaration that is not
# exported by libframescope_ffi.so is a runtime UnsatisfiedLinkError even when both Kotlin and Rust
# unit tests are green, so all product-critical bridges belong in this gate.
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_RustBridge_nativeVersion'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_RustBridge_nativeInspectVideoFd'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_RustBridge_nativeOpenMicroscopeSession'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_RustBridge_nativeStepMicroscope'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_RustBridge_nativeJumpMicroscopeFrame'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_RustBridge_nativeJumpMicroscopeTimestampUs'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_RustBridge_nativeCloseMicroscopeSession'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_RustBridge_nativeCancelInspection'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_RustMicroscopeIndexingProgressSource_nativeMicroscopeIndexingProgress'

nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_MicroscopePreviewBridge_nativeRenderMicroscopePreviewTimestampUs'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_MicroscopePreviewBridge_nativeRenderMicroscopePreviewFrame'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_MicroscopePreviewBridge_nativePrefetchMicroscopePreviewFrame'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_MicroscopePreviewBridge_nativeCancelMicroscopePreviewSession'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_MicroscopePreviewBridge_nativeForgetMicroscopePreviewSession'

nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_MicroscopeSimilarityBridge_nativeFindSimilarFrames'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_MicroscopeSimilarityBridge_nativeCancelSimilarity'

nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_RamAccelerationBridge_nativeConfigureRamAcceleration'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_RamAccelerationBridge_nativeRamAccelerationStats'

nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_RustUniqueExportBridge_nativeExportMicroscopeUniqueGroupsFd'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_FrameScopeStorageBridge_nativeStorageStats'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_FrameScopeStorageBridge_nativeClearStorage'
nm -D "$LIB" | grep -q 'Java_com_framescope_app_data_FrameScopeStorageBridge_nativeClearSourceIndexes'

printf 'Verified hardened Android arm64 Rust JNI library: %s\n' "$LIB"
