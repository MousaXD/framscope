#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${1:-$ROOT/build/video-fixtures}"

command -v ffmpeg >/dev/null || { echo "error: ffmpeg is required" >&2; exit 1; }
command -v ffprobe >/dev/null || { echo "error: ffprobe is required" >&2; exit 1; }

required_encoders=(libx264 libx265 libvpx-vp9 libaom-av1 aac)
encoders="$(ffmpeg -hide_banner -encoders 2>/dev/null)"
for encoder in "${required_encoders[@]}"; do
  if ! grep -Eq "[[:space:]]${encoder}[[:space:]]" <<<"$encoders"; then
    echo "error: ffmpeg encoder '$encoder' is required to generate the Phase 2 fixture set" >&2
    exit 1
  fi
done

rm -rf "$OUT"
mkdir -p "$OUT"

common=(-hide_banner -loglevel error -y -fflags +bitexact -flags:v +bitexact)
x264=(-c:v libx264 -preset veryfast -crf 28 -pix_fmt yuv420p -threads 1 -x264-params "threads=1:lookahead_threads=1:sliced_threads=0")

ffmpeg "${common[@]}" -f lavfi -i "testsrc2=size=64x48:rate=12:duration=1" \
  "${x264[@]}" -an -movflags +faststart "$OUT/h264-cfr.mp4"

# Concatenate two synthetic segments with different source rates. The concat filter
# preserves their timestamps, giving us a stable VFR sample on FFmpeg 6.x and 7.x.
ffmpeg "${common[@]}" \
  -f lavfi -i "testsrc2=size=64x48:rate=4:duration=0.5" \
  -f lavfi -i "testsrc2=size=64x48:rate=12:duration=0.5" \
  -filter_complex "[0:v][1:v]concat=n=2:v=1:a=0[v]" -map "[v]" \
  "${x264[@]}" -fps_mode vfr -an -movflags +faststart "$OUT/h264-vfr.mp4"

ffmpeg "${common[@]}" -f lavfi -i "testsrc2=size=64x48:rate=12:duration=0.5" \
  -c:v libx265 -preset ultrafast -crf 32 -pix_fmt yuv420p -threads 1 \
  -x265-params "log-level=error:pools=1:frame-threads=1" -an -movflags +faststart "$OUT/hevc-cfr.mp4"

ffmpeg "${common[@]}" -f lavfi -i "testsrc2=size=64x48:rate=12:duration=0.5" \
  -c:v libvpx-vp9 -crf 38 -b:v 0 -deadline good -cpu-used 8 -row-mt 0 -threads 1 -an "$OUT/vp9-cfr.webm"

ffmpeg "${common[@]}" -f lavfi -i "testsrc2=size=64x48:rate=8:duration=0.5" \
  -c:v libaom-av1 -crf 45 -b:v 0 -cpu-used 8 -row-mt 0 -threads 1 -an "$OUT/av1-cfr.mkv"

ffmpeg "${common[@]}" -f lavfi -i "testsrc2=size=64x48:rate=12:duration=0.5" \
  "${x264[@]}" -an "$OUT/rotation-base.mp4"
ffmpeg -hide_banner -loglevel error -y -display_rotation:v:0 90 -i "$OUT/rotation-base.mp4" -map 0 -c copy \
  -movflags +faststart "$OUT/rotated-portrait.mp4"
rm "$OUT/rotation-base.mp4"

ffmpeg "${common[@]}" \
  -f lavfi -i "testsrc2=size=64x48:rate=12:duration=0.5" \
  -f lavfi -i "sine=frequency=880:sample_rate=48000:duration=0.5" \
  -map 0:v:0 -map 1:a:0 "${x264[@]}" -c:a aac -b:a 48k -ar 48000 -ac 1 \
  -shortest -movflags +faststart "$OUT/h264-with-audio.mp4"

ffmpeg "${common[@]}" \
  -f lavfi -i "sine=frequency=660:sample_rate=48000:duration=0.5" \
  -c:a aac -b:a 48k -ar 48000 -ac 1 -vn -movflags +faststart "$OUT/audio-only.m4a"

ffmpeg "${common[@]}" \
  -f lavfi -i "testsrc2=size=64x48:rate=8:duration=0.5" \
  -f lavfi -i "color=c=black:size=32x24:rate=8:duration=0.5" \
  -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=0.5" \
  -map 0:v:0 -map 1:v:0 -map 2:a:0 \
  -c:v libx264 -preset veryfast -crf 30 -pix_fmt yuv420p -threads 1 \
  -x264-params "threads=1:lookahead_threads=1:sliced_threads=0" \
  -c:a aac -b:a 48k -ar 48000 -ac 1 -shortest "$OUT/multi-stream.mkv"

ffmpeg "${common[@]}" -f lavfi -i "testsrc2=size=62x46:rate=8:duration=0.5" \
  "${x264[@]}" -an -movflags +faststart "$OUT/unusual-dimensions.mp4"

ffmpeg "${common[@]}" -f lavfi -i "testsrc2=size=64x48:rate=8" -frames:v 1 \
  "${x264[@]}" -an -movflags +faststart "$OUT/very-short.mp4"

# Adversarial fixtures stay deterministic so failures are reproducible across CI runs.
# Keep only the opening MP4 bytes so probing must reject the header-only container.
head -c 12 "$OUT/h264-cfr.mp4" > "$OUT/truncated.mp4"

# Build a fast-start MP4 whose metadata remains intact while the media-data box itself is cut.
# Parsing top-level boxes avoids a fragile percentage-of-file truncation that could remove only
# trailing metadata/free space while leaving every compressed frame readable.
python3 - "$OUT/h264-cfr.mp4" "$OUT/truncated-payload.mp4" <<'PY'
from pathlib import Path
import struct
import sys

source = Path(sys.argv[1]).read_bytes()
out_path = Path(sys.argv[2])
position = 0
moov_position = None
mdat = None

while position + 8 <= len(source):
    size32, box_type = struct.unpack_from(">I4s", source, position)
    header = 8
    if size32 == 1:
        if position + 16 > len(source):
            raise SystemExit("invalid extended MP4 box header")
        size = struct.unpack_from(">Q", source, position + 8)[0]
        header = 16
    elif size32 == 0:
        size = len(source) - position
    else:
        size = size32
    if size < header or position + size > len(source):
        raise SystemExit("invalid top-level MP4 box size")
    if box_type == b"moov":
        moov_position = position
    if box_type == b"mdat":
        mdat = (position, header, size)
        break
    position += size

if moov_position is None or mdat is None:
    raise SystemExit("expected fast-start MP4 with moov and mdat boxes")
mdat_position, mdat_header, mdat_size = mdat
if moov_position > mdat_position:
    raise SystemExit("fixture is not fast-start: moov follows mdat")
payload_start = mdat_position + mdat_header
payload_size = mdat_size - mdat_header
if payload_size < 8:
    raise SystemExit("mdat payload is unexpectedly small")
# Keep only one eighth of compressed media bytes. The original mdat size remains in its header,
# making the file structurally truncated while retaining the complete leading moov metadata.
keep_payload = max(1, payload_size // 8)
out_path.write_bytes(source[: payload_start + keep_payload])
PY

# Zero bytes and deterministic non-container bytes exercise format probing without nondeterminism.
: > "$OUT/empty.bin"
printf 'FrameScope deterministic hostile input\x00\xff\x7fnot-a-container\n' > "$OUT/garbage.bin"

printf 'Generated fixtures in %s\n' "$OUT"
