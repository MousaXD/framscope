#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FFMPEG_VERSION="9.0.1"
FFMPEG_SHA256="cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635"
FFMPEG_URL="https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz"
ABI="arm64-v8a"
ARCH="aarch64"
ANDROID_API="26"
NDK_VERSION="27.3.13750724"
PREFIX="${FRAMESCOPE_FFMPEG_ROOT:-$ROOT/.native/ffmpeg/$ABI}"
DOWNLOAD_DIR="$ROOT/.native/downloads"
WORK_DIR="$ROOT/.native/work/ffmpeg-${FFMPEG_VERSION}-${ABI}"
TARBALL="$DOWNLOAD_DIR/ffmpeg-${FFMPEG_VERSION}.tar.xz"
MARKER="$PREFIX/share/framescope/build-info.env"
CONFIG_COMPONENTS="$PREFIX/share/framescope/config_components.h"
JOBS="${FRAMESCOPE_FFMPEG_JOBS:-}"

for tool in curl sha256sum tar make; do
  command -v "$tool" >/dev/null || {
    echo "error: $tool is required to build FFmpeg" >&2
    exit 1
  }
done

if [[ -z "$JOBS" ]]; then
  if command -v nproc >/dev/null 2>&1; then
    JOBS="$(nproc)"
  else
    JOBS="4"
  fi
fi

if [[ -f "$MARKER" ]] \
  && grep -qx "FFMPEG_VERSION=$FFMPEG_VERSION" "$MARKER" \
  && grep -qx "FFMPEG_SOURCE_SHA256=$FFMPEG_SHA256" "$MARKER" \
  && grep -qx "ANDROID_ABI=$ABI" "$MARKER" \
  && grep -qx "ANDROID_API=$ANDROID_API" "$MARKER" \
  && grep -qx "NDK_VERSION=$NDK_VERSION" "$MARKER" \
  && [[ -f "$PREFIX/include/libavcodec/avcodec.h" ]] \
  && [[ -f "$PREFIX/lib/libavcodec.a" ]] \
  && [[ -f "$PREFIX/lib/libavformat.a" ]] \
  && [[ -f "$PREFIX/lib/libavutil.a" ]] \
  && [[ -f "$PREFIX/lib/libswscale.a" ]]; then
  FRAMESCOPE_FFMPEG_ROOT="$PREFIX" "$ROOT/scripts/verify-ffmpeg-android.sh"
  echo "Reusing verified FFmpeg $FFMPEG_VERSION Android prefix: $PREFIX"
  exit 0
fi

NDK_ROOT="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
if [[ -z "$NDK_ROOT" && -n "${ANDROID_HOME:-}" ]]; then
  NDK_ROOT="$ANDROID_HOME/ndk/$NDK_VERSION"
fi
if [[ -z "$NDK_ROOT" && -n "${ANDROID_SDK_ROOT:-}" ]]; then
  NDK_ROOT="$ANDROID_SDK_ROOT/ndk/$NDK_VERSION"
fi
if [[ -z "$NDK_ROOT" || ! -d "$NDK_ROOT" ]]; then
  echo "error: Android NDK r27d ($NDK_VERSION) was not found." >&2
  echo "Set ANDROID_NDK_HOME, or install ndk;$NDK_VERSION below ANDROID_HOME." >&2
  exit 1
fi

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) HOST_TAG="linux-x86_64" ;;
  Darwin-x86_64|Darwin-arm64) HOST_TAG="darwin-x86_64" ;;
  *)
    echo "error: unsupported FFmpeg build host $(uname -s)-$(uname -m); use Linux x86_64 CI or a supported macOS host" >&2
    exit 1
    ;;
esac

TOOLCHAIN="$NDK_ROOT/toolchains/llvm/prebuilt/$HOST_TAG"
SYSROOT="$TOOLCHAIN/sysroot"
BIN="$TOOLCHAIN/bin"
CC="$BIN/aarch64-linux-android${ANDROID_API}-clang"
CXX="$BIN/aarch64-linux-android${ANDROID_API}-clang++"
AR="$BIN/llvm-ar"
NM="$BIN/llvm-nm"
RANLIB="$BIN/llvm-ranlib"
STRIP="$BIN/llvm-strip"

for tool_path in "$CC" "$CXX" "$AR" "$NM" "$RANLIB" "$STRIP"; do
  if [[ ! -x "$tool_path" ]]; then
    echo "error: expected NDK tool is missing: $tool_path" >&2
    exit 1
  fi
done

mkdir -p "$DOWNLOAD_DIR"
if [[ ! -f "$TARBALL" ]]; then
  echo "Downloading FFmpeg $FFMPEG_VERSION from ffmpeg.org"
  curl --fail --location --retry 3 --retry-delay 2 "$FFMPEG_URL" --output "$TARBALL"
fi

echo "$FFMPEG_SHA256  $TARBALL" | sha256sum -c -

rm -rf "$WORK_DIR" "$PREFIX"
mkdir -p "$WORK_DIR/source" "$WORK_DIR/build" "$PREFIX"
tar -xJf "$TARBALL" --strip-components=1 -C "$WORK_DIR/source"

CONFIGURE_ARGS=(
  "--prefix=$PREFIX"
  "--target-os=android"
  "--arch=$ARCH"
  "--enable-cross-compile"
  "--sysroot=$SYSROOT"
  "--cc=$CC"
  "--cxx=$CXX"
  "--ar=$AR"
  "--nm=$NM"
  "--ranlib=$RANLIB"
  "--strip=$STRIP"
  "--enable-pic"
  "--enable-static"
  "--disable-shared"
  "--disable-programs"
  "--disable-doc"
  "--disable-debug"
  "--disable-network"
  "--disable-autodetect"
  "--disable-avdevice"
  "--disable-avfilter"
  "--disable-swresample"
  "--disable-everything"
  "--enable-decoder=h264,hevc,vp9,av1"
  "--enable-demuxer=mov,matroska,avi"
  "--enable-parser=h264,hevc,vp9,av1"
  "--enable-protocol=file,pipe"
  "--disable-zlib"
  "--disable-bzlib"
  "--disable-lzma"
  "--disable-iconv"
  "--enable-gpl"
  "--enable-version3"
  "--extra-cflags=-fPIC"
  "--extra-ldflags=-Wl,-z,max-page-size=16384"
)

pushd "$WORK_DIR/build" >/dev/null
"$WORK_DIR/source/configure" "${CONFIGURE_ARGS[@]}"
make -j"$JOBS"
make install
popd >/dev/null

mkdir -p "$PREFIX/share/framescope"
cp "$WORK_DIR/build/config_components.h" "$CONFIG_COMPONENTS"
{
  echo "FFMPEG_VERSION=$FFMPEG_VERSION"
  echo "FFMPEG_SOURCE_URL=$FFMPEG_URL"
  echo "FFMPEG_SOURCE_SHA256=$FFMPEG_SHA256"
  echo "ANDROID_ABI=$ABI"
  echo "ANDROID_ARCH=$ARCH"
  echo "ANDROID_API=$ANDROID_API"
  echo "NDK_VERSION=$NDK_VERSION"
  echo "FFMPEG_LICENSE_MODE=GPLv3-or-later"
  echo "FFMPEG_LIBRARIES=avcodec,avformat,avutil,swscale"
  echo "FFMPEG_DECODERS=h264,hevc,vp9,av1"
  echo "FFMPEG_DEMUXERS=mov,matroska,avi"
  echo "FFMPEG_PARSERS=h264,hevc,vp9,av1"
  echo "FFMPEG_PROTOCOLS=file,pipe"
} > "$MARKER"

if [[ "${FRAMESCOPE_FFMPEG_KEEP_WORK:-0}" != "1" ]]; then
  rm -rf "$WORK_DIR"
fi

FRAMESCOPE_FFMPEG_ROOT="$PREFIX" "$ROOT/scripts/verify-ffmpeg-android.sh"
echo "FFmpeg Android prefix ready: $PREFIX"
