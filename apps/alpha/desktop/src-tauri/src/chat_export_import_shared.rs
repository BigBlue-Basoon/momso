// Import known conversation JSON without sending its contents to a model.
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
    let id = crate::chat::new_import_id();
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
    cancelled: &'a AtomicBool,
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
        loop {
            if self.cancelled.load(Ordering::SeqCst) {
                return Err(A::Error::custom("cancelled"));
            }
            let Some(raw) = seq.next_element::<Box<serde_json::value::RawValue>>()? else {
                break;
            };
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
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let mut de = serde_json::Deserializer::from_reader(BufReader::new(reader));
    let mut operation_error = None;
    PreviewSeed {
        db,
        provider,
        cancelled,
        result,
        operation_error: &mut operation_error,
    }
    .deserialize(&mut de)
    .map_err(|_| {
        if cancelled.load(Ordering::SeqCst) {
            "cancelled".to_string()
        } else {
            operation_error.unwrap_or_else(|| "invalid_format".to_string())
        }
    })?;
    de.end().map_err(|_| "invalid_format".to_string())
}
pub(crate) fn preview(db: &Path, file: &Path, provider: &str) -> Result<Preview, String> {
    preview_controlled(db, file, provider, &AtomicBool::new(false))
}
pub(crate) fn preview_controlled(
    db: &Path,
    file: &Path,
    provider: &str,
    cancelled: &AtomicBool,
) -> Result<Preview, String> {
    if cancelled.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
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
        if cancelled.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
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
            if cancelled.load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
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
            preview_parse(entry, &db, provider, &mut result, cancelled)?;
        }
        if !matched {
            return Err("unsupported_format".into());
        }
    } else {
        preview_parse(f, &db, provider, &mut result, cancelled)?;
    }
    Ok(result)
}
