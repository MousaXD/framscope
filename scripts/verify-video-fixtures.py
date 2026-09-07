#!/usr/bin/env python3
from __future__ import annotations

import json
import math
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "fixtures" / "video" / "manifest.json"
FIXTURE_DIR = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / "build" / "video-fixtures"


def run_ffprobe(path: Path, *, frames: bool = False) -> subprocess.CompletedProcess[str]:
    cmd = ["ffprobe", "-v", "error", "-print_format", "json"]
    if frames:
        cmd += ["-select_streams", "v:0", "-show_entries", "frame=best_effort_timestamp_time", "-show_frames"]
    else:
        cmd += ["-count_frames", "-show_streams", "-show_format"]
    cmd.append(str(path))
    return subprocess.run(cmd, check=False, text=True, capture_output=True)


def parse_duration(info: dict) -> float | None:
    raw = info.get("format", {}).get("duration")
    if raw not in (None, "N/A"):
        return float(raw)
    durations = [s.get("duration") for s in info.get("streams", [])]
    numeric = [float(x) for x in durations if x not in (None, "N/A")]
    return max(numeric) if numeric else None


def frame_count(stream: dict) -> int | None:
    for key in ("nb_read_frames", "nb_frames"):
        value = stream.get(key)
        if value not in (None, "N/A"):
            return int(value)
    return None


def normalized_rotation(stream: dict) -> int | None:
    tags = stream.get("tags") or {}
    if "rotate" in tags:
        return int(round(float(tags["rotate"]))) % 360
    for side_data in stream.get("side_data_list") or []:
        if "rotation" in side_data:
            return int(round(float(side_data["rotation"]))) % 360
    return None


def has_variable_timestamps(path: Path) -> bool:
    result = run_ffprobe(path, frames=True)
    if result.returncode != 0:
        raise AssertionError(f"ffprobe frame timestamp read failed for {path.name}: {result.stderr.strip()}")
    frames = json.loads(result.stdout).get("frames", [])
    pts = [float(frame["best_effort_timestamp_time"]) for frame in frames if "best_effort_timestamp_time" in frame]
    deltas = [round(b - a, 6) for a, b in zip(pts, pts[1:]) if b > a]
    return bool(deltas) and (max(deltas) - min(deltas)) > 0.01


def verify_success(entry: dict, path: Path, info: dict) -> None:
    streams = info.get("streams", [])
    video_streams = [s for s in streams if s.get("codec_type") == "video"]
    audio_streams = [s for s in streams if s.get("codec_type") == "audio"]
    assert video_streams, f"{path.name}: expected at least one video stream"
    primary = video_streams[0]

    assert len(streams) == entry["stream_count"], f"{path.name}: stream_count={len(streams)}"
    assert len(audio_streams) == entry.get("audio_stream_count", 0), f"{path.name}: audio stream count mismatch"
    if "video_stream_count" in entry:
        assert len(video_streams) == entry["video_stream_count"], f"{path.name}: video stream count mismatch"
    assert primary.get("codec_name") == entry["video_codec"], f"{path.name}: codec={primary.get('codec_name')}"
    assert int(primary["width"]) == entry["width"], f"{path.name}: width={primary.get('width')}"
    assert int(primary["height"]) == entry["height"], f"{path.name}: height={primary.get('height')}"

    if "audio_codec" in entry:
        assert audio_streams and audio_streams[0].get("codec_name") == entry["audio_codec"], f"{path.name}: audio codec mismatch"

    count = frame_count(primary)
    if count is not None:
        assert count == entry["frame_count"], f"{path.name}: frame_count={count}, expected {entry['frame_count']}"

    duration = parse_duration(info)
    assert duration is not None, f"{path.name}: duration unavailable"
    expected_duration = entry["duration_seconds"]
    tolerance = entry["duration_tolerance_seconds"]
    assert math.isclose(duration, expected_duration, abs_tol=tolerance), (
        f"{path.name}: duration={duration:.6f}, expected {expected_duration}±{tolerance}"
    )

    if "rotation_degrees" in entry:
        rotation = normalized_rotation(primary)
        assert rotation == entry["rotation_degrees"] % 360, f"{path.name}: rotation={rotation}"

    variable = has_variable_timestamps(path)
    assert variable == entry["variable_frame_rate"], f"{path.name}: variable_frame_rate={variable}"


def main() -> int:
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    errors: list[str] = []

    for entry in manifest["fixtures"]:
        path = FIXTURE_DIR / entry["file"]
        if not path.is_file():
            errors.append(f"{entry['file']}: missing fixture")
            continue

        result = run_ffprobe(path)
        if entry["expect_probe"] == "failure":
            if result.returncode == 0:
                errors.append(f"{entry['file']}: expected ffprobe failure but probe succeeded")
            continue

        if result.returncode != 0:
            errors.append(f"{entry['file']}: ffprobe failed: {result.stderr.strip()}")
            continue

        try:
            verify_success(entry, path, json.loads(result.stdout))
        except (AssertionError, KeyError, TypeError, ValueError) as exc:
            errors.append(str(exc))

    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1

    print(f"Verified {len(manifest['fixtures'])} video fixture contracts in {FIXTURE_DIR}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
