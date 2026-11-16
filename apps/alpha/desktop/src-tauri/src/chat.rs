use chrono::{Local, SecondsFormat};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};
use uuid::Uuid;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatSummary {
    id: String,
    title: String,
    created_at: String,
    updated_at: String,
    message_count: i64,
    group_id: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatMessage {
    pub(crate) id: String,
    role: Role,
    text: String,
    created_at: String,
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    skill: Option<String>,
    fast: Option<bool>,
    not_handed_over: Option<i64>,
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
    created_at: String,
    updated_at: String,
    messages: Vec<ChatMessage>,
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
    })
}
fn message(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatMessage> {
    Ok(ChatMessage {
        id: row.get(0)?,
        role: Role::from_db(row.get(1)?)?,
        text: row.get(2)?,
        created_at: row.get(3)?,
        provider: row.get(4)?,
        model: row.get(5)?,
        effort: row.get(6)?,
        skill: row.get(7)?,
        fast: row.get(8)?,
        not_handed_over: row.get(9)?,
    })
}
const SELECT_SUMMARY: &str = "SELECT c.id,c.title,c.created_at,c.updated_at,(SELECT count(*) FROM chat_messages m WHERE m.chat_id=c.id),c.group_id FROM chats c";
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
        let id = format!("chat_{}", now.format("%Y%m%d_%H%M%S"));
        let time = now.to_rfc3339_opts(SecondsFormat::Nanos, false);
        let n = c.execute("INSERT INTO chats(id,title,created_at,updated_at) VALUES (?1,'',?2,?2) ON CONFLICT(id) DO NOTHING", params![id,time]).map_err(|_| "채팅방을 만들지 못했습니다".to_string())?;
        if n == 1 {
            return one(&c, &id);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
pub(crate) fn list(path: &Path) -> Result<Vec<ChatSummary>, String> {
    let c = db(path)?;
    let mut q = c
        .prepare(&format!(
            "{SELECT_SUMMARY} ORDER BY c.updated_at DESC,c.rowid DESC"
        ))
        .map_err(|_| "채팅방 목록을 읽지 못했습니다".to_string())?;
    let rows = q
        .query_map([], summary)
        .map_err(|_| "채팅방 목록을 읽지 못했습니다".to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "채팅방 목록을 읽지 못했습니다".to_string())
}
pub(crate) fn read(path: &Path, id: &str) -> Result<ChatDetail, String> {
    let c = db(path)?;
    let h = one(&c, id)?;
    let mut q = c.prepare("SELECT id,role,text,created_at,provider,model,effort,skill,fast,not_handed_over FROM chat_messages WHERE chat_id=?1 ORDER BY rowid").map_err(|_| "채팅방을 읽지 못했습니다".to_string())?;
    let rows = q
        .query_map([id], message)
        .map_err(|_| "채팅방을 읽지 못했습니다".to_string())?;
    let messages = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "채팅방을 읽지 못했습니다".to_string())?;
    Ok(ChatDetail {
        id: h.id,
        title: h.title,
        created_at: h.created_at,
        updated_at: h.updated_at,
        messages,
    })
}
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
    let mut c = db(path)?;
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| "말을 저장하지 못했습니다".to_string())?;
    let h = one(&tx, id)?;
    let time = stamp();
    let msg_id = Uuid::new_v4().to_string();
    tx.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at,provider,model,effort,skill) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![msg_id,id,role.as_str(),text,time,provider,model,effort,skill]).map_err(|_| "말을 저장하지 못했습니다".to_string())?;
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
        created_at: time,
        provider,
        model,
        effort,
        skill,
        fast: None,
        not_handed_over: None,
    })
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
    fast: Option<bool>,
    omitted: usize,
) -> Result<(), String> {
    let omitted = i64::try_from(omitted).map_err(|_| "채팅방을 저장하지 못했습니다".to_string())?;
    let changed = db(path)?.execute("UPDATE chat_messages SET fast=?1,not_handed_over=?2 WHERE chat_id=?3 AND id=?4 AND role='assistant'", params![fast, omitted, chat_id, message_id]).map_err(|_| "채팅방을 저장하지 못했습니다".to_string())?;
    if changed != 1 {
        return Err("채팅방을 저장하지 못했습니다".into());
    }
    Ok(())
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
    let sql="SELECT c.id,c.title,c.created_at,c.updated_at,(SELECT count(*) FROM chat_messages m WHERE m.chat_id=c.id), c.group_id, (SELECT text FROM chat_messages m WHERE m.chat_id=c.id AND instr(m.text,?1)>0 ORDER BY m.rowid LIMIT 1) AS match_text FROM chats c WHERE instr(c.title,?1)>0 OR EXISTS (SELECT 1 FROM chat_messages m WHERE m.chat_id=c.id AND instr(m.text,?1)>0) ORDER BY c.updated_at DESC,c.rowid DESC";
    let mut q = c
        .prepare(sql)
        .map_err(|_| "채팅방을 검색하지 못했습니다".to_string())?;
    let rows = q
        .query_map([query], |row| {
            let text: Option<String> = row.get(6)?;
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
    fn database() -> (tempfile::TempDir, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("test.db");
        let c = Connection::open(&p).unwrap();
        c.execute_batch(include_str!("../migrations/0009_chats.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0010_chat_codex.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0011_chat_effort.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0012_chat_groups.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0070_chat_skill.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0072_chat_delivery.sql"))
            .unwrap();
        (d, p)
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
        set_delivery_metadata(&path, &chat.id, &reply.id, Some(true), 3).unwrap();
        let saved = serde_json::to_value(read(&path, &chat.id).unwrap()).unwrap();
        assert_eq!(saved["messages"].as_array().unwrap().len(), 2);
        assert!(saved["messages"][0]["fast"].is_null());
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
        set_delivery_metadata(&path, &chat.id, &second.id, Some(false), 0).unwrap();
        let saved = serde_json::to_value(read(&path, &chat.id).unwrap()).unwrap();
        assert_eq!(saved["messages"][2]["fast"], false);
        assert_eq!(saved["messages"][2]["notHandedOver"], 0);
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
        assert_eq!(list(&p).unwrap().len(), 2);
    }
}
