use crate::archive_receipt::{ChatItemBy, ReceiptHead};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::{collections::HashMap, fs, path::Path};

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ChatLink {
    id: String,
    title: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    id: String,
    title: String,
    summary: String,
    kind: String,
    status: String,
    updated_at: String,
    how: Option<&'static str>,
    by: Option<ChatItemBy>,
    evidence: Option<String>,
    at: String,
}

#[derive(Serialize)]
pub(crate) struct ChatItems {
    documents: Vec<Document>,
    media: Vec<ChatMedia>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatMedia {
    id: String,
    kind: &'static str,
    title: Option<String>,
    created_at: String,
    how: Option<&'static str>,
    by: Option<ChatItemBy>,
    evidence: Option<String>,
    at: String,
    path: String,
}

struct Event {
    head: ReceiptHead,
    attachment: bool,
}

fn events(root: &Path) -> Result<Vec<Event>, String> {
    let mut result = Vec::new();
    for entry in fs::read_dir(root.join("receipts")).map_err(|_| "unavailable")? {
        let entry = entry.map_err(|_| "unavailable")?;
        if !entry.file_type().map_err(|_| "unavailable")?.is_file()
            || entry.path().extension().is_none_or(|e| e != "md")
        {
            continue;
        }
        let text = fs::read_to_string(entry.path()).map_err(|_| "unavailable")?;
        let (head, body) = crate::archive_receipt::parse(&text).map_err(|_| "unavailable")?;
        if entry.file_name() != format!("{}.md", head.id).as_str() {
            return Err("unavailable".into());
        }
        let attachment = head.tool == "person"
            && head.wrote.is_empty()
            && head.read.len() == 1
            && body
                .split_once("## 고친 내용")
                .is_some_and(|(_, part)| part.trim() == "원자료를 채팅방에 첨부했습니다.");
        result.push(Event { head, attachment });
    }
    result.sort_by(|a, b| {
        let time = |e: &Event| chrono::DateTime::parse_from_rfc3339(&e.head.created_at).ok();
        time(a)
            .cmp(&time(b))
            .then_with(|| a.head.id.cmp(&b.head.id))
    });
    Ok(result)
}

fn by(head: &ReceiptHead) -> ChatItemBy {
    if head.tool == "person" {
        ChatItemBy::User
    } else {
        ChatItemBy::Ai {
            provider: head.tool.clone(),
            model: (!head.model.is_empty()).then_some(head.model.clone()),
        }
    }
}

fn write_action(event: &Event, id: &str) -> Option<&'static str> {
    match event.head.write_actions.get(id).map(String::as_str) {
        Some("created") => Some("created"),
        Some("edited") => Some("edited"),
        _ => None,
    }
}
fn actor(event: &Event, id: &str) -> Option<ChatItemBy> {
    (!event.head.unknown_actor.iter().any(|value| value == id)).then(|| by(&event.head))
}

pub(crate) fn enrich_media(
    root: &Path,
    database: &Path,
    media: &mut [crate::media::MediaItem],
) -> Result<(), String> {
    let events = events(root)?;
    let conn = Connection::open(database).map_err(|_| "unavailable")?;
    for item in media {
        let mut chats = HashMap::<String, String>::new();
        for event in &events {
            let Some(chat) = &event.head.chat else {
                continue;
            };
            if !event.head.read.contains(&item.id) && !event.head.wrote.contains(&item.id) {
                continue;
            }
            let title: Option<String> = conn
                .query_row("SELECT title FROM chats WHERE id=?1", [chat], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(|_| "unavailable")?;
            if let Some(title) = title {
                chats.insert(chat.clone(), title);
            }
            if event.attachment {
                item.how = Some("attached");
                item.by = Some(ChatItemBy::User);
                item.evidence = None;
                item.at = Some(event.head.created_at.clone());
            } else if let Some(action) = write_action(event, &item.id) {
                item.how = Some(action);
                item.by = actor(event, &item.id);
                item.evidence = event.head.evidence.get(&item.id).cloned();
                item.at = Some(event.head.created_at.clone());
            }
        }
        let mut links: Vec<_> = chats
            .into_iter()
            .map(|(id, title)| ChatLink { id, title })
            .collect();
        links.sort_by(|a, b| a.id.cmp(&b.id));
        item.chats = links;
    }
    Ok(())
}

pub(crate) fn list(root: &Path, database: &Path, chat_id: &str) -> Result<ChatItems, String> {
    let conn = Connection::open(database).map_err(|_| "unavailable")?;
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM chats WHERE id=?1)",
            [chat_id],
            |row| row.get(0),
        )
        .map_err(|_| "unavailable")?;
    if !exists {
        return Err("채팅방을 찾을 수 없습니다".into());
    }
    let events = events(root)?;
    let mut latest = HashMap::<String, (&Event, bool)>::new();
    for event in &events {
        if event.head.chat.as_deref() != Some(chat_id) {
            continue;
        }
        for id in &event.head.read {
            latest.insert(id.clone(), (event, false));
        }
        for id in &event.head.wrote {
            latest.insert(id.clone(), (event, true));
        }
    }
    let mut documents = Vec::new();
    let mut media_events = HashMap::new();
    for (id, (event, wrote)) in latest {
        if crate::archive_zet::valid_asset_id(&id) {
            media_events.insert(id, event);
            continue;
        }
        let path = root.join("zettels").join(format!("{id}.md"));
        let info = path.canonicalize().ok().and_then(|path| {
            let parent = root.join("zettels").canonicalize().ok()?;
            if !parent.starts_with(root.canonicalize().ok()?)
                || path.parent()? != parent
                || path.file_name()? != format!("{id}.md").as_str()
            {
                return None;
            }
            let text = fs::read_to_string(path).ok()?;
            let (head, _) = crate::archive_zet::parse(&text).ok()?;
            (head.id == id).then_some(head)
        });
        if let Some(head) = info {
            let how = if wrote {
                write_action(event, &id)
            } else {
                Some("read")
            };
            let evidence = event.head.evidence.get(&id).cloned();
            let by = actor(event, &id);
            documents.push(Document {
                id,
                title: head.title,
                summary: head.summary,
                kind: head.kind,
                status: head.status,
                updated_at: head.updated_at,
                how,
                by,
                evidence,
                at: event.head.created_at.clone(),
            });
        }
    }
    documents.sort_by(|a, b| {
        chrono::DateTime::parse_from_rfc3339(&b.at)
            .ok()
            .cmp(&chrono::DateTime::parse_from_rfc3339(&a.at).ok())
            .then_with(|| b.id.cmp(&a.id))
    });
    let files = crate::media::collect(&root.join("objects"), database, None, None, usize::MAX)?;
    let mut media = Vec::new();
    for item in files {
        if let Some(event) = media_events.get(&item.id) {
            let how = if event.attachment {
                Some("attached")
            } else {
                write_action(event, &item.id)
            };
            let evidence = event.head.evidence.get(&item.id).cloned();
            let by = actor(event, &item.id);
            media.push(ChatMedia {
                id: item.id,
                kind: item.kind,
                title: item.title,
                created_at: item.created_at,
                how,
                by,
                evidence,
                at: event.head.created_at.clone(),
                path: item.path,
            });
        }
    }
    media.sort_by(|a, b| {
        chrono::DateTime::parse_from_rfc3339(&b.at)
            .ok()
            .cmp(&chrono::DateTime::parse_from_rfc3339(&a.at).ok())
            .then_with(|| b.id.cmp(&a.id))
    });
    Ok(ChatItems { documents, media })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recorded_references_are_scoped_and_unknown_write_kind_is_null() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        fs::create_dir_all(root.join("receipts")).unwrap();
        fs::create_dir_all(root.join("zettels")).unwrap();
        fs::write(root.join("zettels/zet_20261115_120000.md"), "---\nid: zet_20261115_120000\ntitle: 문서\nsummary: 요약\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T12:00:00+09:00'\nupdated_at: '2026-11-15T12:00:00+09:00'\nassets: []\nedges: []\n---\n본문\n").unwrap();
        fs::create_dir_all(root.join("objects/sha256/aa")).unwrap();
        let hash = "a".repeat(64);
        fs::write(
            root.join("objects/sha256/aa").join(&hash),
            b"RIFF\x00\x00\x00\x00WAVE",
        )
        .unwrap();
        let db = temp.path().join("db.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE chats(id TEXT PRIMARY KEY,title TEXT); CREATE TABLE archive_zets(id TEXT PRIMARY KEY,title TEXT,summary TEXT,kind TEXT,status TEXT,updated_at TEXT); CREATE TABLE momso_source_assets(id TEXT,content_sha256 TEXT,original_file_name TEXT,created_at TEXT,integrity_state TEXT); INSERT INTO chats VALUES('chat_20261115_120000','수업'); INSERT INTO chats VALUES('chat_20261115_120001','다른 방'); INSERT INTO archive_zets VALUES('zet_20261115_120000','문서','요약','note','draft','2026-11-15T12:00:00+09:00');").unwrap();
        let receipt = format!("---\nid: rec_20261115_120001\ncreated_at: '2026-11-15T12:00:01+09:00'\ntool: codex\nmodel: test-model\nchat: chat_20261115_120000\nread: [sha256:{hash}]\nwrote: [zet_20261115_120000]\n---\n## 시킨 말\n없음\n## 고친 내용\n없음\n");
        fs::write(root.join("receipts/rec_20261115_120001.md"), receipt).unwrap();
        let result =
            serde_json::to_value(list(&root, &db, "chat_20261115_120000").unwrap()).unwrap();
        assert_eq!(result["documents"][0]["how"], serde_json::Value::Null);
        assert_eq!(result["documents"][0]["by"]["model"], "test-model");
        assert_eq!(result["media"][0]["kind"], "audio");
        assert_eq!(result["media"][0]["how"], serde_json::Value::Null);
        let other =
            serde_json::to_value(list(&root, &db, "chat_20261115_120001").unwrap()).unwrap();
        assert!(other["documents"].as_array().unwrap().is_empty());
        assert!(other["media"].as_array().unwrap().is_empty());
        let mut items = crate::media::collect(&root.join("objects"), &db, None, None, 100).unwrap();
        enrich_media(&root, &db, &mut items).unwrap();
        let result = serde_json::to_value(&items).unwrap();
        assert_eq!(result[0]["chats"][0]["id"], "chat_20261115_120000");
        assert_eq!(result[0]["by"], serde_json::Value::Null);
        assert_eq!(result[0]["how"], serde_json::Value::Null);
        let attached = format!("---\nid: rec_20261115_120002\ncreated_at: '2026-11-15T12:00:02+09:00'\ntool: person\nmodel: ''\nchat: chat_20261115_120000\nread: [sha256:{hash}]\nwrote: []\n---\n## 시킨 말\n없음\n## 고친 내용\n원자료를 채팅방에 첨부했습니다.\n");
        fs::write(root.join("receipts/rec_20261115_120002.md"), attached).unwrap();
        let result =
            serde_json::to_value(list(&root, &db, "chat_20261115_120000").unwrap()).unwrap();
        assert_eq!(result["media"][0]["how"], "attached");
        assert_eq!(result["media"][0]["by"]["kind"], "user");
        enrich_media(&root, &db, &mut items).unwrap();
        let result = serde_json::to_value(items).unwrap();
        assert_eq!(result[0]["how"], "attached");
        assert_eq!(result[0]["by"]["kind"], "user");
    }
}
