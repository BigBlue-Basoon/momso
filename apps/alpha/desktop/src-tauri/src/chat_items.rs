use crate::archive_receipt::{ChatItemBy, ReceiptHead};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

const TEXT_PAGE_BYTES: u64 = 64 * 1024;
const ENCODING_CACHE_LIMIT: usize = 32;

// Text is stored by content hash. Remember a bounded number of detection results
// so requesting successive pages does not rescan a large file for every page.
type EncodingCache = HashMap<PathBuf, (u64, SystemTime, bool)>;
static ENCODING_CACHE: OnceLock<Mutex<EncodingCache>> = OnceLock::new();

fn is_utf8_stream(reader: &mut impl Read) -> std::io::Result<bool> {
    let mut block = vec![0u8; TEXT_PAGE_BYTES as usize + 4];
    let mut pending = 0;
    loop {
        let count = reader.read(&mut block[pending..])?;
        if count == 0 {
            return Ok(pending == 0);
        }
        let end = pending + count;
        match std::str::from_utf8(&block[..end]) {
            Ok(_) => pending = 0,
            Err(error) if error.error_len().is_some() => return Ok(false),
            Err(error) => {
                let valid = error.valid_up_to();
                pending = end - valid;
                block.copy_within(valid..end, 0);
            }
        }
    }
}

fn file_is_utf8(file: &mut fs::File, path: &Path) -> Result<bool, String> {
    let metadata = file.metadata().map_err(|_| "unavailable")?;
    let modified = metadata.modified().ok();
    let cache = ENCODING_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(modified) = modified {
        if let Ok(cache) = cache.lock() {
            if let Some((size, time, utf8)) = cache.get(path) {
                if *size == metadata.len() && *time == modified {
                    return Ok(*utf8);
                }
            }
        }
    }
    file.seek(SeekFrom::Start(0)).map_err(|_| "unavailable")?;
    let utf8 = is_utf8_stream(&mut *file).map_err(|_| "unavailable")?;
    let after = file.metadata().map_err(|_| "unavailable")?;
    if after.len() != metadata.len() || after.modified().ok() != modified {
        return Err("unavailable".into());
    }
    if let Some(modified) = modified {
        if let Ok(mut cache) = cache.lock() {
            if cache.len() >= ENCODING_CACHE_LIMIT {
                cache.clear();
            }
            cache.insert(path.to_path_buf(), (metadata.len(), modified, utf8));
        }
    }
    Ok(utf8)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttachmentTextPage {
    state: &'static str,
    content: Option<String>,
    next_cursor: Option<u64>,
    total_bytes: Option<u64>,
}

fn text_state(state: &'static str) -> AttachmentTextPage {
    AttachmentTextPage {
        state,
        content: None,
        next_cursor: None,
        total_bytes: None,
    }
}

#[cfg(windows)]
fn opened_path_matches(file: &fs::File, expected: &Path) -> bool {
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    let handle = file.as_raw_handle() as _;
    let size = unsafe { GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, 0) };
    if size == 0 || size > 32768 {
        return false;
    }
    let mut path = vec![0u16; size as usize + 1];
    let len = unsafe { GetFinalPathNameByHandleW(handle, path.as_mut_ptr(), path.len() as u32, 0) };
    if len == 0 || len as usize >= path.len() {
        return false;
    }
    let actual = std::ffi::OsString::from_wide(&path[..len as usize]);
    Path::new(&actual) == expected
}

#[cfg(not(windows))]
fn opened_path_matches(_file: &fs::File, _expected: &Path) -> bool {
    // This command is enabled only on Windows: other targets fail closed.
    false
}

#[cfg(windows)]
fn decode_cp949(bytes: &[u8]) -> Option<String> {
    use windows_sys::Win32::Globalization::{MultiByteToWideChar, MB_ERR_INVALID_CHARS};
    if bytes.is_empty() {
        return Some(String::new());
    }
    let len = i32::try_from(bytes.len()).ok()?;
    let count = unsafe {
        MultiByteToWideChar(
            949,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            len,
            std::ptr::null_mut(),
            0,
        )
    };
    if count == 0 {
        return None;
    }
    let mut wide = vec![0u16; count as usize];
    let written = unsafe {
        MultiByteToWideChar(
            949,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            len,
            wide.as_mut_ptr(),
            count,
        )
    };
    (written == count)
        .then(|| String::from_utf16(&wide).ok())
        .flatten()
}

#[cfg(not(windows))]
fn decode_cp949(_bytes: &[u8]) -> Option<String> {
    None
}

pub(crate) fn read_attachment_text(
    root: &Path,
    database: &Path,
    id: &str,
    cursor: u64,
) -> Result<AttachmentTextPage, String> {
    if !crate::archive_zet::valid_asset_id(id) {
        return Ok(text_state("not_found"));
    }
    if !events_impl(root, true)?
        .iter()
        .any(|e| e.attachment && e.head.read.iter().any(|item| item == id))
    {
        return Ok(text_state("not_found"));
    }
    let hash = id.strip_prefix("sha256:").ok_or("unavailable")?;
    let conn = Connection::open(database).map_err(|_| "unavailable")?;
    let name: Option<String> = conn.query_row("SELECT original_file_name FROM momso_source_assets WHERE content_sha256=?1 AND integrity_state='active'", [hash], |r| r.get(0)).optional().map_err(|_| "unavailable")?;
    let Some(name) = name else {
        return Ok(text_state("not_found"));
    };
    let extension = Path::new(&name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "txt"
            | "md"
            | "markdown"
            | "csv"
            | "tsv"
            | "json"
            | "jsonl"
            | "yaml"
            | "yml"
            | "xml"
            | "log"
    ) {
        return Ok(text_state("not_text"));
    }
    let archive = root.canonicalize().map_err(|_| "unavailable")?;
    let objects = root
        .join("objects")
        .canonicalize()
        .map_err(|_| "unavailable")?;
    if !objects.starts_with(&archive) {
        return Ok(text_state("not_found"));
    }
    let path = match objects
        .join("sha256")
        .join(&hash[..2])
        .join(hash)
        .canonicalize()
    {
        Ok(path) if path.starts_with(&objects) && path.is_file() => path,
        _ => return Ok(text_state("not_found")),
    };
    let mut file = fs::File::open(&path).map_err(|_| "unavailable")?;
    if !opened_path_matches(&file, &path) {
        return Ok(text_state("not_found"));
    }
    let total = file.metadata().map_err(|_| "unavailable")?.len();
    if cursor > total {
        return Ok(text_state("invalid_cursor"));
    }
    let utf8 = file_is_utf8(&mut file, &path)?;
    file.seek(SeekFrom::Start(cursor))
        .map_err(|_| "unavailable")?;
    let mut bytes = Vec::new();
    file.take(TEXT_PAGE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|_| "unavailable")?;
    let (content, consumed) = if utf8 {
        match std::str::from_utf8(&bytes) {
            Ok(value) => (value.to_owned(), bytes.len()),
            Err(error)
                if error.error_len().is_none()
                    && cursor + (bytes.len() as u64) < total
                    && error.valid_up_to() > 0 =>
            {
                (
                    std::str::from_utf8(&bytes[..error.valid_up_to()])
                        .map_err(|_| "unavailable")?
                        .to_owned(),
                    error.valid_up_to(),
                )
            }
            Err(_) => return Ok(text_state("invalid_cursor")),
        }
    } else {
        let full = decode_cp949(&bytes).map(|text| (text, bytes.len()));
        let partial = || {
            if cursor + (bytes.len() as u64) < total && bytes.len() > 1 {
                decode_cp949(&bytes[..bytes.len() - 1]).map(|text| (text, bytes.len() - 1))
            } else {
                None
            }
        };
        match full.or_else(partial) {
            Some(value) => value,
            None => return Ok(text_state("invalid_encoding")),
        }
    };
    if content
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Ok(text_state("not_text"));
    }
    let end = cursor + consumed as u64;
    Ok(AttachmentTextPage {
        state: "ok",
        content: Some(content),
        next_cursor: (end < total).then_some(end),
        total_bytes: Some(total),
    })
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ChatLink {
    id: String,
    title: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    id: String,
    title: String,
    summary: String,
    kind: String,
    status: String,
    updated_at: String,
    how: Option<&'static str>,
    by: Option<ChatItemBy>,
    evidence: Option<String>,
    at: String,
}

#[derive(Serialize)]
pub(crate) struct ChatItems {
    documents: Vec<Document>,
    media: Vec<ChatMedia>,
    files: Vec<AttachedFile>,
    deleted: Vec<DeletedItem>,
}

#[derive(Serialize)]
struct DeletedItem {
    id: String,
    how: &'static str,
    by: Option<ChatItemBy>,
    evidence: Option<String>,
    at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AttachedFile {
    id: String,
    title: String,
    created_at: String,
    how: &'static str,
    by: ChatItemBy,
    at: String,
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatMedia {
    id: String,
    kind: &'static str,
    title: Option<String>,
    created_at: String,
    how: Option<&'static str>,
    by: Option<ChatItemBy>,
    evidence: Option<String>,
    at: String,
    path: String,
}

struct Event {
    head: ReceiptHead,
    attachment: bool,
}

fn events(root: &Path) -> Result<Vec<Event>, String> {
    events_impl(root, false)
}

fn events_impl(root: &Path, restrict_to_archive: bool) -> Result<Vec<Event>, String> {
    let mut result = Vec::new();
    let safe_receipts = if restrict_to_archive {
        let archive = root.canonicalize().map_err(|_| "unavailable")?;
        let receipts = root
            .join("receipts")
            .canonicalize()
            .map_err(|_| "unavailable")?;
        if !receipts.starts_with(archive) {
            return Err("unavailable".into());
        }
        Some(receipts)
    } else {
        None
    };
    for entry in fs::read_dir(root.join("receipts")).map_err(|_| "unavailable")? {
        let entry = entry.map_err(|_| "unavailable")?;
        if !entry.file_type().map_err(|_| "unavailable")?.is_file()
            || entry.path().extension().is_none_or(|e| e != "md")
        {
            continue;
        }
        let text = if let Some(receipts) = &safe_receipts {
            let path = entry.path().canonicalize().map_err(|_| "unavailable")?;
            if path.parent() != Some(receipts.as_path()) {
                return Err("unavailable".into());
            }
            let mut file = fs::File::open(&path).map_err(|_| "unavailable")?;
            if !opened_path_matches(&file, &path) {
                return Err("unavailable".into());
            }
            let mut text = String::new();
            file.read_to_string(&mut text).map_err(|_| "unavailable")?;
            text
        } else {
            fs::read_to_string(entry.path()).map_err(|_| "unavailable")?
        };
        let (head, body) = crate::archive_receipt::parse(&text).map_err(|_| "unavailable")?;
        if entry.file_name() != format!("{}.md", head.id).as_str() {
            return Err("unavailable".into());
        }
        let attachment = head.tool == "person"
            && head.wrote.is_empty()
            && head.read.len() == 1
            && body
                .split_once("## 고친 내용")
                .is_some_and(|(_, part)| part.trim() == "원자료를 채팅방에 첨부했습니다.");
        result.push(Event { head, attachment });
    }
    result.sort_by(|a, b| {
        let time = |e: &Event| chrono::DateTime::parse_from_rfc3339(&e.head.created_at).ok();
        time(a)
            .cmp(&time(b))
            .then_with(|| a.head.id.cmp(&b.head.id))
    });
    Ok(result)
}

fn by(head: &ReceiptHead) -> ChatItemBy {
    if head.tool == "person" {
        ChatItemBy::User
    } else {
        ChatItemBy::Ai {
            provider: head.tool.clone(),
            model: (!head.model.is_empty()).then_some(head.model.clone()),
        }
    }
}

fn write_action(event: &Event, id: &str) -> Option<&'static str> {
    match event.head.write_actions.get(id).map(String::as_str) {
        Some("created") => Some("created"),
        Some("edited") => Some("edited"),
        Some("deleted") => Some("deleted"),
        _ => None,
    }
}
fn actor(event: &Event, id: &str) -> Option<ChatItemBy> {
    (!event.head.unknown_actor.iter().any(|value| value == id)).then(|| by(&event.head))
}

pub(crate) fn enrich_media(
    root: &Path,
    database: &Path,
    media: &mut [crate::media::MediaItem],
) -> Result<(), String> {
    let events = events(root)?;
    let conn = Connection::open(database).map_err(|_| "unavailable")?;
    for item in media {
        let mut chats = HashMap::<String, Option<String>>::new();
        for event in &events {
            let Some(chat) = &event.head.chat else {
                continue;
            };
            if !event.head.read.contains(&item.id) && !event.head.wrote.contains(&item.id) {
                continue;
            }
            let title: Option<String> = conn
                .query_row("SELECT title FROM chats WHERE id=?1", [chat], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(|_| "unavailable")?;
            chats.insert(chat.clone(), title);
            if event.attachment {
                item.how = Some("attached");
                item.by = Some(ChatItemBy::User);
                item.evidence = None;
                item.at = Some(event.head.created_at.clone());
            } else if let Some(action) = write_action(event, &item.id) {
                item.how = Some(action);
                item.by = actor(event, &item.id);
                item.evidence = event.head.evidence.get(&item.id).cloned();
                item.at = Some(event.head.created_at.clone());
            }
        }
        let mut links: Vec<_> = chats
            .into_iter()
            .map(|(id, title)| ChatLink { id, title })
            .collect();
        links.sort_by(|a, b| a.id.cmp(&b.id));
        item.chats = links;
    }
    Ok(())
}

pub(crate) fn list(root: &Path, database: &Path, chat_id: &str) -> Result<ChatItems, String> {
    let conn = Connection::open(database).map_err(|_| "unavailable")?;
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM chats WHERE id=?1)",
            [chat_id],
            |row| row.get(0),
        )
        .map_err(|_| "unavailable")?;
    if !exists {
        return Err("채팅방을 찾을 수 없습니다".into());
    }
    let events = events(root)?;
    let mut latest = HashMap::<String, (&Event, bool)>::new();
    let mut attached = HashMap::<String, &Event>::new();
    for event in &events {
        if event.head.chat.as_deref() != Some(chat_id) {
            continue;
        }
        for id in &event.head.read {
            latest.insert(id.clone(), (event, false));
            if event.attachment && crate::archive_zet::valid_asset_id(id) {
                attached.insert(id.clone(), event);
            }
        }
        for id in &event.head.wrote {
            latest.insert(id.clone(), (event, true));
        }
    }
    let mut documents = Vec::new();
    let mut deleted = Vec::new();
    let mut media_events = HashMap::new();
    for (id, (event, wrote)) in latest {
        if wrote && write_action(event, &id) == Some("deleted") {
            deleted.push(DeletedItem {
                id: id.clone(),
                how: "deleted",
                by: actor(event, &id),
                evidence: event.head.evidence.get(&id).cloned(),
                at: event.head.created_at.clone(),
            });
            continue;
        }
        if crate::archive_zet::valid_asset_id(&id) {
            media_events.insert(id, event);
            continue;
        }
        let path = root.join("zettels").join(format!("{id}.md"));
        let info = path.canonicalize().ok().and_then(|path| {
            let parent = root.join("zettels").canonicalize().ok()?;
            if !parent.starts_with(root.canonicalize().ok()?)
                || path.parent()? != parent
                || path.file_name()? != format!("{id}.md").as_str()
            {
                return None;
            }
            let text = fs::read_to_string(path).ok()?;
            let (head, _) = crate::archive_zet::parse(&text).ok()?;
            (head.id == id).then_some(head)
        });
        if let Some(head) = info {
            let how = if wrote {
                write_action(event, &id)
            } else {
                Some("read")
            };
            let evidence = event.head.evidence.get(&id).cloned();
            let by = actor(event, &id);
            documents.push(Document {
                id,
                title: head.title,
                summary: head.summary,
                kind: head.kind,
                status: head.status,
                updated_at: head.updated_at,
                how,
                by,
                evidence,
                at: event.head.created_at.clone(),
            });
        }
    }
    documents.sort_by(|a, b| {
        chrono::DateTime::parse_from_rfc3339(&b.at)
            .ok()
            .cmp(&chrono::DateTime::parse_from_rfc3339(&a.at).ok())
            .then_with(|| b.id.cmp(&a.id))
    });
    let files = crate::media::collect(&root.join("objects"), database, None, None, usize::MAX)?;
    let media_ids: std::collections::HashSet<String> =
        files.iter().map(|item| item.id.clone()).collect();
    let mut media = Vec::new();
    for item in files {
        if let Some(event) = attached
            .get(&item.id)
            .or_else(|| media_events.get(&item.id))
        {
            let how = if event.attachment {
                Some("attached")
            } else {
                write_action(event, &item.id)
            };
            let evidence = event.head.evidence.get(&item.id).cloned();
            let by = actor(event, &item.id);
            media.push(ChatMedia {
                id: item.id,
                kind: item.kind,
                title: item.title,
                created_at: item.created_at,
                how,
                by,
                evidence,
                at: event.head.created_at.clone(),
                path: item.path,
            });
        }
    }
    media.sort_by(|a, b| {
        chrono::DateTime::parse_from_rfc3339(&b.at)
            .ok()
            .cmp(&chrono::DateTime::parse_from_rfc3339(&a.at).ok())
            .then_with(|| b.id.cmp(&a.id))
    });
    let mut other_files = Vec::new();
    let object_root = root.join("objects").canonicalize().ok();
    for (id, event) in attached {
        let Some(object_root) = &object_root else {
            continue;
        };
        if media_ids.contains(&id) {
            continue;
        }
        let hash = id.strip_prefix("sha256:").ok_or("unavailable")?;
        let metadata: Option<(String, String)> = conn.query_row(
            "SELECT original_file_name,created_at FROM momso_source_assets WHERE content_sha256=?1 AND integrity_state='active'",
            [hash], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().map_err(|_| "unavailable")?;
        let Some((title, created_at)) = metadata else {
            continue;
        };
        let path = object_root.join("sha256").join(&hash[..2]).join(hash);
        let Ok(path) = path.canonicalize() else {
            continue;
        };
        if !path.starts_with(object_root) || !path.is_file() {
            continue;
        }
        other_files.push(AttachedFile {
            id,
            title,
            created_at,
            how: "attached",
            by: ChatItemBy::User,
            at: event.head.created_at.clone(),
            path: path.to_string_lossy().into_owned(),
        });
    }
    other_files.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.id.cmp(&a.id)));
    Ok(ChatItems {
        documents,
        media,
        files: other_files,
        deleted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf8_detection_is_streamed_across_character_boundaries() {
        let mut bytes = vec![b'a'; TEXT_PAGE_BYTES as usize + 3];
        bytes.extend_from_slice("요가".as_bytes());
        assert!(is_utf8_stream(&mut bytes.as_slice()).unwrap());
        bytes.extend_from_slice(&[0xb0, 0xa1]);
        assert!(!is_utf8_stream(&mut bytes.as_slice()).unwrap());
        assert!(!is_utf8_stream(&mut &[0xe3, 0x81][..]).unwrap());
    }
    #[cfg(windows)]
    #[test]
    fn opened_file_must_match_the_checked_path() {
        let temp = tempfile::tempdir().unwrap();
        let expected = temp.path().join("expected.txt");
        let other = temp.path().join("other.txt");
        fs::write(&expected, b"expected").unwrap();
        fs::write(&other, b"other").unwrap();
        let file = fs::File::open(&other).unwrap();
        assert!(!opened_path_matches(
            &file,
            &expected.canonicalize().unwrap()
        ));
        assert!(opened_path_matches(&file, &other.canonicalize().unwrap()));
    }
    #[cfg(windows)]
    #[test]
    fn attachment_text_pages_preserve_korean_and_reject_binary_and_unknown_ids() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        fs::create_dir_all(root.join("receipts")).unwrap();
        let db = temp.path().join("db.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE momso_source_assets(content_sha256 TEXT,integrity_state TEXT,original_file_name TEXT)",
        )
        .unwrap();
        let hash = "a".repeat(64);
        fs::create_dir_all(root.join("objects/sha256/aa")).unwrap();
        let path = root.join("objects/sha256/aa").join(&hash);
        let input = format!("{}요가", "a".repeat(TEXT_PAGE_BYTES as usize - 1));
        fs::write(&path, &input).unwrap();
        conn.execute(
            "INSERT INTO momso_source_assets VALUES (?1,'active','lesson.txt')",
            [&hash],
        )
        .unwrap();
        let id = format!("sha256:{hash}");
        let receipt = format!("---\nid: rec_20261115_120003\ncreated_at: '2026-11-15T12:00:03+09:00'\ntool: person\nmodel: ''\nchat: chat_20261115_120000\nread: [{id}]\nwrote: []\n---\n## 시킨 말\n없음\n## 고친 내용\n원자료를 채팅방에 첨부했습니다.\n");
        fs::write(root.join("receipts/rec_20261115_120003.md"), receipt).unwrap();
        let first = read_attachment_text(&root, &db, &id, 0).unwrap();
        assert_eq!(first.state, "ok");
        let second = read_attachment_text(&root, &db, &id, first.next_cursor.unwrap()).unwrap();
        assert_eq!(
            format!("{}{}", first.content.unwrap(), second.content.unwrap()),
            input
        );
        assert_eq!(second.next_cursor, None);
        assert_eq!(
            read_attachment_text(&root, &db, "../../outside", 0)
                .unwrap()
                .state,
            "not_found"
        );
        fs::write(&path, b"\0binary").unwrap();
        assert_eq!(
            read_attachment_text(&root, &db, &id, 0).unwrap().state,
            "not_text"
        );
        let mut korean = vec![b'a'; TEXT_PAGE_BYTES as usize - 1];
        korean.extend_from_slice(&[0xb0, 0xa1]);
        fs::write(&path, korean).unwrap();
        let first = read_attachment_text(&root, &db, &id, 0).unwrap();
        let second = read_attachment_text(&root, &db, &id, first.next_cursor.unwrap()).unwrap();
        assert_eq!(
            first.content.unwrap(),
            "a".repeat(TEXT_PAGE_BYTES as usize - 1)
        );
        assert_eq!(second.content.as_deref(), Some("가"));
        assert_eq!(second.next_cursor, None);
        let mut mixed = vec![0xc2, 0xa1]; // CP949: 징; the same bytes are valid UTF-8.
        mixed.extend(vec![b'a'; TEXT_PAGE_BYTES as usize - 2]);
        mixed.extend_from_slice(&[0xb0, 0xa1]);
        fs::write(&path, mixed).unwrap();
        let first = read_attachment_text(&root, &db, &id, 0).unwrap();
        let second = read_attachment_text(&root, &db, &id, first.next_cursor.unwrap()).unwrap();
        assert!(first.content.unwrap().starts_with("징"));
        assert_eq!(second.content.as_deref(), Some("가"));
        conn.execute(
            "UPDATE momso_source_assets SET original_file_name='lesson.pdf'",
            [],
        )
        .unwrap();
        fs::write(&path, b"plain looking content").unwrap();
        assert_eq!(
            read_attachment_text(&root, &db, &id, 0).unwrap().state,
            "not_text"
        );
    }

    #[cfg(windows)]
    #[test]
    fn cp949_text_is_decoded_without_splitting_a_korean_character() {
        let mut bytes = vec![b'a'; TEXT_PAGE_BYTES as usize - 1];
        bytes.extend_from_slice(&[0xb0, 0xa1]); // CP949: 가
        assert!(decode_cp949(&bytes[..TEXT_PAGE_BYTES as usize]).is_none());
        let text = decode_cp949(&bytes[..TEXT_PAGE_BYTES as usize - 1]).unwrap();
        assert_eq!(text.len(), TEXT_PAGE_BYTES as usize - 1);
        assert_eq!(decode_cp949(&bytes[bytes.len() - 2..]).unwrap(), "가");
        assert!(decode_cp949(&[0xff]).is_none());
    }
    #[test]
    fn recorded_references_are_scoped_and_unknown_write_kind_is_null() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        fs::create_dir_all(root.join("receipts")).unwrap();
        fs::create_dir_all(root.join("zettels")).unwrap();
        fs::write(root.join("zettels/zet_20261115_120000.md"), "---\nid: zet_20261115_120000\ntitle: 문서\nsummary: 요약\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T12:00:00+09:00'\nupdated_at: '2026-11-15T12:00:00+09:00'\nassets: []\nedges: []\n---\n본문\n").unwrap();
        fs::create_dir_all(root.join("objects/sha256/aa")).unwrap();
        let hash = "a".repeat(64);
        fs::write(
            root.join("objects/sha256/aa").join(&hash),
            b"RIFF\x00\x00\x00\x00WAVE",
        )
        .unwrap();
        let db = temp.path().join("db.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE chats(id TEXT PRIMARY KEY,title TEXT); CREATE TABLE archive_zets(id TEXT PRIMARY KEY,title TEXT,summary TEXT,kind TEXT,status TEXT,updated_at TEXT); CREATE TABLE momso_source_assets(id TEXT,content_sha256 TEXT,original_file_name TEXT,created_at TEXT,integrity_state TEXT); INSERT INTO chats VALUES('chat_20261115_120000','수업'); INSERT INTO chats VALUES('chat_20261115_120001','다른 방'); INSERT INTO archive_zets VALUES('zet_20261115_120000','문서','요약','note','draft','2026-11-15T12:00:00+09:00');").unwrap();
        let receipt = format!("---\nid: rec_20261115_120001\ncreated_at: '2026-11-15T12:00:01+09:00'\ntool: codex\nmodel: test-model\nchat: chat_20261115_120000\nread: [sha256:{hash}]\nwrote: [zet_20261115_120000]\n---\n## 시킨 말\n없음\n## 고친 내용\n없음\n");
        fs::write(root.join("receipts/rec_20261115_120001.md"), receipt).unwrap();
        let result =
            serde_json::to_value(list(&root, &db, "chat_20261115_120000").unwrap()).unwrap();
        assert_eq!(result["documents"][0]["how"], serde_json::Value::Null);
        assert_eq!(result["documents"][0]["by"]["model"], "test-model");
        assert_eq!(result["media"][0]["kind"], "audio");
        assert_eq!(result["media"][0]["how"], serde_json::Value::Null);
        let other =
            serde_json::to_value(list(&root, &db, "chat_20261115_120001").unwrap()).unwrap();
        assert!(other["documents"].as_array().unwrap().is_empty());
        assert!(other["media"].as_array().unwrap().is_empty());
        let mut items = crate::media::collect(&root.join("objects"), &db, None, None, 100).unwrap();
        enrich_media(&root, &db, &mut items).unwrap();
        let result = serde_json::to_value(&items).unwrap();
        assert_eq!(result[0]["chats"][0]["id"], "chat_20261115_120000");
        assert_eq!(result[0]["by"], serde_json::Value::Null);
        assert_eq!(result[0]["how"], serde_json::Value::Null);
        let attached = format!("---\nid: rec_20261115_120002\ncreated_at: '2026-11-15T12:00:02+09:00'\ntool: person\nmodel: ''\nchat: chat_20261115_120000\nread: [sha256:{hash}]\nwrote: []\n---\n## 시킨 말\n없음\n## 고친 내용\n원자료를 채팅방에 첨부했습니다.\n");
        fs::write(root.join("receipts/rec_20261115_120002.md"), attached).unwrap();
        let result =
            serde_json::to_value(list(&root, &db, "chat_20261115_120000").unwrap()).unwrap();
        assert_eq!(result["media"][0]["how"], "attached");
        assert_eq!(result["media"][0]["by"]["kind"], "user");
        let text_hash = "b".repeat(64);
        fs::create_dir_all(root.join("objects/sha256/bb")).unwrap();
        fs::write(root.join("objects/sha256/bb").join(&text_hash), "요가 전사").unwrap();
        conn.execute("INSERT INTO momso_source_assets VALUES ('asset-text',?1,'전사.txt','2026-11-15T12:00:03+09:00','active')", [&text_hash]).unwrap();
        let text_receipt = format!("---\nid: rec_20261115_120003\ncreated_at: '2026-11-15T12:00:03+09:00'\ntool: person\nmodel: ''\nchat: chat_20261115_120000\nread: [sha256:{text_hash}]\nwrote: []\n---\n## 시킨 말\n없음\n## 고친 내용\n원자료를 채팅방에 첨부했습니다.\n");
        fs::write(root.join("receipts/rec_20261115_120003.md"), text_receipt).unwrap();
        let list_value =
            serde_json::to_value(list(&root, &db, "chat_20261115_120000").unwrap()).unwrap();
        assert_eq!(list_value["files"][0]["title"], "전사.txt");
        assert_eq!(list_value["files"][0]["how"], "attached");
        assert!(
            serde_json::to_value(list(&root, &db, "chat_20261115_120001").unwrap()).unwrap()
                ["files"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        enrich_media(&root, &db, &mut items).unwrap();
        let result = serde_json::to_value(&items).unwrap();
        assert_eq!(result[0]["how"], "attached");
        assert_eq!(result[0]["by"]["kind"], "user");
        conn.execute("DELETE FROM chats WHERE id=?1", ["chat_20261115_120000"])
            .unwrap();
        enrich_media(&root, &db, &mut items).unwrap();
        let result = serde_json::to_value(&items).unwrap();
        assert_eq!(result[0]["chats"][0]["id"], "chat_20261115_120000");
        assert!(result[0]["chats"][0]["title"].is_null());
        assert!(root.join("receipts/rec_20261115_120002.md").exists());
    }
}
