use chrono::{Local, SecondsFormat};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path, time::Duration};
use uuid::Uuid;

pub(crate) fn canonical_provider(value: Option<&str>) -> Option<&str> {
    match value {
        Some("chatgpt" | "codex") => Some("codex"),
        Some("claude") => Some("claude"),
        Some("gemini") => Some("gemini"),
        _ => None,
    }
}

pub(crate) fn new_import_id() -> String {
    format!("chat_{}", uuid::Uuid::new_v4())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatSummary {
    id: String,
    title: String,
    created_at: Option<String>,
    updated_at: Option<String>,
    message_count: i64,
    group_id: Option<String>,
    pinned: bool,
    pinned_order: Option<i64>,
    imported_from: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatMessage {
    pub(crate) id: String,
    role: Role,
    text: String,
    created_at: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    actual_effort: Option<String>,
    actual_special_effort: Option<String>,
    skill: Option<String>,
    fast: Option<bool>,
    fast_state: Option<String>,
    not_handed_over: Option<i64>,
    finish_reason: Option<FinishReason>,
    auth_method: Option<String>,
    attachments: Vec<ChatReadAttachment>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatReadAttachment {
    asset_id: String,
    name: String,
    media_type: Option<String>,
    deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FinishReason {
    Completed,
    LengthLimit,
    TurnLimit,
    Cancelled,
    Error,
    Refusal,
}
impl FinishReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::LengthLimit => "length_limit",
            Self::TurnLimit => "turn_limit",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
            Self::Refusal => "refusal",
        }
    }
    fn from_db(value: String) -> Result<Self, rusqlite::Error> {
        match value.as_str() {
            "completed" => Ok(Self::Completed),
            "length_limit" => Ok(Self::LengthLimit),
            "turn_limit" => Ok(Self::TurnLimit),
            "cancelled" => Ok(Self::Cancelled),
            "error" => Ok(Self::Error),
            "refusal" => Ok(Self::Refusal),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    User,
    Assistant,
}
impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
    fn from_db(s: String) -> Result<Self, rusqlite::Error> {
        match s.as_str() {
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatDetail {
    id: String,
    title: String,
    created_at: Option<String>,
    updated_at: Option<String>,
    imported_from: Option<String>,
    messages: Vec<ChatMessage>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatPage {
    #[serde(flatten)]
    detail: ChatDetail,
    next_cursor: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatSearchResult {
    #[serde(flatten)]
    chat: ChatSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<String>,
}
fn db(path: &Path) -> Result<Connection, String> {
    let c = Connection::open(path).map_err(|_| "채팅방 저장소를 열지 못했습니다".to_string())?;
    c.busy_timeout(Duration::from_secs(5))
        .map_err(|_| "채팅방 저장소를 열지 못했습니다".to_string())?;
    c.execute_batch("PRAGMA foreign_keys=ON")
        .map_err(|_| "채팅방 저장소를 열지 못했습니다".to_string())?;
    Ok(c)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeleteResult {
    state: &'static str,
    codex_session: Option<&'static str>,
    claude_session: Option<&'static str>,
}

fn delete_result(
    state: &'static str,
    codex_session: Option<&'static str>,
    claude_session: Option<&'static str>,
) -> DeleteResult {
    DeleteResult {
        state,
        codex_session,
        claude_session,
    }
}

fn delete_sessions(path: &Path, id: &str) -> Result<Option<(Option<String>, bool)>, String> {
    db(path)?.query_row("SELECT codex_thread_id, EXISTS(SELECT 1 FROM chat_claude_sessions WHERE chat_id=chats.id) FROM chats WHERE id=?1", [id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional().map_err(|_| "storage_error".to_owned())
}

pub(crate) fn apply_deletion_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    deleted_at: &str,
) -> Result<(), String> {
    if !crate::archive_receipt::valid_chat_id(id)
        || chrono::DateTime::parse_from_rfc3339(deleted_at).is_err()
    {
        return Err("invalid_deletion".into());
    }
    tx.execute(
        "INSERT INTO chat_deletions(chat_id,deleted_at) VALUES(?1,?2) ON CONFLICT(chat_id) DO UPDATE SET deleted_at=excluded.deleted_at",
        params![id, deleted_at],
    ).map_err(|_| "storage_error".to_owned())?;
    for table in [
        "chat_messages",
        "chat_claude_sessions",
        "chat_context_state",
        "chat_provider_handoff",
        "provider_context_limits",
    ] {
        tx.execute(&format!("DELETE FROM {table} WHERE chat_id=?1"), [id])
            .map_err(|_| "storage_error".to_owned())?;
    }
    tx.execute("DELETE FROM chats WHERE id=?1", [id])
        .map_err(|_| "storage_error".to_owned())?;
    tx.execute(
        "DELETE FROM app_runtime_meta WHERE key=?1",
        [format!("chat_archive_write:{id}")],
    )
    .map_err(|_| "storage_error".to_owned())?;
    Ok(())
}

pub(crate) fn apply_archived_deletions_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    deletions: &[(String, String)],
) -> Result<(), String> {
    for (id, _) in deletions {
        let conflict: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM chats WHERE id=?1) AND NOT EXISTS(SELECT 1 FROM chat_deletions WHERE chat_id=?1)",
            [id], |row| row.get(0),
        ).map_err(|_| "storage_error".to_owned())?;
        if conflict {
            return Err("chat_deletion_conflict".into());
        }
    }
    for (id, deleted_at) in deletions {
        apply_deletion_in_transaction(tx, id, deleted_at)?;
    }
    Ok(())
}

fn delete_local(path: &Path, id: &str) -> Result<bool, String> {
    let _archive_guard = crate::chat_archive_store::lock_store().map_err(str::to_owned)?;
    let mut connection = db(path)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| "storage_error".to_owned())?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM chats WHERE id=?1)",
            [id],
            |r| r.get(0),
        )
        .map_err(|_| "storage_error".to_owned())?;
    if !exists {
        return Ok(false);
    }
    apply_deletion_in_transaction(&tx, id, &chrono::Utc::now().to_rfc3339())?;
    tx.execute("INSERT INTO app_runtime_meta(key,value,updated_at) VALUES(?1,'pending',CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value='pending',updated_at=CURRENT_TIMESTAMP", [format!("chat_archive_delete:{id}")]).map_err(|_| "storage_error".to_owned())?;
    tx.commit().map_err(|_| "storage_error".to_owned())?;
    Ok(true)
}

pub(crate) async fn delete_with<F, Fut>(
    path: &Path,
    active: &crate::chat_codex::Active,
    id: &str,
    archive: F,
) -> DeleteResult
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<(), ()>>,
{
    {
        let mut guard = active.lock().await;
        if guard.contains_key(id) {
            return delete_result("busy", None, None);
        }
        guard.insert(id.to_owned(), tokio_util::sync::CancellationToken::new());
    }
    let result = async {
        let sessions = match delete_sessions(path, id) {
            Ok(Some(value)) => value,
            Ok(None) => return delete_result("not_found", None, None),
            Err(_) => return delete_result("storage_error", None, None),
        };
        let claude = Some(if sessions.1 {
            "retained"
        } else {
            "not_present"
        });
        let codex = if let Some(thread) = sessions.0 {
            if archive(thread).await.is_err() {
                return delete_result("provider_cleanup_failed", Some("failed"), claude);
            }
            Some("archived")
        } else {
            Some("not_present")
        };
        match delete_local(path, id) {
            Ok(true) => delete_result("deleted", codex, claude),
            Ok(false) => delete_result("not_found", codex, claude),
            Err(_) => delete_result("storage_error", codex, claude),
        }
    }
    .await;
    active.lock().await.remove(id);
    result
}

#[derive(Serialize)]
pub(crate) struct VisibilityResult {
    pub(crate) state: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArchivedChat {
    id: String,
    title: String,
    group_id: Option<String>,
    updated_at: String,
    archived_at: String,
    provider: Option<String>,
}

pub(crate) fn set_pinned(path: &Path, id: &str, pinned: bool) -> VisibilityResult {
    let apply = || -> Result<usize, rusqlite::Error> {
        let connection = db(path).map_err(|_| rusqlite::Error::InvalidQuery)?;
        connection.execute("UPDATE chats SET pinned_order=CASE WHEN ?2 THEN COALESCE(pinned_order,(SELECT COALESCE(MAX(pinned_order),-1)+1 FROM chats)) ELSE NULL END WHERE id=?1", params![id, pinned])
    };
    VisibilityResult {
        state: match apply() {
            Ok(1) => "ok",
            Ok(_) => "not_found",
            Err(_) => "storage_error",
        },
    }
}

pub(crate) fn set_archived(path: &Path, id: &str, archived: bool) -> VisibilityResult {
    let apply = || -> Result<usize, rusqlite::Error> {
        let connection = db(path).map_err(|_| rusqlite::Error::InvalidQuery)?;
        connection.execute("UPDATE chats SET archived_at=CASE WHEN ?2 THEN COALESCE(archived_at,?3) ELSE NULL END WHERE id=?1", params![id, archived, stamp()])
    };
    VisibilityResult {
        state: match apply() {
            Ok(1) => "ok",
            Ok(_) => "not_found",
            Err(_) => "storage_error",
        },
    }
}

pub(crate) async fn archive_if_idle(
    path: &Path,
    active: &crate::chat_codex::Active,
    id: &str,
) -> VisibilityResult {
    let guard = active.lock().await;
    if guard.contains_key(id) {
        return VisibilityResult { state: "busy" };
    }
    set_archived(path, id, true)
}

pub(crate) fn archived_list(path: &Path) -> Result<Vec<ArchivedChat>, String> {
    let c = db(path)?;
    let mut query = c.prepare("SELECT c.id,c.title,c.group_id,c.updated_at,c.archived_at,(SELECT m.provider FROM chat_messages m WHERE m.chat_id=c.id AND m.provider IS NOT NULL ORDER BY m.rowid DESC LIMIT 1) FROM chats c WHERE c.archived_at IS NOT NULL AND EXISTS(SELECT 1 FROM chat_messages m WHERE m.chat_id=c.id) ORDER BY c.archived_at DESC,c.rowid DESC").map_err(|_| "storage_error".to_string())?;
    let rows = query
        .query_map([], |row| {
            Ok(ArchivedChat {
                id: row.get(0)?,
                title: row.get(1)?,
                group_id: row.get(2)?,
                updated_at: row.get(3)?,
                archived_at: row.get(4)?,
                provider: canonical_provider(row.get::<_, Option<String>>(5)?.as_deref())
                    .map(str::to_owned),
            })
        })
        .map_err(|_| "storage_error".to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "storage_error".to_string())
}
fn stamp() -> String {
    Local::now().to_rfc3339_opts(SecondsFormat::Nanos, false)
}
fn summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatSummary> {
    Ok(ChatSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        created_at: row.get(2)?,
        updated_at: row.get(3)?,
        message_count: row.get(4)?,
        group_id: row.get(5)?,
        pinned: row.get::<_, Option<i64>>(6)?.is_some(),
        pinned_order: row.get(6)?,
        imported_from: row.get(7)?,
    })
}
fn message(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatMessage> {
    Ok(ChatMessage {
        id: row.get(0)?,
        role: Role::from_db(row.get(1)?)?,
        text: row.get(2)?,
        created_at: row.get(3)?,
        provider: canonical_provider(row.get::<_, Option<String>>(4)?.as_deref())
            .map(str::to_owned),
        model: row.get(5)?,
        effort: row.get(6)?,
        actual_effort: row.get(10)?,
        actual_special_effort: None,
        skill: row.get(7)?,
        fast: row.get(8)?,
        fast_state: row.get(13)?,
        not_handed_over: row.get(9)?,
        finish_reason: row
            .get::<_, Option<String>>(11)?
            .map(FinishReason::from_db)
            .transpose()?,
        auth_method: row.get(12)?,
        attachments: Vec::new(),
    })
}
const SELECT_SUMMARY: &str = "SELECT c.id,c.title,NULLIF(c.created_at,''),NULLIF(c.updated_at,''),(SELECT count(*) FROM chat_messages m WHERE m.chat_id=c.id),c.group_id,c.pinned_order,(SELECT source_provider FROM chat_imports i WHERE i.chat_id=c.id) FROM chats c";
const MISSING: &str = "채팅방을 찾을 수 없습니다";
fn one(c: &Connection, id: &str) -> Result<ChatSummary, String> {
    c.query_row(&format!("{SELECT_SUMMARY} WHERE c.id=?1"), [id], summary)
        .optional()
        .map_err(|_| "채팅방을 읽지 못했습니다".to_string())?
        .ok_or_else(|| MISSING.to_string())
}
pub(crate) fn create(path: &Path) -> Result<ChatSummary, String> {
    let c = db(path)?;
    loop {
        let now = Local::now();
        let id = format!("chat_{}", Uuid::new_v4());
        let time = now.to_rfc3339_opts(SecondsFormat::Nanos, false);
        let n = c.execute("INSERT INTO chats(id,title,created_at,updated_at) VALUES (?1,'',?2,?2) ON CONFLICT(id) DO NOTHING", params![id,time]).map_err(|_| "채팅방을 만들지 못했습니다".to_string())?;
        if n == 1 {
            return one(&c, &id);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreatedChatWithMessage {
    pub(crate) chat: ChatSummary,
    pub(crate) message_id: String,
}

pub(crate) fn create_with_message(
    path: &Path,
    text: &str,
    skill: Option<&str>,
) -> Result<CreatedChatWithMessage, String> {
    let skill = skill.filter(|name| !name.trim().is_empty());
    if text.trim().is_empty() && skill.is_none() {
        return Err("empty_message".into());
    }
    let known = crate::saved_logins::known_passwords()
        .map_err(|_| "저장된 로그인 보관함을 확인하지 못했습니다".to_string())?;
    let safe_text = crate::saved_logins::redact_text(text, &known);
    let text = safe_text.as_str();
    let mut c = db(path)?;
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| "storage_error".to_owned())?;
    let id = format!("chat_{}", Uuid::new_v4());
    let message_id = Uuid::new_v4().to_string();
    let time = stamp();
    let title: String = if text.trim().is_empty() {
        String::new()
    } else {
        text.replace("\r\n", " ")
            .replace(['\r', '\n'], " ")
            .chars()
            .take(40)
            .collect()
    };
    tx.execute(
        "INSERT INTO chats(id,title,created_at,updated_at) VALUES (?1,?2,?3,?3)",
        params![id, title, time],
    )
    .map_err(|_| "storage_error".to_owned())?;
    tx.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at,skill,retryable_fast) VALUES (?1,?2,'user',?3,?4,?5,1)", params![message_id,id,text,time,skill]).map_err(|_| "storage_error".to_owned())?;
    let chat = one(&tx, &id)?;
    tx.commit().map_err(|_| "storage_error".to_owned())?;
    Ok(CreatedChatWithMessage { chat, message_id })
}
pub(crate) fn list(path: &Path) -> Result<Vec<ChatSummary>, String> {
    let c = db(path)?;
    let mut q = c
        .prepare(&format!(
            "{SELECT_SUMMARY} WHERE c.archived_at IS NULL AND EXISTS(SELECT 1 FROM chat_messages m WHERE m.chat_id=c.id) ORDER BY c.pinned_order IS NULL,c.pinned_order,c.updated_at DESC,c.rowid DESC"
        ))
        .map_err(|_| "채팅방 목록을 읽지 못했습니다".to_string())?;
    let rows = q
        .query_map([], summary)
        .map_err(|_| "채팅방 목록을 읽지 못했습니다".to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "채팅방 목록을 읽지 못했습니다".to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatListPage {
    items: Vec<ChatSummary>,
    has_more: bool,
    next_offset: Option<u32>,
}

pub(crate) fn list_page(path: &Path, offset: u32) -> Result<ChatListPage, String> {
    const PAGE_SIZE: usize = 80;
    let c = db(path)?;
    let mut q = c.prepare(&format!("{SELECT_SUMMARY} WHERE c.id IN (SELECT id FROM chats WHERE archived_at IS NULL AND EXISTS(SELECT 1 FROM chat_messages m WHERE m.chat_id=chats.id) ORDER BY pinned_order IS NULL,pinned_order,updated_at DESC,rowid DESC LIMIT 81 OFFSET ?1) ORDER BY c.pinned_order IS NULL,c.pinned_order,c.updated_at DESC,c.rowid DESC")).map_err(|_| "채팅방 목록을 읽지 못했습니다".to_owned())?;
    let mut items = q
        .query_map([offset], summary)
        .map_err(|_| "채팅방 목록을 읽지 못했습니다".to_owned())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "채팅방 목록을 읽지 못했습니다".to_owned())?;
    let has_more = items.len() > PAGE_SIZE;
    items.truncate(PAGE_SIZE);
    let next_offset = if has_more {
        offset.checked_add(PAGE_SIZE as u32)
    } else {
        None
    };
    Ok(ChatListPage {
        items,
        has_more,
        next_offset,
    })
}
pub(crate) fn read(path: &Path, id: &str) -> Result<ChatDetail, String> {
    let c = db(path)?;
    let h = one(&c, id)?;
    let mut q = c.prepare("SELECT id,role,text,NULLIF(created_at,''),provider,model,effort,skill,fast,not_handed_over,actual_effort,finish_reason,auth_method,fast_state FROM chat_messages WHERE chat_id=?1 ORDER BY rowid").map_err(|_| "채팅방을 읽지 못했습니다".to_string())?;
    let rows = q
        .query_map([id], message)
        .map_err(|_| "채팅방을 읽지 못했습니다".to_string())?;
    let mut messages = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "채팅방을 읽지 못했습니다".to_string())?;
    let mut attachments = read_attachments(&c, id, i64::MIN, i64::MAX)?;
    for message in &mut messages {
        if message.role == Role::User {
            message.attachments = attachments.remove(&message.id).unwrap_or_default();
        }
    }
    Ok(ChatDetail {
        id: h.id,
        title: h.title,
        created_at: h.created_at,
        updated_at: h.updated_at,
        imported_from: h.imported_from,
        messages,
    })
}

pub(crate) fn read_page(
    path: &Path,
    id: &str,
    before: Option<&str>,
    limit: usize,
) -> Result<ChatPage, String> {
    if !(1..=200).contains(&limit) {
        return Err("invalid_limit".into());
    }
    let mut c = db(path)?;
    let tx = c
        .transaction()
        .map_err(|_| "채팅방을 읽지 못했습니다".to_string())?;
    let h = one(&tx, id)?;
    let cutoff = match before {
        Some(cursor) => tx
            .query_row(
                "SELECT rowid FROM chat_messages WHERE chat_id=?1 AND id=?2",
                params![id, cursor],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|_| "채팅방을 읽지 못했습니다".to_string())?
            .ok_or_else(|| "invalid_cursor".to_owned())?,
        None => i64::MAX,
    };
    let mut q = tx.prepare("SELECT id,role,text,NULLIF(created_at,''),provider,model,effort,skill,fast,not_handed_over,actual_effort,finish_reason,auth_method,fast_state,rowid FROM chat_messages WHERE chat_id=?1 AND rowid<?2 ORDER BY rowid DESC LIMIT ?3").map_err(|_| "채팅방을 읽지 못했습니다".to_string())?;
    let rows = q
        .query_map(params![id, cutoff, (limit + 1) as i64], |row| {
            Ok((row.get::<_, i64>(14)?, message(row)?))
        })
        .map_err(|_| "채팅방을 읽지 못했습니다".to_string())?;
    let mut messages = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "채팅방을 읽지 못했습니다".to_string())?;
    let has_more = messages.len() > limit;
    if has_more {
        messages.pop();
    }
    messages.reverse();
    let next_cursor = has_more.then(|| messages[0].1.id.clone());
    let lower = messages.first().map_or(0, |item| item.0);
    let upper = messages.last().map_or(0, |item| item.0);
    let mut attachments = read_attachments(&tx, id, lower, upper)?;
    let messages = messages
        .into_iter()
        .map(|(_, mut message)| {
            if message.role == Role::User {
                message.attachments = attachments.remove(&message.id).unwrap_or_default();
            }
            message
        })
        .collect();
    Ok(ChatPage {
        detail: ChatDetail {
            id: h.id,
            title: h.title,
            created_at: h.created_at,
            updated_at: h.updated_at,
            imported_from: h.imported_from,
            messages,
        },
        next_cursor,
    })
}

fn read_attachments(
    c: &Connection,
    chat_id: &str,
    lower: i64,
    upper: i64,
) -> Result<HashMap<String, Vec<ChatReadAttachment>>, String> {
    let has_attachments: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='chat_attachments')", [], |row| row.get(0))
        .map_err(|_| "채팅방을 읽지 못했습니다".to_string())?;
    if !has_attachments {
        return Ok(HashMap::new());
    }
    let has_pending: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='chat_archive_pending_attachment')", [], |row| row.get(0))
        .map_err(|_| "채팅방을 읽지 못했습니다".to_string())?;
    let query = if has_pending {
        "SELECT a.message_id,s.content_sha256,s.original_file_name,s.media_type,s.integrity_state,EXISTS(SELECT 1 FROM source_asset_deletions d WHERE d.asset_id=s.id),0 AS source_order,a.rowid AS attachment_order FROM chat_attachments a JOIN momso_source_assets s ON s.id=a.asset_id JOIN chat_messages m ON m.id=a.message_id AND m.chat_id=a.chat_id WHERE a.chat_id=?1 AND m.rowid>=?2 AND m.rowid<=?3 UNION ALL SELECT p.message_id,s.content_sha256,s.original_file_name,s.media_type,s.integrity_state,EXISTS(SELECT 1 FROM source_asset_deletions d WHERE d.asset_id=s.id),1 AS source_order,p.rowid AS attachment_order FROM chat_archive_pending_attachment p JOIN momso_source_assets s ON s.id=p.asset_id JOIN chat_messages m ON m.id=p.message_id AND m.chat_id=p.chat_id WHERE p.chat_id=?1 AND m.rowid>=?2 AND m.rowid<=?3 AND NOT EXISTS(SELECT 1 FROM chat_attachments a WHERE a.receipt_id=p.receipt_id) ORDER BY source_order,attachment_order"
    } else {
        "SELECT a.message_id,s.content_sha256,s.original_file_name,s.media_type,s.integrity_state,EXISTS(SELECT 1 FROM source_asset_deletions d WHERE d.asset_id=s.id) FROM chat_attachments a JOIN momso_source_assets s ON s.id=a.asset_id JOIN chat_messages m ON m.id=a.message_id AND m.chat_id=a.chat_id WHERE a.chat_id=?1 AND m.rowid>=?2 AND m.rowid<=?3 ORDER BY a.rowid"
    };
    let mut q = c
        .prepare(query)
        .map_err(|_| "채팅방을 읽지 못했습니다".to_string())?;
    let rows = q
        .query_map(params![chat_id, lower, upper], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, bool>(5)?,
            ))
        })
        .map_err(|_| "채팅방을 읽지 못했습니다".to_string())?;
    let mut result: HashMap<String, Vec<ChatReadAttachment>> = HashMap::new();
    for row in rows {
        let (message, hash, name, media_type, integrity, deleted) =
            row.map_err(|_| "채팅방을 읽지 못했습니다".to_string())?;
        let asset_id = format!("sha256:{hash}");
        if !crate::archive_zet::valid_asset_id(&asset_id) {
            return Err("채팅방을 읽지 못했습니다".into());
        }
        let path = (!deleted && integrity == "active")
            .then(|| format!("objects/sha256/{}/{}", &hash[..2], hash));
        result.entry(message).or_default().push(ChatReadAttachment {
            asset_id,
            name,
            media_type,
            deleted,
            path,
        });
    }
    Ok(result)
}
#[cfg(test)]
pub(crate) fn append(
    path: &Path,
    id: &str,
    role: Role,
    text: &str,
    provider: Option<String>,
    model: Option<String>,
) -> Result<ChatMessage, String> {
    append_with_effort(path, id, role, text, provider, model, None)
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) fn append_with_effort(
    path: &Path,
    id: &str,
    role: Role,
    text: &str,
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
) -> Result<ChatMessage, String> {
    append_full(path, id, role, text, provider, model, effort, None)
}
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) fn append_full(
    path: &Path,
    id: &str,
    role: Role,
    text: &str,
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    skill: Option<String>,
) -> Result<ChatMessage, String> {
    append_internal(
        path, id, role, text, provider, model, effort, skill, None, None, false,
    )
}

pub(crate) fn append_for_send(
    path: &Path,
    id: &str,
    text: &str,
    skill: Option<String>,
) -> Result<ChatMessage, String> {
    append_internal(
        path,
        id,
        Role::User,
        text,
        None,
        None,
        None,
        skill,
        None,
        None,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) fn append_with_actual_effort(
    path: &Path,
    id: &str,
    role: Role,
    text: &str,
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    skill: Option<String>,
    actual_effort: Option<String>,
) -> Result<ChatMessage, String> {
    append_internal(
        path,
        id,
        role,
        text,
        provider,
        model,
        effort,
        skill,
        actual_effort,
        None,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn append_finished(
    path: &Path,
    id: &str,
    text: &str,
    provider: String,
    model: Option<String>,
    effort: Option<String>,
    reason: FinishReason,
) -> Result<ChatMessage, String> {
    append_finished_with_actual(path, id, text, provider, model, effort, None, reason)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn append_finished_with_actual(
    path: &Path,
    id: &str,
    text: &str,
    provider: String,
    model: Option<String>,
    effort: Option<String>,
    actual_effort: Option<String>,
    reason: FinishReason,
) -> Result<ChatMessage, String> {
    append_finished_with_optional_reason(
        path,
        id,
        text,
        provider,
        model,
        effort,
        actual_effort,
        Some(reason),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn append_finished_with_optional_reason(
    path: &Path,
    id: &str,
    text: &str,
    provider: String,
    model: Option<String>,
    effort: Option<String>,
    actual_effort: Option<String>,
    reason: Option<FinishReason>,
) -> Result<ChatMessage, String> {
    append_internal(
        path,
        id,
        Role::Assistant,
        text,
        Some(provider),
        model,
        effort,
        None,
        actual_effort,
        reason,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn append_internal(
    path: &Path,
    id: &str,
    role: Role,
    text: &str,
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    skill: Option<String>,
    actual_effort: Option<String>,
    finish_reason: Option<FinishReason>,
    claim_attachments: bool,
) -> Result<ChatMessage, String> {
    let known = crate::saved_logins::known_passwords()
        .map_err(|_| "저장된 로그인 보관함을 확인하지 못했습니다".to_string())?;
    let safe_text = crate::saved_logins::redact_text(text, &known);
    let text = safe_text.as_str();
    let mut c = db(path)?;
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| "말을 저장하지 못했습니다".to_string())?;
    let h = one(&tx, id)?;
    let time = stamp();
    let msg_id = Uuid::new_v4().to_string();
    crate::chat_message_storage::insert(
        &tx,
        &crate::chat_message_storage::Message {
            id: &msg_id,
            chat_id: id,
            role: role.as_str(),
            text,
            created_at: &time,
            provider: provider.as_deref(),
            model: model.as_deref(),
            effort: effort.as_deref(),
            skill: skill.as_deref(),
            actual_effort: actual_effort.as_deref(),
            finish_reason: finish_reason.map(FinishReason::as_str),
        },
    )
    .map_err(|_| "말을 저장하지 못했습니다".to_string())?;
    if claim_attachments {
        tx.execute(
            "UPDATE chat_attachments SET message_id=?1 WHERE chat_id=?2 AND message_id IS NULL",
            params![msg_id, id],
        )
        .map_err(|_| "원자료를 연결하지 못했습니다".to_string())?;
    }
    let title = if h.title.is_empty() && role == Role::User {
        text.replace("\r\n", " ")
            .replace(['\r', '\n'], " ")
            .chars()
            .take(40)
            .collect::<String>()
    } else {
        h.title
    };
    tx.execute(
        "UPDATE chats SET title=?1,updated_at=?2 WHERE id=?3",
        params![title, time, id],
    )
    .map_err(|_| "말을 저장하지 못했습니다".to_string())?;
    tx.commit()
        .map_err(|_| "말을 저장하지 못했습니다".to_string())?;
    Ok(ChatMessage {
        id: msg_id,
        role,
        text: text.to_owned(),
        created_at: Some(time),
        provider,
        model,
        effort,
        actual_effort,
        actual_special_effort: None,
        skill,
        fast: None,
        fast_state: None,
        not_handed_over: None,
        finish_reason,
        auth_method: None,
        attachments: Vec::new(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MessageAttachment {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) media_type: Option<String>,
}

pub(crate) fn message_attachment_paths(
    path: &Path,
    archive: &Path,
    chat_id: &str,
    message_id: &str,
) -> Result<Vec<MessageAttachment>, String> {
    let c = db(path)?;
    let mut query = c.prepare("SELECT s.content_sha256,s.integrity_state,s.original_file_name,s.media_type,EXISTS(SELECT 1 FROM source_asset_deletions d WHERE d.asset_id=s.id) FROM chat_attachments a JOIN momso_source_assets s ON s.id=a.asset_id WHERE a.chat_id=?1 AND a.message_id=?2 ORDER BY a.rowid").map_err(|_| "원자료를 읽지 못했습니다".to_string())?;
    let entries = query
        .query_map(params![chat_id, message_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })
        .map_err(|_| "원자료를 읽지 못했습니다".to_string())?;
    let root = archive
        .join("objects")
        .canonicalize()
        .map_err(|_| "원자료를 읽지 못했습니다".to_string())?;
    let mut paths = Vec::new();
    for entry in entries {
        let (hash, state, name, media_type, deleted) =
            entry.map_err(|_| "원자료를 읽지 못했습니다".to_string())?;
        if deleted
            || state != "active"
            || !crate::archive_zet::valid_asset_id(&format!("sha256:{hash}"))
        {
            return Err("원자료를 읽지 못했습니다".into());
        }
        let relative = format!("objects/sha256/{}/{}", &hash[..2], hash);
        let canonical = archive
            .join(&relative)
            .canonicalize()
            .map_err(|_| "원자료를 읽지 못했습니다".to_string())?;
        if !canonical.starts_with(&root) || !canonical.is_file() {
            return Err("원자료를 읽지 못했습니다".into());
        }
        paths.push(MessageAttachment {
            path: relative,
            name,
            media_type,
        });
    }
    Ok(paths)
}

pub(crate) fn claim_retry_attachments(
    path: &Path,
    chat_id: &str,
    message_id: &str,
) -> Result<(), String> {
    db(path)?
        .execute(
            "UPDATE chat_attachments SET message_id=?1 WHERE chat_id=?2 AND message_id IS NULL",
            params![message_id, chat_id],
        )
        .map_err(|_| "원자료를 연결하지 못했습니다".to_string())?;
    Ok(())
}

pub(crate) fn find_fast_retry(
    path: &Path,
    chat_id: &str,
    text: &str,
    skill: Option<&str>,
) -> Result<Option<String>, String> {
    let mut connection = db(path)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| "말을 저장하지 못했습니다".to_string())?;
    let found: Option<String> = tx.query_row(
        "SELECT id FROM chat_messages WHERE chat_id=?1 AND rowid=(SELECT max(rowid) FROM chat_messages WHERE chat_id=?1) AND role='user' AND retryable_fast=1 AND text=?2",
        params![chat_id, text], |row| row.get(0)
    ).optional().map_err(|_| "말을 저장하지 못했습니다".to_string())?;
    if let Some(id) = &found {
        tx.execute(
            "UPDATE chat_messages SET skill=?1 WHERE id=?2",
            params![skill, id],
        )
        .map_err(|_| "말을 저장하지 못했습니다".to_string())?;
    }
    tx.commit()
        .map_err(|_| "말을 저장하지 못했습니다".to_string())?;
    Ok(found)
}

pub(crate) fn retry_input(
    path: &Path,
    chat_id: &str,
    message_id: &str,
) -> Result<Option<(String, Option<String>)>, String> {
    db(path)?.query_row(
        "SELECT m.text,m.skill FROM chat_messages m WHERE m.chat_id=?1 AND m.id=?2 AND m.role='user' AND (m.retryable_fast=1 OR EXISTS (SELECT 1 FROM chat_messages f WHERE f.chat_id=m.chat_id AND f.rowid>m.rowid AND f.role='assistant' AND f.finish_reason IN ('error','cancelled','length_limit','turn_limit','refusal'))) AND NOT EXISTS (SELECT 1 FROM chat_messages n WHERE n.chat_id=m.chat_id AND n.rowid>m.rowid AND (n.role='user' OR (n.role='assistant' AND COALESCE(n.finish_reason,'completed')='completed')))",
        params![chat_id, message_id], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(|_| "채팅방을 읽지 못했습니다".to_string())
}

pub(crate) fn mark_fast_retry(path: &Path, chat_id: &str, message_id: &str) -> Result<(), String> {
    let changed = db(path)?
        .execute(
            "UPDATE chat_messages SET retryable_fast=1 WHERE chat_id=?1 AND id=?2 AND role='user'",
            params![chat_id, message_id],
        )
        .map_err(|_| "말을 저장하지 못했습니다".to_string())?;
    if changed == 1 {
        Ok(())
    } else {
        Err("말을 저장하지 못했습니다".into())
    }
}

pub(crate) fn set_delivery_metadata(
    path: &Path,
    chat_id: &str,
    message_id: &str,
    fast_state: Option<&str>,
    omitted: usize,
) -> Result<(), String> {
    let omitted = i64::try_from(omitted).map_err(|_| "채팅방을 저장하지 못했습니다".to_string())?;
    let fast = match fast_state {
        Some("on") => Some(true),
        Some("off") => Some(false),
        Some("cooldown") | None => None,
        Some(_) => return Err("말을 저장하지 못했습니다".into()),
    };
    let changed = db(path)?.execute("UPDATE chat_messages SET fast=?1,fast_state=?2,not_handed_over=?3 WHERE chat_id=?4 AND id=?5 AND role='assistant'", params![fast, fast_state, omitted, chat_id, message_id]).map_err(|_| "채팅방을 저장하지 못했습니다".to_string())?;
    if changed != 1 {
        return Err("채팅방을 저장하지 못했습니다".into());
    }
    Ok(())
}
pub(crate) fn set_auth_method(
    path: &Path,
    chat_id: &str,
    message_id: &str,
    method: &str,
) -> Result<(), String> {
    if !matches!(method, "account" | "apiKey") {
        return Err("unavailable".into());
    }
    let changed = db(path)?.execute("UPDATE chat_messages SET auth_method=?1 WHERE chat_id=?2 AND id=?3 AND role='assistant'", params![method, chat_id, message_id]).map_err(|_| "unavailable".to_string())?;
    if changed == 1 {
        Ok(())
    } else {
        Err("unavailable".into())
    }
}
pub(crate) fn rename(path: &Path, id: &str, title: &str) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("채팅방 이름을 입력해 주세요".into());
    }
    let c = db(path)?;
    let n = c
        .execute(
            "UPDATE chats SET title=?1,updated_at=?2 WHERE id=?3",
            params![title, stamp(), id],
        )
        .map_err(|_| "채팅방 이름을 바꾸지 못했습니다".to_string())?;
    if n == 0 {
        return Err(MISSING.into());
    }
    Ok(())
}
pub(crate) fn search(path: &Path, query: &str) -> Result<Vec<ChatSearchResult>, String> {
    let c = db(path)?;
    let characters: Vec<char> = query.chars().collect();
    let mut grams = Vec::new();
    if characters.len() >= 3 && !query.contains('\0') {
        let last = characters.len() - 3;
        grams = [0, last / 2, last]
            .into_iter()
            .map(|start| characters[start..start + 3].iter().collect::<String>())
            .collect();
        grams.sort();
        grams.dedup();
    }
    let index_ready: bool = c
        .query_row(
            "SELECT ready FROM chat_search_index_state WHERE id=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .unwrap_or(None)
        .unwrap_or(false);
    let indexed = if grams.is_empty() || !index_ready {
        false
    } else {
        let total: i64 = c
            .query_row("SELECT count(*) FROM chat_message_search_map", [], |r| {
                r.get(0)
            })
            .map_err(|_| "채팅방을 검색하지 못했습니다".to_string())?;
        let mut least = total;
        for gram in &grams {
            let documents: Option<i64> = c
                .query_row(
                    "SELECT doc FROM chat_message_search_vocab WHERE term=?1",
                    [gram],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|_| "채팅방을 검색하지 못했습니다".to_string())?;
            least = least.min(documents.unwrap_or(0));
        }
        total == 0 || least.saturating_mul(20) < total
    };
    let sql = if indexed {
        "WITH matched AS MATERIALIZED (SELECT DISTINCT map.chat_id FROM chat_message_search JOIN chat_message_search_map map ON map.message_rowid=chat_message_search.rowid WHERE chat_message_search MATCH ?2) SELECT c.id,c.title,NULLIF(c.created_at,''),NULLIF(c.updated_at,''),(SELECT count(*) FROM chat_messages m WHERE m.chat_id=c.id),c.group_id,c.pinned_order,(SELECT source_provider FROM chat_imports i WHERE i.chat_id=c.id),(SELECT text FROM chat_messages m WHERE m.chat_id=c.id AND instr(m.text,?1)>0 ORDER BY m.rowid LIMIT 1) AS match_text FROM chats c LEFT JOIN matched ON matched.chat_id=c.id WHERE c.archived_at IS NULL AND (instr(c.title,?1)>0 OR (matched.chat_id IS NOT NULL AND EXISTS (SELECT 1 FROM chat_messages m WHERE m.chat_id=c.id AND instr(m.text,?1)>0))) ORDER BY c.pinned_order IS NULL,c.pinned_order,c.updated_at DESC,c.rowid DESC"
    } else {
        "SELECT c.id,c.title,NULLIF(c.created_at,''),NULLIF(c.updated_at,''),(SELECT count(*) FROM chat_messages m WHERE m.chat_id=c.id),c.group_id,c.pinned_order,(SELECT source_provider FROM chat_imports i WHERE i.chat_id=c.id),(SELECT text FROM chat_messages m WHERE m.chat_id=c.id AND instr(m.text,?1)>0 ORDER BY m.rowid LIMIT 1) AS match_text FROM chats c WHERE c.archived_at IS NULL AND (instr(c.title,?1)>0 OR EXISTS (SELECT 1 FROM chat_messages m WHERE m.chat_id=c.id AND instr(m.text,?1)>0)) ORDER BY c.pinned_order IS NULL,c.pinned_order,c.updated_at DESC,c.rowid DESC"
    };
    let sql = sql.replace("WHERE c.archived_at IS NULL AND (", "WHERE c.archived_at IS NULL AND EXISTS(SELECT 1 FROM chat_messages visible WHERE visible.chat_id=c.id) AND (");
    let mut q = c
        .prepare(&sql)
        .map_err(|_| "채팅방을 검색하지 못했습니다".to_string())?;
    let phrase = if indexed {
        grams
            .into_iter()
            .map(|gram| format!("\"{}\"", gram.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND ")
    } else {
        String::new()
    };
    let params: Vec<&dyn rusqlite::ToSql> = if indexed {
        vec![&query, &phrase]
    } else {
        vec![&query]
    };
    let rows = q
        .query_map(params.as_slice(), |row| {
            let text: Option<String> = row.get(8)?;
            let snippet = text.map(|s| {
                s.lines()
                    .find(|line| line.contains(query))
                    .unwrap_or(&s)
                    .replace(['\r', '\n'], " ")
            });
            Ok(ChatSearchResult {
                chat: summary(row)?,
                snippet,
            })
        })
        .map_err(|_| "채팅방을 검색하지 못했습니다".to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "채팅방을 검색하지 못했습니다".to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatSearchPage {
    items: Vec<ChatSearchResult>,
    has_more: bool,
    next_offset: Option<u32>,
}

pub(crate) fn search_page(path: &Path, query: &str, offset: u32) -> Result<ChatSearchPage, String> {
    const PAGE_SIZE: usize = 80;
    let c = db(path)?;
    let mut statement = c.prepare("WITH selected AS MATERIALIZED (SELECT c.id FROM chats c WHERE c.archived_at IS NULL AND EXISTS(SELECT 1 FROM chat_messages visible WHERE visible.chat_id=c.id) AND (instr(c.title,?1)>0 OR EXISTS (SELECT 1 FROM chat_messages m WHERE m.chat_id=c.id AND instr(m.text,?1)>0)) ORDER BY (c.pinned_order IS NULL),c.pinned_order,c.updated_at DESC,c.id DESC LIMIT 81 OFFSET ?2) SELECT c.id,c.title,NULLIF(c.created_at,''),NULLIF(c.updated_at,''),(SELECT count(*) FROM chat_messages m WHERE m.chat_id=c.id),c.group_id,c.pinned_order,(SELECT source_provider FROM chat_imports i WHERE i.chat_id=c.id),(SELECT text FROM chat_messages m WHERE m.chat_id=c.id AND instr(m.text,?1)>0 ORDER BY m.rowid LIMIT 1) FROM selected s JOIN chats c ON c.id=s.id ORDER BY (c.pinned_order IS NULL),c.pinned_order,c.updated_at DESC,c.id DESC").map_err(|_| "채팅방을 검색하지 못했습니다".to_string())?;
    let rows = statement
        .query_map(params![query, offset], |row| {
            let text: Option<String> = row.get(8)?;
            let snippet = text.map(|s| {
                s.lines()
                    .find(|line| line.contains(query))
                    .unwrap_or(&s)
                    .replace(['\r', '\n'], " ")
            });
            Ok(ChatSearchResult {
                chat: summary(row)?,
                snippet,
            })
        })
        .map_err(|_| "채팅방을 검색하지 못했습니다".to_string())?;
    let mut items = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "채팅방을 검색하지 못했습니다".to_string())?;
    let has_more = items.len() > PAGE_SIZE;
    items.truncate(PAGE_SIZE);
    let next_offset = if has_more {
        offset.checked_add(PAGE_SIZE as u32)
    } else {
        None
    };
    Ok(ChatSearchPage {
        items,
        has_more,
        next_offset,
    })
}
pub(crate) fn automatic_title_event(
    id: &str,
    before: &str,
    after: &str,
) -> Option<serde_json::Value> {
    (before.is_empty() && !after.is_empty()).then(|| serde_json::json!({"chatId":id,"title":after}))
}

pub(crate) fn title(path: &Path, id: &str) -> Result<String, String> {
    Ok(one(&db(path)?, id)?.title)
}
pub(crate) fn codex_thread(path: &Path, id: &str) -> Result<Option<String>, String> {
    let c = db(path)?;
    c.query_row(
        "SELECT codex_thread_id FROM chats WHERE id=?1",
        [id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|_| "채팅방을 읽지 못했습니다".to_string())?
    .ok_or_else(|| MISSING.to_string())
}
pub(crate) fn set_codex_thread(path: &Path, id: &str, thread: &str) -> Result<(), String> {
    let c = db(path)?;
    let n = c
        .execute(
            "UPDATE chats SET codex_thread_id=?1 WHERE id=?2",
            params![thread, id],
        )
        .map_err(|_| "채팅방을 저장하지 못했습니다".to_string())?;
    if n == 0 {
        return Err(MISSING.into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_keeps_attachments_on_the_sent_message_through_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test.db");
        crate::storage::initialize(&path, &temp.path().join("backups"), "test").unwrap();
        let id = serde_json::to_value(create(&path).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let c = db(&path).unwrap();
        for (asset, ch, name, media) in [
            ("one", 'a', "lesson.txt", "text/plain"),
            ("two", 'b', "photo.png", "image/png"),
        ] {
            let hash = ch.to_string().repeat(64);
            c.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,media_type,captured_by,created_at) VALUES(?1,'other',?2,4,?3,?4,?5,'test','now')", params![asset, hash, format!("sha256/{ch}{ch}/{hash}"), name, media]).unwrap();
            c.execute(
                "INSERT INTO chat_attachments(receipt_id,chat_id,asset_id) VALUES(?1,?2,?3)",
                params![format!("receipt-{asset}"), id, asset],
            )
            .unwrap();
        }
        drop(c);
        let sent = append_for_send(&path, &id, "first", None).unwrap();
        append(&path, &id, Role::Assistant, "answer", None, None).unwrap();
        append_for_send(&path, &id, "second", None).unwrap();
        let read_json = || serde_json::to_value(read(&path, &id).unwrap()).unwrap();
        let value = read_json();
        assert_eq!(value["messages"][0]["id"], sent.id);
        assert_eq!(
            value["messages"][0]["attachments"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            value["messages"][0]["attachments"][0]["assetId"],
            format!("sha256:{}", "a".repeat(64))
        );
        assert_eq!(value["messages"][0]["attachments"][0]["name"], "lesson.txt");
        assert_eq!(
            value["messages"][0]["attachments"][1]["mediaType"],
            "image/png"
        );
        assert_eq!(
            value["messages"][0]["attachments"][1]["path"],
            format!("objects/sha256/bb/{}", "b".repeat(64))
        );
        assert_eq!(value["messages"][1]["attachments"], serde_json::json!([]));
        assert_eq!(value["messages"][2]["attachments"], serde_json::json!([]));
        let page = serde_json::to_value(read_page(&path, &id, None, 2).unwrap()).unwrap();
        let cursor = page["nextCursor"].as_str().unwrap();
        let earlier =
            serde_json::to_value(read_page(&path, &id, Some(cursor), 2).unwrap()).unwrap();
        assert_eq!(earlier["messages"][0], value["messages"][0]);
        assert!(earlier["nextCursor"].is_null());
        let c = db(&path).unwrap();
        c.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until) VALUES('one','fixture-one','trashed','now','later')", []).unwrap();
        c.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until) VALUES('two','fixture-two','purged','now','later')", []).unwrap();
        c.execute("UPDATE momso_source_assets SET original_file_name='deleted',media_type=NULL WHERE id='two'", []).unwrap();
        drop(c);
        let value = read_json();
        let attachments = value["messages"][0]["attachments"].as_array().unwrap();
        assert_eq!(attachments[0]["name"], "lesson.txt");
        assert_eq!(attachments[0]["deleted"], true);
        assert!(attachments[0].get("path").is_none());
        assert_eq!(attachments[1]["deleted"], true);
        assert_eq!(attachments[1]["name"], "deleted");
        assert!(attachments[1].get("path").is_none());
    }

    #[test]
    fn page_cursor_preserves_order_and_new_messages_do_not_duplicate() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test.db");
        crate::storage::initialize(&path, &temp.path().join("backups"), "test").unwrap();
        let id = create(&path).unwrap().id;
        let empty = read_page(&path, &id, None, 100).unwrap();
        assert!(empty.detail.messages.is_empty());
        assert!(empty.next_cursor.is_none());
        let mut c = db(&path).unwrap();
        let tx = c.transaction().unwrap();
        for n in 0..2000 {
            tx.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES(?1,?2,'user',?3,'fixture')", params![format!("fixture-{n:04}"), id, format!("message {n}")]).unwrap();
        }
        tx.commit().unwrap();
        drop(c);
        let start = std::time::Instant::now();
        let full = read(&path, &id).unwrap();
        let full_ms = start.elapsed().as_millis();
        let start = std::time::Instant::now();
        let latest = read_page(&path, &id, None, 100).unwrap();
        let page_ms = start.elapsed().as_millis();
        println!("BENCH chat_read_2000_ms={full_ms} chat_read_page_100_ms={page_ms}");
        assert_eq!(full.messages.len(), 2000);
        assert_eq!(latest.detail.messages[0].id, "fixture-1900");
        assert_eq!(latest.detail.messages[99].id, "fixture-1999");
        let cursor = latest.next_cursor.unwrap();
        append_for_send(&path, &id, "new message", None).unwrap();
        let older = read_page(&path, &id, Some(&cursor), 100).unwrap();
        assert_eq!(older.detail.messages[0].id, "fixture-1800");
        assert_eq!(older.detail.messages[99].id, "fixture-1899");
        assert!(read_page(&path, &id, Some("not-in-room"), 100).is_err());
        assert!(read_page(&path, &id, None, 0).is_err());
        assert!(read_page(&path, &id, None, 201).is_err());
        let mut next = Some(cursor);
        let mut seen = 100;
        while let Some(current) = next {
            let page = read_page(&path, &id, Some(&current), 100).unwrap();
            seen += page.detail.messages.len();
            next = page.next_cursor;
        }
        assert_eq!(seen, 2000);
    }

    #[tokio::test]
    async fn deletion_cleans_database_and_preserves_archive_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test.db");
        crate::storage::initialize(&path, &temp.path().join("backups"), "test").unwrap();
        let id = serde_json::to_value(create(&path).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        append(&path, &id, Role::User, "fictional", None, None).unwrap();
        set_codex_thread(&path, &id, "thread-test").unwrap();
        let c = db(&path).unwrap();
        c.execute(
            "INSERT INTO chat_claude_sessions(chat_id,session_id) VALUES (?1,'claude-test')",
            [&id],
        )
        .unwrap();
        c.execute("INSERT INTO chat_context_state(chat_id,provider,updated_at) VALUES (?1,'codex','test')", [&id]).unwrap();
        c.execute("INSERT INTO chat_provider_handoff(chat_id,provider,delivered_rowid) VALUES (?1,'codex',1)", [&id]).unwrap();
        c.execute("INSERT INTO provider_context_limits(chat_id,provider,model,limit_tokens,used_tokens,updated_at) VALUES (?1,'claude','fake',100,1,'test')", [&id]).unwrap();
        let hash = "a".repeat(64);
        c.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset','other',?1,4,?2,'original','test','now')", rusqlite::params![hash, format!("sha256/aa/{hash}")]).unwrap();
        c.execute("INSERT INTO chat_attachments(receipt_id,chat_id,asset_id,message_id) SELECT 'receipt',?1,'asset',id FROM chat_messages WHERE chat_id=?1", [&id]).unwrap();
        drop(c);
        let archive = temp.path().join("archive");
        for folder in ["zettels", "objects", "receipts"] {
            std::fs::create_dir_all(archive.join(folder)).unwrap();
            std::fs::write(archive.join(folder).join("fixture"), b"keep").unwrap();
        }
        let backed_up_document = temp.path().join("backed-up-document");
        std::fs::copy(archive.join("zettels/fixture"), &backed_up_document).unwrap();
        let active = crate::chat_codex::Active::default();
        let result = delete_with(&path, &active, &id, |thread| async move {
            assert_eq!(thread, "thread-test");
            Ok(())
        })
        .await;
        let json = serde_json::to_value(result).unwrap();
        assert_eq!(json["state"], "deleted");
        assert_eq!(json["codexSession"], "archived");
        assert_eq!(json["claudeSession"], "retained");
        let c = db(&path).unwrap();
        for table in [
            "chats",
            "chat_messages",
            "chat_claude_sessions",
            "chat_context_state",
            "chat_provider_handoff",
            "provider_context_limits",
            "chat_attachments",
        ] {
            let count: i64 = c
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
        let assets: i64 = c
            .query_row(
                "SELECT count(*) FROM momso_source_assets WHERE id='asset'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(assets, 1);
        for folder in ["zettels", "objects", "receipts"] {
            assert_eq!(
                std::fs::read(archive.join(folder).join("fixture")).unwrap(),
                b"keep"
            );
        }
        assert_eq!(std::fs::read(backed_up_document).unwrap(), b"keep");
        assert!(!active.lock().await.contains_key(&id));
        assert_eq!(
            delete_with(&path, &active, &id, |_| async { Ok(()) })
                .await
                .state,
            "not_found"
        );
    }

    #[tokio::test]
    async fn deletion_rejects_busy_and_keeps_chat_if_archive_fails() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test.db");
        crate::storage::initialize(&path, &temp.path().join("backups"), "test").unwrap();
        let id = serde_json::to_value(create(&path).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        set_codex_thread(&path, &id, "thread-test").unwrap();
        let active = crate::chat_codex::Active::default();
        active
            .lock()
            .await
            .insert(id.clone(), tokio_util::sync::CancellationToken::new());
        assert_eq!(
            delete_with(&path, &active, &id, |_| async {
                panic!("must not archive")
            })
            .await
            .state,
            "busy"
        );
        active.lock().await.remove(&id);
        assert_eq!(
            delete_with(&path, &active, &id, |_| async { Err(()) })
                .await
                .state,
            "provider_cleanup_failed"
        );
        assert!(read(&path, &id).is_ok());
        assert!(!active.lock().await.contains_key(&id));
    }
    fn database() -> (tempfile::TempDir, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("test.db");
        let c = Connection::open(&p).unwrap();
        c.execute_batch(include_str!("../migrations/0010_chats.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0011_chat_codex.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0012_chat_effort.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0013_chat_groups.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0070_chat_skill.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0072_chat_delivery.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0073_chat_visibility.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0075_chat_actual_effort.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0080_chat_finish_reason.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0081_chat_import.sql"))
            .unwrap();
        c.execute_batch(include_str!(
            "../migrations/0083_reconcile_context_import.sql"
        ))
        .unwrap();
        c.execute_batch(include_str!("../migrations/0084_chat_auth_method.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0085_chat_fast_state.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0089_chat_search.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0091_chat_search_pages.sql"))
            .unwrap();
        (d, p)
    }

    #[test]
    fn chat_list_pages_match_full_order_and_exclude_archived() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("fixture.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "fixture").unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        for n in 0..163 {
            conn.execute("INSERT INTO chats(id,title,created_at,updated_at,archived_at) VALUES(?1,?2,'2026-01-01','2026-01-01',?3)", rusqlite::params![format!("chat_{n:08x}-0000-4000-8000-{n:012x}"),format!("fixture-{n}"),if n == 162 { Some("2026-01-01") } else { None }]).unwrap();
            conn.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES (?1,?2,'user','hello','2026-01-01')", rusqlite::params![format!("message-{n}"),format!("chat_{n:08x}-0000-4000-8000-{n:012x}")]).unwrap();
        }
        let first = serde_json::to_value(list_page(&db, 0).unwrap()).unwrap();
        let second = serde_json::to_value(list_page(&db, 80).unwrap()).unwrap();
        let third = serde_json::to_value(list_page(&db, 160).unwrap()).unwrap();
        assert_eq!(
            (first["hasMore"].as_bool(), first["nextOffset"].as_u64()),
            (Some(true), Some(80))
        );
        assert_eq!(
            (second["hasMore"].as_bool(), second["nextOffset"].as_u64()),
            (Some(true), Some(160))
        );
        assert_eq!(
            (third["hasMore"].as_bool(), third["nextOffset"].as_u64()),
            (Some(false), None)
        );
        let combined: Vec<_> = [first, second, third]
            .iter()
            .flat_map(|page| page["items"].as_array().unwrap().clone())
            .collect();
        assert_eq!(
            serde_json::Value::Array(combined),
            serde_json::to_value(list(&db).unwrap()).unwrap()
        );
    }
    #[test]
    fn authentication_method_roundtrips_without_secret_material() {
        let (_dir, path) = database();
        let id = serde_json::to_value(create(&path).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let message = append_finished(
            &path,
            &id,
            "answer",
            "claude".into(),
            None,
            None,
            FinishReason::Completed,
        )
        .unwrap();
        set_auth_method(&path, &id, &message.id, "apiKey").unwrap();
        let saved = serde_json::to_value(read(&path, &id).unwrap()).unwrap();
        assert_eq!(saved["messages"][0]["authMethod"], "apiKey");
        assert!(set_auth_method(&path, &id, &message.id, "fake-secret").is_err());
    }
    #[test]
    fn paged_search_returns_korean_matches_without_duplicates() {
        let (_dir, path) = database();
        let mut conn = Connection::open(&path).unwrap();
        let tx = conn.transaction().unwrap();
        for i in 0..100 {
            let id = format!("chat-{i:03}");
            tx.execute(
                "INSERT INTO chats(id,title,created_at,updated_at) VALUES(?1,'test','2026','2026')",
                [&id],
            )
            .unwrap();
            tx.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES(?1,?2,'user','수리아사나를 연습합니다','2026')", params![format!("message-{i}"),id]).unwrap();
        }
        tx.commit().unwrap();
        let first = search_page(&path, "아사나를", 0).unwrap();
        assert_eq!(first.items.len(), 80);
        assert!(first.has_more);
        let second = search_page(&path, "아사나를", first.next_offset.unwrap()).unwrap();
        assert_eq!(second.items.len(), 20);
        assert!(!second.has_more);
        let ids: std::collections::HashSet<_> = first
            .items
            .iter()
            .chain(&second.items)
            .map(|item| &item.chat.id)
            .collect();
        assert_eq!(ids.len(), 100);
    }
    #[test]
    fn pin_archive_restore_and_search_preserve_messages_and_group() {
        let (_d, p) = database();
        let first = create(&p).unwrap();
        let second = create(&p).unwrap();
        append(&p, &second.id, Role::User, "second", None, None).unwrap();
        let conn = db(&p).unwrap();
        conn.execute(
            "INSERT INTO chat_groups(id,name,position,created_at) VALUES ('group','group',0,'now')",
            [],
        )
        .unwrap();
        conn.execute("UPDATE chats SET group_id='group' WHERE id=?1", [&first.id])
            .unwrap();
        append(
            &p,
            &first.id,
            Role::Assistant,
            "hello",
            Some("codex".into()),
            None,
        )
        .unwrap();
        assert_eq!(set_pinned(&p, &first.id, true).state, "ok");
        assert_eq!(set_pinned(&p, &second.id, true).state, "ok");
        let ordered = list(&p).unwrap();
        assert_eq!(ordered[0].id, first.id);
        assert_eq!(ordered[1].pinned_order, Some(1));
        assert_eq!(set_archived(&p, &first.id, true).state, "ok");
        assert_eq!(list(&p).unwrap().len(), 1);
        assert!(search(&p, "hello").unwrap().is_empty());
        let archived = serde_json::to_value(archived_list(&p).unwrap()).unwrap();
        assert_eq!(archived[0]["groupId"], "group");
        assert_eq!(archived[0]["provider"], "codex");
        assert_eq!(read(&p, &first.id).unwrap().messages.len(), 1);
        conn.execute("DELETE FROM chat_groups WHERE id='group'", [])
            .unwrap();
        assert_eq!(set_archived(&p, &first.id, false).state, "ok");
        assert!(list(&p)
            .unwrap()
            .iter()
            .any(|c| c.id == first.id && c.group_id.is_none() && c.pinned));
        assert_eq!(set_pinned(&p, &first.id, false).state, "ok");
        assert_eq!(set_archived(&p, "missing", true).state, "not_found");
    }
    #[tokio::test]
    async fn archive_rejects_a_running_chat() {
        let (_d, p) = database();
        let room = create(&p).unwrap();
        let active = crate::chat_codex::Active::default();
        active
            .lock()
            .await
            .insert(room.id.clone(), tokio_util::sync::CancellationToken::new());
        assert_eq!(archive_if_idle(&p, &active, &room.id).await.state, "busy");
        assert!(archived_list(&p).unwrap().is_empty());
    }
    #[test]
    fn thread_is_stored_and_reused() {
        let (_d, p) = database();
        let c = create(&p).unwrap();
        assert_eq!(codex_thread(&p, &c.id).unwrap(), None);
        set_codex_thread(&p, &c.id, "thread-1").unwrap();
        assert_eq!(
            codex_thread(&p, &c.id).unwrap().as_deref(),
            Some("thread-1")
        );
        assert!(codex_thread(&p, "missing").is_err());
    }
    #[test]
    fn create_and_read_empty() {
        let (_d, p) = database();
        let c = create(&p).unwrap();
        assert!(c.id.starts_with("chat_"));
        assert_eq!(c.message_count, 0);
        assert_eq!(read(&p, &c.id).unwrap().messages.len(), 0);
    }
    #[test]
    fn first_message_is_atomic_and_empty_rooms_are_hidden() {
        let _d = tempfile::tempdir().unwrap();
        let p = _d.path().join("test.db");
        crate::storage::initialize(&p, &_d.path().join("backups"), "test").unwrap();
        let empty = create(&p).unwrap();
        assert!(list(&p).unwrap().is_empty());
        assert!(list_page(&p, 0).unwrap().items.is_empty());
        assert!(search(&p, "").unwrap().is_empty());
        assert!(search_page(&p, "", 0).unwrap().items.is_empty());
        assert_eq!(set_archived(&p, &empty.id, true).state, "ok");
        assert!(archived_list(&p).unwrap().is_empty());
        let c = db(&p).unwrap();
        c.execute_batch("CREATE TRIGGER reject_message BEFORE INSERT ON chat_messages BEGIN SELECT RAISE(ABORT, 'rejected'); END;").unwrap();
        assert!(create_with_message(&p, "hello", None).is_err());
        let count: i64 = c
            .query_row("SELECT count(*) FROM chats", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        c.execute_batch("DROP TRIGGER reject_message;").unwrap();
        let created = create_with_message(&p, "hello", Some("")).unwrap();
        assert_eq!(created.chat.message_count, 1);
        assert_eq!(
            retry_input(&p, &created.chat.id, &created.message_id)
                .unwrap()
                .unwrap()
                .0,
            "hello"
        );
        assert_eq!(
            retry_input(&p, &created.chat.id, &created.message_id)
                .unwrap()
                .unwrap()
                .1,
            None
        );
        let fixture = Connection::open(&p).unwrap();
        fixture.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES ('fixture-asset','other',?1,1,'sha256/fixture','fixture.txt','fixture','now')", ["a".repeat(64)]).unwrap();
        fixture.execute("INSERT INTO chat_attachments(receipt_id,chat_id,asset_id) VALUES ('fixture',?1,'fixture-asset')", [&created.chat.id]).unwrap();
        claim_retry_attachments(&p, &created.chat.id, &created.message_id).unwrap();
        let attached: String = fixture
            .query_row(
                "SELECT message_id FROM chat_attachments WHERE receipt_id='fixture'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(attached, created.message_id);
        assert_eq!(list(&p).unwrap().len(), 1);
        append(&p, &empty.id, Role::User, "arrived", None, None).unwrap();
        assert_eq!(archived_list(&p).unwrap().len(), 1);
    }

    #[test]
    fn skill_only_first_message_can_be_retried_without_duplicate() {
        let (_dir, path) = database();
        for (text, skill) in [("", None), ("  ", Some("  ")), ("", Some(""))] {
            assert!(
                matches!(create_with_message(&path, text, skill), Err(error) if error == "empty_message")
            );
        }
        let created = create_with_message(&path, "", Some("sample-skill")).unwrap();
        assert_eq!(created.chat.title, "");
        assert_eq!(created.chat.message_count, 1);
        assert_eq!(
            retry_input(&path, &created.chat.id, &created.message_id).unwrap(),
            Some((String::new(), Some("sample-skill".to_owned())))
        );
        assert_eq!(read(&path, &created.chat.id).unwrap().messages.len(), 1);
        let spaced = create_with_message(&path, " \n ", Some("sample-skill")).unwrap();
        assert_eq!(spaced.chat.title, "");
        assert_eq!(
            retry_input(&path, &spaced.chat.id, &spaced.message_id)
                .unwrap()
                .unwrap()
                .0,
            " \n "
        );
    }
    #[test]
    fn independent_databases_create_distinct_chat_ids() {
        let (_first, first_db) = database();
        let (_second, second_db) = database();
        let first = create(&first_db).unwrap();
        let second = create(&second_db).unwrap();
        assert_ne!(first.id, second.id);
        assert!(Uuid::parse_str(first.id.strip_prefix("chat_").unwrap()).is_ok());
        assert!(Uuid::parse_str(second.id.strip_prefix("chat_").unwrap()).is_ok());
    }
    #[test]
    fn append_and_auto_title() {
        let (_d, p) = database();
        let c = create(&p).unwrap();
        append(&p, &c.id, Role::Assistant, "먼저", None, None).unwrap();
        let m = append(
            &p,
            &c.id,
            Role::User,
            "안녕\n요가",
            Some("claude".into()),
            Some("모델".into()),
        )
        .unwrap();
        let r = read(&p, &c.id).unwrap();
        assert_eq!(r.title, "안녕 요가");
        assert_eq!(r.messages.len(), 2);
        assert_eq!(r.messages[1].id, m.id);
        assert_eq!(r.messages[1].provider.as_deref(), Some("claude"));
    }

    #[test]
    fn explicit_retry_reuses_failed_message_and_rejects_completed_or_other_chat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        crate::storage::initialize(&path, &dir.path().join("backups"), "test").unwrap();
        let room = create(&path).unwrap();
        let id = serde_json::to_value(&room).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let user = append_for_send(&path, &id, "original text", Some("sample".into())).unwrap();
        assert!(retry_input(&path, &id, &user.id).unwrap().is_none());
        mark_fast_retry(&path, &id, &user.id).unwrap();
        let retry = retry_input(&path, &id, &user.id).unwrap().unwrap();
        assert_eq!(retry, ("original text".into(), Some("sample".into())));
        assert!(retry_input(&path, "another-chat", &user.id)
            .unwrap()
            .is_none());
        for provider in ["codex", "claude", "gemini"] {
            append_finished(
                &path,
                &id,
                "partial",
                provider.into(),
                None,
                None,
                FinishReason::Error,
            )
            .unwrap();
            assert!(
                retry_input(&path, &id, &user.id).unwrap().is_some(),
                "{provider}"
            );
        }
        assert_eq!(
            serde_json::to_value(read(&path, &id).unwrap()).unwrap()["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|m| m["role"] == "user")
                .count(),
            1
        );
        append_finished(
            &path,
            &id,
            "done",
            "claude".into(),
            None,
            None,
            FinishReason::Completed,
        )
        .unwrap();
        assert!(retry_input(&path, &id, &user.id).unwrap().is_none());
    }
    #[test]
    fn first_message_title_event_is_shared_and_manual_titles_stay_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.db");
        crate::storage::initialize(&p, &dir.path().join("backups"), "test").unwrap();
        for provider in ["codex", "claude", "gemini"] {
            let c = create(&p).unwrap();
            let before = title(&p, &c.id).unwrap();
            append_for_send(&p, &c.id, "처음 질문", None).unwrap();
            let after = title(&p, &c.id).unwrap();
            let event = automatic_title_event(&c.id, &before, &after).unwrap();
            assert_eq!(event["chatId"], c.id, "{provider}");
            assert_eq!(event["title"], "처음 질문", "{provider}");
            rename(&p, &c.id, "직접 정한 이름").unwrap();
            let before = title(&p, &c.id).unwrap();
            append_for_send(&p, &c.id, "다음 질문", None).unwrap();
            let after = title(&p, &c.id).unwrap();
            assert_eq!(after, "직접 정한 이름", "{provider}");
            assert!(automatic_title_event(&c.id, &before, &after).is_none());
        }
    }
    #[test]
    fn assistant_effort_roundtrips() {
        let (_d, p) = database();
        let c = create(&p).unwrap();
        append_with_effort(
            &p,
            &c.id,
            Role::Assistant,
            "답",
            Some("codex".into()),
            Some("모델".into()),
            Some("high".into()),
        )
        .unwrap();
        let detail = serde_json::to_value(read(&p, &c.id).unwrap()).unwrap();
        assert_eq!(detail["messages"][0]["effort"], "high");
    }
    #[test]
    fn reported_effort_is_distinct_from_requested_effort() {
        let (_dir, path) = database();
        let id = create(&path).unwrap().id;
        append_with_actual_effort(
            &path,
            &id,
            Role::Assistant,
            "answer",
            Some("claude".into()),
            None,
            Some("low".into()),
            None,
            Some("high".into()),
        )
        .unwrap();
        let detail = serde_json::to_value(read(&path, &id).unwrap()).unwrap();
        assert_eq!(detail["messages"][0]["effort"], "low");
        assert_eq!(detail["messages"][0]["actualEffort"], "high");
    }
    #[test]
    fn finish_reason_is_shared_by_providers_and_legacy_is_null() {
        let (_dir, path) = database();
        let room = create(&path).unwrap();
        append(&path, &room.id, Role::Assistant, "old", None, None).unwrap();
        let values = [
            ("codex", FinishReason::Completed, "completed"),
            ("claude", FinishReason::Completed, "completed"),
            ("gemini", FinishReason::Completed, "completed"),
            ("gemini", FinishReason::LengthLimit, "length_limit"),
            ("gemini", FinishReason::TurnLimit, "turn_limit"),
            ("gemini", FinishReason::Refusal, "refusal"),
        ];
        for (provider, reason, _) in values {
            append_finished(
                &path,
                &room.id,
                "reply",
                provider.into(),
                None,
                None,
                reason,
            )
            .unwrap();
        }
        let detail = serde_json::to_value(read(&path, &room.id).unwrap()).unwrap();
        assert!(detail["messages"][0]["finishReason"].is_null());
        for (index, (_, _, reason)) in values.iter().enumerate() {
            assert_eq!(detail["messages"][index + 1]["finishReason"], *reason);
        }
    }
    #[test]
    fn fast_retry_is_reused_and_delivery_metadata_survives_reopen() {
        let (_dir, path) = database();
        let chat = create(&path).unwrap();
        let user = append_full(
            &path,
            &chat.id,
            Role::User,
            "hello",
            None,
            None,
            None,
            Some("skill".into()),
        )
        .unwrap();
        assert!(find_fast_retry(&path, &chat.id, "hello", Some("skill"))
            .unwrap()
            .is_none());
        mark_fast_retry(&path, &chat.id, &user.id).unwrap();
        assert_eq!(
            find_fast_retry(&path, &chat.id, "hello", Some("skill"))
                .unwrap()
                .as_deref(),
            Some(user.id.as_str())
        );
        assert_eq!(
            find_fast_retry(&path, &chat.id, "hello", None)
                .unwrap()
                .as_deref(),
            Some(user.id.as_str())
        );
        assert!(find_fast_retry(&path, &chat.id, "different", Some("skill"))
            .unwrap()
            .is_none());
        let prompt = crate::chat_handoff::prepare_excluding(
            &path,
            &chat.id,
            "claude",
            false,
            "hello",
            Some(&user.id),
        )
        .unwrap();
        assert_eq!(prompt.prompt.matches("hello").count(), 1);
        let reply = append(
            &path,
            &chat.id,
            Role::Assistant,
            "answer",
            Some("claude".into()),
            None,
        )
        .unwrap();
        set_delivery_metadata(&path, &chat.id, &reply.id, Some("on"), 3).unwrap();
        let saved = serde_json::to_value(read(&path, &chat.id).unwrap()).unwrap();
        assert_eq!(saved["messages"].as_array().unwrap().len(), 2);
        assert!(saved["messages"][0]["fast"].is_null());
        assert!(saved["messages"][0]["fastState"].is_null());
        assert!(saved["messages"][0]["skill"].is_null());
        assert_eq!(saved["messages"][1]["fast"], true);
        assert_eq!(saved["messages"][1]["notHandedOver"], 3);
        assert!(find_fast_retry(&path, &chat.id, "hello", Some("skill"))
            .unwrap()
            .is_none());
        let second = append(
            &path,
            &chat.id,
            Role::Assistant,
            "next",
            Some("claude".into()),
            None,
        )
        .unwrap();
        set_delivery_metadata(&path, &chat.id, &second.id, Some("off"), 0).unwrap();
        let saved = serde_json::to_value(read(&path, &chat.id).unwrap()).unwrap();
        assert_eq!(saved["messages"][2]["fast"], false);
        assert_eq!(saved["messages"][2]["fastState"], "off");
        assert_eq!(saved["messages"][2]["notHandedOver"], 0);
        set_delivery_metadata(&path, &chat.id, &second.id, Some("cooldown"), 0).unwrap();
        let saved = serde_json::to_value(read(&path, &chat.id).unwrap()).unwrap();
        assert!(saved["messages"][2]["fast"].is_null());
        assert_eq!(saved["messages"][2]["fastState"], "cooldown");
        assert!(set_delivery_metadata(&path, &chat.id, &second.id, Some("unknown"), 0).is_err());
    }
    #[test]
    fn title_is_limited_to_40_chars() {
        let (_d, p) = database();
        let c = create(&p).unwrap();
        append(&p, &c.id, Role::User, &"가".repeat(45), None, None).unwrap();
        assert_eq!(read(&p, &c.id).unwrap().title.chars().count(), 40);
    }
    #[test]
    fn rename_is_preserved() {
        let (_d, p) = database();
        let c = create(&p).unwrap();
        rename(&p, &c.id, "이름").unwrap();
        append(&p, &c.id, Role::User, "새 말", None, None).unwrap();
        assert_eq!(read(&p, &c.id).unwrap().title, "이름");
        assert!(rename(&p, &c.id, " \n").is_err());
    }
    #[test]
    fn search_title_and_message() {
        let (_d, p) = database();
        let c = create(&p).unwrap();
        rename(&p, &c.id, "제목").unwrap();
        append(&p, &c.id, Role::User, "첫 줄\n골반 수업\n끝", None, None).unwrap();
        assert_eq!(search(&p, "제목").unwrap().len(), 1);
        let r = search(&p, "골반").unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].snippet.as_deref(), Some("골반 수업"));
        assert!(search(&p, "없는말").unwrap().is_empty());
    }
    #[test]
    fn trigram_search_preserves_korean_substrings_and_updates() {
        let (_dir, path) = database();
        let room = create(&path).unwrap();
        rename(&path, &room.id, "선생님수업").unwrap();
        let message = append(
            &path,
            &room.id,
            Role::User,
            "할아사나자세에서 선생님께서 호흡합니다",
            None,
            None,
        )
        .unwrap();
        for term in ["아사나", "선생님께", "수업", "자세에서", "호흡"] {
            assert_eq!(search(&path, term).unwrap().len(), 1, "{term}");
        }
        assert!(search(&path, "없는자세").unwrap().is_empty());
        assert!(search(&path, "아사나없는").unwrap().is_empty());
        let connection = db(&path).unwrap();
        connection
            .execute(
                "UPDATE chat_messages SET text=?1 WHERE id=?2",
                rusqlite::params!["다른자세로 바꿉니다", message.id],
            )
            .unwrap();
        assert!(search(&path, "아사나").unwrap().is_empty());
        assert_eq!(search(&path, "다른자세").unwrap().len(), 1);
        let other = create(&path).unwrap();
        connection
            .execute(
                "UPDATE chat_messages SET chat_id=?1 WHERE id=?2",
                rusqlite::params![other.id, message.id],
            )
            .unwrap();
        let found = search(&path, "다른자세").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].chat.id, other.id);
        connection
            .execute("DELETE FROM chat_messages WHERE id=?1", [&message.id])
            .unwrap();
        assert!(search(&path, "다른자세").unwrap().is_empty());
    }
    #[test]
    fn missing_id_errors() {
        let (_d, p) = database();
        assert!(read(&p, "missing").is_err());
        assert!(append(&p, "missing", Role::User, "말", None, None).is_err());
        assert!(rename(&p, "missing", "이름").is_err());
    }
    #[test]
    fn two_creations_in_same_second_wait() {
        let (_d, p) = database();
        let first = create(&p).unwrap();
        let start = std::time::Instant::now();
        let second = create(&p).unwrap();
        assert_ne!(first.id, second.id);
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(list(&p).unwrap().is_empty());
    }
}
