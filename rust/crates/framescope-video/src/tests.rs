use super::*;
use std::io::Cursor;

fn mp4_box(kind: &[u8; 4], payload: Vec<u8>) -> Vec<u8> {
    let size = u32::try_from(payload.len() + 8).unwrap();
    let mut out = Vec::new();
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(&payload);
    out
}

fn mvhd(timescale: u32, duration: u32) -> Vec<u8> {
    let mut p = vec![0u8; 20];
    p[12..16].copy_from_slice(&timescale.to_be_bytes());
    p[16..20].copy_from_slice(&duration.to_be_bytes());
    mp4_box(b"mvhd", p)
}

fn mdhd(timescale: u32, duration: u32) -> Vec<u8> {
    let mut p = vec![0u8; 20];
    p[12..16].copy_from_slice(&timescale.to_be_bytes());
    p[16..20].copy_from_slice(&duration.to_be_bytes());
    mp4_box(b"mdhd", p)
}

fn hdlr_video() -> Vec<u8> {
    let mut p = vec![0u8; 12];
    p[8..12].copy_from_slice(b"vide");
    mp4_box(b"hdlr", p)
}

fn tkhd(width: u32, height: u32, rotation: i32) -> Vec<u8> {
    let mut p = vec![0u8; 84];
    let (a, b, c, d) = match rotation {
        90 => (0i32, 0x10000, -0x10000, 0),
        180 => (-0x10000, 0, 0, -0x10000),
        270 => (0, -0x10000, 0x10000, 0),
        _ => (0x10000, 0, 0, 0x10000),
    };
    p[40..44].copy_from_slice(&a.to_be_bytes());
    p[44..48].copy_from_slice(&b.to_be_bytes());
    p[52..56].copy_from_slice(&c.to_be_bytes());
    p[56..60].copy_from_slice(&d.to_be_bytes());
    p[76..80].copy_from_slice(&(width << 16).to_be_bytes());
    p[80..84].copy_from_slice(&(height << 16).to_be_bytes());
    mp4_box(b"tkhd", p)
}

fn stts(sample_count: u32, delta: u32) -> Vec<u8> {
    let mut p = vec![0u8; 16];
    p[4..8].copy_from_slice(&1u32.to_be_bytes());
    p[8..12].copy_from_slice(&sample_count.to_be_bytes());
    p[12..16].copy_from_slice(&delta.to_be_bytes());
    mp4_box(b"stts", p)
}

fn synthetic_mp4() -> Vec<u8> {
    let ftyp = mp4_box(b"ftyp", b"isom\0\0\0\0isom".to_vec());
    let stbl = mp4_box(b"stbl", stts(300, 1000));
    let minf = mp4_box(b"minf", stbl);
    let mut mdia_payload = Vec::new();
    mdia_payload.extend(mdhd(30_000, 300_000));
    mdia_payload.extend(hdlr_video());
    mdia_payload.extend(minf);
    let mdia = mp4_box(b"mdia", mdia_payload);
    let mut trak_payload = tkhd(1920, 1080, 90);
    trak_payload.extend(mdia);
    let trak = mp4_box(b"trak", trak_payload);
    let mut moov_payload = mvhd(1_000, 10_000);
    moov_payload.extend(trak);
    let moov = mp4_box(b"moov", moov_payload);
    [ftyp, moov].concat()
}

#[test]
fn inspects_synthetic_mp4_without_decoding_frames() {
    let mut cursor = Cursor::new(synthetic_mp4());
    let metadata = inspect_video(&mut cursor).unwrap();
    assert_eq!(metadata.width, 1920);
    assert_eq!(metadata.height, 1080);
    assert_eq!(metadata.duration_us, 10_000_000);
    assert_eq!(metadata.rotation_degrees, 90);
    assert!((metadata.estimated_frame_rate.unwrap() - 30.0).abs() < 0.001);
}

#[test]
fn rejects_ambiguous_track_transform() {
    assert!(matrix_rotation(0x10000, 1, 0, 0x10000).is_none());
}

#[test]
fn rejects_random_bytes() {
    let mut cursor = Cursor::new(vec![0u8; 32]);
    assert!(matches!(
        inspect_video(&mut cursor),
        Err(FrameScopeError::MalformedContainer(_)) | Err(FrameScopeError::UnsupportedFormat(_))
    ));
}

#[test]
fn rejects_unbounded_stts_entry_count_before_looping() {
    let mut payload = vec![0u8; 8];
    payload[4..8].copy_from_slice(&(MAX_STTS_ENTRIES + 1).to_be_bytes());
    let bytes = mp4_box(b"stts", payload);
    let mut cursor = Cursor::new(bytes);
    let end = cursor.get_ref().len() as u64;
    let header = read_box_header(&mut cursor, 0, end).unwrap();

    assert!(matches!(
        parse_stts_sample_count(&mut cursor, header),
        Err(FrameScopeError::MalformedContainer(_))
    ));
}

#[test]
fn rejects_box_extending_beyond_file() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&100u32.to_be_bytes());
    bytes.extend_from_slice(b"ftyp");
    bytes.extend_from_slice(&[0u8; 4]);
    let mut cursor = Cursor::new(bytes);
    assert!(matches!(
        inspect_video(&mut cursor),
        Err(FrameScopeError::MalformedContainer(_))
    ));
}
