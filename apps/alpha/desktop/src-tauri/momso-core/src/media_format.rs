//! Shared file signature recognition for media.
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
};

pub fn classify(bytes: &[u8]) -> Option<(&'static str, Option<(u32, u32)>)> {
    if (bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE")
        || bytes.starts_with(b"fLaC")
        || bytes.starts_with(b"ID3")
        || (bytes.starts_with(b"OggS")
            && (bytes.windows(8).any(|w| w == b"OpusHead")
                || bytes.windows(7).any(|w| w == b"\x01vorbis")))
    {
        return Some(("audio", None));
    }
    if bytes.len() >= 24 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" && &bytes[12..16] == b"IHDR" {
        let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        return Some((
            "image",
            (width > 0 && height > 0).then_some((width, height)),
        ));
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some(("image", jpeg_dimensions(bytes)));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        let dimensions = if bytes.len() >= 10 {
            let width = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
            let height = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
            (width > 0 && height > 0).then_some((width, height))
        } else {
            None
        };
        return Some(("image", dimensions));
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(("image", webp_dimensions(bytes)));
    }
    if bytes.starts_with(b"\x1a\x45\xdf\xa3")
        && bytes.windows(4).any(|w| w == b"webm")
        && bytes.windows(4).any(|w| w == b"V_VP" || w == b"V_AV")
    {
        return Some(("video", None));
    }
    None
}

pub type MediaInfo = (&'static str, Option<(u32, u32)>);

pub fn classify_file(file: &mut fs::File) -> Result<Option<MediaInfo>, String> {
    file.seek(SeekFrom::Start(0)).map_err(|_| "unavailable")?;
    let mut bytes = [0; 4096];
    let count = file.read(&mut bytes).map_err(|_| "unavailable")?;
    Ok(classify(&bytes[..count]).or_else(|| {
        is_mp4_handler(file, &bytes[..count], b"vide")
            .ok()
            .filter(|found| *found)
            .map(|_| ("video", None))
            .or_else(|| {
                is_mp4_handler(file, &bytes[..count], b"soun")
                    .ok()
                    .filter(|found| *found)
                    .map(|_| ("audio", None))
            })
    }))
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let kind = bytes.get(12..16)?;
    if kind == b"VP8X" {
        let data = bytes.get(24..30)?;
        let width = 1 + u32::from_le_bytes([data[0], data[1], data[2], 0]);
        let height = 1 + u32::from_le_bytes([data[3], data[4], data[5], 0]);
        return (u64::from(width) * u64::from(height) <= u64::from(u32::MAX))
            .then_some((width, height));
    }
    if kind == b"VP8L" && *bytes.get(20)? == 0x2f {
        let packed = u32::from_le_bytes(bytes.get(21..25)?.try_into().ok()?);
        if packed >> 29 != 0 {
            return None;
        }
        return Some(((packed & 0x3fff) + 1, ((packed >> 14) & 0x3fff) + 1));
    }
    if kind == b"VP8 " && bytes.get(23..26)? == b"\x9d\x01\x2a" {
        let width = u16::from_le_bytes(bytes.get(26..28)?.try_into().ok()?) as u32 & 0x3fff;
        let height = u16::from_le_bytes(bytes.get(28..30)?.try_into().ok()?) as u32 & 0x3fff;
        return (width > 0 && height > 0).then_some((width, height));
    }
    None
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    while i + 1 < bytes.len() {
        if bytes[i] != 0xff {
            return None;
        }
        while i < bytes.len() && bytes[i] == 0xff {
            i += 1;
        }
        let marker = *bytes.get(i)?;
        i += 1;
        if marker == 0xd9 || marker == 0xda {
            return None;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let length = u16::from_be_bytes([*bytes.get(i)?, *bytes.get(i + 1)?]) as usize;
        if length < 2 || i.checked_add(length)? > bytes.len() {
            return None;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            if length < 7 {
                return None;
            }
            let height = u16::from_be_bytes([bytes[i + 3], bytes[i + 4]]) as u32;
            let width = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
            return (width > 0 && height > 0).then_some((width, height));
        }
        i += length;
    }
    None
}

// Walk only the container boxes needed to locate a video handler; never read media payloads.
fn mp4_video_boxes(
    file: &mut fs::File,
    start: u64,
    end: u64,
    depth: u8,
    budget: &mut u32,
    handler: &[u8; 4],
) -> std::io::Result<bool> {
    let mut offset = start;
    while end.saturating_sub(offset) >= 8 && *budget > 0 {
        *budget -= 1;
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header)?;
        let size = u32::from_be_bytes(header[..4].try_into().unwrap());
        let mut header_size = 8u64;
        let box_size = if size == 1 {
            if end - offset < 16 {
                return Ok(false);
            }
            let mut extended = [0u8; 8];
            file.read_exact(&mut extended)?;
            header_size = 16;
            u64::from_be_bytes(extended)
        } else if size == 0 {
            end - offset
        } else {
            u64::from(size)
        };
        if box_size < header_size || box_size > end - offset {
            return Ok(false);
        }
        let body = offset + header_size;
        let box_end = offset + box_size;
        if &header[4..] == b"hdlr" && depth == 3 && box_end - body >= 12 {
            file.seek(SeekFrom::Start(body + 8))?;
            let mut kind = [0; 4];
            file.read_exact(&mut kind)?;
            if &kind == handler {
                return Ok(true);
            }
        }
        let expected_container = match depth {
            0 => b"moov",
            1 => b"trak",
            2 => b"mdia",
            _ => b"----",
        };
        if &header[4..] == expected_container
            && mp4_video_boxes(file, body, box_end, depth + 1, budget, handler)?
        {
            return Ok(true);
        }
        offset = box_end;
    }
    Ok(false)
}

fn is_mp4_handler(file: &mut fs::File, bytes: &[u8], handler: &[u8; 4]) -> std::io::Result<bool> {
    if bytes.len() < 12 || &bytes[4..8] != b"ftyp" {
        return Ok(false);
    }
    let len = file.metadata()?.len();
    mp4_video_boxes(file, 0, len, 0, &mut 20_000, handler)
}
