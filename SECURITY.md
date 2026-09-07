# Security Policy

## Scope

FrameScope processes untrusted local video files. Security-sensitive areas include native parsers/decoders, JNI boundaries, file-descriptor ownership, cache invalidation, output paths, and future FFmpeg integration.

Phase 1 intentionally has no backend, account system, analytics, telemetry, ads, or network permission.

## Reporting a vulnerability

Please do not publish an exploit or sensitive proof-of-concept in a public issue before maintainers have had a reasonable chance to assess it. Once the project is hosted publicly, use the repository's private security-advisory channel if available; otherwise contact the maintainer through the private contact method listed by the repository.

A useful report includes affected revision/version, platform/device details, reproduction steps, expected/actual behavior, and impact. Avoid attaching private videos; use a minimal synthetic sample when possible.

## Security invariants

- Selected media remains local during normal operation.
- No broad filesystem permission is required.
- Native parsing uses bounded reads and checked offsets.
- Rust errors and panics must not unwind across JNI.
- Android owns the original SAF file descriptor; Rust owns only a duplicated descriptor.
- Future caches must be bounded and invalidated when source identity changes.
- Secrets and signing material must never be committed.
