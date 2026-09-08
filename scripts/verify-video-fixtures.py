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
EXPECTED_PROBE_MODES = {"success", "failure", "no_video", "damaged_video"}


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


def stream_sets(info: dict) -> tuple[list[dict], list[dict], list[dict]]:
    streams = info.get("streams", [])
    video_streams = [s for s in streams if s.get("codec_type") == "video"]
    audio_streams = [s for s in streams if s.get("codec_type") == "audio"]
    return streams, video_streams, audio_streams


def verify_success(entry: dict, path: Path, info: dict) -> None:
    streams, video_streams, audio_streams = stream_sets(info)
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


def verify_no_video(entry: dict, path: Path, info: dict) -> None:
    streams, video_streams, audio_streams = stream_sets(info)
    assert not video_streams, f"{path.name}: adversarial no-video fixture unexpectedly contains video"
    assert len(streams) == entry["stream_count"], f"{path.name}: stream_count={len(streams)}"
    assert len(audio_streams) == entry["audio_stream_count"], f"{path.name}: audio stream count mismatch"
    if "audio_codec" in entry:
        assert audio_streams and audio_streams[0].get("codec_name") == entry["audio_codec"], f"{path.name}: audio codec mismatch"


def verify_damaged_video(entry: dict, path: Path, result: subprocess.CompletedProcess[str]) -> None:
    baseline = FIXTURE_DIR / "h264-cfr.mp4"
    assert baseline.is_file(), f"{path.name}: healthy baseline fixture is missing"
    assert path.stat().st_size < baseline.stat().st_size, f"{path.name}: damaged fixture is not smaller than baseline"

    # Container probing may fail immediately, which is an acceptable damaged-media outcome.
    if result.returncode != 0:
        return
    info = json.loads(result.stdout)
    _, video_streams, _ = stream_sets(info)
    assert video_streams, f"{path.name}: probe succeeded but reported no video stream"

    # `nb_frames` can come from intact fast-start metadata and therefore is not evidence that the
    # damaged payload is readable. Force frame decoding and count only frames FFprobe can actually
    # enumerate. A decoder error is acceptable; a clean full healthy sequence is not.
    frames_result = run_ffprobe(path, frames=True)
    if frames_result.returncode != 0:
        return
    decoded_frames = json.loads(frames_result.stdout).get("frames", [])
    healthy_count = int(entry["healthy_frame_count"])
    assert len(decoded_frames) < healthy_count, (
        f"{path.name}: damaged payload decoded the complete {healthy_count}-frame healthy sequence"
    )


def main() -> int:
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    errors: list[str] = []

    if manifest.get("schema_version") != 2:
        errors.append(f"manifest schema_version={manifest.get('schema_version')!r}, expected 2")

    for entry in manifest.get("fixtures", []):
        path = FIXTURE_DIR / entry["file"]
        mode = entry.get("expect_probe")
        if mode not in EXPECTED_PROBE_MODES:
            errors.append(f"{entry['file']}: unsupported expect_probe={mode!r}")
            continue
        if not path.is_file():
            errors.append(f"{entry['file']}: missing fixture")
            continue

        result = run_ffprobe(path)
        try:
            if mode == "failure":
                if result.returncode == 0:
                    raise AssertionError(f"{entry['file']}: expected ffprobe failure but probe succeeded")
            elif mode == "damaged_video":
                verify_damaged_video(entry, path, result)
            else:
                if result.returncode != 0:
                    raise AssertionError(f"{entry['file']}: ffprobe failed: {result.stderr.strip()}")
                info = json.loads(result.stdout)
                if mode == "success":
                    verify_success(entry, path, info)
                else:
                    verify_no_video(entry, path, info)
        except (AssertionError, KeyError, TypeError, ValueError, json.JSONDecodeError) as exc:
            errors.append(str(exc))

    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1

    print(f"Verified {len(manifest['fixtures'])} video fixture contracts in {FIXTURE_DIR}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
