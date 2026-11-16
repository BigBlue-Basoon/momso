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
pub(crate) fn valid_chat_id(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("chat_") else {
        return false;
    };
    if uuid::Uuid::parse_str(suffix).is_ok_and(|value| value.hyphenated().to_string() == suffix) {
        return true;
    }
    let candidate = format!("rec_{suffix}");
    valid_id(&candidate)
}

#[cfg(test)]
mod chat_id_tests {
    use super::valid_chat_id;

    #[test]
    fn accepts_existing_and_uuid_chat_ids_without_accepting_paths() {
        assert!(valid_chat_id("chat_20261115_120000"));
        assert!(valid_chat_id(&format!("chat_{}", uuid::Uuid::new_v4())));
        assert!(!valid_chat_id("chat_../../credentials"));
        assert!(!valid_chat_id(
            "chat_00000000-0000-0000-0000-000000000000/secret"
        ));
    }
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
    if head.read.iter().any(|id| {
        !crate::archive_zet::valid_asset_id(id)
            && !crate::archive_zet::valid_zet_id(id)
            && !crate::chat_recovery::valid_document_reference(id)
    }) || head.wrote.iter().any(|id| {
        !crate::archive_zet::valid_zet_id(id)
            && !crate::archive_zet::valid_asset_id(id)
            && !crate::chat_recovery::valid_document_reference(id)
    }) || head.write_actions.iter().any(|(id, action)| {
        !head.wrote.contains(id) || !["created", "edited", "deleted"].contains(&action.as_str())
    }) || head.evidence.iter().any(|(id, evidence)| {
        !head.wrote.contains(id) || !["tool", "diff"].contains(&evidence.as_str())
    }) || head.unknown_actor.iter().any(|id| !head.wrote.contains(id))
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
        deleted: bool,
    },
    File {
        id: String,
        #[serde(rename = "relativePath")]
        relative_path: String,
        missing: bool,
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
    pub(crate) title: Option<String>,
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
        && !crate::chat_recovery::valid_document_reference(reference)
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
                    let deleted: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM source_asset_deletions d JOIN momso_source_assets a ON a.id=d.asset_id WHERE a.content_sha256=?1)", [&reference[7..]], |row| row.get(0)).map_err(|_| "작업 기록의 참조를 읽지 못했습니다".to_string())?;
                    Ok(ReceiptReference::Asset {
                        id: reference.clone(),
                        original_file_name,
                        deleted,
                    })
                } else if crate::chat_recovery::valid_document_reference(reference) {
                    Ok(ReceiptReference::File {
                        id: reference.clone(),
                        relative_path: reference
                            .strip_prefix("document-path:")
                            .unwrap_or_default()
                            .to_owned(),
                        missing: !crate::chat_recovery::document_exists(root, reference),
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
    // Keep attachment validation and recording in the same app-write critical
    // section as source deletion, so a pending deletion cannot be reattached.
    let _guard = crate::chat_file_changes::lock_app_write();
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
            "SELECT a.content_sha256 FROM momso_source_assets a WHERE a.id=?1 AND a.integrity_state='active' AND NOT EXISTS(SELECT 1 FROM source_asset_deletions d WHERE d.asset_id=a.id)",
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
    let archive = root
        .canonicalize()
        .map_err(|_| "원자료 파일을 찾을 수 없습니다")?;
    let objects = root
        .join("objects")
        .canonicalize()
        .map_err(|_| "원자료 파일을 찾을 수 없습니다")?;
    let path = objects.join("sha256").join(&hash[..2]).join(&hash);
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|_| "원자료 파일을 찾을 수 없습니다")?;
    let canonical = path
        .canonicalize()
        .map_err(|_| "원자료 파일을 찾을 수 없습니다")?;
    if !objects.starts_with(&archive)
        || !canonical.starts_with(&objects)
        || !metadata.is_file()
        || crate::archive_location::is_link_or_reparse(&metadata)
    {
        return Err("원자료 파일을 찾을 수 없습니다".into());
    }
    drop(conn);
    let receipt = create(
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
    )?;
    let conn = crate::storage::open_writable_connection(database)
        .map_err(|_| "원자료를 연결하지 못했습니다".to_string())?;
    conn.execute(
        "INSERT INTO chat_attachments(receipt_id,chat_id,asset_id) VALUES (?1,?2,?3)",
        rusqlite::params![receipt, chat_id, asset_id],
    )
    .map_err(|_| "원자료를 연결하지 못했습니다".to_string())?;
    Ok(receipt)
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
    if input.read.iter().any(|id| {
        !crate::archive_zet::valid_asset_id(id)
            && !crate::archive_zet::valid_zet_id(id)
            && !crate::chat_recovery::valid_document_reference(id)
    }) || input.wrote.iter().any(|id| {
        !crate::archive_zet::valid_zet_id(id)
            && !crate::archive_zet::valid_asset_id(id)
            && !crate::chat_recovery::valid_document_reference(id)
    }) || write_actions.iter().any(|(id, action)| {
        !input.wrote.contains(id) || !["created", "edited", "deleted"].contains(&action.as_str())
    }) || evidence
        .iter()
        .any(|(id, value)| !input.wrote.contains(id) || !["tool", "diff"].contains(&value.as_str()))
        || unknown_actor.iter().any(|id| !input.wrote.contains(id))
    {
        return Err("작업 기록의 참조 번호가 올바르지 않습니다".into());
    }
    let _guard = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "작업 기록을 저장하지 못했습니다".to_string())?;
    let (id, created_at, mut file) = loop {
        let (id, created_at, file) = crate::archive::allocate_file(&root.join("receipts"), "rec")?;
        let saved = root
            .join(".git/momso-deleted-receipts")
            .join(format!("{id}.md"));
        match std::fs::symlink_metadata(saved) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                break (id, created_at, file)
            }
            result => {
                drop(file);
                std::fs::remove_file(root.join("receipts").join(format!("{id}.md")))
                    .map_err(|_| "unavailable")?;
                if result.is_err() {
                    return Err("unavailable".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    };
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReceiptDeleteResult {
    state: &'static str,
    id: String,
    pub(crate) chat_id: Option<String>,
}

pub(crate) fn delete(
    root: &std::path::Path,
    database: &std::path::Path,
    id: &str,
) -> Result<ReceiptDeleteResult, String> {
    if !valid_id(id) {
        return Err("invalid_id".into());
    }
    let _write = crate::chat_file_changes::lock_app_write();
    let _index = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "unavailable")?;
    let receipts = root.join("receipts");
    let git = root.join(".git");
    for directory in [&receipts, &git] {
        let meta = std::fs::symlink_metadata(directory).map_err(|_| "unavailable")?;
        if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
            return Err("unsafe_path".into());
        }
    }
    let path = receipts.join(format!("{id}.md"));
    let meta = std::fs::symlink_metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "not_found"
        } else {
            "unavailable"
        }
    })?;
    if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("unsafe_path".into());
    }
    let text = std::fs::read_to_string(&path).map_err(|_| "unavailable")?;
    let (head, _) = parse(&text).map_err(|_| "invalid_receipt")?;
    if head.id != id {
        return Err("invalid_receipt".into());
    }
    let trash = git.join("momso-deleted-receipts");
    match std::fs::create_dir(&trash) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err("unavailable".into()),
    }
    let meta = std::fs::symlink_metadata(&trash).map_err(|_| "unavailable")?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("unsafe_path".into());
    }
    let saved = trash.join(format!("{id}.md"));
    crate::source_delete::move_without_replacement(&path, &saved).map_err(|_| "unavailable")?;
    let update = (|| -> Result<(), String> {
        let mut conn =
            crate::storage::open_writable_connection(database).map_err(|_| "unavailable")?;
        let tx = conn.transaction().map_err(|_| "unavailable")?;
        for table in ["archive_receipts_read", "archive_receipts_wrote"] {
            tx.execute(&format!("DELETE FROM {table} WHERE receipt_id=?1"), [id])
                .map_err(|_| "unavailable")?;
        }
        tx.execute("DELETE FROM archive_receipts WHERE id=?1", [id])
            .map_err(|_| "unavailable")?;
        tx.execute(
            "DELETE FROM chat_attachments WHERE receipt_id=?1 AND message_id IS NULL",
            [id],
        )
        .map_err(|_| "unavailable")?;
        tx.commit().map_err(|_| "unavailable")?;
        Ok(())
    })();
    if let Err(error) = update {
        if crate::source_delete::move_without_replacement(&saved, &path).is_err() {
            return Err("rollback_failed".into());
        }
        return Err(error);
    }
    Ok(ReceiptDeleteResult {
        state: "deleted",
        id: id.to_owned(),
        chat_id: head.chat,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deleting_receipt_preserves_source_and_sent_attachment_and_rolls_back_failure() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        for folder in ["receipts", "objects", ".git"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        let db = temp.path().join("state.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let bytes = b"source bytes";
        let hash = crate::object_store::digest(bytes);
        let object = root.join("objects/sha256").join(&hash[..2]).join(&hash);
        std::fs::create_dir_all(object.parent().unwrap()).unwrap();
        std::fs::write(&object, bytes).unwrap();
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset','other',?1,?2,?3,'source.txt','test','2026-01-01T00:00:00Z')", rusqlite::params![hash,bytes.len() as i64,format!("sha256/{}/{hash}",&hash[..2])]).unwrap();
        drop(conn);
        let receipt = attach_asset(&root, &db, &chat, "asset").unwrap();
        let message = crate::chat::append_for_send(&db, &chat, "message", None).unwrap();
        let path = root.join("receipts").join(format!("{receipt}.md"));
        let original = std::fs::read(&path).unwrap();
        assert!(delete(&root, &temp.path().join("missing.db"), &receipt).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let result = delete(&root, &db, &receipt).unwrap();
        assert_eq!(result.state, "deleted");
        assert!(!path.exists());
        assert_eq!(std::fs::read(&object).unwrap(), bytes);
        assert_eq!(
            crate::chat::message_attachment_paths(&db, &root, &chat, &message.id)
                .unwrap()
                .len(),
            1
        );
        let copies = std::fs::read_dir(root.join(".git/momso-deleted-receipts"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(copies.len(), 1);
        assert_eq!(std::fs::read(copies[0].path()).unwrap(), original);
        assert!(matches!(delete(&root, &db, "../../outside"), Err(error) if error == "invalid_id"));
        let pending = attach_asset(&root, &db, &chat, "asset").unwrap();
        assert_ne!(pending, receipt);
        delete(&root, &db, &pending).unwrap();
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        let pending_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM chat_attachments WHERE chat_id=?1 AND message_id IS NULL",
                [&chat],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending_count, 0);
        assert_eq!(
            crate::chat::message_attachment_paths(&db, &root, &chat, &message.id)
                .unwrap()
                .len(),
            1
        );
    }

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
        conn.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until) VALUES('asset-1','pending-operation','pending','x','x')", []).unwrap();
        assert!(attach_asset(&root, &db, chat, "asset-1").is_err());
        assert_eq!(std::fs::read_dir(root.join("receipts")).unwrap().count(), 0);
        conn.execute(
            "DELETE FROM source_asset_deletions WHERE asset_id='asset-1'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE momso_source_assets SET integrity_state='quarantined',integrity_issue='test',quarantined_at='x',quarantined_by='test' WHERE id='asset-1'",
            [],
        )
        .unwrap();
        assert!(attach_asset(&root, &db, chat, "asset-1").is_err());
        conn.execute(
            "UPDATE momso_source_assets SET integrity_state='active',integrity_issue=NULL,quarantined_at=NULL,quarantined_by=NULL WHERE id='asset-1'",
            [],
        )
        .unwrap();
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
        let other = "chat_20261115_120001";
        conn.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES (?1,'다른 방','x','x')",
            [other],
        )
        .unwrap();
        attach_asset(&root, &db, other, "asset-1").unwrap();
        let message =
            crate::chat::append_for_send(&db, chat, "이 파일을 읽어 주세요", None).unwrap();
        conn.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until) VALUES('asset-1','interrupted-operation','pending','2026-11-16T00:00:00Z','x')", []).unwrap();
        assert!(crate::chat::message_attachment_paths(&db, &root, chat, &message.id).is_err());
        let pending_items =
            serde_json::to_value(crate::chat_items::list(&root, &db, chat).unwrap()).unwrap();
        assert!(pending_items["files"].as_array().unwrap().is_empty());
        assert!(pending_items["deleted"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == format!("sha256:{hash}")));
        conn.execute(
            "DELETE FROM source_asset_deletions WHERE asset_id='asset-1'",
            [],
        )
        .unwrap();
        let paths = crate::chat::message_attachment_paths(&db, &root, chat, &message.id).unwrap();
        let encoded = serde_json::to_value(paths).unwrap();
        assert_eq!(
            encoded[0]["path"],
            format!("objects/sha256/{}/{}", &hash[..2], hash)
        );
        assert_eq!(encoded[0]["name"], "요가.png");
        let pending: i64 = conn
            .query_row(
                "SELECT count(*) FROM chat_attachments WHERE chat_id=?1 AND message_id IS NULL",
                [other],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 1);
        let next = crate::chat::append_for_send(&db, chat, "다음 말", None).unwrap();
        assert!(
            crate::chat::message_attachment_paths(&db, &root, chat, &next.id)
                .unwrap()
                .is_empty()
        );
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
        conn.execute("DELETE FROM chats WHERE id=?1", ["chat_20261115_120000"])
            .unwrap();
        let remaining =
            serde_json::to_value(chats_for_reference(&root, &db, zet).unwrap()).unwrap();
        let deleted = remaining
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == "chat_20261115_120000")
            .unwrap();
        assert!(deleted["title"].is_null());
        assert!(root.join("receipts/rec_20261115_143004.md").exists());
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
