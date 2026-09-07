# Phase 2 video fixtures

The repository does **not** commit generated video binaries. `manifest.json` is the source of truth for the fixture names and expected metadata; `scripts/generate-video-fixtures.sh` recreates the binaries under `build/video-fixtures/` and `scripts/verify-video-fixtures.py` checks them with `ffprobe`.

The generator uses synthetic FFmpeg sources only (`testsrc2`, `color`, and `sine`), so no third-party media is redistributed.

Current fixture contracts cover:

- H.264 CFR and deliberately VFR timestamps;
- HEVC/H.265, VP9, and AV1 encoded samples;
- display-rotation metadata;
- video with and without audio;
- multiple video streams plus audio;
- unusual dimensions;
- a single-frame video;
- deliberately truncated input that probing must reject.

These fixtures prove only that CI can generate known media for tests. Their presence does **not** claim that FrameScope's Android FFmpeg build can decode HEVC, VP9, AV1, or any other codec. Decoder support is owned by the FFmpeg/native configuration and must be asserted separately once Agent 1 and Agent 2 land.

## Local use

```bash
bash ./scripts/generate-video-fixtures.sh
python3 ./scripts/verify-video-fixtures.py
```

To generate into another directory:

```bash
bash ./scripts/generate-video-fixtures.sh /tmp/framescope-fixtures
python3 ./scripts/verify-video-fixtures.py /tmp/framescope-fixtures
```

The generator intentionally fails if its FFmpeg binary lacks one of the encoders needed to create the complete Phase 2 fixture set. That is a fixture-generation prerequisite, not an application codec-support check.
