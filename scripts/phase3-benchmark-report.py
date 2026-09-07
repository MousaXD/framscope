#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

REQUIRED_MEASUREMENTS = (
    "frame_number_lookup",
    "timestamp_lookup",
    "random_seek_setup",
    "cold_access",
    "warm_access",
)


def validate(metrics: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    if metrics.get("schema_version") != 1:
        errors.append("schema_version must be 1")

    index = metrics.get("index") or {}
    frames = int(index.get("frames", 0))
    if frames <= 0:
        errors.append("index.frames must be > 0")
    if int(index.get("persisted_batches", 0)) < 2 and frames > 1:
        errors.append("index.persisted_batches must prove incremental persistence")

    live = int(index.get("max_live_decoded_frames", 0))
    live_limit = int(index.get("live_frame_limit", -1))
    if live_limit < 0 or live > live_limit:
        errors.append("max_live_decoded_frames exceeds configured live_frame_limit")

    buffered = int(index.get("peak_buffered_metadata_entries", 0))
    batch_limit = int(index.get("metadata_batch_limit", -1))
    if batch_limit < 0 or buffered > batch_limit:
        errors.append("peak_buffered_metadata_entries exceeds metadata_batch_limit")

    for name in REQUIRED_MEASUREMENTS:
        sample = (metrics.get("measurements") or {}).get(name) or {}
        if int(sample.get("operations", 0)) <= 0:
            errors.append(f"{name}.operations must be > 0")
        if int(sample.get("elapsed_ns", 0)) <= 0:
            errors.append(f"{name}.elapsed_ns must be > 0")

    return errors


def report(metrics: dict[str, Any]) -> None:
    index = metrics["index"]
    frames = int(index["frames"])
    elapsed_ns = int(index.get("elapsed_ns", 0))
    db_bytes = int(index.get("db_bytes", 0))
    if elapsed_ns > 0:
        fps = frames * 1_000_000_000 / elapsed_ns
        print(f"phase3 benchmark index_throughput_frames_per_s={fps:.2f}")
    if db_bytes > 0:
        print(
            f"phase3 benchmark index_db_bytes={db_bytes} "
            f"bytes_per_frame={db_bytes / frames:.2f}"
        )
    print(
        "phase3 benchmark streaming "
        f"frames={frames} batches={index['persisted_batches']} "
        f"max_live={index['max_live_decoded_frames']}/{index['live_frame_limit']} "
        f"metadata_peak={index['peak_buffered_metadata_entries']}/"
        f"{index['metadata_batch_limit']}"
    )

    for name in REQUIRED_MEASUREMENTS:
        sample = metrics["measurements"][name]
        operations = int(sample["operations"])
        elapsed_ns = int(sample["elapsed_ns"])
        print(
            f"phase3 benchmark {name}_ns_per_op={elapsed_ns / operations:.2f} "
            f"operations={operations}"
        )


def self_test() -> int:
    sample = {
        "schema_version": 1,
        "index": {
            "frames": 1080,
            "elapsed_ns": 1_000_000_000,
            "db_bytes": 64_000,
            "persisted_batches": 5,
            "max_live_decoded_frames": 3,
            "live_frame_limit": 4,
            "peak_buffered_metadata_entries": 256,
            "metadata_batch_limit": 256,
        },
        "measurements": {
            name: {"operations": 100, "elapsed_ns": 10_000}
            for name in REQUIRED_MEASUREMENTS
        },
    }
    assert not validate(sample)
    sample["index"]["max_live_decoded_frames"] = 5
    assert validate(sample)
    print("Phase 3 benchmark-report contract self-test passed")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Validate and print Phase 3 benchmark/instrumentation metrics without flaky timing gates"
        )
    )
    parser.add_argument("metrics", nargs="?", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        return self_test()
    if args.metrics is None:
        parser.error("metrics JSON path is required unless --self-test is used")

    metrics = json.loads(args.metrics.read_text(encoding="utf-8"))
    errors = validate(metrics)
    if errors:
        for error in errors:
            print(f"error: {error}")
        return 1
    report(metrics)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
