#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/android/app/build/generated/jniLibs"
FFMPEG_ROOT="${FRAMESCOPE_FFMPEG_ROOT:-$ROOT/.native/ffmpeg/arm64-v8a}"

command -v cargo >/dev/null || { echo "error: cargo is required" >&2; exit 1; }
if ! cargo ndk --version >/dev/null 2>&1; then
  echo "error: cargo-ndk is required (cargo install cargo-ndk --locked)" >&2
  exit 1
fi

FRAMESCOPE_FFMPEG_ROOT="$FFMPEG_ROOT" "$ROOT/scripts/build-ffmpeg-android.sh"
FRAMESCOPE_FFMPEG_ROOT="$FFMPEG_ROOT" "$ROOT/scripts/verify-ffmpeg-android.sh"

mkdir -p "$OUT"
cd "$ROOT/rust"
FRAMESCOPE_FFMPEG_ROOT="$FFMPEG_ROOT" \
  CARGO_TARGET_DIR="$ROOT/rust/target" \
  cargo ndk \
    -t arm64-v8a \
    -o "$OUT" \
    build -p framescope-ffi --release

test -f "$OUT/arm64-v8a/libframescope_ffi.so"
echo "Rust JNI library: $OUT/arm64-v8a/libframescope_ffi.so"
