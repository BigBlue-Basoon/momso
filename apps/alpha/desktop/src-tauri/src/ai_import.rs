use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const DISCOVERY_LIMIT: usize = 80;
const MAX_SESSION_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConversationMessage {
    role: String,
    text: String,
    timestamp: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConversationPreview {
    id: String,
    provider: String,
    title: String,
    created_at: Option<String>,
    updated_at: Option<String>,
    project_label: Option<String>,
    message_count: usize,
    source_name: String,
    sample_messages: Vec<AiConversationMessage>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConversation {
    id: String,
    provider: String,
    title: String,
    created_at: Option<String>,
    updated_at: Option<String>,
    project_label: Option<String>,
    message_count: usize,
    source_name: String,
    messages: Vec<AiConversationMessage>,
}

struct ParsedConversation {
    id: String,
    provider: String,
    title: String,
    created_at: Option<String>,
    updated_at: Option<String>,
    project_label: Option<String>,
    source_name: String,
    messages: Vec<AiConversationMessage>,
}

impl ParsedConversation {
    fn preview(&self) -> AiConversationPreview {
        AiConversationPreview {
            id: self.id.clone(),
            provider: self.provider.clone(),
            title: self.title.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            project_label: self.project_label.clone(),
            message_count: self.messages.len(),
            source_name: self.source_name.clone(),
            sample_messages: self.messages.iter().take(5).cloned().collect(),
        }
    }

    fn into_conversation(self) -> AiConversation {
        AiConversation {
            id: self.id,
            provider: self.provider,
            title: self.title,
            created_at: self.created_at,
            updated_at: self.updated_at,
            project_label: self.project_label,
            message_count: self.messages.len(),
            source_name: self.source_name,
            messages: self.messages,
        }
    }
}

pub(crate) fn provider_root(provider: &str) -> Result<PathBuf, String> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| "사용자 홈 폴더를 찾지 못했습니다".to_string())?;

    match provider {
        "codex" => Ok(home.join(".codex").join("sessions")),
        "claude-code" => Ok(home.join(".claude").join("projects")),
        _ => Err("이 서비스는 내보내기 파일로 가져와 주세요".to_string()),
    }
}

fn collect_jsonl_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if fs::symlink_metadata(&path)
            .map_or(true, |m| crate::archive_location::is_link_or_reparse(&m))
        {
            continue;
        }
        if file_type.is_dir() {
            collect_jsonl_files(&path, files);
        } else if file_type.is_file()
            && path.extension().and_then(|extension| extension.to_str()) == Some("jsonl")
        {
            files.push(path);
        }
    }
}

fn extract_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_string(),
        Value::Array(values) => values
            .iter()
            .map(extract_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        Value::Object(object) => {
            if let Some(parts) = object.get("parts") {
                return extract_text(parts);
            }
            if let Some(Value::String(content_type)) = object.get("type") {
                if !matches!(content_type.as_str(), "text" | "input_text" | "output_text") {
                    return String::new();
                }
            }
            if let Some(Value::String(text)) = object.get("text") {
                return text.trim().to_string();
            }
            if let Some(content) = object.get("content") {
                return extract_text(content);
            }
            String::new()
        }
        _ => String::new(),
    }
}

fn strip_tagged_block(mut text: String, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    while let Some(start) = text.find(&open) {
        let Some(relative_end) = text[start..].find(&close) else {
            break;
        };
        let end = start + relative_end + close.len();
        text.replace_range(start..end, "");
    }
    text
}

fn clean_user_text(text: String) -> String {
    let mut cleaned = text;
    if cleaned.starts_with("# AGENTS.md instructions") {
        if let Some(end) = cleaned.find("</INSTRUCTIONS>") {
            cleaned = cleaned[end + "</INSTRUCTIONS>".len()..].to_string();
        }
    }
    for tag in [
        "environment_context",
        "permissions instructions",
        "app-context",
        "skills_instructions",
        "apps_instructions",
        "plugins_instructions",
    ] {
        cleaned = strip_tagged_block(cleaned, tag);
    }
    cleaned.trim().to_string()
}

fn compact_title(
    title: Option<String>,
    messages: &[AiConversationMessage],
    fallback: &str,
) -> String {
    let source = title
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            messages
                .iter()
                .find(|message| message.role == "user")
                .map(|message| message.text.clone())
        })
        .unwrap_or_else(|| fallback.to_string());
    let compact = source.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= 72 {
        compact
    } else {
        format!("{}...", compact.chars().take(69).collect::<String>())
    }
}

fn path_project_label(value: Option<&str>) -> Option<String> {
    value.and_then(|path| {
        Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
    })
}

fn value_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_string)
}

fn parse_codex_rows(rows: &[Value], path: &Path) -> Option<ParsedConversation> {
    let session = rows
        .iter()
        .find(|row| row.get("type").and_then(Value::as_str) == Some("session_meta"));
    let payload = session.and_then(|row| row.get("payload"));
    let mut messages = Vec::new();

    for row in rows {
        if row.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        let Some(message) = row.get("payload") else {
            continue;
        };
        if message.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(role @ ("user" | "assistant")) = message.get("role").and_then(Value::as_str)
        else {
            continue;
        };
        let mut text = extract_text(message.get("content").unwrap_or(&Value::Null));
        if role == "user" {
            text = clean_user_text(text);
        }
        if text.is_empty() {
            continue;
        }
        messages.push(AiConversationMessage {
            role: role.to_string(),
            text,
            timestamp: value_string(row.get("timestamp")),
        });
    }
    if messages.is_empty() {
        return None;
    }

    let source_name = path.file_name()?.to_string_lossy().into_owned();
    let id = payload
        .and_then(|value| value.get("id").or_else(|| value.get("session_id")))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| source_name.trim_end_matches(".jsonl").to_string());
    let created_at = payload
        .and_then(|value| value.get("timestamp"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| session.and_then(|row| value_string(row.get("timestamp"))));
    let updated_at = rows
        .iter()
        .rev()
        .find_map(|row| value_string(row.get("timestamp")));
    let cwd = payload
        .and_then(|value| value.get("cwd"))
        .and_then(Value::as_str);

    Some(ParsedConversation {
        id,
        provider: "codex".to_string(),
        title: compact_title(None, &messages, "Codex 채팅방"),
        created_at,
        updated_at,
        project_label: path_project_label(cwd),
        source_name,
        messages,
    })
}

fn parse_claude_rows(rows: &[Value], path: &Path) -> Option<ParsedConversation> {
    let custom_title = rows.iter().rev().find_map(|row| {
        (row.get("type").and_then(Value::as_str) == Some("custom-title"))
            .then(|| value_string(row.get("customTitle")))
            .flatten()
    });
    let first_message_row = rows.iter().find(|row| row.get("message").is_some());
    let mut messages = Vec::new();

    for row in rows {
        if row.get("isMeta").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(message) = row.get("message") else {
            continue;
        };
        let Some(role @ ("user" | "assistant")) = message
            .get("role")
            .and_then(Value::as_str)
            .or_else(|| row.get("type").and_then(Value::as_str))
        else {
            continue;
        };
        let text = extract_text(message.get("content").unwrap_or(&Value::Null));
        if text.is_empty() {
            continue;
        }
        messages.push(AiConversationMessage {
            role: role.to_string(),
            text,
            timestamp: value_string(row.get("timestamp")),
        });
    }
    if messages.is_empty() {
        return None;
    }

    let source_name = path.file_name()?.to_string_lossy().into_owned();
    let id = first_message_row
        .and_then(|row| row.get("sessionId"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| source_name.trim_end_matches(".jsonl").to_string());
    let created_at = first_message_row.and_then(|row| value_string(row.get("timestamp")));
    let updated_at = rows
        .iter()
        .rev()
        .find_map(|row| value_string(row.get("timestamp")));
    let cwd = first_message_row
        .and_then(|row| row.get("cwd"))
        .and_then(Value::as_str);

    Some(ParsedConversation {
        id,
        provider: "claude-code".to_string(),
        title: compact_title(custom_title, &messages, "Claude Code 채팅방"),
        created_at,
        updated_at,
        project_label: path_project_label(cwd),
        source_name,
        messages,
    })
}

fn parse_local_file(provider: &str, path: &Path) -> Result<Option<ParsedConversation>, String> {
    let file = fs::File::open(path).map_err(|_| "invalid_conversation_id")?;
    parse_opened_file(provider, file, path)
}

fn parse_opened_file(
    provider: &str,
    file: fs::File,
    path: &Path,
) -> Result<Option<ParsedConversation>, String> {
    #[cfg(windows)]
    if !crate::chat_items::opened_path_matches(&file, path) {
        return Err("invalid_conversation_id".into());
    }
    let metadata = file.metadata().map_err(|_| "invalid_conversation_id")?;
    if metadata.len() > MAX_SESSION_BYTES {
        return Ok(None);
    }
    let rows = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .collect::<Vec<_>>();

    Ok(match provider {
        "codex" => parse_codex_rows(&rows, path),
        "claude-code" => parse_claude_rows(&rows, path),
        _ => None,
    })
}

#[derive(Default)]
struct LocalResults(Mutex<HashMap<String, (String, PathBuf)>>);
fn local_results() -> &'static LocalResults {
    static RESULTS: OnceLock<LocalResults> = OnceLock::new();
    RESULTS.get_or_init(LocalResults::default)
}

fn safe_local_path(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "invalid_conversation_id")?;
    if relative
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
        || path.extension().and_then(|x| x.to_str()) != Some("jsonl")
    {
        return Err("invalid_conversation_id".into());
    }
    let mut current = root.to_path_buf();
    for component in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(component) = component {
            current.push(component.as_os_str());
        }
        let meta = fs::symlink_metadata(&current).map_err(|_| "invalid_conversation_id")?;
        if crate::archive_location::is_link_or_reparse(&meta) {
            return Err("invalid_conversation_id".into());
        }
    }
    let canonical_root = root.canonicalize().map_err(|_| "invalid_conversation_id")?;
    let canonical_path = path.canonicalize().map_err(|_| "invalid_conversation_id")?;
    if !canonical_path.starts_with(canonical_root) {
        return Err("invalid_conversation_id".into());
    }
    Ok(canonical_path)
}

pub fn discover_local(provider: &str) -> Result<Vec<AiConversationPreview>, String> {
    let root = provider_root(provider)?;
    if let Some(parent) = root.parent() {
        if fs::symlink_metadata(parent)
            .is_ok_and(|m| crate::archive_location::is_link_or_reparse(&m))
        {
            return Err("invalid_conversation_id".into());
        }
    }
    discover_in(provider, &root, local_results())
}

fn discover_in(
    provider: &str,
    root: &Path,
    registry: &LocalResults,
) -> Result<Vec<AiConversationPreview>, String> {
    let mut results = registry.0.lock().map_err(|_| "unavailable")?;
    results.retain(|_, (p, _)| p != provider);
    if !root.exists() {
        return Ok(Vec::new());
    }
    if fs::symlink_metadata(root).map_or(true, |m| crate::archive_location::is_link_or_reparse(&m))
    {
        return Err("invalid_conversation_id".into());
    }
    let mut files = Vec::new();
    collect_jsonl_files(root, &mut files);
    if provider == "claude-code" {
        files.retain(|path| is_primary_claude_session(path));
    }
    files.sort_by_key(|path| {
        std::cmp::Reverse(
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok(),
        )
    });

    let mut previews = Vec::new();
    for path in files.into_iter().take(DISCOVERY_LIMIT) {
        let Ok(safe) = safe_local_path(root, &path) else {
            continue;
        };
        if let Some(conversation) = parse_local_file(provider, &safe)? {
            let id = uuid::Uuid::new_v4().to_string();
            let mut preview = conversation.preview();
            preview.id = id.clone();
            results.insert(id, (provider.to_owned(), path));
            previews.push(preview);
        }
    }
    Ok(previews)
}

fn is_primary_claude_session(path: &Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
        return false;
    };
    if stem.len() != 36 {
        return false;
    }
    stem.chars().enumerate().all(|(index, character)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            character == '-'
        } else {
            character.is_ascii_hexdigit()
        }
    })
}

pub fn load_local(provider: &str, ids: &[String]) -> Result<Vec<AiConversation>, String> {
    let root = provider_root(provider)?;
    if let Some(parent) = root.parent() {
        if fs::symlink_metadata(parent)
            .is_ok_and(|m| crate::archive_location::is_link_or_reparse(&m))
        {
            return Err("invalid_conversation_id".into());
        }
    }
    load_in(provider, &root, ids, local_results())
}

fn load_in(
    provider: &str,
    root: &Path,
    ids: &[String],
    registry: &LocalResults,
) -> Result<Vec<AiConversation>, String> {
    let results = registry.0.lock().map_err(|_| "unavailable")?;
    let mut conversations = Vec::new();
    for id in ids {
        let (saved_provider, saved_path) = results.get(id).ok_or("invalid_conversation_id")?;
        if saved_provider != provider {
            return Err("invalid_conversation_id".into());
        }
        let path = safe_local_path(root, saved_path)?;
        if let Some(conversation) = parse_local_file(provider, &path)? {
            let mut result = conversation.into_conversation();
            result.id = id.clone();
            conversations.push(result);
        }
    }
    Ok(conversations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn opened_conversation_handle_cannot_point_outside_selected_path() {
        let temp = tempfile::tempdir().unwrap();
        let expected = temp.path().join("expected.jsonl");
        let outside = temp.path().join("outside.jsonl");
        fs::write(&expected, "{}").unwrap();
        fs::write(&outside, "{}").unwrap();
        let handle = fs::File::open(&outside).unwrap();
        assert!(parse_opened_file("codex", handle, &expected.canonicalize().unwrap()).is_err());
    }

    #[test]
    fn discovery_uses_opaque_ids_and_rejects_unknown_or_outside_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("session.jsonl");
        let data = [
            serde_json::json!({"type":"session_meta","payload":{"id":"fixture"}}),
            serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}),
        ];
        fs::write(
            &file,
            data.iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let results = LocalResults::default();
        let found = discover_in("codex", &root, &results).unwrap();
        assert_eq!(found.len(), 1);
        let id = found[0].id.clone();
        assert!(!serde_json::to_string(&found[0])
            .unwrap()
            .contains(file.to_str().unwrap()));
        assert_eq!(
            load_in("codex", &root, std::slice::from_ref(&id), &results)
                .unwrap()
                .len(),
            1
        );
        assert!(load_in("codex", &root, &["invented".into()], &results).is_err());
        assert!(load_in("claude-code", &root, std::slice::from_ref(&id), &results).is_err());
        results.0.lock().unwrap().insert(
            "outside".into(),
            ("codex".into(), root.join("..").join("outside.jsonl")),
        );
        assert!(load_in("codex", &root, &["outside".into()], &results).is_err());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let outside = tmp.path().join("outside");
            fs::create_dir_all(&outside).unwrap();
            fs::copy(&file, outside.join("linked.jsonl")).unwrap();
            let junction = root.join("junction");
            let output = std::process::Command::new("cmd.exe")
                .args(["/C", "mklink", "/J"])
                .arg(&junction)
                .arg(&outside)
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(output.status.success());
            results.0.lock().unwrap().insert(
                "link".into(),
                ("codex".into(), junction.join("linked.jsonl")),
            );
            assert!(load_in("codex", &root, &["link".into()], &results).is_err());
        }
    }

    #[test]
    fn parses_codex_user_and_assistant_messages_without_runtime_context() {
        let rows = vec![
            serde_json::json!({"type":"session_meta","timestamp":"2026-07-14T00:00:00Z","payload":{"id":"codex-1","cwd":"C:\\work\\momso"}}),
            serde_json::json!({"type":"response_item","timestamp":"2026-07-14T00:01:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>hidden</environment_context>실제 요청"}]}}),
            serde_json::json!({"type":"response_item","timestamp":"2026-07-14T00:02:00Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"실제 답변"}]}}),
            serde_json::json!({"type":"response_item","payload":{"type":"function_call","name":"ignored"}}),
        ];
        let parsed = parse_codex_rows(&rows, Path::new("session.jsonl")).expect("conversation");
        assert_eq!(parsed.id, "codex-1");
        assert_eq!(parsed.project_label.as_deref(), Some("momso"));
        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.messages[0].text, "실제 요청");
        assert_eq!(parsed.messages[1].text, "실제 답변");
    }

    #[test]
    fn parses_claude_code_custom_title_and_text_blocks() {
        let rows = vec![
            serde_json::json!({"type":"custom-title","customTitle":"제품 방향 회의","sessionId":"claude-1"}),
            serde_json::json!({"type":"user","sessionId":"claude-1","cwd":"C:\\work\\momso","timestamp":"2026-07-14T01:00:00Z","message":{"role":"user","content":"질문"}}),
            serde_json::json!({"type":"assistant","sessionId":"claude-1","timestamp":"2026-07-14T01:01:00Z","message":{"role":"assistant","content":[{"type":"text","text":"답변"},{"type":"tool_use","name":"ignored"}]}}),
        ];
        let parsed = parse_claude_rows(&rows, Path::new("session.jsonl")).expect("conversation");
        assert_eq!(parsed.id, "claude-1");
        assert_eq!(parsed.title, "제품 방향 회의");
        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.messages[1].text, "답변");
    }

    #[test]
    #[ignore = "manual smoke test for local desktop conversation folders"]
    fn discovers_conversations_from_this_computer() {
        for provider in ["codex", "claude-code"] {
            let previews = discover_local(provider).expect("local discovery");
            let message_count = previews
                .iter()
                .map(|preview| preview.message_count)
                .sum::<usize>();
            println!(
                "provider={provider} conversations={} messages={message_count}",
                previews.len()
            );
            assert!(!previews.is_empty(), "no {provider} conversations found");
            assert!(message_count > 0, "no {provider} messages parsed");
        }
    }
}
