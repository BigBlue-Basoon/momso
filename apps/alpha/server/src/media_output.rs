//! Turn-owned output folders and generated media registration.
use super::*;
use momso_server_attachments::{GenerationInput, MAX_BYTES};
use std::{
    ffi::CString,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    os::fd::{AsRawFd, FromRawFd},
    path::{Component, Path as FsPath, PathBuf},
};

fn uuid(value: &str) -> Result<(), &'static str> {
    let id = uuid::Uuid::parse_str(value).map_err(|_| "invalid_arguments")?;
    if id.to_string() != value {
        return Err("invalid_arguments");
    }
    Ok(())
}

fn directory(path: &FsPath) -> Result<(), &'static str> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err("storage_unavailable"),
    }
    let meta = fs::symlink_metadata(path).map_err(|_| "storage_unavailable")?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("unsafe_location");
    }
    Ok(())
}

pub fn prepare(app: &App, chat: &str, turn: &str) -> Result<PathBuf, &'static str> {
    uuid(chat)?;
    uuid(turn)?;
    let root = app.data.join("archive");
    let meta = fs::symlink_metadata(&root).map_err(|_| "storage_unavailable")?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("unsafe_location");
    }
    let mut path = root;
    for part in [".momso", "outputs", chat, turn] {
        path.push(part);
        directory(&path)?;
    }
    path.canonicalize().map_err(|_| "unsafe_location")
}

fn open_beneath(root: &FsPath, path: &FsPath) -> Result<File, &'static str> {
    let relative = path.strip_prefix(root).map_err(|_| "unsafe_location")?;
    let parts = relative.components().collect::<Vec<_>>();
    if parts.is_empty()
        || parts.len() > 9
        || parts.iter().any(|p| !matches!(p, Component::Normal(_)))
    {
        return Err("unsafe_location");
    }
    use std::os::unix::fs::OpenOptionsExt;
    let mut current = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)
        .map_err(|_| "unsafe_location")?;
    for (index, part) in parts.iter().enumerate() {
        use std::os::unix::ffi::OsStrExt;
        let Component::Normal(name) = part else {
            return Err("unsafe_location");
        };
        let name = CString::new(name.as_bytes()).map_err(|_| "unsafe_location")?;
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | if index + 1 < parts.len() {
                libc::O_DIRECTORY
            } else {
                0
            };
        let fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err("unsafe_location");
        }
        current = unsafe { File::from_raw_fd(fd) };
    }
    if !current
        .metadata()
        .map_err(|_| "storage_unavailable")?
        .is_file()
    {
        return Err("unsafe_location");
    }
    Ok(current)
}

fn mime(file: &mut File, path: &FsPath) -> Result<Option<&'static str>, &'static str> {
    let ext = path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut head = [0u8; 16];
    file.seek(SeekFrom::Start(0))
        .map_err(|_| "storage_unavailable")?;
    let count = file.read(&mut head).map_err(|_| "storage_unavailable")?;
    let result = match ext.as_str() {
        "png" if head[..count].starts_with(b"\x89PNG\r\n\x1a\n") => Some(("image/png", "image")),
        "jpg" | "jpeg" if head[..count].starts_with(b"\xff\xd8\xff") => {
            Some(("image/jpeg", "image"))
        }
        "gif" if head[..count].starts_with(b"GIF87a") || head[..count].starts_with(b"GIF89a") => {
            Some(("image/gif", "image"))
        }
        "webp" if count >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" => {
            Some(("image/webp", "image"))
        }
        "mp4" | "mov" if count >= 12 && &head[4..8] == b"ftyp" => Some((
            if ext == "mp4" {
                "video/mp4"
            } else {
                "video/quicktime"
            },
            "video",
        )),
        "webm" if head[..count].starts_with(b"\x1a\x45\xdf\xa3") => Some(("video/webm", "video")),
        _ => None,
    };
    let kind = momso_core::media_format::classify_file(file)
        .map_err(|_| "storage_unavailable")?
        .map(|x| x.0);
    file.seek(SeekFrom::Start(0))
        .map_err(|_| "storage_unavailable")?;
    Ok(result
        .filter(|(_, expected)| Some(*expected) == kind)
        .map(|(mime, _)| mime))
}

pub fn register(
    app: &App,
    chat: &str,
    turn: &str,
    path: &FsPath,
    prompt: &str,
    prompt_source: &str,
    parent: Option<&str>,
) -> Result<Option<String>, &'static str> {
    if prompt.len() > 65536 {
        return Err("invalid_arguments");
    }
    let output = prepare(app, chat, turn)?;
    let mut file = open_beneath(&output, path)?;
    let Some(mime) = mime(&mut file, path)? else {
        return Ok(None);
    };
    let length = file.metadata().map_err(|_| "storage_unavailable")?.len();
    if length > MAX_BYTES {
        return Err("archive_limit");
    }
    let conn = db(app).map_err(|_| "storage_unavailable")?;
    let (message, provider, expected_parent): (String, String, Option<String>) = conn
        .query_row(
            "SELECT user_message_id,provider,regenerate_of FROM turns WHERE id=?1 AND chat_id=?2",
            params![turn, chat],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| "not_found")?;
    if parent.is_some() && parent != expected_parent.as_deref() {
        return Err("invalid_arguments");
    }
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("invalid_arguments")?;
    let store = super::attachment_bridge::store(app)?;
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer).map_err(|_| "storage_unavailable")?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let hash = format!("{:x}", digest.finalize());
    file.seek(SeekFrom::Start(0))
        .map_err(|_| "storage_unavailable")?;
    let asset = if let Some(existing) = store.find_hash(&hash)? {
        existing
    } else {
        let upload = store.begin(name, mime, length, None)?;
        store.mark_generated_upload(&upload.upload_id, chat, &message, &provider)?;
        let mut offset = 0u64;
        loop {
            let count = file.read(&mut buffer).map_err(|_| "storage_unavailable")?;
            if count == 0 {
                break;
            }
            store.write_chunk(&upload.upload_id, offset, &buffer[..count])?;
            offset += count as u64;
        }
        let (asset, _) = store.finish_with_created(&upload.upload_id)?;
        if asset.sha256 != hash {
            return Err("integrity_error");
        }
        asset
    };
    if let Some(existing) = store.generation(&asset.id)? {
        if existing.chat_id == chat
            && existing.message_id == message
            && existing.provider == provider
        {
            link(&conn, turn, chat, &asset.id, asset.byte_length)?;
            return Ok(Some(asset.id));
        }
        return Err("asset_already_exists");
    }
    if !store.owns_generated_asset(&asset.id, chat, &message, &provider)? {
        return Err("asset_already_exists");
    }
    let generation = store.record_generation(GenerationInput {
        asset_id: &asset.id,
        prompt,
        prompt_source,
        provider: &provider,
        model: None,
        created_at: &chrono::Utc::now().to_rfc3339(),
        chat_id: chat,
        message_id: &message,
        parent: expected_parent.as_deref(),
        how: None,
    })?;
    link(&conn, turn, chat, &generation.asset_id, asset.byte_length)?;
    Ok(Some(generation.asset_id))
}

fn link(
    conn: &rusqlite::Connection,
    turn: &str,
    chat: &str,
    asset: &str,
    bytes: u64,
) -> Result<(), &'static str> {
    let bytes = i64::try_from(bytes).map_err(|_| "invalid_arguments")?;
    conn.execute("INSERT OR IGNORE INTO generated_output_links(turn_id,chat_id,asset_id,byte_length) VALUES(?1,?2,?3,?4)",params![turn,chat,asset,bytes]).map_err(|_|"storage_unavailable")?;
    let stored:Option<i64>=conn.query_row("SELECT byte_length FROM generated_output_links WHERE turn_id=?1 AND chat_id=?2 AND asset_id=?3",params![turn,chat,asset],|r|r.get(0)).optional().map_err(|_|"storage_unavailable")?;
    if stored != Some(bytes) {
        return Err("integrity_error");
    }
    Ok(())
}

pub fn scan(app: &App, chat: &str, turn: &str) -> Result<usize, &'static str> {
    let root = prepare(app, chat, turn)?;
    let conn = db(app).map_err(|_| "storage_unavailable")?;
    let prompt:String=conn.query_row("SELECT m.text FROM turns t JOIN chat_messages m ON m.id=t.user_message_id AND m.chat_id=t.chat_id WHERE t.id=?1 AND t.chat_id=?2",params![turn,chat],|r|r.get(0)).map_err(|_|"not_found")?;
    let mut stack = vec![(root, 0usize)];
    let mut seen = 0usize;
    let mut count = 0;
    while let Some((dir, depth)) = stack.pop() {
        for entry in fs::read_dir(dir).map_err(|_| "storage_unavailable")? {
            let entry = entry.map_err(|_| "storage_unavailable")?;
            seen += 1;
            if seen > 10000 {
                return Err("scan_limit");
            }
            let kind = entry.file_type().map_err(|_| "storage_unavailable")?;
            if kind.is_dir() {
                if depth >= 8 {
                    return Err("scan_limit");
                }
                stack.push((entry.path(), depth + 1));
            } else if kind.is_file()
                && register(app, chat, turn, &entry.path(), &prompt, "userRequest", None)?.is_some()
            {
                count += 1;
            }
        }
    }
    Ok(count)
}

pub fn recover(app: &App) -> Result<(), &'static str> {
    let conn = db(app).map_err(|_| "storage_unavailable")?;
    let mut statement = conn
        .prepare("SELECT chat_id,id FROM turns WHERE output_pending='1' ORDER BY rowid")
        .map_err(|_| "storage_unavailable")?;
    let pending = statement
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|_| "storage_unavailable")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "storage_unavailable")?;
    drop(statement);
    drop(conn);
    for (chat, turn) in pending {
        if scan(app, &chat, &turn).is_ok() {
            db(app)
                .map_err(|_| "storage_unavailable")?
                .execute("UPDATE turns SET output_pending=NULL WHERE id=?1", [turn])
                .map_err(|_| "storage_unavailable")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    #[test]
    fn output_open_rejects_outside_and_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("output");
        fs::create_dir(&root).unwrap();
        let outside = temp.path().join("outside.png");
        fs::write(&outside, b"fixture").unwrap();
        symlink(&outside, root.join("link.png")).unwrap();
        assert!(open_beneath(&root, &outside).is_err());
        assert!(open_beneath(&root, &root.join("link.png")).is_err());
    }
}
