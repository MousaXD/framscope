#!/usr/bin/env python3
from __future__ import annotations

import argparse
import copy
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SCHEMA_VERSION = 1
MIN_ANDROID_API = 26
MAX_ANDROID_API = 36
MIN_LARGE_MEDIA_BYTES = 1 << 30
VALID_RESULT_STATES = {"pending", "pass", "fail", "blocked"}
VALID_REPORT_STATES = {"pending", "accepted"}

REQUIRED_PROVIDER_CAPABILITIES = (
    "local_picker_visibility",
    "create_workspace_directory",
    "create_document",
    "open_writable_descriptor",
    "stable_requested_filenames",
    "delete_uncommitted_document",
    "delete_uncommitted_workspace",
)

REQUIRED_CODEC_CASES = (
    "h264_cfr",
    "h264_vfr",
    "hevc_cfr",
    "vp9_cfr",
    "av1_cfr",
    "rotated_portrait",
    "h264_with_audio",
    "multi_stream",
    "unusual_dimensions",
    "very_short",
)

REQUIRED_SCENARIOS = (
    "install_and_launch",
    "open_local_video",
    "frame_step_boundaries",
    "vfr_timestamp_navigation",
    "zoom_and_pan",
    "group_navigation",
    "background_foreground",
    "source_replacement",
    "current_frame_png",
    "current_frame_jpeg",
    "current_frame_webp",
    "batch_frame_range",
    "batch_timestamp_range",
    "batch_every_n",
    "batch_all_frames_small_fixture",
    "batch_unique_groups",
    "batch_cancel",
    "workspace_manifest_and_names",
    "abort_cleanup",
    "force_stop_mid_export_no_false_success",
)

HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
SEMVER = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")


def mapping(value: Any, path: str, errors: list[str]) -> dict[str, Any]:
    if not isinstance(value, dict):
        errors.append(f"{path} must be an object")
        return {}
    return value


def non_empty_string(value: Any, path: str, errors: list[str]) -> None:
    if not isinstance(value, str) or not value.strip():
        errors.append(f"{path} must be a non-empty string")


def positive_number(value: Any, path: str, errors: list[str]) -> None:
    if isinstance(value, bool) or not isinstance(value, (int, float)) or value <= 0:
        errors.append(f"{path} must be > 0")


def positive_integer(value: Any, path: str, errors: list[str]) -> None:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        errors.append(f"{path} must be a positive integer")


def validate_result_map(
    value: Any,
    required: tuple[str, ...],
    path: str,
    require_pass: bool,
    errors: list[str],
) -> None:
    results = mapping(value, path, errors)
    for key in required:
        if key not in results:
            errors.append(f"{path}.{key} is required")
            continue
        state = results[key]
        if state not in VALID_RESULT_STATES:
            errors.append(
                f"{path}.{key} must be one of {sorted(VALID_RESULT_STATES)}, got {state!r}"
            )
        elif require_pass and state != "pass":
            errors.append(f"{path}.{key} must be pass for an accepted report, got {state!r}")


def validate_timestamp(value: Any, path: str, errors: list[str]) -> None:
    if not isinstance(value, str) or not value.endswith("Z"):
        errors.append(f"{path} must be an ISO-8601 UTC timestamp ending in Z")
        return
    try:
        parsed = datetime.fromisoformat(value[:-1] + "+00:00")
    except ValueError:
        errors.append(f"{path} is not a valid ISO-8601 timestamp")
        return
    if parsed.utcoffset() != timezone.utc.utcoffset(parsed):
        errors.append(f"{path} must use UTC")


def validate_report(data: Any, *, require_accepted: bool) -> list[str]:
    errors: list[str] = []
    root = mapping(data, "report", errors)
    if not root:
        return errors

    if root.get("schema_version") != SCHEMA_VERSION:
        errors.append(f"schema_version must be {SCHEMA_VERSION}")

    status = root.get("status")
    if status not in VALID_REPORT_STATES:
        errors.append(f"status must be one of {sorted(VALID_REPORT_STATES)}")
    if require_accepted and status != "accepted":
        errors.append("status must be accepted for release readiness")

    version = root.get("app_version")
    if not isinstance(version, str) or not SEMVER.fullmatch(version):
        errors.append("app_version must be semantic MAJOR.MINOR.PATCH")

    build_type = root.get("build_type")
    if require_accepted:
        if build_type not in {"debug", "release"}:
            errors.append("build_type must be debug or release")
    elif build_type is not None and build_type not in {"debug", "release"}:
        errors.append("build_type must be null, debug, or release")

    tested_commit = root.get("tested_commit")
    if require_accepted:
        if not isinstance(tested_commit, str) or not HEX40.fullmatch(tested_commit):
            errors.append("tested_commit must be a lowercase 40-hex Git commit SHA")
    elif tested_commit is not None and (
        not isinstance(tested_commit, str) or not HEX40.fullmatch(tested_commit)
    ):
        errors.append("tested_commit must be null or a lowercase 40-hex Git commit SHA")

    apk_sha = root.get("tested_apk_sha256")
    if require_accepted:
        if not isinstance(apk_sha, str) or not HEX64.fullmatch(apk_sha):
            errors.append("tested_apk_sha256 must be a lowercase 64-hex SHA-256")
    elif apk_sha is not None and (not isinstance(apk_sha, str) or not HEX64.fullmatch(apk_sha)):
        errors.append("tested_apk_sha256 must be null or a lowercase 64-hex SHA-256")

    tested_at = root.get("tested_at_utc")
    if require_accepted:
        validate_timestamp(tested_at, "tested_at_utc", errors)
    elif tested_at is not None:
        validate_timestamp(tested_at, "tested_at_utc", errors)

    device = mapping(root.get("device"), "device", errors)
    if device:
        for key in ("manufacturer", "model"):
            value = device.get(key)
            if require_accepted:
                non_empty_string(value, f"device.{key}", errors)
            elif value is not None:
                non_empty_string(value, f"device.{key}", errors)

        api = device.get("android_api")
        if require_accepted:
            if isinstance(api, bool) or not isinstance(api, int) or not MIN_ANDROID_API <= api <= MAX_ANDROID_API:
                errors.append(
                    f"device.android_api must be an integer in the supported range "
                    f"{MIN_ANDROID_API}..{MAX_ANDROID_API}"
                )
        elif api is not None and (
            isinstance(api, bool)
            or not isinstance(api, int)
            or not MIN_ANDROID_API <= api <= MAX_ANDROID_API
        ):
            errors.append(
                f"device.android_api must be null or an integer in {MIN_ANDROID_API}..{MAX_ANDROID_API}"
            )

        if device.get("abi") != "arm64-v8a":
            errors.append("device.abi must be arm64-v8a")
        if device.get("is_emulator") is not False:
            errors.append("device.is_emulator must be false; Phase 7 requires physical hardware")

    provider = mapping(root.get("provider"), "provider", errors)
    if provider:
        for key in ("name", "authority"):
            value = provider.get(key)
            if require_accepted:
                non_empty_string(value, f"provider.{key}", errors)
            elif value is not None:
                non_empty_string(value, f"provider.{key}", errors)
        if provider.get("local_only") is not True:
            errors.append("provider.local_only must be true; FrameScope pickers intentionally use EXTRA_LOCAL_ONLY")
        validate_result_map(
            provider.get("capabilities"),
            REQUIRED_PROVIDER_CAPABILITIES,
            "provider.capabilities",
            require_accepted,
            errors,
        )

    validate_result_map(
        root.get("codec_cases"),
        REQUIRED_CODEC_CASES,
        "codec_cases",
        require_accepted,
        errors,
    )
    validate_result_map(
        root.get("scenarios"),
        REQUIRED_SCENARIOS,
        "scenarios",
        require_accepted,
        errors,
    )

    large = mapping(root.get("large_media"), "large_media", errors)
    if large:
        string_fields = ("source_name", "codec")
        for key in string_fields:
            value = large.get(key)
            if require_accepted:
                non_empty_string(value, f"large_media.{key}", errors)
            elif value is not None:
                non_empty_string(value, f"large_media.{key}", errors)

        integer_fields = ("width", "height", "sampled_export_stride", "peak_pss_kib")
        for key in integer_fields:
            value = large.get(key)
            if require_accepted:
                positive_integer(value, f"large_media.{key}", errors)
            elif value is not None:
                positive_integer(value, f"large_media.{key}", errors)

        number_fields = (
            "duration_seconds",
            "open_to_ready_ms",
            "ten_step_navigation_ms",
            "sampled_export_elapsed_ms",
        )
        for key in number_fields:
            value = large.get(key)
            if require_accepted:
                positive_number(value, f"large_media.{key}", errors)
            elif value is not None:
                positive_number(value, f"large_media.{key}", errors)

        size = large.get("source_size_bytes")
        if require_accepted:
            if isinstance(size, bool) or not isinstance(size, int) or size < MIN_LARGE_MEDIA_BYTES:
                errors.append(
                    f"large_media.source_size_bytes must be at least {MIN_LARGE_MEDIA_BYTES} bytes (1 GiB)"
                )
        elif size is not None and (isinstance(size, bool) or not isinstance(size, int) or size <= 0):
            errors.append("large_media.source_size_bytes must be null or a positive integer")

    notes = root.get("notes")
    if not isinstance(notes, list) or any(not isinstance(note, str) for note in notes):
        errors.append("notes must be an array of strings")

    return errors


def accepted_self_test_report() -> dict[str, Any]:
    return {
        "schema_version": 1,
        "status": "accepted",
        "tested_commit": "a" * 40,
        "app_version": "0.1.0",
        "build_type": "debug",
        "tested_apk_sha256": "b" * 64,
        "tested_at_utc": "2026-09-08T04:00:00Z",
        "device": {
            "manufacturer": "Example",
            "model": "Physical Device",
            "android_api": 35,
            "abi": "arm64-v8a",
            "is_emulator": False,
        },
        "provider": {
            "name": "Local documents",
            "authority": "example.documents",
            "local_only": True,
            "capabilities": {key: "pass" for key in REQUIRED_PROVIDER_CAPABILITIES},
        },
        "codec_cases": {key: "pass" for key in REQUIRED_CODEC_CASES},
        "scenarios": {key: "pass" for key in REQUIRED_SCENARIOS},
        "large_media": {
            "source_name": "large.mp4",
            "source_size_bytes": MIN_LARGE_MEDIA_BYTES,
            "duration_seconds": 600,
            "codec": "h264",
            "width": 1920,
            "height": 1080,
            "open_to_ready_ms": 1000,
            "ten_step_navigation_ms": 500,
            "sampled_export_stride": 100,
            "sampled_export_elapsed_ms": 2000,
            "peak_pss_kib": 150000,
        },
        "notes": ["self-test"],
    }


def self_test() -> int:
    valid = accepted_self_test_report()
    assert not validate_report(valid, require_accepted=True)

    emulator = copy.deepcopy(valid)
    emulator["device"]["is_emulator"] = True
    assert any("physical hardware" in error for error in validate_report(emulator, require_accepted=True))

    small_media = copy.deepcopy(valid)
    small_media["large_media"]["source_size_bytes"] = MIN_LARGE_MEDIA_BYTES - 1
    assert any("1 GiB" in error for error in validate_report(small_media, require_accepted=True))

    failed_case = copy.deepcopy(valid)
    failed_case["scenarios"]["batch_cancel"] = "fail"
    assert any("batch_cancel" in error for error in validate_report(failed_case, require_accepted=True))

    pending = copy.deepcopy(valid)
    pending["status"] = "pending"
    pending["tested_commit"] = None
    pending["tested_apk_sha256"] = None
    pending["tested_at_utc"] = None
    pending["build_type"] = None
    pending["device"]["manufacturer"] = None
    pending["device"]["model"] = None
    pending["device"]["android_api"] = None
    pending["provider"]["name"] = None
    pending["provider"]["authority"] = None
    pending["provider"]["capabilities"] = {
        key: "pending" for key in REQUIRED_PROVIDER_CAPABILITIES
    }
    pending["codec_cases"] = {key: "pending" for key in REQUIRED_CODEC_CASES}
    pending["scenarios"] = {key: "pending" for key in REQUIRED_SCENARIOS}
    for key in pending["large_media"]:
        pending["large_media"][key] = None
    assert not validate_report(pending, require_accepted=False)

    print("Phase 7 device-report validator self-test passed")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Validate FrameScope Phase 7 physical-device/local-provider acceptance evidence"
    )
    parser.add_argument("report", nargs="?", type=Path)
    parser.add_argument(
        "--template",
        action="store_true",
        help="validate a pending report/template structurally instead of requiring accepted evidence",
    )
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        return self_test()
    if args.report is None:
        parser.error("report path is required unless --self-test is used")

    try:
        data = json.loads(args.report.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        print(f"error: could not read device report: {exc}", file=sys.stderr)
        return 1

    errors = validate_report(data, require_accepted=not args.template)
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1

    mode = "template" if args.template else "accepted physical-device report"
    print(f"Verified Phase 7 {mode}: {args.report}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
