use serde::{Deserialize, Serialize};

pub fn content_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{:x}", Sha256::digest(text.as_bytes()))
}

pub const EDGE_TYPES: [&str; 5] = [
    "summarizes",
    "derived_from",
    "continues",
    "references",
    "supersedes",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    #[serde(rename = "type")]
    pub kind: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ZetHead {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub kind: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub assets: Vec<String>,
    pub edges: Vec<Edge>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub folder: String,
    #[serde(skip)]
    #[doc(hidden)]
    pub original_header: Option<String>,
}

pub fn valid_zet_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 19
        && &bytes[..4] == b"zet_"
        && bytes[12] == b'_'
        && bytes[4..12]
            .iter()
            .chain(bytes[13..19].iter())
            .all(u8::is_ascii_digit)
        && chrono::NaiveDateTime::parse_from_str(&id[4..], "%Y%m%d_%H%M%S").is_ok()
}

pub fn valid_asset_id(asset: &str) -> bool {
    asset.len() == 71
        && asset.starts_with("sha256:")
        && asset[7..]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

pub fn validate(head: &ZetHead) -> Result<(), String> {
    validate_folder(&head.folder)?;
    if !valid_zet_id(&head.id) {
        return Err("글 번호가 올바르지 않습니다".into());
    }
    if head.title.trim().is_empty() {
        return Err("제목을 입력해 주세요".into());
    }
    if head.summary.trim().is_empty() {
        return Err("요약을 입력해 주세요".into());
    }
    if !["summary", "transcript", "note"].contains(&head.kind.as_str()) {
        return Err("글 종류가 올바르지 않습니다".into());
    }
    if !["draft", "confirmed"].contains(&head.status.as_str()) {
        return Err("글 상태가 올바르지 않습니다".into());
    }
    if chrono::DateTime::parse_from_rfc3339(&head.created_at).is_err()
        || chrono::DateTime::parse_from_rfc3339(&head.updated_at).is_err()
    {
        return Err("글의 시간이 올바르지 않습니다".into());
    }
    for asset in &head.assets {
        if !valid_asset_id(asset) {
            return Err("원본 번호가 올바르지 않습니다".into());
        }
    }
    for edge in &head.edges {
        if !EDGE_TYPES.contains(&edge.kind.as_str()) || !valid_zet_id(&edge.target) {
            return Err("글의 연결이 올바르지 않습니다".into());
        }
    }
    Ok(())
}

pub fn validate_folder(folder: &str) -> Result<(), String> {
    if folder.is_empty() {
        return Ok(());
    }
    if folder.len() > 4096
        || folder.chars().any(char::is_control)
        || folder
            .split('/')
            .any(|part| part.is_empty() || part.trim() != part || part == ".." || part == ".")
        || folder.contains('\\')
    {
        return Err("invalid_folder".into());
    }
    Ok(())
}

pub fn parse(text: &str) -> Result<(ZetHead, &str), String> {
    let (remaining, boundary) = if let Some(text) = text.strip_prefix("---\r\n") {
        (text, "\r\n---\r\n")
    } else if let Some(text) = text.strip_prefix("---\n") {
        (text, "\n---\n")
    } else {
        return Err("글 머리가 올바르지 않습니다".into());
    };
    let (yaml, body) = remaining
        .split_once(boundary)
        .ok_or_else(|| "글 머리가 올바르지 않습니다".to_string())?;
    let fields: serde_yaml::Value =
        serde_yaml::from_str(yaml).map_err(|_| "글 머리가 올바르지 않습니다".to_string())?;
    let keys: Vec<&str> = fields
        .as_mapping()
        .ok_or_else(|| "글 머리가 올바르지 않습니다".to_string())?
        .keys()
        .map(|key| key.as_str().unwrap_or(""))
        .collect();
    let expected = [
        "id",
        "title",
        "summary",
        "kind",
        "status",
        "created_at",
        "updated_at",
        "assets",
        "edges",
    ];
    let without_folder: Vec<_> = keys
        .iter()
        .copied()
        .filter(|key| *key != "folder")
        .collect();
    if without_folder != expected || keys.len() > expected.len() + 1 {
        return Err("글 머리 항목이나 순서가 올바르지 않습니다".into());
    }
    let mut head: ZetHead =
        serde_yaml::from_str(yaml).map_err(|_| "글 머리가 올바르지 않습니다".to_string())?;
    validate(&head)?;
    head.original_header = Some(text[..text.len() - body.len()].to_string());
    Ok((head, body))
}

pub fn render(head: &ZetHead, body: &str) -> Result<String, String> {
    validate(head)?;
    if let Some(original) = &head.original_header {
        let source = format!("{original}{body}");
        if let Ok((mut previous, _)) = parse(&source) {
            let mut current = head.clone();
            previous.original_header = None;
            current.original_header = None;
            if previous == current {
                return Ok(source);
            }
        }
    }
    let yaml = serde_yaml::to_string(head).map_err(|_| "글을 만들지 못했습니다".to_string())?;
    Ok(format!("---\n{}---\n{}", yaml, body))
}
