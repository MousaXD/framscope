#![cfg(feature = "system-ffmpeg")]

use framescope_video::{
    CancellationToken, DecodedFrame, FrameScopeError, MediaKind, ObservedFrameRateMode,
    OpenOptions, VideoDecoder, VideoStreamSelection,
};
use std::fs::File;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../build/video-fixtures")
        .join(name);
    assert!(
        path.is_file(),
        "missing generated fixture {}; run scripts/generate-video-fixtures.sh",
        path.display()
    );
    path
}

fn decode_all(decoder: &mut VideoDecoder) -> Vec<DecodedFrame> {
    let mut frames = Vec::new();
    while let Some(frame) = decoder.next_frame().expect("fixture decode should succeed") {
        frames.push(frame);
    }
    frames
}

fn assert_codec_decodes(file: &str, codec: &str, expected_frames: usize) {
    let mut decoder = VideoDecoder::open_path(fixture(file)).expect("fixture should open");
    assert_eq!(decoder.selected_stream().codec.name, codec);
    let frames = decode_all(&mut decoder);
    assert_eq!(
        frames.len(),
        expected_frames,
        "unexpected frame count for {file}"
    );
    assert!(
        frames
            .iter()
            .all(|frame| frame.presentation_timestamp.is_some())
    );
    assert!(frames.iter().all(|frame| !frame.corrupt));
}

fn assert_probe_rejected(file: &str) {
    let error = match VideoDecoder::open_path(fixture(file)) {
        Ok(_) => panic!("hostile input {file} unexpectedly opened"),
        Err(error) => error,
    };
    assert!(
        matches!(
            error,
            FrameScopeError::MalformedContainer(_)
                | FrameScopeError::UnsupportedFormat(_)
                | FrameScopeError::InvalidSource(_)
        ),
        "hostile input {file} returned unexpected error {error:?}"
    );
}

#[test]
fn h264_cfr_uses_real_presentation_timestamps_and_stable_eof() {
    let mut decoder = VideoDecoder::open_path(fixture("h264-cfr.mp4")).unwrap();
    let info = decoder.info();
    assert_eq!(info.container.stream_count, 1);
    assert_eq!(decoder.selected_stream().codec.name, "h264");
    assert_eq!(decoder.selected_stream().width, Some(64));
    assert_eq!(decoder.selected_stream().height, Some(48));
    assert_eq!(
        decoder.selected_stream().pixel_format.as_deref(),
        Some("yuv420p")
    );
    let duration = info
        .container
        .duration_us
        .expect("MP4 duration should be known");
    assert!(duration.abs_diff(1_000_000) <= 80_000);

    let frames = decode_all(&mut decoder);
    assert_eq!(frames.len(), 12);
    assert_eq!(
        frames.iter().map(|frame| frame.index).collect::<Vec<_>>(),
        (0..12).collect::<Vec<_>>()
    );
    assert!(frames.iter().any(|frame| frame.keyframe));

    let timestamps = frames
        .iter()
        .map(|frame| frame.presentation_timestamp.unwrap())
        .collect::<Vec<_>>();
    assert!(timestamps.windows(2).all(|window| {
        window[0].time_base == window[1].time_base && window[0].ticks < window[1].ticks
    }));
    let deltas = timestamps
        .windows(2)
        .map(|window| window[1].ticks - window[0].ticks)
        .collect::<Vec<_>>();
    assert!(deltas.windows(2).all(|window| window[0] == window[1]));
    assert_eq!(
        decoder.observed_frame_rate_mode(),
        ObservedFrameRateMode::Constant
    );

    assert!(decoder.next_frame().unwrap().is_none());
    assert!(decoder.next_frame().unwrap().is_none());
}

#[test]
fn h264_vfr_preserves_non_uniform_timestamp_deltas() {
    let mut decoder = VideoDecoder::open_path(fixture("h264-vfr.mp4")).unwrap();
    let frames = decode_all(&mut decoder);
    assert_eq!(frames.len(), 8);

    let timestamps = frames
        .iter()
        .map(|frame| frame.presentation_timestamp.unwrap())
        .collect::<Vec<_>>();
    assert!(
        timestamps
            .windows(2)
            .all(|window| window[0].ticks < window[1].ticks)
    );
    let deltas = timestamps
        .windows(2)
        .map(|window| window[1].ticks - window[0].ticks)
        .collect::<Vec<_>>();
    assert!(deltas.windows(2).any(|window| window[0] != window[1]));
    assert_eq!(
        decoder.observed_frame_rate_mode(),
        ObservedFrameRateMode::Variable
    );
}

#[test]
fn configured_phase2_codecs_decode_real_frames() {
    assert_codec_decodes("hevc-cfr.mp4", "hevc", 6);
    assert_codec_decodes("vp9-cfr.webm", "vp9", 6);
    assert_codec_decodes("av1-cfr.mkv", "av1", 4);
}

#[test]
fn discovers_audio_without_confusing_it_with_selected_video() {
    let mut decoder = VideoDecoder::open_path(fixture("h264-with-audio.mp4")).unwrap();
    assert_eq!(decoder.info().streams.len(), 2);
    assert_eq!(
        decoder
            .info()
            .streams
            .iter()
            .filter(|stream| stream.media_kind == MediaKind::Video)
            .count(),
        1
    );
    assert_eq!(
        decoder
            .info()
            .streams
            .iter()
            .filter(|stream| stream.media_kind == MediaKind::Audio)
            .count(),
        1
    );
    assert_eq!(decode_all(&mut decoder).len(), 6);
}

#[test]
fn valid_audio_only_media_is_rejected_as_no_video_track() {
    let error = match VideoDecoder::open_path(fixture("audio-only.m4a")) {
        Ok(_) => panic!("audio-only media unexpectedly opened as video"),
        Err(error) => error,
    };
    assert!(matches!(error, FrameScopeError::NoVideoTrack));
}

#[test]
fn multiple_video_stream_selection_is_predictable_and_explicitly_overridable() {
    let path = fixture("multi-stream.mkv");
    let mut default_decoder = VideoDecoder::open_path(&path).unwrap();
    assert_eq!(default_decoder.info().selected_video_stream, 0);
    let first = default_decoder.next_frame().unwrap().unwrap();
    assert_eq!((first.width, first.height), (64, 48));

    let options = OpenOptions {
        stream_selection: VideoStreamSelection::Index(1),
    };
    let mut second_decoder =
        VideoDecoder::open_path_with_options(&path, options, CancellationToken::new()).unwrap();
    assert_eq!(second_decoder.info().selected_video_stream, 1);
    let first = second_decoder.next_frame().unwrap().unwrap();
    assert_eq!((first.width, first.height), (32, 24));
}

#[test]
fn exposes_rotation_and_unusual_dimensions() {
    let rotated = VideoDecoder::open_path(fixture("rotated-portrait.mp4")).unwrap();
    assert_eq!(rotated.selected_stream().rotation_degrees, Some(90));
    assert_eq!(rotated.selected_stream().width, Some(64));
    assert_eq!(rotated.selected_stream().height, Some(48));

    let mut unusual = VideoDecoder::open_path(fixture("unusual-dimensions.mp4")).unwrap();
    let frame = unusual.next_frame().unwrap().unwrap();
    assert_eq!((frame.width, frame.height), (62, 46));
}

#[test]
fn very_short_video_reaches_eof_after_one_frame() {
    let mut decoder = VideoDecoder::open_path(fixture("very-short.mp4")).unwrap();
    let first = decoder.next_frame().unwrap().unwrap();
    assert_eq!(first.index, 0);
    assert!(decoder.next_frame().unwrap().is_none());
}

#[test]
fn truncated_input_is_a_typed_malformed_error() {
    let error = match VideoDecoder::open_path(fixture("truncated.mp4")) {
        Ok(_) => panic!("truncated input unexpectedly opened"),
        Err(error) => error,
    };
    assert!(matches!(error, FrameScopeError::MalformedContainer(_)));
}

#[test]
fn empty_and_garbage_inputs_are_rejected_without_panics() {
    assert_probe_rejected("empty.bin");
    assert_probe_rejected("garbage.bin");
}

#[test]
fn truncated_payload_never_decodes_as_complete_healthy_source() {
    const HEALTHY_FRAME_COUNT: usize = 12;

    let mut decoder = match VideoDecoder::open_path(fixture("truncated-payload.mp4")) {
        Ok(decoder) => decoder,
        Err(error) => {
            assert!(matches!(
                error,
                FrameScopeError::MalformedContainer(_)
                    | FrameScopeError::InvalidSource(_)
                    | FrameScopeError::DecoderFailure(_)
                    | FrameScopeError::Io(_)
            ));
            return;
        }
    };

    let mut decoded = 0usize;
    loop {
        match decoder.next_frame() {
            Ok(Some(_frame)) => {
                decoded += 1;
                assert!(
                    decoded < HEALTHY_FRAME_COUNT,
                    "truncated payload produced the complete healthy frame sequence"
                );
            }
            Ok(None) => {
                assert!(
                    decoded < HEALTHY_FRAME_COUNT,
                    "truncated payload reached clean EOF after the full healthy frame count"
                );
                break;
            }
            Err(error) => {
                assert!(matches!(
                    error,
                    FrameScopeError::MalformedContainer(_)
                        | FrameScopeError::InvalidSource(_)
                        | FrameScopeError::DecoderFailure(_)
                        | FrameScopeError::Io(_)
                ));
                break;
            }
        }
    }
}

#[test]
fn cancellation_works_before_open_and_during_decode() {
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        VideoDecoder::open_path_with_options(
            fixture("h264-cfr.mp4"),
            OpenOptions::default(),
            token
        ),
        Err(FrameScopeError::Cancelled)
    ));

    let mut decoder = VideoDecoder::open_path(fixture("h264-cfr.mp4")).unwrap();
    decoder.cancel();
    assert!(matches!(
        decoder.next_frame(),
        Err(FrameScopeError::Cancelled)
    ));
}

#[cfg(unix)]
#[test]
fn fd_open_duplicates_without_moving_the_callers_file_offset() {
    use std::io::Seek;
    use std::os::fd::AsFd;

    let mut file = File::open(fixture("h264-cfr.mp4")).unwrap();
    assert_eq!(file.stream_position().unwrap(), 0);
    let mut decoder = VideoDecoder::open_file_descriptor(file.as_fd()).unwrap();
    assert_eq!(file.stream_position().unwrap(), 0);
    assert!(decoder.next_frame().unwrap().is_some());
    assert_eq!(file.stream_position().unwrap(), 0);
}

#[test]
fn seek_flushes_decoder_and_starts_a_new_decode_epoch() {
    let mut decoder = VideoDecoder::open_path(fixture("h264-cfr.mp4")).unwrap();
    let first = decoder.next_frame().unwrap().unwrap();
    assert_eq!(first.decode_epoch, 0);
    assert_eq!(first.index, 0);

    decoder.seek_to_timestamp_us(500_000).unwrap();
    let after_seek = decoder.next_frame().unwrap().unwrap();
    assert_eq!(after_seek.decode_epoch, 1);
    assert_eq!(after_seek.index, 0);
    assert!(after_seek.presentation_timestamp.is_some());
}
