//! Local, redacted chat copies for the archive. Original messages stay in SQLite.
use std::{fs, io::Write, path::Path, sync::Mutex};

static WRITE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn lock_store() -> Result<std::sync::MutexGuard<'static, ()>, &'static str> {
    WRITE_LOCK.lock().map_err(|_| "unavailable")
}

fn io_failure(stage: &'static str, error: &std::io::Error) -> &'static str {
    let reason = if error.kind() == std::io::ErrorKind::PermissionDenied {
        "permission"
    } else {
        let code = error.raw_os_error();
        #[cfg(windows)]
        let (path_too_long, disk_full) = (code == Some(206), matches!(code, Some(112 | 39)));
        #[cfg(not(windows))]
        let (path_too_long, disk_full) = (code == Some(36), code == Some(28));
        if path_too_long {
            "path_length"
        } else if disk_full {
            "disk"
        } else {
            "other"
        }
    };
    match (stage, reason) {
        ("directory", "permission") => "directory_permission_denied",
        ("directory", "path_length") => "directory_path_too_long",
        ("directory", "disk") => "directory_disk_error",
        ("file", "permission") => "file_permission_denied",
        ("file", "path_length") => "file_path_too_long",
        ("file", "disk") => "file_disk_error",
        ("directory", _) => "directory_other",
        _ => "file_other",
    }
}

pub(crate) fn failure_details(error: &str) -> (&'static str, &'static str) {
    match error {
        "archive_missing" => ("archiveMissing", "directory"),
        "git_not_repository" => ("notGitRepository", "git"),
        "directory_permission_denied" => ("permissionDenied", "directory"),
        "file_permission_denied" => ("permissionDenied", "file"),
        "directory_path_too_long" => ("pathTooLong", "directory"),
        "file_path_too_long" => ("pathTooLong", "file"),
        "directory_disk_error" => ("diskError", "directory"),
        "file_disk_error" => ("diskError", "file"),
        "redaction_failed" | "sensitive_metadata" => ("redactionFailed", "redaction"),
        "credentials_unavailable" => ("credentialStoreUnavailable", "credentials"),
        "chat_deletion_conflict" => ("deletionConflict", "conflict"),
        "directory_other" => ("other", "directory"),
        "file_other" => ("other", "file"),
        _ => ("other", "other"),
    }
}

pub(crate) fn safe_diagnostic(error: &str) -> String {
    let (reason, stage) = failure_details(error);
    format!("momso-safe-event event=chat-archive-failure stage={stage} reason={reason} relativePath=chats relativePathLength=5")
}

pub(crate) fn record_failure(error: &str) {
    if std::env::var_os("MOMSO_SAFE_DIAGNOSTICS").as_deref() == Some(std::ffi::OsStr::new("1")) {
        log::info!("{}", safe_diagnostic(error));
    }
}

fn set_pending(db: &Path, key: &str, pending: bool) -> Result<(), &'static str> {
    let connection = crate::storage::open_writable_connection(db).map_err(|_| "storage_error")?;
    if pending {
        connection.execute("INSERT INTO app_runtime_meta(key,value,updated_at) VALUES(?1,'pending',CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value='pending',updated_at=CURRENT_TIMESTAMP", [key]).map_err(|_| "storage_error")?;
    } else {
        connection
            .execute("DELETE FROM app_runtime_meta WHERE key=?1", [key])
            .map_err(|_| "storage_error")?;
    }
    Ok(())
}

fn pending_key(id: &str) -> String {
    format!("chat_archive_write:{id}")
}

fn snapshot_hash_key(id: &str) -> String {
    format!("chat_archive_snapshot_hash:{id}")
}

fn remember_snapshot(db: &Path, id: &str, bytes: &[u8]) -> Result<(), &'static str> {
    let connection = crate::storage::open_writable_connection(db).map_err(|_| "storage_error")?;
    connection.execute(
        "INSERT INTO app_runtime_meta(key,value,updated_at) VALUES(?1,?2,CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=CURRENT_TIMESTAMP",
        rusqlite::params![snapshot_hash_key(id), crate::object_store::digest(bytes)],
    ).map_err(|_| "storage_error")?;
    Ok(())
}

fn snapshot_hash(db: &Path, id: &str) -> Result<Option<String>, &'static str> {
    use rusqlite::OptionalExtension;
    let connection =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "storage_error")?;
    connection
        .query_row(
            "SELECT value FROM app_runtime_meta WHERE key=?1",
            [snapshot_hash_key(id)],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| "storage_error")
}

fn retry_deletions(root: &Path, db: &Path) -> Result<(), &'static str> {
    let connection =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "storage_error")?;
    let mut statement = connection.prepare("SELECT substr(key,21) FROM app_runtime_meta WHERE key GLOB 'chat_archive_delete:*' ORDER BY key").map_err(|_| "storage_error")?;
    let ids = statement
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|_| "storage_error")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "storage_error")?;
    drop(statement);
    for id in ids {
        if !crate::archive_receipt::valid_chat_id(&id) {
            return Err("invalid_chat_id");
        }
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chats WHERE id=?1)",
                [&id],
                |r| r.get(0),
            )
            .map_err(|_| "storage_error")?;
        if exists {
            continue;
        }
        remove(root, db, &id)?;
    }
    Ok(())
}

fn directory(root: &Path) -> Result<std::path::PathBuf, &'static str> {
    let path = root.join("chats");
    match fs::metadata(root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err("archive_missing"),
        Err(e) => return Err(io_failure("directory", &e)),
        Ok(_) => {}
    }
    if !path.exists() {
        fs::create_dir(&path).map_err(|e| io_failure("directory", &e))?;
    }
    let meta = fs::symlink_metadata(&path).map_err(|e| io_failure("directory", &e))?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("directory_other");
    }
    Ok(path)
}

fn target(root: &Path, id: &str) -> Result<std::path::PathBuf, &'static str> {
    if !crate::archive_receipt::valid_chat_id(id) {
        return Err("invalid_chat_id");
    }
    let path = directory(root)?.join(format!("{id}.json"));
    if let Ok(meta) = fs::symlink_metadata(&path) {
        if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
            return Err("file_other");
        }
    }
    Ok(path)
}

fn deletion_marker<'a>(
    data: &'a serde_json::Value,
    id: &str,
) -> Result<Option<&'a str>, &'static str> {
    let Some(deleted) = data.get("deleted") else {
        return Ok(None);
    };
    let valid_room = data
        .get("room")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|room| {
            room.len() == 1 && room.get("id").and_then(serde_json::Value::as_str) == Some(id)
        });
    let deleted_at = data.get("deletedAt").and_then(serde_json::Value::as_str);
    if data.as_object().is_none_or(|object| object.len() != 4)
        || data.get("version").and_then(serde_json::Value::as_u64) != Some(1)
        || deleted.as_bool() != Some(true)
        || !valid_room
        || deleted_at.is_none_or(|at| chrono::DateTime::parse_from_rfc3339(at).is_err())
    {
        return Err("invalid_snapshot");
    }
    Ok(deleted_at)
}

pub(crate) fn write_deletion(
    root: &Path,
    db: &Path,
    id: &str,
    deleted_at: &str,
) -> Result<(), &'static str> {
    let _guard = WRITE_LOCK.lock().map_err(|_| "unavailable")?;
    write_deletion_unlocked(root, db, id, deleted_at)
}

fn write_deletion_unlocked(
    root: &Path,
    db: &Path,
    id: &str,
    deleted_at: &str,
) -> Result<(), &'static str> {
    let path = target(root, id)?;
    let marker =
        serde_json::json!({"version":1,"room":{"id":id},"deleted":true,"deletedAt":deleted_at});
    deletion_marker(&marker, id)?;
    let bytes = serde_json::to_vec(&marker).map_err(|_| "unavailable")?;
    if fs::metadata(&path).is_ok_and(|meta| meta.len() > 32 * 1024 * 1024) {
        return Err("invalid_snapshot");
    }
    match fs::read(&path) {
        Ok(existing) => {
            if existing.len() > 32 * 1024 * 1024 {
                return Err("invalid_snapshot");
            }
            if existing == bytes {
                return Ok(());
            }
            let data: serde_json::Value =
                serde_json::from_slice(&existing).map_err(|_| "invalid_snapshot")?;
            if deletion_marker(&data, id)?.is_none() {
                if data.pointer("/room/id").and_then(serde_json::Value::as_str) != Some(id) {
                    return Err("invalid_snapshot");
                }
                if snapshot_hash(db, id)?.as_deref()
                    != Some(crate::object_store::digest(&existing).as_str())
                {
                    return Err("chat_deletion_conflict");
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(io_failure("file", &error)),
    }
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().ok_or("unavailable")?)
        .map_err(|error| io_failure("file", &error))?;
    tmp.write_all(&bytes)
        .map_err(|error| io_failure("file", &error))?;
    tmp.as_file()
        .sync_all()
        .map_err(|error| io_failure("file", &error))?;
    tmp.persist(&path)
        .map_err(|error| io_failure("file", &error.error))?;
    Ok(())
}

pub(crate) fn deleted_at(database: &Path, id: &str) -> Result<Option<String>, &'static str> {
    use rusqlite::OptionalExtension;
    let db =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "storage_error")?;
    db.query_row(
        "SELECT deleted_at FROM chat_deletions WHERE chat_id=?1",
        [id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|_| "storage_error")
}

pub(crate) fn write_known(
    root: &Path,
    db: &Path,
    id: &str,
    known: &[&str],
) -> Result<(), &'static str> {
    let _guard = lock_store()?;
    if deleted_at(db, id)?.is_some() {
        return Err("chat_deleted");
    }
    let path = target(root, id)?;
    let mut existing = None;
    let received = match fs::symlink_metadata(&path) {
        Ok(meta) => {
            if !meta.is_file()
                || crate::archive_location::is_link_or_reparse(&meta)
                || meta.len() > 32 * 1024 * 1024
            {
                return Err("invalid_snapshot");
            }
            let bytes = fs::read(&path).map_err(|e| io_failure("file", &e))?;
            let data: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| "invalid_snapshot")?;
            if deletion_marker(&data, id)?.is_some() {
                return Err("chat_deletion_conflict");
            }
            if data.pointer("/room/id").and_then(serde_json::Value::as_str) != Some(id) {
                return Err("invalid_snapshot");
            }
            let safe = crate::chat_archive_snapshot::redact_historical_snapshot(&data, known)?;
            crate::chat_archive_snapshot::restore_append(db, &safe)?;
            existing = Some(bytes);
            Some(safe)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(io_failure("file", &error)),
    };
    let data = crate::chat_archive_snapshot::snapshot_with_known(db, id, known).map_err(|e| {
        if e == "sensitive_metadata" {
            "redaction_failed"
        } else {
            e
        }
    })?;
    if let Some(received) = received {
        let saved = data
            .get("messages")
            .and_then(serde_json::Value::as_array)
            .ok_or("invalid_snapshot")?;
        let incoming = received
            .get("messages")
            .and_then(serde_json::Value::as_array)
            .ok_or("invalid_snapshot")?;
        let indexed: std::collections::HashMap<_, _> = saved
            .iter()
            .filter_map(|message| {
                message
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(|id| (id, message))
            })
            .collect();
        let keys = ["id", "chat_id", "role", "created_at"];
        let conn =
            rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|_| "storage_error")?;
        for message in incoming {
            use rusqlite::OptionalExtension;
            let message_id = message
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or("invalid_snapshot")?;
            let local = indexed.get(message_id).ok_or("restore_conflict")?;
            if keys.iter().any(|key| local.get(*key) != message.get(*key)) {
                return Err("restore_conflict");
            }
            if local.get("text") == message.get("text") {
                continue;
            }
            let previous: Option<(String, String, String)> = conn.query_row("SELECT p.source_hash,p.safe_text,m.text FROM chat_redaction_preserved p JOIN chat_messages m ON m.id=p.field_id AND m.chat_id=p.chat_id WHERE p.chat_id=?1 AND p.field_kind='message' AND p.field_id=?2", rusqlite::params![id,message_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(|_| "storage_error")?;
            let safe_change = previous.is_some_and(|(hash, previous, original)| {
                hash != crate::chat_archive_tracking::text_fingerprint(&original)
                    && message.get("text").and_then(serde_json::Value::as_str)
                        == Some(
                            crate::chat_archive_redact::redact_with_known(&previous, known)
                                .text
                                .as_str(),
                        )
            });
            if !safe_change {
                return Err("restore_conflict");
            }
        }
    }
    crate::chat_archive_snapshot::preserve_masked_fields(db, id, &data, known)?;
    let bytes = serde_json::to_vec(&data).map_err(|_| "file_other")?;
    if existing.as_deref() == Some(bytes.as_slice()) {
        return remember_snapshot(db, id, &bytes);
    }
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().ok_or("unavailable")?)
        .map_err(|e| io_failure("file", &e))?;
    tmp.write_all(&bytes).map_err(|e| io_failure("file", &e))?;
    tmp.flush().map_err(|e| io_failure("file", &e))?;
    tmp.as_file()
        .sync_all()
        .map_err(|e| io_failure("file", &e))?;
    tmp.persist(&path)
        .map_err(|e| io_failure("file", &e.error))?;
    remember_snapshot(db, id, &bytes)
}

fn known_values_with(
    data_root: &Path,
    database: &Path,
    get_secret: impl Fn(&str) -> Result<Option<String>, &'static str>,
    get_object_keys: impl Fn(&crate::object_store::Config) -> Result<(String, String), &'static str>,
) -> Result<Vec<String>, &'static str> {
    let mut known = Vec::new();
    for provider in ["openai", "anthropic", "gemini"] {
        let name =
            crate::credentials::ai_key_name(provider).map_err(|_| "credentials_unavailable")?;
        if let Some(value) = get_secret(name)? {
            known.push(value);
        }
    }
    if let Some(config) = crate::object_store::load_config(data_root)? {
        let (access, secret) = get_object_keys(&config)?;
        known.push(access);
        known.push(secret);
    }
    let db =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "storage_error")?;
    use rusqlite::OptionalExtension;
    let workspace: Option<String> = db
        .query_row(
            "SELECT workspace_id FROM momso_backup_profiles ORDER BY created_at ASC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| "storage_error")?;
    if let Some(id) = workspace {
        if let Some(value) = get_secret(&format!("backup:{id}:age-identity"))? {
            known.push(value);
        }
    }
    let waiting = crate::redaction_wait::material_with(&|name| {
        get_secret(name).map_err(|_| crate::credentials::CredentialError::ReadFailed)
    })
    .map_err(|_| "credentials_unavailable")?;
    known.extend(waiting);
    Ok(known)
}

#[cfg(not(test))]
pub(crate) fn known_values(data_root: &Path, database: &Path) -> Result<Vec<String>, &'static str> {
    known_values_with(
        data_root,
        database,
        |name| crate::credentials::get_secret(name).map_err(|_| "credentials_unavailable"),
        |config| {
            let keys =
                crate::object_store::load_keys(config).map_err(|_| "credentials_unavailable")?;
            Ok((keys.access_key_id, keys.secret_access_key))
        },
    )
}

#[cfg(test)]
pub(crate) fn known_values(data_root: &Path, database: &Path) -> Result<Vec<String>, &'static str> {
    known_values_with(data_root, database, |_| Ok(None), |_| Err("unavailable"))
}

pub(crate) fn write(
    root: &Path,
    paths: &crate::runtime::RuntimePaths,
    id: &str,
) -> Result<(), &'static str> {
    let result = (|| {
        let known = known_values(&paths.data_root, &paths.database_path)?;
        let refs: Vec<&str> = known.iter().map(String::as_str).collect();
        write_known(root, &paths.database_path, id, &refs)
    })();
    let _ = set_pending(&paths.database_path, &pending_key(id), result.is_err());
    result
}

pub(crate) fn validate(root: &Path, database: &Path) -> Result<(), &'static str> {
    validate_inner(root, database, false)
}

pub(crate) fn validate_inner(
    root: &Path,
    database: &Path,
    per_file: bool,
) -> Result<(), &'static str> {
    let known = known_values(database.parent().ok_or("unavailable")?, database)?;
    validate_with_known(root, database, per_file, &known)
}

pub(crate) fn validate_with_known(
    root: &Path,
    database: &Path,
    per_file: bool,
    known: &[String],
) -> Result<(), &'static str> {
    let _all = if per_file {
        None
    } else {
        Some(WRITE_LOCK.lock().map_err(|_| "unavailable")?)
    };
    let dir = root.join("chats");
    if !dir.exists() {
        return Ok(());
    }
    let dir = directory(root)?;
    let refs: Vec<&str> = known.iter().map(String::as_str).collect();
    let db =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "storage_error")?;
    for entry in fs::read_dir(dir).map_err(|_| "unavailable")? {
        let _guard = if per_file {
            Some(WRITE_LOCK.lock().map_err(|_| "unavailable")?)
        } else {
            None
        };
        let entry = entry.map_err(|_| "unavailable")?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "invalid_snapshot")?;
        let id = name
            .strip_suffix(".json")
            .filter(|id| crate::archive_receipt::valid_chat_id(id))
            .ok_or("invalid_snapshot")?;
        let meta = fs::symlink_metadata(entry.path()).map_err(|_| "unavailable")?;
        if !meta.is_file()
            || crate::archive_location::is_link_or_reparse(&meta)
            || meta.len() > 32 * 1024 * 1024
        {
            return Err("invalid_snapshot");
        }
        let actual: serde_json::Value =
            serde_json::from_slice(&fs::read(entry.path()).map_err(|_| "unavailable")?)
                .map_err(|_| "invalid_snapshot")?;
        if let Some(deleted_at) = deletion_marker(&actual, id)? {
            let matches: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM chat_deletions WHERE chat_id=?1 AND deleted_at=?2) AND NOT EXISTS(SELECT 1 FROM chats WHERE id=?1)", rusqlite::params![id, deleted_at], |row| row.get(0)).map_err(|_| "storage_error")?;
            if !matches {
                return Err("invalid_snapshot");
            }
            continue;
        }
        let expected =
            crate::chat_archive_snapshot::snapshot_connection_with_known(&db, id, &refs)?;
        if actual != expected {
            return Err("invalid_snapshot");
        }
    }
    Ok(())
}

pub(crate) fn write_all(
    root: &Path,
    paths: &crate::runtime::RuntimePaths,
) -> Result<(), &'static str> {
    let result = write_all_inner(root, paths);
    let _ = set_pending(
        &paths.database_path,
        "chat_archive_write:all",
        result.is_err(),
    );
    result
}

fn write_all_inner(root: &Path, paths: &crate::runtime::RuntimePaths) -> Result<(), &'static str> {
    retry_deletions(root, &paths.database_path)?;
    let db = rusqlite::Connection::open_with_flags(
        &paths.database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| "storage_error")?;
    let mut deletions = db
        .prepare("SELECT chat_id,deleted_at FROM chat_deletions ORDER BY chat_id")
        .map_err(|_| "storage_error")?;
    let deleted = deletions
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| "storage_error")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "storage_error")?;
    drop(deletions);
    for (id, deleted_at) in deleted {
        write_deletion(root, &paths.database_path, &id, &deleted_at)?;
    }
    let mut statement = db
        .prepare("SELECT id FROM chats WHERE NOT EXISTS(SELECT 1 FROM chat_deletions d WHERE d.chat_id=chats.id) ORDER BY id")
        .map_err(|_| "storage_error")?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| "storage_error")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "storage_error")?;
    drop(statement);
    drop(db);
    for id in ids {
        write(root, paths, &id)?;
    }
    Ok(())
}

pub(crate) fn save_pending(
    root: &Path,
    resources: &Path,
    paths: &crate::runtime::RuntimePaths,
) -> Result<(), &'static str> {
    save_pending_inner(root, resources, paths, false)
}

pub(crate) fn save_pending_for_push(
    root: &Path,
    resources: &Path,
    paths: &crate::runtime::RuntimePaths,
) -> Result<(), &'static str> {
    save_pending_inner(root, resources, paths, true)
}

static SAVE_LOCK: Mutex<()> = Mutex::new(());

fn save_pending_inner(
    root: &Path,
    resources: &Path,
    paths: &crate::runtime::RuntimePaths,
    wait_for_credentials: bool,
) -> Result<(), &'static str> {
    let _save = if wait_for_credentials {
        SAVE_LOCK.lock().map_err(|_| "unavailable")?
    } else {
        SAVE_LOCK.try_lock().map_err(|_| "redaction_pending")?
    };
    crate::chat_archive_tracking::ensure_tracking(&paths.database_path)?;
    let proof_key = crate::chat_archive_tracking::proof_key()?;
    let _credentials = if wait_for_credentials {
        crate::credentials::prevent_secret_changes().map_err(|_| "credentials_unavailable")?
    } else {
        crate::credentials::try_prevent_secret_changes().map_err(|_| "redaction_pending")?
    };
    let known = known_values(&paths.data_root, &paths.database_path)?;
    let proof = crate::chat_archive_tracking::proof(root, &known, &proof_key)?;
    let changes = crate::chat_archive_tracking::changes(&paths.database_path)?;
    let (git, _) = crate::git_tool::resolve().ok_or("git_missing")?;
    let saved_proof = crate::chat_archive_tracking::stored_proof(&paths.database_path)?;
    if !known.is_empty() && saved_proof.as_deref() != Some(proof.as_str()) {
        return Err("redaction_pending");
    }
    let mut other_paths = vec![
        "zettels/",
        "receipts/",
        "AGENTS.md",
        "CLAUDE.md",
        "GEMINI.md",
    ];
    let dictionary = crate::yoga_dictionary::paths();
    other_paths.extend(dictionary.iter().map(String::as_str));
    let mut status = vec!["status", "--porcelain", "--untracked-files=all", "--"];
    status.extend(other_paths);
    let other_changes = crate::archive_remote::git(&git, root, &status).map_err(|_| {
        if root.join(".git").exists() {
            "git_unavailable"
        } else {
            "git_not_repository"
        }
    })?;
    if crate::chat_archive_tracking::requires_full_retry(&paths.database_path)? {
        if !known.is_empty() {
            return Err("redaction_pending");
        }
        write_all(root, paths)?;
        if crate::chat_archive_tracking::requires_full_retry(&paths.database_path)? {
            return Err("unavailable");
        }
    }
    if !other_changes.is_empty() {
        crate::archive::save(root, resources, &paths.database_path)
            .map_err(|_| "chat_archive_save_failed")?;
    }
    for chunk in changes.chunks(32) {
        save_pending_changes(root, paths, &git, chunk, &known)?;
    }
    verify_proof_and_finish(root, paths, &changes, &proof, &proof_key)
}

fn redaction_pending_known(
    root: &Path,
    db: &Path,
    key: &str,
    known: &[String],
) -> Result<bool, &'static str> {
    let saved = crate::chat_archive_tracking::stored_proof(db)?;
    if known.is_empty() {
        return Ok(false);
    }
    let proof = crate::chat_archive_tracking::proof(root, known, key)?;
    Ok(saved.as_deref() != Some(proof.as_str()))
}

pub(crate) fn redaction_pending_with_key(root: &Path, db: &Path, key: &str) -> bool {
    let Some(data_root) = db.parent() else {
        return true;
    };
    let Ok(known) = known_values(data_root, db) else {
        return true;
    };
    let Ok(proof) = crate::chat_archive_tracking::proof(root, &known, key) else {
        return true;
    };
    !matches!(crate::chat_archive_tracking::stored_proof(db), Ok(Some(saved)) if saved == proof)
}

pub(crate) fn redaction_pending(root: &Path, db: &Path) -> Result<bool, &'static str> {
    let key = crate::chat_archive_tracking::proof_key()?;
    let _guard =
        crate::credentials::prevent_secret_changes().map_err(|_| "credentials_unavailable")?;
    let data_root = db.parent().ok_or("storage_error")?;
    let known = known_values(data_root, db)?;
    redaction_pending_known(root, db, &key, &known)
}

fn verify_proof_and_finish(
    root: &Path,
    paths: &crate::runtime::RuntimePaths,
    changes: &[(String, i64)],
    proof: &str,
    proof_key: &str,
) -> Result<(), &'static str> {
    let latest = known_values(&paths.data_root, &paths.database_path)?;
    if crate::chat_archive_tracking::proof(root, &latest, proof_key)? != proof {
        return Err("credentials_unavailable");
    }
    crate::chat_archive_tracking::finish(&paths.database_path, changes, proof)?;
    if !crate::chat_archive_tracking::changes(&paths.database_path)?.is_empty() {
        return Err("chat_archive_changed_during_save");
    }
    Ok(())
}

fn save_pending_changes(
    root: &Path,
    paths: &crate::runtime::RuntimePaths,
    git: &Path,
    changes: &[(String, i64)],
    known: &[String],
) -> Result<(), &'static str> {
    #[cfg(test)]
    let mut redaction_time = std::time::Duration::ZERO;
    retry_deletions(root, &paths.database_path)?;
    let _archive = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "unavailable")?;
    let db = rusqlite::Connection::open_with_flags(
        &paths.database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| "storage_error")?;
    let refs: Vec<&str> = known.iter().map(String::as_str).collect();
    let mut files = Vec::new();
    for (id, _) in changes {
        if !crate::archive_receipt::valid_chat_id(id) {
            return Err("invalid_chat_id");
        }
        let relative = format!("chats/{id}.json");
        let exists: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chats WHERE id=?1)",
                [id],
                |row| row.get(0),
            )
            .map_err(|_| "storage_error")?;
        if exists {
            #[cfg(test)]
            let started = std::time::Instant::now();
            write_known(root, &paths.database_path, id, &refs)?;
            #[cfg(test)]
            {
                redaction_time += started.elapsed();
            }
            files.push(relative);
        } else {
            let deleted_at = deleted_at(&paths.database_path, id)?.ok_or("storage_error")?;
            write_deletion(root, &paths.database_path, id, &deleted_at)?;
            files.push(relative);
        }
    }
    #[cfg(test)]
    if std::env::var_os("MOMSO_SCALE_PROFILE").is_some() {
        println!(
            "BENCH fast_snapshot_merge_and_redaction_ms={}",
            redaction_time.as_millis()
        );
    }
    if !files.is_empty() {
        let mut add = vec!["--literal-pathspecs", "add", "-A", "--"];
        add.extend(files.iter().map(String::as_str));
        #[cfg(test)]
        let started = std::time::Instant::now();
        crate::archive_remote::git(git, root, &add).map_err(|_| "git_unavailable")?;
        #[cfg(test)]
        if std::env::var_os("MOMSO_SCALE_PROFILE").is_some() {
            println!("BENCH fast_git_add_ms={}", started.elapsed().as_millis());
        }
        let mut diff = vec![
            "--literal-pathspecs",
            "diff",
            "--cached",
            "--name-only",
            "--",
        ];
        diff.extend(files.iter().map(String::as_str));
        let staged = crate::archive_remote::git(git, root, &diff).map_err(|_| "git_unavailable")?;
        if !staged.is_empty() {
            #[cfg(test)]
            let started = std::time::Instant::now();
            crate::archive::commit_paths(git, root, "몸소 업데이트: 채팅 사본 저장", &files)
                .map_err(|_| "chat_archive_save_failed")?;
            #[cfg(test)]
            if std::env::var_os("MOMSO_SCALE_PROFILE").is_some() {
                println!("BENCH fast_git_commit_ms={}", started.elapsed().as_millis());
            }
        }
    }
    Ok(())
}

pub(crate) fn remove(root: &Path, database: &Path, id: &str) -> Result<(), &'static str> {
    let deleted_at = deleted_at(database, id)?.ok_or("storage_error")?;
    write_deletion(root, database, id, &deleted_at)?;
    if crate::archive_remote::first_receive_pending(root, database) {
        Ok(())
    } else {
        set_pending(database, &format!("chat_archive_delete:{id}"), false)
    }
}

pub(crate) fn restore(root: &Path, db: &Path) -> Result<(), &'static str> {
    restore_with_progress(root, db, |_| {})
}

pub(crate) fn restore_emitting<R: tauri::Runtime>(
    root: &Path,
    db: &Path,
    app: &tauri::AppHandle<R>,
) -> Result<(), &'static str> {
    use tauri::Emitter;
    restore_with_progress(root, db, |progress| {
        let _ = app.emit("chat-archive-restore-progress", progress);
    })
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestoreProgress {
    pub(crate) completed: usize,
    pub(crate) total: usize,
    pub(crate) state: &'static str,
    pub(crate) available: bool,
}

fn message_is_represented(local: &str, incoming: &str, known: &[&str]) -> bool {
    local == incoming
        || crate::chat_archive_redact::redact_with_known(local, known).text == incoming
}

pub(crate) fn restore_with_progress(
    root: &Path,
    db: &Path,
    mut progress: impl FnMut(RestoreProgress),
) -> Result<(), &'static str> {
    let mut total = 0;
    let mut completed = 0;
    let result = (|| {
        retry_deletions(root, db)?;
        let known = db
            .parent()
            .and_then(|parent| known_values(parent, db).ok())
            .unwrap_or_default();
        let known_refs: Vec<&str> = known.iter().map(String::as_str).collect();
        let _guard = WRITE_LOCK.lock().map_err(|_| "unavailable")?;
        let root_meta = fs::symlink_metadata(root).map_err(|_| "unavailable")?;
        if !root_meta.is_dir() || crate::archive_location::is_link_or_reparse(&root_meta) {
            return Err("unavailable");
        }
        let dir = root.join("chats");
        match fs::symlink_metadata(&dir) {
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err("unavailable"),
        }
        let dir = directory(root)?;
        let mut entries = fs::read_dir(dir)
            .map_err(|_| "unavailable")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "unavailable")?;
        entries.sort_by_key(|entry| entry.file_name());
        total = entries.len();
        if entries.is_empty() {
            return Ok(());
        }
        progress(RestoreProgress {
            completed,
            total,
            state: "restoring",
            available: false,
        });
        let mut connection = rusqlite::Connection::open(db).map_err(|_| "storage_error")?;
        connection
            .execute_batch("PRAGMA foreign_keys=ON")
            .map_err(|_| "storage_error")?;
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| "storage_error")?;
        let mut deletions = Vec::new();
        for entry in entries {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "invalid_snapshot")?;
            let id = name
                .strip_suffix(".json")
                .filter(|id| crate::archive_receipt::valid_chat_id(id))
                .ok_or("invalid_snapshot")?;
            let meta = fs::symlink_metadata(entry.path()).map_err(|_| "unavailable")?;
            if !meta.is_file()
                || crate::archive_location::is_link_or_reparse(&meta)
                || meta.len() > 32 * 1024 * 1024
            {
                return Err("invalid_snapshot");
            }
            let bytes = fs::read(entry.path()).map_err(|_| "unavailable")?;
            let data: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| "invalid_snapshot")?;
            if let Some(deleted_at) = deletion_marker(&data, id)? {
                deletions.push((id.to_owned(), deleted_at.to_owned()));
                completed += 1;
                progress(RestoreProgress {
                    completed,
                    total,
                    state: "restoring",
                    available: false,
                });
                continue;
            }
            if data.pointer("/room/id").and_then(serde_json::Value::as_str) != Some(id) {
                return Err("invalid_snapshot");
            }
            crate::chat_archive_snapshot::restore_append_in_transaction(&tx, &data)?;
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM chats WHERE id=?1)",
                    [id],
                    |row| row.get(0),
                )
                .map_err(|_| "storage_error")?;
            if !exists {
                tx.execute(
                    "DELETE FROM app_runtime_meta WHERE key=?1",
                    [snapshot_hash_key(id)],
                )
                .map_err(|_| "storage_error")?;
            }
            let mut represented = exists;
            for message in data["messages"]
                .as_array()
                .ok_or("invalid_snapshot")?
                .iter()
                .filter(|_| exists)
            {
                let message_id = message["id"].as_str().ok_or("invalid_snapshot")?;
                let incoming = message["text"].as_str().ok_or("invalid_snapshot")?;
                let local: String = tx
                    .query_row(
                        "SELECT text FROM chat_messages WHERE id=?1 AND chat_id=?2",
                        rusqlite::params![message_id, id],
                        |row| row.get(0),
                    )
                    .map_err(|_| "storage_error")?;
                if !message_is_represented(&local, incoming, &known_refs) {
                    represented = false;
                    break;
                }
            }
            if represented {
                tx.execute(
                    "INSERT INTO app_runtime_meta(key,value,updated_at) VALUES(?1,?2,CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=CURRENT_TIMESTAMP",
                    rusqlite::params![snapshot_hash_key(id), crate::object_store::digest(&bytes)],
                )
                .map_err(|_| "storage_error")?;
            }
            completed += 1;
            progress(RestoreProgress {
                completed,
                total,
                state: "restoring",
                available: false,
            });
        }
        crate::chat_archive_snapshot::resolve_pending_in_transaction(&tx)?;
        crate::chat::apply_archived_deletions_in_transaction(&tx, &deletions).map_err(|error| {
            if error == "chat_deletion_conflict" {
                "chat_deletion_conflict"
            } else {
                "storage_error"
            }
        })?;
        tx.commit().map_err(|_| "storage_error")
    })();
    progress(RestoreProgress {
        completed,
        total,
        state: if result.is_ok() { "complete" } else { "failed" },
        available: result.is_ok(),
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonblocking_save_returns_pending_when_another_save_holds_lock() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_owned();
        let archive = root.join("momso");
        let db = root.join("momso.db");
        crate::storage::initialize(&db, &root.join("backups"), "fixture").unwrap();
        let location =
            crate::archive_location::ArchiveLocation::load(db.clone(), &root, vec![]).unwrap();
        let paths = crate::runtime::RuntimePaths {
            data_root: root.clone(),
            database_path: db,
            objects_path: archive.join("objects"),
            exports_path: root.join("exports"),
            backups_path: root.join("backups"),
            logs_path: root.join("logs"),
            archive: location,
            archive_lease: None,
        };
        let guard = SAVE_LOCK.lock().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = tx.send(save_pending(&archive, &root, &paths));
        });
        let result = rx.recv_timeout(std::time::Duration::from_secs(2));
        drop(guard);
        worker.join().unwrap();
        assert_eq!(result.unwrap(), Err("redaction_pending"));
    }

    #[test]
    fn empty_credentials_wait_for_short_lock_contention() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "fixture").unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _guard = crate::credentials::prevent_secret_changes().unwrap();
            ready_tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(250));
        });
        ready_rx.recv().unwrap();
        let pending = redaction_pending(temp.path(), &db);
        holder.join().unwrap();
        assert_eq!(pending, Ok(false));
    }

    #[test]
    fn missing_proof_without_keys_does_not_block_but_errors_remain_errors() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "fixture").unwrap();
        assert_eq!(
            redaction_pending_known(temp.path(), &db, "fixture-key", &[]),
            Ok(false)
        );
        assert_eq!(
            redaction_pending_known(temp.path(), &db, "fixture-key", &["fixture-secret".into()]),
            Ok(true)
        );
        assert_eq!(
            redaction_pending_known(
                temp.path(),
                &temp.path().join("missing.db"),
                "fixture-key",
                &[]
            ),
            Err("storage_error")
        );
    }

    #[test]
    fn received_known_secret_matches_only_after_redaction() {
        let original = "lesson fixture-private-value";
        let masked =
            crate::chat_archive_redact::redact_with_known(original, &["fixture-private-value"])
                .text;
        assert!(message_is_represented(
            original,
            &masked,
            &["fixture-private-value"]
        ));
        assert!(!message_is_represented(original, &masked, &[]));
        assert!(!message_is_represented(
            "different text",
            &masked,
            &["fixture-private-value"]
        ));
    }

    #[test]
    fn incremental_save_tracks_only_changed_room_and_preserves_unrelated_staging() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let archive = root.join("momso");
        let db = root.join("momso.db");
        crate::storage::initialize(&db, &root.join("backups"), "test").unwrap();
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
        let room1 = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
        let room2 = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
        let id1 = room1["id"].as_str().unwrap();
        let id2 = room2["id"].as_str().unwrap();
        crate::chat::append(&db, id1, crate::chat::Role::User, "first", None, None).unwrap();
        crate::chat::append(&db, id2, crate::chat::Role::User, "second", None, None).unwrap();
        write_all(&archive, &paths).unwrap();
        crate::archive::save(&archive, root, &db).unwrap();
        while !crate::chat_redaction_recheck::step(&archive, &paths)
            .unwrap()
            .available
        {}
        save_pending_for_push(&archive, root, &paths).unwrap();
        assert!(crate::chat_archive_tracking::changes(&db)
            .unwrap()
            .is_empty());
        let (git, _) = crate::git_tool::resolve().unwrap();
        let untouched = archive.join("chats").join(format!("{id2}.json"));
        let original = fs::read(&untouched).unwrap();
        let fake_key = format!("sk-proj-{}", "x".repeat(32));
        fs::write(&untouched, &fake_key).unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE app_runtime_meta SET value='stale' WHERE key='chat_archive_redaction_proof_v1'",
            [],
        )
        .unwrap();
        drop(conn);
        let mut found_error = None;
        for _ in 0..3 {
            match crate::chat_redaction_recheck::step(&archive, &paths) {
                Ok(progress) if progress.available => break,
                Ok(_) => {}
                Err(error) => {
                    found_error = Some(error);
                    break;
                }
            }
        }
        assert_eq!(found_error, Some("invalid_snapshot"));
        fs::write(&untouched, &original).unwrap();
        while !crate::chat_redaction_recheck::step(&archive, &paths)
            .unwrap()
            .available
        {}
        save_pending_for_push(&archive, root, &paths).unwrap();
        assert!(!fs::read_to_string(&untouched).unwrap().contains(&fake_key));
        fs::write(archive.join("outside-stage"), "fixture").unwrap();
        crate::archive_remote::git(&git, &archive, &["add", "--", "outside-stage"]).unwrap();
        fs::write(&untouched, &fake_key).unwrap();
        crate::chat::append(&db, id1, crate::chat::Role::User, "new reply", None, None).unwrap();
        let dirty = crate::chat_archive_tracking::changes(&db).unwrap();
        assert_eq!(dirty.len(), 1);
        assert_eq!(dirty[0].0, id1);
        save_pending_for_push(&archive, root, &paths).unwrap();
        assert!(crate::chat_archive_tracking::changes(&db)
            .unwrap()
            .is_empty());
        let changed = crate::archive_remote::git(
            &git,
            &archive,
            &["show", "--pretty=format:", "--name-only", "HEAD"],
        )
        .unwrap();
        assert_eq!(changed, format!("chats/{id1}.json"));
        let committed = crate::archive_remote::git(
            &git,
            &archive,
            &["show", &format!("HEAD:chats/{id2}.json")],
        )
        .unwrap();
        assert!(!committed.contains(&fake_key));
        let staged =
            crate::archive_remote::git(&git, &archive, &["diff", "--cached", "--name-only"])
                .unwrap();
        assert_eq!(staged, "outside-stage");
        assert_eq!(validate(&archive, &db), Err("invalid_snapshot"));
        fs::write(&untouched, original).unwrap();
        let mut conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        let tx = conn.transaction().unwrap();
        crate::chat::apply_deletion_in_transaction(&tx, id1, "2026-11-17T00:00:00Z").unwrap();
        tx.commit().unwrap();
        drop(conn);
        save_pending_for_push(&archive, root, &paths).unwrap();
        let tombstone = crate::archive_remote::git(
            &git,
            &archive,
            &["show", &format!("HEAD:chats/{id1}.json")],
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&tombstone).unwrap()["deleted"],
            true
        );
    }

    #[test]
    fn safe_diagnostics_classifies_errors_without_leaking_untrusted_text() {
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            io_failure("directory", &denied),
            "directory_permission_denied"
        );
        #[cfg(windows)]
        {
            assert_eq!(
                io_failure("file", &std::io::Error::from_raw_os_error(206)),
                "file_path_too_long"
            );
            assert_eq!(
                io_failure("file", &std::io::Error::from_raw_os_error(112)),
                "file_disk_error"
            );
        }
        for (code, reason, stage) in [
            ("archive_missing", "archiveMissing", "directory"),
            ("git_not_repository", "notGitRepository", "git"),
            (
                "directory_permission_denied",
                "permissionDenied",
                "directory",
            ),
            ("file_path_too_long", "pathTooLong", "file"),
            ("file_disk_error", "diskError", "file"),
            ("redaction_failed", "redactionFailed", "redaction"),
            ("chat_deletion_conflict", "deletionConflict", "conflict"),
            (
                "credentials_unavailable",
                "credentialStoreUnavailable",
                "credentials",
            ),
        ] {
            assert_eq!(failure_details(code), (reason, stage));
            let line = safe_diagnostic(code);
            assert!(line.contains("relativePath=chats relativePathLength=5"));
            assert!(line.contains(reason));
        }
        let secret = concat!("C:\\", "Users\\private-user\\secret-message");
        let line = safe_diagnostic(secret);
        assert!(!line.contains(secret));
        assert!(line.contains("reason=other"));
        assert_eq!(
            directory(&std::env::temp_dir().join(uuid::Uuid::new_v4().to_string())),
            Err("archive_missing")
        );
    }

    #[test]
    fn empty_credential_store_allows_snapshot_secret_collection() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let result = known_values_with(temp.path(), &db, |_| Ok(None), |_| Err("unavailable"));
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn unavailable_credential_store_fails_closed_without_creating_copies() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let result = known_values_with(
            temp.path(),
            &db,
            |_| Err("unavailable"),
            |_| Err("unavailable"),
        );
        assert!(matches!(result, Err("unavailable")));
        assert!(!temp.path().join("chats").exists());
    }

    #[tokio::test]
    async fn failed_snapshot_deletion_is_retried_after_local_delete() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let db = root.join("momso.db");
        crate::storage::initialize(&db, &root.join("backups"), "test").unwrap();
        let archive = root.join("momso");
        fs::create_dir(&archive).unwrap();
        let room = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
        let id = room["id"].as_str().unwrap();
        write_known(&archive, &db, id, &[]).unwrap();
        let other_db = root.join("other.sqlite");
        crate::storage::initialize(&other_db, &root.join("other-backups"), "test").unwrap();
        let original = crate::chat_archive_snapshot::snapshot(&db, id).unwrap();
        crate::chat_archive_snapshot::restore_new(&other_db, &original).unwrap();
        let deleted =
            crate::chat::delete_with(&db, &Default::default(), id, |_| async { Ok(()) }).await;
        assert_eq!(serde_json::to_value(deleted).unwrap()["state"], "deleted");
        let copy = archive.join("chats").join(format!("{id}.json"));
        assert!(copy.exists());
        fs::rename(archive.join("chats"), archive.join("held-chats")).unwrap();
        fs::write(archive.join("chats"), b"fixture blocker").unwrap();
        assert!(remove(&archive, &db, id).is_err());
        fs::remove_file(archive.join("chats")).unwrap();
        fs::rename(archive.join("held-chats"), archive.join("chats")).unwrap();
        let previous_copy = fs::read(&copy).unwrap();
        crate::chat::append(
            &other_db,
            id,
            crate::chat::Role::User,
            "new remote message",
            None,
            None,
        )
        .unwrap();
        let incoming =
            serde_json::to_vec(&crate::chat_archive_snapshot::snapshot(&other_db, id).unwrap())
                .unwrap();
        fs::write(&copy, &incoming).unwrap();
        assert_eq!(
            retry_deletions(&archive, &db),
            Err("chat_deletion_conflict")
        );
        assert_eq!(fs::read(&copy).unwrap(), incoming);
        let mut progress_events = Vec::new();
        assert_eq!(
            restore_with_progress(&archive, &db, |event| progress_events.push(event)),
            Err("chat_deletion_conflict")
        );
        let last = progress_events
            .last()
            .expect("a failed progress event is emitted");
        assert_eq!(last.state, "failed");
        assert!(!last.available);
        assert_eq!(fs::read(&copy).unwrap(), incoming);
        fs::write(&copy, &previous_copy).unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "DELETE FROM app_runtime_meta WHERE key=?1",
            [snapshot_hash_key(id)],
        )
        .unwrap();
        assert_eq!(
            retry_deletions(&archive, &db),
            Err("chat_deletion_conflict")
        );
        assert_eq!(fs::read(&copy).unwrap(), previous_copy);
        remember_snapshot(&db, id, &previous_copy).unwrap();
        retry_deletions(&archive, &db).unwrap();
        let marker: serde_json::Value = serde_json::from_slice(&fs::read(&copy).unwrap()).unwrap();
        assert_eq!(marker["deleted"], true);
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM app_runtime_meta WHERE key GLOB 'chat_archive_delete:*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn received_snapshot_can_be_deleted_without_rewriting_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let remote = root.join("remote.db");
        let local = root.join("local.db");
        crate::storage::initialize(&remote, &root.join("remote-backups"), "test").unwrap();
        crate::storage::initialize(&local, &root.join("local-backups"), "test").unwrap();
        let archive = root.join("momso");
        fs::create_dir(&archive).unwrap();
        let room = serde_json::to_value(crate::chat::create(&remote).unwrap()).unwrap();
        let id = room["id"].as_str().unwrap();
        write_known(&archive, &remote, id, &[]).unwrap();
        restore(&archive, &local).unwrap();
        crate::chat::append(&remote, id, crate::chat::Role::User, "received", None, None).unwrap();
        write_known(&archive, &remote, id, &[]).unwrap();
        restore(&archive, &local).unwrap();
        assert_eq!(
            serde_json::to_value(crate::chat::read(&local, id).unwrap()).unwrap()["messages"][0]
                ["text"],
            "received"
        );
        let deleted =
            crate::chat::delete_with(&local, &Default::default(), id, |_| async { Ok(()) }).await;
        assert_eq!(serde_json::to_value(deleted).unwrap()["state"], "deleted");
        remove(&archive, &local, id).unwrap();
        let marker: serde_json::Value = serde_json::from_slice(
            &fs::read(archive.join("chats").join(format!("{id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["deleted"], true);
    }

    #[tokio::test]
    async fn changed_message_with_same_id_is_not_acknowledged_for_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let remote = root.join("remote.db");
        let local = root.join("local.db");
        crate::storage::initialize(&remote, &root.join("remote-backups"), "test").unwrap();
        crate::storage::initialize(&local, &root.join("local-backups"), "test").unwrap();
        let archive = root.join("momso");
        fs::create_dir(&archive).unwrap();
        let room = serde_json::to_value(crate::chat::create(&remote).unwrap()).unwrap();
        let id = room["id"].as_str().unwrap();
        crate::chat::append(&remote, id, crate::chat::Role::User, "original", None, None).unwrap();
        write_known(&archive, &remote, id, &[]).unwrap();
        restore(&archive, &local).unwrap();
        let copy = archive.join("chats").join(format!("{id}.json"));
        let mut incoming: serde_json::Value =
            serde_json::from_slice(&fs::read(&copy).unwrap()).unwrap();
        incoming["messages"][0]["text"] = serde_json::json!("different remote text");
        let bytes = serde_json::to_vec(&incoming).unwrap();
        fs::write(&copy, &bytes).unwrap();
        restore(&archive, &local).unwrap();
        let deleted =
            crate::chat::delete_with(&local, &Default::default(), id, |_| async { Ok(()) }).await;
        assert_eq!(serde_json::to_value(deleted).unwrap()["state"], "deleted");
        assert_eq!(remove(&archive, &local, id), Err("chat_deletion_conflict"));
        assert_eq!(fs::read(&copy).unwrap(), bytes);
    }

    #[test]
    fn received_room_can_be_renamed_pinned_and_archived() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let remote = root.join("remote.db");
        let local = root.join("local.db");
        crate::storage::initialize(&remote, &root.join("remote-backups"), "test").unwrap();
        crate::storage::initialize(&local, &root.join("local-backups"), "test").unwrap();
        let archive = root.join("momso");
        fs::create_dir(&archive).unwrap();
        let room = serde_json::to_value(crate::chat::create(&remote).unwrap()).unwrap();
        let id = room["id"].as_str().unwrap();
        write_known(&archive, &remote, id, &[]).unwrap();
        restore(&archive, &local).unwrap();
        crate::chat::rename(&local, id, "renamed").unwrap();
        assert_eq!(crate::chat::set_pinned(&local, id, true).state, "ok");
        assert_eq!(crate::chat::set_archived(&local, id, true).state, "ok");
        write_known(&archive, &local, id, &[]).unwrap();
        let copy: serde_json::Value = serde_json::from_slice(
            &fs::read(archive.join("chats").join(format!("{id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(copy["room"]["title"], "renamed");
        assert!(!copy["room"]["pinned_order"].is_null());
        assert!(!copy["room"]["archived_at"].is_null());
    }

    #[test]
    fn failed_restore_keeps_received_snapshot_until_successful_retry() {
        let dir = tempfile::tempdir().unwrap();
        let remote = dir.path().join("remote.sqlite");
        let local = dir.path().join("local.sqlite");
        crate::storage::initialize(&remote, &dir.path().join("remote-backups"), "test").unwrap();
        crate::storage::initialize(&local, &dir.path().join("local-backups"), "test").unwrap();
        let room = serde_json::to_value(crate::chat::create(&remote).unwrap()).unwrap();
        let id = room["id"].as_str().unwrap();
        let initial = crate::chat_archive_snapshot::snapshot(&remote, id).unwrap();
        crate::chat_archive_snapshot::restore_new(&local, &initial).unwrap();
        crate::chat::append(
            &remote,
            id,
            crate::chat::Role::User,
            "received message",
            None,
            None,
        )
        .unwrap();
        crate::chat::append(
            &remote,
            id,
            crate::chat::Role::Assistant,
            "second message",
            None,
            None,
        )
        .unwrap();
        write_known(dir.path(), &remote, id, &[]).unwrap();
        let file = dir.path().join("chats").join(format!("{id}.json"));
        let saved = fs::read(&file).unwrap();
        assert!(write_known(
            dir.path(),
            &dir.path().join("missing").join("db.sqlite"),
            id,
            &[]
        )
        .is_err());
        assert_eq!(fs::read(&file).unwrap(), saved);
        let db = rusqlite::Connection::open(&local).unwrap();
        db.execute_batch("CREATE TRIGGER reject_second BEFORE INSERT ON chat_messages WHEN NEW.text = 'second message' BEGIN SELECT RAISE(ABORT, 'fixture'); END;").unwrap();
        assert!(write_known(dir.path(), &local, id, &[]).is_err());
        assert_eq!(fs::read(&file).unwrap(), saved);
        let after_failure = serde_json::to_value(crate::chat::read(&local, id).unwrap()).unwrap();
        assert!(after_failure["messages"].as_array().unwrap().is_empty());
        db.execute_batch("DROP TRIGGER reject_second").unwrap();
        write_known(dir.path(), &local, id, &[]).unwrap();
        let restored = serde_json::to_value(crate::chat::read(&local, id).unwrap()).unwrap();
        assert_eq!(restored["messages"][0]["text"], "received message");
        assert_eq!(restored["messages"][1]["text"], "second message");
        db.execute(
            "UPDATE chat_messages SET fast=1 WHERE text='second message'",
            [],
        )
        .unwrap();
        write_known(dir.path(), &local, id, &[]).unwrap();
        let mut conflict: serde_json::Value =
            serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        conflict["messages"][0]["text"] = "different received text".into();
        let conflict_bytes = serde_json::to_vec(&conflict).unwrap();
        fs::write(&file, &conflict_bytes).unwrap();
        assert_eq!(
            write_known(dir.path(), &local, id, &[]),
            Err("restore_conflict")
        );
        assert_eq!(fs::read(&file).unwrap(), conflict_bytes);
    }

    #[test]
    fn restore_progress_rolls_back_all_rooms_on_invalid_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.sqlite");
        let dest = dir.path().join("dest.sqlite");
        crate::storage::initialize(&source, &dir.path().join("backups"), "test").unwrap();
        crate::storage::initialize(&dest, &dir.path().join("other-backups"), "test").unwrap();
        let room = serde_json::to_value(crate::chat::create(&source).unwrap()).unwrap();
        let id = room["id"].as_str().unwrap();
        write_known(dir.path(), &source, id, &[]).unwrap();
        let invalid = dir
            .path()
            .join("chats/chat_ffffffff-ffff-4fff-8fff-ffffffffffff.json");
        fs::write(invalid, b"{}").unwrap();
        let mut events = Vec::new();
        assert!(restore_with_progress(dir.path(), &dest, |event| events.push(event)).is_err());
        assert!(crate::chat::list(&dest).unwrap().is_empty());
        assert_eq!(events.last().unwrap().state, "failed");
        assert!(!events.last().unwrap().available);
    }

    #[test]
    fn restore_progress_commits_before_reporting_available() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.sqlite");
        let dest = dir.path().join("dest.sqlite");
        crate::storage::initialize(&source, &dir.path().join("backups"), "test").unwrap();
        crate::storage::initialize(&dest, &dir.path().join("other-backups"), "test").unwrap();
        let room = serde_json::to_value(crate::chat::create(&source).unwrap()).unwrap();
        write_known(dir.path(), &source, room["id"].as_str().unwrap(), &[]).unwrap();
        let mut events = Vec::new();
        restore_with_progress(dir.path(), &dest, |event| {
            if event.state == "restoring" {
                assert!(crate::chat::list(&dest).unwrap().is_empty());
                assert!(!event.available);
            }
            events.push(event);
        })
        .unwrap();
        assert_eq!(crate::chat::list(&dest).unwrap().len(), 1);
        let event = events.last().unwrap();
        assert_eq!(
            (event.completed, event.total, event.state, event.available),
            (1, 1, "complete", true)
        );
    }

    #[test]
    fn changed_redacted_source_uses_current_keys_and_continues() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("fixture.sqlite");
        crate::storage::initialize(&db, &dir.path().join("backups"), "test").unwrap();
        let room = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
        let id = room["id"].as_str().unwrap();
        let old = "fixture-opaque-old-value";
        let current = "fixture-opaque-current-value";
        let message = serde_json::to_value(
            crate::chat::append(&db, id, crate::chat::Role::User, old, None, None).unwrap(),
        )
        .unwrap();
        let message_id = message["id"].as_str().unwrap();
        write_known(dir.path(), &db, id, &[old]).unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE chat_messages SET text=?1 WHERE id=?2",
            rusqlite::params![format!("changed {old} {current}"), message_id],
        )
        .unwrap();
        write_known(dir.path(), &db, id, &[old, current]).unwrap();
        let path = dir.path().join("chats").join(format!("{id}.json"));
        let copy = fs::read_to_string(&path).unwrap();
        assert!(!copy.contains(old) && !copy.contains(current));
        conn.execute(
            "UPDATE chat_messages SET text=?1 WHERE id=?2",
            rusqlite::params![format!("changed again {old} {current}"), message_id],
        )
        .unwrap();
        write_known(dir.path(), &db, id, &[current]).unwrap();
        let copy = fs::read_to_string(path).unwrap();
        assert!(copy.contains(old));
        assert!(!copy.contains(current));
    }

    #[test]
    fn copy_redacts_and_restore_only_appends() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("first.sqlite");
        crate::storage::initialize(&db, &dir.path().join("backups"), "test").unwrap();
        let room = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
        let id = room["id"].as_str().unwrap();
        let secret = "opaque-fixture-credential";
        crate::chat::append(&db, id, crate::chat::Role::User, secret, None, None).unwrap();
        write_known(dir.path(), &db, id, &[secret]).unwrap();
        let copy_path = dir.path().join("chats").join(format!("{id}.json"));
        let bytes = fs::read_to_string(&copy_path).unwrap();
        assert!(!bytes.contains(secret));
        let file = fs::OpenOptions::new().write(true).open(&copy_path).unwrap();
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000))
            .unwrap();
        drop(file);
        let modified = fs::metadata(&copy_path).unwrap().modified().unwrap();
        write_known(dir.path(), &db, id, &[secret]).unwrap();
        assert_eq!(
            fs::metadata(&copy_path).unwrap().modified().unwrap(),
            modified
        );
        restore(dir.path(), &db).unwrap();
        let original = serde_json::to_value(crate::chat::read(&db, id).unwrap()).unwrap();
        assert_eq!(original["messages"][0]["text"], secret);
        let restored = dir.path().join("second.sqlite");
        crate::storage::initialize(&restored, &dir.path().join("other-backups"), "test").unwrap();
        restore(dir.path(), &restored).unwrap();
        crate::chat::append(&restored, id, crate::chat::Role::User, "local", None, None).unwrap();
        restore(dir.path(), &restored).unwrap();
        let detail = serde_json::to_value(crate::chat::read(&restored, id).unwrap()).unwrap();
        assert_eq!(detail["messages"].as_array().unwrap().len(), 2);
        assert_eq!(detail["messages"][1]["text"], "local");
        crate::chat::append(&db, id, crate::chat::Role::Assistant, "remote", None, None).unwrap();
        write_known(dir.path(), &db, id, &[secret]).unwrap();
        restore(dir.path(), &restored).unwrap();
        let detail = serde_json::to_value(crate::chat::read(&restored, id).unwrap()).unwrap();
        assert_eq!(detail["messages"].as_array().unwrap().len(), 3);
        assert_eq!(detail["messages"][1]["text"], "local");
        assert_eq!(detail["messages"][2]["text"], "remote");
        let stale_copy = fs::read(&copy_path).unwrap();
        let delete_locally = |db: &Path| {
            let mut connection = crate::storage::open_writable_connection(db).unwrap();
            let tx = connection.transaction().unwrap();
            crate::chat::apply_deletion_in_transaction(&tx, id, "2026-11-17T00:00:00Z").unwrap();
            tx.commit().unwrap();
        };
        delete_locally(&db);
        remove(dir.path(), &db, id).unwrap();
        validate(dir.path(), &db).unwrap();
        let marker: serde_json::Value =
            serde_json::from_slice(&fs::read(&copy_path).unwrap()).unwrap();
        assert_eq!(marker["deleted"], true);
        assert_eq!(
            restore(dir.path(), &restored),
            Err("chat_deletion_conflict")
        );
        let detail = serde_json::to_value(crate::chat::read(&restored, id).unwrap()).unwrap();
        assert_eq!(detail["messages"].as_array().unwrap().len(), 3);
        let deletion_copy = fs::read(&copy_path).unwrap();
        assert_eq!(
            write_known(dir.path(), &restored, id, &[secret]),
            Err("chat_deletion_conflict")
        );
        assert_eq!(fs::read(&copy_path).unwrap(), deletion_copy);
        let detail = serde_json::to_value(crate::chat::read(&restored, id).unwrap()).unwrap();
        assert_eq!(detail["messages"].as_array().unwrap().len(), 3);
        fs::write(&copy_path, b"incomplete copy").unwrap();
        assert_eq!(
            write_known(dir.path(), &restored, id, &[secret]),
            Err("invalid_snapshot")
        );
        assert_eq!(fs::read(&copy_path).unwrap(), b"incomplete copy");
        fs::write(&copy_path, b"{}").unwrap();
        assert_eq!(
            write_known(dir.path(), &restored, id, &[secret]),
            Err("invalid_snapshot")
        );
        assert_eq!(fs::read(&copy_path).unwrap(), b"{}");
        fs::write(&copy_path, &deletion_copy).unwrap();
        delete_locally(&restored);
        restore(dir.path(), &restored).unwrap();
        assert!(crate::chat::read(&restored, id).is_err());
        fs::write(&copy_path, &stale_copy).unwrap();
        restore(dir.path(), &restored).unwrap();
        assert!(crate::chat::read(&restored, id).is_err());
        assert_eq!(fs::read(&copy_path).unwrap(), stale_copy);
        assert_eq!(
            remove(dir.path(), &restored, id),
            Err("chat_deletion_conflict")
        );
        assert_eq!(fs::read(&copy_path).unwrap(), stale_copy);
    }
}
