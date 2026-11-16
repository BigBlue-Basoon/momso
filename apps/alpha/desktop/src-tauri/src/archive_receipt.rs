use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReceiptHead {
    pub(crate) id: String,
    pub(crate) created_at: String,
    pub(crate) tool: String,
    pub(crate) model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) chat: Option<String>,
    pub(crate) read: Vec<String>,
    pub(crate) wrote: Vec<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub(crate) write_actions: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub(crate) evidence: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) unknown_actor: Vec<String>,
}
pub(crate) fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 19
        && &bytes[..4] == b"rec_"
        && bytes[12] == b'_'
        && bytes[4..12]
            .iter()
            .chain(bytes[13..19].iter())
            .all(u8::is_ascii_digit)
        && chrono::NaiveDateTime::parse_from_str(&id[4..], "%Y%m%d_%H%M%S").is_ok()
}
fn valid_chat_id(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("chat_") else {
        return false;
    };
    let candidate = format!("rec_{suffix}");
    valid_id(&candidate)
}
pub(crate) fn parse(text: &str) -> Result<(ReceiptHead, &str), String> {
    let (rest, delimiter) = if let Some(s) = text.strip_prefix("---\r\n") {
        (s, "\r\n---\r\n")
    } else if let Some(s) = text.strip_prefix("---\n") {
        (s, "\n---\n")
    } else {
        return Err("작업 기록 머리가 올바르지 않습니다".into());
    };
    let (yaml, body) = rest
        .split_once(delimiter)
        .ok_or_else(|| "작업 기록 머리가 올바르지 않습니다".to_string())?;
    let fields: serde_yaml::Value =
        serde_yaml::from_str(yaml).map_err(|_| "작업 기록 머리가 올바르지 않습니다".to_string())?;
    let keys: Vec<&str> = fields
        .as_mapping()
        .ok_or_else(|| "작업 기록 머리가 올바르지 않습니다".to_string())?
        .keys()
        .map(|key| key.as_str().unwrap_or(""))
        .collect();
    let extended = keys.starts_with(&[
        "id",
        "created_at",
        "tool",
        "model",
        "chat",
        "read",
        "wrote",
        "write_actions",
    ]) && matches!(&keys[8..], ["evidence"] | ["evidence", "unknown_actor"]);
    if !extended
        && keys != ["id", "created_at", "tool", "model", "read", "wrote"]
        && keys != ["id", "created_at", "tool", "model", "chat", "read", "wrote"]
        && keys
            != [
                "id",
                "created_at",
                "tool",
                "model",
                "chat",
                "read",
                "wrote",
                "write_actions",
            ]
    {
        return Err("작업 기록 머리 항목이나 순서가 올바르지 않습니다".into());
    }
    let head: ReceiptHead =
        serde_yaml::from_str(yaml).map_err(|_| "작업 기록 머리가 올바르지 않습니다".to_string())?;
    if !valid_id(&head.id)
        || chrono::DateTime::parse_from_rfc3339(&head.created_at).is_err()
        || !["codex", "claude", "gemini", "person"].contains(&head.tool.as_str())
        || head.chat.as_deref().is_some_and(|id| !valid_chat_id(id))
    {
        return Err("작업 기록의 번호, 시각 또는 도구가 올바르지 않습니다".into());
    }
    if !body.lines().any(|line| line == "## 시킨 말")
        || !body.lines().any(|line| line == "## 고친 내용")
    {
        return Err("작업 기록의 내용 구간이 빠졌습니다".into());
    }
    if head
        .read
        .iter()
        .any(|id| !crate::archive_zet::valid_asset_id(id) && !crate::archive_zet::valid_zet_id(id))
        || head.wrote.iter().any(|id| {
            !crate::archive_zet::valid_zet_id(id) && !crate::archive_zet::valid_asset_id(id)
        })
        || head.write_actions.iter().any(|(id, action)| {
            !head.wrote.contains(id) || !["created", "edited"].contains(&action.as_str())
        })
        || head.evidence.iter().any(|(id, evidence)| {
            !head.wrote.contains(id) || !["tool", "diff"].contains(&evidence.as_str())
        })
        || head.unknown_actor.iter().any(|id| !head.wrote.contains(id))
    {
        return Err("작업 기록의 참조 번호가 올바르지 않습니다".into());
    }
    Ok((head, body))
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ReceiptReference {
    Zet {
        id: String,
        title: Option<String>,
        #[serde(rename = "zetKind")]
        zet_kind: Option<String>,
        status: Option<String>,
        missing: bool,
    },
    Asset {
        id: String,
        #[serde(rename = "originalFileName")]
        original_file_name: Option<String>,
    },
}

#[derive(Serialize)]
pub(crate) struct ReceiptReadHead {
    id: String,
    created_at: String,
    tool: String,
    model: String,
    chat: Option<String>,
    read: Vec<ReceiptReference>,
    wrote: Vec<ReceiptReference>,
}

#[derive(Serialize)]
pub(crate) struct ReceiptReadResult {
    head: ReceiptReadHead,
    body: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ChatItemBy {
    User,
    Ai {
        provider: String,
        model: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ReferencingChat {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) at: String,
    pub(crate) by: Option<ChatItemBy>,
}

/// Looks up only links explicitly recorded in receipt files. Old receipts remain unlinked.
pub(crate) fn chats_for_reference(
    root: &std::path::Path,
    database: &std::path::Path,
    reference: &str,
) -> Result<Vec<ReferencingChat>, String> {
    use rusqlite::OptionalExtension;
    if !crate::archive_zet::valid_asset_id(reference)
        && !crate::archive_zet::valid_zet_id(reference)
    {
        return Err("문서 또는 원자료 번호가 올바르지 않습니다".into());
    }
    let conn = crate::storage::open_writable_connection(database)
        .map_err(|_| "채팅방 연결을 읽지 못했습니다".to_string())?;
    let mut latest = std::collections::HashMap::new();
    let entries = std::fs::read_dir(root.join("receipts"))
        .map_err(|_| "작업 내역을 읽지 못했습니다".to_string())?;
    for entry in entries {
        let entry = entry.map_err(|_| "작업 내역을 읽지 못했습니다".to_string())?;
        if !entry
            .file_type()
            .map_err(|_| "작업 내역을 읽지 못했습니다".to_string())?
            .is_file()
            || entry
                .path()
                .extension()
                .is_none_or(|extension| extension != "md")
        {
            continue;
        }
        let text = std::fs::read_to_string(entry.path())
            .map_err(|_| "작업 내역을 읽지 못했습니다".to_string())?;
        let (head, _) = parse(&text).map_err(|_| "작업 내역을 읽지 못했습니다".to_string())?;
        if entry.file_name() != format!("{}.md", head.id).as_str() {
            return Err("작업 내역 번호와 파일 이름이 다릅니다".into());
        }
        let Some(chat_id) = head.chat.as_deref() else {
            continue;
        };
        if !head
            .read
            .iter()
            .chain(&head.wrote)
            .any(|item| item == reference)
        {
            continue;
        }
        let title = conn
            .query_row("SELECT title FROM chats WHERE id=?1", [chat_id], |row| {
                row.get::<_, String>(0)
            })
            .optional()
            .map_err(|_| "채팅방 연결을 읽지 못했습니다".to_string())?;
        let Some(title) = title else {
            continue;
        };
        let time = chrono::DateTime::parse_from_rfc3339(&head.created_at)
            .map_err(|_| "작업 내역의 시간이 올바르지 않습니다".to_string())?;
        let by = if head.unknown_actor.iter().any(|id| id == reference) {
            None
        } else if head.tool == "person" {
            Some(ChatItemBy::User)
        } else {
            Some(ChatItemBy::Ai {
                provider: head.tool.clone(),
                model: (!head.model.is_empty()).then_some(head.model.clone()),
            })
        };
        let item = ReferencingChat {
            id: chat_id.to_string(),
            title,
            at: head.created_at.clone(),
            by,
        };
        let replace = latest.get(chat_id).is_none_or(
            |(previous, previous_id, _): &(
                chrono::DateTime<chrono::FixedOffset>,
                String,
                ReferencingChat,
            )| (time, &head.id) > (*previous, previous_id),
        );
        if replace {
            latest.insert(chat_id.to_string(), (time, head.id.clone(), item));
        }
    }
    let mut results: Vec<_> = latest.into_values().collect();
    results.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    Ok(results.into_iter().map(|(_, _, item)| item).collect())
}

pub(crate) fn read(
    root: &std::path::Path,
    database: &std::path::Path,
    id: &str,
) -> Result<ReceiptReadResult, String> {
    use rusqlite::OptionalExtension;
    if !valid_id(id) {
        return Err("작업 기록 번호가 올바르지 않습니다".into());
    }
    let text = std::fs::read_to_string(root.join("receipts").join(format!("{id}.md")))
        .map_err(|_| "작업 기록을 읽지 못했습니다".to_string())?;
    let (head, body) = parse(&text)?;
    if head.id != id {
        return Err("작업 기록 번호와 파일 이름이 다릅니다".into());
    }
    let conn = crate::storage::open_writable_connection(database)
        .map_err(|_| "작업 기록의 참조를 읽지 못했습니다".to_string())?;
    let resolve = |ids: &[String]| -> Result<Vec<ReceiptReference>, String> {
        ids.iter()
            .map(|reference| {
                if crate::archive_zet::valid_asset_id(reference) {
                    let original_file_name = conn.query_row(
                    "SELECT original_file_name FROM momso_source_assets WHERE content_sha256=?1",
                    [&reference[7..]], |row| row.get::<_, String>(0),
                ).optional().map_err(|_| "작업 기록의 참조를 읽지 못했습니다".to_string())?;
                    Ok(ReceiptReference::Asset {
                        id: reference.clone(),
                        original_file_name,
                    })
                } else {
                    let metadata: Option<(String, String, String)> = conn
                        .query_row(
                            "SELECT title,kind,status FROM archive_zets WHERE id=?1",
                            [reference],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        )
                        .optional()
                        .map_err(|_| "작업 기록의 참조를 읽지 못했습니다".to_string())?;
                    let metadata = metadata.filter(|_| {
                        root.join("zettels")
                            .join(format!("{reference}.md"))
                            .is_file()
                    });
                    let missing = metadata.is_none();
                    let (title, zet_kind, status) = match metadata {
                        Some((title, kind, status)) => (Some(title), Some(kind), Some(status)),
                        None => (None, None, None),
                    };
                    Ok(ReceiptReference::Zet {
                        id: reference.clone(),
                        title,
                        zet_kind,
                        status,
                        missing,
                    })
                }
            })
            .collect()
    };
    Ok(ReceiptReadResult {
        head: ReceiptReadHead {
            id: head.id,
            created_at: head.created_at,
            tool: head.tool,
            model: head.model,
            chat: head.chat,
            read: resolve(&head.read)?,
            wrote: resolve(&head.wrote)?,
        },
        body: body.to_owned(),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReceiptCreate {
    pub(crate) tool: String,
    pub(crate) model: String,
    #[serde(default)]
    pub(crate) chat: Option<String>,
    pub(crate) read: Vec<String>,
    pub(crate) wrote: Vec<String>,
    pub(crate) prompt: String,
    pub(crate) changes: String,
}

pub(crate) fn attach_asset(
    root: &std::path::Path,
    database: &std::path::Path,
    chat_id: &str,
    asset_id: &str,
) -> Result<String, String> {
    use rusqlite::OptionalExtension;
    let conn = crate::storage::open_writable_connection(database)
        .map_err(|_| "원자료를 연결하지 못했습니다".to_string())?;
    let chat_exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM chats WHERE id=?1)",
            [chat_id],
            |row| row.get(0),
        )
        .map_err(|_| "채팅방을 확인하지 못했습니다".to_string())?;
    if !chat_exists {
        return Err("채팅방을 찾을 수 없습니다".into());
    }
    let hash: Option<String> = conn
        .query_row(
            "SELECT content_sha256 FROM momso_source_assets WHERE id=?1",
            [asset_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| "원자료를 확인하지 못했습니다".to_string())?;
    let hash = hash.ok_or("원자료를 찾을 수 없습니다")?;
    let reference = format!("sha256:{hash}");
    if !crate::archive_zet::valid_asset_id(&reference) {
        return Err("원자료 번호가 올바르지 않습니다".into());
    }
    if !root
        .join("objects")
        .join("sha256")
        .join(&hash[..2])
        .join(&hash)
        .is_file()
    {
        return Err("원자료 파일을 찾을 수 없습니다".into());
    }
    drop(conn);
    create(
        root,
        database,
        ReceiptCreate {
            tool: "person".into(),
            model: String::new(),
            chat: Some(chat_id.into()),
            read: vec![reference],
            wrote: vec![],
            prompt: String::new(),
            changes: "원자료를 채팅방에 첨부했습니다.".into(),
        },
    )
}

pub(crate) fn create(
    root: &std::path::Path,
    database: &std::path::Path,
    input: ReceiptCreate,
) -> Result<String, String> {
    create_with_actions(root, database, input, Default::default())
}

pub(crate) fn create_with_actions(
    root: &std::path::Path,
    database: &std::path::Path,
    input: ReceiptCreate,
    write_actions: std::collections::BTreeMap<String, String>,
) -> Result<String, String> {
    create_with_evidence(
        root,
        database,
        input,
        write_actions,
        Default::default(),
        Vec::new(),
    )
}

pub(crate) fn create_with_evidence(
    root: &std::path::Path,
    database: &std::path::Path,
    input: ReceiptCreate,
    write_actions: std::collections::BTreeMap<String, String>,
    evidence: std::collections::BTreeMap<String, String>,
    unknown_actor: Vec<String>,
) -> Result<String, String> {
    use std::io::Write;
    if !["codex", "claude", "gemini", "person"].contains(&input.tool.as_str()) {
        return Err("작업 도구가 올바르지 않습니다".into());
    }
    if input.chat.as_deref().is_some_and(|id| !valid_chat_id(id)) {
        return Err("채팅방 번호가 올바르지 않습니다".into());
    }
    if input
        .read
        .iter()
        .any(|id| !crate::archive_zet::valid_asset_id(id) && !crate::archive_zet::valid_zet_id(id))
        || input.wrote.iter().any(|id| {
            !crate::archive_zet::valid_zet_id(id) && !crate::archive_zet::valid_asset_id(id)
        })
        || write_actions.iter().any(|(id, action)| {
            !input.wrote.contains(id) || !["created", "edited"].contains(&action.as_str())
        })
        || evidence.iter().any(|(id, value)| {
            !input.wrote.contains(id) || !["tool", "diff"].contains(&value.as_str())
        })
        || unknown_actor.iter().any(|id| !input.wrote.contains(id))
    {
        return Err("작업 기록의 참조 번호가 올바르지 않습니다".into());
    }
    let (id, created_at, mut file) = crate::archive::allocate_file(&root.join("receipts"), "rec")?;
    let head = ReceiptHead {
        id: id.clone(),
        created_at,
        tool: input.tool,
        model: input.model,
        chat: input.chat,
        read: input.read,
        wrote: input.wrote,
        write_actions,
        evidence,
        unknown_actor,
    };
    let yaml =
        serde_yaml::to_string(&head).map_err(|_| "작업 기록을 만들지 못했습니다".to_string())?;
    let prompt = if input.prompt.is_empty() {
        "없음"
    } else {
        &input.prompt
    };
    let changes = if input.changes.is_empty() {
        "없음"
    } else {
        &input.changes
    };
    let text = format!("---\n{yaml}---\n## 시킨 말\n{prompt}\n\n## 고친 내용\n{changes}\n");
    file.write_all(text.as_bytes())
        .map_err(|_| "작업 기록을 저장하지 못했습니다".to_string())?;
    file.sync_all()
        .map_err(|_| "작업 기록을 저장하지 못했습니다".to_string())?;
    let mut conn = crate::storage::open_writable_connection(database)
        .map_err(|_| "작업 기록 색인을 열지 못했습니다".to_string())?;
    let tx = conn
        .transaction()
        .map_err(|_| "작업 기록 색인을 갱신하지 못했습니다".to_string())?;
    tx.execute(
        "INSERT INTO archive_receipts(id,created_at,tool,model) VALUES (?1,?2,?3,?4)",
        rusqlite::params![head.id, head.created_at, head.tool, head.model],
    )
    .map_err(|_| "작업 기록 색인을 갱신하지 못했습니다".to_string())?;
    for reference in &head.read {
        tx.execute(
            "INSERT OR IGNORE INTO archive_receipts_read(receipt_id,reference_id) VALUES (?1,?2)",
            rusqlite::params![id, reference],
        )
        .map_err(|_| "작업 기록 색인을 갱신하지 못했습니다".to_string())?;
    }
    for reference in &head.wrote {
        tx.execute(
            "INSERT OR IGNORE INTO archive_receipts_wrote(receipt_id,reference_id) VALUES (?1,?2)",
            rusqlite::params![id, reference],
        )
        .map_err(|_| "작업 기록 색인을 갱신하지 못했습니다".to_string())?;
    }
    tx.commit()
        .map_err(|_| "작업 기록 색인을 갱신하지 못했습니다".to_string())?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn attachment_records_an_existing_asset_in_the_requested_chat() {
        use sha2::{Digest, Sha256};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        std::fs::create_dir_all(root.join("receipts")).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        let chat = "chat_20261115_120000";
        conn.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES (?1,'수업','x','x')",
            [chat],
        )
        .unwrap();
        let bytes = "요가 원자료".as_bytes();
        let hash = format!("{:x}", Sha256::digest(bytes));
        let path = root.join("objects").join("sha256").join(&hash[..2]);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join(&hash), bytes).unwrap();
        conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES ('asset-1','other',?1,?2,?3,'요가.png','person','2026-11-15T12:00:00+09:00')", rusqlite::params![hash,i64::try_from(bytes.len()).unwrap(),format!("sha256/{}/{}", &hash[..2],hash)]).unwrap();
        assert!(attach_asset(&root, &db, "chat_20261115_120001", "asset-1").is_err());
        assert!(attach_asset(&root, &db, chat, "missing").is_err());
        let receipt = attach_asset(&root, &db, chat, "asset-1").unwrap();
        let text =
            std::fs::read_to_string(root.join("receipts").join(format!("{receipt}.md"))).unwrap();
        let (head, _) = parse(&text).unwrap();
        assert_eq!(head.chat.as_deref(), Some(chat));
        assert_eq!(head.read, vec![format!("sha256:{hash}")]);
        assert!(!text.contains("요가 원자료"));
        let linked = chats_for_reference(&root, &db, &format!("sha256:{hash}")).unwrap();
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].id, chat);
    }
    #[test]
    fn reverse_lookup_uses_only_explicit_links_and_latest_event() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        std::fs::create_dir_all(root.join("receipts")).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        for (id, title) in [
            ("chat_20261115_120000", "수업"),
            ("chat_20261115_120001", "상담"),
        ] {
            conn.execute(
                "INSERT INTO chats(id,title,created_at,updated_at) VALUES (?1,?2,'x','x')",
                rusqlite::params![id, title],
            )
            .unwrap();
        }
        let zet = "zet_20261115_143052";
        let asset = format!("sha256:{}", "a".repeat(64));
        let save = |second: u8, chat: Option<&str>, tool: &str, model: &str, refs: &[&str]| {
            let id = format!("rec_20261115_1430{second:02}");
            let h = ReceiptHead {
                id: id.clone(),
                created_at: format!("2026-11-15T14:30:{second:02}+09:00"),
                tool: tool.into(),
                model: model.into(),
                chat: chat.map(str::to_owned),
                read: refs.iter().map(|value| (*value).to_string()).collect(),
                wrote: vec![],
                write_actions: Default::default(),
                evidence: Default::default(),
                unknown_actor: Default::default(),
            };
            let yaml = serde_yaml::to_string(&h).unwrap();
            std::fs::write(
                root.join("receipts").join(format!("{id}.md")),
                format!("---\n{yaml}---\n## 시킨 말\n없음\n## 고친 내용\n없음\n"),
            )
            .unwrap();
        };
        save(
            1,
            Some("chat_20261115_120000"),
            "codex",
            "gpt-test",
            &[zet, &asset],
        );
        save(2, None, "codex", "gpt-test", &[zet]);
        save(
            3,
            Some("chat_20261115_120001"),
            "claude",
            "claude-test",
            &[zet],
        );
        save(4, Some("chat_20261115_120000"), "person", "", &[zet]);
        let chats = chats_for_reference(&root, &db, zet).unwrap();
        let json = serde_json::to_value(chats).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 2);
        assert_eq!(json[0]["id"], "chat_20261115_120000");
        assert_eq!(json[0]["title"], "수업");
        assert_eq!(json[0]["by"]["kind"], "user");
        assert_eq!(json[1]["by"]["provider"], "claude");
        assert_eq!(json[1]["by"]["model"], "claude-test");
        let linked =
            serde_json::to_value(crate::archive_zet::linked(&root, &db, zet).unwrap()).unwrap();
        assert_eq!(linked["chats"], json);
        let chats = serde_json::to_value(chats_for_reference(&root, &db, &asset).unwrap()).unwrap();
        assert_eq!(chats.as_array().unwrap().len(), 1);
        assert_eq!(chats[0]["by"]["provider"], "codex");
        assert!(chats_for_reference(&root, &db, "bad").is_err());
        std::fs::write(
            root.join("receipts").join("rec_20261115_143005.md"),
            "잘못된 내용",
        )
        .unwrap();
        assert_eq!(
            chats_for_reference(&root, &db, zet).unwrap_err(),
            "작업 내역을 읽지 못했습니다"
        );
    }
    #[test]
    fn receipt_chat_is_optional_and_preserved() {
        let original = "---\nid: rec_20261115_143110\ncreated_at: 2026-11-15T14:31:10+09:00\ntool: codex\nmodel: example\nread: []\nwrote: []\n---\n## 시킨 말\n없음\n## 고친 내용\n없음\n";
        assert!(parse(original).unwrap().0.chat.is_none());
        let linked = original.replace(
            "model: example\n",
            "model: example\nchat: chat_20261115_143052\n",
        );
        assert_eq!(
            parse(&linked).unwrap().0.chat.as_deref(),
            Some("chat_20261115_143052")
        );
        assert!(parse(&linked.replace("chat_20261115_143052", "invalid")).is_err());
    }
    #[test]
    fn receipt_read_resolves_mixed_references_and_missing_zets_without_changing_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        crate::archive::initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let zet = crate::archive_zet::create(
            &root,
            &db,
            crate::archive_zet::ZetCreate {
                title: "수업".into(),
                summary: "요약".into(),
                kind: "note".into(),
                body: "본문".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        let hash = "a".repeat(64);
        let asset = format!("sha256:{hash}");
        let unnamed = format!("sha256:{}", "b".repeat(64));
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES ('asset-1','audio_recording',?1,0,'sha256/aa/test','녹음.m4a','person','2026-11-15T14:30:52+09:00')", [&hash]).unwrap();
        let missing = "zet_20261115_143052".to_string();
        let id = create(
            &root,
            &db,
            ReceiptCreate {
                tool: "codex".into(),
                model: "model".into(),
                chat: None,
                read: vec![zet.clone(), asset.clone(), unnamed.clone(), missing.clone()],
                wrote: vec![zet.clone(), missing.clone()],
                prompt: "시킨 말".into(),
                changes: "고친 내용".into(),
            },
        )
        .unwrap();
        let path = root.join("receipts").join(format!("{id}.md"));
        let before = std::fs::read(&path).unwrap();
        let result = serde_json::to_value(read(&root, &db, &id).unwrap()).unwrap();
        assert_eq!(result["head"]["read"][0]["type"], "zet");
        assert_eq!(result["head"]["read"][0]["title"], "수업");
        assert_eq!(result["head"]["read"][0]["zetKind"], "note");
        assert_eq!(result["head"]["read"][0]["status"], "draft");
        assert_eq!(result["head"]["read"][1]["type"], "asset");
        assert_eq!(result["head"]["read"][1]["id"], asset);
        assert_eq!(result["head"]["read"][1]["originalFileName"], "녹음.m4a");
        assert!(result["head"]["read"][2]["originalFileName"].is_null());
        assert_eq!(result["head"]["read"][3]["missing"], true);
        assert_eq!(result["head"]["wrote"][1]["id"], missing);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(root.join("zettels").join(format!("{zet}.md"))).unwrap();
        let deleted = serde_json::to_value(read(&root, &db, &id).unwrap()).unwrap();
        assert_eq!(deleted["head"]["read"][0]["missing"], true);
        assert!(deleted["head"]["read"][0]["title"].is_null());
        assert_eq!(deleted["head"]["wrote"][0]["missing"], true);
    }

    #[test]
    fn preserves_prompt_exactly_without_reading_referenced_sources() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        crate::archive::initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let prompt = "원문 인용: 그대로\n두 번째 줄";
        let id = create(
            &root,
            &db,
            ReceiptCreate {
                tool: "codex".into(),
                model: "model".into(),
                chat: None,
                read: vec![format!("sha256:{}", "a".repeat(64))],
                wrote: vec![],
                prompt: prompt.into(),
                changes: String::new(),
            },
        )
        .unwrap();
        let text = std::fs::read_to_string(root.join("receipts").join(format!("{id}.md"))).unwrap();
        let (head, body) = parse(&text).unwrap();
        assert_eq!(head.read.len(), 1);
        assert_eq!(
            body,
            format!("## 시킨 말\n{prompt}\n\n## 고친 내용\n없음\n")
        );
    }
}
