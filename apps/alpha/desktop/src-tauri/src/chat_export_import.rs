//! Import known conversation JSON without sending its contents to a model.
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{
    de::{DeserializeSeed, Error, SeqAccess, Visitor},
    Serialize,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fmt,
    fs::File,
    io::{BufReader, Read},
    path::Path,
};
use uuid::Uuid;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::collections::HashMap;

const MAX_ENTRY: u64 = 8 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
static RUNNING: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
fn running() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> { RUNNING.get_or_init(|| Mutex::new(HashMap::new())) }
pub(crate) struct ImportGuard(String);
impl Drop for ImportGuard { fn drop(&mut self) { if let Ok(mut map) = running().lock() { map.remove(&self.0); } } }
pub(crate) fn cancel(id: &str) {
    if let Ok(map) = running().lock() { if let Some(token) = map.get(id) { token.store(true, Ordering::SeqCst); } }
}
pub(crate) fn reserve(id: &str) -> Result<(ImportGuard, Arc<AtomicBool>), String> {
    if id.is_empty() || id.len() > 128 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') { return Err("invalid_request_id".into()); }
    let token = Arc::new(AtomicBool::new(false));
    { let mut map = running().lock().map_err(|_| "unavailable")?; if map.contains_key(id) { return Err("request_busy".into()); } map.insert(id.to_owned(), token.clone()); }
    Ok((ImportGuard(id.to_owned()), token))
}
pub(crate) fn import_with_id(db: &Path, file: &Path, provider: &str, id: &str, progress: &dyn Fn(u64)) -> Result<ResultCounts, String> {
    let (_guard, token) = reserve(id)?;
    import_controlled(db, file, provider, &token, progress)
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResultCounts {
    pub(crate) imported: u64,
    pub(crate) duplicate: u64,
    pub(crate) skipped: u64,
    omitted_non_text_messages: u64,
    omitted_attachments: u64,
    updated: u64,
    added_messages: u64,
    conflicts: u64,
}

struct ImportedMessage {
    source_id: Option<String>,
    role: String,
    text: String,
    time: String,
    model: Option<String>,
    fingerprint: String,
}

fn fingerprint(role: &str, text: &str) -> String {
    let mut digest = Sha256::new();
    digest.update((role.len() as u64).to_be_bytes());
    digest.update(role.as_bytes());
    digest.update(text.as_bytes());
    format!("{:x}", digest.finalize())
}

fn count_attachment(value: &Value, ids: &mut HashSet<String>, anonymous: &mut u64) {
    let id = ["id", "file_id", "asset_pointer"]
        .into_iter()
        .filter_map(|name| value.get(name).and_then(Value::as_str))
        .find(|s| !s.is_empty());
    if let Some(id) = id {
        ids.insert(id.strip_prefix("file-service://").unwrap_or(id).to_owned());
    } else {
        *anonymous += 1;
    }
}

fn content_attachments(value: &Value, ids: &mut HashSet<String>, anonymous: &mut u64) {
    if let Some(items) = value.as_array() {
        for item in items {
            content_attachments(item, ids, anonymous);
        }
        return;
    }
    let Some(object) = value.as_object() else {
        return;
    };
    if let Some(parts) = object.get("parts").and_then(Value::as_array) {
        for item in parts {
            content_attachments(item, ids, anonymous);
        }
        return;
    }
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| object.get("content_type").and_then(Value::as_str));
    if matches!(
        kind,
        Some(
            "image"
                | "image_asset_pointer"
                | "input_image"
                | "audio"
                | "audio_asset_pointer"
                | "input_audio"
                | "output_audio"
                | "video"
                | "file"
                | "document"
                | "attachment"
        )
    ) || object.contains_key("asset_pointer")
        || object.contains_key("file_id")
    {
        count_attachment(value, ids, anonymous);
    }
}

fn omitted_attachments(message: &Value) -> u64 {
    let mut ids = HashSet::new();
    let mut anonymous = 0;
    for attachments in [
        message.get("attachments"),
        message.pointer("/metadata/attachments"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_array)
    {
        for item in attachments {
            count_attachment(item, &mut ids, &mut anonymous);
        }
    }
    if let Some(content) = message.get("content") {
        content_attachments(content, &mut ids, &mut anonymous);
    }
    ids.len() as u64 + anonymous
}

fn timestamp(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.to_owned();
    }
    if let Some(n) = v.as_f64().filter(|n| n.is_finite() && *n >= 0.0) {
        return DateTime::<Utc>::from_timestamp(
            n.trunc() as i64,
            (n.fract() * 1_000_000_000.0) as u32,
        )
        .map(|t| t.to_rfc3339())
        .unwrap_or_default();
    }
    String::new()
}

fn text_parts(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_owned());
    }
    if let Some(a) = v.as_array() {
        let pieces: Vec<_> = a.iter().filter_map(text_parts).collect();
        return (!pieces.is_empty()).then(|| pieces.join("\n"));
    }
    let o = v.as_object()?;
    if let Some(t) = o.get("type").and_then(Value::as_str) {
        if !matches!(t, "text" | "input_text" | "output_text") {
            return None;
        }
    }
    o.get("parts")
        .or_else(|| o.get("text"))
        .and_then(text_parts)
}

fn non_text(v: &Value) -> bool {
    if v.is_string() || v.is_null() {
        return false;
    }
    if let Some(a) = v.as_array() {
        return a.iter().any(non_text);
    }
    let Some(o) = v.as_object() else {
        return true;
    };
    if let Some(t) = o.get("type").and_then(Value::as_str) {
        if !matches!(t, "text" | "input_text" | "output_text") {
            return true;
        }
    }
    if let Some(t) = o.get("content_type").and_then(Value::as_str) {
        if t != "text" {
            return true;
        }
    }
    o.get("parts")
        .or_else(|| o.get("text"))
        .map(non_text)
        .unwrap_or(true)
}

fn record_source(
    tx: &Transaction<'_>,
    chat: &str,
    ordinal: usize,
    message_id: &str,
    message: &ImportedMessage,
) -> Result<(), String> {
    tx.execute("INSERT INTO chat_import_message_state(chat_id,ordinal,message_id,source_message_id,fingerprint) VALUES(?1,?2,?3,?4,?5)", params![chat, ordinal as i64, message_id, message.source_id, message.fingerprint]).map_err(|_| "storage_error")?;
    Ok(())
}

fn insert_message(
    tx: &Transaction<'_>,
    chat: &str,
    provider: &str,
    ordinal: usize,
    message: &ImportedMessage,
) -> Result<(), String> {
    let id = Uuid::new_v4().to_string();
    tx.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at,provider,model) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![id, chat, message.role, message.text, message.time, if message.role == "assistant" { Some(provider) } else { None }, message.model]).map_err(|_| "storage_error")?;
    record_source(tx, chat, ordinal, &id, message)
}

fn update_existing(
    tx: &Transaction<'_>,
    chat: &str,
    provider: &str,
    messages: &[ImportedMessage],
    counts: &mut ResultCounts,
) -> Result<(), String> {
    let mut query = tx.prepare("SELECT ordinal,source_message_id,fingerprint FROM chat_import_message_state WHERE chat_id=?1 ORDER BY ordinal").map_err(|_| "storage_error")?;
    let saved: Vec<(i64, Option<String>, String)> = query
        .query_map([chat], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|_| "storage_error")?
        .collect::<Result<_, _>>()
        .map_err(|_| "storage_error")?;
    drop(query);
    let mut legacy = Vec::new();
    let baseline = if saved.is_empty() {
        let mut query = tx
            .prepare("SELECT id,role,text FROM chat_messages WHERE chat_id=?1 ORDER BY rowid")
            .map_err(|_| "storage_error")?;
        legacy = query
            .query_map([chat], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|_| "storage_error")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "storage_error")?;
        for ((_, role, text), incoming) in legacy.iter().zip(messages) {
            if fingerprint(role, text) != incoming.fingerprint {
                counts.conflicts += 1;
                return Ok(());
            }
        }
        if messages.len() < legacy.len() {
            counts.duplicate += 1;
            return Ok(());
        }
        legacy.len()
    } else {
        for (index, (ordinal, source_id, hash)) in saved.iter().enumerate() {
            if *ordinal != index as i64 {
                counts.conflicts += 1;
                return Ok(());
            }
            if let Some(incoming) = messages.get(index) {
                if hash != &incoming.fingerprint
                    || (source_id.is_some()
                        && incoming.source_id.is_some()
                        && source_id != &incoming.source_id)
                {
                    counts.conflicts += 1;
                    return Ok(());
                }
            }
        }
        saved.len()
    };
    if messages.len() <= baseline {
        // A complete matching legacy export supplies the missing provenance.
        // A shorter export cannot establish where the prior import ended.
        if messages.len() == baseline {
            for (index, (id, _, _)) in legacy.iter().enumerate() {
                record_source(tx, chat, index, id, &messages[index])?;
            }
        }
        counts.duplicate += 1;
        return Ok(());
    }
    for (index, (id, _, _)) in legacy.iter().enumerate() {
        record_source(tx, chat, index, id, &messages[index])?;
    }
    for (index, message) in messages.iter().enumerate().skip(baseline) {
        insert_message(tx, chat, provider, index, message)?;
    }
    counts.updated += 1;
    counts.added_messages += (messages.len() - baseline) as u64;
    Ok(())
}

fn import_one(
    tx: &Transaction<'_>,
    provider: &str,
    value: Value,
    counts: &mut ResultCounts,
) -> Result<(), String> {
    let obj = value.as_object().ok_or("invalid_format")?;
    let source = if provider == "chatgpt" {
        obj.get("conversation_id").or_else(|| obj.get("id"))
    } else {
        obj.get("uuid").or_else(|| obj.get("id"))
    }
    .and_then(Value::as_str)
    .filter(|s| !s.is_empty());
    let Some(source) = source else {
        counts.skipped += 1;
        return Ok(());
    };
    let existing_chat: Option<String> = tx
        .query_row(
            "SELECT chat_id FROM chat_imports WHERE source_provider=?1 AND source_id=?2",
            params![provider, source],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| "storage_error")?;
    let mut messages: Vec<(&Value, &str, Option<&str>)> = Vec::new();
    if provider == "chatgpt" {
        let mapping = obj
            .get("mapping")
            .and_then(Value::as_object)
            .ok_or("invalid_format")?;
        let mut next = obj.get("current_node").and_then(Value::as_str);
        let mut visited = HashSet::new();
        while let Some(id) = next {
            if !visited.insert(id) {
                return Err("invalid_format".into());
            }
            let node = mapping.get(id).ok_or("invalid_format")?;
            if let Some(m) = node.get("message").filter(|m| m.is_object()) {
                if let Some(role @ ("user" | "assistant")) =
                    m.pointer("/author/role").and_then(Value::as_str)
                {
                    let message_id = m
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .or(Some(id));
                    messages.push((m, role, message_id));
                }
            }
            next = node.get("parent").and_then(Value::as_str);
        }
        messages.reverse();
    } else {
        let list = obj
            .get("chat_messages")
            .and_then(Value::as_array)
            .ok_or("invalid_format")?;
        for m in list {
            let role = match m.get("sender").and_then(Value::as_str) {
                Some("human") => "user",
                Some("assistant") => "assistant",
                _ => continue,
            };
            let message_id = m
                .get("uuid")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    m.get("id")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                });
            messages.push((m, role, message_id));
        }
    }
    counts.omitted_non_text_messages += messages
        .iter()
        .filter(|(m, _, _)| {
            m.get("content").is_some_and(non_text)
                || m.get("attachments")
                    .and_then(Value::as_array)
                    .is_some_and(|a| !a.is_empty())
                || m.pointer("/metadata/attachments")
                    .and_then(Value::as_array)
                    .is_some_and(|a| !a.is_empty())
        })
        .count() as u64;
    counts.omitted_attachments += messages
        .iter()
        .map(|(m, _, _)| omitted_attachments(m))
        .sum::<u64>();
    let messages: Vec<_> = messages
        .into_iter()
        .filter_map(|(m, role, source_id)| {
            let content = m.get("content").or_else(|| m.get("text"))?;
            let text = text_parts(content)
                .or_else(|| m.get("text").and_then(Value::as_str).map(str::to_owned))?;
            if text.is_empty() {
                return None;
            }
            let time = timestamp(
                m.get(if provider == "chatgpt" {
                    "create_time"
                } else {
                    "created_at"
                })
                .unwrap_or(&Value::Null),
            );
            let model = m
                .pointer("/metadata/model_slug")
                .or_else(|| m.get("model"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            Some(ImportedMessage {
                source_id: source_id.map(str::to_owned),
                role: role.into(),
                fingerprint: fingerprint(role, &text),
                text,
                time,
                model,
            })
        })
        .collect();
    if messages.is_empty() {
        counts.skipped += 1;
        return Ok(());
    }
    let mut source_ids = HashSet::new();
    if messages
        .iter()
        .filter_map(|m| m.source_id.as_deref())
        .any(|id| !source_ids.insert(id))
    {
        counts.conflicts += 1;
        return Ok(());
    }
    if let Some(id) = existing_chat {
        return update_existing(tx, &id, provider, &messages, counts);
    }
    let id = format!("chat_{}", Uuid::new_v4());
    let title = obj
        .get(if provider == "chatgpt" {
            "title"
        } else {
            "name"
        })
        .and_then(Value::as_str)
        .unwrap_or("");
    let created = timestamp(
        obj.get(if provider == "chatgpt" {
            "create_time"
        } else {
            "created_at"
        })
        .unwrap_or(&Value::Null),
    );
    let updated = timestamp(
        obj.get(if provider == "chatgpt" {
            "update_time"
        } else {
            "updated_at"
        })
        .unwrap_or(&Value::Null),
    );
    tx.execute(
        "INSERT INTO chats(id,title,created_at,updated_at) VALUES(?1,?2,?3,?4)",
        params![id, title, created, updated],
    )
    .map_err(|_| "storage_error")?;
    tx.execute(
        "INSERT INTO chat_imports(chat_id,source_provider,source_id) VALUES(?1,?2,?3)",
        params![id, provider, source],
    )
    .map_err(|_| "storage_error")?;
    for (ordinal, message) in messages.iter().enumerate() {
        insert_message(tx, &id, provider, ordinal, message)?;
    }
    counts.imported += 1;
    Ok(())
}

struct ArraySeed<'a, 'b> {
    tx: &'a Transaction<'b>,
    provider: &'a str,
    counts: &'a mut ResultCounts,
    cancelled: &'a AtomicBool,
    progress: &'a dyn Fn(u64),
    processed: &'a mut u64,
}
impl<'de> DeserializeSeed<'de> for ArraySeed<'_, '_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for ArraySeed<'_, '_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("conversation array")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(v) = seq.next_element::<Value>()? {
            if self.cancelled.load(Ordering::SeqCst) { return Err(A::Error::custom("cancelled")); }
            import_one(self.tx, self.provider, v, self.counts).map_err(A::Error::custom)?;
            *self.processed += 1;
            (self.progress)(*self.processed);
        }
        Ok(())
    }
}

fn parse<R: Read>(
    reader: R,
    tx: &Transaction<'_>,
    provider: &str,
    counts: &mut ResultCounts,
    cancelled: &AtomicBool,
    progress: &dyn Fn(u64),
    processed: &mut u64,
) -> Result<(), String> {
    let mut de = serde_json::Deserializer::from_reader(BufReader::new(reader));
    ArraySeed {
        tx,
        provider,
        counts,
        cancelled,
        progress,
        processed,
    }
    .deserialize(&mut de)
    .map_err(|_| if cancelled.load(Ordering::SeqCst) { "cancelled" } else { "invalid_format" }.to_owned())?;
    de.end().map_err(|_| "invalid_format".to_owned())
}

pub(crate) fn import(db: &Path, file: &Path, provider: &str) -> Result<ResultCounts, String> {
    import_controlled(db, file, provider, &AtomicBool::new(false), &|_| {})
}

pub(crate) fn import_controlled(db: &Path, file: &Path, provider: &str, cancelled: &AtomicBool, progress: &dyn Fn(u64)) -> Result<ResultCounts, String> {
    if provider == "gemini" {
        return Err("unsupported_format".into());
    }
    if !matches!(provider, "chatgpt" | "claude") {
        return Err("unsupported_provider".into());
    }
    let mut db = Connection::open(db).map_err(|_| "storage_error")?;
    db.execute_batch("PRAGMA foreign_keys=ON")
        .map_err(|_| "storage_error")?;
    let tx = db.transaction().map_err(|_| "storage_error")?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS chat_import_message_state (chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE, ordinal INTEGER NOT NULL, message_id TEXT NOT NULL REFERENCES chat_messages(id) ON DELETE CASCADE, source_message_id TEXT, fingerprint TEXT NOT NULL, PRIMARY KEY(chat_id,ordinal))").map_err(|_| "storage_error")?;
    let mut counts = ResultCounts::default();
    let mut processed = 0;
    let f = File::open(file).map_err(|_| "file_error")?;
    if file
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("zip"))
    {
        let mut zip = zip::ZipArchive::new(f).map_err(|_| "invalid_format")?;
        if zip.len() > MAX_ENTRIES {
            return Err("archive_limit".into());
        }
        let mut matched = false;
        for i in 0..zip.len() {
            let entry = zip.by_index(i).map_err(|_| "invalid_format")?;
            let name = entry
                .name()
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            let is_conversation = name == "conversations.json"
                || (name.starts_with("conversations-") && name.ends_with(".json"))
                || (name.starts_with("conversations_") && name.ends_with(".json"));
            if !is_conversation {
                continue;
            }
            if entry.size() > MAX_ENTRY {
                return Err("archive_limit".into());
            }
            matched = true;
            parse(entry, &tx, provider, &mut counts, cancelled, progress, &mut processed)?;
        }
        if !matched {
            return Err("unsupported_format".into());
        }
    } else {
        parse(f, &tx, provider, &mut counts, cancelled, progress, &mut processed)?;
    }
    if cancelled.load(Ordering::SeqCst) { return Err("cancelled".into()); }
    tx.commit().map_err(|_| "storage_error")?;
    Ok(counts)
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Preview {
    pub(crate) conversations: u64,
    pub(crate) already_imported: u64,
    pub(crate) unreadable: u64,
    oldest: Option<String>,
    newest: Option<String>,
}

struct PreviewSeed<'a> {
    db: &'a Connection,
    provider: &'a str,
    result: &'a mut Preview,
}
fn readable_conversation(v: &Value, provider: &str) -> bool {
    if provider == "claude" {
        return v.get("chat_messages").and_then(Value::as_array).is_some_and(|items| items.iter().any(|m| {
            matches!(m.get("sender").and_then(Value::as_str), Some("human" | "assistant")) && m.get("content").or_else(|| m.get("text")).and_then(text_parts).or_else(|| m.get("text").and_then(Value::as_str).map(str::to_owned)).is_some_and(|t| !t.is_empty())
        }));
    }
    let Some(mapping) = v.get("mapping").and_then(Value::as_object) else { return false; };
    let mut current = v.get("current_node").and_then(Value::as_str);
    let mut seen = HashSet::new();
    let mut has_text = false;
    while let Some(id) = current {
        if !seen.insert(id) { return false; }
        let Some(node) = mapping.get(id) else { return false; };
        if let Some(m) = node.get("message") {
            if matches!(m.pointer("/author/role").and_then(Value::as_str), Some("user" | "assistant")) && m.get("content").or_else(|| m.get("text")).and_then(text_parts).or_else(|| m.get("text").and_then(Value::as_str).map(str::to_owned)).is_some_and(|t| !t.is_empty()) { has_text = true; }
        }
        current = node.get("parent").and_then(Value::as_str);
    }
    has_text
}
impl<'de> DeserializeSeed<'de> for PreviewSeed<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> { d.deserialize_seq(self) }
}
impl<'de> Visitor<'de> for PreviewSeed<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str("conversation array") }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(v) = seq.next_element::<Value>()? {
            self.result.conversations += 1;
            let source = if self.provider == "chatgpt" { v.get("conversation_id").or_else(|| v.get("id")) } else { v.get("uuid").or_else(|| v.get("id")) }.and_then(Value::as_str).filter(|s| !s.is_empty());
            let valid = readable_conversation(&v, self.provider);
            let Some(source) = source.filter(|_| valid) else { self.result.unreadable += 1; continue; };
            let exists: bool = self.db.query_row("SELECT EXISTS(SELECT 1 FROM chat_imports WHERE source_provider=?1 AND source_id=?2)", params![self.provider, source], |r| r.get(0)).map_err(A::Error::custom)?;
            if exists { self.result.already_imported += 1; }
            let date = timestamp(v.get(if self.provider == "chatgpt" { "create_time" } else { "created_at" }).unwrap_or(&Value::Null));
            if let Ok(parsed) = DateTime::parse_from_rfc3339(&date) {
                let normalized = parsed.with_timezone(&Utc).to_rfc3339();
                if self.result.oldest.as_ref().is_none_or(|d| normalized < *d) { self.result.oldest = Some(normalized.clone()); }
                if self.result.newest.as_ref().is_none_or(|d| normalized > *d) { self.result.newest = Some(normalized); }
            }
        }
        Ok(())
    }
}
fn preview_parse<R: Read>(reader: R, db: &Connection, provider: &str, result: &mut Preview) -> Result<(), String> {
    let mut de = serde_json::Deserializer::from_reader(BufReader::new(reader));
    PreviewSeed { db, provider, result }.deserialize(&mut de).map_err(|_| "invalid_format".to_string())?;
    de.end().map_err(|_| "invalid_format".to_string())
}
pub(crate) fn preview(db: &Path, file: &Path, provider: &str) -> Result<Preview, String> {
    if provider == "gemini" { return Err("unsupported_format".into()); }
    if !matches!(provider, "chatgpt" | "claude") { return Err("unsupported_provider".into()); }
    let db = Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|_| "storage_error")?;
    let f = File::open(file).map_err(|_| "file_error")?;
    let mut result = Preview::default();
    if file.extension().and_then(|s| s.to_str()).is_some_and(|s| s.eq_ignore_ascii_case("zip")) {
        let mut zip = zip::ZipArchive::new(f).map_err(|_| "invalid_format")?;
        if zip.len() > MAX_ENTRIES { return Err("archive_limit".into()); }
        let mut matched = false;
        for i in 0..zip.len() {
            let entry = zip.by_index(i).map_err(|_| "invalid_format")?;
            let name = entry.name().rsplit('/').next().unwrap_or("").to_ascii_lowercase();
            if !(name == "conversations.json" || (name.starts_with("conversations-") && name.ends_with(".json")) || (name.starts_with("conversations_") && name.ends_with(".json"))) { continue; }
            if entry.size() > MAX_ENTRY { return Err("archive_limit".into()); }
            matched = true;
            preview_parse(entry, &db, provider, &mut result)?;
        }
        if !matched { return Err("unsupported_format".into()); }
    } else { preview_parse(f, &db, provider, &mut result)?; }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("momso.db");
        crate::storage::initialize(&db, &dir.path().join("backup"), "test").unwrap();
        (dir, db)
    }

    #[test]
    fn preview_counts_without_writing_and_cancel_rolls_back() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let item = |id: &str, date: &str| serde_json::json!({"uuid":id,"created_at":date,"chat_messages":[{"sender":"human","text":"private contents"}]});
        std::fs::write(&file, serde_json::json!([item("a","2020-01-01T00:00:00Z")]).to_string()).unwrap();
        import(&db, &file, "claude").unwrap();
        std::fs::write(&file, serde_json::json!([item("a","2020-01-01T00:00:00Z"),item("b","2024-01-01T00:00:00Z"),{"uuid":"broken"}]).to_string()).unwrap();
        let p = preview(&db, &file, "claude").unwrap();
        assert_eq!((p.conversations,p.already_imported,p.unreadable), (3,1,1));
        assert!(p.oldest.unwrap().starts_with("2020-01-01"));
        assert!(p.newest.unwrap().starts_with("2024-01-01"));
        let count = || Connection::open(&db).unwrap().query_row("SELECT count(*) FROM chats", [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(count(), 1);
        let cancelled = AtomicBool::new(false);
        let result = import_controlled(&db, &file, "claude", &cancelled, &|n| { if n == 1 { cancelled.store(true, Ordering::SeqCst); } });
        assert!(matches!(result, Err(e) if e == "cancelled"));
        assert_eq!(count(), 1);
        assert!(matches!(preview(&db, &file, "gemini"), Err(e) if e == "unsupported_format"));
    }

    #[test]
    fn progress_request_cancels_without_saving_and_cleans_registry() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        std::fs::write(&file, serde_json::json!([
            {"uuid":"one","chat_messages":[{"sender":"human","text":"one"}]},
            {"uuid":"two","chat_messages":[{"sender":"human","text":"two"}]}
        ]).to_string()).unwrap();
        let id = format!("test-{}", Uuid::new_v4());
        let result = import_with_id(&db, &file, "claude", &id, &|n| if n == 1 { cancel(&id); });
        assert!(matches!(result, Err(e) if e == "cancelled"));
        assert_eq!(Connection::open(&db).unwrap().query_row("SELECT count(*) FROM chats", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert!(!running().lock().unwrap().contains_key(&id));
    }

    #[test]
    fn imports_chatgpt_branch_once_and_preserves_history() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        std::fs::write(&file, serde_json::json!([{"id":"source-1","title":"원래 제목","create_time":1700000000,"current_node":"b","mapping":{"a":{"parent":null,"message":{"author":{"role":"user"},"content":{"parts":["  질문\n"]}}},"b":{"parent":"a","message":{"author":{"role":"assistant"},"create_time":1700000001,"metadata":{"model_slug":"gpt-x"},"content":{"parts":["답"]}}},"other":{"parent":"a","message":{"author":{"role":"assistant"},"content":{"parts":["다른 가지"]}}}}}]).to_string()).unwrap();
        let first = import(&db, &file, "chatgpt").unwrap();
        assert_eq!((first.imported, first.duplicate), (1, 0));
        let second = import(&db, &file, "chatgpt").unwrap();
        assert_eq!((second.imported, second.duplicate), (0, 1));
        let list = serde_json::to_value(crate::chat::list(&db).unwrap()).unwrap();
        assert_eq!(list[0]["importedFrom"], "chatgpt");
        let id = list[0]["id"].as_str().unwrap();
        let detail = serde_json::to_value(crate::chat::read(&db, id).unwrap()).unwrap();
        assert_eq!(detail["messages"][0]["text"], "  질문\n");
        assert!(detail["messages"][0]["createdAt"].is_null());
        assert_eq!(detail["messages"][1]["model"], "gpt-x");
        assert_eq!(detail["messages"].as_array().unwrap().len(), 2);
        let handoff = crate::chat_handoff::prepare(&db, id, "claude", false, "이어 질문").unwrap();
        assert!(handoff.prompt.contains("  질문\n"));
        assert!(handoff.prompt.contains("답"));
        assert!(!handoff.prompt.contains("다른 가지"));
    }

    // Based on the reproduction in d26dcab7: later exports must add the reply.
    #[test]
    fn audit_reproduces_later_export_messages_skipped_as_duplicate() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let old = serde_json::json!([{"uuid":"same-id","name":"lesson","chat_messages":[{"sender":"human","text":"first"}]}]);
        std::fs::write(&file, old.to_string()).unwrap();
        assert_eq!(import(&db, &file, "claude").unwrap().imported, 1);
        let later = serde_json::json!([{"uuid":"same-id","name":"lesson","chat_messages":[{"sender":"human","text":"first"},{"sender":"assistant","text":"new reply"}]}]);
        std::fs::write(&file, later.to_string()).unwrap();
        let result = import(&db, &file, "claude").unwrap();
        assert_eq!(
            (
                result.imported,
                result.updated,
                result.added_messages,
                result.conflicts
            ),
            (0, 1, 1, 0)
        );
        let count: i64 = Connection::open(&db)
            .unwrap()
            .query_row("SELECT count(*) FROM chat_messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        assert!(std::fs::read_to_string(&file)
            .unwrap()
            .contains("new reply"));
        assert_eq!(import(&db, &file, "claude").unwrap().duplicate, 1);
    }

    #[test]
    fn reimport_preserves_local_messages_and_reports_divergence() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let export = |msgs: Vec<Value>| {
            serde_json::json!([{"uuid":"source","name":"lesson","chat_messages":msgs}]).to_string()
        };
        let first = serde_json::json!({"uuid":"m1","sender":"human","text":"first"});
        std::fs::write(&file, export(vec![first.clone()])).unwrap();
        import(&db, &file, "claude").unwrap();
        let c = Connection::open(&db).unwrap();
        let chat: String = c
            .query_row("SELECT chat_id FROM chat_imports", [], |r| r.get(0))
            .unwrap();
        drop(c);
        crate::chat::append(
            &db,
            &chat,
            crate::chat::Role::User,
            "local reply",
            None,
            None,
        )
        .unwrap();
        let next = serde_json::json!({"uuid":"m2","sender":"assistant","text":"export reply"});
        std::fs::write(&file, export(vec![first.clone(), next.clone()])).unwrap();
        let result = import(&db, &file, "claude").unwrap();
        assert_eq!((result.added_messages, result.conflicts), (1, 0));
        let read = serde_json::to_value(crate::chat::read(&db, &chat).unwrap()).unwrap();
        assert_eq!(read["messages"][1]["text"], "local reply");
        assert_eq!(read["messages"][2]["text"], "export reply");
        let altered = serde_json::json!({"uuid":"m1","sender":"human","text":"changed"});
        std::fs::write(&file, export(vec![altered, next])).unwrap();
        let result = import(&db, &file, "claude").unwrap();
        assert_eq!((result.added_messages, result.conflicts), (0, 1));
        assert_eq!(
            serde_json::to_value(crate::chat::read(&db, &chat).unwrap()).unwrap(),
            read
        );
    }

    #[test]
    fn omitted_attachments_are_counted_as_items_not_messages() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        std::fs::write(&file, serde_json::json!([{"uuid":"source","chat_messages":[{"sender":"human","text":"hello","attachments":[{"file_name":"one"},{"file_name":"two"}]},{"sender":"assistant","content":[{"type":"text","text":"answer"},{"type":"image","source":{}}]}]}]).to_string()).unwrap();
        let result = import(&db, &file, "claude").unwrap();
        assert_eq!(result.omitted_non_text_messages, 2);
        assert_eq!(result.omitted_attachments, 3);
    }

    #[test]
    fn attachment_references_in_metadata_and_content_are_not_double_counted() {
        let message = serde_json::json!({"metadata":{"attachments":[{"id":"file-one"}]},"content":{"content_type":"multimodal_text","parts":["hello",{"content_type":"image_asset_pointer","asset_pointer":"file-service://file-one"},{"content_type":"image_asset_pointer","asset_pointer":"file-service://file-two"}]}});
        assert_eq!(omitted_attachments(&message), 2);
        assert_eq!(
            omitted_attachments(
                &serde_json::json!({"content":[{"type":"thinking","thinking":"private"}]})
            ),
            0
        );
        let message = serde_json::json!({"metadata":{"attachments":[{"id":null,"file_id":"file-one"}]},"content":{"parts":[{"asset_pointer":"file-service://file-one"},{"file_id":"file-two"}]}});
        assert_eq!(omitted_attachments(&message), 2);
        assert_eq!(
            omitted_attachments(
                &serde_json::json!({"content":{"type":null,"content_type":"image"}})
            ),
            1
        );
    }

    #[test]
    fn legacy_import_with_ambiguous_local_tail_is_not_overwritten() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let c = Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES('legacy','title','','')",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO chat_imports(chat_id,source_provider,source_id) VALUES('legacy','claude','source')", []).unwrap();
        c.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('one','legacy','user','first','')", []).unwrap();
        c.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('local','legacy','user','local','')", []).unwrap();
        drop(c);
        std::fs::write(&file, serde_json::json!([{"uuid":"source","chat_messages":[{"sender":"human","text":"first"},{"sender":"assistant","text":"later"}]}]).to_string()).unwrap();
        let result = import(&db, &file, "claude").unwrap();
        assert_eq!((result.conflicts, result.added_messages), (1, 0));
        let detail = serde_json::to_value(crate::chat::read(&db, "legacy").unwrap()).unwrap();
        assert_eq!(detail["messages"][1]["text"], "local");
    }

    #[test]
    fn legacy_import_with_matching_history_adds_new_export_messages() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let c = Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES('legacy','title','','')",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO chat_imports(chat_id,source_provider,source_id) VALUES('legacy','claude','source')", []).unwrap();
        c.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('one','legacy','user','first','')", []).unwrap();
        drop(c);
        std::fs::write(&file, serde_json::json!([{"uuid":"source","chat_messages":[{"uuid":"m1","sender":"human","text":"first"},{"uuid":"m2","sender":"assistant","text":"later"}]}]).to_string()).unwrap();
        let result = import(&db, &file, "claude").unwrap();
        assert_eq!(
            (result.updated, result.added_messages, result.conflicts),
            (1, 1, 0)
        );
        assert_eq!(import(&db, &file, "claude").unwrap().duplicate, 1);
        let detail = serde_json::to_value(crate::chat::read(&db, "legacy").unwrap()).unwrap();
        assert_eq!(detail["messages"].as_array().unwrap().len(), 2);
        assert_eq!(detail["messages"][1]["text"], "later");
    }

    #[test]
    fn matching_legacy_reimport_tracks_original_before_local_continuation() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let c = Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES('legacy','title','','')",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO chat_imports(chat_id,source_provider,source_id) VALUES('legacy','claude','source')", []).unwrap();
        c.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('one','legacy','user','first','')", []).unwrap();
        drop(c);
        let original = serde_json::json!([{"uuid":"source","chat_messages":[{"uuid":"m1","sender":"human","text":"first"}]}]);
        std::fs::write(&file, original.to_string()).unwrap();
        assert_eq!(import(&db, &file, "claude").unwrap().duplicate, 1);
        crate::chat::append(&db, "legacy", crate::chat::Role::User, "local", None, None).unwrap();
        let later = serde_json::json!([{"uuid":"source","chat_messages":[{"uuid":"m1","sender":"human","text":"first"},{"uuid":"m2","sender":"assistant","text":"later"}]}]);
        std::fs::write(&file, later.to_string()).unwrap();
        let result = import(&db, &file, "claude").unwrap();
        assert_eq!((result.added_messages, result.conflicts), (1, 0));
        let detail = serde_json::to_value(crate::chat::read(&db, "legacy").unwrap()).unwrap();
        assert_eq!(detail["messages"][1]["text"], "local");
        assert_eq!(detail["messages"][2]["text"], "later");
    }

    #[test]
    fn shorter_legacy_export_does_not_claim_an_unknown_tail() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let c = Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES('legacy','title','','')",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO chat_imports(chat_id,source_provider,source_id) VALUES('legacy','claude','source')", []).unwrap();
        c.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('one','legacy','user','first','')", []).unwrap();
        c.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('two','legacy','assistant','unknown tail','')", []).unwrap();
        drop(c);
        std::fs::write(&file, serde_json::json!([{"uuid":"source","chat_messages":[{"sender":"human","text":"first"}]}]).to_string()).unwrap();
        assert_eq!(import(&db, &file, "claude").unwrap().duplicate, 1);
        let c = Connection::open(&db).unwrap();
        let count: i64 = c
            .query_row(
                "SELECT count(*) FROM chat_import_message_state WHERE chat_id='legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn chatgpt_new_branch_message_and_attachment_status_are_reported() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let mut source = serde_json::json!([{"id":"source","current_node":"a","mapping":{"a":{"parent":null,"message":{"id":"m1","author":{"role":"user"},"content":{"parts":["hello"]}}}}}]);
        std::fs::write(&file, source.to_string()).unwrap();
        import(&db, &file, "chatgpt").unwrap();
        source[0]["current_node"] = "b".into();
        source[0]["mapping"]["b"] = serde_json::json!({"parent":"a","message":{"id":"m2","author":{"role":"assistant"},"content":{"parts":["reply",{"content_type":"image_asset_pointer","asset_pointer":"file-service://file-one"}]}}});
        std::fs::write(&file, source.to_string()).unwrap();
        let result = serde_json::to_value(import(&db, &file, "chatgpt").unwrap()).unwrap();
        assert_eq!(result["updated"], 1);
        assert_eq!(result["addedMessages"], 1);
        assert_eq!(result["omittedAttachments"], 1);
        source[0]["mapping"]["a"]["message"]["id"] = "different-id".into();
        std::fs::write(&file, source.to_string()).unwrap();
        assert_eq!(import(&db, &file, "chatgpt").unwrap().conflicts, 1);
    }

    #[test]
    fn imports_claude_zip_and_rejects_unknown_gemini() {
        let (dir, db) = fixture();
        let path = dir.path().join("export.zip");
        let file = File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file(
            "conversations.json",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        write!(zip, "{}", serde_json::json!([{"uuid":"s-1","name":"이름","chat_messages":[{"sender":"human","text":"안녕"},{"sender":"assistant","content":[{"type":"text","text":"반가워"}],"created_at":"2025-01-01T00:00:00Z"}]}])).unwrap();
        zip.finish().unwrap();
        assert_eq!(import(&db, &path, "claude").unwrap().imported, 1);
        let list = serde_json::to_value(crate::chat::list(&db).unwrap()).unwrap();
        let detail =
            serde_json::to_value(crate::chat::read(&db, list[0]["id"].as_str().unwrap()).unwrap())
                .unwrap();
        assert_eq!(detail["messages"][1]["text"], "반가워");
        assert_eq!(detail["messages"][1]["createdAt"], "2025-01-01T00:00:00Z");
        assert!(detail["messages"][1]["model"].is_null());
        assert!(matches!(import(&db,&path,"gemini"),Err(e) if e == "unsupported_format"));
    }

    #[test]
    fn malformed_file_rolls_back_all_conversations() {
        let (dir, db) = fixture();
        let path = dir.path().join("bad.json");
        std::fs::write(
            &path,
            r#"[{"uuid":"one","chat_messages":[{"sender":"human","text":"x"}]}, {"oops": ]"#,
        )
        .unwrap();
        assert!(import(&db, &path, "claude").is_err());
        assert!(crate::chat::list(&db).unwrap().is_empty());
    }

    #[test]
    fn streams_many_conversations_and_deduplicates() {
        let (dir, db) = fixture();
        let path = dir.path().join("many.json");
        let mut f = File::create(&path).unwrap();
        write!(f, "[").unwrap();
        for n in 0..2000 {
            if n != 0 {
                write!(f, ",").unwrap();
            }
            write!(f,"{{\"uuid\":\"{n}\",\"chat_messages\":[{{\"sender\":\"human\",\"text\":\"hello\"}}]}}").unwrap();
        }
        write!(f, "]").unwrap();
        drop(f);
        assert_eq!(import(&db, &path, "claude").unwrap().imported, 2000);
        assert_eq!(import(&db, &path, "claude").unwrap().duplicate, 2000);
    }
}
