#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_SCRIPT="$ROOT/scripts/build-ffmpeg-android.sh"
FFMPEG_VERSION="9.0.1"
FFMPEG_SHA256="cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635"
NDK_VERSION="27.3.13750724"
ABI="arm64-v8a"
PREFIX="${FRAMESCOPE_FFMPEG_ROOT:-$ROOT/.native/ffmpeg/$ABI}"
MARKER="$PREFIX/share/framescope/build-info.env"
COMPONENTS="$PREFIX/share/framescope/config_components.h"

command -v sha256sum >/dev/null || { echo "error: sha256sum is required" >&2; exit 1; }
BUILD_SCRIPT_SHA256="$(sha256sum "$BUILD_SCRIPT" | awk '{print $1}')"

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

grep -qx "FFMPEG_VERSION=$FFMPEG_VERSION" "$MARKER"
grep -qx "FFMPEG_SOURCE_SHA256=$FFMPEG_SHA256" "$MARKER"
grep -qx "FFMPEG_BUILD_SCRIPT_SHA256=$BUILD_SCRIPT_SHA256" "$MARKER"
grep -qx "NDK_VERSION=$NDK_VERSION" "$MARKER"
grep -qx 'FFMPEG_LICENSE_MODE=GPLv3-or-later' "$MARKER"
grep -qx 'ANDROID_ABI=arm64-v8a' "$MARKER"
grep -qx 'ANDROID_API=26' "$MARKER"
grep -qx 'FFMPEG_LIBRARIES=avcodec,avformat,avutil,swscale' "$MARKER"
grep -qx 'FFMPEG_DECODERS=h264,hevc,vp9,av1' "$MARKER"
grep -qx 'FFMPEG_DEMUXERS=mov,matroska,avi' "$MARKER"

echo "Verified pinned FFmpeg Android source provenance, headers, static libraries, codec configuration, and build metadata."
