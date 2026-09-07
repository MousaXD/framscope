# Android ↔ Rust video bridge

Phase 2 keeps Android storage access, application state, and the Rust decoder boundary deliberately narrow.

## Source access

FrameScope uses Android's Storage Access Framework through `ActivityResultContracts.OpenDocument` and accepts the returned `content://` URI. The application does not request broad storage permissions and does not attempt to resolve a content URI into a filesystem path.

The source flow is:

1. Android's document picker returns a `content://` URI.
2. `ContentResolver.openFileDescriptor(uri, "r")` returns a `ParcelFileDescriptor`.
3. Kotlin passes only the borrowed integer descriptor to `framescope-ffi`.
4. Rust immediately calls `dup(2)` and creates its own `File` from the duplicated descriptor.
5. Rust owns and closes only the duplicate. Android continues to own the original `ParcelFileDescriptor`, which is closed by Kotlin's `use` block.

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

An active inspection can be cancelled from the UI. Its coroutine is cancelled and the generation is invalidated immediately. `AndroidFrameScopeRepository` deliberately rethrows `CancellationException` rather than wrapping it in `Result.failure`, so lifecycle cancellation cannot turn into an ordinary media error.

The current Phase 1 native inspection entry point is synchronous. Coroutine cancellation therefore suppresses stale results immediately, but a native call already executing can only stop early when the Phase 2 Rust engine exposes cooperative cancellation. Agent 2 should connect its cancellation primitive at the `framescope-ffi` boundary rather than adding a second Android bridge technology.

Until that engine seam lands, the duplicated Rust descriptor remains owned by Rust for the duration of the native call and is released on return. Kotlin then closes the original `ParcelFileDescriptor`.

## Metadata contract

The Android bridge requires:

- `width`
- `height`
- `rotation_degrees` (defaults to `0` when absent)

It accepts the following fields when Rust can provide them:

- `duration_us`
- `estimated_frame_rate` or `nominal_frame_rate`
- `container`, `container_name`, or `container_format`
- `codec` or `codec_name`
- `video_stream_index` or `stream_index`
- `video_stream_count`
- `audio_stream_count`
- `pixel_format` or `pixel_format_name`
- `variable_frame_rate` or `is_variable_frame_rate`

Missing optional values are displayed as unknown or omitted. The UI explicitly labels FPS as nominal/estimated information and does not use it as a timing source.

The Rust response remains the authority. Kotlin does not parse the container or duplicate video metadata extraction.

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

## Agent 2 integration point

Agent 2 can evolve `framescope-core` / `framescope-video` metadata and cancellation primitives without Android dependencies. Agent 3's bridge should remain the only place that converts the Rust result envelope into Android application models.

When Agent 2's final API is available, the integrator should:

1. project the engine's selected stream/container/codec metadata into the JSON fields above;
2. wire the engine cancellation token through `framescope-ffi`;
3. preserve the existing `dup` ownership invariant;
4. keep frame buffers out of this Phase 2 metadata bridge.
