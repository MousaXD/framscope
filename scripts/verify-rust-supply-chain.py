#!/usr/bin/env python3
"""Fail-closed Rust dependency source/license/checksum policy for Phase 7 CI."""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RUST_ROOT = ROOT / "rust"
LOCKFILE = RUST_ROOT / "Cargo.lock"

# Vetted SPDX atoms accepted for dependencies linked into this GPL-3.0-only application.
# Expressions may combine these atoms with AND/OR/WITH; a new atom fails CI until reviewed here.
ALLOWED_LICENSE_ATOMS = {
    "0BSD",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSL-1.0",
    "CC0-1.0",
    "GPL-3.0-only",
    "GPL-3.0-or-later",
    "ISC",
    "LGPL-2.1-only",
    "LGPL-2.1-or-later",
    "LGPL-3.0-only",
    "LGPL-3.0-or-later",
    "MIT",
    "MIT-0",
    "MPL-2.0",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "Unlicense",
    "Zlib",
}
ALLOWED_EXCEPTIONS = {"LLVM-exception"}
OPERATORS = {"AND", "OR", "WITH"}
ALLOWED_REGISTRY_PREFIXES = (
    "registry+https://github.com/rust-lang/crates.io-index",
    "registry+https://index.crates.io/",
)
TOKEN_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9.+-]*")
SHA256_RE = re.compile(r"[0-9a-f]{64}")


def cargo_metadata() -> dict:
    result = subprocess.run(
        [
            "cargo",
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--manifest-path",
            str(RUST_ROOT / "Cargo.toml"),
        ],
        cwd=ROOT,
        check=False,
        text=True,
        capture_output=True,
    )
    if result.returncode != 0:
        print(result.stdout, end="", file=sys.stderr)
        print(result.stderr, end="", file=sys.stderr)
        raise RuntimeError("cargo metadata --locked failed")
    return json.loads(result.stdout)


def license_atoms(expression: str) -> set[str]:
    tokens = set(TOKEN_RE.findall(expression))
    return tokens - OPERATORS


def verify_metadata(metadata: dict) -> list[str]:
    errors: list[str] = []
    reviewed: set[str] = set()

    for package in metadata.get("packages", []):
        source = package.get("source")
        if source is None:
            # Workspace/path crates inherit FrameScope's GPL-3.0-only policy.
            continue

        identity = f"{package.get('name', '<unknown>')} {package.get('version', '<unknown>')}"
        source = str(source)
        if not source.startswith(ALLOWED_REGISTRY_PREFIXES):
            errors.append(f"{identity}: dependency source is not the approved crates.io registry: {source}")

        license_expression = package.get("license")
        if not isinstance(license_expression, str) or not license_expression.strip():
            errors.append(f"{identity}: dependency has no SPDX license expression")
            continue

        atoms = license_atoms(license_expression)
        if not atoms:
            errors.append(f"{identity}: could not parse license expression {license_expression!r}")
            continue
        unknown = atoms - ALLOWED_LICENSE_ATOMS - ALLOWED_EXCEPTIONS
        if unknown:
            errors.append(
                f"{identity}: unreviewed license atom(s) {sorted(unknown)} in {license_expression!r}"
            )
        reviewed.add(license_expression)

    if reviewed:
        print("Reviewed Rust dependency license expressions:")
        for expression in sorted(reviewed):
            print(f"  {expression}")
    return errors


def verify_lockfile() -> list[str]:
    errors: list[str] = []
    with LOCKFILE.open("rb") as handle:
        lock = tomllib.load(handle)

    for package in lock.get("package", []):
        source = package.get("source")
        if not isinstance(source, str):
            continue
        identity = f"{package.get('name', '<unknown>')} {package.get('version', '<unknown>')}"
        if not source.startswith(ALLOWED_REGISTRY_PREFIXES):
            errors.append(f"{identity}: lockfile contains an unapproved non-registry source: {source}")
            continue
        checksum = package.get("checksum")
        if not isinstance(checksum, str) or SHA256_RE.fullmatch(checksum) is None:
            errors.append(f"{identity}: crates.io package is missing a valid SHA-256 lockfile checksum")

    return errors


def main() -> int:
    errors: list[str] = []
    try:
        errors.extend(verify_metadata(cargo_metadata()))
        errors.extend(verify_lockfile())
    except (OSError, RuntimeError, ValueError, json.JSONDecodeError, tomllib.TOMLDecodeError) as exc:
        print(f"error: supply-chain verification could not run: {exc}", file=sys.stderr)
        return 1

    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1

    print("Rust dependency sources, SPDX license atoms, and registry checksums satisfy Phase 7 policy.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
