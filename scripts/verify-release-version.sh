#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION_FILE="$ROOT/version.properties"
TAG="${1:-${GITHUB_REF_NAME:-}}"

test -f "$VERSION_FILE" || { echo "error: version.properties is missing" >&2; exit 1; }
version_name="$(awk -F= '$1 == "versionName" { print $2 }' "$VERSION_FILE" | tr -d '\r')"
version_code="$(awk -F= '$1 == "versionCode" { print $2 }' "$VERSION_FILE" | tr -d '\r')"

[[ "$version_name" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  echo "error: versionName must be MAJOR.MINOR.PATCH" >&2
  exit 1
}
[[ "$version_code" =~ ^[1-9][0-9]*$ ]] || {
  echo "error: versionCode must be a positive integer" >&2
  exit 1
}
[[ "$TAG" == "v$version_name" ]] || {
  echo "error: release tag '$TAG' must exactly match v$version_name" >&2
  exit 1
}

printf 'Verified release tag %s for Android versionCode %s\n' "$TAG" "$version_code"
