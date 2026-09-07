#!/usr/bin/env python3
from __future__ import annotations

import argparse
import bisect
import json
import subprocess
from pathlib import Path
from typing import Any


def parse_ratio(raw: str) -> tuple[int, int]:
    left, right = raw.split("/", 1)
    num, den = int(left), int(right)
    if den == 0:
        raise ValueError(f"invalid zero time-base denominator: {raw}")
    return num, den


def ffprobe(path: Path) -> dict[str, Any]:
    cmd = [
        "ffprobe",
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=index,time_base,nb_frames,duration_ts,has_b_frames",
        "-show_entries",
        "frame=best_effort_timestamp,duration,key_frame,pict_type",
        "-show_streams",
        "-show_frames",
        "-of",
        "json",
        str(path),
    ]
    result = subprocess.run(cmd, check=False, text=True, capture_output=True)
    if result.returncode != 0:
        raise RuntimeError(f"ffprobe failed for {path.name}: {result.stderr.strip()}")
    return json.loads(result.stdout)


def micros(ticks: int, num: int, den: int) -> int:
    numerator = ticks * num * 1_000_000
    if numerator >= 0:
        return (numerator + den // 2) // den
    return -((-numerator + den // 2) // den)


def variable_pts(pts: list[int]) -> bool:
    deltas = [b - a for a, b in zip(pts, pts[1:])]
    return len(set(deltas)) > 1


def nearest_index(pts: list[int], target: int) -> int | None:
    if not pts:
        return None
    pos = bisect.bisect_left(pts, target)
    if pos == 0:
        return 0
    if pos == len(pts):
        return len(pts) - 1
    before, after = pts[pos - 1], pts[pos]
    return pos - 1 if target - before <= after - target else pos


def lookup_cases(pts: list[int]) -> list[dict[str, int | None]]:
    if not pts:
        return []
    targets = {pts[0], pts[-1], pts[0] - 1, pts[-1] + 1}
    for left, right in zip(pts, pts[1:]):
        targets.add(left + (right - left) // 2)
    cases = []
    for target in sorted(targets):
        after = bisect.bisect_left(pts, target)
        before = bisect.bisect_right(pts, target) - 1
        cases.append(
            {
                "target_ticks": target,
                "at_or_before_frame": before if before >= 0 else None,
                "at_or_after_frame": after if after < len(pts) else None,
                "nearest_frame": nearest_index(pts, target),
            }
        )
    return cases


def capture_fixture(path: Path) -> dict[str, Any]:
    info = ffprobe(path)
    streams = info.get("streams", [])
    if len(streams) != 1:
        raise AssertionError(f"{path.name}: expected exactly one selected v:0 stream")
    stream = streams[0]
    num, den = parse_ratio(stream["time_base"])
    raw_frames = info.get("frames", [])
    frames = []
    last_keyframe: int | None = None
    pts: list[int] = []
    for index, frame in enumerate(raw_frames):
        if "best_effort_timestamp" not in frame:
            raise AssertionError(f"{path.name}: frame {index} has no presentation timestamp")
        timestamp = int(frame["best_effort_timestamp"])
        if pts and timestamp < pts[-1]:
            raise AssertionError(f"{path.name}: presentation timestamps regressed at frame {index}")
        pts.append(timestamp)
        is_key = bool(frame.get("key_frame", 0))
        if is_key:
            last_keyframe = index
        frames.append(
            {
                "frame_index": index,
                "timestamp_ticks": timestamp,
                "timestamp_us": micros(timestamp, num, den),
                "duration_ticks": int(frame["duration"])
                if frame.get("duration") not in (None, "N/A")
                else None,
                "keyframe": is_key,
                "pict_type": frame.get("pict_type"),
                "nearest_keyframe_frame": last_keyframe,
            }
        )
    return {
        "file": path.name,
        "selected_video_stream_index": int(stream["index"]),
        "time_base_num": num,
        "time_base_den": den,
        "duration_ticks": int(stream["duration_ts"])
        if stream.get("duration_ts") not in (None, "N/A")
        else None,
        "has_b_frames": int(stream.get("has_b_frames", 0)) > 0,
        "frame_count": len(frames),
        "variable_frame_rate": variable_pts(pts),
        "keyframe_indices": [frame["frame_index"] for frame in frames if frame["keyframe"]],
        "frames": frames,
        "lookup_cases": lookup_cases(pts),
    }


def validate_fixture(contract: dict[str, Any], truth: dict[str, Any]) -> None:
    name = contract["file"]
    assert truth["frame_count"] == contract["frame_count"], f"{name}: frame count mismatch"
    assert truth["keyframe_indices"] == contract["keyframe_indices"], (
        f"{name}: keyframe positions mismatch"
    )
    assert truth["variable_frame_rate"] == contract["variable_frame_rate"], f"{name}: VFR mismatch"
    if contract.get("requires_b_frames"):
        assert truth["has_b_frames"], f"{name}: expected B frames"
    pts = [frame["timestamp_ticks"] for frame in truth["frames"]]
    assert pts == sorted(pts), f"{name}: PTS not presentation-ordered"
    for frame in truth["frames"]:
        anchor = frame["nearest_keyframe_frame"]
        assert anchor is not None and anchor <= frame["frame_index"], f"{name}: invalid keyframe anchor"
        assert truth["frames"][anchor]["keyframe"], f"{name}: anchor is not a keyframe"


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Capture and validate Phase 3 exact frame truth with ffprobe"
    )
    parser.add_argument("fixture_dir", type=Path)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--stress-seconds", type=int, default=45)
    args = parser.parse_args()

    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    captured = []
    for contract in manifest["fixtures"]:
        truth = capture_fixture(args.fixture_dir / contract["file"])
        validate_fixture(contract, truth)
        captured.append(truth)

    stress_contract = manifest["stress"]
    stress = capture_fixture(args.fixture_dir / stress_contract["file"])
    expected_stress_frames = stress_contract["rate"] * args.stress_seconds
    assert stress["frame_count"] == expected_stress_frames, (
        f"{stress['file']}: frame_count={stress['frame_count']}, expected={expected_stress_frames}"
    )

    payload = {
        "schema_version": 1,
        "oracle": "ffprobe best_effort_timestamp in presentation order",
        "timestamp_selection": manifest["timestamp_selection"],
        "fixtures": captured,
        "stress": {
            "file": stress["file"],
            "frame_count": stress["frame_count"],
            "file_bytes": (args.fixture_dir / stress["file"]).stat().st_size,
            "keyframe_count": len(stress["keyframe_indices"]),
        },
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )

    vfr = next(item for item in captured if item["file"] == "h264-vfr-transitions.mp4")
    long_gop = next(
        item for item in captured if item["file"] == "h264-long-gop-bframes.mp4"
    )
    print(
        "phase3 truth: "
        f"fixtures={len(captured)} "
        f"stress_frames={stress['frame_count']} "
        f"stress_bytes={payload['stress']['file_bytes']}"
    )
    print(f"phase3 VFR PTS ticks: {[frame['timestamp_ticks'] for frame in vfr['frames']]}")
    print(f"phase3 long-GOP keyframes: {long_gop['keyframe_indices']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
