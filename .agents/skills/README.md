# FrameScope agent skills

FrameScope keeps Android/Compose skills project-local so coding agents can use the same guidance in every environment.

## Official Google Android skills

Installed with Google's Android CLI from `android/skills`:

- `android-cli`
- `testing-setup`
- `android-profiler`
- `r8-analyzer`
- `edge-to-edge`
- `android-intent-security`
- `adaptive`
- `navigation-3`

Refresh these with `android skills add <skill> --project=. --agent=codex`; keep Google's generated contents intact.

## GUI / player Compose skills

Vendored from reviewed public upstreams at exact commits:

- `compose-expert` — `aldefy/compose-skill@954ef54ea32288fbc90745f012d09d7b791f0d8a` (MIT; bundled AOSP source references are Apache-2.0)
- `compose-performance` — `chrisbanes/skills@2db11bb412254246bf0a5f6e5e320177107a7c53` (Apache-2.0)
- `compose-state-and-effects` — `chrisbanes/skills@2db11bb412254246bf0a5f6e5e320177107a7c53` (Apache-2.0)
- `compose-component-design` — `chrisbanes/skills@2db11bb412254246bf0a5f6e5e320177107a7c53` (Apache-2.0)
- `compose-ui-testing-patterns` — `chrisbanes/skills@2db11bb412254246bf0a5f6e5e320177107a7c53` (Apache-2.0)
- `material-3` — `hamen/material-3-skill@14385f2bf3804d8779f8b4db2604211f1e70b4c1` (MIT)

Each community skill contains `LICENSE.upstream`. The Material 3 package's Claude-plugin metadata is intentionally excluded because FrameScope vendors these skills for its project-local agent skill directory, not as a Claude plugin.
