#![cfg(feature = "system-ffmpeg")]

use framescope_ffmpeg::{CancellationToken, Session};
use framescope_video::VideoDecoder;
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

#[test]
fn current_decoded_frame_copies_to_owned_tightly_packed_rgba() {
    let mut session = Session::open_path(&fixture("h264-cfr.mp4"), None, CancellationToken::new())
        .expect("fixture should open");

    assert!(session.copy_current_frame_rgba().is_err());

    let first = session
        .next_frame()
        .expect("decode should succeed")
        .expect("fixture should contain a frame");
    assert_eq!((first.width, first.height), (64, 48));

    let rgba = session
        .copy_current_frame_rgba()
        .expect("decoded frame should convert to RGBA");
    assert_eq!((rgba.width, rgba.height), (64, 48));
    assert_eq!(rgba.stride, 64 * 4);
    assert_eq!(rgba.pixels.len(), rgba.stride * rgba.height as usize);

    let snapshot = rgba.pixels.clone();
    session
        .next_frame()
        .expect("second decode should succeed")
        .expect("fixture should contain another frame");
    assert_eq!(
        rgba.pixels, snapshot,
        "owned pixels changed after decoder advanced"
    );
}

#[test]
fn video_decoder_exposes_source_quality_owned_rgba_with_pts_metadata() {
    let mut decoder =
        VideoDecoder::open_path(fixture("h264-cfr.mp4")).expect("fixture should open");
    let first = decoder
        .next_frame_rgba()
        .expect("RGBA decode should succeed")
        .expect("fixture should contain a frame");

    assert_eq!(first.frame.index, 0);
    assert!(first.frame.presentation_timestamp.is_some());
    assert_eq!((first.frame.width, first.frame.height), (64, 48));
    assert_eq!(first.stride_bytes, 64 * 4);
    assert_eq!(
        first.pixels.len(),
        first.stride_bytes * first.frame.height as usize
    );

    let snapshot = first.pixels.clone();
    let second = decoder
        .next_frame_rgba()
        .expect("second RGBA decode should succeed")
        .expect("fixture should contain another frame");
    assert_eq!(second.frame.index, 1);
    assert_eq!(first.pixels, snapshot, "owned pixels changed after decode");
}

#[test]
fn seek_invalidates_the_previous_native_frame_copy_window() {
    let mut session = Session::open_path(&fixture("h264-cfr.mp4"), None, CancellationToken::new())
        .expect("fixture should open");

    session
        .next_frame()
        .expect("decode should succeed")
        .expect("fixture should contain a frame");
    session
        .copy_current_frame_rgba()
        .expect("current frame should be copyable");

    session.seek_us(0).expect("fixture should seek");
    assert!(
        session.copy_current_frame_rgba().is_err(),
        "seek must invalidate access to the pre-seek reusable AVFrame"
    );
}
