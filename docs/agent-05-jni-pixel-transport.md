# Agent 5 JNI and pixel transport audit

Branch: `agent-05/jni-zero-copy`

Audited production base: `d2fb242744c9b29503aaaf01cbc52f4446ec18d8`

This report covers Rust ↔ JNI ↔ Kotlin pixel transport only. It does not change frame identity, frame ordering, VFR timestamp truth, PTS/DTS semantics, cancellation authority, persistent-index correctness, source-quality extraction, similarity semantics, or storage safety.

## Root causes verified

1. Full-resolution decoded pixels are materialized by FFmpeg into a Rust `Vec<u8>` through `framescope_ffmpeg_copy_current_frame_rgba` and `sws_scale`.
2. The FFmpeg shim currently creates and frees an `SwsContext` for every RGBA snapshot. This is safe but causes avoidable conversion-context setup churn. A future narrow follow-up can retain a session-owned cached swscale context using FFmpeg's `sws_getCachedContext` contract.
3. `OwnedRgbaFrame` uses `Arc<[u8]>`, so normal cache hits and `.clone()` operations share immutable storage and do not copy the RGBA payload.
4. Cache admission from the decoder `Vec<u8>` into `Arc<[u8]>` currently performs another allocation/full payload relocation. That backing-store redesign belongs to Agent 3 because it changes cache ownership.
5. A downscaled scrub-preview miss allocates an output `Vec<u8>`, fills it, then converts it into `OwnedRgbaFrame`. Cache insertion and cache lookup after that are Arc ownership clones rather than pixel copies.
6. Before this branch, every live scrub request allocated a new direct JVM `ByteBuffer` sized to `maxEdge * maxEdge * 4`. At the default edge of 640, that is 1,638,400 bytes of direct allocation per request regardless of the actual preview dimensions.
7. Native preview delivery performs one full preview-payload copy into the JNI direct buffer.
8. Android display performs another full preview-payload copy from the direct buffer into an `ARGB_8888` Bitmap.
9. The generic UI path previously converted the live scrub buffer into a Bitmap after publication. This branch moves that conversion into the synchronous bridge so the JNI scratch buffer can be safely returned to a bounded pool before the result escapes into UI state.
10. Authoritative full-resolution frames still use a dedicated direct buffer. They are deliberately not pooled because Inspector/full-resolution presentation can retain them beyond the JNI call.

## Copy and allocation map

### Source-quality authoritative frame

| Stage | Allocation | Pixel copy / write | Notes |
| --- | --- | --- | --- |
| FFmpeg decode to `AVFrame` | FFmpeg-owned | decoder-owned | Reusable native `AVFrame`; no Java pointer exposure. |
| `AVFrame` → Rust RGBA `Vec<u8>` | 1 Rust Vec | full-frame `sws_scale` write | Current shim also allocates/frees an `SwsContext` per snapshot. |
| Rust `Vec<u8>` → cache `Arc<[u8]>` | 1 Arc backing allocation | full-frame relocation | Cache ownership dependency for Agent 3. |
| Cached frame clone | none for pixels | none | Arc refcount ownership only. |
| Prepared Rust RGBA → Android direct buffer | 1 direct buffer per authoritative frame | full-frame copy | Not pooled until Inspector lifetime has an explicit retirement contract. |
| Direct buffer → display Bitmap | 1 Bitmap | full display-payload copy | UI may additionally downsample very large authoritative frames for ordinary display; Inspector owns full-resolution rendering behavior. |

### Live scrub preview, cache miss

| Stage | Allocation | Pixel copy / write | Notes |
| --- | --- | --- | --- |
| Indexed target navigation | source-dependent | source-dependent | Exact persistent FrameId/timestamp reconciliation remains authoritative. |
| Source frame → downscaled preview Vec | 1 Vec when scaling/packing is needed | full preview generation | Nearest-neighbour preview only; never authoritative. |
| Preview Vec → `OwnedRgbaFrame` | Arc backing allocation | full preview relocation | Cache ownership dependency for Agent 3. |
| Preview cache insert/get | no pixel allocation after backing exists | none | Arc clone only. |
| Preview → JNI direct buffer | pool allocation only on cold/mismatched capacity | one full preview copy | Pool retains at most two best-fit direct buffers. |
| JNI direct buffer → Bitmap | 1 Bitmap | one full preview copy | Bitmap becomes the published display object. |
| Compose display | no FrameScope-managed pixel copy identified | none in this bridge | Bitmap is retired with Compose `DisposableEffect` cleanup. |

### Live scrub preview, preview-cache hit

The source decode and preview generation stages disappear. Remaining FrameScope-managed pixel movement is:

1. cached immutable preview → pooled JNI direct buffer;
2. pooled direct buffer → Bitmap.

After the pool warms, the direct transport allocation itself is reused. The Bitmap is still one allocation per published live preview until a stronger UI lifetime/pooling contract exists.

## Implemented safe optimizations

- Added a synchronized, bounded best-fit direct-buffer pool for live scrub JNI scratch transport.
- Active leases cannot be reused. A buffer returns to the pool only after the synchronous Bitmap copy finishes.
- No raw native pointer or mutable pooled buffer escapes into ViewModel/UI state.
- Live previews are published as their final Bitmap instead of publishing the JNI scratch ByteBuffer.
- Compose recycles a retired live-preview Bitmap only from `DisposableEffect.onDispose`, after that exact Bitmap leaves composition.
- Authoritative full-resolution buffers remain caller-owned and unpooled.

## Instrumentation

`FrameScopePixels` records only measured transport facts:

- direct-buffer allocations;
- direct-buffer reuse;
- direct bytes allocated;
- native-to-JVM payload bytes copied;
- Bitmap allocations;
- Bitmap allocated bytes;
- bytes copied into Bitmaps;
- JNI service time;
- Bitmap conversion/copy time.

Perfetto-compatible Android trace sections:

- `FrameScope.pixels.preview_jni`
- `FrameScope.pixels.preview_bitmap`
- `FrameScope.pixels.frame_jni`

Native swscale time is not currently split out from total JNI service time. No zero-valued placeholder is reported as though it were measured.

## Tests

Focused JVM tests cover:

- active direct-buffer leases never sharing storage;
- reuse only after lease close;
- idempotent close;
- refusal to return undersized retained buffers;
- retaining more useful larger capacity when the bounded pool is full;
- zero-retention behavior;
- preview descriptor/session/layout safety;
- deterministic byte/allocation/timing counter accounting.

Repository CI also exercises Rust format/clippy/tests, real FFmpeg fixtures, native Android arm64 build verification, Android unit tests, Android lint, Compose Android-test compilation, APK build/packaging, cache/index contracts, similarity contracts, and release/device-report contracts.

## Physical-device validation still required

Do not claim a speedup from this branch until a physical Android device captures:

- cold vs warm direct-buffer allocation counts;
- `FrameScope.pixels.preview_jni` P50/P95;
- `FrameScope.pixels.preview_bitmap` P50/P95;
- end-to-end scrub preview P50/P95;
- allocation/GC behavior during sustained scrub direction changes;
- process PSS/native heap before and after a long scrub session;
- exact settle latency after gesture release;
- visual correctness after rapid stale-request cancellation.

## Cross-agent dependencies

- **Agent 3, RAM/cache v2:** cache backing currently relocates decoder Vec storage into Arc backing. Any ownership change must preserve cache identity, byte accounting, eviction, and memory-pressure behavior.
- **Agent 4, Android hardware decode:** a proven Surface-backed non-authoritative preview path can bypass RGBA → JNI → Bitmap for coarse/live preview. It must not become authoritative indexing/extraction without metadata-equivalence proof.
- **Agent 9, Inspector:** authoritative full-resolution direct buffers must remain valid as long as Inspector can sample/render them. Pooling requires an explicit retirement or lease contract.
- **Agent 12, integration benchmarks:** use `FrameScopePixels` and Perfetto sections to establish physical allocation/copy cost and decide whether Bitmap reuse or a Surface path is worth further complexity.

## Remaining risks

- Per-preview Bitmap allocation remains and can still contribute to allocation churn.
- Full-resolution authoritative presentation still performs a full JNI copy and Bitmap copy.
- FFmpeg currently constructs/frees an `SwsContext` for every RGBA snapshot instead of retaining a session-owned cached context.
- A Surface path can reduce live-preview copies further, but belongs with hardware decode/presentation integration rather than being bolted onto the authoritative RGBA path.
