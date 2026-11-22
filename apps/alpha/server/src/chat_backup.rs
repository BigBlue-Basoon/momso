//! Portable chat copies. Originals are never modified by export or conflict handling.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
};

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Journal {
    entries: BTreeMap<String, JournalEntry>,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct JournalEntry {
    current: Option<String>,
    pending: Option<String>,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn read_regular(path: &Path) -> Result<Option<Vec<u8>>, &'static str> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("backup_storage_unavailable"),
    };
    if !file
        .metadata()
        .map_err(|_| "backup_storage_unavailable")?
        .is_file()
    {
        return Err("backup_unsafe_path");
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| "backup_storage_unavailable")?;
    Ok(Some(bytes))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), &'static str> {
    let parent = path.parent().ok_or("backup_storage_unavailable")?;
    let mut file =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| "backup_storage_unavailable")?;
    file.write_all(bytes)
        .map_err(|_| "backup_storage_unavailable")?;
    file.as_file()
        .sync_all()
        .map_err(|_| "backup_storage_unavailable")?;
    file.persist(path)
        .map_err(|_| "backup_storage_unavailable")?;
    std::fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|_| "backup_storage_unavailable")
}

fn save_journal(path: &Path, journal: &Journal) -> Result<(), &'static str> {
    let bytes = serde_json::to_vec(journal).map_err(|_| "backup_storage_unavailable")?;
    atomic_write(path, &bytes)
}

fn stage_generated(
    path: &Path,
    key: &str,
    bytes: &[u8],
    state_path: &Path,
    journal: &mut Journal,
) -> Result<(), &'static str> {
    let new_hash = digest(bytes);
    let existing = read_regular(path)?.map(|value| digest(&value));
    let entry = journal.entries.entry(key.to_owned()).or_default();
    if entry.pending.is_some() && existing == entry.pending {
        entry.current = entry.pending.take();
    }
    if existing.as_ref() == Some(&new_hash) {
        entry.current = Some(new_hash);
        entry.pending = None;
        return save_journal(state_path, journal);
    }
    if existing != entry.current {
        return Err("chat_backup_conflict");
    }
    entry.pending = Some(new_hash.clone());
    save_journal(state_path, journal)?;
    atomic_write(path, bytes)?;
    let entry = journal
        .entries
        .get_mut(key)
        .ok_or("backup_journal_invalid")?;
    entry.current = Some(new_hash);
    entry.pending = None;
    save_journal(state_path, journal)
}

/// Stage exported copies without overwriting files that were edited by a person.
/// A pending hash is persisted before replacing each previously generated copy.
pub fn stage_chat_copies(
    db: &mut Connection,
    archive: &Path,
    data: &Path,
    known: &[&str],
) -> Result<usize, &'static str> {
    let _guard = momso_core::lock::exclusive(archive)?;
    let directory = archive.join("chats");
    match std::fs::create_dir(&directory) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err("backup_storage_unavailable"),
    }
    if !std::fs::symlink_metadata(&directory)
        .map_err(|_| "backup_storage_unavailable")?
        .file_type()
        .is_dir()
    {
        return Err("backup_unsafe_path");
    }
    let state_path = data.join("chat-export-journal.json");
    let mut journal: Journal = match read_regular(&state_path)? {
        Some(bytes) => serde_json::from_slice(&bytes).map_err(|_| "backup_journal_invalid")?,
        None => Journal::default(),
    };
    let tx = db.transaction().map_err(|_| "storage_error")?;
    let ids: Vec<String> = {
        let mut statement = tx
            .prepare("SELECT id FROM chats UNION SELECT chat_id FROM chat_deletions ORDER BY 1")
            .map_err(|_| "storage_error")?;
        let result = statement
            .query_map([], |row| row.get(0))
            .map_err(|_| "storage_error")?
            .collect::<Result<_, _>>()
            .map_err(|_| "storage_error")?;
        result
    };
    for id in &ids {
        let deleted_at: Option<String> = tx
            .query_row(
                "SELECT deleted_at FROM chat_deletions WHERE chat_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| "storage_error")?;
        let snapshot = if let Some(deleted_at) = deleted_at {
            if uuid::Uuid::parse_str(id)
                .map_err(|_| "invalid_snapshot")?
                .to_string()
                != *id
                || chrono::DateTime::parse_from_rfc3339(&deleted_at).is_err()
            {
                return Err("invalid_snapshot");
            }
            require_safe_metadata(&deleted_at, known)?;
            json!({"version":1,"room":{"id":format!("chat_{id}")},"deleted":true,"deletedAt":deleted_at})
        } else {
            export_chat(&tx, id, known)?
        };
        let bytes = serde_json::to_vec(&snapshot).map_err(|_| "backup_storage_unavailable")?;
        let path = directory.join(format!("chat_{id}.json"));
        stage_generated(&path, id, &bytes, &state_path, &mut journal)?;
    }
    let groups = export_groups(&tx, known)?;
    if !groups["groups"]
        .as_array()
        .ok_or("invalid_snapshot")?
        .is_empty()
        || journal.entries.contains_key("__groups__")
    {
        let bytes = serde_json::to_vec(&groups).map_err(|_| "backup_storage_unavailable")?;
        stage_generated(
            &archive.join("chat-groups.json"),
            "__groups__",
            &bytes,
            &state_path,
            &mut journal,
        )?;
    }
    tx.commit().map_err(|_| "storage_error")?;
    Ok(ids.len())
}

fn clean_text(text: &str, known: &[&str]) -> (String, usize) {
    momso_github_backup::redact_chat_copy(text, known)
}

fn require_safe_metadata(value: &str, known: &[&str]) -> Result<(), &'static str> {
    if clean_text(value, known).1 != 0 {
        return Err("sensitive_metadata");
    }
    Ok(())
}

fn exact_fields(value: &Value, fields: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == fields.len() && fields.iter().all(|field| object.contains_key(*field))
    })
}

fn export_groups(db: &Connection, known: &[&str]) -> Result<Value, &'static str> {
    let mut statement = db
        .prepare("SELECT id,name,position,created_at,deleted_at FROM chat_groups ORDER BY id")
        .map_err(|_| "storage_error")?;
    let mut rows = statement.query([]).map_err(|_| "storage_error")?;
    let mut groups = Vec::new();
    while let Some(row) = rows.next().map_err(|_| "storage_error")? {
        let id: String = row.get(0).map_err(|_| "storage_error")?;
        let name: String = row.get(1).map_err(|_| "storage_error")?;
        let position: i64 = row.get(2).map_err(|_| "storage_error")?;
        let created: String = row.get(3).map_err(|_| "storage_error")?;
        let deleted: Option<String> = row.get(4).map_err(|_| "storage_error")?;
        if uuid::Uuid::parse_str(&id)
            .map_err(|_| "invalid_snapshot")?
            .to_string()
            != id
            || position < 0
            || chrono::DateTime::parse_from_rfc3339(&created).is_err()
            || deleted
                .as_deref()
                .is_some_and(|value| chrono::DateTime::parse_from_rfc3339(value).is_err())
        {
            return Err("invalid_snapshot");
        }
        for value in [&id, &created].into_iter().chain(deleted.iter()) {
            require_safe_metadata(value, known)?;
        }
        let (name, _) = clean_text(&name, known);
        if name.chars().count() > 200 {
            return Err("sensitive_metadata");
        }
        groups.push(json!({"id":id,"name":name,"position":position,"created_at":created,"deleted_at":deleted}));
    }
    Ok(json!({"version":1,"groups":groups}))
}

fn restore_groups_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    manifest: &Value,
) -> Result<(), &'static str> {
    if !exact_fields(manifest, &["version", "groups"]) || manifest["version"] != 1 {
        return Err("invalid_snapshot");
    }
    let groups = manifest["groups"].as_array().ok_or("invalid_snapshot")?;
    if groups.len() > 10000 {
        return Err("invalid_snapshot");
    }
    let mut seen = std::collections::HashSet::new();
    for group in groups {
        if !exact_fields(
            group,
            &["id", "name", "position", "created_at", "deleted_at"],
        ) {
            return Err("invalid_snapshot");
        }
        let id = group["id"].as_str().ok_or("invalid_snapshot")?;
        let name = group["name"].as_str().ok_or("invalid_snapshot")?;
        let position = group["position"]
            .as_i64()
            .filter(|v| *v >= 0)
            .ok_or("invalid_snapshot")?;
        let created = group["created_at"].as_str().ok_or("invalid_snapshot")?;
        let deleted = match &group["deleted_at"] {
            Value::Null => None,
            Value::String(value) if chrono::DateTime::parse_from_rfc3339(value).is_ok() => {
                Some(value.as_str())
            }
            _ => return Err("invalid_snapshot"),
        };
        if uuid::Uuid::parse_str(id)
            .map_err(|_| "invalid_snapshot")?
            .to_string()
            != id
            || !seen.insert(id)
            || name.chars().count() > 200
            || name.chars().any(char::is_control)
            || (deleted.is_none() && name.trim().is_empty())
            || chrono::DateTime::parse_from_rfc3339(created).is_err()
        {
            return Err("invalid_snapshot");
        }
        let current: Option<(String, i64, String, Option<String>)> = tx
            .query_row(
                "SELECT name,position,created_at,deleted_at FROM chat_groups WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|_| "storage_error")?;
        if let Some((current_name, current_position, current_created, current_deleted)) = current {
            if current_name != name
                || current_position != position
                || current_created != created
                || current_deleted.as_deref() != deleted
            {
                return Err("restore_conflict");
            }
        } else {
            tx.execute("INSERT INTO chat_groups(id,name,position,created_at,deleted_at) VALUES(?1,?2,?3,?4,?5)",params![id,name,position,created,deleted]).map_err(|_| "storage_error")?;
        }
    }
    Ok(())
}

/// Produce the desktop v1 shape for one server chat, with an extra server-only
/// section preserving attachment names and media types for a later restore.
pub fn export_chat(db: &Connection, id: &str, known: &[&str]) -> Result<Value, &'static str> {
    let uuid = uuid::Uuid::parse_str(id).map_err(|_| "invalid_chat_id")?;
    if uuid.to_string() != id {
        return Err("invalid_chat_id");
    }
    let portable_id = format!("chat_{id}");
    let (title, created, updated, archived, pinned, group_id): (
        String,
        String,
        String,
        Option<String>,
        Option<i64>,
        Option<String>,
    ) = db
        .query_row(
            "SELECT title,created_at,updated_at,archived_at,pinned_order,group_id FROM chats WHERE id=?1",
            [id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .map_err(|_| "storage_error")?;
    let (title, mut redactions) = clean_text(&title, known);
    require_safe_metadata(&created, known)?;
    require_safe_metadata(&updated, known)?;
    if let Some(archived) = &archived {
        require_safe_metadata(archived, known)?;
    }
    let mut groups = Vec::new();
    if let Some(group_id) = &group_id {
        if uuid::Uuid::parse_str(group_id)
            .map_err(|_| "invalid_snapshot")?
            .to_string()
            != *group_id
        {
            return Err("invalid_snapshot");
        }
        let (name,position,created_at): (String,i64,String) = db.query_row("SELECT name,position,created_at FROM chat_groups WHERE id=?1 AND deleted_at IS NULL",[group_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(|_| "invalid_snapshot")?;
        require_safe_metadata(group_id, known)?;
        require_safe_metadata(&created_at, known)?;
        let (name, count) = clean_text(&name, known);
        redactions += count;
        groups.push(json!({"id":group_id,"name":name,"position":position,"created_at":created_at}));
    }
    let mut statement = db.prepare("SELECT id,role,text,created_at,provider,model,effort,skill,actual_effort,finish_reason FROM chat_messages WHERE chat_id=?1 ORDER BY rowid").map_err(|_| "storage_error")?;
    let mut rows = statement.query([id]).map_err(|_| "storage_error")?;
    let mut messages = Vec::new();
    let mut redacted_messages = std::collections::HashSet::new();
    while let Some(row) = rows.next().map_err(|_| "storage_error")? {
        let message_id: String = row.get(0).map_err(|_| "storage_error")?;
        let role: String = row.get(1).map_err(|_| "storage_error")?;
        let original: String = row.get(2).map_err(|_| "storage_error")?;
        let created_at: String = row.get(3).map_err(|_| "storage_error")?;
        let metadata: Vec<Option<String>> = (4..10)
            .map(|index| row.get(index).map_err(|_| "storage_error"))
            .collect::<Result<_, _>>()?;
        for item in [&message_id, &role, &created_at]
            .into_iter()
            .chain(metadata.iter().flatten())
        {
            require_safe_metadata(item, known)?;
        }
        let (text, count) = clean_text(&original, known);
        if count > 0 {
            redacted_messages.insert(message_id.clone());
        }
        redactions += count;
        messages.push(json!({"id":message_id,"chat_id":portable_id,"role":role,"text":text,"created_at":created_at,"provider":metadata[0],"model":metadata[1],"effort":metadata[2],"skill":metadata[3],"fast":0,"not_handed_over":0,"retryable_fast":0,"actual_effort":metadata[4],"finish_reason":metadata[5],"auth_method":null,"fast_state":null}));
    }
    let mut statement=db.prepare("SELECT source_provider,source_id FROM chat_imports WHERE chat_id=?1 ORDER BY source_provider,source_id").map_err(|_|"storage_error")?;
    let entries = statement
        .query_map([id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| "storage_error")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "storage_error")?;
    let mut imports = Vec::new();
    for (provider, source) in entries {
        require_safe_metadata(&provider, known)?;
        require_safe_metadata(&source, known)?;
        imports.push(json!({"chat_id":portable_id,"source_provider":provider,"source_id":source}));
    }
    let mut statement=db.prepare("SELECT ordinal,message_id,source_message_id,fingerprint FROM chat_import_message_state WHERE chat_id=?1 ORDER BY ordinal").map_err(|_|"storage_error")?;
    let entries = statement
        .query_map([id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|_| "storage_error")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "storage_error")?;
    let mut import_messages = Vec::new();
    for (ordinal, message, source, fingerprint) in entries {
        require_safe_metadata(&message, known)?;
        if let Some(source) = &source {
            require_safe_metadata(source, known)?;
        }
        require_safe_metadata(&fingerprint, known)?;
        let fingerprint = if redacted_messages.contains(&message) {
            "redacted-v1"
        } else {
            &fingerprint
        };
        import_messages.push(json!({"chat_id":portable_id,"ordinal":ordinal,"message_id":message,"source_message_id":source,"fingerprint":fingerprint}));
    }
    let mut statement = db.prepare("SELECT message_id,asset_id,name,media_type,byte_length FROM message_attachments WHERE chat_id=?1 ORDER BY rowid").map_err(|_| "storage_error")?;
    let mut rows = statement.query([id]).map_err(|_| "storage_error")?;
    let mut attachments = Vec::new();
    let mut server_attachments = Vec::new();
    while let Some(row) = rows.next().map_err(|_| "storage_error")? {
        let message: String = row.get(0).map_err(|_| "storage_error")?;
        let asset: String = row.get(1).map_err(|_| "storage_error")?;
        let name: String = row.get(2).map_err(|_| "storage_error")?;
        let media_type: String = row.get(3).map_err(|_| "storage_error")?;
        let size: i64 = row.get(4).map_err(|_| "storage_error")?;
        for item in [&message, &asset, &name, &media_type] {
            require_safe_metadata(item, known)?;
        }
        if size < 0 {
            return Err("invalid_attachment");
        }
        attachments.push(json!({"receipt_id":format!("server_{message}_{asset}"),"chat_id":portable_id,"asset_id":asset,"message_id":message}));
        server_attachments.push(json!({"messageId":message,"assetId":asset,"name":name,"mediaType":media_type,"byteLength":size}));
    }
    let mut statement = db.prepare("SELECT id,state,text,error,user_message_id,retry_of,permission_mode,provider,model,regenerate_of FROM turns WHERE chat_id=?1 ORDER BY rowid").map_err(|_| "storage_error")?;
    let mut rows = statement.query([id]).map_err(|_| "storage_error")?;
    let mut turns = Vec::new();
    while let Some(row) = rows.next().map_err(|_| "storage_error")? {
        let turn_id: String = row.get(0).map_err(|_| "storage_error")?;
        let state: String = row.get(1).map_err(|_| "storage_error")?;
        let original: String = row.get(2).map_err(|_| "storage_error")?;
        let metadata: Vec<Option<String>> = (3..10)
            .map(|index| row.get(index).map_err(|_| "storage_error"))
            .collect::<Result<_, _>>()?;
        for item in [&turn_id, &state]
            .into_iter()
            .chain(metadata.iter().flatten())
        {
            require_safe_metadata(item, known)?;
        }
        let (text, count) = clean_text(&original, known);
        redactions += count;
        let mut item = json!({"id":turn_id,"state":state,"text":text,"error":metadata[0],"userMessageId":metadata[1],"retryOf":metadata[2],"permissionMode":metadata[3],"provider":metadata[4],"model":metadata[5]});
        if let Some(parent) = &metadata[6] {
            item["regenerateOf"] = json!(parent);
        }
        turns.push(item);
    }
    let mut statement=db.prepare("SELECT turn_id,asset_id,byte_length FROM generated_output_links WHERE chat_id=?1 ORDER BY turn_id,asset_id").map_err(|_|"storage_error")?;
    let generated=statement.query_map([id],|r|Ok(json!({"turnId":r.get::<_,String>(0)?,"assetId":r.get::<_,String>(1)?,"byteLength":r.get::<_,i64>(2)?}))).map_err(|_|"storage_error")?.collect::<Result<Vec<_>,_>>().map_err(|_|"storage_error")?;
    let mut server = json!({"attachments":server_attachments,"turns":turns});
    if !generated.is_empty() {
        server["generated"] = json!(generated);
    }
    Ok(
        json!({"version":1,"room":{"id":portable_id,"title":title,"created_at":created,"updated_at":updated,"group_id":group_id,"pinned_order":pinned,"archived_at":archived},"messages":messages,"groups":groups,"imports":imports,"importMessages":import_messages,"attachments":attachments,"redactions":redactions,"server":server}),
    )
}

/// Restore only into an absent chat, or accept an identical previous restore.
/// A different existing chat is never overwritten or partially modified.
pub fn restore_chat(db: &mut Connection, snapshot: &Value) -> Result<(), &'static str> {
    restore_chats(db, std::slice::from_ref(snapshot))
}

/// Apply all copies in one transaction, so one conflicting room changes none.
pub fn restore_chats(db: &mut Connection, snapshots: &[Value]) -> Result<(), &'static str> {
    restore_bundle(db, snapshots, None)
}

pub fn restore_bundle(
    db: &mut Connection,
    snapshots: &[Value],
    groups: Option<&Value>,
) -> Result<(), &'static str> {
    let tx = db.transaction().map_err(|_| "storage_error")?;
    if let Some(groups) = groups {
        restore_groups_in_transaction(&tx, groups)?;
    }
    for snapshot in snapshots {
        restore_chat_in_transaction(&tx, snapshot)?;
    }
    tx.commit().map_err(|_| "storage_error")
}

fn restore_chat_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    snapshot: &Value,
) -> Result<(), &'static str> {
    if snapshot.get("deleted").is_some() {
        if !exact_fields(snapshot, &["version", "room", "deleted", "deletedAt"])
            || snapshot["version"] != 1
            || snapshot["deleted"] != true
            || !exact_fields(&snapshot["room"], &["id"])
        {
            return Err("invalid_snapshot");
        }
        let portable = snapshot["room"]["id"].as_str().ok_or("invalid_snapshot")?;
        let id = portable.strip_prefix("chat_").ok_or("invalid_snapshot")?;
        if uuid::Uuid::parse_str(id)
            .map_err(|_| "invalid_snapshot")?
            .to_string()
            != id
        {
            return Err("invalid_snapshot");
        }
        let deleted = snapshot["deletedAt"].as_str().ok_or("invalid_snapshot")?;
        let incoming =
            chrono::DateTime::parse_from_rfc3339(deleted).map_err(|_| "invalid_snapshot")?;
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chats WHERE id=?1)",
                [id],
                |r| r.get(0),
            )
            .map_err(|_| "storage_error")?;
        if exists {
            return Err("restore_conflict");
        }
        let prior: Option<String> = tx
            .query_row(
                "SELECT deleted_at FROM chat_deletions WHERE chat_id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "storage_error")?;
        let should_update = match prior {
            Some(value) => {
                incoming
                    > chrono::DateTime::parse_from_rfc3339(&value).map_err(|_| "storage_error")?
            }
            None => true,
        };
        if should_update {
            tx.execute("INSERT INTO chat_deletions(chat_id,deleted_at) VALUES(?1,?2) ON CONFLICT(chat_id) DO UPDATE SET deleted_at=excluded.deleted_at",params![id,deleted]).map_err(|_| "storage_error")?;
        }
        return Ok(());
    }
    if !exact_fields(
        snapshot,
        &[
            "version",
            "room",
            "messages",
            "groups",
            "imports",
            "importMessages",
            "attachments",
            "redactions",
            "server",
        ],
    ) || !snapshot.get("server").is_some_and(|value| {
        exact_fields(value, &["attachments", "turns"])
            || exact_fields(value, &["attachments", "turns", "generated"])
    }) || !snapshot.get("room").is_some_and(|value| {
        exact_fields(
            value,
            &[
                "id",
                "title",
                "created_at",
                "updated_at",
                "group_id",
                "pinned_order",
                "archived_at",
            ],
        )
    }) {
        return Err("unsupported_snapshot");
    }
    if snapshot.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("invalid_snapshot");
    }
    let room = snapshot
        .get("room")
        .and_then(Value::as_object)
        .ok_or("invalid_snapshot")?;
    let imports = snapshot
        .get("imports")
        .and_then(Value::as_array)
        .ok_or("invalid_snapshot")?;
    let import_messages = snapshot
        .get("importMessages")
        .and_then(Value::as_array)
        .ok_or("invalid_snapshot")?;
    if imports.len() > 1 || (imports.is_empty() && !import_messages.is_empty()) {
        return Err("invalid_snapshot");
    }
    let groups = snapshot["groups"].as_array().ok_or("invalid_snapshot")?;
    let group_id = match room.get("group_id") {
        Some(Value::Null) if groups.is_empty() => None,
        Some(Value::String(id))
            if groups.len() == 1
                && exact_fields(&groups[0], &["id", "name", "position", "created_at"])
                && groups[0]["id"].as_str() == Some(id) =>
        {
            let mut group = groups[0].clone();
            group["deleted_at"] = Value::Null;
            restore_groups_in_transaction(tx, &json!({"version":1,"groups":[group]}))?;
            Some(id.as_str())
        }
        _ => return Err("invalid_snapshot"),
    };
    let pinned = match room.get("pinned_order") {
        Some(Value::Null) => None,
        Some(Value::Number(value)) => Some(
            value
                .as_i64()
                .filter(|value| *value >= 0)
                .ok_or("invalid_snapshot")?,
        ),
        _ => return Err("invalid_snapshot"),
    };
    let archived = match room.get("archived_at") {
        Some(Value::Null) => None,
        Some(Value::String(value)) if chrono::DateTime::parse_from_rfc3339(value).is_ok() => {
            Some(value.as_str())
        }
        _ => return Err("invalid_snapshot"),
    };
    let portable = room
        .get("id")
        .and_then(Value::as_str)
        .ok_or("invalid_snapshot")?;
    let id = portable.strip_prefix("chat_").ok_or("invalid_snapshot")?;
    if uuid::Uuid::parse_str(id)
        .map_err(|_| "invalid_snapshot")?
        .to_string()
        != id
    {
        return Err("invalid_snapshot");
    }
    let deleted: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_deletions WHERE chat_id=?1)",
            [id],
            |r| r.get(0),
        )
        .map_err(|_| "storage_error")?;
    if deleted {
        return Err("restore_conflict");
    }
    let messages = snapshot
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("invalid_snapshot")?;
    let exists = tx
        .query_row("SELECT 1 FROM chats WHERE id=?1", [id], |_| Ok(()))
        .optional()
        .map_err(|_| "storage_error")?
        .is_some();
    if exists {
        let mut existing = export_chat(tx, id, &[])?;
        existing["redactions"] = snapshot
            .get("redactions")
            .cloned()
            .ok_or("invalid_snapshot")?;
        return if existing == *snapshot {
            Ok(())
        } else {
            Err("restore_conflict")
        };
    }
    let text = |key: &str| {
        room.get(key)
            .and_then(Value::as_str)
            .ok_or("invalid_snapshot")
    };
    tx.execute(
        "INSERT INTO chats(id,title,created_at,updated_at,thread_id,archived_at,pinned_order,group_id) VALUES(?1,?2,?3,?4,NULL,?5,?6,?7)",
        params![id, text("title")?, text("created_at")?, text("updated_at")?, archived, pinned, group_id],
    )
    .map_err(|_| "storage_error")?;
    for message in messages {
        if !exact_fields(
            message,
            &[
                "id",
                "chat_id",
                "role",
                "text",
                "created_at",
                "provider",
                "model",
                "effort",
                "skill",
                "fast",
                "not_handed_over",
                "retryable_fast",
                "actual_effort",
                "finish_reason",
                "auth_method",
                "fast_state",
            ],
        ) || ["fast", "not_handed_over", "retryable_fast"]
            .iter()
            .any(|key| message.get(*key).and_then(Value::as_i64) != Some(0))
            || ["auth_method", "fast_state"]
                .iter()
                .any(|key| !message.get(*key).is_some_and(Value::is_null))
        {
            return Err("unsupported_snapshot");
        }
        if message.get("chat_id").and_then(Value::as_str) != Some(portable) {
            return Err("invalid_snapshot");
        }
        let text = |key: &str| {
            message
                .get(key)
                .and_then(Value::as_str)
                .ok_or("invalid_snapshot")
        };
        let optional = |key: &str| -> Result<Option<&str>, &'static str> {
            match message.get(key) {
                Some(Value::Null) => Ok(None),
                Some(Value::String(v)) => Ok(Some(v)),
                _ => Err("invalid_snapshot"),
            }
        };
        tx.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at,provider,model,effort,skill,actual_effort,finish_reason) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![text("id")?,id,text("role")?,text("text")?,text("created_at")?,optional("provider")?,optional("model")?,optional("effort")?,optional("skill")?,optional("actual_effort")?,optional("finish_reason")?]).map_err(|_| "storage_error")?;
    }
    for entry in imports {
        if !exact_fields(entry, &["chat_id", "source_provider", "source_id"])
            || entry["chat_id"].as_str() != Some(portable)
        {
            return Err("invalid_snapshot");
        }
        let provider = entry["source_provider"]
            .as_str()
            .filter(|v| matches!(*v, "chatgpt" | "claude" | "gemini"))
            .ok_or("invalid_snapshot")?;
        let source = entry["source_id"]
            .as_str()
            .filter(|v| !v.is_empty() && v.len() <= 4096 && !v.chars().any(char::is_control))
            .ok_or("invalid_snapshot")?;
        tx.execute(
            "INSERT INTO chat_imports(chat_id,source_provider,source_id) VALUES(?1,?2,?3)",
            params![id, provider, source],
        )
        .map_err(|_| "restore_conflict")?;
    }
    for (index, entry) in import_messages.iter().enumerate() {
        if !exact_fields(
            entry,
            &[
                "chat_id",
                "ordinal",
                "message_id",
                "source_message_id",
                "fingerprint",
            ],
        ) || entry["chat_id"].as_str() != Some(portable)
            || entry["ordinal"].as_u64() != Some(index as u64)
        {
            return Err("invalid_snapshot");
        }
        let message = entry["message_id"].as_str().ok_or("invalid_snapshot")?;
        let source = match &entry["source_message_id"] {
            Value::Null => None,
            Value::String(s) if s.len() <= 4096 && !s.chars().any(char::is_control) => {
                Some(s.as_str())
            }
            _ => return Err("invalid_snapshot"),
        };
        let fingerprint = entry["fingerprint"]
            .as_str()
            .filter(|v| {
                *v == "redacted-v1" || (v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
            })
            .ok_or("invalid_snapshot")?;
        let belongs: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_messages WHERE id=?1 AND chat_id=?2)",
                params![message, id],
                |r| r.get(0),
            )
            .map_err(|_| "storage_error")?;
        if !belongs {
            return Err("invalid_snapshot");
        }
        tx.execute("INSERT INTO chat_import_message_state(chat_id,ordinal,message_id,source_message_id,fingerprint) VALUES(?1,?2,?3,?4,?5)",params![id,index as i64,message,source,fingerprint]).map_err(|_|"storage_error")?;
    }
    let attachments = snapshot
        .pointer("/server/attachments")
        .and_then(Value::as_array)
        .ok_or("invalid_snapshot")?;
    let portable_attachments = snapshot
        .get("attachments")
        .and_then(Value::as_array)
        .ok_or("invalid_snapshot")?;
    if portable_attachments.len() != attachments.len() {
        return Err("unsupported_snapshot");
    }
    for (item, portable_item) in attachments.iter().zip(portable_attachments) {
        if !exact_fields(
            item,
            &["messageId", "assetId", "name", "mediaType", "byteLength"],
        ) {
            return Err("unsupported_snapshot");
        }
        let text = |key: &str| {
            item.get(key)
                .and_then(Value::as_str)
                .ok_or("invalid_snapshot")
        };
        let size = item
            .get("byteLength")
            .and_then(Value::as_i64)
            .filter(|n| *n >= 0)
            .ok_or("invalid_snapshot")?;
        let message = text("messageId")?;
        if *portable_item
            != json!({"receipt_id":format!("server_{}_{}",message,text("assetId")?),"chat_id":portable,"asset_id":text("assetId")?,"message_id":message})
        {
            return Err("invalid_snapshot");
        }
        let belongs: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_messages WHERE id=?1 AND chat_id=?2)",
                params![message, id],
                |row| row.get(0),
            )
            .map_err(|_| "storage_error")?;
        if !belongs {
            return Err("invalid_snapshot");
        }
        tx.execute("INSERT INTO message_attachments(message_id,chat_id,asset_id,name,media_type,byte_length) VALUES(?1,?2,?3,?4,?5,?6)",params![message,id,text("assetId")?,text("name")?,text("mediaType")?,size]).map_err(|_| "storage_error")?;
    }
    let turns = snapshot
        .pointer("/server/turns")
        .and_then(Value::as_array)
        .ok_or("invalid_snapshot")?;
    for turn in turns {
        let mut checked = turn.clone();
        let parent = checked
            .as_object_mut()
            .ok_or("invalid_snapshot")?
            .remove("regenerateOf");
        if !exact_fields(
            &checked,
            &[
                "id",
                "state",
                "text",
                "error",
                "userMessageId",
                "retryOf",
                "permissionMode",
                "provider",
                "model",
            ],
        ) {
            return Err("unsupported_snapshot");
        }
        let regenerate = match parent {
            None | Some(Value::Null) => None,
            Some(Value::String(value))
                if uuid::Uuid::parse_str(&value).is_ok_and(|id| id.to_string() == value) =>
            {
                Some(value)
            }
            _ => return Err("invalid_snapshot"),
        };
        let text = |key: &str| {
            turn.get(key)
                .and_then(Value::as_str)
                .ok_or("invalid_snapshot")
        };
        let optional = |key: &str| -> Result<Option<&str>, &'static str> {
            match turn.get(key) {
                Some(Value::Null) => Ok(None),
                Some(Value::String(v)) => Ok(Some(v)),
                _ => Err("invalid_snapshot"),
            }
        };
        let state = text("state")?;
        if !["running", "completed", "cancelled", "failed"].contains(&state) {
            return Err("invalid_snapshot");
        }
        if let Some(message) = optional("userMessageId")? {
            let belongs: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM chat_messages WHERE id=?1 AND chat_id=?2)",
                    params![message, id],
                    |row| row.get(0),
                )
                .map_err(|_| "storage_error")?;
            if !belongs {
                return Err("invalid_snapshot");
            }
        }
        tx.execute("INSERT INTO turns(id,chat_id,state,text,error,user_message_id,retry_of,permission_mode,provider,model,regenerate_of) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", params![text("id")?,id,state,text("text")?,optional("error")?,optional("userMessageId")?,optional("retryOf")?,optional("permissionMode")?,optional("provider")?,optional("model")?,regenerate]).map_err(|_| "storage_error")?;
    }
    if let Some(generated) = snapshot.pointer("/server/generated") {
        for item in generated.as_array().ok_or("invalid_snapshot")? {
            if !exact_fields(item, &["turnId", "assetId", "byteLength"]) {
                return Err("invalid_snapshot");
            }
            let turn = item["turnId"].as_str().ok_or("invalid_snapshot")?;
            let asset = item["assetId"]
                .as_str()
                .filter(|value| {
                    uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == *value)
                })
                .ok_or("invalid_snapshot")?;
            let bytes = item["byteLength"]
                .as_i64()
                .filter(|value| *value >= 0 && *value <= 2 * 1024 * 1024 * 1024)
                .ok_or("invalid_snapshot")?;
            let belongs: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM turns WHERE id=?1 AND chat_id=?2)",
                    params![turn, id],
                    |r| r.get(0),
                )
                .map_err(|_| "storage_error")?;
            if !belongs {
                return Err("invalid_snapshot");
            }
            tx.execute("INSERT INTO generated_output_links(turn_id,chat_id,asset_id,byte_length) VALUES(?1,?2,?3,?4)",params![turn,id,asset,bytes]).map_err(|_|"storage_error")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE chat_deletions(chat_id TEXT PRIMARY KEY,deleted_at TEXT NOT NULL); CREATE TABLE chat_groups(id TEXT PRIMARY KEY,name TEXT NOT NULL,position INTEGER NOT NULL,created_at TEXT NOT NULL,deleted_at TEXT); CREATE TABLE chat_imports(chat_id TEXT PRIMARY KEY,source_provider TEXT NOT NULL,source_id TEXT NOT NULL,UNIQUE(source_provider,source_id)); CREATE TABLE chat_import_message_state(chat_id TEXT NOT NULL,ordinal INTEGER NOT NULL,message_id TEXT NOT NULL,source_message_id TEXT,fingerprint TEXT NOT NULL,PRIMARY KEY(chat_id,ordinal)); CREATE TABLE generated_output_links(turn_id TEXT NOT NULL,chat_id TEXT NOT NULL,asset_id TEXT NOT NULL,byte_length INTEGER NOT NULL,PRIMARY KEY(turn_id,asset_id));",
        )
        .unwrap();
        db.execute_batch("CREATE TABLE chats(id TEXT PRIMARY KEY,title TEXT NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,thread_id TEXT,archived_at TEXT,pinned_order INTEGER,group_id TEXT); CREATE TABLE chat_messages(id TEXT PRIMARY KEY,chat_id TEXT NOT NULL,role TEXT NOT NULL,text TEXT NOT NULL,created_at TEXT NOT NULL,provider TEXT,model TEXT,effort TEXT,skill TEXT,actual_effort TEXT,finish_reason TEXT); CREATE TABLE message_attachments(message_id TEXT NOT NULL,chat_id TEXT NOT NULL,asset_id TEXT NOT NULL,name TEXT NOT NULL,media_type TEXT NOT NULL,byte_length INTEGER NOT NULL,PRIMARY KEY(message_id,asset_id)); CREATE TABLE turns(id TEXT PRIMARY KEY,chat_id TEXT NOT NULL,state TEXT NOT NULL,text TEXT NOT NULL,error TEXT,user_message_id TEXT,retry_of TEXT,permission_mode TEXT,provider TEXT,model TEXT,regenerate_of TEXT);").unwrap();
        db
    }

    #[test]
    fn imported_message_provenance_survives_backup_without_secret_fingerprint() {
        let source = database();
        let id = "527f64c5-74cf-4b21-ac6f-2d205f74c100";
        source.execute("INSERT INTO chats VALUES(?1,'fixture','2026-01-01','2026-01-02',NULL,NULL,NULL,NULL)",[id]).unwrap();
        source.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('message-1',?1,'user','private-key-value','2026-01-01')",[id]).unwrap();
        source
            .execute(
                "INSERT INTO chat_imports VALUES(?1,'claude','source-id')",
                [id],
            )
            .unwrap();
        source.execute("INSERT INTO chat_import_message_state VALUES(?1,0,'message-1','external-message',?2)",params![id,"a".repeat(64)]).unwrap();
        let snapshot = export_chat(&source, id, &["private-key-value"]).unwrap();
        assert_eq!(snapshot["importMessages"][0]["fingerprint"], "redacted-v1");
        assert!(!snapshot.to_string().contains("private-key-value"));
        let mut target = database();
        restore_chat(&mut target, &snapshot).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("restored.sqlite3");
        target
            .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        let export = dir.path().join("export.json");
        std::fs::write(&export,json!([{"uuid":"source-id","name":"fixture","chat_messages":[{"uuid":"external-message","sender":"human","text":"private-key-value"}]}]).to_string()).unwrap();
        let counts = crate::chat_export_import::import(&path, &export, "claude").unwrap();
        assert_eq!(counts.duplicate, 1);
        assert_eq!(
            target
                .query_row("SELECT source_id FROM chat_imports", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "source-id"
        );
        assert_eq!(
            target
                .query_row(
                    "SELECT fingerprint FROM chat_import_message_state",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "redacted-v1"
        );
        restore_chat(&mut target, &snapshot).unwrap();
    }

    #[test]
    fn groups_and_empty_groups_survive_backup_restore_without_overwriting_conflicts() {
        let mut source = database();
        let chat = "527f64c5-74cf-4b21-ac6f-2d205f74c100";
        let group = "65e2dc8c-1515-4bf0-bc6b-f5436aa5ad00";
        let empty = "65e2dc8c-1515-4bf0-bc6b-f5436aa5ad01";
        let deleted = "65e2dc8c-1515-4bf0-bc6b-f5436aa5ad02";
        for (id, name, position, deleted_at) in [
            (group, "classes", 0, None),
            (empty, "empty", 1, None),
            (deleted, "", 2, Some("2026-11-22T12:00:00+00:00")),
        ] {
            source
                .execute(
                    "INSERT INTO chat_groups VALUES(?1,?2,?3,'2026-11-21T12:00:00+00:00',?4)",
                    params![id, name, position, deleted_at],
                )
                .unwrap();
        }
        source.execute("INSERT INTO chats VALUES(?1,'fixture','2026-01-01','2026-01-02',NULL,NULL,NULL,?2)",params![chat,group]).unwrap();
        let snapshot = export_chat(&source, chat, &[]).unwrap();
        let groups = export_groups(&source, &[]).unwrap();
        let mut target = database();
        restore_bundle(&mut target, std::slice::from_ref(&snapshot), Some(&groups)).unwrap();
        restore_bundle(&mut target, std::slice::from_ref(&snapshot), Some(&groups)).unwrap();
        let actual: String = target
            .query_row("SELECT group_id FROM chats WHERE id=?1", [chat], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(actual, group);
        let count: i64 = target
            .query_row(
                "SELECT count(*) FROM chat_groups WHERE deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
        let deleted_exists: bool = target
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_groups WHERE id=?1 AND deleted_at IS NOT NULL)",
                [deleted],
                |r| r.get(0),
            )
            .unwrap();
        assert!(deleted_exists);
        target
            .execute("UPDATE chat_groups SET name='local' WHERE id=?1", [empty])
            .unwrap();
        assert_eq!(
            restore_bundle(&mut target, &[], Some(&groups)),
            Err("restore_conflict")
        );
        let name: String = target
            .query_row("SELECT name FROM chat_groups WHERE id=?1", [empty], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(name, "local");
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("archive");
        let data = temp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        momso_core::filesystem::initialize(&archive).unwrap();
        stage_chat_copies(&mut source, &archive, &data, &[]).unwrap();
        let saved: Value =
            serde_json::from_slice(&std::fs::read(archive.join("chat-groups.json")).unwrap())
                .unwrap();
        assert_eq!(saved, groups);
    }

    #[test]
    fn deletion_marker_prevents_old_chat_resurrection_without_overwriting_live_chat() {
        let source = database();
        let id = "527f64c5-74cf-4b21-ac6f-2d205f74c100";
        source
            .execute(
                "INSERT INTO chats VALUES(?1,'fixture','2026-01-01','2026-01-02',NULL,NULL,NULL,NULL)",
                [id],
            )
            .unwrap();
        let live = export_chat(&source, id, &[]).unwrap();
        let marker = json!({"version":1,"room":{"id":format!("chat_{id}")},"deleted":true,"deletedAt":"2026-11-22T12:00:00+00:00"});
        let mut target = database();
        restore_chat(&mut target, &live).unwrap();
        assert_eq!(restore_chat(&mut target, &marker), Err("restore_conflict"));
        let count: i64 = target
            .query_row("SELECT count(*) FROM chats", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        target
            .execute("DELETE FROM chats WHERE id=?1", [id])
            .unwrap();
        restore_chat(&mut target, &marker).unwrap();
        restore_chat(&mut target, &marker).unwrap();
        assert_eq!(restore_chat(&mut target, &live), Err("restore_conflict"));
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("archive");
        let data = directory.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        momso_core::filesystem::initialize(&archive).unwrap();
        stage_chat_copies(&mut target, &archive, &data, &[]).unwrap();
        let stored = std::fs::read(archive.join("chats").join(format!("chat_{id}.json"))).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&stored).unwrap(), marker);
    }

    #[test]
    fn archived_chat_survives_backup_and_restore() {
        let source = database();
        let id = "527f64c5-74cf-4b21-ac6f-2d205f74c100";
        source.execute("INSERT INTO chats VALUES(?1,'fixture','2026-01-01','2026-01-02',NULL,'2026-11-22T12:00:00+00:00',2,NULL)", [id]).unwrap();
        let snapshot = export_chat(&source, id, &[]).unwrap();
        assert_eq!(snapshot["room"]["archived_at"], "2026-11-22T12:00:00+00:00");
        assert_eq!(snapshot["room"]["pinned_order"], 2);
        let mut restored = database();
        restore_chat(&mut restored, &snapshot).unwrap();
        restore_chat(&mut restored, &snapshot).unwrap();
        let archived: String = restored
            .query_row("SELECT archived_at FROM chats WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(archived, "2026-11-22T12:00:00+00:00");
        assert_eq!(
            restored
                .query_row("SELECT pinned_order FROM chats WHERE id=?1", [id], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            2
        );
    }

    #[test]
    fn redacted_copy_restores_once_and_preserves_existing_conflict() {
        let source = database();
        let id = "527f64c5-74cf-4b21-ac6f-2d205f74c100";
        source
            .execute(
                "INSERT INTO chats VALUES(?1,'fixture secret-one','2026-01-01','2026-01-02',NULL,NULL,NULL,NULL)",
                [id],
            )
            .unwrap();
        source.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('m1',?1,'user','hello secret-one','2026-01-01')",[id]).unwrap();
        source.execute("INSERT INTO message_attachments VALUES('m1',?1,'asset-1','fixture.txt','text/plain',3)",[id]).unwrap();
        source.execute("INSERT INTO turns(id,chat_id,state,text,user_message_id) VALUES('turn-1',?1,'failed','partial secret-one','m1')",[id]).unwrap();
        let snapshot = export_chat(&source, id, &["secret-one"]).unwrap();
        assert!(!snapshot.to_string().contains("secret-one"));
        let mut bad = snapshot.clone();
        bad["server"]["attachments"][0]["messageId"] = Value::String("not-the-message".into());
        let mut untouched = database();
        assert_eq!(restore_chat(&mut untouched, &bad), Err("invalid_snapshot"));
        let count: i64 = untouched
            .query_row("SELECT count(*) FROM chats", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
        let original: String = source
            .query_row("SELECT text FROM chat_messages WHERE id='m1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(original, "hello secret-one");
        let mut restored = database();
        restore_chat(&mut restored, &snapshot).unwrap();
        restore_chat(&mut restored, &snapshot).unwrap();
        let count: i64 = restored
            .query_row("SELECT count(*) FROM message_attachments", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
        let turn: String = restored
            .query_row("SELECT text FROM turns WHERE id='turn-1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(!turn.contains("secret-one"));
        restored
            .execute(
                "UPDATE chat_messages SET text='local edit' WHERE id='m1'",
                [],
            )
            .unwrap();
        assert_eq!(
            restore_chat(&mut restored, &snapshot),
            Err("restore_conflict")
        );
        let kept: String = restored
            .query_row("SELECT text FROM chat_messages WHERE id='m1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(kept, "local edit");
        let next_id = "65e2dc8c-1515-4bf0-bc6b-f5436aa5ad00";
        source
            .execute(
                "INSERT INTO chats VALUES(?1,'second','2026-01-01','2026-01-02',NULL,NULL,NULL,NULL)",
                [next_id],
            )
            .unwrap();
        let second = export_chat(&source, next_id, &[]).unwrap();
        assert_eq!(
            restore_chats(&mut restored, &[second, snapshot]),
            Err("restore_conflict")
        );
        let count: i64 = restored
            .query_row("SELECT count(*) FROM chats", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn chat_copy_journal_recovers_and_never_overwrites_edits() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("archive");
        let data = temp.path().join("data");
        std::fs::create_dir(&data).unwrap();
        momso_core::filesystem::initialize(&archive).unwrap();
        let mut db = database();
        let id = "527f64c5-74cf-4b21-ac6f-2d205f74c100";
        db.execute(
            "INSERT INTO chats VALUES(?1,'fixture','2026-01-01','2026-01-02',NULL,NULL,NULL,NULL)",
            [id],
        )
        .unwrap();
        db.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('m1',?1,'user','first','2026-01-01')",[id]).unwrap();
        assert_eq!(stage_chat_copies(&mut db, &archive, &data, &[]), Ok(1));
        let path = archive.join("chats").join(format!("chat_{id}.json"));
        let state_path = data.join("chat-export-journal.json");
        let old = std::fs::read(&path).unwrap();
        db.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('m2',?1,'assistant','second','2026-01-02')",[id]).unwrap();
        let next = serde_json::to_vec(&export_chat(&db, id, &[]).unwrap()).unwrap();
        let mut journal: Journal =
            serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
        journal.entries.get_mut(id).unwrap().pending = Some(digest(&next));
        save_journal(&state_path, &journal).unwrap();
        std::fs::write(&path, &next).unwrap();
        assert_eq!(stage_chat_copies(&mut db, &archive, &data, &[]), Ok(1));
        assert_ne!(old, next);
        std::fs::write(&path, "person edit").unwrap();
        assert_eq!(
            stage_chat_copies(&mut db, &archive, &data, &[]),
            Err("chat_backup_conflict")
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "person edit");
    }
}
