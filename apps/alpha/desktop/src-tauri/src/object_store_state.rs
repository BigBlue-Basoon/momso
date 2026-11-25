use crate::{
    credentials, object_store, object_store_restore, object_store_transfer as transfer,
    runtime::RuntimePaths, storage, AppState,
};
use age::{secrecy::ExposeSecret, x25519};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppStateBackupResult {
    state: &'static str,
    created_at: String,
    size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppStateRestoreResult {
    state: &'static str,
    snapshot_at: String,
    safety_backup_created: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    bundle_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppStateKeepResult {
    state: &'static str,
}

type RestoreCandidate = (
    tempfile::NamedTempFile,
    String,
    Option<crate::paired_backup::CompletedPair>,
);

fn choose_restore_candidate(
    object: Result<RestoreCandidate, &'static str>,
    git: Result<RestoreCandidate, &'static str>,
) -> Result<(RestoreCandidate, bool), &'static str> {
    match (object, git) {
        (Ok(object), Ok(git)) => {
            let object_time =
                chrono::DateTime::parse_from_rfc3339(&object.1).map_err(|_| "invalid_snapshot")?;
            let git_time =
                chrono::DateTime::parse_from_rfc3339(&git.1).map_err(|_| "invalid_snapshot")?;
            if git_time > object_time {
                Ok((git, false))
            } else {
                Ok((object, true))
            }
        }
        (Ok(_), Err("local_write_failed")) | (Err("local_write_failed"), Ok(_)) => {
            Err("local_write_failed")
        }
        (Ok(_), Err("local_read_failed")) | (Err("local_read_failed"), Ok(_)) => {
            Err("local_read_failed")
        }
        (Ok(object), Err(_)) => Ok((object, true)),
        (Err(_), Ok(git)) => Ok((git, false)),
        (Err(object_error), Err(_)) => Err(object_error),
    }
}

pub(crate) async fn backup_snapshot(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    paths: &RuntimePaths,
) -> Result<transfer::Manifest, &'static str> {
    backup_snapshot_inner(config, keys, identity, paths, false, false).await
}

pub(crate) async fn backup_paired_snapshot(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    paths: &RuntimePaths,
    paired: bool,
) -> Result<transfer::Manifest, &'static str> {
    backup_snapshot_inner(config, keys, identity, paths, true, paired).await
}

async fn backup_snapshot_inner(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    paths: &RuntimePaths,
    verify_originals: bool,
    paired: bool,
) -> Result<transfer::Manifest, &'static str> {
    let temp = tempfile::Builder::new()
        .prefix("momso-state-")
        .tempdir_in(&paths.backups_path)
        .map_err(|_| "local_write_failed")?;
    let snapshot = temp.path().join("momso.db");
    storage::create_snapshot_file(&paths.database_path, &snapshot)
        .map_err(|_| "snapshot_failed")?;
    storage::sanitize_user_owned_backup_snapshot(&snapshot).map_err(|_| "snapshot_failed")?;
    if verify_originals {
        crate::paired_backup::verify_referenced_originals(config, keys, identity, &snapshot)
            .await?;
    }
    let stage = paths
        .data_root
        .join("object-store-staging")
        .join(&config.credential_id);
    let manifest = transfer::upload_file(
        config,
        keys,
        identity,
        transfer::UploadSpec {
            source: &snapshot,
            relative: if paired {
                "paired-momso.db"
            } else {
                "momso.db"
            },
            kind: "app_state",
            stage: &stage,
        },
        |_, _| {},
    )
    .await?;
    if std::fs::read_dir(&stage)
        .map_err(|_| "local_read_failed")?
        .next()
        .is_none()
    {
        std::fs::remove_dir(&stage).map_err(|_| "local_write_failed")?;
        if let Some(parent) = stage.parent() {
            if std::fs::read_dir(parent)
                .map_err(|_| "local_read_failed")?
                .next()
                .is_none()
            {
                std::fs::remove_dir(parent).map_err(|_| "local_write_failed")?;
            }
        }
    }
    Ok(manifest)
}

fn snapshot_workspace_id(path: &Path) -> Result<String, &'static str> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "invalid_snapshot")?;
    db.query_row(
        "SELECT workspace_id FROM momso_backup_profiles ORDER BY created_at ASC LIMIT 1",
        [],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|_| "invalid_snapshot")?
    .ok_or("invalid_snapshot")
}

fn preserve_recovery_identity(
    snapshot: &Path,
    identity: &x25519::Identity,
) -> Result<(), &'static str> {
    preserve_recovery_identity_with(
        snapshot,
        identity,
        |id| credentials::get_backup_identity(id).map_err(|_| "unavailable"),
        |id, key| credentials::import_backup_identity(id, key).map_err(|_| "unavailable"),
    )
}

fn preserve_recovery_identity_with(
    snapshot: &Path,
    identity: &x25519::Identity,
    get: impl FnOnce(&str) -> Result<Option<x25519::Identity>, &'static str>,
    put: impl FnOnce(&str, &str) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let workspace_id = snapshot_workspace_id(snapshot)?;
    match get(&workspace_id)? {
        Some(existing) if existing.to_public().to_string() != identity.to_public().to_string() => {
            Err("recovery_key_conflict")
        }
        Some(_) => Ok(()),
        None => put(&workspace_id, identity.to_string().expose_secret()),
    }
}

/// Refuse a replacement that would discard local user records absent from the backup.
/// An empty initial chat is allowed; imported records already in the snapshot are safe.
fn ensure_local_content_preserved(local: &Path, snapshot: &Path) -> Result<(), &'static str> {
    let db = Connection::open_with_flags(local, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "restore_failed")?;
    let snapshot = snapshot.to_str().ok_or("invalid_snapshot")?;
    db.execute("ATTACH DATABASE ?1 AS incoming", [snapshot])
        .map_err(|_| "invalid_snapshot")?;
    let chats = "SELECT id,title,group_id,pinned_order,archived_at FROM main.chats c \
        WHERE title<>'' OR group_id IS NOT NULL OR pinned_order IS NOT NULL \
        OR archived_at IS NOT NULL OR EXISTS(SELECT 1 FROM main.chat_messages m WHERE m.chat_id=c.id) \
        EXCEPT SELECT id,title,group_id,pinned_order,archived_at FROM incoming.chats";
    let conflict: bool = db
        .query_row(&format!("SELECT EXISTS({chats})"), [], |row| row.get(0))
        .map_err(|_| "local_data_conflict")?;
    if conflict {
        #[cfg(test)]
        {
            for (label, condition) in [
                ("missing", "incoming.id IS NULL"),
                ("title", "local.title IS NOT incoming.title"),
                ("group", "local.group_id IS NOT incoming.group_id"),
                ("pin", "local.pinned_order IS NOT incoming.pinned_order"),
                ("archive", "local.archived_at IS NOT incoming.archived_at"),
            ] {
                let sql = format!("SELECT count(*) FROM main.chats local LEFT JOIN incoming.chats incoming ON local.id=incoming.id WHERE (local.title<>'' OR local.group_id IS NOT NULL OR local.pinned_order IS NOT NULL OR local.archived_at IS NOT NULL OR EXISTS(SELECT 1 FROM main.chat_messages m WHERE m.chat_id=local.id)) AND {condition}");
                eprintln!(
                    "restore comparison chats {label}: {}",
                    db.query_row(&sql, [], |row| row.get::<_, i64>(0))
                        .unwrap_or(-1)
                );
            }
        }
        return Err("local_data_conflict");
    }
    // Git chat copies redact secret-shaped text. A matching redacted copy on
    // the new PC is represented by the original in the encrypted snapshot.
    let columns = db
        .prepare("PRAGMA main.table_info(chat_messages)")
        .and_then(|mut statement| {
            let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|_| "restore_failed")?;
    let columns = columns
        .iter()
        .filter(|column| column.as_str() != "text")
        .map(|column| format!("\"{}\"", column.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(",");
    let differs: bool = db
        .query_row(
            &format!("SELECT EXISTS(SELECT {columns} FROM main.chat_messages EXCEPT SELECT {columns} FROM incoming.chat_messages)"),
            [],
            |row| row.get(0),
        )
        .map_err(|_| "local_data_conflict")?;
    if differs {
        #[cfg(test)]
        eprintln!("restore comparison: message metadata differs");
        return Err("local_data_conflict");
    }
    let mut messages = db
        .prepare("SELECT local.text,backup.text FROM main.chat_messages local JOIN incoming.chat_messages backup ON local.id=backup.id WHERE local.text<>backup.text")
        .map_err(|_| "local_data_conflict")?;
    let rows = messages
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| "local_data_conflict")?;
    for row in rows {
        let (local, backup) = row.map_err(|_| "local_data_conflict")?;
        if crate::chat_archive_redact::redact(&backup).text != local {
            #[cfg(test)]
            eprintln!("restore comparison: message text differs");
            return Err("local_data_conflict");
        }
    }
    ensure_tables_preserved(
        &db,
        &[
            "chat_groups",
            "chat_attachments",
            "chat_imports",
            "media_generation",
            "generated_output_turns",
            "momso_source_assets",
            "momso_records",
            "momso_record_versions",
            "momso_record_bodies",
            "momso_record_source_assets",
            "momso_transcription_terms",
        ],
    )
}

fn ensure_tables_preserved(db: &Connection, tables: &[&str]) -> Result<(), &'static str> {
    for table in tables {
        let has_local: bool = db
            .query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM main.{table})"),
                [],
                |row| row.get(0),
            )
            .map_err(|_| "restore_failed")?;
        if !has_local {
            continue;
        }
        let differs: bool = db
            .query_row(
                &format!("SELECT EXISTS(SELECT * FROM main.{table} EXCEPT SELECT * FROM incoming.{table})"),
                [],
                |row| row.get(0),
            )
            .map_err(|_| "local_data_conflict")?;
        if differs {
            #[cfg(test)]
            eprintln!("restore comparison: {table} differs");
            return Err("local_data_conflict");
        }
    }
    Ok(())
}

pub(crate) fn ensure_nonchat_content_preserved(
    local: &Path,
    snapshot: &Path,
) -> Result<(), &'static str> {
    let db = Connection::open_with_flags(local, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "restore_failed")?;
    let snapshot = snapshot.to_str().ok_or("invalid_snapshot")?;
    db.execute("ATTACH DATABASE ?1 AS incoming", [snapshot])
        .map_err(|_| "invalid_snapshot")?;
    ensure_tables_preserved(
        &db,
        &[
            "media_generation",
            "generated_output_turns",
            "momso_source_assets",
            "momso_records",
            "momso_record_versions",
            "momso_record_bodies",
            "momso_record_source_assets",
            "momso_transcription_terms",
        ],
    )
}

#[cfg(test)]
fn apply_snapshot(snapshot: &Path, paths: &RuntimePaths) -> Result<PathBuf, &'static str> {
    apply_snapshot_with_callback(snapshot, paths, |_| {})
}

fn apply_snapshot_with_progress<R: tauri::Runtime>(
    snapshot: &Path,
    paths: &RuntimePaths,
    app: Option<&tauri::AppHandle<R>>,
) -> Result<PathBuf, &'static str> {
    let result = apply_snapshot_with_callback(snapshot, paths, |progress| {
        use tauri::Emitter;
        if let Some(app) = app {
            let _ = app.emit("chat-archive-restore-progress", progress);
        }
    });
    if result.is_ok() && app.is_some() {
        crate::chat_search_index::start(paths.database_path.clone());
    }
    result
}

fn apply_snapshot_with_callback(
    snapshot: &Path,
    paths: &RuntimePaths,
    mut emit: impl FnMut(crate::chat_archive_store::RestoreProgress),
) -> Result<PathBuf, &'static str> {
    let _receive_guard = crate::archive_remote::lock_remote_for_restore()?;
    let _receive_file_lock = crate::archive_remote::lock_first_receive(&paths.database_path)?;
    let restore_guard = crate::chat_search_index::RESTORE_LOCK
        .lock()
        .map_err(|_| "restore_failed")?;
    let root = paths.objects_path.parent().ok_or("invalid_snapshot")?;
    if crate::archive_remote::first_receive_pending(root, &paths.database_path) {
        return Err("first_receive_in_progress");
    }
    storage::validate_snapshot_file(snapshot).map_err(|_| "invalid_snapshot")?;
    let path = root.to_str().ok_or("invalid_snapshot")?;
    let db = Connection::open(snapshot).map_err(|_| "invalid_snapshot")?;
    db.execute("INSERT INTO archive_location_settings(id,path,initialized) VALUES(1,?1,1) ON CONFLICT(id) DO UPDATE SET path=excluded.path,initialized=1", [path]).map_err(|_| "invalid_snapshot")?;
    drop(db);
    crate::archive::reindex(root, snapshot).map_err(|_| "invalid_snapshot")?;
    let mut last_progress = None;
    let result = storage::restore_snapshot_file_with_hook(
        snapshot,
        &paths.database_path,
        &paths.backups_path,
        || {
            crate::chat_archive_store::restore_with_progress(
                root,
                &paths.database_path,
                |mut progress| {
                    if progress.state != "restoring" {
                        progress.state = "restoring";
                        progress.available = false;
                    }
                    emit(progress.clone());
                    last_progress = Some(progress);
                },
            )
            .map_err(|error| storage::StorageError::Integrity(error.to_owned()))
        },
    );
    if let Some(mut progress) = last_progress {
        progress.state = if result.is_ok() { "complete" } else { "failed" };
        progress.available = result.is_ok();
        emit(progress);
    }
    drop(restore_guard);
    result.map_err(|error| match error {
        storage::StorageError::RestoreRollbackFailed => "restore_rollback_failed",
        _ => "restore_failed",
    })
}

#[tauri::command]
pub(crate) async fn app_state_backup(
    state: State<'_, AppState>,
) -> Result<AppStateBackupResult, &'static str> {
    let paths = state.paths.scoped().await;
    let _guard = object_store::CONNECTION_LOCK.lock().await;
    let config = object_store::load_config(&paths.data_root)?.ok_or("not_connected")?;
    let keys = object_store::load_keys(&config)?;
    let identity = transfer::identity(&paths.database_path)?;
    let manifest = backup_snapshot(&config, &keys, &identity, &paths).await?;
    Ok(AppStateBackupResult {
        state: "complete",
        created_at: manifest.created_at,
        size: manifest.size,
    })
}

#[tauri::command]
pub(crate) async fn app_state_restore(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<AppStateRestoreResult, &'static str> {
    restore_with_choice(app, state, false).await
}

/// The user explicitly chose the backup after seeing a conflict. The existing
/// database is retained by `restore_snapshot_file_with_hook` before replacement.
#[tauri::command]
pub(crate) async fn app_state_restore_use_backup(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<AppStateRestoreResult, &'static str> {
    restore_with_choice(app, state, true).await
}

#[tauri::command]
pub(crate) fn app_state_restore_keep_current() -> AppStateKeepResult {
    AppStateKeepResult { state: "kept" }
}

pub(crate) async fn restore_with_choice<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    use_backup: bool,
) -> Result<AppStateRestoreResult, &'static str> {
    let location = state.paths.archive.current.clone().write_owned().await;
    let active = state.chat_active.clone().lock_owned().await;
    if !active.is_empty() {
        return Err("restore_busy");
    }
    let mut paths = state.paths.clone();
    paths.objects_path = location.join("objects");
    if crate::archive_remote::first_receive_pending(&location, &paths.database_path) {
        return Err("first_receive_in_progress");
    }
    let _guard = object_store::CONNECTION_LOCK.lock().await;
    let identity = transfer::identity(&paths.database_path)?;
    let (snapshot, snapshot_at, pair, class_snapshot) =
        match object_store::load_config(&paths.data_root)? {
            Some(config) => {
                let keys = object_store::load_keys(&config)?;
                let object = crate::paired_backup::download_for_restore(
                    &app, &paths, &config, &keys, &identity, use_backup,
                )
                .await
                .map(|(snapshot, manifest, pair)| {
                    let at = pair
                        .as_ref()
                        .map_or(manifest.created_at, |pair| pair.created_at.clone());
                    (snapshot, at, pair)
                });
                let git = match crate::archive_remote::paired_git_context_for_restore(
                    &app,
                    &location,
                    &paths.database_path,
                    !use_backup,
                ) {
                    Ok((tool, target, _)) => match crate::paired_git_store::download(
                        &tool, &location, &target, &paths, &identity,
                    ) {
                        Ok((snapshot, pair)) => {
                            match crate::paired_backup::verify_referenced_originals(
                                &config,
                                &keys,
                                &identity,
                                snapshot.path(),
                            )
                            .await
                            {
                                Ok(()) => {
                                    let at = pair.created_at.clone();
                                    Ok((snapshot, at, Some(pair)))
                                }
                                Err(error) => Err(error),
                            }
                        }
                        Err(error) => Err(error),
                    },
                    Err(error) => Err(error),
                };
                let ((snapshot, at, pair), object_selected) =
                    choose_restore_candidate(object, git)?;
                let class_snapshot = if object_selected {
                    crate::class_student_backup::download(&config, &keys, &identity, &paths).await?
                } else {
                    None
                };
                (snapshot, at, pair, class_snapshot)
            }
            None => {
                let (tool, target, _) = crate::archive_remote::paired_git_context_for_restore(
                    &app,
                    &location,
                    &paths.database_path,
                    !use_backup,
                )?;
                let (snapshot, pair) = crate::paired_git_store::download(
                    &tool, &location, &target, &paths, &identity,
                )?;
                let at = pair.created_at.clone();
                (snapshot, at, Some(pair), None)
            }
        };
    preserve_recovery_identity(snapshot.path(), &identity)?;
    let mut switch = None;
    if let Some(pair) = &pair {
        let (tool, _, head) = crate::archive_remote::paired_git_context_for_restore(
            &app,
            &location,
            &paths.database_path,
            !use_backup,
        )?;
        if head != pair.git_revision {
            if !use_backup {
                let db = Connection::open_with_flags(
                    &paths.database_path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .map_err(|_| "local_state_unavailable")?;
                let received: Option<String> = db
                    .query_row(
                        "SELECT value FROM app_runtime_meta WHERE key='paired_first_receive_head'",
                        [],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|_| "local_state_unavailable")?;
                if received.as_deref() != Some(head.as_str()) {
                    return Err("local_data_conflict");
                }
                ensure_nonchat_content_preserved(&paths.database_path, snapshot.path())?;
            }
            switch = Some((tool, head, pair.git_revision.clone()));
        }
    }
    if !use_backup && switch.is_none() {
        ensure_local_content_preserved(&paths.database_path, snapshot.path())?;
    }
    let class_root = paths.data_root.clone();
    let restore_paths = paths.clone();
    let restore_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // These owned guards stay with the blocking work even if the command
        // is cancelled while Git or SQLite is being changed.
        let _active = active;
        let root = location;
        let rollback = if let Some((tool, head, target)) = switch {
            let journal = crate::paired_restore::begin(
                &tool,
                &root,
                &restore_paths,
                &head,
                &target,
                use_backup,
            )?;
            Some((tool, journal))
        } else {
            None
        };
        let result =
            apply_snapshot_with_progress(snapshot.path(), &restore_paths, Some(&restore_app))
                .map(|_| ())
                .and_then(|()| {
                    if let Some(file) = class_snapshot {
                        crate::class_student_backup::install(file, &class_root)?;
                    }
                    Ok(())
                });
        if let Err(error) = result {
            if let Some((tool, journal)) = &rollback {
                crate::paired_restore::rollback(tool, &root, &restore_paths, journal)?;
            }
            return Err(error);
        }
        if rollback.is_some() {
            crate::paired_restore::finish(&restore_paths)?;
        }
        Ok(())
    })
    .await
    .map_err(|_| "restore_failed")??;
    Ok(AppStateRestoreResult {
        state: "restored",
        snapshot_at,
        safety_backup_created: true,
        bundle_id: pair.map(|pair| pair.pair_id),
    })
}

pub(crate) async fn download_latest_snapshot(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    paths: &RuntimePaths,
) -> Result<(tempfile::NamedTempFile, transfer::Manifest), &'static str> {
    let manifest = transfer::catalog(config, keys, identity)
        .await?
        .into_iter()
        .filter(|item| item.kind == "app_state" && item.relative_path == "momso.db")
        .max_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then(a.upload_id.cmp(&b.upload_id))
        })
        .ok_or("remote_missing")?;
    let snapshot = object_store_restore::download_verified(
        config,
        keys,
        identity,
        &manifest,
        &paths.backups_path,
    )
    .await?;
    storage::validate_snapshot_file(snapshot.path()).map_err(|_| "invalid_snapshot")?;
    Ok((snapshot, manifest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        fs,
        io::{Read, Write},
        net::TcpListener,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
    };

    #[test]
    fn restore_selects_newest_complete_storage_candidate() {
        let object = (
            tempfile::NamedTempFile::new().unwrap(),
            "2026-01-01T00:00:00Z".into(),
            None,
        );
        let git = (
            tempfile::NamedTempFile::new().unwrap(),
            "2026-01-02T00:00:00Z".into(),
            None,
        );
        let (selected, object_selected) = choose_restore_candidate(Ok(object), Ok(git)).unwrap();
        assert!(!object_selected);
        assert_eq!(selected.1, "2026-01-02T00:00:00Z");
        let older = (
            tempfile::NamedTempFile::new().unwrap(),
            "2026-01-01T00:00:00Z".into(),
            None,
        );
        let (_, object_selected) =
            choose_restore_candidate(Ok(older), Err("backup_pair_incomplete")).unwrap();
        assert!(object_selected);
    }

    #[test]
    fn restore_refuses_distinct_local_conversation_without_replacing_it() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("local.db");
        let backups = temp.path().join("backups");
        storage::initialize(&database, &backups, "test").unwrap();
        let blank = crate::chat::create(&database).unwrap();
        let blank_id = serde_json::to_value(blank).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let snapshot = temp.path().join("snapshot.db");
        storage::create_snapshot_file(&database, &snapshot).unwrap();
        assert_eq!(ensure_local_content_preserved(&database, &snapshot), Ok(()));
        crate::chat::append(
            &database,
            &blank_id,
            crate::chat::Role::User,
            "local unsaved message",
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            ensure_local_content_preserved(&database, &snapshot),
            Err("local_data_conflict")
        );
        let chat = serde_json::to_value(crate::chat::read(&database, &blank_id).unwrap()).unwrap();
        assert_eq!(chat["messages"][0]["text"], "local unsaved message");
    }

    #[test]
    fn restore_allows_local_content_already_in_backup() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("local.db");
        let backups = temp.path().join("backups");
        storage::initialize(&database, &backups, "test").unwrap();
        let chat = serde_json::to_value(crate::chat::create(&database).unwrap()).unwrap();
        let id = chat["id"].as_str().unwrap();
        crate::chat::append(
            &database,
            id,
            crate::chat::Role::User,
            "same message",
            None,
            None,
        )
        .unwrap();
        let snapshot = temp.path().join("snapshot.db");
        storage::create_snapshot_file(&database, &snapshot).unwrap();
        assert_eq!(ensure_local_content_preserved(&database, &snapshot), Ok(()));
        // The untouched empty chat made on a fresh installation is not a conflict.
        crate::chat::create(&database).unwrap();
        assert_eq!(ensure_local_content_preserved(&database, &snapshot), Ok(()));
    }

    #[test]
    fn restore_accepts_redacted_git_copy_but_rejects_changed_message() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("local.db");
        storage::initialize(&database, &temp.path().join("backups"), "test").unwrap();
        let chat = serde_json::to_value(crate::chat::create(&database).unwrap()).unwrap();
        let id = chat["id"].as_str().unwrap();
        let original = format!("sk-proj-{}", "Z".repeat(30));
        crate::chat::append(
            &database,
            id,
            crate::chat::Role::User,
            &original,
            None,
            None,
        )
        .unwrap();
        let snapshot = temp.path().join("snapshot.db");
        storage::create_snapshot_file(&database, &snapshot).unwrap();
        let db = storage::open_writable_connection(&database).unwrap();
        db.execute(
            "UPDATE chat_messages SET text=?1 WHERE chat_id=?2",
            rusqlite::params![crate::chat_archive_redact::redact(&original).text, id],
        )
        .unwrap();
        assert_eq!(ensure_local_content_preserved(&database, &snapshot), Ok(()));
        db.execute(
            "UPDATE chat_messages SET text='different message' WHERE chat_id=?1",
            [id],
        )
        .unwrap();
        assert_eq!(
            ensure_local_content_preserved(&database, &snapshot),
            Err("local_data_conflict")
        );
    }

    #[test]
    fn restore_refuses_distinct_local_asset_without_replacing_it() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("local.db");
        storage::initialize(&database, &temp.path().join("backups"), "test").unwrap();
        let snapshot = temp.path().join("snapshot.db");
        storage::create_snapshot_file(&database, &snapshot).unwrap();
        let hash = "a".repeat(64);
        let db = storage::open_writable_connection(&database).unwrap();
        db.execute(
            "INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset','other',?1,1,?2,'local.txt','fixture','now')",
            rusqlite::params![hash, format!("sha256/aa/{hash}")],
        ).unwrap();
        drop(db);
        assert_eq!(
            ensure_local_content_preserved(&database, &snapshot),
            Err("local_data_conflict")
        );
        let db = storage::open_writable_connection(&database).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT original_file_name FROM momso_source_assets WHERE id='asset'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "local.txt"
        );
    }

    #[test]
    fn restore_rejects_corrupt_database_without_touching_local_content() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("local.db");
        storage::initialize(&database, &temp.path().join("backups"), "test").unwrap();
        let chat = serde_json::to_value(crate::chat::create(&database).unwrap()).unwrap();
        let id = chat["id"].as_str().unwrap();
        crate::chat::append(
            &database,
            id,
            crate::chat::Role::User,
            "preserved",
            None,
            None,
        )
        .unwrap();
        let corrupt = temp.path().join("corrupt.db");
        fs::write(&corrupt, b"not a sqlite snapshot").unwrap();
        assert!(storage::validate_snapshot_file(&corrupt).is_err());
        let result = serde_json::to_value(crate::chat::read(&database, id).unwrap()).unwrap();
        assert_eq!(result["messages"][0]["text"], "preserved");
    }

    #[test]
    fn restore_choice_preserves_old_database_and_keep_is_noop() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("local.db");
        let backups = temp.path().join("backups");
        storage::initialize(&database, &backups, "test").unwrap();
        let backup = temp.path().join("snapshot.db");
        storage::create_snapshot_file(&database, &backup).unwrap();
        let chat = serde_json::to_value(crate::chat::create(&database).unwrap()).unwrap();
        let id = chat["id"].as_str().unwrap();
        crate::chat::append(&database, id, crate::chat::Role::User, "local", None, None).unwrap();
        assert_eq!(
            ensure_local_content_preserved(&database, &backup),
            Err("local_data_conflict")
        );
        assert_eq!(
            serde_json::to_value(app_state_restore_keep_current()).unwrap()["state"],
            "kept"
        );
        assert!(crate::chat::read(&database, id).is_ok());
        let safety =
            storage::restore_snapshot_file_with_hook(&backup, &database, &backups, || Ok(()))
                .unwrap();
        assert!(crate::chat::read(&database, id).is_err());
        assert_eq!(
            serde_json::to_value(crate::chat::read(&safety, id).unwrap()).unwrap()["messages"][0]
                ["text"],
            "local"
        );
    }

    #[tokio::test]
    async fn old_app_snapshot_preserves_a_deleted_chat_marker() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let database_path = root.join("momso.db");
        let backups_path = root.join("backups");
        fs::create_dir_all(&backups_path).unwrap();
        storage::initialize(&database_path, &backups_path, "test").unwrap();
        let archive_root = root.join("momso");
        let archive =
            crate::archive_location::ArchiveLocation::load(database_path.clone(), root, vec![])
                .unwrap();
        crate::archive::initialize(&archive_root, root).unwrap();
        let paths = RuntimePaths {
            data_root: root.to_path_buf(),
            database_path: database_path.clone(),
            objects_path: archive_root.join("objects"),
            exports_path: root.join("exports"),
            backups_path,
            logs_path: root.join("logs"),
            archive,
            archive_lease: None,
        };
        let room = crate::chat::create(&database_path).unwrap();
        let id = serde_json::to_value(room).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        crate::chat::append(
            &database_path,
            &id,
            crate::chat::Role::User,
            "fixture",
            None,
            None,
        )
        .unwrap();
        crate::chat_archive_store::write(&archive_root, &paths, &id).unwrap();
        let snapshot = root.join("old-snapshot.db");
        storage::create_snapshot_file(&database_path, &snapshot).unwrap();
        let survivor = serde_json::to_value(crate::chat::create(&database_path).unwrap()).unwrap()
            ["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let document = archive_root.join("zettels").join("fixture.txt");
        let original = archive_root.join("objects").join("fixture.bin");
        fs::write(&document, b"user document").unwrap();
        fs::write(&original, b"user original").unwrap();
        let active = crate::chat_codex::Active::default();
        let result =
            crate::chat::delete_with(&database_path, &active, &id, |_| async { Ok(()) }).await;
        assert_eq!(serde_json::to_value(result).unwrap()["state"], "deleted");
        crate::chat_archive_store::remove(&archive_root, &database_path, &id).unwrap();
        assert!(crate::chat::read(&database_path, &id).is_err());
        let marker: serde_json::Value = serde_json::from_slice(
            &fs::read(archive_root.join("chats").join(format!("{id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["deleted"], true);
        // Even before a Git copy is present, the local database deletion must win.
        fs::remove_file(archive_root.join("chats").join(format!("{id}.json"))).unwrap();
        let safety = apply_snapshot(&snapshot, &paths).unwrap();
        assert!(
            crate::chat::read(&database_path, &id).is_err(),
            "the old snapshot must not resurrect the deleted chat"
        );
        assert!(
            crate::chat::read(&safety, &survivor).is_ok(),
            "pre-restore chat remains in the safety copy"
        );
        assert_eq!(fs::read(document).unwrap(), b"user document");
        assert_eq!(fs::read(original).unwrap(), b"user original");
    }

    #[test]
    fn app_state_restore_refuses_pending_first_receive_without_replacing_database() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        let database_path = base.join("momso.db");
        let backups_path = base.join("backups");
        storage::initialize(&database_path, &backups_path, "test").unwrap();
        let archive =
            crate::archive_location::ArchiveLocation::load(database_path.clone(), base, vec![])
                .unwrap();
        let archive_root = base.join("momso");
        crate::archive::initialize(&archive_root, base).unwrap();
        let snapshot = base.join("snapshot.db");
        storage::create_snapshot_file(&database_path, &snapshot).unwrap();
        let room = crate::chat::create(&database_path).unwrap();
        let id = serde_json::to_value(room).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        Connection::open(&database_path).unwrap().execute(
            "INSERT INTO app_runtime_meta(key,value,updated_at) VALUES(?1,'pending',CURRENT_TIMESTAMP)",
            [crate::archive_remote::FIRST_RECEIVE_JOURNAL],
        ).unwrap();
        let paths = RuntimePaths {
            data_root: base.to_path_buf(),
            database_path: database_path.clone(),
            objects_path: archive_root.join("objects"),
            exports_path: base.join("exports"),
            backups_path,
            logs_path: base.join("logs"),
            archive,
            archive_lease: None,
        };
        assert_eq!(
            apply_snapshot(&snapshot, &paths),
            Err("first_receive_in_progress")
        );
        assert!(crate::chat::read(&database_path, &id).is_ok());
    }

    #[test]
    fn failed_chat_import_restores_pre_restore_database() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let database_path = root.join("momso.db");
        let backups_path = root.join("backups");
        fs::create_dir_all(&backups_path).unwrap();
        storage::initialize(&database_path, &backups_path, "test").unwrap();
        let archive_root = root.join("momso");
        let archive =
            crate::archive_location::ArchiveLocation::load(database_path.clone(), root, vec![])
                .unwrap();
        crate::archive::initialize(&archive_root, root).unwrap();
        let paths = RuntimePaths {
            data_root: root.to_path_buf(),
            database_path: database_path.clone(),
            objects_path: archive_root.join("objects"),
            exports_path: root.join("exports"),
            backups_path,
            logs_path: root.join("logs"),
            archive,
            archive_lease: None,
        };
        let id = "chat_00000000-0000-4000-8000-000000000000";
        let db = storage::open_writable_connection(&database_path).unwrap();
        db.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES(?1,'fixture','x','x')",
            [id],
        )
        .unwrap();
        drop(db);
        let snapshot = root.join("snapshot.db");
        storage::create_snapshot_file(&database_path, &snapshot).unwrap();
        crate::chat::append(
            &database_path,
            id,
            crate::chat::Role::User,
            "pre-restore message",
            None,
            None,
        )
        .unwrap();
        fs::create_dir_all(archive_root.join("chats")).unwrap();
        let marker = serde_json::json!({"version":1,"room":{"id":id},"deleted":true,"deletedAt":"2026-11-17T00:00:00Z"});
        fs::write(
            archive_root.join("chats").join(format!("{id}.json")),
            serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();
        fs::write(
            archive_root
                .join("chats")
                .join("chat_ffffffff-ffff-4fff-8fff-ffffffffffff.json"),
            b"invalid json",
        )
        .unwrap();
        let mut failed_after_rollback = false;
        let result = apply_snapshot_with_callback(&snapshot, &paths, |event| {
            if event.state == "failed" {
                let detail =
                    serde_json::to_value(crate::chat::read(&database_path, id).unwrap()).unwrap();
                assert_eq!(detail["messages"][0]["text"], "pre-restore message");
                assert!(!event.available);
                failed_after_rollback = true;
            }
        });
        assert_eq!(result, Err("restore_failed"));
        assert!(failed_after_rollback);
        let preserved =
            serde_json::to_value(crate::chat::read(&database_path, id).unwrap()).unwrap();
        assert_eq!(preserved["messages"][0]["text"], "pre-restore message");
    }

    async fn fake_skill_connection(chat_id: &str, cancel_first: bool) -> Option<String> {
        let action =
            serde_json::json!({"method":"object_store_gcp_connect", "account":"fixture-project"});
        let (service, account, region) =
            crate::setup_actions::storage_account(&action).expect("stage 5: skill action");
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let secrets = crate::setup_secret::SecretBroker::in_memory();
        let approvals = crate::approval::ApprovalBroker::default();
        let secret_events = events.clone();
        let approval_events = events.clone();
        let connection_events = events.clone();
        let secret_chat = chat_id.to_owned();
        let approval_chat = chat_id.to_owned();
        let connection_chat = chat_id.to_owned();
        let selected = Arc::new(Mutex::new(None::<String>));
        let connection_selected = selected.clone();
        let outcome = crate::setup_actions::run_storage_flow(
            chat_id,
            "teacher",
            service,
            account,
            region,
            "momso-teacher",
            tokio_util::sync::CancellationToken::new(),
            move |request| {
                let broker = secrets.clone();
                let responder = broker.clone();
                let events = secret_events.clone();
                let chat = secret_chat.clone();
                async move {
                    broker
                        .request_with(
                            request,
                            tokio_util::sync::CancellationToken::new(),
                            move |event| {
                                events.lock().unwrap().push("secret".into());
                                assert_eq!(event.resource_name, "momso-teacher");
                                let id = event.request_id.clone();
                                tokio::spawn(async move {
                                    if cancel_first {
                                        responder.cancel(&id, &chat).await.unwrap();
                                    } else {
                                        responder
                                            .submit(
                                                &id,
                                                &chat,
                                                crate::setup_secret::SecretInput {
                                                    access_key_id: "fixture-id".into(),
                                                    secret_access_key: "fixture-secret".into(),
                                                    gcp_oauth_token: Some("fixture-oauth".into()),
                                                },
                                            )
                                            .await
                                            .unwrap();
                                    }
                                });
                                true
                            },
                            |_| {},
                        )
                        .await
                }
            },
            move |plan| {
                let broker = approvals.clone();
                let responder = broker.clone();
                let events = approval_events.clone();
                let chat = approval_chat.clone();
                async move {
                    let expected_chat = chat.clone();
                    plan.authorize_with(
                        &broker,
                        &chat,
                        "codex",
                        tokio_util::sync::CancellationToken::new(),
                        move |event| {
                            let card = serde_json::to_value(event).unwrap();
                            assert_eq!(card["chatId"], expected_chat);
                            assert_eq!(card["setup"]["resourceName"], "momso-teacher");
                            events.lock().unwrap().push("approval".into());
                            let id = card["requestId"].as_str().unwrap().to_owned();
                            tokio::spawn(async move {
                                responder
                                    .respond(&id, crate::approval::ApprovalChoice::AllowOnce)
                                    .await
                                    .unwrap();
                            });
                            true
                        },
                        |_| {},
                    )
                    .await
                }
            },
            move |permit, secret| {
                let chat = connection_chat.clone();
                let events = connection_events.clone();
                let selected = connection_selected.clone();
                async move {
                    let details = permit.consume(&chat)?;
                    assert_eq!(details.resource_name, "momso-teacher");
                    assert_eq!(secret.take()?.access_key_id, "fixture-id");
                    *selected.lock().unwrap() = Some(details.resource_name);
                    events.lock().unwrap().push("connected".into());
                    Ok(())
                }
            },
        )
        .await;
        if cancel_first {
            assert_eq!(outcome["status"], "cancelled");
            assert_eq!(*events.lock().unwrap(), vec!["secret"]);
        } else {
            assert_eq!(outcome["status"], "connected");
            assert_eq!(
                *events.lock().unwrap(),
                vec!["secret", "approval", "connected"]
            );
        }
        let result = selected.lock().unwrap().clone();
        result
    }

    #[tokio::test]
    async fn sqlite_snapshot_round_trip_preserves_current_state_before_restore() {
        exercise_new_pc_round_trip("s3_compatible", false).await;
    }

    #[tokio::test]
    async fn oauth_r2_new_pc_recovers_documents_sources_and_chat_state() {
        exercise_new_pc_round_trip("r2_oauth", false).await;
    }

    #[tokio::test]
    async fn ncp_new_pc_recovers_documents_sources_and_chat_state() {
        exercise_new_pc_round_trip("ncp", false).await;
    }

    #[tokio::test]
    async fn aws_new_pc_recovers_documents_sources_and_chat_state() {
        exercise_new_pc_round_trip("aws", false).await;
    }

    #[tokio::test]
    async fn r2_key_new_pc_recovers_documents_sources_and_chat_state() {
        exercise_new_pc_round_trip("cloudflare", false).await;
    }

    #[cfg(windows)]
    #[test]
    fn new_pc_r2_restore_after_interrupted_first_receive() {
        if std::env::var_os("MOMSO_D35_CHILD").is_some() {
            assert!(std::env::var_os("MOMSO_TEST_FAKE_CREDENTIAL_STORE").is_some());
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(exercise_new_pc_round_trip("r2_oauth", true));
            return;
        }
        let output = crate::process::hidden_command(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "object_store_state::tests::new_pc_r2_restore_after_interrupted_first_receive",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("MOMSO_D35_CHILD", "1")
            .env("MOMSO_TEST_FAKE_CREDENTIAL_STORE", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated fake restore failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn restoring_older_state_preserves_local_source_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("state.db");
        let backups = temp.path().join("backups");
        storage::initialize(&database, &backups, "test").unwrap();
        let old_hash = "b".repeat(64);
        let conn = storage::open_writable_connection(&database).unwrap();
        conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('older-asset','other',?1,1,?2,'old-private-name','tester','old')", rusqlite::params![old_hash,format!("sha256/bb/{old_hash}")]).unwrap();
        for (id, hash, name) in [
            ("retry", "c".repeat(64), "old-retry.wav"),
            ("offline", "d".repeat(64), "old-offline.wav"),
        ] {
            conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES(?1,'other',?2,1,?3,?4,'tester','old')", rusqlite::params![id,hash,format!("sha256/{}/{hash}",&hash[..2]),name]).unwrap();
        }
        drop(conn);
        let older = temp.path().join("older.db");
        storage::create_snapshot_file(&database, &older).unwrap();
        let conn = storage::open_writable_connection(&database).unwrap();
        let hash = "a".repeat(64);
        conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset','other',?1,1,?2,'deleted','redacted','now')", rusqlite::params![hash,format!("sha256/aa/{hash}")]).unwrap();
        conn.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until) VALUES('asset',?1,'purge_pending','now','later')", [uuid::Uuid::new_v4().to_string()]).unwrap();
        conn.execute("UPDATE momso_source_assets SET original_file_name='deleted',captured_by='redacted' WHERE id='older-asset'", []).unwrap();
        conn.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until) VALUES('older-asset',?1,'purge_pending','now','later')", [uuid::Uuid::new_v4().to_string()]).unwrap();
        for (id, state, name, remaining) in [
            (
                "retry",
                "purge_pending",
                "retry-name.wav",
                r#"["remote-copy"]"#,
            ),
            (
                "offline",
                "purged",
                "offline-name.wav",
                r#"["remote_access:not_connected"]"#,
            ),
        ] {
            conn.execute(
                "UPDATE momso_source_assets SET original_file_name=?1 WHERE id=?2",
                rusqlite::params![name, id],
            )
            .unwrap();
            conn.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until,remote_remaining_json) VALUES(?1,?2,?3,'now','later',?4)", rusqlite::params![id,uuid::Uuid::new_v4().to_string(),state,remaining]).unwrap();
        }
        drop(conn);
        let failure = storage::restore_snapshot_file_with_hook(&older, &database, &backups, || {
            Err(storage::StorageError::Integrity("injected".into()))
        });
        assert!(failure.is_err());
        let conn = storage::open_writable_connection(&database).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM source_asset_deletions", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            4
        );
        drop(conn);
        storage::restore_snapshot_file(&older, &database, &backups).unwrap();
        let conn = storage::open_writable_connection(&database).unwrap();
        let preserved: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM source_asset_deletions d JOIN momso_source_assets a ON a.id=d.asset_id WHERE a.content_sha256=?1 AND d.state='purge_pending')", [&hash], |r| r.get(0)).unwrap();
        assert!(
            preserved,
            "restoring an older snapshot must retain the local purge marker"
        );
        let name: String = conn
            .query_row(
                "SELECT original_file_name FROM momso_source_assets WHERE content_sha256=?1",
                [&old_hash],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "deleted");
        for (id, expected) in [("retry", "retry-name.wav"), ("offline", "offline-name.wav")] {
            let name: String = conn
                .query_row(
                    "SELECT original_file_name FROM momso_source_assets WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(name, expected);
        }
        assert_eq!(
            conn.query_row("SELECT count(*) FROM source_asset_deletions", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            4
        );
    }

    async fn exercise_new_pc_round_trip(provider: &'static str, crash_first_receive: bool) {
        let oauth = provider == "r2_oauth";
        let ncp = provider == "ncp";
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let database_path = root.join("momso.db");
        let backups_path = root.join("backups");
        fs::create_dir_all(&backups_path).unwrap();
        storage::initialize(&database_path, &backups_path, "test").unwrap();
        let archive =
            crate::archive_location::ArchiveLocation::load(database_path.clone(), root, vec![])
                .unwrap();
        let archive_root = root.join("momso");
        crate::archive::initialize(&archive_root, root).unwrap();
        crate::archive::profile_set(
            &archive_root,
            root,
            "Teacher".into(),
            "Yoga instructor".into(),
            "Use short answers".into(),
        )
        .expect("stage 1: profile and rule files");
        let paths = RuntimePaths {
            data_root: root.to_path_buf(),
            database_path,
            objects_path: archive_root.join("objects"),
            exports_path: root.join("exports"),
            backups_path,
            logs_path: root.join("logs"),
            archive,
            archive_lease: None,
        };
        let username = serde_json::to_value(crate::archive_remote::username_set_db(
            &paths.database_path,
            "teacher".into(),
        ))
        .unwrap();
        assert_eq!(username["state"], "ok", "stage 1: username");
        crate::permission_modes::set(
            &paths.database_path,
            crate::permission_modes::Provider::Codex,
            "Full access",
        )
        .unwrap();
        let original_id = crate::archive_zet::create(
            &archive_root,
            &paths.database_path,
            crate::archive_zet::ZetCreate {
                title: "original".into(),
                summary: "original summary".into(),
                kind: "note".into(),
                body: "original body".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .expect("stage 1: seed document");
        let new_id = crate::archive_zet::create(
            &archive_root,
            &paths.database_path,
            crate::archive_zet::ZetCreate {
                title: "created".into(),
                summary: "created summary".into(),
                kind: "note".into(),
                body: "created body".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .expect("stage 1: create valid document template");
        let new_path = archive_root.join("zettels").join(format!("{new_id}.md"));
        let template_path = root.join("new-document-template.txt");
        fs::copy(&new_path, &template_path).unwrap();
        fs::remove_file(&new_path).unwrap();
        let room =
            serde_json::to_value(crate::chat::create(&paths.database_path).unwrap()).unwrap();
        let chat_id = room["id"].as_str().unwrap().to_owned();
        let sent = crate::chat::append(
            &paths.database_path,
            &chat_id,
            crate::chat::Role::User,
            "hello",
            None,
            None,
        )
        .expect("stage 2: chat message");
        crate::chat::append_finished(
            &paths.database_path,
            &chat_id,
            "partial",
            "codex".into(),
            None,
            None,
            crate::chat::FinishReason::Error,
        )
        .expect("stage 2: failed answer");
        let (retry_text, retry_skill) =
            crate::chat::retry_input(&paths.database_path, &chat_id, &sent.id)
                .expect("stage 2: read failed message")
                .expect("stage 2: failed message is retryable");
        assert_eq!(retry_text, "hello", "stage 2: retry original text");
        assert!(retry_skill.is_none());
        let script = root.join("fake-codex.ps1");
        fs::write(&script, r#"
$null = [Console]::ReadLine()
'{"id":1,"result":{}}'
$null = [Console]::ReadLine()
$null = [Console]::ReadLine()
'{"id":2,"result":{"thread":{"id":"fixture-thread"},"approvalPolicy":"on-request","approvalsReviewer":"user","sandbox":{"type":"workspaceWrite"},"model":"fixture-model","reasoningEffort":"low"}}'
$null = [Console]::ReadLine()
$old = Join-Path 'zettels' ($env:FIXTURE_OLD + '.md')
$new = Join-Path 'zettels' ($env:FIXTURE_NEW + '.md')
$utf8 = [Text.UTF8Encoding]::new($false)
[IO.File]::WriteAllText($old, ([IO.File]::ReadAllText($old).Replace('original body', 'AI edited body')), $utf8)
[IO.File]::Copy($env:FIXTURE_TEMPLATE, $new)
$change = @{method='item/completed';params=@{threadId='fixture-thread';item=@{id='file-write';type='fileChange';status='completed';changes=@(@{path=$old;kind=@{type='update'}},@{path=$new;kind=@{type='add'}})}}} | ConvertTo-Json -Depth 12 -Compress
[Console]::WriteLine($change)
'{"method":"thread/tokenUsage/updated","params":{"threadId":"fixture-thread","tokenUsage":{"last":{"totalTokens":12},"modelContextWindow":1000}}}'
'{"method":"item/agentMessage/delta","params":{"delta":"hello teacher"}}'
'{"method":"turn/completed","params":{"turn":{"status":"completed"}}}'
"#).unwrap();
        let mut command = crate::process::hidden_async_command("powershell.exe");
        command
            .args(["-NoProfile", "-NonInteractive", "-File"])
            .arg(&script)
            .env("FIXTURE_OLD", &original_id)
            .env("FIXTURE_NEW", &new_id)
            .env("FIXTURE_TEMPLATE", &template_path)
            .current_dir(&archive_root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut events = Vec::new();
        crate::chat_codex::run_command(
            command,
            &paths.database_path,
            &archive_root,
            &chat_id,
            &retry_text,
            &[],
            None,
            None,
            None,
            None,
            None,
            &crate::permission_modes::codex_settings("Ask for approval").unwrap(),
            None,
            tokio_util::sync::CancellationToken::new(),
            None,
            None,
            None,
            |name, value| events.push((name.to_owned(), value)),
        )
        .await
        .expect("stage 2: fake CLI reply");
        let messages =
            serde_json::to_value(crate::chat::read(&paths.database_path, &chat_id).unwrap())
                .unwrap();
        assert_eq!(
            messages["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| message["role"] == "user")
                .count(),
            1,
            "stage 2: retry does not duplicate the user message"
        );
        assert!(
            crate::chat::retry_input(&paths.database_path, &chat_id, &sent.id)
                .unwrap()
                .is_none(),
            "stage 2: completed retry closes retry eligibility"
        );
        assert!(
            fs::read_to_string(
                archive_root
                    .join("zettels")
                    .join(format!("{original_id}.md"))
            )
            .unwrap()
            .contains("AI edited body"),
            "stage 4: fake AI edits document"
        );
        assert!(new_path.is_file(), "stage 4: fake AI creates document");
        let done = events
            .iter()
            .find(|(name, _)| name == "chat-done")
            .expect("stage 2: done event");
        assert_eq!(done.1["finishReason"], "completed");
        assert!(done.1.get("fastState").is_some());
        assert_eq!(
            serde_json::to_value(crate::chat_context::get(&paths.database_path, &chat_id).unwrap())
                .unwrap()["usedTokens"],
            12
        );
        assert!(
            !serde_json::to_value(crate::chat::read(&paths.database_path, &chat_id).unwrap())
                .unwrap()["title"]
                .as_str()
                .unwrap()
                .is_empty()
        );
        let text_file = root.join("lesson.txt");
        fs::write(&text_file, "original bytes").unwrap();
        let proposal_json = serde_json::json!({"sourcePath": text_file, "assetKind": "raw_transcript", "mediaType": "text/plain", "recordLink": null, "actor": "test"});
        let proposal = serde_json::from_value(proposal_json.clone()).unwrap();
        let plan = serde_json::to_value(
            crate::source_assets::plan_import(&paths.database_path, &paths.objects_path, proposal)
                .expect("stage 3: plan attachment"),
        )
        .unwrap();
        let input = serde_json::from_value(serde_json::json!({"proposal": proposal_json, "planDigest": plan["planDigest"], "importConfirmed": true})).unwrap();
        let asset = serde_json::to_value(
            crate::source_assets::apply_import(&paths.database_path, &paths.objects_path, input)
                .expect("stage 3: import attachment"),
        )
        .unwrap();
        let hash = asset["contentSha256"].as_str().unwrap();
        let reference = format!("sha256:{hash}");
        let object_relative = format!("sha256/{}/{hash}", &hash[..2]);
        let deleted_file = root.join("deleted-lesson.txt");
        fs::write(&deleted_file, "deleted original bytes").unwrap();
        let deleted_proposal_json = serde_json::json!({"sourcePath":deleted_file,"assetKind":"raw_transcript","mediaType":"text/plain","recordLink":null,"actor":"test"});
        let deleted_plan = serde_json::to_value(
            crate::source_assets::plan_import(
                &paths.database_path,
                &paths.objects_path,
                serde_json::from_value(deleted_proposal_json.clone()).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        let deleted_input = serde_json::from_value(serde_json::json!({"proposal":deleted_proposal_json,"planDigest":deleted_plan["planDigest"],"importConfirmed":true})).unwrap();
        let deleted_asset = serde_json::to_value(
            crate::source_assets::apply_import(
                &paths.database_path,
                &paths.objects_path,
                deleted_input,
            )
            .unwrap(),
        )
        .unwrap();
        let deleted_asset_id = deleted_asset["id"].as_str().unwrap();
        let deleted_reference = format!(
            "sha256:{}",
            deleted_asset["contentSha256"].as_str().unwrap()
        );
        crate::archive_receipt::attach_asset(
            &archive_root,
            &paths.database_path,
            &chat_id,
            deleted_asset_id,
        )
        .unwrap();
        let delete_plan = serde_json::to_value(
            crate::source_delete::preview(&archive_root, &paths.database_path, deleted_asset_id)
                .unwrap(),
        )
        .unwrap();
        crate::source_delete::trash_for_test(
            &archive_root,
            &paths.database_path,
            deleted_asset_id,
            delete_plan["planDigest"].as_str().unwrap(),
        )
        .unwrap();
        crate::archive_receipt::attach_asset(
            &archive_root,
            &paths.database_path,
            &chat_id,
            asset["id"].as_str().unwrap(),
        )
        .expect("stage 3: attach");
        struct FixtureCleanup(Vec<std::path::PathBuf>);
        impl Drop for FixtureCleanup {
            fn drop(&mut self) {
                #[cfg(windows)]
                for path in &self.0 {
                    let _ = crate::source_assets::clear_readonly_attribute(path);
                }
            }
        }
        let mut cleanup = FixtureCleanup(Vec::new());
        let mut media_fixtures: Vec<(String, Vec<u8>, String)> = Vec::new();
        let mut generation_receipts = Vec::new();
        if crash_first_receive {
            for (index, (extension, mime, base)) in [
                (
                    "png",
                    "image/png",
                    &include_bytes!("../icons/32x32.png")[..],
                ),
                (
                    "png",
                    "image/png",
                    &include_bytes!("../icons/128x128.png")[..],
                ),
                (
                    "webm",
                    "video/webm",
                    &include_bytes!("../tests/fixtures/d35-video-one.webm")[..],
                ),
                (
                    "webm",
                    "video/webm",
                    &include_bytes!("../tests/fixtures/d35-video-two.webm")[..],
                ),
            ]
            .into_iter()
            .enumerate()
            {
                let bytes = base.to_vec();
                let file = root.join(format!("generated-{index}.{extension}"));
                fs::write(&file, &bytes).unwrap();
                let proposal = serde_json::json!({"sourcePath":file,"assetKind":"other","mediaType":mime,"recordLink":null,"actor":"test"});
                let plan = serde_json::to_value(
                    crate::source_assets::plan_import(
                        &paths.database_path,
                        &paths.objects_path,
                        serde_json::from_value(proposal.clone()).unwrap(),
                    )
                    .unwrap(),
                )
                .unwrap();
                let input = serde_json::from_value(serde_json::json!({"proposal":proposal,"planDigest":plan["planDigest"],"importConfirmed":true})).unwrap();
                let item = serde_json::to_value(
                    crate::source_assets::apply_import(
                        &paths.database_path,
                        &paths.objects_path,
                        input,
                    )
                    .unwrap(),
                )
                .unwrap();
                let digest = item["contentSha256"].as_str().unwrap().to_owned();
                let relative = format!("sha256/{}/{digest}", &digest[..2]);
                for pc in ["momso", "storage-only-pc/momso", "new-pc/momso"] {
                    cleanup
                        .0
                        .push(root.join(pc).join("objects").join(&relative));
                }
                let reference = format!("sha256:{digest}");
                if index == 0 {
                    crate::archive_receipt::attach_asset(
                        &archive_root,
                        &paths.database_path,
                        &chat_id,
                        item["id"].as_str().unwrap(),
                    )
                    .unwrap();
                }
                let previous = if index == 1 || index == 3 {
                    vec![format!("sha256:{}", media_fixtures[index - 1].0)]
                } else {
                    vec![]
                };
                let receipt = crate::archive_receipt::create(
                    &archive_root,
                    &paths.database_path,
                    crate::archive_receipt::ReceiptCreate {
                        tool: "codex".into(),
                        model: "fixture-model".into(),
                        chat: Some(chat_id.clone()),
                        read: previous,
                        wrote: vec![reference],
                        prompt: format!("fixture generation prompt {index}"),
                        changes: format!("fixture version {index}"),
                    },
                )
                .unwrap();
                generation_receipts.push(receipt);
                media_fixtures.push((digest, bytes, mime.into()));
            }
        }
        let page = serde_json::to_value(
            crate::chat_items::read_attachment_text(
                &archive_root,
                &paths.database_path,
                &reference,
                0,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            page["content"], "original bytes",
            "stage 3: attachment text"
        );
        let id = crate::archive_zet::create(
            &archive_root,
            &paths.database_path,
            crate::archive_zet::ZetCreate {
                title: "document".into(),
                summary: "summary".into(),
                kind: "note".into(),
                body: "body".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        if crash_first_receive {
            crate::archive_zet::set_folder(
                &archive_root,
                &paths.database_path,
                &id,
                "classes/morning",
            )
            .unwrap();
            let nested_id = crate::archive_zet::create_in_folder(
                &archive_root,
                &paths.database_path,
                crate::archive_zet::ZetCreate {
                    title: "folder document".into(),
                    summary: "folder summary".into(),
                    kind: "note".into(),
                    body: "folder document body".into(),
                    assets: vec![],
                    edges: vec![],
                },
                "classes/evening",
            )
            .unwrap();
            crate::archive_zet::create_folder(&archive_root, &paths.database_path, "empty/folder")
                .unwrap();
            for document in [&id, &nested_id] {
                assert!(archive_root
                    .join("zettels")
                    .join(format!("{document}.md"))
                    .is_file());
            }
        }
        let tool = crate::process::resolve_tool("git.exe").unwrap();
        let bare = root.join("remote.git");
        fs::write(
            archive_root.join("dictionary/people.json"),
            r#"{"people":[{"id":"fixture-person","name":"fixture-person","confirmed":true}]}"#,
        )
        .unwrap();
        fs::write(
            archive_root.join("dictionary/my-terms.json"),
            r#"{"terms":[{"term":{"id":"fixture-term","sanskrit":"fixture-term","korean":[],"koreanNames":[],"meaning":"test","sources":[]},"baseMeaning":null}]}"#,
        )
        .unwrap();
        let custom_skill = archive_root.join("skills/fixture-skill");
        fs::create_dir_all(&custom_skill).unwrap();
        fs::write(
            custom_skill.join("SKILL.md"),
            "---\nname: fixture-skill\ntitle: fixture skill\ndescription: fixture description\n---\nfixture skill content\n",
        )
        .unwrap();
        crate::archive_remote::git(&tool, root, &["init", "--bare", bare.to_str().unwrap()])
            .unwrap();
        crate::archive_remote::git(
            &tool,
            &archive_root,
            &[
                "add",
                "--",
                ".gitignore",
                "AGENTS.md",
                "CLAUDE.md",
                "GEMINI.md",
                "zettels/",
                "receipts/",
                "dictionary/",
                "skills/",
                ".agents/skills/",
                ".claude/skills/",
            ],
        )
        .unwrap();
        crate::archive_remote::git(
            &tool,
            &archive_root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "original",
            ],
        )
        .unwrap();
        crate::archive::save_one(
            &archive_root,
            root,
            &paths.database_path,
            &id,
            crate::archive_zet::ZetUpdate {
                title: None,
                summary: None,
                body: Some("edited body".into()),
                status: None,
                assets: None,
                edges: None,
            },
            None,
        )
        .expect("stage 4: save edited document");
        assert!(
            crate::archive::history(&archive_root, root, &id)
                .unwrap()
                .len()
                >= 2,
            "stage 4: history"
        );
        let revision =
            crate::archive_remote::git(&tool, &archive_root, &["rev-parse", "HEAD"]).unwrap();
        let document_path = archive_root.join("zettels").join(format!("{id}.md"));
        fs::remove_file(&document_path).unwrap();
        crate::chat_recovery::restore_git(
            &archive_root,
            root,
            Some(&paths.database_path),
            &id,
            revision.trim(),
        )
        .expect("stage 4: recover deleted document");
        assert!(
            fs::read_to_string(&document_path)
                .unwrap()
                .contains("edited body"),
            "stage 4: restored content"
        );
        let document_listing = |root: &Path, database: &Path| {
            crate::momso_document_read::sync_document_index(root, database).unwrap();
            let mut entries = crate::archive_zet::list(database, None, None).unwrap();
            crate::archive_zet::add_folders(root, &mut entries).unwrap();
            serde_json::to_value(entries).unwrap()
        };
        let old_documents =
            crash_first_receive.then(|| document_listing(&archive_root, &paths.database_path));
        let old_folders =
            crash_first_receive.then(|| crate::archive_zet::folders(&archive_root).unwrap());
        if let Some(folders) = &old_folders {
            assert_eq!(
                folders,
                &[
                    "classes",
                    "classes/evening",
                    "classes/morning",
                    "empty",
                    "empty/folder"
                ]
            );
        }
        assert!(
            archive_root
                .join("skills/object-store-connect/SKILL.md")
                .is_file(),
            "stage 5: builtin skill"
        );
        assert!(fake_skill_connection(&chat_id, true).await.is_none());
        let selected_bucket = fake_skill_connection(&chat_id, false)
            .await
            .expect("stage 5: selected fake bucket");
        let group = serde_json::to_value(
            crate::chat_group::create(&paths.database_path, "fixture group").unwrap(),
        )
        .unwrap();
        let group_id = group["id"].as_str().unwrap();
        crate::chat_group::set_group(&paths.database_path, &chat_id, Some(group_id.to_owned()))
            .unwrap();
        crate::chat::rename(&paths.database_path, &chat_id, "fixture room").unwrap();
        assert_eq!(
            crate::chat::set_pinned(&paths.database_path, &chat_id, true).state,
            "ok"
        );
        let secret = format!("sk-proj-{}", "Z".repeat(30));
        let connection_secret = format!("sk-proj-{}", "Q".repeat(30));
        if crash_first_receive {
            crate::credentials::put_secret_unlocked("ai:openai:api-key", &connection_secret)
                .unwrap();
        }
        crate::chat::append(
            &paths.database_path,
            &chat_id,
            crate::chat::Role::User,
            &secret,
            None,
            None,
        )
        .unwrap();
        crate::chat_archive_store::write(&archive_root, &paths, &chat_id).unwrap();
        let archived = serde_json::to_value(crate::chat::create(&paths.database_path).unwrap())
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        crate::chat::append(
            &paths.database_path,
            &archived,
            crate::chat::Role::User,
            "archived message",
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            crate::chat::set_archived(&paths.database_path, &archived, true).state,
            "ok"
        );
        crate::chat_archive_store::write(&archive_root, &paths, &archived).unwrap();
        let imported_file = root.join("fixture-import.json");
        fs::write(&imported_file, serde_json::json!([{"id":"fixture-import","current_node":"a","mapping":{"a":{"parent":null,"message":{"author":{"role":"user"},"content":{"parts":["imported message"]}}}}}]).to_string()).unwrap();
        crate::chat_export_import::import(&paths.database_path, &imported_file, "chatgpt").unwrap();
        let imported_id: String = storage::open_writable_connection(&paths.database_path)
            .unwrap()
            .query_row(
                "SELECT chat_id FROM chat_imports WHERE source_id='fixture-import'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        crate::chat_archive_store::write(&archive_root, &paths, &imported_id).unwrap();
        while !crate::chat_redaction_recheck::step(&archive_root, &paths)
            .unwrap()
            .available
        {}
        crate::chat_archive_store::save_pending_for_push(&archive_root, root, &paths).unwrap();
        crate::archive_remote::transfer(
            &tool,
            &archive_root,
            bare.to_str().unwrap(),
            "main",
            "push",
        )
        .unwrap();
        let profile = crate::backup_profile::ensure(&paths.database_path).unwrap();
        let aligned_snapshot = root.join("same-point-in-time.db");
        storage::create_snapshot_file(&paths.database_path, &aligned_snapshot).unwrap();
        storage::sanitize_user_owned_backup_snapshot(&aligned_snapshot).unwrap();
        let db = storage::open_writable_connection(&paths.database_path).unwrap();
        db.execute("INSERT INTO app_runtime_meta(key,value,updated_at) VALUES('test_state','before','now')", []).unwrap();
        db.execute(
            "UPDATE chats SET pinned_order=7,archived_at='archived' WHERE id=?1",
            [&chat_id],
        )
        .unwrap();
        db.execute(
            "UPDATE chat_messages SET fast=1,retryable_fast=1 WHERE id=?1",
            [&sent.id],
        )
        .unwrap();
        db.execute("INSERT INTO archive_location_settings(id,path,initialized) VALUES(1,'foreign-machine',1) ON CONFLICT(id) DO UPDATE SET path='foreign-machine'", []).unwrap();
        drop(db);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let objects = Arc::new(Mutex::new(HashMap::<String, Vec<u8>>::new()));
        let folder_backed = provider == "s3_compatible";
        let object_folder = root.join("fixture-object-backup");
        fs::create_dir_all(&object_folder).unwrap();
        let server_folder = object_folder.clone();
        let remote = Arc::clone(&objects);
        let stopped = Arc::new(AtomicBool::new(false));
        let reconnected = Arc::new(AtomicBool::new(false));
        let server_reconnected = reconnected.clone();
        let server_stopped = stopped.clone();
        let server_bucket = selected_bucket.clone();
        let server = std::thread::spawn(move || {
            while !server_stopped.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(15)))
                    .unwrap();
                let mut received = Vec::new();
                let mut buffer = [0u8; 16384];
                let end = loop {
                    let n = stream.read(&mut buffer).unwrap();
                    assert_ne!(n, 0);
                    received.extend_from_slice(&buffer[..n]);
                    if let Some(at) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                };
                let head = String::from_utf8(received[..end].to_vec()).unwrap();
                if ncp {
                    assert!(head.lines().any(|line| {
                        line.to_ascii_lowercase()
                            .starts_with("authorization: aws4-hmac-sha256 ")
                            && line.contains("/kr-standard/s3/aws4_request")
                    }));
                    assert!(!head.to_ascii_lowercase().contains("if-none-match:"));
                }
                let mut line = head.lines().next().unwrap().split_whitespace();
                let method = line.next().unwrap();
                let path = line.next().unwrap();
                let oauth_path = format!(
                    "/client/v4/accounts/{}/r2/buckets/momso-fixture/objects",
                    "a".repeat(32)
                );
                let oauth_bucket = oauth_path.strip_suffix("/objects").unwrap();
                if oauth && crash_first_receive && path == oauth_bucket {
                    server_reconnected.store(true, Ordering::SeqCst);
                }
                if oauth && crash_first_receive && server_reconnected.load(Ordering::SeqCst) {
                    assert!(head
                        .lines()
                        .any(|line| line.eq_ignore_ascii_case("authorization: Bearer test-token")));
                }
                if oauth {
                    assert!(
                        path == oauth_path
                            || path.starts_with(&format!("{oauth_path}/"))
                            || path.starts_with(&format!("{oauth_path}?"))
                            || (crash_first_receive && path == oauth_bucket)
                    );
                }
                let oauth_prefix = format!("{oauth_path}/");
                let s3_prefix = format!("/{server_bucket}/");
                let key = path
                    .trim_start_matches(if oauth { &oauth_prefix } else { &s3_prefix })
                    .split('?')
                    .next()
                    .unwrap();
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while received.len() - end < length {
                    let n = stream.read(&mut buffer).unwrap();
                    assert_ne!(n, 0);
                    received.extend_from_slice(&buffer[..n]);
                }
                let (status, body) = if oauth
                    && crash_first_receive
                    && method == "GET"
                    && path == oauth_bucket
                {
                    (
                        "200 OK",
                        serde_json::to_vec(
                            &serde_json::json!({"success":true,"result":{"name":"momso-fixture"}}),
                        )
                        .unwrap(),
                    )
                } else if method == "GET"
                    && ((oauth && path.starts_with(&format!("{oauth_path}?")))
                        || (!oauth && path.starts_with(&format!("/{server_bucket}?"))))
                {
                    if oauth {
                        let data_list = path.contains("data%2F");
                        let entries: Vec<_> = remote
                            .lock()
                            .unwrap()
                            .keys()
                            .filter(|key| {
                                key.contains(if data_list { "/data/" } else { "/manifests/" })
                            })
                            .map(|key| serde_json::json!({"key":key}))
                            .collect();
                        ("200 OK", serde_json::to_vec(&serde_json::json!({"success":true,"result":entries,"result_info":{"is_truncated":false}})).unwrap())
                    } else {
                        let entries = remote
                            .lock()
                            .unwrap()
                            .keys()
                            .filter(|key| key.contains("/manifests/"))
                            .map(|key| format!("<Contents><Key>{key}</Key></Contents>"))
                            .collect::<String>();
                        ("200 OK", format!("<ListBucketResult><IsTruncated>false</IsTruncated>{entries}</ListBucketResult>").into_bytes())
                    }
                } else if method == "PUT" {
                    let mut data = remote.lock().unwrap();
                    assert!(!data.contains_key(key) || key.contains("/class-student/slot-"));
                    let value = received[end..end + length].to_vec();
                    if folder_backed {
                        let path = server_folder.join(key);
                        fs::create_dir_all(path.parent().unwrap()).unwrap();
                        fs::write(path, &value).unwrap();
                    }
                    data.insert(key.to_owned(), value);
                    ("200 OK", Vec::new())
                } else {
                    if folder_backed {
                        match fs::read(server_folder.join(key)) {
                            Ok(data) => ("200 OK", data),
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                ("404 Not Found", Vec::new())
                            }
                            Err(error) => panic!("fixture read failed: {error}"),
                        }
                    } else {
                        match remote.lock().unwrap().get(key) {
                            Some(data) => ("200 OK", data.clone()),
                            None => ("404 Not Found", Vec::new()),
                        }
                    }
                };
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let config = object_store::Config {
            provider: if oauth { "cloudflare" } else { provider }.into(),
            endpoint: format!("http://{address}"),
            region: if oauth {
                "wrangleroauth"
            } else if ncp {
                "kr-standard"
            } else {
                "auto"
            }
            .into(),
            bucket: if oauth {
                "momso-fixture".into()
            } else {
                selected_bucket
            },
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(if oauth || ncp {
                object_store::WriteSafety::CheckThenWrite
            } else {
                object_store::WriteSafety::Conditional
            }),
        };
        let keys = object_store::Keys {
            access_key_id: "TEST".into(),
            secret_access_key: "test".into(),
        };
        let identity = x25519::Identity::generate();
        let _original = transfer::upload_file(
            &config,
            &keys,
            &identity,
            transfer::UploadSpec {
                source: &paths.objects_path.join(&object_relative),
                relative: &object_relative,
                kind: "object",
                stage: &root.join("stage").join("upload"),
            },
            |_, _| {},
        )
        .await
        .unwrap();
        for (index, (digest, _, _)) in media_fixtures.iter().enumerate() {
            let relative = format!("sha256/{}/{digest}", &digest[..2]);
            transfer::upload_file(
                &config,
                &keys,
                &identity,
                transfer::UploadSpec {
                    source: &paths.objects_path.join(&relative),
                    relative: &relative,
                    kind: "object",
                    stage: &root.join("stage").join(format!("media-{index}")),
                },
                |_, _| {},
            )
            .await
            .unwrap();
        }
        let state_manifest = backup_snapshot(&config, &keys, &identity, &paths)
            .await
            .unwrap();
        // An object-store-only recovery restores the database and objects, not Git files.
        let storage_pc = root.join("storage-only-pc");
        fs::create_dir_all(&storage_pc).unwrap();
        let storage_db = storage_pc.join("momso.db");
        let storage_backups = storage_pc.join("backups");
        fs::create_dir_all(&storage_backups).unwrap();
        storage::initialize(&storage_db, &storage_backups, "test").unwrap();
        let storage_archive = storage_pc.join("momso");
        let storage_location =
            crate::archive_location::ArchiveLocation::load(storage_db.clone(), &storage_pc, vec![])
                .unwrap();
        crate::archive::initialize(&storage_archive, &storage_pc).unwrap();
        let storage_paths = RuntimePaths {
            data_root: storage_pc.clone(),
            database_path: storage_db.clone(),
            objects_path: storage_archive.join("objects"),
            exports_path: storage_pc.join("exports"),
            backups_path: storage_backups,
            logs_path: storage_pc.join("logs"),
            archive: storage_location,
            archive_lease: None,
        };
        assert!(matches!(
            transfer::identity(&storage_db),
            Err("recovery_key_missing")
        ));
        assert!(matches!(
            object_store_restore::download_verified(
                &config,
                &keys,
                &x25519::Identity::generate(),
                &state_manifest,
                &storage_paths.backups_path
            )
            .await,
            Err("decryption_failed")
        ));
        let chunk = transfer::chunk_key(&state_manifest, 0);
        if folder_backed {
            let ciphertext = fs::read(object_folder.join(&chunk)).expect("local backup folder");
            assert!(!ciphertext
                .windows(b"original bytes".len())
                .any(|part| part == b"original bytes"));
        }
        let original_chunk = {
            let mut remote = objects.lock().unwrap();
            let data = remote.get_mut(&chunk).expect("state backup chunk");
            let original = data.clone();
            data[0] ^= 1;
            if folder_backed {
                fs::write(object_folder.join(&chunk), &*data).unwrap();
            }
            original
        };
        assert!(matches!(
            object_store_restore::download_verified(
                &config,
                &keys,
                &identity,
                &state_manifest,
                &storage_paths.backups_path
            )
            .await,
            Err("hash_mismatch")
        ));
        if folder_backed {
            fs::write(object_folder.join(&chunk), &original_chunk).unwrap();
        }
        objects.lock().unwrap().insert(chunk, original_chunk);
        assert!(crate::chat::list(&storage_db).unwrap().is_empty());
        if oauth {
            let crm_path = paths.data_root.join(crate::class_student_backup::FILE_NAME);
            let crm = Connection::open(&crm_path).unwrap();
            crm.execute_batch(
                "CREATE TABLE fixture(value TEXT); INSERT INTO fixture VALUES('fictional');",
            )
            .unwrap();
            drop(crm);
            crate::class_student_backup::backup_inner(&config, &keys, &identity, &paths)
                .await
                .unwrap();
            crate::class_student_backup::backup_inner(&config, &keys, &identity, &paths)
                .await
                .unwrap();
            {
                let snapshots = objects.lock().unwrap();
                let slots: Vec<_> = snapshots
                    .iter()
                    .filter(|(key, _)| key.contains("/class-student/slot-"))
                    .collect();
                assert_eq!(slots.len(), 1);
                assert!(!slots[0]
                    .1
                    .windows(b"fictional".len())
                    .any(|window| window == b"fictional"));
            }
            let restored =
                crate::class_student_backup::download(&config, &keys, &identity, &storage_paths)
                    .await
                    .unwrap()
                    .unwrap();
            crate::class_student_backup::install(restored, &storage_paths.data_root).unwrap();
            let restored = Connection::open(
                storage_paths
                    .data_root
                    .join(crate::class_student_backup::FILE_NAME),
            )
            .unwrap();
            assert_eq!(
                restored
                    .query_row("SELECT value FROM fixture", [], |row| row
                        .get::<_, String>(0))
                    .unwrap(),
                "fictional"
            );
            drop(restored);
            crate::class_student_backup::restore_if_missing(
                &config,
                &keys,
                &identity,
                &storage_paths,
            )
            .await
            .unwrap();
            let changed = Connection::open(
                storage_paths
                    .data_root
                    .join(crate::class_student_backup::FILE_NAME),
            )
            .unwrap();
            changed
                .execute("INSERT INTO fixture VALUES('local')", [])
                .unwrap();
            drop(changed);
            crate::class_student_backup::restore_if_missing(
                &config,
                &keys,
                &identity,
                &storage_paths,
            )
            .await
            .unwrap();
            let recovered = fs::read_dir(&storage_paths.data_root)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("class-student-recovered-")
                })
                .count();
            assert_eq!(recovered, 1);
            let old_database = fs::read(&crm_path).unwrap();
            {
                let mut remote = objects.lock().unwrap();
                for number in 0..7 {
                    let mut plain = b"MOMSOCR1".to_vec();
                    let at =
                        chrono::Utc::now().timestamp_millis() - (8 + number as i64) * 86_400_000;
                    plain.extend_from_slice(&at.to_be_bytes());
                    plain.extend_from_slice(&old_database);
                    remote.insert(
                        format!("{}/class-student/slot-{number}.age", object_store::PREFIX),
                        transfer::encrypt_bytes(&plain, &identity).unwrap(),
                    );
                }
            }
            let changed = Connection::open(&crm_path).unwrap();
            changed
                .execute("INSERT INTO fixture VALUES('next')", [])
                .unwrap();
            drop(changed);
            crate::class_student_backup::backup_inner(&config, &keys, &identity, &paths)
                .await
                .unwrap();
            let slots = objects
                .lock()
                .unwrap()
                .keys()
                .filter(|key| key.contains("/class-student/slot-"))
                .count();
            assert_eq!(slots, 7);
            let next =
                crate::class_student_backup::download(&config, &keys, &identity, &storage_paths)
                    .await
                    .unwrap()
                    .unwrap();
            let next = Connection::open(next.path()).unwrap();
            assert_eq!(
                next.query_row("SELECT count(*) FROM fixture", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                2
            );
        }
        let (storage_snapshot, _) =
            download_latest_snapshot(&config, &keys, &identity, &storage_paths)
                .await
                .unwrap();
        assert_eq!(
            ensure_local_content_preserved(&storage_db, storage_snapshot.path()),
            Ok(())
        );
        apply_snapshot(storage_snapshot.path(), &storage_paths).unwrap();
        object_store_restore::restore_selected(&config, &keys, &identity, &storage_paths, None)
            .await
            .unwrap();
        assert_eq!(
            fs::read(storage_paths.objects_path.join(&object_relative)).unwrap(),
            b"original bytes",
            "storage only: original restored"
        );
        assert_eq!(
            transfer::hash_file(&storage_paths.objects_path.join(&object_relative))
                .unwrap()
                .0,
            hash,
            "the restored attachment must match the original SHA-256"
        );
        assert_eq!(
            serde_json::to_value(crate::chat::read(&storage_db, &chat_id).unwrap()).unwrap()
                ["title"],
            "fixture room",
            "storage only: chat from application snapshot"
        );
        assert_eq!(
            crate::permission_modes::get(&storage_db, crate::permission_modes::Provider::Codex)
                .unwrap(),
            "Full access"
        );
        let deleted_count: i64 = storage::open_writable_connection(&storage_db)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM source_asset_deletions WHERE asset_id=?1 AND state='trashed'",
                [deleted_asset_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(deleted_count, 1);
        assert!(
            !storage_archive
                .join("zettels")
                .join(format!("{id}.md"))
                .exists(),
            "storage only: document files need Git"
        );
        #[cfg(windows)]
        crate::source_assets::clear_readonly_attribute(
            &storage_paths.objects_path.join(&object_relative),
        )
        .unwrap();
        let source_archive_root = archive_root.clone();
        if crash_first_receive {
            assert_eq!(
                crate::credentials::get_secret("ai:openai:api-key")
                    .unwrap()
                    .as_deref(),
                Some(connection_secret.as_str())
            );
            crate::credentials::remove_secret_unlocked("ai:openai:api-key").unwrap();
            assert!(crate::credentials::get_secret("ai:openai:api-key")
                .unwrap()
                .is_none());
        }
        let new_pc = root.join("new-pc");
        fs::create_dir_all(&new_pc).unwrap();
        let database_path = new_pc.join("momso.db");
        let backups_path = new_pc.join("backups");
        fs::create_dir_all(&backups_path).unwrap();
        storage::initialize(&database_path, &backups_path, "test").unwrap();
        let archive =
            crate::archive_location::ArchiveLocation::load(database_path.clone(), &new_pc, vec![])
                .unwrap();
        let archive_root = new_pc.join("momso");
        crate::archive::initialize(&archive_root, &new_pc).unwrap();
        crate::archive_remote::connect_local(&tool, &archive_root, bare.to_str().unwrap()).unwrap();
        if crash_first_receive {
            let status = crate::process::hidden_command(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "archive_remote::tests::first_receive_checkout_crash_child",
                    "--nocapture",
                ])
                .env("MOMSO_C28_ROOT", &archive_root)
                .env("MOMSO_C28_DB", &database_path)
                .env("MOMSO_C28_GIT", &tool)
                .env("MOMSO_C28_TARGET", &bare)
                .env("MOMSO_C28_CRASH_AFTER_CHECKOUT", "1")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert_eq!(
                status.code(),
                Some(73),
                "first receive is interrupted after checkout"
            );
        }
        crate::archive_remote::receive_connected(
            &tool,
            &archive_root,
            &database_path,
            bare.to_str().unwrap(),
            "main",
        )
        .unwrap();
        if crash_first_receive {
            assert_eq!(
                document_listing(&archive_root, &database_path),
                *old_documents.as_ref().unwrap(),
                "zet_list matches the old PC after interrupted first receive"
            );
            assert_eq!(
                crate::archive_zet::folders(&archive_root).unwrap(),
                *old_folders.as_ref().unwrap()
            );
            for (index, receipt) in generation_receipts.iter().enumerate() {
                let text =
                    fs::read_to_string(archive_root.join("receipts").join(format!("{receipt}.md")))
                        .unwrap();
                assert!(text.contains(&format!("fixture generation prompt {index}")));
                let (head, _) = crate::archive_receipt::parse(&text).unwrap();
                assert_eq!(
                    head.wrote,
                    vec![format!("sha256:{}", media_fixtures[index].0)]
                );
                if index == 1 || index == 3 {
                    assert_eq!(
                        head.read,
                        vec![format!("sha256:{}", media_fixtures[index - 1].0)]
                    );
                }
            }
            assert!(
                object_store::load_config(&new_pc).unwrap().is_none(),
                "new PC must reconnect the object store"
            );
        }
        assert!(
            archive_root
                .join("zettels")
                .join(format!("{id}.md"))
                .is_file(),
            "stage 6: document from Git"
        );
        assert!(
            archive_root
                .join("skills/github-connect/SKILL.md")
                .is_file(),
            "stage 6: builtin skill from Git"
        );
        assert!(
            archive_root.join("dictionary/yoga.json").is_file(),
            "stage 6: bundled dictionary"
        );
        assert!(
            fs::read_to_string(archive_root.join("dictionary/people.json"))
                .unwrap()
                .contains("fixture-person"),
            "stage 6: people file"
        );
        assert!(
            fs::read_to_string(archive_root.join("dictionary/my-terms.json"))
                .unwrap()
                .contains("fixture-term")
        );
        let skill = fs::read_to_string(archive_root.join("skills/fixture-skill/SKILL.md")).unwrap();
        assert!(crate::skills::parse("fixture-skill", &skill).is_ok());
        assert!(
            serde_json::to_value(crate::yoga_dictionary::list(&archive_root).unwrap())
                .unwrap()
                .to_string()
                .contains("fixture-term")
        );
        let archived_db = storage::open_writable_connection(&database_path).unwrap();
        assert!(archived_db
            .query_row(
                "SELECT archived_at FROM chats WHERE id=?1",
                [&archived],
                |row| row.get::<_, Option<String>>(0)
            )
            .unwrap()
            .is_some());
        drop(archived_db);
        assert_eq!(
            serde_json::to_value(
                crate::chat_export_import::import(&database_path, &imported_file, "chatgpt")
                    .unwrap()
            )
            .unwrap()["duplicate"],
            1,
            "Git only: repeated import remains deduplicated"
        );
        let github_profile =
            serde_json::to_value(crate::archive::profile_get(&archive_root).unwrap()).unwrap();
        assert_eq!(
            github_profile["name"], "Teacher",
            "stage 6: profile from Git"
        );
        let restored_chat =
            serde_json::to_value(crate::chat::read(&database_path, &chat_id).unwrap()).unwrap();
        assert_eq!(
            restored_chat["title"], "fixture room",
            "stage 6: chat from Git"
        );
        assert!(
            !restored_chat.to_string().contains(&secret),
            "stage 6: redacted copy"
        );
        let summaries = serde_json::to_value(crate::chat::list(&database_path).unwrap()).unwrap();
        assert_eq!(summaries[0]["pinned"], true);
        assert_eq!(summaries[0]["groupId"], group_id);
        let before = restored_chat["messages"].as_array().unwrap().len();
        crate::chat_archive_store::restore(&archive_root, &database_path).unwrap();
        let repeated =
            serde_json::to_value(crate::chat::read(&database_path, &chat_id).unwrap()).unwrap();
        assert_eq!(repeated["messages"].as_array().unwrap().len(), before);
        assert!(
            crate::archive::history(&archive_root, &new_pc, &id)
                .unwrap()
                .len()
                >= 2,
            "Git only: document history"
        );
        let new_paths = RuntimePaths {
            data_root: new_pc.clone(),
            database_path: database_path.clone(),
            objects_path: archive_root.join("objects"),
            exports_path: new_pc.join("exports"),
            backups_path: backups_path.clone(),
            logs_path: new_pc.join("logs"),
            archive: crate::archive_location::ArchiveLocation::load(
                database_path.clone(),
                &new_pc,
                vec![],
            )
            .unwrap(),
            archive_lease: None,
        };
        let (different_time_snapshot, _) =
            download_latest_snapshot(&config, &keys, &identity, &new_paths)
                .await
                .unwrap();
        assert_eq!(
            ensure_local_content_preserved(&database_path, different_time_snapshot.path()),
            Err("local_data_conflict"),
            "different Git and DB backup states must not overwrite each other"
        );
        let mut after_git_snapshot = None;
        if folder_backed {
            let aligned = transfer::upload_file(
                &config,
                &keys,
                &identity,
                transfer::UploadSpec {
                    source: &aligned_snapshot,
                    relative: "momso.db",
                    kind: "app_state",
                    stage: &root.join("stage").join("aligned"),
                },
                |_, _| {},
            )
            .await
            .unwrap();
            let snapshot = object_store_restore::download_verified(
                &config,
                &keys,
                &identity,
                &aligned,
                &new_paths.backups_path,
            )
            .await
            .unwrap();
            assert_eq!(
                ensure_local_content_preserved(&database_path, snapshot.path()),
                Ok(()),
                "a same-point-in-time backup must be restorable"
            );
            after_git_snapshot = Some(snapshot);
            // The same new profile receives Git and then its encrypted DB and files.
            let combined = root.join("combined-new-pc");
            fs::create_dir_all(&combined).unwrap();
            let combined_db = combined.join("momso.db");
            let combined_backups = combined.join("backups");
            storage::initialize(&combined_db, &combined_backups, "test").unwrap();
            let combined_archive = combined.join("momso");
            let combined_location = crate::archive_location::ArchiveLocation::load(
                combined_db.clone(),
                &combined,
                vec![],
            )
            .unwrap();
            crate::archive::initialize(&combined_archive, &combined).unwrap();
            crate::archive_remote::connect_local(&tool, &combined_archive, bare.to_str().unwrap())
                .unwrap();
            crate::archive_remote::receive_connected(
                &tool,
                &combined_archive,
                &combined_db,
                bare.to_str().unwrap(),
                "main",
            )
            .unwrap();
            let combined_paths = RuntimePaths {
                data_root: combined.clone(),
                database_path: combined_db.clone(),
                objects_path: combined_archive.join("objects"),
                exports_path: combined.join("exports"),
                backups_path: combined_backups,
                logs_path: combined.join("logs"),
                archive: combined_location,
                archive_lease: None,
            };
            let snapshot = object_store_restore::download_verified(
                &config,
                &keys,
                &identity,
                &aligned,
                &combined_paths.backups_path,
            )
            .await
            .unwrap();
            assert_eq!(
                ensure_local_content_preserved(&combined_db, snapshot.path()),
                Ok(())
            );
            apply_snapshot(snapshot.path(), &combined_paths).unwrap();
            object_store_restore::restore_selected(
                &config,
                &keys,
                &identity,
                &combined_paths,
                None,
            )
            .await
            .unwrap();
            assert!(combined_archive
                .join("zettels")
                .join(format!("{id}.md"))
                .is_file());
            assert_eq!(
                transfer::hash_file(&combined_paths.objects_path.join(&object_relative))
                    .unwrap()
                    .0,
                hash
            );
            assert_eq!(
                serde_json::to_value(crate::chat::read(&combined_db, &chat_id).unwrap()).unwrap()
                    ["title"],
                "fixture room"
            );
            #[cfg(windows)]
            {
                crate::source_assets::clear_readonly_attribute(
                    &combined_paths.objects_path.join(&object_relative),
                )
                .unwrap();
                for (digest, _, _) in &media_fixtures {
                    let relative = format!("sha256/{}/{digest}", &digest[..2]);
                    crate::source_assets::clear_readonly_attribute(
                        &combined_paths.objects_path.join(relative),
                    )
                    .unwrap();
                }
            }
            // Leave the original newer fixture visible to the rest of this test.
            let manifest_key = format!(
                "{}/manifests/{}.age",
                object_store::PREFIX,
                aligned.upload_id
            );
            objects.lock().unwrap().remove(&manifest_key);
            fs::remove_file(object_folder.join(manifest_key)).unwrap();
        }
        crate::chat::append(
            &database_path,
            &chat_id,
            crate::chat::Role::User,
            "continued on the new device",
            None,
            None,
        )
        .unwrap();
        if let Some(snapshot) = &after_git_snapshot {
            assert_eq!(
                ensure_local_content_preserved(&database_path, snapshot.path()),
                Err("local_data_conflict"),
                "the old backup must not replace a new PC's later conversation"
            );
        }
        let reply_script = new_pc.join("fake-codex-reply.ps1");
        fs::write(&reply_script, r#"
$null = [Console]::ReadLine()
'{"id":1,"result":{}}'
$null = [Console]::ReadLine()
$null = [Console]::ReadLine()
'{"id":2,"result":{"thread":{"id":"fixture-new-thread"},"approvalPolicy":"on-request","approvalsReviewer":"user","sandbox":{"type":"workspaceWrite"},"model":"fixture-model","reasoningEffort":"low"}}'
$null = [Console]::ReadLine()
'{"method":"item/agentMessage/delta","params":{"delta":"new device answer"}}'
'{"method":"turn/completed","params":{"turn":{"status":"completed"}}}'
"#).unwrap();
        let mut reply_command = crate::process::hidden_async_command("powershell.exe");
        reply_command
            .args(["-NoProfile", "-NonInteractive", "-File"])
            .arg(&reply_script)
            .current_dir(&archive_root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        crate::chat_codex::run_command(
            reply_command,
            &database_path,
            &archive_root,
            &chat_id,
            "continued on the new device",
            &[],
            None,
            None,
            None,
            None,
            None,
            &crate::permission_modes::codex_settings("Ask for approval").unwrap(),
            None,
            tokio_util::sync::CancellationToken::new(),
            None,
            None,
            None,
            |_, _| {},
        )
        .await
        .unwrap();
        crate::chat_archive_store::write(&archive_root, &new_paths, &chat_id).unwrap();
        crate::archive::save_one(
            &archive_root,
            &new_pc,
            &database_path,
            &id,
            crate::archive_zet::ZetUpdate {
                title: None,
                summary: None,
                body: Some("continued document".into()),
                status: None,
                assets: None,
                edges: None,
            },
            None,
        )
        .unwrap();
        while !crate::chat_redaction_recheck::step(&archive_root, &new_paths)
            .unwrap()
            .available
        {}
        crate::chat_archive_store::save_pending_for_push(&archive_root, &new_pc, &new_paths)
            .unwrap();
        crate::archive_remote::transfer(
            &tool,
            &archive_root,
            bare.to_str().unwrap(),
            "main",
            "push",
        )
        .unwrap();
        crate::archive_remote::transfer(
            &tool,
            &source_archive_root,
            bare.to_str().unwrap(),
            "main",
            "pull",
        )
        .unwrap();
        crate::archive::reindex(&source_archive_root, &paths.database_path).unwrap();
        crate::chat_archive_store::restore(&source_archive_root, &paths.database_path).unwrap();
        assert!(
            fs::read_to_string(source_archive_root.join("zettels").join(format!("{id}.md")))
                .unwrap()
                .contains("continued document"),
            "one device at a time: old device receives edited document"
        );
        assert!(
            serde_json::to_value(crate::chat::read(&paths.database_path, &chat_id).unwrap())
                .unwrap()
                .to_string()
                .contains("continued on the new device"),
            "one device at a time: old device receives new message"
        );
        assert!(
            serde_json::to_value(crate::chat::read(&paths.database_path, &chat_id).unwrap())
                .unwrap()
                .to_string()
                .contains("new device answer")
        );
        let active = crate::chat_codex::Active::default();
        let deleted =
            crate::chat::delete_with(&paths.database_path, &active, &chat_id, |_| async {
                Ok(())
            })
            .await;
        assert_eq!(serde_json::to_value(deleted).unwrap()["state"], "deleted");
        crate::chat_archive_store::remove(&source_archive_root, &paths.database_path, &chat_id)
            .unwrap();
        while !crate::chat_redaction_recheck::step(&source_archive_root, &paths)
            .unwrap()
            .available
        {}
        crate::chat_archive_store::save_pending_for_push(&source_archive_root, root, &paths)
            .unwrap();
        crate::archive_remote::transfer(
            &tool,
            &source_archive_root,
            bare.to_str().unwrap(),
            "main",
            "push",
        )
        .unwrap();
        let third = root.join("third-pc");
        fs::create_dir_all(&third).unwrap();
        let third_db = third.join("momso.db");
        storage::initialize(&third_db, &third.join("backups"), "test").unwrap();
        let third_archive_location =
            crate::archive_location::ArchiveLocation::load(third_db.clone(), &third, vec![])
                .unwrap();
        let third_archive = third.join("momso");
        crate::archive::initialize(&third_archive, &third).unwrap();
        crate::archive_remote::connect_local(&tool, &third_archive, bare.to_str().unwrap())
            .unwrap();
        crate::archive_remote::receive_connected(
            &tool,
            &third_archive,
            &third_db,
            bare.to_str().unwrap(),
            "main",
        )
        .unwrap();
        assert!(
            crate::chat::read(&third_db, &chat_id).is_err(),
            "stage 6: deleted chat stays deleted"
        );
        let third_paths = RuntimePaths {
            data_root: third.clone(),
            database_path: third_db.clone(),
            objects_path: third_archive.join("objects"),
            exports_path: third.join("exports"),
            backups_path: third.join("backups"),
            logs_path: third.join("logs"),
            archive: third_archive_location,
            archive_lease: None,
        };
        let (old_snapshot, _) = download_latest_snapshot(&config, &keys, &identity, &third_paths)
            .await
            .unwrap();
        apply_snapshot(old_snapshot.path(), &third_paths).unwrap();
        assert!(
            crate::chat::read(&third_db, &chat_id).is_err(),
            "old app backup must not resurrect chat after Git received its deletion"
        );
        assert!(
            !archive_root.join("objects").join(&object_relative).exists(),
            "stage 6: Git alone does not restore originals"
        );
        assert_eq!(
            crate::permission_modes::get(&database_path, crate::permission_modes::Provider::Codex)
                .unwrap(),
            "Ask for approval"
        );
        let missing = serde_json::to_value(
            crate::chat_items::read_attachment_text(
                &archive_root,
                &database_path,
                &deleted_reference,
                0,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(missing["state"], "not_found");
        assert!(missing["content"].is_null());
        let items = serde_json::to_value(
            crate::chat_items::list(&archive_root, &database_path, &chat_id).unwrap(),
        )
        .unwrap();
        assert!(!items["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["id"] == deleted_reference));
        assert!(!items["deleted"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["id"] == deleted_reference));
        let paths = RuntimePaths {
            data_root: new_pc.clone(),
            database_path,
            objects_path: archive_root.join("objects"),
            exports_path: new_pc.join("exports"),
            backups_path,
            logs_path: new_pc.join("logs"),
            archive,
            archive_lease: None,
        };
        crate::backup_profile::ensure(&paths.database_path).unwrap();
        let db = storage::open_writable_connection(&paths.database_path).unwrap();
        db.execute(
            "INSERT INTO app_runtime_meta(key,value,updated_at) VALUES('test_state','after','now')",
            [],
        )
        .unwrap();
        drop(db);
        let reconnected_config = if crash_first_receive {
            #[cfg(windows)]
            {
                let config = crate::r2_oauth::fake_reconnect_existing(
                    &format!("http://{address}/"),
                    &new_pc,
                )
                .await
                .expect("new PC reconnects to the existing account-scoped R2 bucket");
                let stored = fs::read(new_pc.join("object-store.json")).unwrap();
                assert!(!stored
                    .windows(b"test-token".len())
                    .any(|part| part == b"test-token"));
                assert_eq!(config.bucket, "momso-fixture");
                assert!(reconnected.load(Ordering::SeqCst));
                config
            }
            #[cfg(not(windows))]
            {
                unreachable!("the reconnect fixture is Windows-only")
            }
        } else {
            config.clone()
        };
        let reconnected_keys = if crash_first_receive {
            object_store::load_keys(&reconnected_config).unwrap()
        } else {
            object_store::Keys {
                access_key_id: keys.access_key_id.clone(),
                secret_access_key: keys.secret_access_key.clone(),
            }
        };
        if crash_first_receive {
            object_store_restore::restore_selected(
                &reconnected_config,
                &reconnected_keys,
                &identity,
                &paths,
                None,
            )
            .await
            .unwrap();
            for (digest, expected, _) in &media_fixtures {
                let relative = format!("sha256/{}/{digest}", &digest[..2]);
                assert_eq!(
                    fs::read(paths.objects_path.join(relative)).unwrap(),
                    *expected
                );
            }
        }
        let (downloaded, _) =
            download_latest_snapshot(&reconnected_config, &reconnected_keys, &identity, &paths)
                .await
                .unwrap();
        if crash_first_receive {
            let bytes = fs::read(downloaded.path()).unwrap();
            assert!(
                !bytes
                    .windows(connection_secret.len())
                    .any(|part| part == connection_secret.as_bytes()),
                "connection credentials must not be present in the restored backup"
            );
        }
        assert_eq!(
            snapshot_workspace_id(downloaded.path()).unwrap(),
            profile.workspace_id
        );
        let new_pc_keys = std::cell::RefCell::new(HashMap::<String, String>::new());
        preserve_recovery_identity_with(
            downloaded.path(),
            &identity,
            |id| Ok(new_pc_keys.borrow().get(id).map(|key| key.parse().unwrap())),
            |id, key| {
                new_pc_keys
                    .borrow_mut()
                    .insert(id.to_owned(), key.to_owned());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            new_pc_keys.borrow().get(&profile.workspace_id).unwrap(),
            identity.to_string().expose_secret()
        );
        let failure = storage::restore_snapshot_file_with_hook(
            downloaded.path(),
            &paths.database_path,
            &paths.backups_path,
            || {
                Err(storage::StorageError::Integrity(
                    "injected after replacement".into(),
                ))
            },
        );
        assert!(matches!(failure, Err(storage::StorageError::Integrity(_))));
        let after_failure = storage::open_writable_connection(&paths.database_path).unwrap();
        let value: String = after_failure
            .query_row(
                "SELECT value FROM app_runtime_meta WHERE key='test_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "after");
        let indexed: i64 = after_failure
            .query_row(
                "SELECT count(*) FROM archive_zets WHERE id=?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(indexed, 1);
        drop(after_failure);
        let safety = apply_snapshot(downloaded.path(), &paths).unwrap();
        if crash_first_receive {
            let folders_by_id = |list: &serde_json::Value| {
                list.as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| {
                        (
                            entry["id"].as_str().unwrap().to_owned(),
                            entry["folder"].as_str().unwrap().to_owned(),
                        )
                    })
                    .collect::<std::collections::BTreeMap<_, _>>()
            };
            assert_eq!(
                folders_by_id(&document_listing(&archive_root, &paths.database_path)),
                folders_by_id(old_documents.as_ref().unwrap()),
                "document folders survive app-state restore"
            );
            assert_eq!(
                crate::archive_zet::folders(&archive_root).unwrap(),
                *old_folders.as_ref().unwrap()
            );
        }
        let restored = storage::open_writable_connection(&paths.database_path).unwrap();
        let restored_path: String = restored
            .query_row(
                "SELECT path FROM archive_location_settings WHERE id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(std::path::Path::new(&restored_path), archive_root);
        let indexed: i64 = restored
            .query_row(
                "SELECT count(*) FROM archive_zets WHERE id=?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(indexed, 1);
        let value: String = restored
            .query_row(
                "SELECT value FROM app_runtime_meta WHERE key='test_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "before");
        let visibility: (i64, String) = restored
            .query_row(
                "SELECT pinned_order, archived_at FROM chats WHERE id=?1",
                [&chat_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(visibility, (7, "archived".into()));
        let message: (String, i64, i64) = restored
            .query_row(
                "SELECT text,fast,retryable_fast FROM chat_messages WHERE id=?1",
                [&sent.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(message, ("hello".into(), 1, 1));
        drop(restored);
        object_store_restore::restore_selected(
            &reconnected_config,
            &reconnected_keys,
            &identity,
            &paths,
            None,
        )
        .await
        .unwrap();
        if crash_first_receive {
            for (digest, expected, _) in &media_fixtures {
                let relative = format!("sha256/{}/{digest}", &digest[..2]);
                let restored = paths.objects_path.join(relative);
                assert_eq!(fs::read(&restored).unwrap(), *expected);
                assert_eq!(transfer::hash_file(&restored).unwrap().0, *digest);
            }
            let items = serde_json::to_value(
                crate::chat_items::list(&archive_root, &paths.database_path, &chat_id).unwrap(),
            )
            .unwrap();
            let media = items["media"].as_array().unwrap();
            for (digest, _, _) in &media_fixtures {
                assert!(media
                    .iter()
                    .any(|entry| entry["id"] == format!("sha256:{digest}")));
            }
            for (index, receipt) in generation_receipts.iter().enumerate() {
                let value = serde_json::to_value(
                    crate::archive_receipt::read(&archive_root, &paths.database_path, receipt)
                        .unwrap(),
                )
                .unwrap();
                assert!(value["body"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("fixture generation prompt {index}")));
                assert_eq!(
                    value["head"]["wrote"][0]["id"],
                    format!("sha256:{}", media_fixtures[index].0)
                );
                if index == 1 || index == 3 {
                    assert_eq!(
                        value["head"]["read"][0]["id"],
                        format!("sha256:{}", media_fixtures[index - 1].0)
                    );
                }
            }
            let stored = fs::read(new_pc.join("object-store.json")).unwrap();
            assert!(
                !stored
                    .windows(b"test-token".len())
                    .any(|part| part == b"test-token"),
                "restoring application state must not restore credentials"
            );
            assert!(
                crate::credentials::get_secret("ai:openai:api-key")
                    .unwrap()
                    .is_none(),
                "the new PC must reconnect the provider"
            );
        }
        assert_eq!(
            fs::read(paths.objects_path.join(&object_relative)).unwrap(),
            b"original bytes"
        );
        #[cfg(windows)]
        {
            let restored = paths.objects_path.join(&object_relative);
            crate::source_assets::clear_readonly_attribute(&restored).unwrap();
        }
        assert_eq!(
            crate::chat::set_archived(&paths.database_path, &chat_id, false).state,
            "ok",
            "stage 7: restore room"
        );
        assert_eq!(
            crate::chat::set_pinned(&paths.database_path, &chat_id, true).state,
            "ok",
            "stage 7: pin room"
        );
        assert_eq!(
            crate::chat::set_archived(&paths.database_path, &chat_id, true).state,
            "ok",
            "stage 7: archive room"
        );
        assert_eq!(
            crate::chat::set_archived(&paths.database_path, &chat_id, false).state,
            "ok",
            "stage 7: unarchive room"
        );
        let deletion = crate::chat::delete_with(
            &paths.database_path,
            &Default::default(),
            &chat_id,
            |_| async { Ok(()) },
        )
        .await;
        assert_eq!(
            serde_json::to_value(deletion).unwrap()["state"],
            "deleted",
            "stage 7: delete room"
        );
        assert!(
            archive_root
                .join("zettels")
                .join(format!("{id}.md"))
                .is_file(),
            "stage 7: document remains"
        );
        assert_eq!(
            fs::read(paths.objects_path.join(&object_relative)).unwrap(),
            b"original bytes",
            "stage 7: original remains"
        );
        let chatgpt_export = new_pc.join("chatgpt.json");
        fs::write(&chatgpt_export, serde_json::json!([{"id":"fixture-chatgpt","current_node":"a","mapping":{"a":{"parent":null,"message":{"author":{"role":"user"},"content":{"parts":["hello"]}}}}}]).to_string()).unwrap();
        let claude_export = new_pc.join("claude.json");
        fs::write(&claude_export, serde_json::json!([{"uuid":"fixture-claude","name":"lesson","chat_messages":[{"sender":"human","text":"hello"}]}]).to_string()).unwrap();
        for (provider, file) in [("chatgpt", &chatgpt_export), ("claude", &claude_export)] {
            let result = serde_json::to_value(
                crate::chat_export_import::import(&paths.database_path, file, provider)
                    .expect("stage 8: import"),
            )
            .unwrap();
            assert_eq!(result["imported"], 1, "stage 8: {provider}");
        }
        assert_eq!(
            crate::chat::list(&paths.database_path).unwrap().len(),
            3,
            "stage 8: imported rooms"
        );
        let safety_db = Connection::open(safety).unwrap();
        let value: String = safety_db
            .query_row(
                "SELECT value FROM app_runtime_meta WHERE key='test_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "after");
        let safety_count = fs::read_dir(&paths.backups_path)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("pre-restore-")
                    && entry.path().extension().is_some_and(|ext| ext == "db")
            })
            .count();
        assert_eq!(safety_count, 2);
        assert!(objects
            .lock()
            .unwrap()
            .values()
            .all(|bytes| !bytes.windows(b"before".len()).any(|part| part == b"before")));
        stopped.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }
}
