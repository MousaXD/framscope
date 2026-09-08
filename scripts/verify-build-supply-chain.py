#!/usr/bin/env python3
"""Static supply-chain policy for GitHub Actions and Android Gradle declarations."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FULL_SHA = re.compile(r"[0-9a-f]{40}")
USES_RE = re.compile(r"^\s*-?\s*uses:\s*['\"]?([^'\"\s#]+)")
PLUGIN_VERSION_RE = re.compile(r"\bversion\s+\"([^\"]+)\"")
COORDINATE_RE = re.compile(r"\"([A-Za-z0-9_.-]+:[A-Za-z0-9_.-]+:([^\"]+))\"")
FORBIDDEN_VERSION_MARKERS = ("+", "SNAPSHOT", "latest.", "[", "]", "(", ")")


def verify_action_pins() -> list[str]:
    errors: list[str] = []
    workflow_root = ROOT / ".github" / "workflows"
    for path in sorted(workflow_root.glob("*.y*ml")):
        for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            match = USES_RE.match(line)
            if match is None:
                continue
            target = match.group(1)
            if target.startswith("./"):
                continue
            if target.startswith("docker://"):
                if "@sha256:" not in target:
                    errors.append(f"{path.relative_to(ROOT)}:{line_number}: docker action is not digest pinned: {target}")
                continue
            if "@" not in target:
                errors.append(f"{path.relative_to(ROOT)}:{line_number}: external action has no immutable ref: {target}")
                continue
            _, ref = target.rsplit("@", 1)
            if FULL_SHA.fullmatch(ref) is None:
                errors.append(
                    f"{path.relative_to(ROOT)}:{line_number}: external action must use a full 40-hex commit SHA: {target}"
                )
    return errors


def verify_gradle_versions_and_repositories() -> list[str]:
    errors: list[str] = []
    android_root = ROOT / "android"
    gradle_files = sorted(android_root.rglob("*.gradle.kts"))

    for path in gradle_files:
        text = path.read_text(encoding="utf-8")
        for version in PLUGIN_VERSION_RE.findall(text):
            if any(marker in version for marker in FORBIDDEN_VERSION_MARKERS):
                errors.append(f"{path.relative_to(ROOT)}: dynamic plugin version is forbidden: {version}")
        for coordinate, version in COORDINATE_RE.findall(text):
            if any(marker in version for marker in FORBIDDEN_VERSION_MARKERS):
                errors.append(f"{path.relative_to(ROOT)}: dynamic dependency version is forbidden: {coordinate}")

        if re.search(r"\bmaven\s*\{", text):
            errors.append(
                f"{path.relative_to(ROOT)}: custom Maven repositories require explicit Phase 7 review; use google()/mavenCentral()"
            )
        if re.search(r"\bivy\s*\{", text):
            errors.append(f"{path.relative_to(ROOT)}: custom Ivy repositories are not approved")

    settings = (android_root / "settings.gradle.kts").read_text(encoding="utf-8")
    if "RepositoriesMode.FAIL_ON_PROJECT_REPOS" not in settings:
        errors.append("android/settings.gradle.kts: dependency repositories must remain centralized/fail-on-project-repos")
    for required in ("google()", "mavenCentral()"):
        if required not in settings:
            errors.append(f"android/settings.gradle.kts: required reviewed repository {required} is missing")

    return errors


def main() -> int:
    errors = verify_action_pins() + verify_gradle_versions_and_repositories()
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    print("GitHub Actions are immutable-SHA pinned and Android dependency declarations avoid dynamic/custom sources.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
