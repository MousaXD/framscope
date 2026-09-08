# Releases

FrameScope uses semantic Android versions and a fail-closed signed GitHub Release workflow.

## Version source

`version.properties` is the authoritative application version source.

`versionName` must be exactly `MAJOR.MINOR.PATCH`. A release tag must be exactly `v<versionName>`.

## Main snapshot releases

Every push to `main` starts a dedicated snapshot build. The workflow publishes an installable testing pre-release only after that exact commit successfully builds and verifies the native arm64 library, passes Android unit tests and lint, builds the debug APK, and verifies APK native packaging.

Snapshot tags use `main-<12-char-commit-sha>`. Each snapshot contains that workflow's verified arm64 debug APK plus a SHA-256 checksum. Failed snapshot builds publish nothing.

These snapshots are for testing and rapid access to the latest `main` build. They are not signed production releases and do not replace the physical-device acceptance or signing requirements below.

## Physical-device release evidence

Before the first production release, an accepted Phase 7 physical-device report is required.

The report must match the tested source commit and application version. The report must cover real non-emulator arm64 Android hardware, local SAF provider behavior, representative large media profiling, export lifecycle behavior, and the documented codec/navigation scenarios.

The signed release workflow refuses to publish when:

- the report is absent or invalid;
- the report version does not match the release version;
- the tested commit is not an ancestor of the release tag;
- APK-affecting source/build inputs changed after the tested commit.

Documentation-only updates after the physical test are allowed.

## Signing material

Signing material must never be committed to the repository. The release workflow expects GitHub Actions repository secrets for the Android signing keystore and credentials.

The workflow refuses to publish when signing values are incomplete. The keystore is decoded only into the ephemeral runner and is not persisted.

## Release procedure

1. Update `version.properties` on a pull request.
2. Complete canonical CI, Phase 3/4 contracts, Phase 7 production/release contracts, and physical-device acceptance.
3. Configure signing secrets.
4. Create and push the exact version tag.
5. The release workflow verifies source version, physical evidence, Rust workspace, Android build, signing identity, APK packaging, and checksum before publishing.

The workflow creates:

- signed arm64 APK;
- SHA-256 checksum;
- generated GitHub release notes linked to the immutable source tag.
