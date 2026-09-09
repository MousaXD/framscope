#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NDK_VERSION="27.3.13750724"
ANDROID_API="26"
ABI="arm64-v8a"
PREFIX="${FRAMESCOPE_FFMPEG_HWDECODE_ROOT:-$ROOT/.native/ffmpeg-hwdecode-probe/$ABI}"
OUTPUT_DIR="${FRAMESCOPE_HWDECODE_BENCH_OUT:-$ROOT/.native/hwdecode-bench/$ABI}"
SOURCE="$ROOT/tools/android-hwdecode/framescope_decode_bench.c"

"$ROOT/scripts/build-ffmpeg-android-hwdecode-probe.sh"

NDK_ROOT="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
if [[ -z "$NDK_ROOT" && -n "${ANDROID_HOME:-}" ]]; then
  NDK_ROOT="$ANDROID_HOME/ndk/$NDK_VERSION"
fi
if [[ -z "$NDK_ROOT" && -n "${ANDROID_SDK_ROOT:-}" ]]; then
  NDK_ROOT="$ANDROID_SDK_ROOT/ndk/$NDK_VERSION"
fi
if [[ -z "$NDK_ROOT" || ! -d "$NDK_ROOT" ]]; then
  echo "error: Android NDK r27d ($NDK_VERSION) was not found" >&2
  exit 1
fi

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) HOST_TAG="linux-x86_64" ;;
  Darwin-x86_64|Darwin-arm64) HOST_TAG="darwin-x86_64" ;;
  *)
    echo "error: unsupported host $(uname -s)-$(uname -m)" >&2
    exit 1
    ;;
esac

CC="$NDK_ROOT/toolchains/llvm/prebuilt/$HOST_TAG/bin/aarch64-linux-android${ANDROID_API}-clang"
mkdir -p "$OUTPUT_DIR"

"$CC" \
  -std=c17 \
  -O2 \
  -fPIE \
  -pie \
  -Wall \
  -Wextra \
  -Werror \
  -Wl,-z,max-page-size=16384 \
  -I"$PREFIX/include" \
  "$SOURCE" \
  -L"$PREFIX/lib" \
  -Wl,--start-group \
  -lavformat \
  -lavcodec \
  -lavutil \
  -Wl,--end-group \
  -lm \
  -ldl \
  -llog \
  -landroid \
  -o "$OUTPUT_DIR/framescope-decode-bench"

echo "Android hardware-decode benchmark ready: $OUTPUT_DIR/framescope-decode-bench"
echo "No root is required. Push it with adb to an app-writable or shell-writable location for physical-device runs."
