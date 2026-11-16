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
    let workspace_id = snapshot_workspace_id(snapshot)?;
    match credentials::get_backup_identity(&workspace_id).map_err(|_| "unavailable")? {
        Some(existing) if existing.to_public().to_string() != identity.to_public().to_string() => {
            Err("recovery_key_conflict")
        }
        Some(_) => Ok(()),
        None => {
            credentials::import_backup_identity(&workspace_id, identity.to_string().expose_secret())
                .map_err(|_| "unavailable")
        }
    }
}

fn apply_snapshot(snapshot: &Path, paths: &RuntimePaths) -> Result<PathBuf, &'static str> {
    storage::validate_snapshot_file(snapshot).map_err(|_| "invalid_snapshot")?;
    storage::restore_snapshot_file(snapshot, &paths.database_path, &paths.backups_path)
        .map_err(|_| "restore_failed")
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

#[allow(dead_code)]
pub(crate) async fn app_state_restore(
    state: State<'_, AppState>,
) -> Result<AppStateRestoreResult, &'static str> {
    let _guard = object_store::CONNECTION_LOCK.lock().await;
    let config = object_store::load_config(&state.paths.data_root)?.ok_or("not_connected")?;
    let keys = object_store::load_keys(&config)?;
    let identity = transfer::identity(&state.paths.database_path)?;
    let manifest = transfer::catalog(&config, &keys, &identity)
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
        &config,
        &keys,
        &identity,
        &manifest,
        &state.paths.backups_path,
    )
    .await?;
    storage::validate_snapshot_file(snapshot.path()).map_err(|_| "invalid_snapshot")?;
    preserve_recovery_identity(snapshot.path(), &identity)?;
    apply_snapshot(snapshot.path(), &state.paths)?;
    Ok(AppStateRestoreResult {
        state: "restored",
        snapshot_at: manifest.created_at,
        safety_backup_created: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        fs,
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
    };

    #[tokio::test]
    async fn sqlite_snapshot_round_trip_preserves_current_state_before_restore() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let database = root.join("momso.db");
        let backups = root.join("backups");
        fs::create_dir_all(&backups).unwrap();
        storage::initialize(&database, &backups, "test").unwrap();
        let archive =
            crate::archive_location::ArchiveLocation::load(database.clone(), root, vec![]).unwrap();
        let paths = RuntimePaths {
            data_root: root.to_path_buf(),
            database_path: database,
            objects_path: root.join("objects"),
            exports_path: root.join("exports"),
            backups_path: backups,
            logs_path: root.join("logs"),
            archive,
            archive_lease: None,
        };
        fs::create_dir_all(&paths.backups_path).unwrap();
        storage::initialize(&paths.database_path, &paths.backups_path, "test").unwrap();
        let profile = crate::backup_profile::ensure(&paths.database_path).unwrap();
        let db = storage::open_writable_connection(&paths.database_path).unwrap();
        db.execute("INSERT INTO app_runtime_meta(key,value,updated_at) VALUES('test_state','before','now')", []).unwrap();
        drop(db);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let objects = Arc::new(Mutex::new(HashMap::<String, Vec<u8>>::new()));
        let remote = Arc::clone(&objects);
        let server = std::thread::spawn(move || {
            for _ in 0..5 {
                let (mut stream, _) = listener.accept().unwrap();
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
                let key = line.next().unwrap().trim_start_matches("/bucket/");
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
                let (status, body) = if method == "PUT" {
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
            provider: "s3_compatible".into(),
            endpoint: format!("http://{address}"),
            region: "auto".into(),
            bucket: "bucket".into(),
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(object_store::WriteSafety::Conditional),
        };
        let keys = object_store::Keys {
            access_key_id: "TEST".into(),
            secret_access_key: "test".into(),
        };
        let identity = x25519::Identity::generate();
        let manifest = backup_snapshot(&config, &keys, &identity, &paths)
            .await
            .unwrap();
        let db = storage::open_writable_connection(&paths.database_path).unwrap();
        db.execute(
            "UPDATE app_runtime_meta SET value='after' WHERE key='test_state'",
            [],
        )
        .unwrap();
        drop(db);
        let downloaded = object_store_restore::download_verified(
            &config,
            &keys,
            &identity,
            &manifest,
            &paths.backups_path,
        )
        .await
        .unwrap();
        assert_eq!(
            snapshot_workspace_id(downloaded.path()).unwrap(),
            profile.workspace_id
        );
        let safety = apply_snapshot(downloaded.path(), &paths).unwrap();
        let restored = storage::open_writable_connection(&paths.database_path).unwrap();
        let value: String = restored
            .query_row(
                "SELECT value FROM app_runtime_meta WHERE key='test_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "before");
        let safety_db = Connection::open(safety).unwrap();
        let value: String = safety_db
            .query_row(
                "SELECT value FROM app_runtime_meta WHERE key='test_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "after");
        assert!(objects
            .lock()
            .unwrap()
            .values()
            .all(|bytes| !bytes.windows(b"before".len()).any(|part| part == b"before")));
        server.join().unwrap();
    }
}
