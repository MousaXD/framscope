use super::*;

pub(super) fn parse_minf_sample_count<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    end: u64,
) -> Result<Option<u64>, FrameScopeError> {
    let mut pos = start;
    while pos < end {
        let header = read_box_header(reader, pos, end)?;
        if &header.kind == b"stbl" {
            return parse_stbl_sample_count(reader, header.data_start, header.end);
        }
        pos = header.end;
    }
    Ok(None)
}

fn parse_stbl_sample_count<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    end: u64,
) -> Result<Option<u64>, FrameScopeError> {
    let mut pos = start;
    while pos < end {
        let header = read_box_header(reader, pos, end)?;
        if &header.kind == b"stts" {
            return Ok(Some(parse_stts_sample_count(reader, header)?));
        }
        pos = header.end;
    }
    Ok(None)
}

pub(super) fn parse_stts_sample_count<R: Read + Seek>(
    reader: &mut R,
    header: BoxHeader,
) -> Result<u64, FrameScopeError> {
    ensure_payload_len(header, 8)?;
    reader.seek(SeekFrom::Start(header.data_start + 4))?; // version/flags
    let entry_count = read_u32(reader)?;
    if entry_count > MAX_STTS_ENTRIES {
        return Err(FrameScopeError::MalformedContainer(format!(
            "stts entry count {entry_count} exceeds safety bound"
        )));
    }
    let required = 8u64
        .checked_add(entry_count as u64 * 8)
        .ok_or_else(|| FrameScopeError::MalformedContainer("stts size overflow".into()))?;
    ensure_payload_len(header, required)?;

    let mut total = 0u64;
    for _ in 0..entry_count {
        total = total
            .checked_add(read_u32(reader)? as u64)
            .ok_or_else(|| FrameScopeError::MalformedContainer("sample count overflow".into()))?;
        let _sample_delta = read_u32(reader)?;
    }
    Ok(total)
}

pub(super) fn parse_mvhd<R: Read + Seek>(
    reader: &mut R,
    header: BoxHeader,
) -> Result<Option<(u64, u32)>, FrameScopeError> {
    ensure_payload_len(header, 20)?;
    reader.seek(SeekFrom::Start(header.data_start))?;
    let version = read_u8(reader)?;
    match version {
        0 => {
            reader.seek(SeekFrom::Start(header.data_start + 12))?;
            let timescale = read_u32(reader)?;
            let duration = read_u32(reader)? as u64;
            Ok(Some((duration, timescale)))
        }
        1 => {
            ensure_payload_len(header, 32)?;
            reader.seek(SeekFrom::Start(header.data_start + 20))?;
            let timescale = read_u32(reader)?;
            let duration = read_u64(reader)?;
            Ok(Some((duration, timescale)))
        }
        other => Err(FrameScopeError::MalformedContainer(format!(
            "unsupported mvhd version {other}"
        ))),
    }
}

pub(super) fn parse_mdhd<R: Read + Seek>(
    reader: &mut R,
    header: BoxHeader,
) -> Result<Option<(u64, u32)>, FrameScopeError> {
    parse_mvhd(reader, header)
}

pub(super) fn parse_hdlr_is_video<R: Read + Seek>(
    reader: &mut R,
    header: BoxHeader,
) -> Result<bool, FrameScopeError> {
    ensure_payload_len(header, 12)?;
    reader.seek(SeekFrom::Start(header.data_start + 8))?;
    let mut kind = [0u8; 4];
    reader.read_exact(&mut kind)?;
    Ok(&kind == b"vide")
}

pub(super) fn parse_tkhd<R: Read + Seek>(
    reader: &mut R,
    header: BoxHeader,
) -> Result<(u32, u32, i32), FrameScopeError> {
    reader.seek(SeekFrom::Start(header.data_start))?;
    let version = read_u8(reader)?;
    let (matrix_offset, width_offset, required) = match version {
        0 => (40u64, 76u64, 84u64),
        1 => (52u64, 88u64, 96u64),
        other => {
            return Err(FrameScopeError::MalformedContainer(format!(
                "unsupported tkhd version {other}"
            )));
        }
    };
    ensure_payload_len(header, required)?;

    reader.seek(SeekFrom::Start(header.data_start + matrix_offset))?;
    let a = read_i32(reader)?;
    let b = read_i32(reader)?;
    let _u = read_i32(reader)?;
    let c = read_i32(reader)?;
    let d = read_i32(reader)?;
    let _v = read_i32(reader)?;
    let _x = read_i32(reader)?;
    let _y = read_i32(reader)?;
    let _w = read_i32(reader)?;
    let rotation = matrix_rotation(a, b, c, d).ok_or_else(|| {
        FrameScopeError::InvalidMetadata("video track uses an unsupported transform matrix".into())
    })?;

    reader.seek(SeekFrom::Start(header.data_start + width_offset))?;
    let width_fixed = read_u32(reader)?;
    let height_fixed = read_u32(reader)?;
    let width = width_fixed >> 16;
    let height = height_fixed >> 16;
    Ok((width, height, rotation))
}

pub(super) fn matrix_rotation(a: i32, b: i32, c: i32, d: i32) -> Option<i32> {
    if b == 0 && c == 0 && a > 0 && d > 0 {
        Some(0)
    } else if a == 0 && d == 0 && b > 0 && c < 0 {
        Some(90)
    } else if b == 0 && c == 0 && a < 0 && d < 0 {
        Some(180)
    } else if a == 0 && d == 0 && b < 0 && c > 0 {
        Some(270)
    } else {
        None
    }
}

pub(super) fn read_box_header<R: Read + Seek>(
    reader: &mut R,
    pos: u64,
    parent_end: u64,
) -> Result<BoxHeader, FrameScopeError> {
    if parent_end.saturating_sub(pos) < 8 {
        return Err(FrameScopeError::MalformedContainer(
            "truncated ISO BMFF box header".into(),
        ));
    }
    reader.seek(SeekFrom::Start(pos))?;
    let size32 = read_u32(reader)?;
    let mut kind = [0u8; 4];
    reader.read_exact(&mut kind)?;

    let (size, header_size) = match size32 {
        0 => (parent_end - pos, 8u64),
        1 => {
            if parent_end.saturating_sub(pos) < 16 {
                return Err(FrameScopeError::MalformedContainer(
                    "truncated extended-size box header".into(),
                ));
            }
            (read_u64(reader)?, 16u64)
        }
        value => (value as u64, 8u64),
    };

    if size < header_size {
        return Err(FrameScopeError::MalformedContainer(format!(
            "box {:?} is smaller than its header",
            String::from_utf8_lossy(&kind)
        )));
    }
    let end = pos
        .checked_add(size)
        .ok_or_else(|| FrameScopeError::MalformedContainer("box size overflow".into()))?;
    if end > parent_end {
        return Err(FrameScopeError::MalformedContainer(format!(
            "box {:?} extends beyond its parent",
            String::from_utf8_lossy(&kind)
        )));
    }

    Ok(BoxHeader {
        kind,
        data_start: pos + header_size,
        end,
    })
}

fn ensure_payload_len(header: BoxHeader, needed: u64) -> Result<(), FrameScopeError> {
    if header.end.saturating_sub(header.data_start) < needed {
        return Err(FrameScopeError::MalformedContainer(format!(
            "box {:?} is truncated",
            String::from_utf8_lossy(&header.kind)
        )));
    }
    Ok(())
}

fn read_u8<R: Read>(reader: &mut R) -> Result<u8, FrameScopeError> {
    let mut buf = [0u8; 1];
    reader.read_exact(&mut buf)?;
    Ok(buf[0])
}

fn read_u32<R: Read>(reader: &mut R) -> Result<u32, FrameScopeError> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_be_bytes(buf))
}

fn read_i32<R: Read>(reader: &mut R) -> Result<i32, FrameScopeError> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(i32::from_be_bytes(buf))
}

fn read_u64<R: Read>(reader: &mut R) -> Result<u64, FrameScopeError> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(u64::from_be_bytes(buf))
}
