# Agent 03: RAM acceleration + cache v2 remediation

Branch: `agent-03/ram-cache-v2`

Audited production base: `d2fb242744c9b29503aaaf01cbc52f4446ec18d8`

## Scope

This remediation is limited to FrameScope's native RAM-cache policy, cache budgeting, source-frame cache, scrub-preview cache, eviction/accounting, Android memory-pressure behavior, and the user-facing RAM controls. It does not change frame identity, timestamp/index authority, exact ordering, VFR handling, PTS/DTS semantics, extraction source quality, persistent-index correctness, or decoder selection.

No root access, privileged Android permission, kernel/sysfs/proc tuning, thermal override, governor change, cpuset manipulation, swap/zram change, or privileged scheduling mechanism is used or required.

## Production root causes verified on current main

### 1. The UI/native hard ceiling was 512 MiB

`RamAccelerationPolicy` limited Custom to 512 MiB and Automatic to 256 MiB. JNI independently rejected a combined source + preview budget above 512 MiB.

The new native hard safety ceiling is 1 GiB, but this is only a ceiling. Caches populate lazily from real navigation/scrub demand and are further limited by device class, live available-memory headroom, Android low-memory state, and pressure callbacks.

### 2. Automatic incorrectly treated Java `memoryClass` as a native-cache ceiling

The source/preview caches are Rust/native allocations, while Android's `ActivityManager.getMemoryClass()` describes the approximate per-app managed heap class. The old Automatic policy divided that managed-heap number by three and included it in a `min(...)`, which could collapse an 8-12 GiB phone to a tiny native cache recommendation.

The new policy uses managed heap class only as part of reserved headroom for the rest of the app. Native cache targets are instead derived from physical RAM and live available-memory headroom, with low-RAM devices kept conservative.

Authoritative references:

- Android memory overview: https://developer.android.com/topic/performance/memory-overview
- `ActivityManager.MemoryInfo`: https://developer.android.com/reference/android/app/ActivityManager.MemoryInfo
- Android native memory / memory management guidance: https://developer.android.com/topic/performance/memory-management

### 3. The RAM controller configured the wrong cache namespace

`FrameScopeApplication` initialized RAM acceleration at `filesDir/framescope-cache`, while `MainActivity`, microscope navigation, scrub preview, and storage all use `cacheDir/framescope`.

That meant live shared source-cache resize/trim could target a namespace the active microscope never used. Global budget atomics partially masked this for future sessions, but an open session's source cache could remain unaffected by a settings or pressure change.

The controller now uses exactly `cacheDir/framescope`.

### 4. Preview residency had a hidden 12-frame ceiling

The scrub preview cache was byte-bounded but also count-bounded to only 12 entries. A normal 640x360 RGBA preview is about 0.88 MiB, so 12 entries are only about 10.5 MiB. Raising a preview byte budget well beyond that could therefore leave most of the budget permanently unused.

The secondary count safety guard is now 1,024 entries while the byte budget remains authoritative. This does not preallocate 1,024 frames.

### 5. RAM acceleration does not accelerate the initial indexing pass

The persistent indexing loop records authoritative frame metadata. The source-quality and scrub-preview RAM caches are populated by post-index exact navigation and scrub decode paths. Therefore the old product wording could make a larger budget look ineffective during indexing even though the caches were simply not being populated by that workload.

The UI now states this explicitly: RAM acceleration is for post-index navigation/scrubbing. Index-throughput architecture remains owned by the indexing/throughput agents.

### 6. The old source/preview split over-weighted full-resolution RGBA

The old policy reserved 75% for source-quality frames and 25% for previews. A 1920x1080 RGBA frame is about 7.9 MiB, so large source budgets can turn into relatively few full-resolution frames while a small preview cache provides poor timeline coverage.

The new policy assigns 30% to source-quality exact-frame locality and 70% to lower-resolution preview coverage. This is still a workload hypothesis that must be validated on device, but it avoids spending most of a large budget on a small pile of full-resolution RGBA frames.

## Existing copy behavior verified

Rust `OwnedRgbaFrame` pixel storage is `Arc<[u8]>`; cache hits clone the Arc rather than cloning the large pixel allocation. Existing cache tests already verify pointer reuse.

There is still a JNI preview handoff copy into Android's direct `ByteBuffer`, and the Kotlin preview bridge currently allocates a direct buffer for a render request. Buffer ownership/pooling/zero-copy JNI work belongs to Agent 05 and is intentionally not redesigned here.

## Adaptive modes

### Automatic

- physical target: up to 1/16 of physical RAM
- hard mode ceiling: 768 MiB
- further limited by live available-memory headroom
- low-RAM device target: 32 MiB

### Aggressive

- opt-in mode
- physical target: up to 1/12 of physical RAM
- hard mode ceiling: 1 GiB
- further limited by live available-memory headroom
- low-RAM device target: 64 MiB

### Custom

- saved preference can range from 16 MiB to 1 GiB
- device-stable slider maximum is also limited to 1/8 of physical RAM
- low-RAM device maximum is 128 MiB
- active budget can be temporarily reduced below the saved request when current Android memory headroom is insufficient

## Memory pressure and recovery

The controller now reacts to trim levels as ranges rather than exact-value equality. UI-hidden/background/critical pressure can synchronously shrink cache ceilings, and shrinking native ceilings evicts disposable cached pixels immediately.

On foreground entry, FrameScope refreshes Android memory state and restores the configured ceiling only if Android no longer reports low memory. It does not poll `ActivityManager.MemoryInfo` continuously.

Android documents trim levels as ranges and deprecates several old `RUNNING_*` callbacks on modern Android, so the implementation does not depend on receiving those callbacks exclusively:

- https://developer.android.com/reference/android/content/ComponentCallbacks2

## Visible metrics added

RAM settings now expose:

- requested budget
- active budget
- source/preview budget split
- actual cache-owned resident bytes
- source resident bytes / frames
- preview resident bytes / frames
- source and preview hit/miss rate
- source and preview evictions
- pressure-reduction count

The displayed residency intentionally counts cache-owned RGBA payload bytes only. It does not mislabel decoder buffers, JNI handoff buffers, Android Bitmaps, Java/Kotlin objects, or total process PSS as cache residency.

## Correctness and safety

- Source-quality cache identity and persistent index reconciliation are unchanged.
- Full-resolution cache hits still return the immutable source-quality allocation only when the existing `FrameCacheKey` identity matches.
- Scrub previews remain non-authoritative and remain keyed by `(FrameId, max_edge)`.
- Byte-bounded LRU eviction remains authoritative for both source and preview caches.
- Budget shrink continues to evict immediately.
- No disk proxy is promoted to source-quality extraction.
- No change is made to timestamp seek-safety checks, VFR truth, PTS/DTS handling, frame ordering, cancellation priority, or extraction provenance.
- No eager GiB-scale allocation is introduced.

## Tests added/updated

Kotlin/JVM:

- automatic native-cache recommendation is no longer hard-capped by Java heap class
- 12 GiB profile can receive 768 MiB Automatic / 1 GiB Aggressive or Custom ceiling
- low-live-headroom behavior
- conservative low-RAM behavior
- Off = zero budget
- 1 GiB Custom cap and 30/70 split
- Custom request preserved while active headroom clamps it
- pressure scaling and system low-memory scaling
- native metrics parser accepts real counters and rejects malformed/negative counters

Rust:

- root-scoped shared source-cache stats without opening/allocating another cache
- native 1 GiB safety ceiling
- actual root-scoped preview residency reporting
- existing process-wide preview-budget, zero-budget trim, retained-session, Arc-sharing, eviction, and cache-identity tests remain intact

## Physical-device validation still required

No device performance improvement is claimed by this change. A physical Android run should capture, for Off / Automatic / Aggressive / representative Custom:

1. requested vs active budget
2. source/preview resident bytes over a scrub/navigation trace
3. hit/miss/eviction counters
4. process PSS/native heap alongside cache-owned residency
5. warm adjacent-frame latency and random exact seek latency
6. warm scrub-preview P50/P95
7. behavior after backgrounding and Android memory pressure
8. recovery after returning foreground
9. absence of OOM / LMKD kill across a longer session

Agent 12 should own final baseline/acceptance numbers.

## Remaining risks / dependencies

- Agent 05: JNI/direct-buffer ownership and remaining preview-copy elimination or pooling.
- Agent 01/02: prefetch trajectory and decoder-locality determine whether the larger preview/source budgets are actually populated with useful entries.
- Agent 06/07: initial indexing throughput is independent of these post-index RAM caches.
- Agent 12: device benchmarks must tune the 30/70 split and mode ceilings if telemetry shows poor hit efficiency or memory pressure.
- Large custom budgets intentionally remain disposable. Android may kill background processes under system pressure before any application can guarantee a callback, so no budget is treated as committed memory.
