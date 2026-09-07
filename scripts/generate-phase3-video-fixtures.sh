#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${1:-$ROOT/build/phase3-video-fixtures}"
STRESS_SECONDS="${PHASE3_STRESS_SECONDS:-45}"

command -v ffmpeg >/dev/null || { echo "error: ffmpeg is required" >&2; exit 1; }
command -v ffprobe >/dev/null || { echo "error: ffprobe is required" >&2; exit 1; }

encoders="$(ffmpeg -hide_banner -encoders 2>/dev/null)"
if ! grep -Eq '[[:space:]]libx264[[:space:]]' <<<"$encoders"; then
  echo "error: ffmpeg encoder 'libx264' is required" >&2
  exit 1
fi

rm -rf "$OUT"
mkdir -p "$OUT"

common=(-hide_banner -loglevel error -y -fflags +bitexact -flags:v +bitexact)
x264=(-c:v libx264 -preset veryfast -crf 30 -pix_fmt yuv420p -threads 1 \
  -x264-params "threads=1:lookahead_threads=1:sliced_threads=0:scenecut=0")

# Keyframe every three presentation frames. This makes anchor expectations obvious.
ffmpeg "${common[@]}" -f lavfi -i "testsrc2=size=64x48:rate=12:duration=2" \
  "${x264[@]}" -g 3 -keyint_min 3 -bf 0 -an -movflags +faststart \
  "$OUT/h264-dense-keyframes.mp4"

# Two GOPs with B-frame reordering. Presentation order must remain monotonic even
# though decode/packet order differs around B frames.
ffmpeg "${common[@]}" -f lavfi -i "testsrc2=size=64x48:rate=12:duration=3" \
  "${x264[@]}" -g 24 -keyint_min 24 -bf 3 -an -movflags +faststart \
  "$OUT/h264-long-gop-bframes.mp4"

# Three rate regimes: 4 fps -> 12 fps -> 6 fps. concat preserves source PTS and
# fps_mode=vfr keeps the non-uniform presentation timeline.
ffmpeg "${common[@]}" \
  -f lavfi -i "testsrc2=size=64x48:rate=4:duration=0.5" \
  -f lavfi -i "testsrc2=size=64x48:rate=12:duration=0.5" \
  -f lavfi -i "testsrc2=size=64x48:rate=6:duration=0.5" \
  -filter_complex "[0:v][1:v][2:v]concat=n=3:v=1:a=0[v]" -map "[v]" \
  "${x264[@]}" -g 120 -keyint_min 120 -bf 2 -fps_mode vfr -an -movflags +faststart \
  "$OUT/h264-vfr-transitions.mp4"

# Generated only in CI/build output. It is deliberately low-resolution and low bitrate,
# so frame count is large enough to expose whole-video accumulation without a giant artifact.
ffmpeg "${common[@]}" -f lavfi \
  -i "testsrc2=size=64x48:rate=24:duration=${STRESS_SECONDS}" \
  -c:v libx264 -preset ultrafast -crf 40 -pix_fmt yuv420p -threads 1 \
  -x264-params "threads=1:lookahead_threads=1:sliced_threads=0:scenecut=0:keyint=48:min-keyint=48" \
  -g 48 -keyint_min 48 -bf 0 -an -movflags +faststart \
  "$OUT/h264-stress-long.mp4"

printf 'Generated Phase 3 fixtures in %s (stress=%ss)\n' "$OUT" "$STRESS_SECONDS"
