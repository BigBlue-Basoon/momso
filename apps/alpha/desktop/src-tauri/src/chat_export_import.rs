//! Import known conversation JSON without sending its contents to a model.
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{
    de::{DeserializeSeed, Error, SeqAccess, Visitor},
    Serialize,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::{
    collections::HashSet,
    fmt,
    fs::File,
    io::{BufReader, Read},
    path::Path,
};
use uuid::Uuid;

const MAX_ENTRY: u64 = 8 * 1024 * 1024 * 1024;
fn file_open_error(error: std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::NotFound => "file_not_found",
        std::io::ErrorKind::PermissionDenied => "file_permission_denied",
        _ => "file_error",
    }
}
fn import_storage_error(error: rusqlite::Error) -> &'static str {
    use rusqlite::ErrorCode;
    match error.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => "storage_busy",
        Some(ErrorCode::ReadOnly | ErrorCode::PermissionDenied) => "storage_permission_denied",
        Some(ErrorCode::DiskFull) => "storage_full",
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => "storage_corrupt",
        _ => "storage_error",
    }
}
const MAX_ENTRIES: usize = 100_000;
const GEMINI_MAX_BYTES: u64 = 64 * 1024 * 1024;
const GEMINI_GROUP_MINUTES: i64 = 30;
pub(crate) const GEMINI_GROUPING_NOTICE: &str = "제미나이 기록은 대화 단위 정보가 없어 같은 UTC 날짜에서 항목 사이가 30분 이내인 기록을 한 채팅방으로 나눴습니다.";
pub(crate) const GEMINI_JSON_GUIDANCE: &str =
    "구글 테이크아웃에서 '내 활동'의 형식을 JSON으로 골라 내보내 주세요.";
const GEMINI_HTML_NEEDS_JSON: &str = "gemini_html_needs_json";
static RUNNING: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
fn running() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    RUNNING.get_or_init(|| Mutex::new(HashMap::new()))
}
pub(crate) struct ImportGuard(String);
impl Drop for ImportGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = running().lock() {
            map.remove(&self.0);
        }
    }
}
pub(crate) fn cancel(id: &str) {
    if let Ok(map) = running().lock() {
        if let Some(token) = map.get(id) {
            token.store(true, Ordering::SeqCst);
        }
    }
}
pub(crate) fn reserve(id: &str) -> Result<(ImportGuard, Arc<AtomicBool>), String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("invalid_request_id".into());
    }
    let token = Arc::new(AtomicBool::new(false));
    {
        let mut map = running().lock().map_err(|_| "unavailable")?;
        if map.contains_key(id) {
            return Err("request_busy".into());
        }
        map.insert(id.to_owned(), token.clone());
    }
    Ok((ImportGuard(id.to_owned()), token))
}
pub(crate) fn import_with_id(
    db: &Path,
    file: &Path,
    provider: &str,
    id: &str,
    progress: &dyn Fn(u64),
) -> Result<ResultCounts, String> {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) grouping_notice: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) guidance: Option<&'static str>,
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
        if let [only] = a.as_slice() {
            return text_parts(only);
        }
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
    tx.execute("INSERT INTO chat_import_message_state(chat_id,ordinal,message_id,source_message_id,fingerprint) VALUES(?1,?2,?3,?4,?5)", params![chat, ordinal as i64, message_id, message.source_id, message.fingerprint]).map_err(import_storage_error)?;
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
    tx.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at,provider,model) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![id, chat, message.role, message.text, message.time, if message.role == "assistant" { crate::chat::canonical_provider(Some(provider)) } else { None }, message.model]).map_err(import_storage_error)?;
    record_source(tx, chat, ordinal, &id, message)
}

fn matches_redacted_message(saved: &str, incoming: &str) -> bool {
    let markers = [
        "[비밀값으로 보여 가림: 개인 키]",
        "[비밀값으로 보여 가림: API 키]",
        "[비밀값으로 보여 가림: 토큰]",
        "[비밀값으로 보여 가림: 저장된 값]",
    ];
    let mut matches = markers
        .iter()
        .flat_map(|marker| saved.match_indices(marker).map(move |(i, _)| (i, *marker)));
    let Some((offset, marker)) = matches.next() else {
        return false;
    };
    if matches.next().is_some() {
        return false;
    }
    let prefix = &saved[..offset];
    let suffix = &saved[offset + marker.len()..];
    incoming.len() >= prefix.len() + suffix.len()
        && incoming.starts_with(prefix)
        && incoming.ends_with(suffix)
}

fn update_existing(
    tx: &Transaction<'_>,
    chat: &str,
    provider: &str,
    messages: &[ImportedMessage],
    counts: &mut ResultCounts,
) -> Result<(), String> {
    let mut query = tx.prepare("SELECT s.ordinal,s.source_message_id,s.fingerprint,m.role,m.text FROM chat_import_message_state s JOIN chat_messages m ON m.id=s.message_id WHERE s.chat_id=?1 ORDER BY s.ordinal").map_err(import_storage_error)?;
    let saved: Vec<(i64, Option<String>, String, String, String)> = query
        .query_map([chat], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .map_err(import_storage_error)?
        .collect::<Result<_, _>>()
        .map_err(import_storage_error)?;
    drop(query);
    let mut legacy = Vec::new();
    let baseline = if saved.is_empty() {
        let mut query = tx
            .prepare("SELECT id,role,text FROM chat_messages WHERE chat_id=?1 ORDER BY rowid")
            .map_err(import_storage_error)?;
        legacy = query
            .query_map([chat], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(import_storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(import_storage_error)?;
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
        for (index, (ordinal, source_id, hash, role, text)) in saved.iter().enumerate() {
            if *ordinal != index as i64 {
                counts.conflicts += 1;
                return Ok(());
            }
            if let Some(incoming) = messages.get(index) {
                let same = if hash == "redacted-v1" {
                    source_id.is_some()
                        && source_id == &incoming.source_id
                        && saved.iter().filter(|row| &row.1 == source_id).count() == 1
                        && messages
                            .iter()
                            .filter(|row| &row.source_id == source_id)
                            .count()
                            == 1
                        && role == &incoming.role
                        && matches_redacted_message(text, &incoming.text)
                } else {
                    hash == &incoming.fingerprint
                };
                if !same
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
    .filter(|s| !s.is_empty())
    .map(str::to_owned);
    let Some(source) = source else {
        counts.skipped += 1;
        return Ok(());
    };
    let existing_chat: Option<String> = tx
        .query_row(
            "SELECT chat_id FROM chat_imports WHERE source_provider=?1 AND source_id=?2",
            params![provider, &source],
            |row| row.get(0),
        )
        .optional()
        .map_err(import_storage_error)?;
    let title = obj
        .get(if provider == "chatgpt" {
            "title"
        } else {
            "name"
        })
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
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
    drop(value);
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
    tx.execute(
        "INSERT INTO chats(id,title,created_at,updated_at) VALUES(?1,?2,?3,?4)",
        params![id, title, created, updated],
    )
    .map_err(import_storage_error)?;
    tx.execute(
        "INSERT INTO chat_imports(chat_id,source_provider,source_id) VALUES(?1,?2,?3)",
        params![id, provider, source],
    )
    .map_err(import_storage_error)?;
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
    operation_error: &'a mut Option<String>,
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
        while let Some(raw) = seq.next_element::<Box<serde_json::value::RawValue>>()? {
            if self.cancelled.load(Ordering::SeqCst) {
                return Err(A::Error::custom("cancelled"));
            }
            let v: Value = serde_json::from_str(raw.get()).map_err(A::Error::custom)?;
            drop(raw);
            if let Err(error) = import_one(self.tx, self.provider, v, self.counts) {
                *self.operation_error = Some(error);
                return Err(A::Error::custom("import operation failed"));
            }
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
    let mut operation_error = None;
    ArraySeed {
        tx,
        provider,
        counts,
        cancelled,
        progress,
        processed,
        operation_error: &mut operation_error,
    }
    .deserialize(&mut de)
    .map_err(|_| {
        if cancelled.load(Ordering::SeqCst) {
            "cancelled".to_owned()
        } else {
            operation_error.unwrap_or_else(|| "invalid_format".to_owned())
        }
    })?;
    de.end().map_err(|_| "invalid_format".to_owned())
}

pub(crate) fn import(db: &Path, file: &Path, provider: &str) -> Result<ResultCounts, String> {
    import_controlled(db, file, provider, &AtomicBool::new(false), &|_| {})
}

#[derive(Debug)]
struct GeminiTurn {
    time: DateTime<Utc>,
    prompt: String,
    answer: Option<String>,
}

fn gemini_html_text(html: &str) -> String {
    let fragment = scraper::Html::parse_fragment(html);
    let mut result = String::new();
    for node in fragment.root_element().descendants() {
        match node.value() {
            scraper::Node::Text(text) => result.push_str(text),
            scraper::Node::Element(element)
                if matches!(element.name(), "p" | "div" | "br" | "li" | "pre")
                    && !result.is_empty()
                    && !result.ends_with('\n') =>
            {
                result.push('\n');
            }
            _ => {}
        }
    }
    result.trim().to_owned()
}

fn gemini_record(value: &Value) -> Option<GeminiTurn> {
    let is_gemini = value
        .get("header")
        .and_then(Value::as_str)
        .is_some_and(|s| s.to_lowercase().contains("gemini") || s.contains("제미나이"))
        || value
            .get("products")
            .and_then(Value::as_array)
            .is_some_and(|products| {
                products
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|s| s.to_lowercase().contains("gemini") || s.contains("제미나이"))
            })
        || value
            .get("titleUrl")
            .and_then(Value::as_str)
            .is_some_and(|s| s.starts_with("https://gemini.google.com/"));
    if !is_gemini {
        return None;
    }
    let details = value.get("details").and_then(Value::as_array);
    let detail = |name: &str| {
        details.and_then(|items| {
            items.iter().find_map(|item| {
                (item.get("name").and_then(Value::as_str) == Some(name))
                    .then(|| item.get("value").and_then(Value::as_str))
                    .flatten()
            })
        })
    };
    let title = value.get("title")?.as_str()?.trim();
    let prompt = detail("Request")
        .unwrap_or_else(|| title.strip_prefix("Prompted ").unwrap_or(title))
        .trim();
    let time = DateTime::parse_from_rfc3339(value.get("time")?.as_str()?)
        .ok()?
        .with_timezone(&Utc);
    if prompt.is_empty() || (detail("Request").is_none() && title == "Used Gemini Apps") {
        return None;
    }
    let answer = detail("Response").map(str::to_owned).or_else(|| {
        value
            .get("safeHtmlItem")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.get("html").and_then(Value::as_str))
                    .map(gemini_html_text)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|s| !s.is_empty())
    });
    Some(GeminiTurn {
        time,
        prompt: prompt.to_owned(),
        answer,
    })
}

fn gemini_parse<R: Read>(reader: R) -> Result<(Vec<GeminiTurn>, u64), String> {
    let mut bytes = Vec::new();
    reader
        .take(GEMINI_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "file_error")?;
    if bytes.len() as u64 > GEMINI_MAX_BYTES {
        return Err("gemini_file_too_large".into());
    }
    let items: Vec<Value> = serde_json::from_slice(&bytes).map_err(|_| "unsupported_format")?;
    let mut turns = Vec::new();
    let mut skipped = 0;
    for item in items {
        if let Some(turn) = gemini_record(&item) {
            turns.push(turn);
        } else {
            skipped += 1;
        }
    }
    if turns.is_empty() {
        return Err("unsupported_format".into());
    }
    turns.sort_by(|a, b| a.time.cmp(&b.time).then_with(|| a.prompt.cmp(&b.prompt)));
    Ok((turns, skipped))
}

// Google documents HTML activity records but not Gemini's HTML schema. This
// deliberately accepts only the publicly observed English Prompted layout.
fn gemini_html_time(value: &str) -> Option<DateTime<Utc>> {
    let normalized = value.replace(['\u{a0}', '\u{202f}'], " ");
    let (date, zone) = normalized.trim().rsplit_once(' ')?;
    let offset = match zone {
        "UTC" | "GMT" => "+0000".to_owned(),
        "PST" => "-0800".to_owned(),
        "PDT" => "-0700".to_owned(),
        "EST" => "-0500".to_owned(),
        "EDT" => "-0400".to_owned(),
        "KST" => "+0900".to_owned(),
        _ if zone.starts_with("GMT+") || zone.starts_with("GMT-") => zone[3..].replace(':', ""),
        _ => return None,
    };
    let text = format!("{} {}", date.replace("Sept ", "Sep "), offset);
    ["%b %e, %Y, %I:%M:%S %p %z", "%e %b %Y, %H:%M:%S %z"]
        .iter()
        .find_map(|format| DateTime::parse_from_str(&text, format).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn gemini_parse_html<R: Read>(reader: R) -> Result<(Vec<GeminiTurn>, u64), String> {
    let mut bytes = Vec::new();
    reader
        .take(GEMINI_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "file_error")?;
    if bytes.len() as u64 > GEMINI_MAX_BYTES {
        return Err("gemini_file_too_large".into());
    }
    let html = std::str::from_utf8(&bytes).map_err(|_| GEMINI_HTML_NEEDS_JSON)?;
    let document = scraper::Html::parse_document(html);
    let selector =
        scraper::Selector::parse(".outer-cell .content-cell").map_err(|_| "unsupported_format")?;
    let mut turns = Vec::new();
    let mut skipped = 0;
    let mut unknown_korean = false;
    for cell in document.select(&selector) {
        let mut text = String::new();
        for node in cell.descendants() {
            match node.value() {
                scraper::Node::Text(value) => text.push_str(value),
                scraper::Node::Element(value)
                    if matches!(value.name(), "br" | "p") && !text.ends_with('\n') =>
                {
                    text.push('\n')
                }
                _ => {}
            }
        }
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        let Some(first) = lines.first() else { continue };
        let Some(prompt) = first
            .strip_prefix("Prompted ")
            .or_else(|| first.strip_prefix("Prompted\u{a0}"))
        else {
            unknown_korean |= first.chars().any(|ch| ('가'..='힣').contains(&ch));
            if lines.iter().any(|line| gemini_html_time(line).is_some()) {
                skipped += 1;
            }
            continue;
        };
        let Some((index, time)) = lines
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(i, line)| gemini_html_time(line).map(|time| (i, time)))
        else {
            skipped += 1;
            continue;
        };
        let mut prompt = prompt.trim().to_owned();
        for line in &lines[1..index] {
            if *line != "Audio included." {
                prompt.push('\n');
                prompt.push_str(line);
            }
        }
        if prompt.is_empty() {
            skipped += 1;
            continue;
        }
        let answer = lines[index + 1..].join("\n");
        turns.push(GeminiTurn {
            time,
            prompt,
            answer: (!answer.is_empty()).then_some(answer),
        });
    }
    if turns.is_empty() || unknown_korean || skipped != 0 {
        return Err(GEMINI_HTML_NEEDS_JSON.into());
    }
    turns.sort_by(|a, b| a.time.cmp(&b.time).then_with(|| a.prompt.cmp(&b.prompt)));
    Ok((turns, skipped))
}

fn gemini_groups(turns: Vec<GeminiTurn>) -> Vec<Value> {
    let mut groups: Vec<Vec<GeminiTurn>> = Vec::new();
    for turn in turns {
        let same_group = groups.last().and_then(|g| g.last()).is_some_and(|last| {
            turn.time.date_naive() == last.time.date_naive()
                && turn.time.signed_duration_since(last.time).num_seconds()
                    <= GEMINI_GROUP_MINUTES * 60
        });
        if !same_group {
            groups.push(Vec::new());
        }
        groups.last_mut().unwrap().push(turn);
    }
    groups.into_iter().map(|group| {
        let first = &group[0];
        let id = format!("gemini-{}", first.time.to_rfc3339());
        let title: String = first.prompt.chars().take(80).collect();
        let created = first.time.to_rfc3339();
        let updated = group.last().unwrap().time.to_rfc3339();
        let mut messages = Vec::new();
        for turn in group {
            let time = turn.time.to_rfc3339();
            let message_id = format!("gemini-{time}");
            messages.push(serde_json::json!({"uuid":format!("{message_id}-user"), "sender":"human", "text":turn.prompt, "created_at":time}));
            if let Some(answer) = turn.answer {
                messages.push(serde_json::json!({"uuid":format!("{message_id}-assistant"), "sender":"assistant", "text":answer, "created_at":time}));
            }
        }
        serde_json::json!({"uuid":id, "name":title, "created_at":created, "updated_at":updated, "chat_messages":messages})
    }).collect()
}

fn gemini_load(file: &Path) -> Result<(Vec<Value>, u64), String> {
    let f = File::open(file).map_err(file_open_error)?;
    if file
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("html") || s.eq_ignore_ascii_case("htm"))
    {
        let (turns, skipped) = gemini_parse_html(f)?;
        return Ok((gemini_groups(turns), skipped));
    }
    if !file
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("zip"))
    {
        let (turns, skipped) = gemini_parse(f)?;
        return Ok((gemini_groups(turns), skipped));
    }
    let mut zip = zip::ZipArchive::new(f).map_err(|_| "unsupported_format")?;
    if zip.len() > MAX_ENTRIES {
        return Err("archive_limit".into());
    }
    let mut all = Vec::new();
    let mut skipped = 0;
    let mut total_bytes: u64 = 0;
    let mut html_needs_json = false;
    for json_pass in [true, false] {
        if !json_pass && !all.is_empty() {
            break;
        }
        for i in 0..zip.len() {
            let entry = zip.by_index(i).map_err(|_| "unsupported_format")?;
            let name = entry.name().rsplit('/').next().unwrap_or("");
            let is_json = name.eq_ignore_ascii_case("MyActivity.json");
            let is_html = name.eq_ignore_ascii_case("MyActivity.html")
                || name.eq_ignore_ascii_case("MyActivity.htm")
                || name == "내활동.html";
            if (json_pass && !is_json) || (!json_pass && !is_html) {
                continue;
            }
            if !json_pass
                && entry.name().contains('/')
                && !entry
                    .name()
                    .split('/')
                    .any(|part| part.to_lowercase().contains("gemini") || part.contains("제미나이"))
            {
                continue;
            }
            total_bytes = total_bytes.saturating_add(entry.size());
            if total_bytes > GEMINI_MAX_BYTES {
                return Err("gemini_file_too_large".into());
            }
            if entry.size() > GEMINI_MAX_BYTES {
                return Err("gemini_file_too_large".into());
            }
            match if is_json {
                gemini_parse(entry)
            } else {
                gemini_parse_html(entry)
            } {
                Ok((mut turns, count)) => {
                    all.append(&mut turns);
                    skipped += count;
                }
                Err(e) if e == "unsupported_format" => continue,
                Err(e) if e == GEMINI_HTML_NEEDS_JSON => {
                    html_needs_json = true;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }
    if all.is_empty() {
        if html_needs_json {
            return Err(GEMINI_HTML_NEEDS_JSON.into());
        }
        return Err("unsupported_format".into());
    }
    all.sort_by(|a, b| a.time.cmp(&b.time).then_with(|| a.prompt.cmp(&b.prompt)));
    Ok((gemini_groups(all), skipped))
}

#[derive(Default)]
struct GeminiPartition {
    groups: Vec<Value>,
    duplicate: u64,
    duplicate_groups: u64,
    conflicts: u64,
}

fn gemini_partition(db: &Connection, groups: Vec<Value>) -> Result<GeminiPartition, String> {
    let mut incoming: HashMap<String, usize> = HashMap::new();
    for message in groups
        .iter()
        .filter_map(|group| group.get("chat_messages").and_then(Value::as_array))
        .flatten()
    {
        let id = message
            .get("uuid")
            .and_then(Value::as_str)
            .ok_or("unsupported_format")?;
        *incoming.entry(id.to_owned()).or_default() += 1;
    }
    let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='chat_import_message_state')", [], |row| row.get(0)).map_err(import_storage_error)?;
    let mut known: HashMap<String, Vec<(String, String)>> = HashMap::new();
    if exists {
        let mut query = db.prepare("SELECT s.source_message_id,s.fingerprint,m.text FROM chat_import_message_state s JOIN chat_messages m ON m.id=s.message_id JOIN chat_imports i ON i.chat_id=s.chat_id WHERE i.source_provider='gemini' AND s.source_message_id IS NOT NULL").map_err(import_storage_error)?;
        let mut rows = query.query([]).map_err(import_storage_error)?;
        while let Some(row) = rows.next().map_err(import_storage_error)? {
            let id: String = row.get(0).map_err(import_storage_error)?;
            if incoming.contains_key(&id) {
                let fingerprint = row.get(1).map_err(import_storage_error)?;
                let text = row.get(2).map_err(import_storage_error)?;
                known.entry(id).or_default().push((fingerprint, text));
            }
        }
    }
    let mut result = GeminiPartition::default();
    for group in groups {
        let previous_duplicate = result.duplicate;
        let previous_conflicts = result.conflicts;
        let mut group_turns = 0;
        let messages = group
            .get("chat_messages")
            .and_then(Value::as_array)
            .ok_or("unsupported_format")?;
        let mut pending: Vec<Value> = Vec::new();
        let flush = |pending: &mut Vec<Value>,
                     result: &mut GeminiPartition|
         -> Result<(), String> {
            if pending.is_empty() {
                return Ok(());
            }
            let first = &pending[0];
            let time = first
                .get("created_at")
                .and_then(Value::as_str)
                .ok_or("unsupported_format")?;
            let prompt = first
                .get("text")
                .and_then(Value::as_str)
                .ok_or("unsupported_format")?;
            let last = pending
                .last()
                .and_then(|m| m.get("created_at"))
                .and_then(Value::as_str)
                .unwrap_or(time);
            result.groups.push(serde_json::json!({"uuid":format!("gemini-{time}"),"name":prompt.chars().take(80).collect::<String>(),"created_at":time,"updated_at":last,"chat_messages":pending}));
            pending.clear();
            Ok(())
        };
        let mut index = 0;
        while index < messages.len() {
            group_turns += 1;
            let user = &messages[index];
            let mut end = index + 1;
            if messages
                .get(end)
                .and_then(|m| m.get("sender"))
                .and_then(Value::as_str)
                == Some("assistant")
            {
                end += 1;
            }
            let matching = |item: &Value| -> Result<Option<bool>, String> {
                let id = item
                    .get("uuid")
                    .and_then(Value::as_str)
                    .ok_or("unsupported_format")?;
                let role = if item.get("sender").and_then(Value::as_str) == Some("human") {
                    "user"
                } else {
                    "assistant"
                };
                let text = item
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or("unsupported_format")?;
                let candidate = fingerprint(role, text);
                Ok(known.get(id).map(|saved| {
                    saved.iter().any(|(hash, previous)| {
                        hash == &candidate
                            || (hash == "redacted-v1"
                                && saved.len() == 1
                                && incoming.get(id) == Some(&1)
                                && matches_redacted_message(previous, text))
                    })
                }))
            };
            match matching(user)? {
                Some(true) => {
                    flush(&mut pending, &mut result)?;
                    if end > index + 1 && matching(&messages[index + 1])? != Some(true) {
                        result.conflicts += 1;
                    } else {
                        result.duplicate += 1;
                    }
                }
                Some(false) => {
                    flush(&mut pending, &mut result)?;
                    result.conflicts += 1;
                }
                None => {
                    for item in &messages[index..end] {
                        let item_id = item
                            .get("uuid")
                            .and_then(Value::as_str)
                            .ok_or("unsupported_format")?;
                        let role = if item.get("sender").and_then(Value::as_str) == Some("human") {
                            "user"
                        } else {
                            "assistant"
                        };
                        let item_text = item
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or("unsupported_format")?;
                        known
                            .entry(item_id.to_owned())
                            .or_default()
                            .push((fingerprint(role, item_text), item_text.to_owned()));
                        pending.push(item.clone());
                    }
                }
            }
            index = end;
        }
        flush(&mut pending, &mut result)?;
        if result.duplicate - previous_duplicate == group_turns
            && result.conflicts == previous_conflicts
        {
            result.duplicate_groups += 1;
        }
    }
    Ok(result)
}

pub(crate) fn import_controlled(
    db: &Path,
    file: &Path,
    provider: &str,
    cancelled: &AtomicBool,
    progress: &dyn Fn(u64),
) -> Result<ResultCounts, String> {
    if !matches!(provider, "chatgpt" | "claude" | "gemini") {
        return Err("unsupported_provider".into());
    }
    let mut db = Connection::open(db).map_err(import_storage_error)?;
    db.execute_batch("PRAGMA foreign_keys=ON")
        .map_err(import_storage_error)?;
    let tx = db.transaction().map_err(import_storage_error)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS chat_import_message_state (chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE, ordinal INTEGER NOT NULL, message_id TEXT NOT NULL REFERENCES chat_messages(id) ON DELETE CASCADE, source_message_id TEXT, fingerprint TEXT NOT NULL, PRIMARY KEY(chat_id,ordinal))").map_err(import_storage_error)?;
    let mut counts = ResultCounts::default();
    let mut processed = 0;
    if provider == "gemini" {
        counts.grouping_notice = Some(GEMINI_GROUPING_NOTICE);
        let (groups, skipped) = match gemini_load(file) {
            Ok(value) => value,
            Err(error) if error == GEMINI_HTML_NEEDS_JSON => {
                counts.guidance = Some(GEMINI_JSON_GUIDANCE);
                return Ok(counts);
            }
            Err(error) => return Err(error),
        };
        let partition = gemini_partition(&tx, groups)?;
        counts.skipped = skipped;
        counts.duplicate = partition.duplicate;
        counts.conflicts = partition.conflicts;
        for group in partition.groups {
            if cancelled.load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            import_one(&tx, provider, group, &mut counts)?;
            processed += 1;
            progress(processed);
        }
        if cancelled.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        tx.commit().map_err(import_storage_error)?;
        return Ok(counts);
    }
    let f = File::open(file).map_err(file_open_error)?;
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
            parse(
                entry,
                &tx,
                provider,
                &mut counts,
                cancelled,
                progress,
                &mut processed,
            )?;
        }
        if !matched {
            return Err("unsupported_format".into());
        }
    } else {
        parse(
            f,
            &tx,
            provider,
            &mut counts,
            cancelled,
            progress,
            &mut processed,
        )?;
    }
    if cancelled.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    tx.commit().map_err(import_storage_error)?;
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
    #[serde(skip_serializing_if = "Option::is_none")]
    gemini_new_items: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gemini_duplicate_items: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gemini_conflicting_items: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    grouping_notice: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) guidance: Option<&'static str>,
}

struct PreviewSeed<'a> {
    db: &'a Connection,
    provider: &'a str,
    result: &'a mut Preview,
    operation_error: &'a mut Option<String>,
}
fn readable_conversation(v: &Value, provider: &str) -> bool {
    if matches!(provider, "claude" | "gemini") {
        return v
            .get("chat_messages")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|m| {
                    matches!(
                        m.get("sender").and_then(Value::as_str),
                        Some("human" | "assistant")
                    ) && m
                        .get("content")
                        .or_else(|| m.get("text"))
                        .and_then(text_parts)
                        .or_else(|| m.get("text").and_then(Value::as_str).map(str::to_owned))
                        .is_some_and(|t| !t.is_empty())
                })
            });
    }
    let Some(mapping) = v.get("mapping").and_then(Value::as_object) else {
        return false;
    };
    let mut current = v.get("current_node").and_then(Value::as_str);
    let mut seen = HashSet::new();
    let mut has_text = false;
    while let Some(id) = current {
        if !seen.insert(id) {
            return false;
        }
        let Some(node) = mapping.get(id) else {
            return false;
        };
        if let Some(m) = node.get("message") {
            if matches!(
                m.pointer("/author/role").and_then(Value::as_str),
                Some("user" | "assistant")
            ) && m
                .get("content")
                .or_else(|| m.get("text"))
                .and_then(text_parts)
                .or_else(|| m.get("text").and_then(Value::as_str).map(str::to_owned))
                .is_some_and(|t| !t.is_empty())
            {
                has_text = true;
            }
        }
        current = node.get("parent").and_then(Value::as_str);
    }
    has_text
}
impl<'de> DeserializeSeed<'de> for PreviewSeed<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for PreviewSeed<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("conversation array")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(raw) = seq.next_element::<Box<serde_json::value::RawValue>>()? {
            let v: Value = serde_json::from_str(raw.get()).map_err(A::Error::custom)?;
            drop(raw);
            self.result.conversations += 1;
            let source = if self.provider == "chatgpt" {
                v.get("conversation_id").or_else(|| v.get("id"))
            } else {
                v.get("uuid").or_else(|| v.get("id"))
            }
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
            let valid = readable_conversation(&v, self.provider);
            let Some(source) = source.filter(|_| valid) else {
                self.result.unreadable += 1;
                continue;
            };
            let exists: bool = match self.db.query_row("SELECT EXISTS(SELECT 1 FROM chat_imports WHERE source_provider=?1 AND source_id=?2)", params![self.provider, source], |r| r.get(0)) {
                Ok(exists) => exists,
                Err(error) => {
                    *self.operation_error = Some(import_storage_error(error).to_owned());
                    return Err(A::Error::custom("preview operation failed"));
                }
            };
            if exists {
                self.result.already_imported += 1;
            }
            let date = timestamp(
                v.get(if self.provider == "chatgpt" {
                    "create_time"
                } else {
                    "created_at"
                })
                .unwrap_or(&Value::Null),
            );
            if let Ok(parsed) = DateTime::parse_from_rfc3339(&date) {
                let normalized = parsed.with_timezone(&Utc).to_rfc3339();
                if self.result.oldest.as_ref().is_none_or(|d| normalized < *d) {
                    self.result.oldest = Some(normalized.clone());
                }
                if self.result.newest.as_ref().is_none_or(|d| normalized > *d) {
                    self.result.newest = Some(normalized);
                }
            }
        }
        Ok(())
    }
}
fn preview_parse<R: Read>(
    reader: R,
    db: &Connection,
    provider: &str,
    result: &mut Preview,
) -> Result<(), String> {
    let mut de = serde_json::Deserializer::from_reader(BufReader::new(reader));
    let mut operation_error = None;
    PreviewSeed {
        db,
        provider,
        result,
        operation_error: &mut operation_error,
    }
    .deserialize(&mut de)
    .map_err(|_| operation_error.unwrap_or_else(|| "invalid_format".to_string()))?;
    de.end().map_err(|_| "invalid_format".to_string())
}
pub(crate) fn preview(db: &Path, file: &Path, provider: &str) -> Result<Preview, String> {
    if !matches!(provider, "chatgpt" | "claude" | "gemini") {
        return Err("unsupported_provider".into());
    }
    let db = Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(import_storage_error)?;
    if provider == "gemini" {
        let (groups, unreadable) = match gemini_load(file) {
            Ok(value) => value,
            Err(error) if error == GEMINI_HTML_NEEDS_JSON => {
                return Ok(Preview {
                    guidance: Some(GEMINI_JSON_GUIDANCE),
                    ..Preview::default()
                })
            }
            Err(error) => return Err(error),
        };
        let mut result = Preview {
            unreadable,
            conversations: groups.len() as u64,
            grouping_notice: Some(GEMINI_GROUPING_NOTICE),
            ..Preview::default()
        };
        for group in &groups {
            for (field, target) in [
                ("created_at", &mut result.oldest),
                ("updated_at", &mut result.newest),
            ] {
                if let Some(date) = group.get(field).and_then(Value::as_str) {
                    if target.as_ref().is_none_or(|old| {
                        if field == "created_at" {
                            date < old.as_str()
                        } else {
                            date > old.as_str()
                        }
                    }) {
                        *target = Some(date.to_owned());
                    }
                }
            }
        }
        let partition = gemini_partition(&db, groups)?;
        result.already_imported = partition.duplicate_groups;
        result.gemini_new_items = Some(
            partition
                .groups
                .iter()
                .filter_map(|g| g.get("chat_messages").and_then(Value::as_array))
                .map(|messages| {
                    messages
                        .iter()
                        .filter(|m| m.get("sender").and_then(Value::as_str) == Some("human"))
                        .count() as u64
                })
                .sum(),
        );
        result.gemini_duplicate_items = Some(partition.duplicate);
        result.gemini_conflicting_items = Some(partition.conflicts);
        return Ok(result);
    }
    let f = File::open(file).map_err(file_open_error)?;
    let mut result = Preview::default();
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
            if !(name == "conversations.json"
                || (name.starts_with("conversations-") && name.ends_with(".json"))
                || (name.starts_with("conversations_") && name.ends_with(".json")))
            {
                continue;
            }
            if entry.size() > MAX_ENTRY {
                return Err("archive_limit".into());
            }
            matched = true;
            preview_parse(entry, &db, provider, &mut result)?;
        }
        if !matched {
            return Err("unsupported_format".into());
        }
    } else {
        preview_parse(f, &db, provider, &mut result)?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    #[test]
    fn masked_import_comparison_rejects_changed_context_or_ambiguous_markers() {
        let masked = "앞 [비밀값으로 보여 가림: 저장된 값] 뒤";
        assert!(super::matches_redacted_message(
            masked,
            "앞 fake-example 뒤"
        ));
        assert!(!super::matches_redacted_message(
            masked,
            "변경 fake-example 뒤"
        ));
        assert!(!super::matches_redacted_message(
            "[비밀값으로 보여 가림: API 키] [비밀값으로 보여 가림: 토큰]",
            "one two"
        ));
    }
    use super::*;
    use std::io::Write;

    #[test]
    fn import_storage_errors_distinguish_actionable_failures() {
        let error = |code| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None);
        assert_eq!(
            import_storage_error(error(rusqlite::ffi::SQLITE_BUSY)),
            "storage_busy"
        );
        assert_eq!(
            import_storage_error(error(rusqlite::ffi::SQLITE_READONLY)),
            "storage_permission_denied"
        );
        assert_eq!(
            import_storage_error(error(rusqlite::ffi::SQLITE_FULL)),
            "storage_full"
        );
        assert_eq!(
            import_storage_error(error(rusqlite::ffi::SQLITE_CORRUPT)),
            "storage_corrupt"
        );
        assert_eq!(
            import_storage_error(error(rusqlite::ffi::SQLITE_ERROR)),
            "storage_error"
        );
    }

    #[test]
    fn import_and_preview_keep_database_failures_distinct_from_bad_json() {
        let mut db = Connection::open_in_memory().unwrap();
        let data = br#"[{"uuid":"test","chat_messages":[{"sender":"human","text":"hello"}]}]"#;
        let tx = db.transaction().unwrap();
        let mut counts = ResultCounts::default();
        let mut processed = 0;
        assert_eq!(
            parse(
                &data[..],
                &tx,
                "claude",
                &mut counts,
                &AtomicBool::new(false),
                &|_| {},
                &mut processed
            ),
            Err("storage_error".into())
        );
        drop(tx);
        assert_eq!(
            preview_parse(&data[..], &db, "claude", &mut Preview::default()),
            Err("storage_error".into())
        );
    }

    #[test]
    fn import_file_errors_distinguish_missing_and_permission() {
        use std::io::{Error, ErrorKind};
        assert_eq!(
            file_open_error(Error::from(ErrorKind::NotFound)),
            "file_not_found"
        );
        assert_eq!(
            file_open_error(Error::from(ErrorKind::PermissionDenied)),
            "file_permission_denied"
        );
        assert_eq!(file_open_error(Error::from(ErrorKind::Other)), "file_error");
    }

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("momso.db");
        crate::storage::initialize(&db, &dir.path().join("backup"), "test").unwrap();
        (dir, db)
    }

    #[test]
    fn gemini_json_preview_import_repeat_and_cancellation() {
        let (dir, db) = fixture();
        let file = dir.path().join("MyActivity.json");
        let item = |prompt: &str, time: &str| {
            serde_json::json!({
                "header":"Gemini Apps", "products":["Gemini"], "title":format!("Prompted {prompt}"),
                "time":time, "safeHtmlItem":[{"html":"<p>Answer &amp; details</p>"}], "unknown":true
            })
        };
        std::fs::write(
            &file,
            serde_json::json!([
                item("First question", "2026-01-01T10:00:00Z"),
                item("Second question", "2026-01-01T10:25:00Z"),
                item("Later question", "2026-01-01T11:00:00Z")
            ])
            .to_string(),
        )
        .unwrap();
        let p = preview(&db, &file, "gemini").unwrap();
        assert_eq!(p.conversations, 2);
        let cancelled = AtomicBool::new(false);
        assert!(
            matches!(import_controlled(&db, &file, "gemini", &cancelled, &|_| cancelled.store(true, Ordering::SeqCst)), Err(e) if e == "cancelled")
        );
        assert_eq!(
            Connection::open(&db)
                .unwrap()
                .query_row("SELECT count(*) FROM chats", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(import(&db, &file, "gemini").unwrap().imported, 2);
        assert_eq!(import(&db, &file, "gemini").unwrap().duplicate, 3);
        assert_eq!(preview(&db, &file, "gemini").unwrap().already_imported, 2);
        let connection = Connection::open(&db).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM chat_messages WHERE text='Answer & details'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            3
        );
        drop(connection);
        let root = dir.path();
        let archive_root = root.join("momso");
        let archive =
            crate::archive_location::ArchiveLocation::load(db.clone(), root, vec![]).unwrap();
        crate::archive::initialize(&archive_root, root).unwrap();
        let paths = crate::runtime::RuntimePaths {
            data_root: root.to_path_buf(),
            database_path: db.clone(),
            objects_path: archive_root.join("objects"),
            exports_path: root.join("exports"),
            backups_path: root.join("backup"),
            logs_path: root.join("logs"),
            archive,
            archive_lease: None,
        };
        crate::chat_archive_store::write_all(&archive_root, &paths).unwrap();
        assert_eq!(
            std::fs::read_dir(archive_root.join("chats"))
                .unwrap()
                .count(),
            2
        );
    }

    #[test]
    fn gemini_details_export_imports_request_response_and_time() {
        let (dir, db) = fixture();
        let file = dir.path().join("MyActivity.json");
        std::fs::write(&file, serde_json::json!([{
            "header":"Gemini", "title":"Used Gemini Apps",
            "titleUrl":"https://gemini.google.com/app/c/example",
            "time":"2024-02-17T22:05:10.123Z", "products":["Gemini Apps"],
            "details":[{"name":"Request","value":"A question"},{"name":"Response","value":"An answer"}]
        }]).to_string()).unwrap();
        assert_eq!(import(&db, &file, "gemini").unwrap().imported, 1);
        let c = Connection::open(&db).unwrap();
        let mut q = c
            .prepare("SELECT role,text,created_at FROM chat_messages ORDER BY rowid")
            .unwrap();
        let messages: Vec<(String, String, String)> = q
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(messages.len(), 2);
        assert_eq!((&*messages[0].0, &*messages[0].1), ("user", "A question"));
        assert_eq!(
            (&*messages[1].0, &*messages[1].1),
            ("assistant", "An answer")
        );
        assert!(messages
            .iter()
            .all(|m| m.2.starts_with("2024-02-17T22:05:10.123")));
    }

    #[test]
    fn gemini_rejects_empty_broken_and_other_google_activity() {
        for bytes in [
            &b"[]"[..],
            &b"{"[..],
            &b"[{\"header\":\"Search\",\"title\":\"query\",\"time\":\"2026-01-01T00:00:00Z\"}]"[..],
        ] {
            assert!(matches!(gemini_parse(bytes), Err(e) if e == "unsupported_format"));
        }
    }

    #[test]
    fn gemini_reexport_with_earlier_and_later_turns_has_no_duplicates() {
        let (dir, db) = fixture();
        let file = dir.path().join("MyActivity.json");
        let turn = |n: i64| serde_json::json!({"header":"Gemini Apps", "title":format!("Prompted question {n}"), "time":DateTime::from_timestamp(1_800_000_000 + n * 60, 0).unwrap().to_rfc3339(), "safeHtmlItem":[{"html":format!("<p>answer {n}</p>")}]});
        std::fs::write(
            &file,
            serde_json::json!((5..15).map(turn).collect::<Vec<_>>()).to_string(),
        )
        .unwrap();
        assert_eq!(import(&db, &file, "gemini").unwrap().imported, 1);
        std::fs::write(
            &file,
            serde_json::json!((0..18).map(turn).collect::<Vec<_>>()).to_string(),
        )
        .unwrap();
        let preview = preview(&db, &file, "gemini").unwrap();
        assert_eq!(preview.gemini_new_items, Some(8));
        assert_eq!(preview.gemini_duplicate_items, Some(10));
        let counts = import(&db, &file, "gemini").unwrap();
        assert_eq!(
            (counts.imported, counts.duplicate, counts.conflicts),
            (2, 10, 0)
        );
        let db = Connection::open(&db).unwrap();
        let (count, unique): (i64, i64) = db
            .query_row(
                "SELECT COUNT(*),COUNT(DISTINCT text) FROM chat_messages WHERE role='user'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((count, unique), (18, 18));
    }

    #[test]
    fn gemini_same_timestamp_and_changed_answers_are_conflicts() {
        let (dir, db) = fixture();
        let file = dir.path().join("MyActivity.json");
        let turn = |prompt: &str, answer: &str| serde_json::json!({"header":"Gemini Apps", "title":format!("Prompted {prompt}"), "time":"2026-01-01T10:00:00Z", "safeHtmlItem":[{"html":format!("<p>{answer}</p>")}]});
        std::fs::write(
            &file,
            serde_json::json!([turn("first", "original")]).to_string(),
        )
        .unwrap();
        assert_eq!(import(&db, &file, "gemini").unwrap().imported, 1);
        std::fs::write(
            &file,
            serde_json::json!([turn("first", "changed"), turn("other", "answer")]).to_string(),
        )
        .unwrap();
        let counts = import(&db, &file, "gemini").unwrap();
        assert_eq!(
            (counts.imported, counts.duplicate, counts.conflicts),
            (0, 0, 2)
        );
        let db = Connection::open(&db).unwrap();
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM chat_messages", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn gemini_redacted_match_requires_unique_timestamp() {
        let (dir, db) = fixture();
        let file = dir.path().join("MyActivity.json");
        let turn = |prompt: &str| serde_json::json!({"header":"Gemini Apps", "title":format!("Prompted {prompt}"), "time":"2026-01-01T10:00:00Z"});
        std::fs::write(&file, serde_json::json!([turn("common first")]).to_string()).unwrap();
        assert_eq!(import(&db, &file, "gemini").unwrap().imported, 1);
        let connection = Connection::open(&db).unwrap();
        connection
            .execute(
                "UPDATE chat_messages SET text=?1 WHERE role='user'",
                ["common [비밀값으로 보여 가림: 저장된 값]"],
            )
            .unwrap();
        connection.execute("UPDATE chat_import_message_state SET fingerprint='redacted-v1' WHERE message_id IN (SELECT id FROM chat_messages WHERE role='user')", []).unwrap();
        drop(connection);
        assert_eq!(
            preview(&db, &file, "gemini")
                .unwrap()
                .gemini_duplicate_items,
            Some(1)
        );
        std::fs::write(
            &file,
            serde_json::json!([turn("common first"), turn("common second")]).to_string(),
        )
        .unwrap();
        let counts = import(&db, &file, "gemini").unwrap();
        assert_eq!(
            (counts.imported, counts.duplicate, counts.conflicts),
            (0, 0, 2)
        );
    }

    #[test]
    fn gemini_html_and_size_have_specific_reasons() {
        use std::io::Write;
        let (dir, db) = fixture();
        let html = dir.path().join("MyActivity.html");
        std::fs::write(&html, "<html lang='ko'><div class='outer-cell'><div class='content-cell'>알 수 없는 활동<br>2026. 1. 1. 오후 1:00:00</div></div></html>").unwrap();
        let result = preview(&db, &html, "gemini").unwrap();
        assert_eq!(result.guidance, Some(GEMINI_JSON_GUIDANCE));
        assert_eq!(result.conversations, 0);
        let result = import(&db, &html, "gemini").unwrap();
        assert_eq!(result.guidance, Some(GEMINI_JSON_GUIDANCE));
        assert_eq!(result.imported, 0);
        assert_eq!(
            Connection::open(&db)
                .unwrap()
                .query_row("SELECT COUNT(*) FROM chats", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        let zip_path = dir.path().join("activity.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        zip.start_file(
            "Takeout/My Activity/Gemini Apps/MyActivity.html",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        write!(zip, "<html></html>").unwrap();
        zip.finish().unwrap();
        assert_eq!(
            preview(&db, &zip_path, "gemini").unwrap().guidance,
            Some(GEMINI_JSON_GUIDANCE)
        );
        assert_eq!(
            import(&db, &zip_path, "gemini").unwrap().guidance,
            Some(GEMINI_JSON_GUIDANCE)
        );
        assert!(
            matches!(gemini_parse(std::io::repeat(b'x').take(GEMINI_MAX_BYTES+1)), Err(e) if e == "gemini_file_too_large")
        );
    }

    #[test]
    fn gemini_html_import_is_deduplicated_and_reports_grouping() {
        use std::io::Write;
        let (dir, db) = fixture();
        let html = "<div class='outer-cell'><div class='content-cell'>Prompted\u{a0}How are you?<br>Mar 10, 2026, 10:25:03 PM PDT<br><p>I am well.</p></div><div class='content-cell'>meta</div></div>";
        let file = dir.path().join("MyActivity.html");
        std::fs::write(&file, html).unwrap();
        let preview = serde_json::to_value(preview(&db, &file, "gemini").unwrap()).unwrap();
        assert_eq!(preview["groupingNotice"], GEMINI_GROUPING_NOTICE);
        let first = import(&db, &file, "gemini").unwrap();
        assert_eq!(first.imported, 1);
        assert_eq!(first.grouping_notice, Some(GEMINI_GROUPING_NOTICE));
        let second = import(&db, &file, "gemini").unwrap();
        assert_eq!((second.imported, second.duplicate), (0, 1));
        let c = Connection::open(&db).unwrap();
        let mut q = c
            .prepare("SELECT role,text,created_at FROM chat_messages ORDER BY rowid")
            .unwrap();
        let messages: Vec<(String, String, String)> = q
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(messages.len(), 2);
        assert_eq!((&*messages[0].0, &*messages[0].1), ("user", "How are you?"));
        assert_eq!(
            (&*messages[1].0, &*messages[1].1),
            ("assistant", "I am well.")
        );
        assert!(messages
            .iter()
            .all(|m| m.2.starts_with("2026-03-11T05:25:03")));
        let path = dir.path().join("activity.zip");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        zip.start_file(
            "Takeout/My Activity/Search/MyActivity.json",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        write!(
            zip,
            "[{{\"header\":\"Search\",\"title\":\"Searched\",\"time\":\"2026-03-01T00:00:00Z\"}}]"
        )
        .unwrap();
        zip.start_file(
            "Takeout/My Activity/Gemini Apps/MyActivity.html",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        write!(zip, "{html}").unwrap();
        zip.finish().unwrap();
        assert_eq!(import(&db, &path, "gemini").unwrap().duplicate, 1);
    }

    #[test]
    #[ignore = "set MOMSO_IMPORT_BENCH_FILE and MOMSO_IMPORT_BENCH_PROVIDER for a synthetic large export"]
    fn benchmark_synthetic_large_export() {
        let file = std::env::var_os("MOMSO_IMPORT_BENCH_FILE").expect("file");
        let provider = std::env::var("MOMSO_IMPORT_BENCH_PROVIDER").expect("provider");
        let (_dir, db) = fixture();
        let counts = import(&db, Path::new(&file), &provider).unwrap();
        assert_eq!(counts.imported, 1);
    }

    #[test]
    fn gemini_zip_accepts_localized_folders() {
        use std::io::Write;
        let (dir, db) = fixture();
        for folder in ["My Activity/Gemini Apps", "내 활동/Gemini 앱"] {
            let archive = dir.path().join("gemini.zip");
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
            zip.start_file(
                format!("Takeout/{folder}/MyActivity.json"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            write!(zip, "{}", serde_json::json!([{"header":"Gemini Apps","title":"Prompted example", "time":"2026-01-01T00:00:00Z", "safeHtmlItem":[{"html":"<p>one</p><p>two</p>"}]}])).unwrap();
            zip.finish().unwrap();
            assert_eq!(preview(&db, &archive, "gemini").unwrap().conversations, 1);
            let (groups, _) = gemini_load(&archive).unwrap();
            assert_eq!(groups[0]["chat_messages"][1]["text"], "one\ntwo");
        }
    }

    #[test]
    #[ignore = "set MOMSO_GEMINI_EXPORT to inspect a real export without printing its contents"]
    fn inspect_gemini_export_counts_only() {
        let path = std::env::var_os("MOMSO_GEMINI_EXPORT").expect("MOMSO_GEMINI_EXPORT");
        match gemini_load(Path::new(&path)) {
            Ok((groups, unreadable)) => {
                let items: usize = groups
                    .iter()
                    .filter_map(|group| group.get("chat_messages").and_then(Value::as_array))
                    .map(|messages| {
                        messages
                            .iter()
                            .filter(|message| {
                                message.get("sender").and_then(Value::as_str) == Some("human")
                            })
                            .count()
                    })
                    .sum();
                println!("items={items} groups={} unreadable={unreadable} reason=unrecognized_or_missing_fields", groups.len())
            }
            Err(reason) => println!("groups=0 unreadable=unknown reason={reason}"),
        }
    }

    #[test]
    fn preview_counts_without_writing_and_cancel_rolls_back() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let item = |id: &str, date: &str| serde_json::json!({"uuid":id,"created_at":date,"chat_messages":[{"sender":"human","text":"private contents"}]});
        std::fs::write(
            &file,
            serde_json::json!([item("a", "2020-01-01T00:00:00Z")]).to_string(),
        )
        .unwrap();
        import(&db, &file, "claude").unwrap();
        std::fs::write(&file, serde_json::json!([item("a","2020-01-01T00:00:00Z"),item("b","2024-01-01T00:00:00Z"),{"uuid":"broken"}]).to_string()).unwrap();
        let p = preview(&db, &file, "claude").unwrap();
        assert_eq!(
            (p.conversations, p.already_imported, p.unreadable),
            (3, 1, 1)
        );
        assert!(p.oldest.unwrap().starts_with("2020-01-01"));
        assert!(p.newest.unwrap().starts_with("2024-01-01"));
        let count = || {
            Connection::open(&db)
                .unwrap()
                .query_row("SELECT count(*) FROM chats", [], |r| r.get::<_, i64>(0))
                .unwrap()
        };
        assert_eq!(count(), 1);
        let cancelled = AtomicBool::new(false);
        let result = import_controlled(&db, &file, "claude", &cancelled, &|n| {
            if n == 1 {
                cancelled.store(true, Ordering::SeqCst);
            }
        });
        assert!(matches!(result, Err(e) if e == "cancelled"));
        assert_eq!(count(), 1);
        assert!(matches!(preview(&db, &file, "gemini"), Err(e) if e == "unsupported_format"));
    }

    #[test]
    fn progress_request_cancels_without_saving_and_cleans_registry() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        std::fs::write(
            &file,
            serde_json::json!([
                {"uuid":"one","chat_messages":[{"sender":"human","text":"one"}]},
                {"uuid":"two","chat_messages":[{"sender":"human","text":"two"}]}
            ])
            .to_string(),
        )
        .unwrap();
        let id = format!("test-{}", Uuid::new_v4());
        let result = import_with_id(&db, &file, "claude", &id, &|n| {
            if n == 1 {
                cancel(&id);
            }
        });
        assert!(matches!(result, Err(e) if e == "cancelled"));
        assert_eq!(
            Connection::open(&db)
                .unwrap()
                .query_row("SELECT count(*) FROM chats", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
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
        assert_eq!(detail["messages"][1]["provider"], "codex");
        assert_eq!(detail["messages"].as_array().unwrap().len(), 2);
        let connection = Connection::open(&db).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT provider FROM chat_messages WHERE role='assistant'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "codex"
        );
        connection
            .execute(
                "UPDATE chat_messages SET provider='chatgpt' WHERE role='assistant'",
                [],
            )
            .unwrap();
        drop(connection);
        let legacy = serde_json::to_value(crate::chat::read(&db, id).unwrap()).unwrap();
        assert_eq!(legacy["messages"][1]["provider"], "codex");
        assert_eq!(legacy["importedFrom"], "chatgpt");
        crate::chat::set_archived(&db, id, true);
        let archived = serde_json::to_value(crate::chat::archived_list(&db).unwrap()).unwrap();
        assert_eq!(archived[0]["provider"], "codex");
        let snapshot = crate::chat_archive_snapshot::snapshot(&db, id).unwrap();
        assert_eq!(snapshot["messages"][1]["provider"], "codex");
        assert_eq!(snapshot["imports"][0]["source_provider"], "chatgpt");
        let mut old_snapshot = snapshot.clone();
        old_snapshot["messages"][1]["provider"] = serde_json::json!("chatgpt");
        assert!(crate::chat_archive_snapshot::merge_snapshots(&snapshot, &old_snapshot).is_ok());
        assert_eq!(
            Connection::open(&db)
                .unwrap()
                .query_row(
                    "SELECT provider FROM chat_messages WHERE role='assistant'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "chatgpt"
        );
        let resumed = crate::chat_handoff::prepare(&db, id, "codex", false, "계속").unwrap();
        assert!(resumed.prompt.contains("  질문\n") && resumed.prompt.contains("답"));
        assert!(crate::chat_codex::supported("codex"));
        let handoff = crate::chat_handoff::prepare(&db, id, "claude", false, "이어 질문").unwrap();
        assert!(handoff.prompt.contains("  질문\n"));
        assert!(handoff.prompt.contains("답"));
        assert!(!handoff.prompt.contains("다른 가지"));
    }

    #[test]
    fn imported_claude_and_gemini_keep_canonical_provider_and_source() {
        let (dir, db) = fixture();
        let claude = dir.path().join("claude.json");
        std::fs::write(&claude, serde_json::json!([{"uuid":"claude-room","chat_messages":[{"sender":"assistant","text":"answer"}]}]).to_string()).unwrap();
        import(&db, &claude, "claude").unwrap();
        let gemini = dir.path().join("gemini.json");
        std::fs::write(&gemini, serde_json::json!([{"header":"Gemini Apps","title":"Prompted question","time":"2026-01-01T10:00:00Z","safeHtmlItem":[{"html":"<p>answer</p>"}]}]).to_string()).unwrap();
        import(&db, &gemini, "gemini").unwrap();
        for room in serde_json::to_value(crate::chat::list(&db).unwrap())
            .unwrap()
            .as_array()
            .unwrap()
        {
            let source = room["importedFrom"].as_str().unwrap();
            let detail =
                serde_json::to_value(crate::chat::read(&db, room["id"].as_str().unwrap()).unwrap())
                    .unwrap();
            let assistant = detail["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "assistant")
                .unwrap();
            assert_eq!(assistant["provider"], source);
            assert!(assistant["model"].is_null());
        }
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
