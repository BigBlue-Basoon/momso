//! Resumable revalidation of chat copies after credential inputs change.
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::{path::Path, time::Duration};
use tauri::Emitter;

#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Progress {
    state: &'static str,
    completed: u64,
    total: u64,
    pub(crate) available: bool,
}

fn meta(db: &Connection, key: &str) -> Result<Option<String>, &'static str> {
    db.query_row(
        "SELECT value FROM app_runtime_meta WHERE key=?1",
        [key],
        |r| r.get(0),
    )
    .optional()
    .map_err(|_| "storage_error")
}

fn put(db: &Connection, key: &str, value: &str) -> Result<(), &'static str> {
    db.execute("INSERT INTO app_runtime_meta(key,value,updated_at) VALUES(?1,?2,CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=CURRENT_TIMESTAMP", params![key,value]).map_err(|_| "storage_error")?;
    Ok(())
}

/// One item per transaction; cursor and proof contain no secret values.
pub(crate) fn step_with_known(
    root: &Path,
    paths: &crate::runtime::RuntimePaths,
    known: &[String],
    proof_key: &str,
) -> Result<Progress, &'static str> {
    let proof = crate::chat_archive_tracking::proof(root, known, proof_key)?;
    let mut db = crate::storage::open_writable_connection(&paths.database_path)
        .map_err(|_| "storage_error")?;
    let needs_retry = crate::chat_archive_tracking::requires_full_retry(&paths.database_path)?;
    if crate::chat_archive_tracking::stored_proof(&paths.database_path)?.as_deref()
        == Some(proof.as_str())
        && !needs_retry
    {
        let completed = meta(&db, "chat_redaction_recheck_completed")?
            .and_then(|x| x.parse().ok())
            .unwrap_or(0);
        let total = meta(&db, "chat_redaction_recheck_total")?
            .and_then(|x| x.parse().ok())
            .unwrap_or(0);
        return Ok(Progress {
            state: "complete",
            completed,
            total,
            available: true,
        });
    }
    if meta(&db, "chat_redaction_recheck_proof")?.as_deref() != Some(proof.as_str())
        || (needs_retry
            && meta(&db, "chat_redaction_recheck_state")?.as_deref() == Some("complete"))
    {
        let total: i64 = db.query_row("SELECT (SELECT count(*) FROM chats)+(SELECT count(*) FROM chat_deletions WHERE NOT EXISTS(SELECT 1 FROM chats WHERE id=chat_id))", [], |r| r.get(0)).map_err(|_| "storage_error")?;
        let tx = db.transaction().map_err(|_| "storage_error")?;
        put(&tx, "chat_redaction_recheck_proof", &proof)?;
        put(&tx, "chat_redaction_recheck_cursor", "")?;
        put(&tx, "chat_redaction_recheck_completed", "0")?;
        put(&tx, "chat_redaction_recheck_total", &total.to_string())?;
        put(&tx, "chat_redaction_recheck_state", "checking")?;
        tx.commit().map_err(|_| "storage_error")?;
    }
    let cursor = meta(&db, "chat_redaction_recheck_cursor")?.unwrap_or_default();
    let completed: u64 = meta(&db, "chat_redaction_recheck_completed")?
        .and_then(|x| x.parse().ok())
        .ok_or("storage_error")?;
    let total: u64 = meta(&db, "chat_redaction_recheck_total")?
        .and_then(|x| x.parse().ok())
        .ok_or("storage_error")?;
    let next: Option<String> = db.query_row("SELECT id FROM (SELECT id FROM chats UNION SELECT chat_id AS id FROM chat_deletions) WHERE id>?1 ORDER BY id LIMIT 1", [&cursor], |r| r.get(0)).optional().map_err(|_| "storage_error")?;
    if let Some(id) = next {
        if !crate::archive_receipt::valid_chat_id(&id) {
            return Err("invalid_chat_id");
        }
        let refs: Vec<&str> = known.iter().map(String::as_str).collect();
        if let Some(deleted_at) = crate::chat_archive_store::deleted_at(&paths.database_path, &id)?
        {
            crate::chat_archive_store::write_deletion(
                root,
                &paths.database_path,
                &id,
                &deleted_at,
            )?;
        } else {
            crate::chat_archive_store::write_known(root, &paths.database_path, &id, &refs)?;
        }
        let tx = db.transaction().map_err(|_| "storage_error")?;
        tx.execute("INSERT INTO chat_archive_changes(chat_id,revision) VALUES(?1,1) ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1", [&id]).map_err(|_| "storage_error")?;
        tx.execute(
            "DELETE FROM app_runtime_meta WHERE key IN (?1,?2)",
            params![
                format!("chat_archive_write:{id}"),
                format!("chat_archive_delete:{id}")
            ],
        )
        .map_err(|_| "storage_error")?;
        put(&tx, "chat_redaction_recheck_cursor", &id)?;
        put(
            &tx,
            "chat_redaction_recheck_completed",
            &(completed + 1).to_string(),
        )?;
        tx.commit().map_err(|_| "storage_error")?;
        return Ok(Progress {
            state: "checking",
            completed: completed + 1,
            total: total.max(completed + 1),
            available: false,
        });
    }
    crate::chat_archive_store::validate_with_known(root, &paths.database_path, true, known)?;
    let tx = db.transaction().map_err(|_| "storage_error")?;
    tx.execute(
        "DELETE FROM app_runtime_meta WHERE key='chat_archive_write:all'",
        [],
    )
    .map_err(|_| "storage_error")?;
    put(&tx, "chat_archive_redaction_proof_v1", &proof)?;
    put(&tx, "chat_redaction_recheck_state", "complete")?;
    tx.commit().map_err(|_| "storage_error")?;
    Ok(Progress {
        state: "complete",
        completed,
        total: total.max(completed),
        available: true,
    })
}

pub(crate) fn step(
    root: &Path,
    paths: &crate::runtime::RuntimePaths,
) -> Result<Progress, &'static str> {
    crate::chat_archive_tracking::ensure_tracking(&paths.database_path)?;
    let key = crate::chat_archive_tracking::proof_key()?;
    let _guard =
        crate::credentials::prevent_secret_changes().map_err(|_| "credentials_unavailable")?;
    crate::redaction_wait::expire_unlocked(chrono::Utc::now().timestamp())
        .map_err(|_| "credentials_unavailable")?;
    let known = crate::chat_archive_store::known_values(&paths.data_root, &paths.database_path)?;
    step_with_known(root, paths, &known, &key)
}

pub(crate) fn start(app: tauri::AppHandle, paths: crate::runtime::RuntimePaths) {
    std::thread::spawn(move || {
        let mut last: Option<Progress> = None;
        loop {
            let root = match paths.archive.current.try_read() {
                Ok(root) => root,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
            };
            let result = step(&root, &paths);
            drop(root);
            match result {
                Ok(progress) => {
                    let running = !progress.available;
                    if last.as_ref() != Some(&progress) {
                        let counts = crate::redaction_wait::counts().ok();
                        let pending = counts.map(|value| value.0);
                        let expired = counts.map(|value| value.1);
                        let _ = app.emit("chat-archive-redaction-progress", serde_json::json!({"state":progress.state,"completed":progress.completed,"total":progress.total,"available":progress.available,"retiredKeysPending":pending,"retiredKeysExpired":expired}));
                        last = Some(progress);
                    }
                    std::thread::sleep(if running {
                        Duration::from_millis(20)
                    } else {
                        Duration::from_secs(2)
                    });
                }
                Err(_) => {
                    let progress = Progress {
                        state: "failed",
                        completed: 0,
                        total: 0,
                        available: false,
                    };
                    if last.as_ref() != Some(&progress) {
                        let _ = app.emit("chat-archive-redaction-progress", &progress);
                        last = Some(progress);
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_proof_bootstraps_new_and_existing_chats_without_keys() {
        for count in [0, 2] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let archive = root.join("momso");
            let db = root.join("momso.db");
            crate::storage::initialize(&db, &root.join("backups"), "fixture").unwrap();
            let location =
                crate::archive_location::ArchiveLocation::load(db.clone(), root, vec![]).unwrap();
            crate::archive::initialize(&archive, root).unwrap();
            let paths = crate::runtime::RuntimePaths {
                data_root: root.to_owned(),
                database_path: db.clone(),
                objects_path: archive.join("objects"),
                exports_path: root.join("exports"),
                backups_path: root.join("backups"),
                logs_path: root.join("logs"),
                archive: location,
                archive_lease: None,
            };
            for _ in 0..count {
                crate::chat::create(&db).unwrap();
            }
            assert!(crate::chat_archive_tracking::stored_proof(&db)
                .unwrap()
                .is_none());
            for _ in 0..=count {
                if step_with_known(&archive, &paths, &[], "fixture-key")
                    .unwrap()
                    .available
                {
                    break;
                }
            }
            assert!(crate::chat_archive_tracking::stored_proof(&db)
                .unwrap()
                .is_some());
            assert!(
                step_with_known(&archive, &paths, &[], "fixture-key")
                    .unwrap()
                    .available
            );
        }
    }
    #[test]
    fn recheck_resumes_after_restart_and_masks_only_copies() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let archive = root.join("momso");
        let db = root.join("momso.db");
        crate::storage::initialize(&db, &root.join("backups"), "fixture").unwrap();
        let location =
            crate::archive_location::ArchiveLocation::load(db.clone(), root, vec![]).unwrap();
        crate::archive::initialize(&archive, root).unwrap();
        let paths = crate::runtime::RuntimePaths {
            data_root: root.to_path_buf(),
            database_path: db.clone(),
            objects_path: archive.join("objects"),
            exports_path: root.join("exports"),
            backups_path: root.join("backups"),
            logs_path: root.join("logs"),
            archive: location,
            archive_lease: None,
        };
        let secret = "fixture-opaque-personal-value";
        let mut ids = Vec::new();
        for _ in 0..2 {
            let room = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
            let id = room["id"].as_str().unwrap().to_owned();
            crate::chat::append(&db, &id, crate::chat::Role::User, secret, None, None).unwrap();
            crate::chat_archive_store::write(&archive, &paths, &id).unwrap();
            ids.push(id);
        }
        let keys = vec![secret.to_owned()];
        let first = step_with_known(&archive, &paths, &keys, "fixture-proof-key").unwrap();
        assert_eq!(first.state, "checking");
        assert_eq!(first.completed, 1);
        crate::chat::append(
            &db,
            &ids[0],
            crate::chat::Role::User,
            "continue while checking",
            None,
            None,
        )
        .unwrap();
        crate::chat_archive_store::write_known(&archive, &db, &ids[0], &[secret]).unwrap();
        // Drop and reopen all connections as on restart; cursor is durable.
        let second = step_with_known(&archive, &paths, &keys, "fixture-proof-key").unwrap();
        assert_eq!(second.completed, 2);
        let done = step_with_known(&archive, &paths, &keys, "fixture-proof-key").unwrap();
        assert_eq!(done.state, "complete");
        assert!(done.available);
        for id in ids {
            let copy =
                std::fs::read_to_string(archive.join("chats").join(format!("{id}.json"))).unwrap();
            assert!(!copy.contains(secret));
            assert!(serde_json::to_string(&crate::chat::read(&db, &id).unwrap())
                .unwrap()
                .contains(secret));
        }
        let conn = Connection::open(&db).unwrap();
        let leaked: i64 = conn
            .query_row(
                "SELECT count(*) FROM app_runtime_meta WHERE value LIKE ?1",
                [format!("%{secret}%")],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(leaked, 0);
    }

    #[test]
    fn pending_check_never_waits_for_close_and_document_save_excludes_chats() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let archive = root.join("momso");
        let db = root.join("momso.db");
        crate::storage::initialize(&db, &root.join("backups"), "fixture").unwrap();
        let location =
            crate::archive_location::ArchiveLocation::load(db.clone(), root, vec![]).unwrap();
        crate::archive::initialize(&archive, root).unwrap();
        let paths = crate::runtime::RuntimePaths {
            data_root: root.to_path_buf(),
            database_path: db.clone(),
            objects_path: archive.join("objects"),
            exports_path: root.join("exports"),
            backups_path: root.join("backups"),
            logs_path: root.join("logs"),
            archive: location,
            archive_lease: None,
        };
        let room = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
        let id = room["id"].as_str().unwrap();
        crate::chat::append(&db, id, crate::chat::Role::User, "fixture", None, None).unwrap();
        crate::chat_archive_store::write(&archive, &paths, id).unwrap();
        crate::archive_zet::create(
            &archive,
            &db,
            crate::archive_zet::ZetCreate {
                title: "lesson".into(),
                summary: "fixture".into(),
                kind: "note".into(),
                body: "body".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        let _guard = crate::credentials::prevent_secret_changes().unwrap();
        let start = std::time::Instant::now();
        assert_eq!(
            crate::chat_archive_store::save_pending(&archive, root, &paths),
            Err("redaction_pending")
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        crate::archive::save(&archive, root, &db).unwrap();
        let (git, _) = crate::git_tool::resolve().unwrap();
        let listed =
            crate::archive_remote::git(&git, &archive, &["ls-tree", "-r", "--name-only", "HEAD"])
                .unwrap();
        assert!(listed.contains("zettels/"));
        assert!(!listed.contains("chats/"));
    }

    #[test]
    fn repeated_key_changes_preserve_older_masks_across_recheck() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let archive = root.join("momso");
        let db = root.join("momso.db");
        crate::storage::initialize(&db, &root.join("backups"), "fixture").unwrap();
        let location =
            crate::archive_location::ArchiveLocation::load(db.clone(), root, vec![]).unwrap();
        crate::archive::initialize(&archive, root).unwrap();
        let paths = crate::runtime::RuntimePaths {
            data_root: root.to_path_buf(),
            database_path: db.clone(),
            objects_path: archive.join("objects"),
            exports_path: root.join("exports"),
            backups_path: root.join("backups"),
            logs_path: root.join("logs"),
            archive: location,
            archive_lease: None,
        };
        let old = "fixture-opaque-previous-value";
        let mut ids = Vec::new();
        for _ in 0..2 {
            let room = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
            let id = room["id"].as_str().unwrap().to_owned();
            ids.push(id);
        }
        ids.sort();
        for (index, id) in ids.iter().enumerate() {
            crate::chat::append(
                &db,
                id,
                crate::chat::Role::User,
                if index == 1 { old } else { "normal lesson" },
                None,
                None,
            )
            .unwrap();
            crate::chat_archive_store::write(&archive, &paths, id).unwrap();
        }
        assert!(
            !step_with_known(&archive, &paths, &[old.to_owned()], "fixture-proof")
                .unwrap()
                .available
        );
        while !step_with_known(
            &archive,
            &paths,
            &[old.to_owned(), "fixture-new-value".to_owned()],
            "fixture-proof",
        )
        .unwrap()
        .available
        {}
        assert!(ids.iter().all(|id| !std::fs::read_to_string(
            archive.join("chats").join(format!("{id}.json"))
        )
        .unwrap()
        .contains(old)));
        // Once the retired key is removed, regenerating a copy must keep the mask.
        crate::chat_archive_store::write_known(&archive, &db, &ids[1], &[]).unwrap();
        assert!(
            !std::fs::read_to_string(archive.join("chats").join(format!("{}.json", ids[1])))
                .unwrap()
                .contains(old)
        );
        let conn = rusqlite::Connection::open(&db).unwrap();
        let stored: (String,String) = conn.query_row("SELECT source_hash,safe_text FROM chat_redaction_preserved WHERE chat_id=?1 AND field_kind='message'", [&ids[1]], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert!(!stored.0.contains(old) && !stored.1.contains(old));
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        drop(conn);
        let copied_db = root.join("restored-on-another-device.db");
        std::fs::copy(&db, &copied_db).unwrap();
        let copied_archive = root.join("restored-archive");
        std::fs::create_dir(&copied_archive).unwrap();
        crate::chat_archive_store::write_known(&copied_archive, &copied_db, &ids[1], &[]).unwrap();
        assert!(!std::fs::read_to_string(
            copied_archive
                .join("chats")
                .join(format!("{}.json", ids[1]))
        )
        .unwrap()
        .contains(old));
    }
}
