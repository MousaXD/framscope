# Security Policy

## Scope

FrameScope processes untrusted local video files through Rust, FFmpeg, JNI, Android Storage Access Framework descriptors, persistent indexes/caches, similarity analysis, and frame export. Security-sensitive areas include native parser/decoder state, checked media metadata, file-descriptor ownership, source-identity invalidation, cache and similarity persistence, output transactions, and release signing.

The normal application path is local-first and has no backend, account system, analytics, telemetry, ads, or `INTERNET` permission. Broad Android storage permissions are not used, and cleartext network traffic is disabled defensively at the application manifest level.

## Reporting a vulnerability

Please do not publish an exploit or sensitive proof-of-concept in a public issue before maintainers have had a reasonable chance to assess it. Prefer the repository's private GitHub Security Advisory reporting channel when available. If that channel is unavailable, contact the maintainer through a private contact method associated with the repository rather than posting exploit details publicly.

A useful report includes the affected commit/version, Android/device details when relevant, reproduction steps, expected and actual behavior, and impact. Avoid attaching private videos; use a minimal synthetic sample whenever possible.

## Security invariants

- Selected media and exported frames remain local during normal operation.
- No network or broad filesystem permission is required.
- Android owns original SAF descriptors; native code owns only explicit duplicates.
- Rust panics and errors must not unwind across JNI.
- Frame identity and media time use validated persistent `FrameId`/PTS contracts rather than FPS reconstruction.
- Native dimensions, strides, offsets, timestamps, counts, and output sizes use checked validation before allocation or handoff.
- Full-resolution extraction never consumes lossy navigation proxies.
- Persistent indexes, caches, and similarity groups are derived state and must be invalidated or rebuilt when their strong source/stream/config identity does not match.
- RAM and disk caches, grouping, extraction planning, encoded output, and manifests remain bounded/streaming for large media.
- Partial Android export documents/workspaces are rolled back unless their transaction reaches an accepted commit boundary.
- The arm64 JNI library must pass ELF hardening verification and expose only the expected application bridge surface.
- Official releases must be signed with the long-lived Android release key and verified before publication. Debug or unsigned APKs are never official release assets.
- Signing material, passwords, API tokens, and other secrets must never be committed.

## Native dependency provenance

Android FFmpeg is built from the repository-pinned official source version/checksum and verified build configuration. CI verifies the Android ABI, FFmpeg provenance, linked native library, JNI exports, release-mode APK packaging, and repository privacy invariants.

Third-party dependency and advisory review remains part of Phase 7 production hardening. A dependency update must not weaken the accepted media, source-quality, bounded-memory, or local-first contracts merely to obtain a newer version number.

## Release key handling

The release workflow reads signing material only from GitHub Actions secrets and decodes the keystore into an ephemeral hosted runner. See `docs/releases.md` for the required secret names and release procedure. The long-lived keystore must also have an offline backup because losing the signing identity can prevent compatible updates for existing installations.
