# Android indexing performance and scheduler workflow

This document is the Agent 11 performance contract for FrameScope. It is intentionally usable on a normal non-root Android phone.

## Safety boundary

Do not use root or `su`. Do not change CPU governors, cpusets, thermal controls, swap/zram, kernel scheduling knobs, or privileged `/sys` or `/proc` files. The workflow below uses app APIs, `adb shell`, and the platform Perfetto service only.

## What source inspection proves today

On the production base merged into `main` on 2026-09-09:

- the Android microscope open/index call runs synchronously on a Kotlin `Dispatchers.IO` worker;
- the outer Rust indexer remains structurally serial around `decoder.next_frame()`, metadata construction, and periodic SQLite batch commits;
- FFmpeg opens the ordinary codec selected by `avcodec_find_decoder()` and does not explicitly configure an Android MediaCodec hardware decoder in this path;
- FFmpeg codec worker threading, if any, is internal to libavcodec; FrameScope does not currently choose or report the worker count;
- there was no app-level thread-priority policy, ADPF/`PerformanceHintManager` session, thermal monitor, or scheduler trace surface before this branch.

These facts make low total CPU utilization plausible, but they do **not** prove which component dominates a physical device. A task-manager percentage cannot distinguish a serial coordinator, codec workers, runnable queue delay, blocking I/O, SQLite, CPU-frequency scaling, or thermal throttling.

## Runtime telemetry added by Agent 11

`FrameScopePerformanceRuntime` surrounds only the synchronous native microscope open/index operation. It does not alter frame identity, ordering, timestamps, cancellation, index persistence, or similarity behavior.

In `SustainedThroughput` mode it:

1. records the Android TID and temporarily applies `THREAD_PRIORITY_FOREGROUND` to the indexing coordinator, restoring the original priority in `finally`;
2. starts a `FrameScope#indexing` Android trace section;
3. waits for genuine advancing native progress samples before creating an ADPF hint session on Android 12 / API 31+;
4. calibrates the initial ADPF target from the first measured native work interval instead of inventing a target FPS;
5. ignores duplicate/stale Kotlin polls and honors the device's preferred ADPF update rate;
6. enables power-efficiency preference on API 35+ once thermal status reaches `MODERATE`;
7. closes the ADPF session at `SEVERE` or hotter instead of fighting platform thermal throttling;
8. samples process CPU time, PSS, system available-memory/low-memory state, app background restriction, thermal status/headroom, logical processor count, and native indexing throughput at a low diagnostic cadence.

`Balanced` mode keeps the diagnostic/tracing surface but skips the priority and ADPF throughput hints.

The runtime logs one privacy-safe line roughly every five seconds under tag `FrameScopePerf`. `cpu_core_eq_pct` is process CPU time divided by wall time and can exceed 100% on multicore hardware. It is deliberately not presented as the same number a vendor task manager reports.

Example:

```text
indexing op=7 backend=ffmpeg-avcodec fps=184.2 cpu_core_eq_pct=356.4 pss_mib=241 adpf=true thermal=1 headroom=0.42 low_memory=false background_restricted=false
```

No source URI, path, frame pixels, or user content is logged.

## Reproducible Perfetto capture

Requirements:

- Android 12+ recommended for the cleanest app tracing behavior;
- USB debugging or wireless ADB;
- `adb` available on the host;
- a reproducible test video and the same FrameScope build for repeated runs.

Start FrameScope and get to the point immediately before indexing. From the repository root run:

```bash
adb logcat -s FrameScopePerf:I '*:S'
```

Use a second terminal to capture 90 seconds:

```bash
adb shell perfetto --txt -c - \
  -o /data/misc/perfetto-traces/framescope-indexing.pftrace \
  < scripts/perfetto/framescope-indexing.pbtxt
```

Start indexing as soon as capture begins. When Perfetto exits, retrieve the trace without root:

```bash
mkdir -p artifacts/perfetto
adb shell cat /data/misc/perfetto-traces/framescope-indexing.pftrace \
  > artifacts/perfetto/framescope-indexing.pftrace
```

Open the file in the Perfetto UI. Keep the raw `.pftrace`, FrameScope commit SHA, device model/SoC, Android version, video fixture identity, battery level, and starting thermal state together with any benchmark result.

## What to inspect

### 1. Native/FFmpeg thread count and CPU ownership

Expand `com.framescope.app` in the Perfetto process list. Count TIDs that are runnable/running during the `FrameScope#indexing` slice and identify which ones consume CPU. Do not substitute `Thread.getAllStackTraces()` for native-thread evidence.

A useful PerfettoSQL starting point is:

```sql
INCLUDE PERFETTO MODULE sched.with_context;
SELECT
  thread_name,
  ROUND(SUM(dur) / 1e6, 1) AS cpu_ms
FROM sched_with_thread_process
WHERE process_name = 'com.framescope.app'
GROUP BY thread_name
ORDER BY cpu_ms DESC;
```

If most CPU time sits on one coordinator TID plus a small codec pool, the roughly 50% owner observation is consistent with insufficient parallel work. If many worker TIDs are runnable but not scheduled, investigate scheduling/thermal/frequency instead.

### 2. Runnable versus blocked time

```sql
SELECT
  t.name AS thread_name,
  ts.state,
  ROUND(SUM(ts.dur) / 1e6, 1) AS state_ms
FROM thread_state ts
JOIN thread t ON t.utid = ts.utid
JOIN process p ON p.upid = t.upid
WHERE p.name = 'com.framescope.app'
GROUP BY t.name, ts.state
ORDER BY state_ms DESC;
```

Large runnable (`R`) time without CPU residency points toward scheduler contention. Long sleep/uninterruptible periods, combined with `sched_blocked_reason`, point toward mutex, syscall, provider I/O, or storage waits. Correlate those periods with the existing Rust decode/SQLite timers rather than assuming the cause.

### 3. CPU placement and frequency

The trace records `power/cpu_frequency`, `power/cpu_idle`, and a 250 ms cpufreq polling fallback. Inspect which CPU IDs run the hot FrameScope TIDs and their frequency tracks. On heterogeneous SoCs, higher-capacity clusters usually have a different maximum-frequency envelope; use the device trace rather than hard-coded core numbers.

Perfetto's CPU-cycle standard-library table can help:

```sql
INCLUDE PERFETTO MODULE linux.cpu.frequency;
SELECT
  t.name AS thread_name,
  c.cpu,
  ROUND(c.runtime / 1e6, 1) AS runtime_ms,
  c.avg_freq,
  c.max_freq
FROM cpu_cycles_per_thread_per_cpu c
JOIN thread t ON t.utid = c.utid
JOIN process p ON p.upid = t.upid
WHERE p.name = 'com.framescope.app'
ORDER BY runtime_ms DESC;
```

### 4. Thermal behavior

Correlate the `FrameScope thermal status` counter and `FrameScopePerf` headroom logs with indexing FPS and CPU frequency. Android documents `SEVERE` and hotter as thermally significant performance throttling states. `getThermalHeadroom()` is sampled no more often than every 10 seconds because the platform explicitly warns that faster polling can return `NaN`.

A performance regression that appears only after frequency drops and thermal status/headroom worsens is not fixed by demanding still more scheduler priority. The workload or architecture needs to become more efficient.

### 5. Memory pressure

Correlate `FrameScope process PSS KiB`, `availableMemoryBytes`, `lowMemory`, and the latest `onTrimMemory` level with cache telemetry owned by Agent 3. A 12 GB physical-RAM device does not imply an app should allocate most of that RAM; Android's app heap/headroom and low-memory signals remain authoritative.

### 6. App/background constraints

`backgroundRestricted=true` means Android can aggressively restrict work when the app is backgrounded. Capture benchmark traces with FrameScope foreground and the screen state documented. Do not mask background restrictions with privileged scheduling changes.

## A/B acceptance procedure

For each source fixture, capture `main` and this branch under the same conditions. At minimum record:

- total indexing wall time and frames/s;
- process CPU core-equivalent percentage;
- number of materially active native/FFmpeg TIDs;
- per-thread CPU runtime and runnable/blocked time;
- CPU IDs/frequencies used by the hot threads;
- Rust decode and SQLite timing counters already produced by the indexing layer;
- PSS and low-memory/trim state;
- thermal status/headroom at start, midpoint, and end;
- whether ADPF was supported and active.

Do not claim a throughput win from CI or synthetic JVM tests. Physical-device acceptance remains required.

## Interpreting likely outcomes

- **One/few hot threads, little runnable delay, decode-heavy:** throughput/decoder architecture belongs to Agents 7 and 4. More scheduler tuning will not manufacture parallel decode work.
- **Many runnable workers, low CPU residency/frequency, cool device:** Agent 11 scheduling/ADPF behavior is relevant; compare A/B traces.
- **Many runnable workers, falling frequency, high thermal state/headroom:** reduce sustained power/work or use a more efficient decode path; do not fight thermal policy.
- **Long blocked/uninterruptible states:** correlate with provider reads and SQLite timing before changing worker count.
- **Low memory / trim callbacks during acceleration:** coordinate with Agent 3's cache budget rather than increasing resident memory blindly.

## Ownership dependencies

- Android MediaCodec/hardware decode: **Agent 4**.
- JNI zero-copy/frame transfer: **Agent 5**.
- index pipeline/acceleration structure: **Agent 6**.
- indexing concurrency/throughput architecture and FFmpeg worker policy: **Agent 7**.
- final physical integration baselines and benchmark gates: **Agent 12**.

Agent 11 should provide scheduler evidence and ordinary Android performance coordination, not duplicate those subsystems.

## Authoritative platform references

- Android `PerformanceHintManager` and `PerformanceHintManager.Session` API references.
- Android `Process` thread-priority API reference.
- Android ADPF Thermal API guidance.
- Android `ActivityManager.MemoryInfo` and `isBackgroundRestricted` API references.
- Perfetto CPU scheduling, CPU frequency, trace analysis, and CLI documentation.
