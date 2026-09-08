# Releases

FrameScope uses semantic Android versions and a fail-closed signed GitHub Release workflow.

## Version source

`version.properties` is the authoritative application version source:

```properties
versionName=0.1.0
versionCode=1
```

`versionName` must be exactly `MAJOR.MINOR.PATCH`. `versionCode` must be a positive Android integer and must increase for every published Android update.

The Android Gradle build reads these values directly. A release tag must be exactly `v<versionName>`, for example `v0.1.0`.

## Signing material

Signing material must never be committed to the repository. The release workflow expects these GitHub Actions repository secrets:

- `FRAMESCOPE_SIGNING_KEYSTORE_B64`: base64 of the long-lived Android signing keystore;
- `FRAMESCOPE_SIGNING_STORE_PASSWORD`;
- `FRAMESCOPE_SIGNING_KEY_ALIAS`;
- `FRAMESCOPE_SIGNING_KEY_PASSWORD`.

The workflow refuses to publish when any value is absent. It decodes the keystore only into the ephemeral GitHub-hosted runner, gives it mode `0600`, and passes signing configuration to Gradle through environment variables.

The signing keystore is an application identity. Keep an offline backup separate from GitHub. Losing it can make future updates incompatible with already-installed releases.

## Release procedure

1. Update `version.properties` on a normal pull request. Increase `versionCode` and choose the next semantic `versionName`.
2. Merge only after canonical CI and the Phase 7 release-package gate pass.
3. Confirm the four signing secrets are configured in the repository.
4. Create and push the exact version tag, such as `v0.1.0`, from the accepted `main` commit.
5. The tag also runs canonical CI. The dedicated `Release` workflow independently verifies the source version, Rust workspace, Android release build, signing certificate, APK identity/privacy/native packaging, and checksum before publishing.

The workflow creates:

- `FrameScope-<version>-arm64-v8a.apk`;
- `FrameScope-<version>-arm64-v8a.apk.sha256`;
- generated GitHub release notes linked to the immutable source tag.

No debug APK and no unsigned release APK is published by the release workflow.

## Pull-request release gate

`.github/workflows/phase7.yml` builds an unsigned **release candidate** on ordinary pull requests. That artifact is used only to prove release-mode configuration, non-debuggability, version identity, ABI packaging, and privacy invariants. It is not published as an installable production release.

The normal native CI additionally verifies hardening properties of `libframescope_ffi.so`: AArch64 identity, GNU RELRO, eager binding, a non-executable stack, no text relocations, no runtime search path, and expected JNI exports.

## Signing a local release build

The Gradle release build accepts the same four values as environment variables, except that the keystore value is a path rather than base64:

- `FRAMESCOPE_SIGNING_STORE_FILE`;
- `FRAMESCOPE_SIGNING_STORE_PASSWORD`;
- `FRAMESCOPE_SIGNING_KEY_ALIAS`;
- `FRAMESCOPE_SIGNING_KEY_PASSWORD`.

All four must be supplied together. With none supplied, `assembleRelease` intentionally produces an unsigned candidate suitable for verification only. With a partial set, Gradle fails instead of silently producing a differently signed artifact.
