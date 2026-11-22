//! Shared format of archive work receipts.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptHead {
    pub id: String,
    pub created_at: String,
    pub tool: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<String>,
    pub read: Vec<String>,
    pub wrote: Vec<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub write_actions: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub evidence: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown_actor: Vec<String>,
}
pub fn valid_id(id: &str) -> bool {
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
pub fn valid_chat_id(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("chat_") else {
        return false;
    };
    if uuid::Uuid::parse_str(suffix).is_ok_and(|value| value.hyphenated().to_string() == suffix) {
        return true;
    }
    let candidate = format!("rec_{suffix}");
    valid_id(&candidate)
}

pub fn parse(text: &str) -> Result<(ReceiptHead, &str), String> {
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
            && !crate::safe_path::valid_document_reference(id)
    }) || head.wrote.iter().any(|id| {
        !crate::archive_zet::valid_zet_id(id)
            && !crate::archive_zet::valid_asset_id(id)
            && !crate::safe_path::valid_document_reference(id)
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
