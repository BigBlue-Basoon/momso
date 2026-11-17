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
}

async fn backup_snapshot(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    paths: &RuntimePaths,
) -> Result<transfer::Manifest, &'static str> {
    let temp = tempfile::Builder::new()
        .prefix("momso-state-")
        .tempdir_in(&paths.backups_path)
        .map_err(|_| "local_write_failed")?;
    let snapshot = temp.path().join("momso.db");
    storage::create_snapshot_file(&paths.database_path, &snapshot)
        .map_err(|_| "snapshot_failed")?;
    storage::sanitize_user_owned_backup_snapshot(&snapshot).map_err(|_| "snapshot_failed")?;
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
            relative: "momso.db",
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

#[cfg(test)]
fn apply_snapshot(snapshot: &Path, paths: &RuntimePaths) -> Result<PathBuf, &'static str> {
    apply_snapshot_with_progress(snapshot, paths, None)
}

fn apply_snapshot_with_progress(
    snapshot: &Path,
    paths: &RuntimePaths,
    app: Option<&tauri::AppHandle>,
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
    let location = state.paths.archive.current.clone().write_owned().await;
    let mut paths = state.paths.clone();
    paths.objects_path = location.join("objects");
    if crate::archive_remote::first_receive_pending(&location, &paths.database_path) {
        return Err("first_receive_in_progress");
    }
    let _guard = object_store::CONNECTION_LOCK.lock().await;
    let config = object_store::load_config(&paths.data_root)?.ok_or("not_connected")?;
    let keys = object_store::load_keys(&config)?;
    let identity = transfer::identity(&paths.database_path)?;
    let (snapshot, manifest) = download_latest_snapshot(&config, &keys, &identity, &paths).await?;
    preserve_recovery_identity(snapshot.path(), &identity)?;
    tauri::async_runtime::spawn_blocking(move || {
        apply_snapshot_with_progress(snapshot.path(), &paths, Some(&app))
    })
    .await
    .map_err(|_| "restore_failed")??;
    Ok(AppStateRestoreResult {
        state: "restored",
        snapshot_at: manifest.created_at,
        safety_backup_created: true,
    })
}

async fn download_latest_snapshot(
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
        exercise_new_pc_round_trip(false).await;
    }

    #[tokio::test]
    async fn oauth_r2_new_pc_recovers_documents_sources_and_chat_state() {
        exercise_new_pc_round_trip(true).await;
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

    async fn exercise_new_pc_round_trip(oauth: bool) {
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
        let mut command = tokio::process::Command::new("powershell.exe");
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
        crate::chat_archive_store::save_pending(&archive_root, root, &paths).unwrap();
        crate::archive_remote::transfer(
            &tool,
            &archive_root,
            bare.to_str().unwrap(),
            "main",
            "push",
        )
        .unwrap();
        let profile = crate::backup_profile::ensure(&paths.database_path).unwrap();
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
        let remote = Arc::clone(&objects);
        let stopped = Arc::new(AtomicBool::new(false));
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
                let mut line = head.lines().next().unwrap().split_whitespace();
                let method = line.next().unwrap();
                let path = line.next().unwrap();
                let oauth_path = format!(
                    "/client/v4/accounts/{}/r2/buckets/momso-fixture/objects",
                    "a".repeat(32)
                );
                if oauth {
                    assert!(
                        path == oauth_path
                            || path.starts_with(&format!("{oauth_path}/"))
                            || path.starts_with(&format!("{oauth_path}?"))
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
                let (status, body) = if method == "GET"
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
                    assert!(!data.contains_key(key));
                    data.insert(key.to_owned(), received[end..end + length].to_vec());
                    ("200 OK", Vec::new())
                } else {
                    match remote.lock().unwrap().get(key) {
                        Some(data) => ("200 OK", data.clone()),
                        None => ("404 Not Found", Vec::new()),
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
            provider: if oauth { "cloudflare" } else { "s3_compatible" }.into(),
            endpoint: format!("http://{address}"),
            region: if oauth { "wrangleroauth" } else { "auto" }.into(),
            bucket: if oauth {
                "momso-fixture".into()
            } else {
                selected_bucket
            },
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(if oauth {
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
        let _manifest = backup_snapshot(&config, &keys, &identity, &paths)
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
        let (storage_snapshot, _) =
            download_latest_snapshot(&config, &keys, &identity, &storage_paths)
                .await
                .unwrap();
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
        crate::archive_remote::receive_connected(
            &tool,
            &archive_root,
            &database_path,
            bare.to_str().unwrap(),
            "main",
        )
        .unwrap();
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
        crate::chat::append(
            &database_path,
            &chat_id,
            crate::chat::Role::User,
            "continued on the new device",
            None,
            None,
        )
        .unwrap();
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
        let mut reply_command = tokio::process::Command::new("powershell.exe");
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
        crate::chat_archive_store::save_pending(&archive_root, &new_pc, &new_paths).unwrap();
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
        crate::chat_archive_store::save_pending(&source_archive_root, root, &paths).unwrap();
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
        let (downloaded, _) = download_latest_snapshot(&config, &keys, &identity, &paths)
            .await
            .unwrap();
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
        object_store_restore::restore_selected(&config, &keys, &identity, &paths, None)
            .await
            .unwrap();
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
