//! Save a verified media asset to a user-selected destination without overwrite.
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

pub(crate) fn prepare(
    root: &Path,
    database: &Path,
    asset_id: &str,
) -> Result<(PathBuf, String), String> {
    if !crate::archive_zet::valid_asset_id(asset_id) {
        return Err("invalid_asset".into());
    }
    let hash = &asset_id[7..];
    let conn = rusqlite::Connection::open(database).map_err(|_| "unavailable")?;
    let name: Option<String> = conn.query_row("SELECT a.original_file_name FROM momso_source_assets a WHERE a.content_sha256=?1 AND a.integrity_state='active' AND NOT EXISTS(SELECT 1 FROM source_asset_deletions d WHERE d.asset_id=a.id)", [hash], |r| r.get(0)).optional().map_err(|_| "unavailable")?;
    let name = name.ok_or("asset_not_found")?;
    let objects = root
        .join("objects")
        .canonicalize()
        .map_err(|_| "asset_not_found")?;
    let path = objects.join("sha256").join(&hash[..2]).join(hash);
    let meta = std::fs::symlink_metadata(&path).map_err(|_| "asset_not_found")?;
    if !meta.is_file()
        || crate::archive_location::is_link_or_reparse(&meta)
        || !path
            .canonicalize()
            .map_err(|_| "asset_not_found")?
            .starts_with(&objects)
    {
        return Err("asset_not_found".into());
    }
    let mut file = std::fs::File::open(&path).map_err(|_| "asset_not_found")?;
    #[cfg(windows)]
    if !crate::chat_items::opened_path_matches(
        &file,
        &path.canonicalize().map_err(|_| "asset_not_found")?,
    ) {
        return Err("asset_not_found".into());
    }
    if !crate::media::classify_file(&mut file)?
        .is_some_and(|(kind, _)| kind == "image" || kind == "video")
    {
        return Err("invalid_kind".into());
    }
    let safe_name = Path::new(&name)
        .file_name()
        .and_then(|v| v.to_str())
        .filter(|v| {
            !v.is_empty()
                && !v
                    .chars()
                    .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        })
        .unwrap_or("momso-media")
        .to_owned();
    Ok((path, safe_name))
}

fn copy_verified(
    source: &Path,
    asset_id: &str,
    destination: &Path,
    archive: &Path,
    data_root: &Path,
) -> Result<(), String> {
    if !crate::archive_zet::valid_asset_id(asset_id) || !destination.is_absolute() {
        return Err("invalid_destination".into());
    }
    let parent = destination
        .parent()
        .ok_or("invalid_destination")?
        .canonicalize()
        .map_err(|_| "invalid_destination")?;
    let archive = archive.canonicalize().map_err(|_| "unavailable")?;
    let data = data_root.canonicalize().map_err(|_| "unavailable")?;
    if parent.starts_with(&archive) || parent.starts_with(&data) {
        return Err("invalid_destination".into());
    }
    let name = destination.file_name().ok_or("invalid_destination")?;
    let output = parent.join(name);
    if output.exists() || std::fs::symlink_metadata(&output).is_ok() {
        return Err("destination_exists".into());
    }
    let mut source_file = std::fs::File::open(source).map_err(|_| "asset_not_found")?;
    #[cfg(windows)]
    if !crate::chat_items::opened_path_matches(
        &source_file,
        &source.canonicalize().map_err(|_| "asset_not_found")?,
    ) {
        return Err("asset_not_found".into());
    }
    let mut temp = tempfile::NamedTempFile::new_in(&parent).map_err(|_| "save_failed")?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 65_536];
    loop {
        let count = source_file.read(&mut buffer).map_err(|_| "save_failed")?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        temp.write_all(&buffer[..count])
            .map_err(|_| "save_failed")?;
    }
    if format!("{:x}", hasher.finalize()) != asset_id[7..] {
        return Err("asset_changed".into());
    }
    temp.as_file().sync_all().map_err(|_| "save_failed")?;
    temp.persist_noclobber(&output).map_err(|error| {
        if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            "destination_exists"
        } else {
            "save_failed"
        }
    })?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn media_save_as(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
    asset_id: String,
) -> Result<serde_json::Value, String> {
    let root = std::sync::Arc::new(state.paths.archive.current.clone().read_owned().await);
    let worker_root = root.clone();
    let database = state.paths.database_path.clone();
    let data_root = state.paths.data_root.clone();
    let _lease = root;
    tauri::async_runtime::spawn_blocking(move || {
        let (source, name) = prepare(&worker_root, &database, &asset_id)?;
        let mut dialog = app.dialog().file().set_file_name(name);
        if let Some(window) = app.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        let Some(destination) = dialog
            .blocking_save_file()
            .and_then(|value| value.into_path().ok())
        else {
            return Ok(serde_json::json!({"status":"cancelled"}));
        };
        copy_verified(&source, &asset_id, &destination, &worker_root, &data_root)?;
        Ok(serde_json::json!({"status":"saved"}))
    })
    .await
    .map_err(|_| "unavailable".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copy_refuses_overwrite_internal_destination_and_changed_source() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("archive");
        let data = temp.path().join("data");
        let output = temp.path().join("output");
        for dir in [&archive, &data, &output] {
            std::fs::create_dir(dir).unwrap();
        }
        let source = archive.join("source");
        std::fs::write(&source, b"picture bytes").unwrap();
        let id = format!("sha256:{:x}", Sha256::digest(b"picture bytes"));
        let target = output.join("photo.png");
        copy_verified(&source, &id, &target, &archive, &data).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"picture bytes");
        assert_eq!(
            copy_verified(&source, &id, &target, &archive, &data).unwrap_err(),
            "destination_exists"
        );
        assert_eq!(
            copy_verified(&source, &id, &archive.join("copy"), &archive, &data).unwrap_err(),
            "invalid_destination"
        );
        std::fs::write(&source, b"modified").unwrap();
        assert_eq!(
            copy_verified(&source, &id, &output.join("changed"), &archive, &data).unwrap_err(),
            "asset_changed"
        );
        assert!(!output.join("changed").exists());
    }
}
