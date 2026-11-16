use std::{fs, io::Read, path::Path};

const MAX_SNAPSHOT_BYTES: u64 = 32 * 1024 * 1024;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncStatus {
    state: &'static str,
    pending_writes: i64,
    pending_deletions: i64,
}

#[tauri::command]
pub(crate) async fn chat_archive_sync_status(
    state: tauri::State<'_, crate::AppState>,
) -> Result<SyncStatus, &'static str> {
    let db = state.paths.database_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|_| "unavailable")?;
        let (writes, deletions): (i64, i64) = conn.query_row("SELECT count(*) FILTER (WHERE key GLOB 'chat_archive_write:*'), count(*) FILTER (WHERE key GLOB 'chat_archive_delete:*') FROM app_runtime_meta", [], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|_| "unavailable")?;
        Ok(SyncStatus { state: if writes + deletions == 0 { "saved" } else { "pending" }, pending_writes: writes, pending_deletions: deletions })
    }).await.map_err(|_| "unavailable")?
}

fn redaction_count(root: &Path, chat_id: &str) -> Result<Option<u64>, &'static str> {
    if !crate::archive_receipt::valid_chat_id(chat_id) {
        return Err("invalid_chat_id");
    }
    let directory = root.join("chats");
    let dir_meta = match fs::symlink_metadata(&directory) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("unavailable"),
    };
    if !dir_meta.is_dir() || dir_meta.file_type().is_symlink() {
        return Err("unavailable");
    }
    let path = directory.join(format!("{chat_id}.json"));
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("unavailable"),
    };
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > MAX_SNAPSHOT_BYTES {
        return Err("unavailable");
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "unavailable")?
        .take(MAX_SNAPSHOT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "unavailable")?;
    if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err("unavailable");
    }
    let data: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "unavailable")?;
    if data.get("version").and_then(|v| v.as_u64()) != Some(1)
        || data.pointer("/room/id").and_then(|v| v.as_str()) != Some(chat_id)
    {
        return Err("unavailable");
    }
    data.get("redactions")
        .and_then(|v| v.as_u64())
        .map(Some)
        .ok_or("unavailable")
}

#[tauri::command]
pub(crate) async fn chat_archive_redaction_count(
    state: tauri::State<'_, crate::AppState>,
    chat_id: String,
) -> Result<Option<u64>, String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    tauri::async_runtime::spawn_blocking(move || redaction_count(&root, &chat_id))
        .await
        .map_err(|_| "unavailable".to_string())?
        .map_err(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_count_without_accepting_invalid_paths_or_other_rooms() {
        let dir = tempfile::tempdir().unwrap();
        let id = "chat_20261115_120000";
        assert_eq!(redaction_count(dir.path(), id), Ok(None));
        assert_eq!(
            redaction_count(dir.path(), "../secret"),
            Err("invalid_chat_id")
        );
        fs::create_dir(dir.path().join("chats")).unwrap();
        let file = dir.path().join("chats").join(format!("{id}.json"));
        fs::write(
            &file,
            format!(r#"{{"version":1,"room":{{"id":"{id}"}},"redactions":3}}"#),
        )
        .unwrap();
        assert_eq!(redaction_count(dir.path(), id), Ok(Some(3)));
        fs::write(
            &file,
            r#"{"version":1,"room":{"id":"other"},"redactions":3}"#,
        )
        .unwrap();
        assert_eq!(redaction_count(dir.path(), id), Err("unavailable"));
    }
}
