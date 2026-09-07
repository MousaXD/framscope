# Phase 5 Performance Contract

FrameScope performance targets are correctness-first.

## Rules

- Frame stepping must use indexed navigation paths.
- Repeated frame access should benefit from cache layers.
- Cache limits remain byte bounded.
- UI responsiveness must not depend on full-video decoding.
- Performance benchmarks report measurements without replacing correctness tests.

## CI policy

Heavy validation belongs in GitHub Actions. Local machines are not required to reproduce full FFmpeg, Android, and media fixture verification.
