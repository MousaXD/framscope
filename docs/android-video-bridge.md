# Android ↔ Rust video bridge

Phase 2 keeps Android storage access, application state, and the Rust decoder boundary deliberately narrow.

## Source access

FrameScope uses Android's Storage Access Framework through `ActivityResultContracts.OpenDocument` and accepts the returned `content://` URI. The application does not request broad storage permissions and does not attempt to resolve a content URI into a filesystem path.

The source flow is:

1. Android's document picker returns a `content://` URI.
2. `ContentResolver.openFileDescriptor(uri, "r")` returns a `ParcelFileDescriptor`.
3. Kotlin passes only the borrowed integer descriptor and an operation ID to `framescope-ffi`.
4. `framescope-ffi` creates a borrowed descriptor view for the duration of the JNI call.
5. `VideoDecoder` / the FFmpeg backend immediately duplicates the descriptor and owns/closes only that duplicate.
6. Android continues to own the original `ParcelFileDescriptor`, which is closed by Kotlin's `use` block.

This ownership split prevents double-close bugs and keeps multi-gigabyte media streaming. No full-file `ByteArray`, RAM copy, or app-private duplicate is created.

## Threading

`AndroidFrameScopeRepository` performs URI queries, descriptor opening, and the blocking JNI call on `Dispatchers.IO`. Compose and the main thread only observe `StateFlow` updates.

The repository stores only the application `ContentResolver`; it does not retain an `Activity` or other short-lived UI context.

## UI state

The ViewModel uses explicit states rather than independent booleans:

- `Idle`
- `Picking`
- `Opening`
- `Inspecting`
- `Ready(video)`
- `Error(message, diagnostic)`
- `Cancelled`

Engine availability is tracked independently as `Checking`, `Ready(version)`, or `Unavailable(message)`.

Every inspection is assigned a monotonically increasing generation. Progress/results from an older generation are ignored after cancellation or replacement, preventing a slow native call from publishing stale data into a newer UI state.

## Cancellation

Picker cancellation is represented as `Cancelled`, not as an error.

An active native inspection also receives a monotonically increasing operation ID. `AndroidFrameScopeRepository.cancelActiveInspection()` forwards that ID through the existing JNI bridge before cancelling the coroutine job. Rust maps the operation ID to the video engine's cloneable `CancellationToken`, which is observed by FFmpeg's interrupt callback and by the decoder loop.

The token registry also preserves a cancellation that races just ahead of native inspection startup: a pre-cancelled token is reused when that operation enters the JNI call. Completed operations remove their registry entry. The registry is bounded against an accumulation of pre-start cancellation tombstones.

`AndroidFrameScopeRepository` deliberately rethrows `CancellationException` rather than wrapping it in `Result.failure`, so lifecycle cancellation cannot turn into an ordinary media error.

## Metadata contract

The JNI inspection path opens the FFmpeg-backed `VideoDecoder`; it does not use the Phase 1 ISO-BMFF metadata parser for Android inspection.

The Android bridge receives:

- `width`
- `height`
- `rotation_degrees`
- `duration_us` when available
- `estimated_frame_rate` when available
- `container`
- `codec`
- `video_stream_index`
- `video_stream_count`
- `audio_stream_count`
- `pixel_format` when available
- `variable_frame_rate` when variation is actually observed in decoded presentation timestamps

FPS remains informational only. Frame timing in the Rust engine is driven by presentation timestamps and stream time bases, never by `frame_number / fps`.

The JNI inspection samples only a small bounded number of decoded frames. A differing presentation interval is sufficient to report `variable_frame_rate=true`; a constant bounded prefix is not sufficient to prove the entire source is CFR, so the bridge leaves that field unknown instead of reporting `false`. A future full timeline/index pass may establish a whole-source CFR classification without changing the timestamp model.

The bounded inspection does not retain frame buffers or build a frame cache. The Rust response remains the authority; Kotlin validates and displays the response but does not parse the container or duplicate video metadata extraction.

## Error mapping

Stable native error codes are mapped to user-facing categories:

- unsupported format/codec → unsupported video
- missing video stream → no video track
- malformed container/data/metadata → corrupt media
- decoder failure → decoder failure
- I/O or invalid source → unreadable source
- revoked permission → permission error
- cancellation → coroutine cancellation

Detailed native diagnostics are retained for logs/state diagnostics, while ordinary UI messages avoid exposing raw FFmpeg numeric error codes.

## Phase 2 integration invariants

- `framescope-video` remains Android-independent.
- `framescope-ffi` is the only Android JNI boundary.
- Android owns and closes the original SAF descriptor.
- FFmpeg owns and closes only its duplicated descriptor.
- No URI-to-filesystem-path conversion is introduced.
- No whole-video copy or unbounded buffering is introduced.
- No frame buffers cross JNI in this Phase 2 metadata/control bridge.
