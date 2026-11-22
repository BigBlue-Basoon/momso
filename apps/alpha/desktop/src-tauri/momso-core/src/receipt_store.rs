//! Read existing desktop-format work records without following links.
use crate::receipt_format::{self, ReceiptHead};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

fn directory(root: &Path) -> Result<std::path::PathBuf, &'static str> {
    let path = root.join("receipts");
    let meta = fs::symlink_metadata(&path).map_err(|_| "storage_unavailable")?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("unsafe_path");
    }
    Ok(path)
}
fn file(directory: &Path, name: &str) -> Result<File, &'static str> {
    let path = directory.join(name);
    let metadata = fs::symlink_metadata(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "not_found"
        } else {
            "storage_unavailable"
        }
    })?;
    if !metadata.is_file()
        || crate::archive_location::is_link_or_reparse(&metadata)
        || metadata.len() > 1024 * 1024
    {
        return Err("unsafe_path");
    }
    #[cfg(unix)]
    let file = {
        use std::os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::OpenOptionsExt,
        };
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(directory)
            .map_err(|_| "unsafe_path")?;
        let name = std::ffi::CString::new(name).map_err(|_| "unsafe_path")?;
        // The directory descriptor anchors the lookup and prevents link traversal.
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err("unsafe_path");
        }
        // openat returned a new descriptor owned by this function.
        unsafe { File::from_raw_fd(fd) }
    };
    #[cfg(not(unix))]
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|_| "storage_unavailable")?;
    let meta = file.metadata().map_err(|_| "storage_unavailable")?;
    if !meta.is_file() || meta.len() > 1024 * 1024 {
        return Err("unsafe_path");
    }
    Ok(file)
}
pub fn read(root: &Path, id: &str) -> Result<(ReceiptHead, String), &'static str> {
    if !receipt_format::valid_id(id) {
        return Err("invalid_id");
    }
    let dir = directory(root)?;
    let mut text = String::new();
    file(&dir, &format!("{id}.md"))?
        .take(1024 * 1024 + 1)
        .read_to_string(&mut text)
        .map_err(|_| "invalid_record")?;
    if text.len() > 1024 * 1024 {
        return Err("invalid_record");
    }
    let (head, body) = receipt_format::parse(&text).map_err(|_| "invalid_record")?;
    if head.id != id {
        return Err("invalid_record");
    }
    Ok((head, body.to_owned()))
}
pub fn list(root: &Path) -> Result<(Vec<ReceiptHead>, usize), &'static str> {
    let mut result = Vec::new();
    let mut unreadable = 0usize;
    for entry in fs::read_dir(directory(root)?).map_err(|_| "storage_unavailable")? {
        let entry = entry.map_err(|_| "storage_unavailable")?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            unreadable += 1;
            continue;
        };
        let Some(id) = name.strip_suffix(".md") else {
            continue;
        };
        if !receipt_format::valid_id(id) {
            unreadable += 1;
            continue;
        }
        match read(root, id) {
            Ok((head, _)) => result.push(head),
            Err(_) => unreadable += 1,
        }
        if result.len() + unreadable > 100_000 {
            return Err("record_limit");
        }
    }
    result.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    Ok((result, unreadable))
}

/// Record a completed document operation without storing the document or prompt.
pub fn record_document(
    root: &Path,
    tool: &str,
    chat: Option<&str>,
    id: &str,
    action: &str,
) -> Result<String, &'static str> {
    if !matches!(tool, "person" | "codex" | "claude" | "gemini")
        || !crate::document::valid_zet_id(id)
        || !matches!(action, "created" | "edited")
        || chat.is_some_and(|id| !receipt_format::valid_chat_id(id))
    {
        return Err("invalid_arguments");
    }
    let body = if action == "created" {
        "문서를 만들었습니다."
    } else {
        "문서를 수정했습니다."
    };
    write_record(root, tool, chat, Some((id, action)), body)
}

/// Records only the fact of a successful operation; values entered on a site are never stored.
pub fn record_action(
    root: &Path,
    tool: &str,
    chat: Option<&str>,
    action: &str,
) -> Result<String, &'static str> {
    if !matches!(tool, "person" | "codex" | "claude" | "gemini")
        || chat.is_some_and(|id| !receipt_format::valid_chat_id(id))
    {
        return Err("invalid_arguments");
    }
    let body = match action {
        "browser_click" => "사이트에서 항목을 선택했습니다.",
        "browser_fill" => "사이트에 내용을 입력했습니다.",
        "crm_upsert" => "수업이나 예약 정보를 정리했습니다.",
        "asset_upload" => "첨부파일을 보관했습니다.",
        "asset_trash" => "원자료를 휴지통으로 옮겼습니다.",
        "asset_restore" => "원자료를 휴지통에서 복구했습니다.",
        _ => return Err("invalid_arguments"),
    };
    write_record(root, tool, chat, None, body)
}

fn write_record(
    root: &Path,
    tool: &str,
    chat: Option<&str>,
    reference: Option<(&str, &str)>,
    body: &str,
) -> Result<String, &'static str> {
    let _lock = crate::lock::exclusive(root)?;
    let dir = directory(root)?;
    for _ in 0..10 {
        let now = chrono::Local::now();
        let record = format!("rec_{}", now.format("%Y%m%d_%H%M%S"));
        let head = ReceiptHead {
            id: record.clone(),
            created_at: now.to_rfc3339(),
            tool: tool.to_owned(),
            model: String::new(),
            chat: chat.map(str::to_owned),
            read: Vec::new(),
            wrote: reference
                .map(|(id, _)| vec![id.to_owned()])
                .unwrap_or_default(),
            write_actions: if chat.is_some() && reference.is_some() {
                let (id, action) = reference.ok_or("invalid_arguments")?;
                std::collections::BTreeMap::from([(id.to_owned(), action.to_owned())])
            } else {
                Default::default()
            },
            evidence: if tool == "person" || chat.is_none() || reference.is_none() {
                Default::default()
            } else {
                let (id, _) = reference.ok_or("invalid_arguments")?;
                std::collections::BTreeMap::from([(id.to_owned(), "tool".into())])
            },
            unknown_actor: Vec::new(),
        };
        let yaml = serde_yaml::to_string(&head).map_err(|_| "storage_unavailable")?;
        let text = format!(
            "---\n{}---\n## 시킨 말\n없음\n\n## 고친 내용\n{body}\n",
            yaml.strip_prefix("---\n").unwrap_or(&yaml)
        );
        let mut temp = tempfile::NamedTempFile::new_in(&dir).map_err(|_| "storage_unavailable")?;
        temp.write_all(text.as_bytes())
            .and_then(|_| temp.as_file().sync_all())
            .map_err(|_| "storage_unavailable")?;
        match temp.persist_noclobber(dir.join(format!("{record}.md"))) {
            Ok(_) => {
                #[cfg(unix)]
                fs::File::open(&dir)
                    .and_then(|f| f.sync_all())
                    .map_err(|_| "storage_unavailable")?;
                return Ok(record);
            }
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                std::thread::sleep(std::time::Duration::from_millis(1100))
            }
            Err(_) => return Err("storage_unavailable"),
        }
    }
    Err("record_busy")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_desktop_receipts_and_rejects_linked_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir(root.join("receipts")).unwrap();
        let id = "rec_20261122_120000";
        fs::write(root.join("receipts").join(format!("{id}.md")),"---\nid: rec_20261122_120000\ncreated_at: '2026-11-22T12:00:00Z'\ntool: codex\nmodel: fixture\nread: []\nwrote: []\n---\n## 시킨 말\n가짜\n\n## 고친 내용\n없음\n").unwrap();
        let (head, body) = read(root, id).unwrap();
        assert_eq!(head.id, id);
        assert!(body.contains("가짜"));
        assert_eq!(list(root).unwrap().0.len(), 1);
        assert!(read(root, "../outside").is_err());
        #[cfg(unix)]
        {
            let outside = root.join("outside");
            fs::write(&outside, "secret fixture").unwrap();
            std::os::unix::fs::symlink(&outside, root.join("receipts/rec_20261122_120001.md"))
                .unwrap();
            assert!(read(root, "rec_20261122_120001").is_err());
            assert_eq!(list(root).unwrap().1, 1);
        }
    }

    #[test]
    fn new_document_records_use_desktop_format() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir(root.join("receipts")).unwrap();
        let id = "zet_20261122_120000";
        let user = record_document(root, "person", None, id, "created").unwrap();
        let (head, body) = read(root, &user).unwrap();
        assert_eq!(head.tool, "person");
        assert_eq!(head.wrote, vec![id]);
        assert!(body.contains("문서를 만들었습니다."));
        let chat = format!("chat_{}", uuid::Uuid::new_v4());
        let ai = record_document(root, "codex", Some(&chat), id, "edited").unwrap();
        let (head, _) = read(root, &ai).unwrap();
        assert_eq!(head.chat.as_deref(), Some(chat.as_str()));
        assert_eq!(head.evidence[id], "tool");
    }

    #[test]
    fn action_receipt_contains_only_a_fixed_description() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("receipts")).unwrap();
        let chat = format!("chat_{}", uuid::Uuid::new_v4());
        let id = record_action(temp.path(), "codex", Some(&chat), "browser_fill").unwrap();
        let (head, body) = read(temp.path(), &id).unwrap();
        assert_eq!(head.chat.as_deref(), Some(chat.as_str()));
        assert!(head.wrote.is_empty());
        assert!(body.contains("사이트에 내용을 입력했습니다."));
        assert!(record_action(temp.path(), "codex", Some(&chat), "private input").is_err());
    }
}
