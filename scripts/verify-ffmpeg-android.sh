#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ABI="arm64-v8a"
PREFIX="${FRAMESCOPE_FFMPEG_ROOT:-$ROOT/.native/ffmpeg/$ABI}"
MARKER="$PREFIX/share/framescope/build-info.env"
COMPONENTS="$PREFIX/share/framescope/config_components.h"

required_files=(
  "$PREFIX/include/libavcodec/avcodec.h"
  "$PREFIX/include/libavformat/avformat.h"
  "$PREFIX/include/libavutil/avutil.h"
  "$PREFIX/include/libswscale/swscale.h"
  "$PREFIX/lib/libavcodec.a"
  "$PREFIX/lib/libavformat.a"
  "$PREFIX/lib/libavutil.a"
  "$PREFIX/lib/libswscale.a"
  "$MARKER"
  "$COMPONENTS"
)

for path in "${required_files[@]}"; do
  [[ -f "$path" ]] || {
    echo "error: required FFmpeg build output is missing: $path" >&2
    exit 1
  }
done

for macro in \
  CONFIG_H264_DECODER \
  CONFIG_HEVC_DECODER \
  CONFIG_VP9_DECODER \
  CONFIG_AV1_DECODER \
  CONFIG_MOV_DEMUXER \
  CONFIG_MATROSKA_DEMUXER; do
  if ! grep -Eq "^#define[[:space:]]+$macro[[:space:]]+1$" "$COMPONENTS"; then
    echo "error: FFmpeg build does not enable expected component $macro" >&2
    exit 1
  fi
done

if grep -Eq '^#define[[:space:]]+CONFIG_(LIBX264|LIBX265|LIBAOM|LIBDAV1D)[[:space:]]+1$' "$COMPONENTS"; then
  echo "error: unexpected external codec library was enabled by autodetection" >&2
  exit 1
fi

grep -qx 'FFMPEG_LICENSE_MODE=GPLv3-or-later' "$MARKER"
grep -qx 'ANDROID_ABI=arm64-v8a' "$MARKER"
grep -qx 'ANDROID_API=26' "$MARKER"

echo "Verified FFmpeg Android headers, static libraries, codec configuration, and build metadata."
