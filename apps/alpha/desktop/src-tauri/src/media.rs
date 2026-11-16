use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::Path,
};
use tauri::{Manager, State};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaItem {
    pub(crate) id: String,
    pub(crate) kind: &'static str,
    pub(crate) title: Option<String>,
    pub(crate) created_at: String,
    width: Option<u32>,
    height: Option<u32>,
    duration_sec: Option<f64>,
    pub(crate) path: String,
    poster_path: Option<String>,
    chat_id: Option<String>,
    pub(crate) by: Option<crate::archive_receipt::ChatItemBy>,
    pub(crate) evidence: Option<String>,
    pub(crate) how: Option<&'static str>,
    pub(crate) at: Option<String>,
    pub(crate) chats: Vec<crate::chat_items::ChatLink>,
}

fn classify(bytes: &[u8]) -> Option<(&'static str, Option<(u32, u32)>)> {
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

pub(crate) fn collect(
    root: &Path,
    database: &Path,
    kind: Option<&str>,
    before: Option<&str>,
    limit: usize,
) -> Result<Vec<MediaItem>, String> {
    if kind.is_some_and(|k| k != "image" && k != "video" && k != "audio") {
        return Err("invalid_kind".into());
    }
    let cursor = before
        .map(|value| {
            let (time, id) = value
                .split_once('|')
                .map_or((value, None), |(t, i)| (t, Some(i)));
            DateTime::parse_from_rfc3339(time)
                .map(|t| (t.with_timezone(&Utc), id))
                .map_err(|_| "invalid_before".to_string())
        })
        .transpose()?;
    let root_metadata = match fs::symlink_metadata(root) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("unavailable".into()),
    };
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err("unavailable".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if root_metadata.file_attributes() & 0x400 != 0 {
            return Err("unavailable".into());
        }
    }
    let root = root.canonicalize().map_err(|_| "unavailable".to_string())?;
    let db = if database.exists() {
        Some(
            Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|_| "unavailable".to_string())?,
        )
    } else {
        None
    };
    let mut items = Vec::new();
    for entry in WalkDir::new(&root)
        .follow_links(false)
        .min_depth(1)
        .into_iter()
    {
        let entry = entry.map_err(|_| "unavailable".to_string())?;
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(&root) else {
            continue;
        };
        let parts: Vec<_> = relative.components().collect();
        if parts.len() != 3 || parts[0].as_os_str() != "sha256" {
            continue;
        }
        let Some(hash) = parts[2].as_os_str().to_str() else {
            continue;
        };
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || parts[1].as_os_str() != &hash[..2]
        {
            continue;
        }
        let Ok(path) = entry.path().canonicalize() else {
            continue;
        };
        if !path.starts_with(&root) {
            continue;
        }
        let mut bytes = [0; 4096];
        let mut file = fs::File::open(&path).map_err(|_| "unavailable".to_string())?;
        let count = file
            .read(&mut bytes)
            .map_err(|_| "unavailable".to_string())?;
        let classification = classify(&bytes[..count]).or_else(|| {
            is_mp4_handler(&mut file, &bytes[..count], b"vide")
                .ok()
                .filter(|found| *found)
                .map(|_| ("video", None))
                .or_else(|| {
                    is_mp4_handler(&mut file, &bytes[..count], b"soun")
                        .ok()
                        .filter(|found| *found)
                        .map(|_| ("audio", None))
                })
        });
        let Some((detected, dimensions)) = classification else {
            continue;
        };
        if kind.is_some_and(|k| k != detected) {
            continue;
        }
        let metadata = file.metadata().map_err(|_| "unavailable".to_string())?;
        let created: DateTime<Utc> = match metadata.created() {
            Ok(time) => time.into(),
            Err(_) => continue,
        };
        let info: Option<(String, String, String)> = match &db {
            Some(conn) => conn.query_row(
                "SELECT original_file_name, created_at, integrity_state FROM momso_source_assets WHERE content_sha256 = ?1 ORDER BY created_at, id LIMIT 1",
                [hash], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .optional().map_err(|_| "unavailable".to_string())?,
            None => None,
        };
        if info.as_ref().is_some_and(|(_, _, state)| state != "active") {
            continue;
        }
        let timestamp = info
            .as_ref()
            .and_then(|(_, date, _)| {
                DateTime::parse_from_rfc3339(date)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            })
            .unwrap_or(created);
        let id = format!("sha256:{hash}");
        if let Some((time, tie_id)) = cursor {
            if timestamp > time || (timestamp == time && tie_id.is_none_or(|t| id.as_str() >= t)) {
                continue;
            }
        }
        let (width, height) = dimensions.map_or((None, None), |(w, h)| (Some(w), Some(h)));
        items.push((
            timestamp,
            MediaItem {
                id,
                kind: detected,
                title: info.map(|x| x.0),
                created_at: timestamp.to_rfc3339(),
                width,
                height,
                duration_sec: None,
                path: path.to_string_lossy().into_owned(),
                poster_path: None,
                chat_id: None,
                by: None,
                evidence: None,
                how: None,
                at: None,
                chats: Vec::new(),
            },
        ));
    }
    items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.id.cmp(&a.1.id)));
    Ok(items
        .into_iter()
        .take(limit.max(1))
        .map(|(_, item)| item)
        .collect())
}

#[tauri::command]
pub(crate) async fn media_list(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
    kind: Option<String>,
    before: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<MediaItem>, String> {
    let root = std::sync::Arc::new(state.paths.archive.current.clone().read_owned().await);
    let worker_root = root.clone();
    let _lease = root;
    let database = state.paths.database_path.clone();
    let items = tauri::async_runtime::spawn_blocking(move || {
        let mut items = collect(
            &worker_root.join("objects"),
            &database,
            kind.as_deref(),
            before.as_deref(),
            limit.unwrap_or(40).clamp(1, 100),
        )?;
        crate::chat_items::enrich_media(&worker_root, &database, &mut items)?;
        Ok::<_, String>(items)
    })
    .await
    .map_err(|_| "unavailable".to_string())??;
    for item in &items {
        app.asset_protocol_scope()
            .allow_file(&item.path)
            .map_err(|_| "unavailable".to_string())?;
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(root: &Path, n: u8, contents: &[u8]) {
        let hash = format!("{:064x}", n);
        let dir = root.join("sha256").join(&hash[..2]);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(hash), contents).unwrap();
    }
    fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut result = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        result.extend_from_slice(kind);
        result.extend_from_slice(body);
        result
    }
    fn mp4(handler: &[u8; 4]) -> Vec<u8> {
        let mut body = vec![0; 8];
        body.extend_from_slice(handler);
        let mdia = atom(b"mdia", &atom(b"hdlr", &body));
        let moov = atom(b"moov", &atom(b"trak", &mdia));
        let mut data = atom(b"ftyp", b"isom\0\0\0\0");
        data.extend(atom(b"mdat", &vec![0; 5000]));
        data.extend(moov);
        data
    }
    #[test]
    fn filters_and_returns_only_media() {
        let t = tempfile::tempdir().unwrap();
        fixture(t.path(), 1, b"\xff\xd8\xffjpeg");
        fixture(t.path(), 2, &mp4(b"vide"));
        fixture(t.path(), 3, b"not media");
        fixture(t.path(), 4, &mp4(b"soun"));
        let db = t.path().join("missing");
        assert_eq!(collect(t.path(), &db, None, None, 20).unwrap().len(), 3);
        assert_eq!(
            collect(t.path(), &db, Some("audio"), None, 20)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            collect(t.path(), &db, Some("image"), None, 20)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            collect(t.path(), &db, Some("video"), None, 20)
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn orders_and_paginates_without_losing_equal_timestamps() {
        let t = tempfile::tempdir().unwrap();
        for n in 1..=3 {
            fixture(t.path(), n, b"\xff\xd8\xffjpeg");
        }
        let db = t.path().join("missing");
        let all = collect(t.path(), &db, None, None, 10).unwrap();
        let first = collect(t.path(), &db, None, None, 1).unwrap();
        let cursor = format!("{}|{}", first[0].created_at, first[0].id);
        let next = collect(t.path(), &db, None, Some(&cursor), 10).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[1].id, next[0].id);
        assert_eq!(all[2].id, next[1].id);
    }
    #[test]
    fn missing_metadata_is_null_and_png_dimensions_are_read() {
        let t = tempfile::tempdir().unwrap();
        fixture(
            t.path(),
            1,
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x02\0\0\0\x03",
        );
        let items = collect(t.path(), &t.path().join("missing"), None, None, 4).unwrap();
        assert_eq!((items[0].width, items[0].height), (Some(2), Some(3)));
        assert!(
            items[0].title.is_none()
                && items[0].duration_sec.is_none()
                && items[0].poster_path.is_none()
                && items[0].chat_id.is_none()
        );
    }

    #[test]
    fn quarantine_is_not_returned_and_unreadable_index_is_not_ignored() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("index.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE momso_source_assets (id TEXT, content_sha256 TEXT, original_file_name TEXT, created_at TEXT, integrity_state TEXT);").unwrap();
        fixture(t.path(), 1, b"\xff\xd8\xffjpeg");
        let hash = format!("{:064x}", 1);
        conn.execute("INSERT INTO momso_source_assets VALUES ('id', ?1, 'photo.jpg', '2026-01-01T00:00:00Z', 'quarantined')", [&hash]).unwrap();
        assert!(collect(t.path(), &db, None, None, 4).unwrap().is_empty());
        conn.execute(
            "UPDATE momso_source_assets SET integrity_state = 'active'",
            [],
        )
        .unwrap();
        assert_eq!(collect(t.path(), &db, None, None, 4).unwrap().len(), 1);
        conn.execute_batch("DROP TABLE momso_source_assets;")
            .unwrap();
        assert!(collect(t.path(), &db, None, None, 4).is_err());
    }

    #[test]
    fn rejects_a_file_as_the_objects_root() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("objects");
        fs::write(&root, b"not a directory").unwrap();
        assert!(collect(&root, &t.path().join("missing"), None, None, 4).is_err());
    }

    #[test]
    fn reads_jpeg_and_gif_dimensions_without_guessing_truncated_values() {
        let jpeg =
            b"\xff\xd8\xff\xe0\x00\x04AB\xff\xc0\x00\x0b\x08\x00\x03\x00\x02\x01\x01\x11\x00";
        assert_eq!(classify(jpeg), Some(("image", Some((2, 3)))));
        assert_eq!(
            classify(b"GIF89a\x02\x00\x03\x00"),
            Some(("image", Some((2, 3))))
        );
        assert_eq!(
            classify(b"\xff\xd8\xff\xc0\x00\x0b\x08"),
            Some(("image", None))
        );
        assert_eq!(classify(b"GIF89a"), Some(("image", None)));
    }

    #[test]
    fn reads_webp_dimensions_without_guessing_truncated_values() {
        let extended = b"RIFF\x16\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0\x01\0\0\x02\0\0";
        let lossless = b"RIFF\x0d\0\0\0WEBPVP8L\x05\0\0\0\x2f\x01\x80\0\0";
        let lossy = b"RIFF\x16\0\0\0WEBPVP8 \x0e\0\0\0\0\0\0\x9d\x01\x2a\x02\0\x03\0";
        for bytes in [extended.as_slice(), lossless.as_slice(), lossy.as_slice()] {
            assert_eq!(classify(bytes), Some(("image", Some((2, 3)))));
            assert_eq!(classify(&bytes[..20]), Some(("image", None)));
        }
    }
}
