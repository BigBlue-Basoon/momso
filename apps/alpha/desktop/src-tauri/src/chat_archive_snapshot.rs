//! Read-only export prototype. It is not connected to Git or restoration yet.
use rusqlite::{
    types::{Value as SqlValue, ValueRef},
    Connection, OpenFlags, OptionalExtension, Transaction,
};
use serde_json::{Map, Value};
use std::path::Path;

fn has_known_secret(value: &Value) -> bool {
    match value {
        Value::String(s) => crate::chat_archive_redact::redact(s).count != 0,
        Value::Array(items) => items.iter().any(has_known_secret),
        Value::Object(items) => items.iter().any(|(key, value)| {
            crate::chat_archive_redact::redact(key).count != 0 || has_known_secret(value)
        }),
        _ => false,
    }
}

pub(crate) fn contains_stored_secret(value: &Value, known: &[&str]) -> bool {
    match value {
        Value::String(s) => known
            .iter()
            .any(|secret| !secret.is_empty() && s.contains(secret)),
        Value::Array(items) => items.iter().any(|item| contains_stored_secret(item, known)),
        Value::Object(items) => items.iter().any(|(key, item)| {
            known
                .iter()
                .any(|secret| !secret.is_empty() && key.contains(secret))
                || contains_stored_secret(item, known)
        }),
        _ => false,
    }
}

/// Re-redact a historical copy without changing identity metadata or local originals.
pub(crate) fn redact_historical_snapshot(
    value: &Value,
    known: &[&str],
) -> Result<Value, &'static str> {
    if value.get("version").and_then(Value::as_u64) != Some(1)
        || value.pointer("/room/id").and_then(Value::as_str).is_none()
    {
        return Err("invalid_snapshot");
    }
    let mut result = value.clone();
    if result.get("deleted").and_then(Value::as_bool) == Some(true) {
        return if contains_stored_secret(&result, known) || has_known_secret(&result) {
            Err("sensitive_metadata")
        } else {
            Ok(result)
        };
    }
    let mut count = 0_u64;
    let mut masked_ids = Vec::new();
    let mut redact_field = |object: &mut Value, name: &str| {
        if let Some(field) = object.get_mut(name) {
            if let Some(text) = field.as_str() {
                let redacted = crate::chat_archive_redact::redact_with_known(text, known);
                count += redacted.count as u64;
                *field = Value::String(redacted.text);
                return redacted.count > 0;
            }
        }
        false
    };
    redact_field(result.get_mut("room").ok_or("invalid_snapshot")?, "title");
    for message in result
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .ok_or("invalid_snapshot")?
    {
        if redact_field(message, "text") {
            masked_ids.push(message.get("id").cloned().ok_or("invalid_snapshot")?);
        }
    }
    for group in result
        .get_mut("groups")
        .and_then(Value::as_array_mut)
        .ok_or("invalid_snapshot")?
    {
        redact_field(group, "name");
    }
    for entry in result
        .get_mut("importMessages")
        .and_then(Value::as_array_mut)
        .ok_or("invalid_snapshot")?
    {
        if entry
            .get("message_id")
            .is_some_and(|id| masked_ids.contains(id))
        {
            entry["fingerprint"] = Value::String("redacted-v1".into());
        }
    }
    let previous = result
        .get("redactions")
        .and_then(Value::as_u64)
        .ok_or("invalid_snapshot")?;
    result["redactions"] = Value::from(previous.saturating_add(count));
    if contains_stored_secret(&result, known) || has_known_secret(&result) {
        return Err("sensitive_metadata");
    }
    Ok(result)
}

fn merge_rows(
    left: &Value,
    right: &Value,
    field: &str,
    key: &str,
) -> Result<Vec<Value>, &'static str> {
    let mut result = std::collections::BTreeMap::new();
    for source in [left, right] {
        for row in source
            .get(field)
            .and_then(Value::as_array)
            .ok_or("invalid_snapshot")?
        {
            let row_key = row.get(key).ok_or("invalid_snapshot")?.to_string();
            if let Some(previous) = result.insert(row_key, row.clone()) {
                if previous != *row {
                    return Err("merge_conflict");
                }
            }
        }
    }
    Ok(result.into_values().collect())
}

fn message_from_value(value: &Value) -> Result<crate::chat_archive_merge::Message, &'static str> {
    let mut object = value.as_object().ok_or("invalid_snapshot")?.clone();
    if let Some(provider) = object.get_mut("provider") {
        *provider = crate::chat::canonical_provider(provider.as_str())
            .map_or(Value::Null, |s| Value::String(s.to_owned()));
    }
    let mut take = |name: &str| {
        object
            .remove(name)
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or("invalid_snapshot")
    };
    let (id, role, text, created_at) = (
        take("id")?,
        take("role")?,
        take("text")?,
        take("created_at")?,
    );
    Ok(crate::chat_archive_merge::Message {
        id,
        role,
        text,
        created_at,
        metadata: object.into_iter().collect(),
    })
}

fn message_to_value(message: crate::chat_archive_merge::Message) -> Value {
    let mut map: Map<String, Value> = message.metadata.into_iter().collect();
    map.insert("id".into(), Value::String(message.id));
    map.insert("role".into(), Value::String(message.role));
    map.insert("text".into(), Value::String(message.text));
    map.insert("created_at".into(), Value::String(message.created_at));
    Value::Object(map)
}

/// Prototype only: a metadata conflict is reported instead of silently choosing
/// either device's title, project, pin or archive state.
pub(crate) fn merge_snapshots(left: &Value, right: &Value) -> Result<Value, &'static str> {
    if left.get("version").and_then(Value::as_u64) != Some(1)
        || right.get("version").and_then(Value::as_u64) != Some(1)
        || has_known_secret(left)
        || has_known_secret(right)
    {
        return Err("invalid_snapshot");
    }
    let mut room = left
        .get("room")
        .and_then(Value::as_object)
        .ok_or("invalid_snapshot")?
        .clone();
    let mut other = right
        .get("room")
        .and_then(Value::as_object)
        .ok_or("invalid_snapshot")?
        .clone();
    let updated = room.remove("updated_at").ok_or("invalid_snapshot")?;
    let other_updated = other.remove("updated_at").ok_or("invalid_snapshot")?;
    if room != other {
        return Err("merge_conflict");
    }
    let left_time = updated.as_str().ok_or("invalid_snapshot")?;
    let right_time = other_updated.as_str().ok_or("invalid_snapshot")?;
    let latest = if left_time == right_time {
        left_time
    } else {
        let a = chrono::DateTime::parse_from_rfc3339(left_time).map_err(|_| "merge_conflict")?;
        let b = chrono::DateTime::parse_from_rfc3339(right_time).map_err(|_| "merge_conflict")?;
        if a >= b {
            left_time
        } else {
            right_time
        }
    };
    room.insert("updated_at".into(), Value::String(latest.to_owned()));
    let parse_messages = |source: &Value| -> Result<Vec<_>, &'static str> {
        source
            .get("messages")
            .and_then(Value::as_array)
            .ok_or("invalid_snapshot")?
            .iter()
            .map(message_from_value)
            .collect()
    };
    let messages =
        crate::chat_archive_merge::merge_messages(&parse_messages(left)?, &parse_messages(right)?)
            .map_err(|_| "merge_conflict")?
            .into_iter()
            .map(message_to_value)
            .collect::<Vec<_>>();
    let groups = merge_rows(left, right, "groups", "id")?;
    let imports = merge_rows(left, right, "imports", "chat_id")?;
    let import_messages = merge_rows(left, right, "importMessages", "ordinal")?;
    let attachments = merge_rows(left, right, "attachments", "receipt_id")?;
    let mut result = serde_json::json!({"version":1,"room":room,"messages":messages,"groups":groups,"imports":imports,"importMessages":import_messages,"attachments":attachments,"redactions":0});
    fn count_masks(value: &Value) -> usize {
        match value {
            Value::String(s) => s.matches("[비밀값으로 보여 가림:").count(),
            Value::Array(a) => a.iter().map(count_masks).sum(),
            Value::Object(o) => o.values().map(count_masks).sum(),
            _ => 0,
        }
    }
    result["redactions"] = Value::from(count_masks(&result));
    Ok(result)
}

fn rows(db: &Connection, sql: &str, id: &str) -> Result<Vec<Value>, &'static str> {
    let mut statement = db.prepare(sql).map_err(|_| "storage_error")?;
    let names: Vec<String> = statement
        .column_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mapped = statement
        .query_map([id], |row| {
            let mut out = Map::new();
            for (index, name) in names.iter().enumerate() {
                let value = match row.get_ref(index)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(n) => Value::from(n),
                    ValueRef::Text(text) => Value::String(
                        std::str::from_utf8(text)
                            .map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    index,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?
                            .to_owned(),
                    ),
                    _ => return Err(rusqlite::Error::InvalidQuery),
                };
                out.insert(name.clone(), value);
            }
            Ok(Value::Object(out))
        })
        .map_err(|_| "storage_error")?;
    mapped
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "storage_error")
}

/// Export only explicitly selected chat fields. Local authentication and binary
/// objects are intentionally not queried; the returned copy is never written here.
#[cfg(test)]
pub(crate) fn snapshot(path: &Path, id: &str) -> Result<Value, &'static str> {
    snapshot_with_known(path, id, &[])
}

pub(crate) fn snapshot_with_known(
    path: &Path,
    id: &str,
    known: &[&str],
) -> Result<Value, &'static str> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "storage_error")?;
    db.execute_batch("BEGIN").map_err(|_| "storage_error")?;
    let result = snapshot_connection_with_known(&db, id, known)?;
    db.execute_batch("COMMIT").map_err(|_| "storage_error")?;
    Ok(result)
}

#[cfg(test)]
fn snapshot_connection(db: &Connection, id: &str) -> Result<Value, &'static str> {
    snapshot_connection_with_known(db, id, &[])
}

pub(crate) fn snapshot_connection_with_known(
    db: &Connection,
    id: &str,
    known: &[&str],
) -> Result<Value, &'static str> {
    let mut room = rows(db, "SELECT id,title,created_at,updated_at,group_id,pinned_order,archived_at FROM chats WHERE id=?1", id)?;
    if room.len() != 1 {
        return Err("not_found");
    }
    let mut messages = rows(db, "SELECT id,chat_id,role,text,created_at,provider,model,effort,skill,fast,not_handed_over,retryable_fast,actual_effort,finish_reason,auth_method,fast_state FROM chat_messages WHERE chat_id=?1 ORDER BY rowid", id)?;
    for message in &mut messages {
        if let Some(provider) = message.get_mut("provider") {
            *provider = crate::chat::canonical_provider(provider.as_str())
                .map_or(Value::Null, |s| Value::String(s.to_owned()));
        }
    }
    let mut groups = rows(db, "SELECT id,name,position,created_at FROM chat_groups WHERE id=(SELECT group_id FROM chats WHERE id=?1)", id)?;
    let imports = rows(
        db,
        "SELECT chat_id,source_provider,source_id FROM chat_imports WHERE chat_id=?1",
        id,
    )?;
    let has_import_state: bool = db
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='chat_import_message_state'",
            [],
            |_| Ok(true),
        )
        .optional()
        .map_err(|_| "storage_error")?
        .unwrap_or(false);
    let mut import_messages = if has_import_state {
        rows(db, "SELECT chat_id,ordinal,message_id,source_message_id,fingerprint FROM chat_import_message_state WHERE chat_id=?1 ORDER BY ordinal", id)?
    } else {
        vec![]
    };
    let mut attachments = rows(
        db,
        "SELECT receipt_id,chat_id,asset_id,message_id FROM chat_attachments WHERE chat_id=?1",
        id,
    )?;
    let has_pending: bool = db.query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name='chat_archive_pending_attachment'", [], |_| Ok(true)).optional().map_err(|_| "storage_error")?.unwrap_or(false);
    if has_pending {
        attachments.extend(rows(db, "SELECT receipt_id,chat_id,asset_id,message_id FROM chat_archive_pending_attachment WHERE chat_id=?1", id)?);
    }
    // Never serialize an original-text fingerprint for a redacted message.
    if !import_messages.is_empty() {
        for message in &messages {
            let text = message.get("text").and_then(Value::as_str).unwrap_or("");
            if crate::chat_archive_redact::redact_with_known(text, known).count > 0 {
                for entry in &mut import_messages {
                    if entry.get("message_id") == message.get("id") {
                        entry["fingerprint"] = Value::String("redacted-v1".to_owned());
                    }
                }
            }
        }
    }
    let mut redactions = 0;
    let mut preserved_message_ids = Vec::new();
    for (collection, field, kind) in [
        (&mut room, "title", "room"),
        (&mut messages, "text", "message"),
        (&mut groups, "name", "group"),
    ] {
        for item in collection {
            let field_id = item
                .get("id")
                .and_then(Value::as_str)
                .ok_or("storage_error")?
                .to_owned();
            if let Some(value) = item.get_mut(field) {
                if let Some(text) = value.as_str() {
                    let preserved: Option<(String, String, i64)> = db.query_row(
                        "SELECT source_hash,safe_text,redaction_count FROM chat_redaction_preserved WHERE chat_id=?1 AND field_kind=?2 AND field_id=?3",
                        rusqlite::params![id,kind,field_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))
                    ).optional().map_err(|_| "storage_error")?;
                    let source = if let Some((_, safe, count)) = preserved.filter(|(hash, _, _)| {
                        crate::chat_archive_tracking::text_fingerprint(text) == *hash
                    }) {
                        if kind == "message" {
                            preserved_message_ids.push(field_id);
                        }
                        redactions += usize::try_from(count).map_err(|_| "storage_error")?;
                        safe
                    } else {
                        text.to_owned()
                    };
                    let redacted = crate::chat_archive_redact::redact_with_known(&source, known);
                    redactions += redacted.count;
                    *value = Value::String(redacted.text);
                }
            }
        }
    }
    for entry in &mut import_messages {
        if entry
            .get("message_id")
            .and_then(Value::as_str)
            .is_some_and(|id| preserved_message_ids.iter().any(|saved| saved == id))
        {
            entry["fingerprint"] = Value::String("redacted-v1".to_owned());
        }
    }
    let result = serde_json::json!({"version":1,"room":room.remove(0),"messages":messages,"groups":groups,"imports":imports,"importMessages":import_messages,"attachments":attachments,"redactions":redactions});
    if has_known_secret(&result) || contains_stored_secret(&result, known) {
        return Err("sensitive_metadata");
    }
    Ok(result)
}

/// Save only already-masked text and a portable digest of its local source.
pub(crate) fn preserve_masked_fields(
    path: &Path,
    id: &str,
    data: &Value,
    known: &[&str],
) -> Result<(), &'static str> {
    let mut db = crate::storage::open_writable_connection(path).map_err(|_| "storage_error")?;
    let tx = db.transaction().map_err(|_| "storage_error")?;
    for (collection, field, kind, sql) in [
        (
            "room",
            "title",
            "room",
            "SELECT title FROM chats WHERE id=?1",
        ),
        (
            "messages",
            "text",
            "message",
            "SELECT text FROM chat_messages WHERE id=?1",
        ),
        (
            "groups",
            "name",
            "group",
            "SELECT name FROM chat_groups WHERE id=?1",
        ),
    ] {
        let items: Vec<&Value> = if collection == "room" {
            vec![data.get(collection).ok_or("invalid_snapshot")?]
        } else {
            data.get(collection)
                .and_then(Value::as_array)
                .ok_or("invalid_snapshot")?
                .iter()
                .collect()
        };
        for item in items {
            let field_id = item
                .get("id")
                .and_then(Value::as_str)
                .ok_or("invalid_snapshot")?;
            let safe = match item.get(field).and_then(Value::as_str) {
                Some(value) => value,
                None => continue,
            };
            let original: String = tx
                .query_row(sql, [field_id], |row| row.get(0))
                .map_err(|_| "storage_error")?;
            if original != safe {
                let mac = crate::chat_archive_tracking::text_fingerprint(&original);
                let old: Option<(String,String,i64)> = tx.query_row("SELECT source_hash,safe_text,redaction_count FROM chat_redaction_preserved WHERE chat_id=?1 AND field_kind=?2 AND field_id=?3", rusqlite::params![id,kind,field_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(|_| "storage_error")?;
                let count = match old {
                    Some((hash, previous, count)) if hash == mac => {
                        usize::try_from(count).map_err(|_| "storage_error")?
                            + crate::chat_archive_redact::redact_with_known(&previous, known).count
                    }
                    _ => crate::chat_archive_redact::redact_with_known(&original, known).count,
                };
                let count = i64::try_from(count).map_err(|_| "storage_error")?;
                tx.execute("INSERT INTO chat_redaction_preserved(chat_id,field_kind,field_id,source_hash,safe_text,redaction_count) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(chat_id,field_kind,field_id) DO UPDATE SET source_hash=excluded.source_hash,safe_text=excluded.safe_text,redaction_count=excluded.redaction_count", rusqlite::params![id,kind,field_id,mac,safe,count]).map_err(|_| "storage_error")?;
            } else {
                tx.execute("DELETE FROM chat_redaction_preserved WHERE chat_id=?1 AND field_kind=?2 AND field_id=?3", rusqlite::params![id,kind,field_id]).map_err(|_| "storage_error")?;
            }
        }
    }
    tx.commit().map_err(|_| "storage_error")
}

fn insert_rows(
    tx: &Transaction<'_>,
    data: &Value,
    key: &str,
    table: &str,
    columns: &[&str],
) -> Result<(), &'static str> {
    let entries = data
        .get(key)
        .and_then(Value::as_array)
        .ok_or("invalid_snapshot")?;
    let sql = format!(
        "INSERT INTO {table} ({}) VALUES ({})",
        columns.join(","),
        vec!["?"; columns.len()].join(",")
    );
    let mut statement = tx.prepare(&sql).map_err(|_| "storage_error")?;
    for entry in entries {
        let object = entry.as_object().ok_or("invalid_snapshot")?;
        if object.len() != columns.len() {
            return Err("invalid_snapshot");
        }
        let mut values = Vec::new();
        for column in columns {
            values.push(match object.get(*column).ok_or("invalid_snapshot")? {
                Value::Null => SqlValue::Null,
                Value::String(text) => SqlValue::Text(text.clone()),
                Value::Number(number) => {
                    SqlValue::Integer(number.as_i64().ok_or("invalid_snapshot")?)
                }
                _ => return Err("invalid_snapshot"),
            });
        }
        statement
            .execute(rusqlite::params_from_iter(values))
            .map_err(|_| "restore_conflict")?;
    }
    Ok(())
}

/// Prototype first-restore path. Existing rows cause a transaction-wide error.
#[cfg(test)]
pub(crate) fn restore_new(path: &Path, data: &Value) -> Result<(), &'static str> {
    let mut db = Connection::open(path).map_err(|_| "storage_error")?;
    db.execute_batch("PRAGMA foreign_keys=ON")
        .map_err(|_| "storage_error")?;
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| "storage_error")?;
    restore_new_in_transaction(&tx, data)?;
    tx.commit().map_err(|_| "storage_error")
}

fn restore_new_in_transaction(tx: &Transaction<'_>, data: &Value) -> Result<(), &'static str> {
    if data.get("version").and_then(Value::as_u64) != Some(1) || has_known_secret(data) {
        return Err("invalid_snapshot");
    }
    let room = data
        .get("room")
        .and_then(Value::as_object)
        .ok_or("invalid_snapshot")?;
    let id = room
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or("invalid_snapshot")?;
    if !crate::archive_receipt::valid_chat_id(id) {
        return Err("invalid_snapshot");
    }
    let groups = data
        .get("groups")
        .and_then(Value::as_array)
        .ok_or("invalid_snapshot")?;
    match room.get("group_id").ok_or("invalid_snapshot")? {
        Value::Null if groups.is_empty() => {}
        Value::String(group)
            if groups.len() == 1
                && groups[0].get("id").and_then(Value::as_str) == Some(group.as_str()) => {}
        _ => return Err("invalid_snapshot"),
    }
    let room_array = Value::Array(vec![Value::Object(room.clone())]);
    let mut input = data.as_object().ok_or("invalid_snapshot")?.clone();
    input.insert("rooms".into(), room_array);
    let input = Value::Object(input);
    for key in ["messages", "imports", "importMessages", "attachments"] {
        for row in input
            .get(key)
            .and_then(Value::as_array)
            .ok_or("invalid_snapshot")?
        {
            if row.get("chat_id").and_then(Value::as_str) != Some(id) {
                return Err("invalid_snapshot");
            }
        }
    }
    let deleted: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_deletions WHERE chat_id=?1)",
            [id],
            |row| row.get(0),
        )
        .map_err(|_| "storage_error")?;
    if deleted {
        return Ok(());
    }
    tx.execute_batch("CREATE TABLE IF NOT EXISTS chat_import_message_state (chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE, ordinal INTEGER NOT NULL, message_id TEXT NOT NULL REFERENCES chat_messages(id) ON DELETE CASCADE, source_message_id TEXT, fingerprint TEXT NOT NULL, PRIMARY KEY(chat_id,ordinal))").map_err(|_| "storage_error")?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS chat_archive_pending_attachment (receipt_id TEXT PRIMARY KEY NOT NULL, chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE, asset_id TEXT NOT NULL, message_id TEXT REFERENCES chat_messages(id) ON DELETE CASCADE)").map_err(|_| "storage_error")?;
    for group in groups {
        let group_id = group
            .get("id")
            .and_then(Value::as_str)
            .ok_or("invalid_snapshot")?;
        let present = rows(
            tx,
            "SELECT id,name,position,created_at FROM chat_groups WHERE id=?1",
            group_id,
        )?;
        if present.is_empty() {
            let single = serde_json::json!({"groups":[group]});
            insert_rows(
                tx,
                &single,
                "groups",
                "chat_groups",
                &["id", "name", "position", "created_at"],
            )?;
        } else if present.len() != 1 || present[0] != *group {
            return Err("restore_conflict");
        }
    }
    insert_rows(
        tx,
        &input,
        "rooms",
        "chats",
        &[
            "id",
            "title",
            "created_at",
            "updated_at",
            "group_id",
            "pinned_order",
            "archived_at",
        ],
    )?;
    insert_rows(
        tx,
        &input,
        "messages",
        "chat_messages",
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
    )?;
    insert_rows(
        tx,
        &input,
        "imports",
        "chat_imports",
        &["chat_id", "source_provider", "source_id"],
    )?;
    insert_import_messages(tx, &input, id)?;
    insert_attachments(tx, &input, id)?;
    Ok(())
}

fn insert_import_messages(
    tx: &Transaction<'_>,
    input: &Value,
    id: &str,
) -> Result<(), &'static str> {
    for entry in input["importMessages"]
        .as_array()
        .ok_or("invalid_snapshot")?
    {
        let message = entry
            .get("message_id")
            .and_then(Value::as_str)
            .ok_or("invalid_snapshot")?;
        let belongs: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_messages WHERE id=?1 AND chat_id=?2)",
                [message, id],
                |row| row.get(0),
            )
            .map_err(|_| "storage_error")?;
        if !belongs {
            return Err("invalid_snapshot");
        }
    }
    insert_rows(
        tx,
        input,
        "importMessages",
        "chat_import_message_state",
        &[
            "chat_id",
            "ordinal",
            "message_id",
            "source_message_id",
            "fingerprint",
        ],
    )
}

fn insert_attachments(tx: &Transaction<'_>, input: &Value, id: &str) -> Result<(), &'static str> {
    for attachment in input["attachments"].as_array().ok_or("invalid_snapshot")? {
        let asset = attachment
            .get("asset_id")
            .and_then(Value::as_str)
            .ok_or("invalid_snapshot")?;
        if let Some(message) = attachment.get("message_id").and_then(Value::as_str) {
            let belongs: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM chat_messages WHERE id=?1 AND chat_id=?2)",
                    [message, id],
                    |row| row.get(0),
                )
                .map_err(|_| "storage_error")?;
            if !belongs {
                return Err("invalid_snapshot");
            }
        }
        let present: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM momso_source_assets WHERE id=?1)",
                [asset],
                |row| row.get(0),
            )
            .map_err(|_| "storage_error")?;
        let single = serde_json::json!({"attachments":[attachment]});
        insert_rows(
            tx,
            &single,
            "attachments",
            if present {
                "chat_attachments"
            } else {
                "chat_archive_pending_attachment"
            },
            &["receipt_id", "chat_id", "asset_id", "message_id"],
        )?;
    }
    Ok(())
}

#[cfg(test)]
fn check_message_dependents(tx: &Transaction<'_>) -> Result<(), &'static str> {
    let mut query = tx
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .map_err(|_| "storage_error")?;
    let tables = query
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| "storage_error")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "storage_error")?;
    drop(query);
    for table in tables {
        let mut query = tx
            .prepare("SELECT \"table\" FROM pragma_foreign_key_list(?1)")
            .map_err(|_| "storage_error")?;
        let targets = query
            .query_map([&table], |row| row.get::<_, String>(0))
            .map_err(|_| "storage_error")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "storage_error")?;
        if targets.iter().any(|target| target == "chat_messages")
            && ![
                "chat_attachments",
                "chat_import_message_state",
                "chat_archive_pending_attachment",
            ]
            .contains(&table.as_str())
        {
            return Err("unsupported_references");
        }
    }
    let triggers: i64 = tx
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='trigger' AND tbl_name='chat_messages' AND name NOT IN ('chat_message_search_insert','chat_message_search_delete','chat_message_search_update','chat_message_search_move','chat_archive_message_insert','chat_archive_message_delete','chat_archive_message_update')",
            [],
            |row| row.get(0),
        )
        .map_err(|_| "storage_error")?;
    if triggers != 0 {
        return Err("unsupported_references");
    }
    Ok(())
}

/// Prototype transactional merge into an existing room. Raw local messages
/// remain local; only messages missing on this PC are read from the safe copy.
#[cfg(test)]
pub(crate) fn restore_merge(path: &Path, incoming: &Value) -> Result<(), &'static str> {
    let id = incoming
        .get("room")
        .and_then(|r| r.get("id"))
        .and_then(Value::as_str)
        .filter(|id| crate::archive_receipt::valid_chat_id(id))
        .ok_or("invalid_snapshot")?;
    let mut db = Connection::open(path).map_err(|_| "storage_error")?;
    db.execute_batch("PRAGMA foreign_keys=ON")
        .map_err(|_| "storage_error")?;
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| "storage_error")?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS chat_import_message_state (chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE, ordinal INTEGER NOT NULL, message_id TEXT NOT NULL REFERENCES chat_messages(id) ON DELETE CASCADE, source_message_id TEXT, fingerprint TEXT NOT NULL, PRIMARY KEY(chat_id,ordinal)); CREATE TABLE IF NOT EXISTS chat_archive_pending_attachment (receipt_id TEXT PRIMARY KEY NOT NULL, chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE, asset_id TEXT NOT NULL, message_id TEXT REFERENCES chat_messages(id) ON DELETE CASCADE)").map_err(|_| "storage_error")?;
    let current = snapshot_connection(&tx, id)?;
    let mut merged = merge_snapshots(&current, incoming)?;
    for key in ["messages", "imports", "importMessages", "attachments"] {
        for row in merged
            .get(key)
            .and_then(Value::as_array)
            .ok_or("invalid_snapshot")?
        {
            if row.get("chat_id").and_then(Value::as_str) != Some(id) {
                return Err("invalid_snapshot");
            }
        }
    }
    let raw = rows(&tx, "SELECT id,chat_id,role,text,created_at,provider,model,effort,skill,fast,not_handed_over,retryable_fast,actual_effort,finish_reason,auth_method,fast_state FROM chat_messages WHERE chat_id=?1 ORDER BY rowid", id)?;
    let mut originals = std::collections::BTreeMap::new();
    for row in raw {
        let message_id = row
            .get("id")
            .and_then(Value::as_str)
            .ok_or("storage_error")?
            .to_owned();
        originals.insert(message_id, row);
    }
    for row in merged["messages"]
        .as_array_mut()
        .ok_or("invalid_snapshot")?
    {
        let message_id = row
            .get("id")
            .and_then(Value::as_str)
            .ok_or("invalid_snapshot")?;
        if let Some(original) = originals.get(message_id) {
            *row = original.clone();
        }
    }
    check_message_dependents(&tx)?;
    for table in [
        "chat_attachments",
        "chat_archive_pending_attachment",
        "chat_import_message_state",
        "chat_messages",
        "chat_imports",
    ] {
        tx.execute(&format!("DELETE FROM {table} WHERE chat_id=?1"), [id])
            .map_err(|_| "storage_error")?;
    }
    insert_rows(
        &tx,
        &merged,
        "messages",
        "chat_messages",
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
    )?;
    insert_rows(
        &tx,
        &merged,
        "imports",
        "chat_imports",
        &["chat_id", "source_provider", "source_id"],
    )?;
    insert_import_messages(&tx, &merged, id)?;
    insert_attachments(&tx, &merged, id)?;
    let updated = merged["room"]["updated_at"]
        .as_str()
        .ok_or("invalid_snapshot")?;
    tx.execute("UPDATE chats SET updated_at=?1 WHERE id=?2", [updated, id])
        .map_err(|_| "storage_error")?;
    tx.commit().map_err(|_| "storage_error")
}

pub(crate) fn restore_append(path: &Path, incoming: &Value) -> Result<(), &'static str> {
    let mut db = Connection::open(path).map_err(|_| "storage_error")?;
    db.execute_batch("PRAGMA foreign_keys=ON")
        .map_err(|_| "storage_error")?;
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| "storage_error")?;
    restore_append_in_transaction(&tx, incoming)?;
    tx.commit().map_err(|_| "storage_error")
}

/// Restore a received room without changing locally retained messages or metadata.
pub(crate) fn restore_append_in_transaction(
    tx: &Transaction<'_>,
    incoming: &Value,
) -> Result<(), &'static str> {
    if incoming.get("version").and_then(Value::as_u64) != Some(1) || has_known_secret(incoming) {
        return Err("invalid_snapshot");
    }
    let id = incoming
        .pointer("/room/id")
        .and_then(Value::as_str)
        .filter(|id| crate::archive_receipt::valid_chat_id(id))
        .ok_or("invalid_snapshot")?;
    for key in ["messages", "imports", "importMessages", "attachments"] {
        for row in incoming
            .get(key)
            .and_then(Value::as_array)
            .ok_or("invalid_snapshot")?
        {
            if row.get("chat_id").and_then(Value::as_str) != Some(id) {
                return Err("invalid_snapshot");
            }
        }
    }
    let deleted: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_deletions WHERE chat_id=?1)",
            [id],
            |row| row.get(0),
        )
        .map_err(|_| "storage_error")?;
    if deleted {
        return Ok(());
    }
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM chats WHERE id=?1)",
            [id],
            |r| r.get(0),
        )
        .map_err(|_| "storage_error")?;
    if !exists {
        return restore_new_in_transaction(tx, incoming);
    }
    tx.execute_batch("CREATE TABLE IF NOT EXISTS chat_import_message_state (chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE, ordinal INTEGER NOT NULL, message_id TEXT NOT NULL REFERENCES chat_messages(id) ON DELETE CASCADE, source_message_id TEXT, fingerprint TEXT NOT NULL, PRIMARY KEY(chat_id,ordinal)); CREATE TABLE IF NOT EXISTS chat_archive_pending_attachment (receipt_id TEXT PRIMARY KEY NOT NULL, chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE, asset_id TEXT NOT NULL, message_id TEXT REFERENCES chat_messages(id) ON DELETE CASCADE)").map_err(|_| "storage_error")?;
    for message in incoming["messages"].as_array().ok_or("invalid_snapshot")? {
        let message_id = message
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("invalid_snapshot")?;
        let owner: Option<String> = tx
            .query_row(
                "SELECT chat_id FROM chat_messages WHERE id=?1",
                [message_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "storage_error")?;
        if owner.as_deref().is_some_and(|owner| owner != id) {
            return Err("restore_conflict");
        }
        if owner.is_none() {
            insert_rows(
                tx,
                &serde_json::json!({"messages":[message]}),
                "messages",
                "chat_messages",
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
            )?;
        }
    }
    let has_import: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_imports WHERE chat_id=?1)",
            [id],
            |r| r.get(0),
        )
        .map_err(|_| "storage_error")?;
    if !has_import {
        insert_rows(
            tx,
            incoming,
            "imports",
            "chat_imports",
            &["chat_id", "source_provider", "source_id"],
        )?;
    }
    for entry in incoming["importMessages"]
        .as_array()
        .ok_or("invalid_snapshot")?
    {
        let ordinal = entry
            .get("ordinal")
            .and_then(Value::as_i64)
            .ok_or("invalid_snapshot")?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT message_id FROM chat_import_message_state WHERE chat_id=?1 AND ordinal=?2",
                rusqlite::params![id, ordinal],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "storage_error")?;
        if let Some(existing) = existing {
            if entry.get("message_id").and_then(Value::as_str) != Some(existing.as_str()) {
                return Err("restore_conflict");
            }
        } else {
            insert_import_messages(tx, &serde_json::json!({"importMessages":[entry]}), id)?;
        }
    }
    for attachment in incoming["attachments"]
        .as_array()
        .ok_or("invalid_snapshot")?
    {
        let receipt = attachment
            .get("receipt_id")
            .and_then(Value::as_str)
            .ok_or("invalid_snapshot")?;
        let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_attachments WHERE receipt_id=?1 UNION ALL SELECT 1 FROM chat_archive_pending_attachment WHERE receipt_id=?1)", [receipt], |r| r.get(0)).map_err(|_| "storage_error")?;
        if !exists {
            insert_attachments(tx, &serde_json::json!({"attachments":[attachment]}), id)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[cfg(test)]
pub(crate) fn resolve_pending(path: &Path) -> Result<(), &'static str> {
    let mut db = Connection::open(path).map_err(|_| "storage_error")?;
    db.execute_batch("PRAGMA foreign_keys=ON")
        .map_err(|_| "storage_error")?;
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| "storage_error")?;
    resolve_pending_in_transaction(&tx)?;
    tx.commit().map_err(|_| "storage_error")
}

pub(crate) fn resolve_pending_in_transaction(tx: &Transaction<'_>) -> Result<(), &'static str> {
    tx.execute_batch("CREATE TABLE IF NOT EXISTS chat_archive_pending_attachment (receipt_id TEXT PRIMARY KEY NOT NULL, chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE, asset_id TEXT NOT NULL, message_id TEXT REFERENCES chat_messages(id) ON DELETE CASCADE)").map_err(|_| "storage_error")?;
    tx.execute_batch("INSERT INTO chat_attachments(receipt_id,chat_id,asset_id,message_id) SELECT p.receipt_id,p.chat_id,p.asset_id,p.message_id FROM chat_archive_pending_attachment p JOIN momso_source_assets a ON a.id=p.asset_id; DELETE FROM chat_archive_pending_attachment WHERE receipt_id IN (SELECT receipt_id FROM chat_attachments)").map_err(|_| "restore_conflict")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_copy_masks_newly_known_value_and_keeps_identity() {
        let secret = "fixture-opaque-personal-value";
        let input = serde_json::json!({"version":1,"room":{"id":"chat_fixture","title":format!("class {secret}")},"messages":[{"id":"m1","text":format!("lesson {secret}")}],"groups":[],"imports":[],"importMessages":[{"message_id":"m1","fingerprint":"old"}],"attachments":[],"redactions":0});
        let output = redact_historical_snapshot(&input, &[secret]).unwrap();
        assert!(!output.to_string().contains(secret));
        assert_eq!(output["room"]["id"], "chat_fixture");
        assert_eq!(output["messages"][0]["id"], "m1");
        assert_eq!(output["importMessages"][0]["fingerprint"], "redacted-v1");
        assert_eq!(output["redactions"], 2);
        let mut invalid = input;
        invalid["room"]["id"] = Value::String(secret.into());
        assert_eq!(
            redact_historical_snapshot(&invalid, &[secret]),
            Err("sensitive_metadata")
        );
        let mut key_in_field = output;
        key_in_field
            .as_object_mut()
            .unwrap()
            .insert(secret.into(), Value::Null);
        assert_eq!(
            redact_historical_snapshot(&key_in_field, &[secret]),
            Err("sensitive_metadata")
        );
    }
    #[test]
    fn imported_redacted_message_does_not_export_original_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        crate::storage::initialize(&path, &dir.path().join("backups"), "test").unwrap();
        let source = dir.path().join("source.json");
        let secret = "opaque-example-owned-credential";
        std::fs::write(&source, serde_json::json!([{"uuid":"source-test", "name":"lesson", "chat_messages":[{"uuid":"msg-test", "sender":"human", "text":secret}]}]).to_string()).unwrap();
        crate::chat_export_import::import(&path, &source, "claude").unwrap();
        let list = serde_json::to_value(crate::chat::list(&path).unwrap()).unwrap();
        let id = list[0]["id"].as_str().unwrap();
        let original_fingerprint: String = Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT fingerprint FROM chat_import_message_state WHERE chat_id=?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        let copy = snapshot_with_known(&path, id, &[secret]).unwrap();
        assert_eq!(copy["importMessages"][0]["fingerprint"], "redacted-v1");
        assert!(!copy.to_string().contains(secret));
        assert!(!copy.to_string().contains(&original_fingerprint));
        let other = dir.path().join("other.sqlite");
        crate::storage::initialize(&other, &dir.path().join("other-backups"), "test").unwrap();
        restore_new(&other, &copy).unwrap();
        let repeated = crate::chat_export_import::import(&other, &source, "claude").unwrap();
        assert_eq!(serde_json::to_value(repeated).unwrap()["duplicate"], 1);
        let restored = serde_json::to_value(crate::chat::read(&other, id).unwrap()).unwrap();
        assert!(!restored.to_string().contains(secret));
    }
    #[test]
    fn owned_secret_is_redacted_only_in_exported_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        crate::storage::initialize(&path, &dir.path().join("backups"), "test").unwrap();
        let room = crate::chat::create(&path).unwrap();
        let id = serde_json::to_value(room).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let secret = "opaque-example-owned-credential";
        crate::chat::append(&path, &id, crate::chat::Role::User, secret, None, None).unwrap();
        let exported = snapshot_with_known(&path, &id, &[secret]).unwrap();
        assert_eq!(exported["redactions"], 2);
        assert!(!exported.to_string().contains(secret));
        let local = serde_json::to_value(crate::chat::read(&path, &id).unwrap()).unwrap();
        assert_eq!(local["messages"][0]["text"], secret);
    }

    #[test]
    fn exports_a_redacted_copy_without_mutating_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        crate::storage::initialize(&path, &dir.path().join("backups"), "test").unwrap();
        let room = crate::chat::create(&path).unwrap();
        let id = serde_json::to_value(room).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let secret = ["sk-proj-", &"Z".repeat(30)].concat();
        crate::chat::append(&path, &id, crate::chat::Role::User, &secret, None, None).unwrap();
        let exported = snapshot(&path, &id).unwrap();
        assert_eq!(exported["redactions"], 2); // The automatic room title also contains the key.
        assert!(!exported.to_string().contains(&secret));
        let local = serde_json::to_value(crate::chat::read(&path, &id).unwrap()).unwrap();
        assert_eq!(local["messages"][0]["text"], secret);
        assert!(exported["attachments"].as_array().unwrap().is_empty());
    }

    #[test]
    fn known_secret_in_identity_metadata_blocks_the_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        crate::storage::initialize(&path, &dir.path().join("backups"), "test").unwrap();
        let room = crate::chat::create(&path).unwrap();
        let id = serde_json::to_value(room).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let secret = ["ghp_", &"A".repeat(36)].concat();
        Connection::open(&path).unwrap().execute("INSERT INTO chat_imports(chat_id,source_provider,source_id) VALUES(?1,'chatgpt',?2)", [&id, &secret]).unwrap();
        assert_eq!(snapshot(&path, &id), Err("sensitive_metadata"));
        let opaque = "opaque-example-owned-credential";
        Connection::open(&path)
            .unwrap()
            .execute(
                "UPDATE chat_imports SET source_id=?2 WHERE chat_id=?1",
                [&id, opaque],
            )
            .unwrap();
        assert_eq!(
            snapshot_with_known(&path, &id, &[opaque]),
            Err("sensitive_metadata")
        );
    }

    #[test]
    fn imported_chat_keeps_its_deduplication_provenance_in_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        crate::storage::initialize(&path, &dir.path().join("backups"), "test").unwrap();
        let source = dir.path().join("source.json");
        std::fs::write(
            &source,
            serde_json::json!([{
                "uuid": "source-test", "name": "lesson", "chat_messages": [
                    {"uuid": "msg-test", "sender": "human", "text": "hello"}
                ]
            }])
            .to_string(),
        )
        .unwrap();
        let result = crate::chat_export_import::import(&path, &source, "claude").unwrap();
        assert_eq!(serde_json::to_value(result).unwrap()["imported"], 1);
        let list = serde_json::to_value(crate::chat::list(&path).unwrap()).unwrap();
        let id = list[0]["id"].as_str().unwrap();
        let exported = snapshot(&path, id).unwrap();
        assert_eq!(exported["imports"][0]["source_id"], "source-test");
        assert_eq!(
            exported["importMessages"][0]["source_message_id"],
            "msg-test"
        );
        assert!(exported["importMessages"][0]["fingerprint"]
            .as_str()
            .is_some());
        let roundtrip: Value =
            serde_json::from_slice(&serde_json::to_vec_pretty(&exported).unwrap()).unwrap();
        assert_eq!(roundtrip, exported);
        let other = dir.path().join("other.sqlite");
        crate::storage::initialize(&other, &dir.path().join("other-backups"), "test").unwrap();
        restore_new(&other, &roundtrip).unwrap();
        let repeated = crate::chat_export_import::import(&other, &source, "claude").unwrap();
        assert_eq!(serde_json::to_value(repeated).unwrap()["duplicate"], 1);
        let restored = serde_json::to_value(crate::chat::read(&other, id).unwrap()).unwrap();
        assert_eq!(restored["messages"][0]["text"], "hello");
        assert_eq!(restore_new(&other, &roundtrip), Err("restore_conflict"));
        assert_eq!(
            serde_json::to_value(crate::chat::read(&other, id).unwrap()).unwrap(),
            restored
        );
    }

    #[test]
    fn attachment_copy_contains_identifiers_not_source_file_details() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        crate::storage::initialize(&path, &dir.path().join("backups"), "test").unwrap();
        let room = crate::chat::create(&path).unwrap();
        let id = serde_json::to_value(room).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let db = Connection::open(&path).unwrap();
        let hash = "a".repeat(64);
        db.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset-test','audio_recording',?1,5,?2,'local-original.wav','person','2026-01-01')", rusqlite::params![hash, format!("sha256/{hash}")]).unwrap();
        db.execute("INSERT INTO chat_attachments(receipt_id,chat_id,asset_id) VALUES('receipt-test',?1,'asset-test')", [&id]).unwrap();
        drop(db);
        let exported = snapshot(&path, &id).unwrap();
        assert_eq!(exported["attachments"][0]["asset_id"], "asset-test");
        let copy = exported.to_string();
        assert!(!copy.contains("local-original.wav"));
        assert!(!copy.contains(&hash));
        let other = dir.path().join("other.sqlite");
        crate::storage::initialize(&other, &dir.path().join("other-backups"), "test").unwrap();
        restore_new(&other, &exported).unwrap();
        assert_eq!(
            snapshot(&other, &id).unwrap()["attachments"][0]["asset_id"],
            "asset-test"
        );
        let db = Connection::open(&other).unwrap();
        db.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset-test','audio_recording',?1,5,?2,'local-original.wav','person','2026-01-01')", rusqlite::params![hash, format!("sha256/{hash}")]).unwrap();
        drop(db);
        resolve_pending(&other).unwrap();
        let db = Connection::open(&other).unwrap();
        let (attached, pending): (i64, i64) = db.query_row("SELECT (SELECT count(*) FROM chat_attachments),(SELECT count(*) FROM chat_archive_pending_attachment)", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!((attached, pending), (1, 0));
    }

    #[test]
    fn prototype_roundtrip_via_local_bare_git_keeps_chat() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.sqlite");
        let second = dir.path().join("second.sqlite");
        crate::storage::initialize(&first, &dir.path().join("first-backups"), "test").unwrap();
        crate::storage::initialize(&second, &dir.path().join("second-backups"), "test").unwrap();
        let room = crate::chat::create(&first).unwrap();
        let id = serde_json::to_value(room).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        crate::chat::append(
            &first,
            &id,
            crate::chat::Role::User,
            "local test",
            None,
            None,
        )
        .unwrap();
        let repo = dir.path().join("repo");
        let remote = dir.path().join("remote.git");
        let receiver = dir.path().join("receiver");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(&receiver).unwrap();
        let git = |cwd: &Path, args: &[&str]| {
            let output = Command::new("git")
                .arg("-c")
                .arg("init.templateDir=")
                .arg("-c")
                .arg("commit.gpgsign=false")
                .args(args)
                .current_dir(cwd)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env(
                    "GIT_CONFIG_GLOBAL",
                    if cfg!(windows) { "NUL" } else { "/dev/null" },
                )
                .output()
                .unwrap();
            assert!(output.status.success(), "local git fixture failed");
            output.stdout
        };
        git(dir.path(), &["init", "--bare", remote.to_str().unwrap()]);
        git(&repo, &["init"]);
        std::fs::write(
            repo.join("chat.json"),
            serde_json::to_vec_pretty(&snapshot(&first, &id).unwrap()).unwrap(),
        )
        .unwrap();
        git(&repo, &["add", "chat.json"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "fixture",
            ],
        );
        git(
            &repo,
            &["push", remote.to_str().unwrap(), "HEAD:refs/heads/main"],
        );
        git(&receiver, &["init"]);
        git(
            &receiver,
            &["fetch", remote.to_str().unwrap(), "refs/heads/main"],
        );
        let copy = git(&receiver, &["show", "FETCH_HEAD:chat.json"]);
        restore_new(&second, &serde_json::from_slice(&copy).unwrap()).unwrap();
        let restored = serde_json::to_value(crate::chat::read(&second, &id).unwrap()).unwrap();
        assert_eq!(restored["messages"][0]["text"], "local test");
    }

    #[test]
    fn restores_group_title_pin_and_archive_metadata_in_empty_database() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.sqlite");
        let second = dir.path().join("second.sqlite");
        crate::storage::initialize(&first, &dir.path().join("first-backups"), "test").unwrap();
        crate::storage::initialize(&second, &dir.path().join("second-backups"), "test").unwrap();
        let id = serde_json::to_value(crate::chat::create(&first).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let second_id = serde_json::to_value(crate::chat::create(&first).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let db = Connection::open(&first).unwrap();
        db.execute("INSERT INTO chat_groups(id,name,position,created_at) VALUES('group-test','project',3,'2026-01-01')", []).unwrap();
        db.execute("UPDATE chats SET title='lesson',group_id='group-test',pinned_order=2,archived_at='2026-01-02' WHERE id=?1", [&id]).unwrap();
        db.execute(
            "UPDATE chats SET title='other',group_id='group-test' WHERE id=?1",
            [&second_id],
        )
        .unwrap();
        drop(db);
        restore_new(&second, &snapshot(&first, &id).unwrap()).unwrap();
        restore_new(&second, &snapshot(&first, &second_id).unwrap()).unwrap();
        let db = Connection::open(&second).unwrap();
        let values: (String, String, i64, String) = db
            .query_row(
                "SELECT title,group_id,pinned_order,archived_at FROM chats WHERE id=?1",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            values,
            ("lesson".into(), "group-test".into(), 2, "2026-01-02".into())
        );
        let group: String = db
            .query_row(
                "SELECT name FROM chat_groups WHERE id='group-test'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(group, "project");
    }

    #[test]
    fn prototype_merge_of_two_databases_keeps_both_replies() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.sqlite");
        let second = dir.path().join("second.sqlite");
        crate::storage::initialize(&first, &dir.path().join("first-backups"), "test").unwrap();
        crate::storage::initialize(&second, &dir.path().join("second-backups"), "test").unwrap();
        let id = serde_json::to_value(crate::chat::create(&first).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        crate::chat::rename(&first, &id, "lesson").unwrap();
        let initial = snapshot(&first, &id).unwrap();
        restore_new(&second, &initial).unwrap();
        crate::chat::append(
            &first,
            &id,
            crate::chat::Role::User,
            "first device",
            None,
            None,
        )
        .unwrap();
        crate::chat::append(
            &second,
            &id,
            crate::chat::Role::User,
            "second device",
            None,
            None,
        )
        .unwrap();
        let merged = merge_snapshots(
            &snapshot(&first, &id).unwrap(),
            &snapshot(&second, &id).unwrap(),
        )
        .unwrap();
        let texts: Vec<_> = merged["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts.len(), 2);
        assert!(texts.contains(&"first device"));
        assert!(texts.contains(&"second device"));
        let third = dir.path().join("third.sqlite");
        crate::storage::initialize(&third, &dir.path().join("third-backups"), "test").unwrap();
        restore_new(&third, &merged).unwrap();
        assert_eq!(
            serde_json::to_value(crate::chat::read(&third, &id).unwrap()).unwrap()["messages"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn prototype_merges_existing_rooms_without_replacing_local_secret_text() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.sqlite");
        let second = dir.path().join("second.sqlite");
        crate::storage::initialize(&first, &dir.path().join("first-backups"), "test").unwrap();
        crate::storage::initialize(&second, &dir.path().join("second-backups"), "test").unwrap();
        let id = serde_json::to_value(crate::chat::create(&first).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        crate::chat::rename(&first, &id, "lesson").unwrap();
        crate::chat::append(&first, &id, crate::chat::Role::User, "shared", None, None).unwrap();
        restore_new(&second, &snapshot(&first, &id).unwrap()).unwrap();
        let secret = ["sk-proj-", &"Z".repeat(32)].concat();
        crate::chat::append(&first, &id, crate::chat::Role::User, &secret, None, None).unwrap();
        crate::chat::append(&second, &id, crate::chat::Role::User, "reply", None, None).unwrap();
        let first_copy = snapshot(&first, &id).unwrap();
        let second_copy = snapshot(&second, &id).unwrap();
        restore_merge(&first, &second_copy).unwrap();
        restore_merge(&second, &first_copy).unwrap();
        let first_read = serde_json::to_value(crate::chat::read(&first, &id).unwrap()).unwrap();
        let second_read = serde_json::to_value(crate::chat::read(&second, &id).unwrap()).unwrap();
        assert!(first_read.to_string().contains(&secret));
        assert!(!second_read.to_string().contains(&secret));
        assert!(second_read.to_string().contains("비밀값으로 보여 가림"));
        assert!(first_read.to_string().contains("reply"));
        assert!(second_read.to_string().contains("reply"));
        assert_eq!(first_read["messages"].as_array().unwrap().len(), 3);
        assert_eq!(second_read["messages"].as_array().unwrap().len(), 3);
        let mut tampered = snapshot(&second, &id).unwrap();
        tampered["messages"][0]["text"] = Value::String("changed".into());
        assert_eq!(restore_merge(&first, &tampered), Err("merge_conflict"));
        assert_eq!(
            serde_json::to_value(crate::chat::read(&first, &id).unwrap()).unwrap(),
            first_read
        );
    }

    #[test]
    fn unknown_message_dependents_block_merge_before_data_is_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        crate::storage::initialize(&path, &dir.path().join("backups"), "test").unwrap();
        let id = serde_json::to_value(crate::chat::create(&path).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let message =
            crate::chat::append(&path, &id, crate::chat::Role::User, "keep", None, None).unwrap();
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE extra_message_reference(message_id TEXT REFERENCES chat_messages(id) ON DELETE CASCADE)").unwrap();
        db.execute(
            "INSERT INTO extra_message_reference(message_id) VALUES(?1)",
            [&message.id],
        )
        .unwrap();
        drop(db);
        let copy = snapshot(&path, &id).unwrap();
        assert_eq!(restore_merge(&path, &copy), Err("unsupported_references"));
        let db = Connection::open(&path).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM extra_message_reference", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            serde_json::to_value(crate::chat::read(&path, &id).unwrap()).unwrap()["messages"][0]
                ["text"],
            "keep"
        );
    }
}
