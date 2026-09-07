# Phase 2 Rust video engine

`framescope-video` owns FrameScope's platform-neutral video semantics. `framescope-ffmpeg` is a deliberately narrow native ownership boundary around FFmpeg resources and does not contain application policy.

## Data flow

```text
path or borrowed file descriptor
        |
        v
FFmpeg demuxer
        |
        v
deterministic video stream selection
        |
        v
FFmpeg decoder
        |
        v
DecodedFrame metadata + exact presentation timestamp
```

The decoder is streaming. It keeps one format context, one codec context, one reusable packet, and one reusable frame. It does not preload the source, predecode the source, or retain a full-resolution frame cache.

## Public Rust API

The primary entry point is `framescope_video::VideoDecoder`.

- `VideoDecoder::open_path(...)`
- `VideoDecoder::open_path_with_options(...)`
- `VideoDecoder::open_file_descriptor(...)` on Unix/Android
- `VideoDecoder::info()` for container and all-stream discovery
- `VideoDecoder::selected_stream()` for the chosen video stream
- `VideoDecoder::next_frame()` for sequential display-frame decoding
- `VideoDecoder::seek_to_timestamp_us(...)` for foundational timestamp/keyframe seeking
- `VideoDecoder::cancel()` and `CancellationToken` for cooperative cancellation
- `VideoDecoder::observed_frame_rate_mode()` for timestamp-derived CFR/VFR observation

Default video-stream selection is deterministic:

1. consider video streams that have a decoder enabled in the linked FFmpeg build;
2. prefer the first stream carrying `AV_DISPOSITION_DEFAULT`;
3. otherwise choose the lowest supported video stream index.

`VideoStreamSelection::Index` allows an explicit stream index without changing the default policy.

## Timestamp model

Frame number is never converted to time.

Each decoded frame can carry:

- the FFmpeg best-effort presentation timestamp in stream ticks;
- the exact stream time base;
- a checked integer microsecond convenience conversion;
- frame duration in the same time base when FFmpeg exposes it;
- a sequential `index` scoped to a `decode_epoch`.

`index` is identity/navigation data only. It is not timing data.

The engine uses `AVFrame.best_effort_timestamp`, falling back to `AVFrame.pts` only when needed. The timestamp is interpreted in `AVStream.time_base`. This preserves variable-frame-rate timing and avoids the invalid `frame_number / fps` assumption.

Average and nominal frame rates are exposed only as optional informational metadata. `ObservedFrameRateMode` compares actual decoded timestamp deltas and can change from constant to variable when a later delta differs.

## Frame access

Phase 2 intentionally does not copy decoded pixel planes into Rust-owned image buffers. Successful `AVFrame` output exposes dimensions, pixel-format metadata, keyframe state, corruption state, timestamps, and duration while the native layer reuses the underlying frame storage.

Phase 3 can add an index/cache above this API without undoing hidden full-video buffering.

## Seeking

`seek_to_timestamp_us` is a foundation, not an exact arbitrary-frame promise. FFmpeg seeks around timestamps/keyframes, the codec context is flushed, packet/frame state is cleared, and the next decoded frame starts a new `decode_epoch` with frame index zero.

A future Phase 3 index can use source identity, stream identity, decode epoch, frame index, exact timestamps, and keyframe markers to provide fast navigation.

## Android file descriptors

The native backend duplicates a supplied descriptor immediately. The caller retains ownership of the original descriptor.

For seekable descriptors, FrameScope uses `pread` with its own logical position. This avoids changing the caller's shared file offset. The duplicate is closed when the native session is destroyed.

For non-seekable descriptors, streaming reads are supported where the demuxer can operate without seeking. Exact seeking is not available for such sources. A cancellation request can interrupt FFmpeg when its interrupt callback runs, but it cannot forcibly abort a kernel read that is already blocked inside a non-seekable descriptor.

## Native ownership and unsafe-code review

The C shim owns:

- `AVFormatContext`
- `AVCodecContext`
- reusable `AVPacket`
- reusable `AVFrame`
- optional custom `AVIOContext` and its buffer
- the duplicated file descriptor and FD state

One cleanup path releases all resources on success, failure, and normal drop.

Rust unsafe code is limited to:

- C ABI calls;
- the cancellation callback's opaque `Arc<AtomicBool>` pointer;
- `Send` for the uniquely owned native session.

The cancellation pointer stays valid until after native close returns. Decoder mutation requires `&mut self`, so the FFmpeg contexts are not concurrently accessed through the Rust session. The session is `Send` but intentionally not `Sync`.

## Error model

User-controlled media is reported with typed errors rather than panics:

- unsupported format
- unsupported codec
- invalid source
- no video track
- malformed container/data
- decoder failure
- I/O failure
- cancellation
- seek unavailable
- backend unavailable

FFmpeg's diagnostic text is retained in the internal/native error before mapping to FrameScope's public categories.

## Actual Android FFmpeg capability

Agent 1 pins FFmpeg 9.0.1 for Android API 26+ on `arm64-v8a` and enables:

- decoders: H.264, HEVC/H.265, VP9, AV1;
- demuxers: MOV family (including MP4), Matroska family (including WebM), AVI;
- parsers: H.264, HEVC, VP9, AV1;
- protocols: `file`, `pipe`.

The Android build deliberately disables broad autodetection and unrelated FFmpeg components. Audio streams are still discoverable as container streams, but an audio codec such as AAC is not a supported Phase 2 Android decoder unless Agent 1's pinned configuration is expanded later.

Host integration tests use the `system-ffmpeg` feature solely to test the same C/Rust decoder API against Ubuntu's development libraries. Normal host builds do not acquire an FFmpeg dependency.

## Deferred work

Phase 2 does not implement:

- frame cache or persistent frame index;
- similarity/scene-change detection;
- extraction/export;
- full-resolution Rust pixel copies;
- exact instant arbitrary-frame seeking;
- Android rendering or Compose viewer controls.
