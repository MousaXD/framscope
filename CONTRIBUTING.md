# Contributing to FrameScope

Thank you for contributing. FrameScope is deliberately narrow: it inspects, navigates, compares, and extracts video frames. Features outside that purpose should remain outside the project.

## Before opening a change

1. Read `docs/architecture.md` and `docs/roadmap.md`.
2. Keep work inside the current phase unless the project has explicitly moved forward.
3. Do not add cloud services, telemetry, analytics, authentication, ads, subscriptions, or remote APIs.
4. Preserve the local-first privacy model and SAF-based file access.
5. Do not weaken parser bounds, tests, or CI checks to make a change pass.

## Development checks

Run:

```bash
./scripts/verify.sh
```

At minimum, Rust changes need formatting, clippy, and tests. Android changes need unit tests where logic is testable, lint, and a debug build.

## Architecture expectations

- Put shared Rust domain types in `framescope-core`.
- Put media semantics in `framescope-video`.
- Keep Android-only JNI code in `framescope-ffi`.
- Keep UI logic out of Activities and large Composables.
- Do not load entire video files into memory.
- Do not introduce raw-PNG-all-frames caching.

## Pull requests

A good pull request explains:

- the user-visible or architectural problem;
- why the change belongs in the current phase;
- tests added/updated;
- relevant performance, memory, privacy, and malformed-input considerations.

By contributing, you agree that your contribution is licensed under the repository's GPL-3.0 license.
