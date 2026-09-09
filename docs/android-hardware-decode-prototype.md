# Android hardware video decode prototype

This document is the Agent 04 hardware-decode handoff for the FrameScope performance and UX remediation wave.

## Production state verified before this branch

FrameScope's Android FFmpeg build enables the native `h264`, `hevc`, `vp9`, and `av1` decoders. The production shim selects decoders with `avcodec_find_decoder(codec_id)` and opens them with `avcodec_open2`. There is no explicit MediaCodec decoder selection in the current production path.

Indexing calls the metadata-only `next_frame` API, so it does not pay the RGBA/sws_scale conversion used by presentation. It still reconstructs every presentation frame in the software decoder. Hardware decode therefore has to beat decoder reconstruction cost; removing RGBA conversion is not an indexing optimization.

## Safety boundary

Nothing in this branch changes the production decoder used by indexing, exact navigation, extraction, or scrub presentation.

Hardware decode remains a benchmark/prototype candidate until a physical-device corpus proves the required semantics. A faster result is not acceptable if it changes frame count, presentation ordering, VFR timestamps, PTS/DTS behavior, duration, clean-keyframe identity, corrupt-frame state, cancellation behavior, or the persistent FrameId contract.

No root access, privileged Android permission, governor change, thermal override, cpuset modification, swap/zram change, or kernel tuning is used or required.

## Capability matrix

The matrix is intentionally runtime-driven rather than a claim that every Android device implements every codec in hardware.

| Codec | Production software fallback | Direct Android MediaCodec candidate | FFmpeg MediaCodec probe | Automatic hardware classification |
| --- | --- | --- | --- | --- |
| H.264 / `video/avc` | `h264` | yes, when enumerated | `h264_mediacodec` | API 29+ explicit API flag only |
| HEVC / `video/hevc` | `hevc` | yes, when enumerated | `hevc_mediacodec` | API 29+ explicit API flag only |
| VP9 / `video/x-vnd.on2.vp9` | `vp9` | yes, when enumerated | `vp9_mediacodec` | API 29+ explicit API flag only |
| AV1 / `video/av01` | `av1` | yes, when enumerated | `av1_mediacodec` | API 29+ explicit API flag only |

Android 10 / API 29 added `MediaCodecInfo.isHardwareAccelerated`, `isSoftwareOnly`, `isVendor`, and `isAlias`. FrameScope uses those flags as a candidate filter, not as proof of performance or correctness. On the API 26-28 floor the prototype reports acceleration as `Unknown` rather than inferring it from component names. The existing FFmpeg software path is the fallback.

## Prototype A: current FFmpeg software decoder

`tools/android-hwdecode/framescope_decode_bench.c` can force the native software decoder by name. This avoids accidentally comparing MediaCodec against itself when both decoder families are present in the probe build.

Reported fields include:

- decoder name and codec id;
- frames decoded;
- decoder/container open time;
- time to first decoded frame;
- total wall time and frames/second;
- process CPU time;
- optional benchmark-only seek-to-target settle time.

The process CPU measurement does not include work performed in a codec service or dedicated hardware block, so it must not be interpreted as total device energy cost.

## Prototype B: direct Android MediaCodec

`AndroidHardwareDecodeCapabilities` enumerates actual device decoders. `AndroidMediaCodecBenchmark.decodeByteBuffer` selects only a component explicitly marked hardware-accelerated on API 29+, configures ByteBuffer output, and records:

- frames decoded and frames/second;
- wall time and process CPU time;
- time to first output frame;
- optional seek-to-target settle time;
- first/last output presentation timestamp plus a sequence digest;
- negotiated output color format;
- app-visible thermal status before and after the run;
- permission-free `BatteryManager` energy-counter samples when the device implements them, plus a derived whole-device average battery-power estimate for a discharging run.

Battery energy/power fields are nullable. They are whole-device observations, can be too coarse for short runs, and are not attributed solely to FrameScope or the codec. The benchmark never substitutes process CPU or GPU utilization for a power measurement.

The opt-in instrumentation entry point is `AndroidMediaCodecDeviceBenchmarkTest`. CI compiles it but does not fabricate physical performance results. The input fixture must already be readable by the debug app; the benchmark does not request storage or privileged permissions.

Direct MediaCodec is currently a **preview candidate, not an authoritative indexing candidate**. `MediaCodec.BufferInfo` gives output presentation time and flags, but it does not reproduce all fields in FrameScope's persistent `FrameIndexEntry` contract, notably frame duration and the current FFmpeg corrupt/decode-error state. A timestamp sequence alone is therefore insufficient proof for authoritative indexing.

Surface output is especially attractive for coarse/live preview because it can avoid copying decoded pixels through the CPU. It is not suitable by itself for Inspector, similarity, or source-quality extraction, which need inspectable pixel data. ByteBuffer/YUV output is the relevant direct-MediaCodec experiment for those consumers, and must be validated for vendor color-layout/stride/crop behavior before use.

## Prototype C: FFmpeg MediaCodec

`scripts/build-ffmpeg-android-hwdecode-probe.sh` builds a separate FFmpeg 9.0.1 prefix. It enables JNI/MediaCodec plus both software and MediaCodec decoders for H.264, HEVC, VP9, and AV1. The production pinned FFmpeg prefix is unchanged.

The benchmark forces FFmpeg's `ndk_codec=1` option for the MediaCodec pass. This allows the probe to exercise the NDK MediaCodec wrapper without requiring FrameScope to register a Java VM with FFmpeg.

Build the probe executable:

```bash
bash ./scripts/build-android-hwdecode-bench.sh
```

A non-root physical-device run can then use ordinary `adb` developer tooling, for example:

```bash
adb push .native/hwdecode-bench/arm64-v8a/framescope-decode-bench /data/local/tmp/
adb push fixture.mp4 /data/local/tmp/
adb shell chmod 755 /data/local/tmp/framescope-decode-bench
adb shell /data/local/tmp/framescope-decode-bench compare /data/local/tmp/fixture.mp4 3000
```

The `compare` mode decodes the same prefix once with FFmpeg software and once with FFmpeg MediaCodec. It compares, frame by frame:

- presentation timestamp selection (`best_effort_timestamp`, falling back to `pts`);
- decoded-frame `pkt_dts` presence/value;
- positive frame duration;
- keyframe flag;
- corrupt/decode-error state;
- decoded presentation-frame count/order.

The presentation timestamp, duration, keyframe and corrupt fields are the current persistent-index-driving metadata. DTS is additionally compared because the remediation wave requires it to remain correct even though it is not currently persisted in `FrameIndexEntry`.

Any first mismatch returns exit status 2. Decoder/open failures return exit status 1. This is intentionally fail-closed.

Benchmark-only seek timing is also available:

```bash
adb shell /data/local/tmp/framescope-decode-bench seek software /data/local/tmp/fixture.mp4 10000000 500
adb shell /data/local/tmp/framescope-decode-bench seek ffmpeg-mediacodec /data/local/tmp/fixture.mp4 10000000 500
```

These seek numbers are **not** proof that timestamp seeking is safe for FrameId resolution. FrameScope's existing timestamp-seek safety contract remains authoritative and must still fall back when an anchor is ambiguous.

## Physical acceptance matrix still required

At minimum, collect software, direct MediaCodec, and FFmpeg MediaCodec results on representative physical devices for:

1. 1080p H.264 with B-frames;
2. 1080p or 4K HEVC with long GOPs;
3. VP9 where a true hardware component is reported;
4. AV1 where a true hardware component is reported;
5. a VFR fixture with non-uniform presentation intervals;
6. an intentionally damaged/corrupt fixture already accepted by the software test corpus;
7. repeated warm and cold runs long enough to observe thermal behavior and, where supported, battery-energy-counter movement.

For authoritative-index consideration, FFmpeg MediaCodec must produce an exact metadata-row match against software on the entire fixture, not only an equal frame count. For preview consideration, record visual correctness, stale-request behavior, cancellation latency, seek latency, TTFF, FPS, process CPU, thermal start/end, supported battery-energy/power observations, and device/model/build information.

Power remains a best-effort physical-device metric. If the battery energy counter is unavailable or too coarse, report it as unavailable rather than inferring power from GPU utilization or process CPU alone.

## Recommendation by stage

| Stage | Recommendation now | Promotion requirement |
| --- | --- | --- |
| Structural / authoritative indexing | keep FFmpeg software authoritative | consider FFmpeg MediaCodec only after exact full-row equivalence across the physical corpus plus cancellation/restart tests |
| Coarse/live scrub preview | direct MediaCodec Surface is the strongest future candidate | integrate only with Agent 01/02 scrub generation/cancellation rules; stale outputs must never win |
| ByteBuffer preview / analysis | benchmark direct MediaCodec YUV | prove color format, stride, crop, rotation, timestamp, and pixel ownership on vendor devices |
| Exact indexed navigation | keep existing authoritative FrameId path | decoder acceleration may be substituted only after FrameId is resolved and the accelerated decoder proves equivalent output ordering/timestamps |
| Inspector/full-resolution extraction | keep source-quality FFmpeg path | hardware path needs source-quality pixel/color evidence, not only timing evidence |

## Cross-agent dependencies

- Agent 02 decoder/navigation owns production decoder locality and exact-navigation policy. This branch must not bypass its timestamp-seek/FrameId rules.
- Agent 06/07 indexing owns index structure and throughput. Hardware MediaCodec should be evaluated against their device instrumentation before becoming an indexing backend.
- Agent 12 owns final integration/device benchmarks. The benchmark outputs from this branch are inputs to that acceptance lane.
- Any future Surface preview integration must coordinate with Agent 01 scrub UX so cancellation/generation ordering remains correct.
