use crate::{
    approval::{ApprovalChoice, ApprovalRequest, ApprovalTarget, ChangeKind},
    chat, chat_claude,
    chat_codex::ApprovalChannel,
    chat_context, chat_file_changes, process,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{ChildStdin, ChildStdout, Command},
};
use tokio_util::sync::CancellationToken;

fn executable() -> Option<PathBuf> {
    process::resolve_interactive_tool("agy").or_else(|| {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|p| p.join("agy/bin/agy.exe"))
            .filter(|p| p.is_file())
    })
}

pub(crate) fn previous(db: &Path, id: &str) -> Result<Option<String>, String> {
    let c = Connection::open(db).map_err(|_| "채팅방 저장소를 열지 못했습니다")?;
    c.query_row(
        "SELECT value FROM app_runtime_meta WHERE key=?1",
        [format!("chat_gemini_session:{id}")],
        |r| r.get(0),
    )
    .optional()
    .map_err(|_| "채팅방을 읽지 못했습니다".into())
}

fn save_session(db: &Path, id: &str, session: &str, key: &str) -> Result<(), String> {
    if session.is_empty() || session.len() > 512 || session.contains(key) {
        return Err("채팅방 정보를 저장하지 못했습니다".into());
    }
    Connection::open(db).map_err(|_| "채팅방 저장소를 열지 못했습니다")?
        .execute("INSERT INTO app_runtime_meta(key,value,updated_at) VALUES(?1,?2,CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at", params![format!("chat_gemini_session:{id}"),session])
        .map_err(|_| "채팅방 정보를 저장하지 못했습니다")?;
    Ok(())
}

async fn send(input: &mut ChildStdin, value: &Value) -> Result<(), String> {
    input
        .write_all(format!("{value}\n").as_bytes())
        .await
        .map_err(|_| "AI에 요청을 보내지 못했습니다".to_string())?;
    input
        .flush()
        .await
        .map_err(|_| "AI에 요청을 보내지 못했습니다".to_string())
}

async fn read(
    lines: &mut Lines<BufReader<ChildStdout>>,
    cancel: &CancellationToken,
) -> Result<Value, String> {
    loop {
        let line = tokio::select! {
            _ = cancel.cancelled() => return Err("요청을 중단했습니다".into()),
            v = tokio::time::timeout(Duration::from_secs(180), lines.next_line()) => v.map_err(|_| "AI의 응답 시간이 초과되었습니다")?.map_err(|_| "AI의 답을 읽지 못했습니다")?.ok_or("AI 연결이 끊겼습니다")?,
        };
        if line.len() > 2 * 1024 * 1024 {
            return Err("AI의 답을 읽지 못했습니다".into());
        }
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            return Ok(value);
        }
    }
}

async fn cancel_turn(
    input: &mut ChildStdin,
    lines: &mut Lines<BufReader<ChildStdout>>,
    session: &str,
) {
    if send(
        input,
        &json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}}),
    )
    .await
    .is_err()
    {
        return;
    }
    let drain = async {
        while let Ok(Some(line)) = lines.next_line().await {
            if line.len() > 2 * 1024 * 1024 {
                break;
            }
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if value["id"] == 7 && value.get("method").is_none() {
                break;
            }
            if value["method"] == "session/request_permission" && value.get("id").is_some() {
                if send(input, &json!({"jsonrpc":"2.0","id":value["id"],"result":{"outcome":{"outcome":"cancelled"}}})).await.is_err() { break; }
            } else if value.get("method").is_some() && value.get("id").is_some()
                && send(input, &json!({"jsonrpc":"2.0","id":value["id"],"error":{"code":-32601,"message":"Unsupported request"}})).await.is_err() {
                break;
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), drain).await;
}

async fn request(
    input: &mut ChildStdin,
    lines: &mut Lines<BufReader<ChildStdout>>,
    cancel: &CancellationToken,
    id: u64,
    method: &str,
    params: Value,
    pending: &mut VecDeque<Value>,
) -> Result<Value, String> {
    send(
        input,
        &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
    )
    .await?;
    loop {
        let value = read(lines, cancel).await?;
        if value["id"] == id && value.get("method").is_none() {
            if authentication_required(&value) {
                return Err("__momso_auth_failed".into());
            }
            return value
                .get("result")
                .cloned()
                .ok_or_else(|| "AI 연결을 완료하지 못했습니다".into());
        }
        if value["method"] == "session/update"
            && matches!(
                value["params"]["update"]["sessionUpdate"].as_str(),
                Some("usage_update" | "session_info_update")
            )
        {
            if pending.len() >= 256 {
                return Err("AI의 답을 읽지 못했습니다".into());
            }
            pending.push_back(value);
        } else if value.get("id").is_some() && value.get("method").is_some() {
            send(input, &json!({"jsonrpc":"2.0","id":value["id"],"error":{"code":-32601,"message":"Unsupported request"}})).await?;
        }
    }
}

// ACP's auth_required means authentication is required; it does not identify
// whether a saved API key is invalid, expired, or missing other access.
fn authentication_required(value: &Value) -> bool {
    value.pointer("/error/code").and_then(Value::as_i64) == Some(-32000)
}

fn options(value: &Value, kind: &str) -> Option<String> {
    value
        .as_array()?
        .iter()
        .find(|v| v["kind"] == kind)?
        .get("optionId")?
        .as_str()
        .map(str::to_owned)
}

fn merge_tool(tools: &mut HashMap<String, Value>, update: &Value) -> Value {
    let Some(id) = update["toolCallId"].as_str() else {
        return update.clone();
    };
    let entry = tools.entry(id.to_owned()).or_insert_with(|| json!({}));
    if let (Some(target), Some(source)) = (entry.as_object_mut(), update.as_object()) {
        for (key, value) in source {
            if !value.is_null() {
                target.insert(key.clone(), value.clone());
            }
        }
    }
    entry.clone()
}

fn tool_references(archive: &Path, tool: &Value) -> Vec<String> {
    let mut paths: Vec<&str> = tool["locations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|location| location["path"].as_str())
        .collect();
    if matches!(tool["kind"].as_str(), Some("edit" | "delete" | "move")) {
        for content in tool["content"].as_array().into_iter().flatten() {
            if content["type"] == "diff" {
                if let Some(path) = content["path"].as_str() {
                    if !paths.contains(&path) {
                        paths.push(path);
                    }
                }
            }
        }
    }
    paths
        .into_iter()
        .filter_map(|path| chat_file_changes::reference(archive, path))
        .collect()
}

fn approval(chat_id: &str, params: &Value) -> ApprovalRequest {
    let tool = &params["toolCall"];
    let kind = tool["kind"].as_str().unwrap_or("");
    let change = match kind {
        "delete" => Some(ChangeKind::Delete),
        _ => None,
    };
    let mut paths: Vec<&str> = tool["locations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v["path"].as_str())
        .collect();
    for diff in tool["content"].as_array().into_iter().flatten() {
        if diff["type"] == "diff" {
            if let Some(path) = diff["path"].as_str() {
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
    }
    let targets: Vec<_> = paths
        .into_iter()
        .map(|v| {
            let path = v;
            let diff = tool["content"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|c| c["type"] == "diff" && c["path"] == path);
            let change = diff
                .and_then(|d| {
                    if d.get("oldText").is_some_and(Value::is_null) {
                        Some(ChangeKind::Create)
                    } else if d.get("oldText").is_some_and(Value::is_string) {
                        Some(ChangeKind::Edit)
                    } else {
                        None
                    }
                })
                .or(change);
            ApprovalTarget {
                path: path.into(),
                change,
            }
        })
        .collect();
    let change = if targets.len() == 1 {
        targets[0].change
    } else {
        None
    };
    let mut choices = Vec::new();
    if options(&params["options"], "allow_once").is_some() {
        choices.push(ApprovalChoice::AllowOnce);
    }
    if options(&params["options"], "reject_once").is_some() {
        choices.push(ApprovalChoice::Deny);
    }
    choices.push(ApprovalChoice::Cancel);
    let title = tool["title"].as_str().unwrap_or("");
    let description = if kind == "execute" {
        match tool
            .pointer("/rawInput/command")
            .and_then(Value::as_str)
            .filter(|command| !command.is_empty() && *command != title)
        {
            Some(command) if title.is_empty() => command.to_owned(),
            Some(command) => format!("{title}\n{command}"),
            None => title.to_owned(),
        }
    } else {
        title.to_owned()
    };
    ApprovalRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        chat_id: Some(chat_id.into()),
        provider: "gemini".into(),
        kind: if kind == "execute" {
            "command"
        } else if matches!(kind, "edit" | "delete" | "move") {
            "file_change"
        } else {
            "other"
        }
        .into(),
        description,
        change,
        targets,
        reason: None,
        options: choices,
        setup: None,
    }
}

fn current_config<'a>(v: &'a Value, id: &str) -> Option<&'a str> {
    v["configOptions"]
        .as_array()?
        .iter()
        .find(|c| c["id"] == id)?["currentValue"]
        .as_str()
}

fn discard_ambiguous_usage(pending: &mut VecDeque<Value>) {
    pending.retain(|value| value["params"]["update"]["sessionUpdate"] != "usage_update");
}

fn update_title(title: &mut Option<String>, update: &Value) {
    match update.get("title") {
        None => (),
        Some(Value::Null) => *title = None,
        Some(Value::String(value)) => *title = (!value.trim().is_empty()).then(|| value.clone()),
        _ => (),
    }
}

fn session_id(prior: Option<&str>, response: &Value) -> Result<String, String> {
    let returned = match response.get("sessionId") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if !value.is_empty() => Some(value.as_str()),
        _ => return Err("AI 세션을 읽지 못했습니다".into()),
    };
    match (prior, returned) {
        (Some(""), _) => Err("AI 세션을 읽지 못했습니다".into()),
        (Some(expected), Some(actual)) if expected != actual => {
            Err("AI 세션이 일치하지 않습니다".into())
        }
        (Some(expected), _) => Ok(expected.to_owned()),
        (None, Some(actual)) if !actual.is_empty() => Ok(actual.to_owned()),
        _ => Err("AI 세션을 읽지 못했습니다".into()),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run<F: FnMut(&str, Value) + Send>(
    db: &Path,
    archive: &Path,
    id: &str,
    text: &str,
    model: Option<&str>,
    effort: Option<&str>,
    mode: &str,
    skill: Option<&str>,
    key: String,
    approvals: Option<ApprovalChannel>,
    cancel: CancellationToken,
    emit: &mut F,
) -> Result<(), String> {
    let exe = executable().ok_or("Antigravity CLI를 찾지 못했습니다")?;
    let config_dir = db
        .parent()
        .ok_or("AI 저장소를 찾지 못했습니다")?
        .join("gemini-acp");
    std::fs::create_dir_all(&config_dir).map_err(|_| "AI 저장소를 열지 못했습니다")?;
    let mut cmd = Command::new(exe);
    cmd.args(["--settings", GEMINI_SETTINGS, "acp", "--gemini-dir"])
        .arg(config_dir)
        .arg("--log-file")
        .arg(if cfg!(windows) { "NUL" } else { "/dev/null" });
    run_command(
        cmd, db, archive, id, text, model, effort, mode, skill, key, approvals, cancel, emit,
    )
    .await
}

const GEMINI_SETTINGS: &str =
    "{\"modelProvider\":\"gemini\",\"permissions\":{\"deny\":[\"write_file(objects/)\"]}}";

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_command<F: FnMut(&str, Value) + Send>(
    mut cmd: Command,
    db: &Path,
    archive: &Path,
    id: &str,
    text: &str,
    model: Option<&str>,
    effort: Option<&str>,
    mode: &str,
    skill: Option<&str>,
    key: String,
    approvals: Option<ApprovalChannel>,
    cancel: CancellationToken,
    emit: &mut F,
) -> Result<(), String> {
    cmd.env_remove("GOOGLE_API_KEY")
        .env("GEMINI_API_KEY", &key)
        .current_dir(archive)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let mut child = cmd
        .spawn()
        .map_err(|_| "Antigravity CLI를 시작하지 못했습니다")?;
    #[cfg(windows)]
    crate::setup_bridge::bind(id, child.id());
    let result = async {
        let mut input = child.stdin.take().ok_or("AI에 요청을 보내지 못했습니다")?;
        let mut lines = BufReader::new(child.stdout.take().ok_or("AI의 답을 읽지 못했습니다")?).lines();
        let mut pending = VecDeque::new();
        let init = request(&mut input, &mut lines, &cancel, 1, "initialize", json!({"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"momso","version":env!("CARGO_PKG_VERSION")}}), &mut pending).await?;
        request(&mut input, &mut lines, &cancel, 2, "authenticate", json!({"methodId":"gemini-api-key"}), &mut pending).await?;
        let prior = previous(db, id)?;
        if prior.is_some() && !init.pointer("/agentCapabilities/sessionCapabilities/resume").is_some_and(Value::is_object) { return Err("AI 채팅방을 이어갈 수 없습니다".into()); }
        let server = crate::momso_mcp::acp_config(&crate::momso_mcp::executable()?, id);
        let (method, params) = if let Some(s) = prior.as_deref() { ("session/resume", json!({"sessionId":s,"cwd":archive,"mcpServers":[server]})) } else { ("session/new", json!({"cwd":archive,"mcpServers":[server]})) };
        let mut config = request(&mut input, &mut lines, &cancel, 3, method, params, &mut pending).await?;
        let session = session_id(prior.as_deref(), &config)?;
        let wanted_mode = match mode { "default" => "default", "auto_edit" => "accept-edits", "plan" => "plan", _ => return Err("이 권한 단계는 사용할 수 없습니다".into()) };
        for (n, option, wanted) in [(4, "mode", Some(wanted_mode)), (5, "model", model), (6, "thought_level", effort)] {
            if let Some(wanted) = wanted.filter(|v| !v.trim().is_empty()) {
                if current_config(&config, option) != Some(wanted) {
                    if option == "model" { discard_ambiguous_usage(&mut pending); }
                    config = request(&mut input, &mut lines, &cancel, n, "session/set_config_option", json!({"sessionId":session,"configId":option,"value":wanted}), &mut pending).await?;
                    if option == "model" { discard_ambiguous_usage(&mut pending); }
                    if current_config(&config, option) != Some(wanted) { return Err("AI 설정을 적용하지 못했습니다".into()); }
                }
            }
        }
        let actual_model = current_config(&config, "model").map(str::to_owned);
        let actual_effort = current_config(&config, "thought_level").map(str::to_owned);
        if let Some(m) = &actual_model { emit("momso-internal-observed-model", json!(m)); }
        if let Some(e) = &actual_effort { emit("momso-internal-observed-effort", json!(e)); }
        let prompt = if let Some(skill) = skill { format!("/{skill} {text}") } else { text.to_owned() };
        send(&mut input, &json!({"jsonrpc":"2.0","id":7,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":prompt}]}})).await?;
        let mut answer = String::new();
        let mut title: Option<String> = None;
        let mut tools: HashMap<String, Value> = HashMap::new();
        loop {
            if cancel.is_cancelled() { cancel_turn(&mut input, &mut lines, &session).await; return Err("요청을 중단했습니다".into()); }
            let v = if let Some(v) = pending.pop_front() { v } else { match read(&mut lines, &cancel).await { Ok(v) => v, Err(e) => { if cancel.is_cancelled() { cancel_turn(&mut input, &mut lines, &session).await; } return Err(e); } } };
            if v["method"] == "session/request_permission" {
                let p = &v["params"];
                if !p.is_object() || !p["toolCall"].is_object() { return Err("AI의 요청을 읽지 못했습니다".into()); }
                if p["sessionId"].as_str().is_some_and(|s| s != session) { return Err("AI 세션이 일치하지 않습니다".into()); }
                let mut combined = p.clone();
                combined["toolCall"] = merge_tool(&mut tools, &p["toolCall"]);
                let choice = if let Some(channel) = &approvals { channel(approval(id, &combined), cancel.clone()).await.unwrap_or(ApprovalChoice::Cancel) } else { ApprovalChoice::Cancel };
                let option = match choice { ApprovalChoice::AllowOnce => options(&p["options"],"allow_once"), ApprovalChoice::Deny => options(&p["options"],"reject_once"), _ => None };
                let stop_turn = cancel.is_cancelled() || option.is_none();
                let outcome = match option { Some(id) if !stop_turn => json!({"outcome":"selected","optionId":id}), _ => json!({"outcome":"cancelled"}) };
                send(&mut input, &json!({"jsonrpc":"2.0","id":v["id"],"result":{"outcome":outcome}})).await?;
                if stop_turn { cancel_turn(&mut input, &mut lines, &session).await; return Err("요청을 중단했습니다".into()); }
            } else if v["method"] == "session/update" {
                if v["params"]["sessionId"].as_str().is_some_and(|s| s != session) { continue; }
                let u = &v["params"]["update"];
                match u["sessionUpdate"].as_str().unwrap_or("") {
                    "agent_message_chunk" => if u["content"]["type"] == "text" { if let Some(delta) = u["content"]["text"].as_str() { answer.push_str(delta); emit("chat-stream", json!({"chatId":id,"delta":delta})); } },
                    "session_info_update" => update_title(&mut title, u),
                    "usage_update" => if let Some(used) = u["used"].as_i64() { if let Some(context) = chat_context::update_measurement(db,id,"gemini",actual_model.as_deref(),used,u["size"].as_i64().filter(|v| *v > 0))? { emit("chat-context-updated",json!({"chatId":id,"context":context})); } },
                    "tool_call" | "tool_call_update" if u["toolCallId"].as_str().is_some() => {
                        let entry = merge_tool(&mut tools, u);
                        if entry["status"] == "completed" {
                            let refs = tool_references(archive, &entry);
                            match entry["kind"].as_str() { Some("read") => emit("momso-internal-file-reads",json!(refs)), Some("edit"|"delete"|"move") => emit("momso-internal-file-writes",json!(refs)), _ => {} }
                        }
                    },
                    _ => {}
                }
            } else if v["id"] == 7 && v.get("method").is_none() {
                if authentication_required(&v) { return Err("__momso_auth_failed".into()); }
                let stop_reason = v["result"]["stopReason"].as_str().ok_or("AI가 응답을 완료하지 못했습니다")?;
                match stop_reason {
                    "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" if !answer.is_empty() => (),
                    "cancelled" => return Err("요청을 중단했습니다".into()),
                    "refusal" => return Err("AI가 요청을 거절했습니다".into()),
                    _ => return Err("AI가 응답을 완료하지 못했습니다".into()),
                }
                save_session(db, id, &session, &key)?;
                let context_model = actual_model.clone();
                let reason = match stop_reason { "max_tokens" => chat::FinishReason::LengthLimit, "max_turn_requests" => chat::FinishReason::TurnLimit, "refusal" => chat::FinishReason::Refusal, _ => chat::FinishReason::Completed };
                let msg = chat::append_finished(db,id,&answer,"gemini".into(),actual_model,actual_effort,reason)?;
                let mut done = json!({"chatId":id,"messageId":msg.id,"actualEffort":null,"fastState":null,"stopReason":null,"finishReason":reason});
                if stop_reason != "end_turn" { done["stopReason"] = json!(stop_reason); }
                emit("chat-done",done);
                if let Some(old) = chat_claude::initial_title(db,id) {
                        let mut next_title = title.take();
                        let collect_title = async {
                            loop {
                                let Ok(message) = read(&mut lines, &cancel).await else { break };
                                if message["method"] == "session/update" {
                                    if message["params"]["sessionId"].as_str().is_some_and(|s| s != session) { continue; }
                                    let update = &message["params"]["update"];
                                    if update["sessionUpdate"] == "session_info_update" {
                                        update_title(&mut next_title, update);
                                    } else if update["sessionUpdate"] == "usage_update" {
                                        if let Some(used) = update["used"].as_i64() {
                                            if let Ok(Some(context)) = chat_context::update_measurement(db,id,"gemini",context_model.as_deref(),used,update["size"].as_i64().filter(|v| *v > 0)) { emit("chat-context-updated",json!({"chatId":id,"context":context})); }
                                        }
                                    }
                                } else if message.get("method").is_some() && message.get("id").is_some()
                                    && send(&mut input, &json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32601,"message":"Unsupported request"}})).await.is_err() {
                                    break;
                                }
                            }
                        };
                        let _ = tokio::time::timeout(Duration::from_secs(3), collect_title).await;
                        title = next_title;
                    if let Some(t) = title.as_deref() { if chat_claude::store_generated_title(db,id,&old,t) { if let Ok(saved) = chat::title(db,id) { emit("chat-renamed",json!({"chatId":id,"title":saved})); } } }
                }
                return Ok(());
            } else if v.get("method").is_some() && v.get("id").is_some() {
                send(&mut input,&json!({"jsonrpc":"2.0","id":v["id"],"error":{"code":-32601,"message":"Unsupported request"}})).await?;
            }
        }
    }.await;
    if result.is_err()
        || !matches!(
            tokio::time::timeout(Duration::from_secs(5), child.wait()).await,
            Ok(Ok(_))
        )
    {
        let _ = child.kill().await;
    }
    let _ = child.wait().await;
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn auth_required_uses_acp_error_code_only() {
        assert!(super::authentication_required(
            &serde_json::json!({"error":{"code":-32000}})
        ));
        assert!(!super::authentication_required(
            &serde_json::json!({"error":{"code":401,"message":"auth_required"}})
        ));
        assert!(!super::authentication_required(
            &serde_json::json!({"result":{"error":{"code":-32000}}})
        ));
    }
    use super::*;

    #[test]
    fn settings_deny_writes_to_originals() {
        let value: serde_json::Value = serde_json::from_str(GEMINI_SETTINGS).unwrap();
        assert_eq!(value["modelProvider"], "gemini");
        assert_eq!(value["permissions"]["deny"][0], "write_file(objects/)");
    }

    #[test]
    fn session_title_updates_distinguish_omission_and_clear() {
        let mut title = None;
        update_title(&mut title, &json!({"title":"first"}));
        update_title(&mut title, &json!({"updatedAt":"2026-11-16T00:00:00Z"}));
        assert_eq!(title.as_deref(), Some("first"));
        update_title(&mut title, &json!({"title":null}));
        assert_eq!(title, None);
        update_title(&mut title, &json!({"title":"final"}));
        assert_eq!(title.as_deref(), Some("final"));
    }

    #[test]
    fn model_switch_discards_usage_but_keeps_title_updates() {
        let mut pending = VecDeque::from([
            json!({"params":{"update":{"sessionUpdate":"usage_update","used":42}}}),
            json!({"params":{"update":{"sessionUpdate":"session_info_update","title":"title"}}}),
        ]);
        discard_ambiguous_usage(&mut pending);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["params"]["update"]["title"], "title");
    }

    #[test]
    fn resume_allows_an_empty_result_but_rejects_a_different_session() {
        assert_eq!(session_id(Some("saved"), &json!({})).unwrap(), "saved");
        assert_eq!(
            session_id(Some("saved"), &json!({"sessionId":"saved"})).unwrap(),
            "saved"
        );
        assert!(session_id(Some("saved"), &json!({"sessionId":"other"})).is_err());
        assert!(session_id(Some("saved"), &json!({"sessionId":7})).is_err());
        assert!(session_id(Some(""), &json!({})).is_err());
        assert!(session_id(None, &json!({})).is_err());
        assert_eq!(
            session_id(None, &json!({"sessionId":"new"})).unwrap(),
            "new"
        );
    }

    #[test]
    fn sparse_permission_preserves_reported_tool_information() {
        let mut tools = HashMap::new();
        merge_tool(
            &mut tools,
            &json!({"toolCallId":"edit-1","kind":"edit","title":"Edit a file","locations":[{"path":"a.md"}]}),
        );
        let combined = merge_tool(
            &mut tools,
            &json!({"toolCallId":"edit-1","kind":null,"title":null,"status":"pending"}),
        );
        let request = approval(
            "room",
            &json!({"toolCall":combined,"options":[{"kind":"allow_once","optionId":"yes"}]}),
        );
        assert_eq!(request.description, "Edit a file");
        assert_eq!(request.kind, "file_change");
        assert_eq!(request.targets[0].path, "a.md");
    }

    #[test]
    fn completed_diff_paths_are_recorded_as_tool_writes() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("zettels")).unwrap();
        let file = d.path().join("zettels/zet_20261116_030000.md");
        std::fs::write(&file, "content").unwrap();
        let diff = json!({"kind":"edit","content":[{"type":"diff","path":file}]});
        assert_eq!(tool_references(d.path(), &diff), ["zet_20261116_030000"]);
        assert!(
            tool_references(d.path(), &json!({"kind":"read","content":diff["content"]})).is_empty()
        );
    }

    #[test]
    fn approval_uses_only_explicit_one_time_options_and_diff() {
        let params = json!({"toolCall":{"kind":"edit","title":"Write","locations":[{"path":"a.md"}],"content":[{"type":"diff","path":"a.md","oldText":null,"newText":"hi"}]},"options":[{"kind":"allow_once","optionId":"yes"},{"kind":"allow_always","optionId":"always"},{"kind":"reject_once","optionId":"no"}]});
        let result = approval("room", &params);
        assert_eq!(result.chat_id.as_deref(), Some("room"));
        assert_eq!(result.change, Some(ChangeKind::Create));
        assert_eq!(
            result.options,
            vec![
                ApprovalChoice::AllowOnce,
                ApprovalChoice::Deny,
                ApprovalChoice::Cancel
            ]
        );
        assert_eq!(
            options(&params["options"], "allow_once").as_deref(),
            Some("yes")
        );
        let diff_only = approval(
            "room",
            &json!({"toolCall":{"kind":"edit","content":[{"type":"diff","path":"b.md","oldText":"before","newText":"after"}]},"options":[]}),
        );
        assert_eq!(diff_only.targets[0].path, "b.md");
        assert_eq!(diff_only.change, Some(ChangeKind::Edit));
        let command = approval(
            "room",
            &json!({"toolCall":{"kind":"execute","title":"Run","rawInput":{"command":"git status --short"}},"options":[]}),
        );
        assert_eq!(command.description, "Run\ngit status --short");
    }

    #[test]
    fn session_roundtrip_rejects_secret() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("db");
        Connection::open(&db).unwrap().execute_batch("CREATE TABLE app_runtime_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL,updated_at TEXT NOT NULL)").unwrap();
        assert!(save_session(&db, "room", "prefix-dummy-key", "dummy-key").is_err());
        save_session(&db, "room", "opaque-session", "dummy-key").unwrap();
        assert_eq!(
            previous(&db, "room").unwrap().as_deref(),
            Some("opaque-session")
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn fake_acp_streams_approves_and_resumes_without_saving_key() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("db.sqlite");
        crate::storage::initialize(&db, &d.path().join("backups"), "test").unwrap();
        let id = serde_json::to_value(chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let script = d.path().join("fake.ps1");
        std::fs::write(&script, r#"
if ($env:GEMINI_API_KEY -ne 'dummy-test-only') { exit 10 }
if ($env:GOOGLE_API_KEY) { exit 19 }
while ($line = [Console]::ReadLine()) {
  $v = $line | ConvertFrom-Json
  switch ($v.method) {
    'initialize' { '{"jsonrpc":"2.0","id":1,"result":{"agentCapabilities":{"sessionCapabilities":{"resume":{}}}}}' }
    'authenticate' { if ($v.params.methodId -ne 'gemini-api-key') { exit 11 }; '{"jsonrpc":"2.0","id":2,"result":{}}' }
    'session/new' { if ($v.params.mcpServers.Count -ne 1 -or $v.params.mcpServers[0].name -ne 'momso_internal_runtime_v1' -or $v.params.mcpServers[0].args[0] -ne '--momso-mcp-stdio') { exit 18 }; '{"jsonrpc":"2.0","id":3,"result":{"sessionId":"test-session","configOptions":[{"id":"mode","currentValue":"default"},{"id":"model","currentValue":"fake"},{"id":"thought_level","currentValue":"low"}]}}' }
    'session/resume' { if ($v.params.sessionId -ne 'test-session' -or $v.params.mcpServers[0].name -ne 'momso_internal_runtime_v1') { exit 12 }; '{"jsonrpc":"2.0","id":3,"result":{"configOptions":[{"id":"mode","currentValue":"default"},{"id":"model","currentValue":"fake"},{"id":"thought_level","currentValue":"low"}]}}' }
    'session/set_config_option' {
      if ($v.params.configId -ne 'mode' -or $v.params.value -ne 'plan') { exit 17 }
      '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"test-session","update":{"sessionUpdate":"usage_update","used":64,"size":100}}}'
      '{"jsonrpc":"2.0","id":4,"result":{"configOptions":[{"id":"mode","currentValue":"plan"},{"id":"model","currentValue":"fake"},{"id":"thought_level","currentValue":"low"}]}}'
    }
    'session/prompt' {
      if ($env:MOMSO_EXPECT_SKILL -eq 'yes' -and -not $v.params.prompt[0].text.StartsWith('/sample ')) { exit 14 }
      if ($env:MOMSO_EXPECT_DECISION -in @('cancelled','error')) {
        '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"partial"}}}}'
        if ($env:MOMSO_EXPECT_DECISION -eq 'error') { exit 20 }
      }
      '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call","toolCallId":"edit-1","kind":"edit","title":"Write","locations":[{"path":"a.md"}]}}}'
      '{"jsonrpc":"2.0","id":99,"method":"session/request_permission","params":{"toolCall":{"toolCallId":"edit-1"},"options":[{"kind":"allow_once","optionId":"yes"},{"kind":"reject_once","optionId":"no"}]}}'
      $response = [Console]::ReadLine() | ConvertFrom-Json
      if ($env:MOMSO_EXPECT_DECISION -eq 'cancelled') {
        if ($response.result.outcome.outcome -ne 'cancelled') { exit 15 }
        $stop = [Console]::ReadLine() | ConvertFrom-Json
        if ($stop.method -ne 'session/cancel') { exit 16 }
        [System.IO.File]::WriteAllText($env:MOMSO_FAKE_CANCEL_MARKER, 'cancelled')
        '{"jsonrpc":"2.0","id":7,"result":{"stopReason":"cancelled"}}'
        exit 0
      }
      if ($response.result.outcome.optionId -ne $env:MOMSO_EXPECT_DECISION) { exit 13 }
      '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"session_info_update","title":"Superseded"}}}'
      '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"usage_update","used":32,"size":100}}}'
      '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}}}'
      switch ($env:MOMSO_STOP_REASON) {
        'max_tokens' { '{"jsonrpc":"2.0","id":7,"result":{"stopReason":"max_tokens"}}' }
        'max_turn_requests' { '{"jsonrpc":"2.0","id":7,"result":{"stopReason":"max_turn_requests"}}' }
        'refusal' { '{"jsonrpc":"2.0","id":7,"result":{"stopReason":"refusal"}}' }
        default { '{"jsonrpc":"2.0","id":7,"result":{"stopReason":"end_turn"}}' }
      }
      '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"test-session","update":{"sessionUpdate":"usage_update","used":48,"size":100}}}'
      '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"test-session","update":{"sessionUpdate":"session_info_update","title":"Late superseded"}}}'
      '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"test-session","update":{"sessionUpdate":"session_info_update","title":null}}}'
      '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"test-session","update":{"sessionUpdate":"session_info_update","title":"A title"}}}'
    }
  }
}
"#).unwrap();
        for i in 0..9 {
            chat::append(&db, &id, chat::Role::User, "question", None, None).unwrap();
            let mut cmd = Command::new("powershell.exe");
            cmd.args(["-NoProfile", "-NonInteractive", "-File"])
                .arg(&script);
            cmd.env("GOOGLE_API_KEY", "fake-inherited-key");
            cmd.env(
                "MOMSO_EXPECT_DECISION",
                if i == 0 {
                    "yes"
                } else if i == 1 {
                    "no"
                } else if i == 2 || i == 6 || i == 7 {
                    "cancelled"
                } else if i == 8 {
                    "error"
                } else {
                    "yes"
                },
            );
            let stop_reason = match i {
                3 => "max_tokens",
                4 => "max_turn_requests",
                5 => "refusal",
                _ => "end_turn",
            };
            cmd.env("MOMSO_STOP_REASON", stop_reason);
            cmd.env("MOMSO_EXPECT_SKILL", if i == 1 { "yes" } else { "no" });
            let marker = d.path().join(format!("cancel-{i}"));
            cmd.env("MOMSO_FAKE_CANCEL_MARKER", &marker);
            let channel: ApprovalChannel = Box::new(move |request, cancellation| {
                Box::pin(async move {
                    assert_eq!(request.provider, "gemini");
                    assert_eq!(request.description, "Write");
                    assert_eq!(request.targets[0].path, "a.md");
                    if i == 2 || i == 7 {
                        cancellation.cancel();
                    }
                    Ok(if i == 0 || i == 7 || (3..6).contains(&i) {
                        ApprovalChoice::AllowOnce
                    } else if i == 1 {
                        ApprovalChoice::Deny
                    } else {
                        ApprovalChoice::Cancel
                    })
                })
            });
            let mut events = Vec::new();
            let mut capture = crate::chat_finish::Capture::default();
            let result = run_command(
                cmd,
                &db,
                d.path(),
                &id,
                "question",
                Some("fake"),
                if i == 0 {
                    None
                } else if i == 1 {
                    Some("  ")
                } else {
                    Some("low")
                },
                if i == 1 { "plan" } else { "default" },
                if i == 1 { Some("sample") } else { None },
                "dummy-test-only".into(),
                Some(channel),
                CancellationToken::new(),
                &mut |name, value| {
                    capture.observe(name, &value);
                    events.push((name.to_string(), value));
                },
            )
            .await;
            if i == 2 || i == 6 || i == 7 || i == 8 {
                assert!(result.is_err());
                if i != 8 {
                    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "cancelled");
                }
                capture
                    .persist_partial(&db, &id, "gemini", i != 8, 0)
                    .unwrap();
                continue;
            }
            result.unwrap();
            assert!(events.iter().any(|(name, _)| name == "chat-stream"));
            assert!(events.iter().any(|(name, _)| name == "chat-done"));
            if i >= 3 {
                assert!(
                    events
                        .iter()
                        .any(|(name, value)| name == "chat-done"
                            && value["stopReason"] == stop_reason)
                );
                assert_eq!(
                    events
                        .iter()
                        .find(|(name, _)| name == "chat-done")
                        .map(|(_, value)| crate::chat_finish::incomplete_reply(value)),
                    Some(i != 5)
                );
            }
            if i == 1 {
                assert!(events
                    .iter()
                    .any(|(name, value)| name == "chat-context-updated"
                        && value["context"]["usedTokens"] == 64));
            }
            if i == 0 {
                assert!(events
                    .iter()
                    .any(|(name, value)| name == "chat-renamed" && value["title"] == "A title"));
                assert!(events
                    .iter()
                    .any(|(name, value)| name == "chat-context-updated"
                        && value["context"]["usedTokens"] == 48));
                assert_eq!(
                    chat_context::model_limit(&db, &id, "gemini", Some("fake")).unwrap(),
                    Some((100, 48))
                );
            }
        }
        assert_eq!(previous(&db, &id).unwrap().as_deref(), Some("test-session"));
        let saved: i64 = Connection::open(&db).unwrap().query_row("SELECT count(*) FROM chat_messages WHERE chat_id=?1 AND role='assistant' AND text='hello'", [&id], |row| row.get(0)).unwrap();
        assert_eq!(saved, 5);
        let detail = serde_json::to_value(chat::read(&db, &id).unwrap()).unwrap();
        let reasons: Vec<_> = detail["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "assistant")
            .map(|message| message["finishReason"].as_str().unwrap())
            .collect();
        assert_eq!(
            reasons,
            [
                "completed",
                "completed",
                "cancelled",
                "length_limit",
                "turn_limit",
                "refusal",
                "cancelled",
                "cancelled",
                "error"
            ]
        );
        assert!(!std::fs::read(&db)
            .unwrap()
            .windows("dummy-test-only".len())
            .any(|x| x == b"dummy-test-only"));
    }
}
