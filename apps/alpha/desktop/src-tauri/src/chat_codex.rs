use crate::{
    approval::{ApprovalChoice, ApprovalClosed, ApprovalRequest},
    chat, chat_context,
    permission_modes::CodexSettings,
    process,
};
use chrono::Local;
use serde_json::{json, Value};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
};
use tokio_util::sync::CancellationToken;

pub(crate) fn supported(provider: &str) -> bool {
    provider == "codex"
}

fn declined_approval(chat_id: &str, method: &str, params: &Value) -> Option<ApprovalRequest> {
    let (kind, description) = match method {
        "item/commandExecution/requestApproval" => (
            "command",
            params.get("command").and_then(Value::as_str).unwrap_or(""),
        ),
        "item/fileChange/requestApproval" => (
            "file_change",
            params
                .get("grantRoot")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ),
        "item/permissions/requestApproval" => ("other", ""),
        _ => return None,
    };
    let mut targets: Vec<crate::approval::ApprovalTarget> = params
        .get("fileChanges")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(path, value)| {
            let change = match value.get("type").and_then(Value::as_str) {
                Some("add") => Some(crate::approval::ChangeKind::Create),
                Some("update") => Some(crate::approval::ChangeKind::Edit),
                Some("delete") => Some(crate::approval::ChangeKind::Delete),
                _ => None,
            };
            crate::approval::ApprovalTarget {
                path: path.clone(),
                change,
            }
        })
        .collect();
    targets.sort_by(|a, b| a.path.cmp(&b.path));
    let change = if targets.len() == 1 {
        targets[0].change
    } else {
        None
    };
    Some(ApprovalRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        chat_id: Some(chat_id.to_owned()),
        provider: "codex".into(),
        kind: kind.into(),
        description: description.into(),
        change,
        targets,
        reason: params
            .get("reason")
            .and_then(Value::as_str)
            .map(str::to_owned),
        options: vec![ApprovalChoice::Deny],
        setup: None,
    })
}
fn emit_declined_request<F: FnMut(&str, Value)>(
    chat_id: &str,
    method: &str,
    params: &Value,
    emit: &mut F,
) -> Option<ApprovalClosed> {
    let request = declined_approval(chat_id, method, params)?;
    let closed = request.closed();
    if let Ok(payload) = serde_json::to_value(&request) {
        emit("agent-approval-request", payload);
    }
    Some(closed)
}
fn emit_declined_closed<F: FnMut(&str, Value)>(closed: ApprovalClosed, emit: &mut F) {
    if let Ok(payload) = serde_json::to_value(closed) {
        emit("agent-approval-closed", payload);
    }
}
pub(crate) type ApprovalChannel = Box<
    dyn Fn(
            ApprovalRequest,
            CancellationToken,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<ApprovalChoice, String>> + Send>,
        > + Send
        + Sync,
>;
fn interactive_approval(chat_id: &str, method: &str, params: &Value) -> Option<ApprovalRequest> {
    if !matches!(
        method,
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval"
    ) {
        return None;
    }
    let mut request = declined_approval(chat_id, method, params)?;
    let advertised = params.get("availableDecisions").and_then(Value::as_array);
    let choices = [
        ("accept", ApprovalChoice::AllowOnce),
        ("acceptForSession", ApprovalChoice::AllowSession),
        ("decline", ApprovalChoice::Deny),
        ("cancel", ApprovalChoice::Cancel),
    ];
    request.options = choices
        .into_iter()
        .filter_map(|(wire, choice)| {
            if advertised.is_none_or(|list| list.iter().any(|item| item.as_str() == Some(wire))) {
                Some(choice)
            } else {
                None
            }
        })
        .collect();
    if request.options.is_empty() {
        None
    } else {
        Some(request)
    }
}
fn started_file_changes(params: &Value) -> Option<(String, Value)> {
    let item = params.get("item")?;
    if item.get("type")?.as_str()? != "fileChange" {
        return None;
    }
    let id = item.get("id")?.as_str()?.to_owned();
    let mut changes = serde_json::Map::new();
    for change in item.get("changes")?.as_array()? {
        let Some(path) = change.get("path").and_then(Value::as_str) else {
            continue;
        };
        let kind = change.pointer("/kind/type").and_then(Value::as_str);
        changes.insert(path.to_owned(), json!({"type":kind}));
    }
    Some((id, Value::Object(changes)))
}
fn effective_settings_match(result: &Value, settings: &CodexSettings) -> bool {
    result.get("approvalPolicy").and_then(Value::as_str) == Some(settings.approval_policy)
        && result.get("approvalsReviewer").and_then(Value::as_str) == Some(settings.reviewer)
        && result.pointer("/sandbox/type").and_then(Value::as_str) == Some(settings.sandbox_type)
}
fn is_compaction_event(method: &str, event: &Value, thread: Option<&str>) -> bool {
    matches!(method, "item/started" | "item/completed")
        && event.pointer("/params/item/type").and_then(Value::as_str) == Some("contextCompaction")
        && thread
            .is_some_and(|id| Some(id) == event.pointer("/params/threadId").and_then(Value::as_str))
}
fn is_context_limit(event: &Value) -> bool {
    event
        .pointer("/params/turn/error/codexErrorInfo")
        .and_then(Value::as_str)
        == Some("contextWindowExceeded")
}
fn is_unauthorized(event: &Value) -> bool {
    event
        .pointer("/params/turn/error/codexErrorInfo")
        .and_then(Value::as_str)
        == Some("unauthorized")
}
fn approval_wire(choice: ApprovalChoice) -> &'static str {
    match choice {
        ApprovalChoice::AllowOnce => "accept",
        ApprovalChoice::AllowSession => "acceptForSession",
        ApprovalChoice::Deny => "decline",
        ApprovalChoice::Cancel => "cancel",
    }
}
async fn send(input: &mut ChildStdin, value: Value) -> Result<(), String> {
    input
        .write_all(format!("{value}\n").as_bytes())
        .await
        .map_err(|_| "AI에 요청을 보내지 못했습니다".to_string())
}
pub(crate) async fn rename_thread(thread_id: &str, title: &str) -> Result<(), String> {
    let request = process::JsonRpcRequest {
        initialize_params: json!({"clientInfo":{"name":"momso","title":"momso","version":env!("CARGO_PKG_VERSION")}}),
        method: "thread/name/set",
        params: json!({"threadId":thread_id,"name":title}),
    };
    process::run_json_rpc_tool(
        "codex",
        &["app-server"],
        None,
        &process::empty_environment(),
        request,
        Duration::from_secs(30),
    )
    .await
    .map_err(|_| "Codex 채팅방 이름을 바꾸지 못했습니다".to_string())?;
    Ok(())
}

fn archive_request(thread_id: &str) -> process::JsonRpcRequest<'static> {
    process::JsonRpcRequest {
        initialize_params: json!({"clientInfo":{"name":"momso","title":"momso","version":env!("CARGO_PKG_VERSION")}}),
        method: "thread/archive",
        params: json!({"threadId":thread_id}),
    }
}

pub(crate) async fn archive_thread(thread_id: &str) -> Result<(), ()> {
    process::run_json_rpc_tool(
        "codex",
        &["app-server"],
        None,
        &process::empty_environment(),
        archive_request(thread_id),
        Duration::from_secs(30),
    )
    .await
    .map(|_| ())
    .map_err(|_| ())
}
fn thread_params(archive: &Path, settings: &CodexSettings, now: &str) -> Value {
    json!({"cwd":archive,"sandbox":settings.sandbox,"approvalPolicy":settings.approval_policy,
           "config":{"approvals_reviewer":settings.reviewer},
           "developerInstructions":format!("지금 시각: {now}")})
}
async fn terminate(child: &mut Child) {
    let _ = child.kill().await;
    let _ = child.wait().await;
}

// The callback is the only route to the screen, and is also usable by integration tests.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run<F>(
    db: &Path,
    archive: &Path,
    chat_id: &str,
    text: &str,
    model: Option<&str>,
    effort: Option<&str>,
    settings: &CodexSettings,
    approvals: Option<ApprovalChannel>,
    cancel: CancellationToken,
    emit: F,
) -> Result<(), String>
where
    F: FnMut(&str, Value) + Send,
{
    run_with_skill_auth(
        db, archive, chat_id, text, model, effort, None, None, None, None, settings, approvals,
        cancel, emit,
    )
    .await
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_with_skill<F>(
    db: &Path,
    archive: &Path,
    chat_id: &str,
    text: &str,
    model: Option<&str>,
    effort: Option<&str>,
    skill: Option<&str>,
    fast: Option<bool>,
    fallback_prompt: Option<&str>,
    settings: &CodexSettings,
    approvals: Option<ApprovalChannel>,
    cancel: CancellationToken,
    emit: F,
) -> Result<(), String>
where
    F: FnMut(&str, Value) + Send,
{
    run_with_skill_auth(
        db,
        archive,
        chat_id,
        text,
        model,
        effort,
        skill,
        fast,
        fallback_prompt,
        None,
        settings,
        approvals,
        cancel,
        emit,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_with_skill_auth<F>(
    db: &Path,
    archive: &Path,
    chat_id: &str,
    text: &str,
    model: Option<&str>,
    effort: Option<&str>,
    skill: Option<&str>,
    fast: Option<bool>,
    fallback_prompt: Option<&str>,
    api_key: Option<&str>,
    settings: &CodexSettings,
    approvals: Option<ApprovalChannel>,
    cancel: CancellationToken,
    mut emit: F,
) -> Result<(), String>
where
    F: FnMut(&str, Value) + Send,
{
    if let Some(name) = skill {
        if !crate::skills::callable(archive, name).map_err(str::to_owned)? {
            return Err("skill_unavailable".into());
        }
    }
    let exe = process::resolve_interactive_tool("codex")
        .ok_or_else(|| "Codex를 찾지 못했습니다".to_string())?;
    std::fs::metadata(archive.join("AGENTS.md"))
        .map_err(|_| "몸소 폴더 규칙을 읽지 못했습니다".to_string())?;
    let previous = chat::codex_thread(db, chat_id)?;
    let mut command = Command::new(exe);
    for option in crate::momso_mcp::codex_config(&crate::momso_mcp::executable()?, chat_id)? {
        command.arg("-c").arg(option);
    }
    if api_key.is_some() {
        command.arg("-c").arg("model_providers.momso_api={name='momso_api',base_url='https://api.openai.com/v1',env_key='MOMSO_CODEX_API_KEY',wire_api='responses',requires_openai_auth=false}");
    }
    apply_api_key_environment(&mut command, api_key);
    command
        .arg("app-server")
        .current_dir(archive)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY");
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    run_command(
        command,
        db,
        archive,
        chat_id,
        text,
        model,
        effort,
        skill,
        fast,
        fallback_prompt,
        settings,
        approvals,
        cancel,
        previous,
        Some(if api_key.is_some() {
            "momso_api"
        } else {
            "openai"
        }),
        &mut emit,
    )
    .await?;
    // The reply and momso title have already been saved and chat-done emitted.
    // Synchronizing the provider's thread title is best effort: a second CLI
    // failure must not turn a successfully saved reply into a chat error.
    if let (Ok(Some(thread_id)), Ok(title)) =
        (chat::codex_thread(db, chat_id), chat::title(db, chat_id))
    {
        if !title.is_empty() {
            let _ = rename_thread(&thread_id, &title).await;
        }
    }
    Ok(())
}

fn apply_api_key_environment(command: &mut Command, api_key: Option<&str>) {
    command
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .env_remove("MOMSO_CODEX_API_KEY");
    if let Some(key) = api_key {
        command.env("MOMSO_CODEX_API_KEY", key);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_command<F>(
    mut command: Command,
    db: &Path,
    archive: &Path,
    chat_id: &str,
    text: &str,
    model: Option<&str>,
    effort: Option<&str>,
    skill: Option<&str>,
    fast: Option<bool>,
    fallback_prompt: Option<&str>,
    settings: &CodexSettings,
    approvals: Option<ApprovalChannel>,
    cancel: CancellationToken,
    previous: Option<String>,
    expected_provider: Option<&str>,
    mut emit: F,
) -> Result<(), String>
where
    F: FnMut(&str, Value) + Send,
{
    let mut child = command
        .spawn()
        .map_err(|_| "Codex를 시작하지 못했습니다".to_string())?;
    #[cfg(windows)]
    crate::setup_bridge::bind(chat_id, child.id());
    let result = async {
        let mut input = child.stdin.take().ok_or("AI에 요청을 보내지 못했습니다")?;
        let output = child.stdout.take().ok_or("AI의 답을 읽지 못했습니다")?;
        let mut lines = BufReader::new(output).lines();
        send(&mut input, json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"momso","title":"momso","version":env!("CARGO_PKG_VERSION")}}})).await?;
        let mut phase = 1_u8;
        let mut answer = String::new();
        let mut actual_model: Option<String> = None;
        let mut actual_effort: Option<String> = None;
        let mut active_thread = previous.clone();
        let mut resuming = previous.is_some();
        let mut turn_text = text;
        let mut before_compaction: Option<chat_context::Context> = None;
        let mut pending_file_changes = std::collections::HashMap::<String, Value>::new();
        loop {
            let line = tokio::select! {
                _ = cancel.cancelled() => return Err("요청을 중단했습니다".to_string()),
                line = tokio::time::timeout(Duration::from_secs(180), lines.next_line()) => line.map_err(|_| "AI의 응답 시간이 초과되었습니다".to_string())?.map_err(|_| "AI의 답을 읽지 못했습니다".to_string())?.ok_or("AI 연결이 끊겼습니다")?,
            };
            if line.len() > 2 * 1024 * 1024 { return Err("AI의 답을 읽지 못했습니다".into()); }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue; };
            if let Some(method) = v.get("method").and_then(Value::as_str) {
                if let Some(id) = v.get("id") {
                    let mut approval_params = v["params"].clone();
                    if method == "item/fileChange/requestApproval" {
                        if let Some(changes) = approval_params.get("itemId").and_then(Value::as_str).and_then(|id| pending_file_changes.get(id)) {
                            approval_params["fileChanges"] = changes.clone();
                        }
                    }
                    if let (Some(channel), Some(request)) = (&approvals, interactive_approval(chat_id, method, &approval_params)) {
                        let choice = channel(request, cancel.clone()).await?;
                        send(&mut input, json!({"id":id,"result":{"decision":approval_wire(choice)}})).await?;
                    } else if let Some(closed) = emit_declined_request(chat_id, method, &approval_params, &mut emit) {
                        let result = if method == "item/permissions/requestApproval" {
                            send(&mut input, json!({"id":id,"result":{"permissions":{}}})).await
                        } else {
                            send(&mut input, json!({"id":id,"result":{"decision":"decline"}})).await
                        };
                        emit_declined_closed(closed, &mut emit);
                        result?;
                    } else {
                        send(&mut input, json!({"id":id,"error":{"code":-32601,"message":"Request unavailable"}})).await?;
                    }
                    continue;
                }
                if method == "item/started" && active_thread.is_some() && v.pointer("/params/threadId").and_then(Value::as_str) == active_thread.as_deref() {
                    if let Some((id, changes)) = started_file_changes(&v["params"]) { pending_file_changes.insert(id, changes); }
                }
                if method == "thread/started" && active_thread.is_none() {
                    active_thread = v.pointer("/params/thread/id").and_then(Value::as_str).map(str::to_owned);
                } else if method == "thread/tokenUsage/updated" {
                    let notification_thread = v.pointer("/params/threadId").and_then(Value::as_str);
                    if active_thread.as_deref().is_some_and(|id| Some(id) == notification_thread) {
                        if let Some(context) = chat_context::update(db, chat_id, actual_model.as_deref(), &v["params"])? {
                            emit("chat-context-updated", json!({"chatId":chat_id,"context":context}));
                        }
                    }
                } else if is_compaction_event(method, &v, active_thread.as_deref()) {
                    if method == "item/started" {
                        before_compaction = Some(chat_context::get(db, chat_id)?);
                    } else {
                        let now = chat_context::get(db, chat_id)?;
                        let after = before_compaction.as_ref().filter(|before| before.measurement_differs(&now)).map(|_| now);
                        emit("chat-compacted", json!({"chatId":chat_id,"before":before_compaction.take(),"after":after}));
                    }
                } else if method == "item/completed" && v.pointer("/params/item/type").and_then(Value::as_str) == Some("fileChange") && v.pointer("/params/item/status").and_then(Value::as_str) == Some("completed") {
                    let written = file_change_references(archive, &v);
                    emit("momso-internal-file-writes", json!(written));
                } else if method == "item/completed" && v.pointer("/params/item/type").and_then(Value::as_str) == Some("commandExecution") && v.pointer("/params/item/status").and_then(Value::as_str) == Some("completed") && v.pointer("/params/item/exitCode").and_then(Value::as_i64) == Some(0) {
                    let reads: Vec<String> = v.pointer("/params/item/commandActions").and_then(Value::as_array).into_iter().flatten().filter(|action| action.get("type").and_then(Value::as_str) == Some("read")).filter_map(|action| action.get("path").and_then(Value::as_str)).filter_map(|path| crate::chat_file_changes::reference(archive, path)).collect();
                    emit("momso-internal-file-reads", json!(reads));
                } else if method == "item/agentMessage/delta" {
                    if let Some(delta) = v.pointer("/params/delta").and_then(Value::as_str) {
                        answer.push_str(delta); emit("chat-stream", json!({"chatId":chat_id,"delta":delta}));
                    }
                } else if method == "turn/completed" {
                    if v.pointer("/params/turn/status").and_then(Value::as_str) != Some("completed") {
                        if let Some(info) = v.pointer("/params/turn/error/codexErrorInfo").and_then(crate::chat_finish::codex_error) { emit("momso-internal-provider-error", info); }
                        if is_unauthorized(&v) { return Err("__momso_auth_failed".into()); }
                        if is_context_limit(&v) { emit("momso-internal-finish-reason", json!("length_limit")); }
                        return Err("AI가 응답을 완료하지 못했습니다".into());
                    }
                    if answer.is_empty() { return Err("AI의 답을 읽지 못했습니다".into()); }
                    let message=chat::append_finished(db, chat_id, &answer, "codex".into(), actual_model, actual_effort, chat::FinishReason::Completed)?;
                    let done = json!({"chatId":chat_id,"messageId":message.id,"actualEffort":null,"fastState":null,"stopReason":null,"finishReason":"completed"});
                    emit("chat-done", done);
                    return Ok(());
                }
                continue;
            }
            if v.get("id").and_then(Value::as_u64) != Some(phase as u64) { continue; }
            if let Some(error) = v.get("error") {
                if let Some(fallback) = fallback_prompt.filter(|_| phase == 2 && resuming && previous.as_deref().is_some_and(|id| missing_thread_error(error, id))) {
                    if cancel.is_cancelled() { return Err("요청을 중단했습니다".into()); }
                    let mut params = thread_params(archive, settings, &Local::now().to_rfc3339());
                    if let Some(provider) = expected_provider { params["modelProvider"] = json!(provider); }
                    if let Some(m) = model.filter(|x| !x.trim().is_empty()) { params["model"] = json!(m); }
                    active_thread = None;
                    resuming = false;
                    turn_text = fallback;
                    emit("momso-internal-handoff-fallback", Value::Null);
                    send(&mut input, json!({"id":2,"method":"thread/start","params":params})).await?;
                    continue;
                }
                return Err("AI 연결을 완료하지 못했습니다".into());
            }
            let result = v.get("result").ok_or("AI의 답을 읽지 못했습니다")?;
            match phase {
                1 => {
                    send(&mut input,json!({"method":"initialized","params":{}})).await?;
                    let mut params=thread_params(archive, settings, &Local::now().to_rfc3339());
                    if let Some(provider) = expected_provider { params["modelProvider"] = json!(provider); }
                    if let Some(m)=model.filter(|x| !x.trim().is_empty()) { params["model"]=json!(m); }
                    let method=if let Some(id)=previous.as_ref() { params["threadId"]=json!(id); "thread/resume" } else { "thread/start" };
                    send(&mut input,json!({"id":2,"method":method,"params":params})).await?; phase=2;
                }
                2 => {
                    if !effective_settings_match(result, settings) { return Err("선택한 권한 설정을 적용하지 못했습니다".into()); }
                    if expected_provider.is_some_and(|provider| result.get("modelProvider").and_then(Value::as_str) != Some(provider)) { return Err("unavailable".into()); }
                    let id=result.pointer("/thread/id").and_then(Value::as_str).ok_or("채팅방을 열지 못했습니다")?;
                    actual_model=result.get("model").and_then(Value::as_str).map(str::to_owned);
                    actual_effort=effort.filter(|e| !e.trim().is_empty()).map(str::to_owned).or_else(|| result.get("reasoningEffort").and_then(Value::as_str).map(str::to_owned));
                    if let Some(model) = &actual_model { emit("momso-internal-observed-model", json!(model)); }
                    if let Some(effort) = &actual_effort { emit("momso-internal-observed-effort", json!(effort)); }
                    chat::set_codex_thread(db, chat_id, id)?;
                    active_thread=Some(id.to_owned());
                    let selected=skill.map(|name| format!("${name} ")).unwrap_or_default();
                    let prompt=format!("{selected}{turn_text}");
                    let sandbox = if settings.sandbox_type == "workspaceWrite" {json!({"type":"workspaceWrite","writableRoots":[archive]})} else {json!({"type":"dangerFullAccess"})};
                    let mut params=json!({"threadId":id,"cwd":archive,"approvalPolicy":settings.approval_policy,"sandboxPolicy":sandbox,"input":[{"type":"text","text":prompt}]});
                    if let Some(m)=model.filter(|x| !x.trim().is_empty()) { params["model"]=json!(m); }
                    if let Some(e)=effort.filter(|e| !e.trim().is_empty()) { params["effort"]=json!(e); }
                    set_turn_service_tier(&mut params, fast);
                    send(&mut input,json!({"id":3,"method":"turn/start","params":params})).await?; phase=3;
                }
                _ => { phase=4; }
            }
        }
    }.await;
    terminate(&mut child).await;
    result
}

fn service_tier_for_turn(fast: Option<bool>) -> Option<&'static str> {
    fast.map(|enabled| if enabled { "fast" } else { "default" })
}

fn missing_thread_error(error: &Value, id: &str) -> bool {
    error.get("code").and_then(Value::as_i64) == Some(-32600)
        && error.get("data").is_none_or(Value::is_null)
        && error.get("message").and_then(Value::as_str)
            == Some(format!("no rollout found for thread id {id}").as_str())
}

fn set_turn_service_tier(params: &mut Value, fast: Option<bool>) {
    if let Some(tier) = service_tier_for_turn(fast) {
        params["serviceTierForTurn"] = json!(tier);
    }
}

fn file_change_references(archive: &Path, event: &Value) -> Vec<String> {
    event
        .pointer("/params/item/changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|change| change.get("path").and_then(Value::as_str))
        .filter_map(|path| crate::chat_file_changes::reference(archive, path))
        .collect()
}

pub(crate) type Active =
    std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, CancellationToken>>>;

pub(crate) async fn finish_active<T>(
    active: &Active,
    chat_id: &str,
    persist: impl FnOnce() -> T,
) -> T {
    let result = persist();
    active.lock().await.remove(chat_id);
    result
}

pub(crate) fn error_payload(chat_id: &str, error: &str) -> Value {
    json!({"chatId":chat_id,"reason":error})
}
pub(crate) fn valid_model(model: Option<String>) -> Option<String> {
    model.filter(|s| !s.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_request_targets_the_stored_thread() {
        let request = archive_request("thread-test");
        assert_eq!(request.method, "thread/archive");
        assert_eq!(request.params, json!({"threadId":"thread-test"}));
    }
    #[test]
    fn context_limit_uses_only_the_structured_error() {
        assert!(is_context_limit(
            &json!({"params":{"turn":{"error":{"codexErrorInfo":"contextWindowExceeded"}}}})
        ));
        assert!(!is_context_limit(
            &json!({"params":{"turn":{"error":{"message":"contextWindowExceeded"}}}})
        ));
    }
    #[test]
    fn unauthorized_uses_only_the_structured_error() {
        assert!(is_unauthorized(
            &json!({"params":{"turn":{"error":{"codexErrorInfo":"unauthorized"}}}})
        ));
        assert!(!is_unauthorized(
            &json!({"params":{"turn":{"error":{"message":"unauthorized"}}}})
        ));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn fake_cli_records_stop_error_limit_and_completion() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("db.sqlite");
        crate::storage::initialize(&db, &d.path().join("backups"), "test").unwrap();
        let id = serde_json::to_value(chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let script = d.path().join("fake-codex.ps1");
        std::fs::write(&script, r#"
if (-not $env:MOMSO_CODEX_API_KEY -or $env:OPENAI_API_KEY -or $env:CODEX_API_KEY) { exit 7 }
$null = [Console]::In.ReadLine()
[Console]::WriteLine('{"id":1,"result":{}}')
$null = [Console]::In.ReadLine()
$request = [Console]::In.ReadLine() | ConvertFrom-Json
if ($request.params.modelProvider -ne 'momso_api') { exit 8 }
[Console]::WriteLine('{"id":2,"result":{"thread":{"id":"fixture-thread"},"approvalPolicy":"on-request","approvalsReviewer":"user","sandbox":{"type":"workspaceWrite"},"modelProvider":"momso_api","model":"fixture-model","reasoningEffort":"low"}}')
$null = [Console]::In.ReadLine()
[Console]::WriteLine('{"method":"item/agentMessage/delta","params":{"delta":"partial"}}')
switch ($env:MOMSO_FAKE_RESULT) {
 'stop' { [Threading.Thread]::Sleep(30000) }
 'error' { [Console]::WriteLine('{"method":"turn/completed","params":{"turn":{"status":"failed","error":{"codexErrorInfo":"other"}}}}') }
 'limit' { [Console]::WriteLine('{"method":"turn/completed","params":{"turn":{"status":"failed","error":{"codexErrorInfo":"contextWindowExceeded"}}}}') }
 'completed' { [Console]::WriteLine('{"method":"turn/completed","params":{"turn":{"status":"completed"}}}') }
}
"#).unwrap();
        for mode in ["stop", "error", "limit", "completed"] {
            chat::append(&db, &id, chat::Role::User, "question", None, None).unwrap();
            let mut cmd = Command::new("powershell.exe");
            cmd.args(["-NoProfile", "-File"])
                .arg(&script)
                .env("MOMSO_FAKE_RESULT", mode)
                .current_dir(d.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            apply_api_key_environment(&mut cmd, Some("momso-fake-codex-key-b84f39c7"));
            let cancel = CancellationToken::new();
            let trigger = cancel.clone();
            let mut capture = crate::chat_finish::Capture::default();
            let previous = chat::codex_thread(&db, &id).unwrap();
            let result = run_command(
                cmd,
                &db,
                d.path(),
                &id,
                "question",
                None,
                None,
                None,
                None,
                None,
                &crate::permission_modes::codex_settings("Ask for approval").unwrap(),
                None,
                cancel,
                previous,
                Some("momso_api"),
                |name, value| {
                    capture.observe(name, &value);
                    if mode == "stop" && name == "chat-stream" {
                        trigger.cancel();
                    }
                },
            )
            .await;
            if mode == "completed" {
                assert!(result.is_ok());
            } else {
                assert!(result.is_err());
                capture
                    .persist_partial(&db, &id, "codex", mode == "stop", 0)
                    .unwrap();
            }
        }
        let detail = serde_json::to_value(chat::read(&db, &id).unwrap()).unwrap();
        let replies: Vec<_> = detail["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|value| value["role"] == "assistant")
            .collect();
        assert_eq!(replies.len(), 4);
        for (reply, expected) in
            replies
                .iter()
                .zip(["cancelled", "error", "length_limit", "completed"])
        {
            assert_eq!(reply["finishReason"], expected);
            assert_eq!(reply["text"], "partial");
            assert_eq!(reply["provider"], "codex");
            assert_eq!(reply["model"], "fixture-model");
        }
        let fake_key = b"momso-fake-codex-key-b84f39c7";
        for entry in walkdir::WalkDir::new(d.path()) {
            let entry = entry.unwrap();
            if entry.file_type().is_file() {
                let bytes = std::fs::read(entry.path()).unwrap();
                assert!(!bytes.windows(fake_key.len()).any(|part| part == fake_key));
            }
        }
    }

    #[tokio::test]
    async fn active_chat_is_released_only_after_delivery_persistence() {
        let active = Active::default();
        for expected in [Ok(()), Err("save failed")] {
            active
                .lock()
                .await
                .insert("room".into(), CancellationToken::new());
            let result = finish_active(&active, "room", || {
                std::thread::scope(|scope| {
                    assert!(scope
                        .spawn(|| active.try_lock().unwrap().contains_key("room"))
                        .join()
                        .unwrap());
                });
                expected
            })
            .await;
            assert_eq!(result, expected);
            assert!(!active.lock().await.contains_key("room"));
        }
    }
    #[test]
    fn fallback_only_for_the_exact_missing_thread_error() {
        let id = "00000000-0000-0000-0000-000000000000";
        assert!(missing_thread_error(
            &json!({"code":-32600,"message":format!("no rollout found for thread id {id}")}),
            id
        ));
        assert!(!missing_thread_error(
            &json!({"code":-32600,"message":"rejected: blocked by policy"}),
            id
        ));
        assert!(!missing_thread_error(
            &json!({"code":-32600,"message":format!("no rollout found for thread id {id}"),"data":{"reason":"blocked by policy"}}),
            id
        ));
        assert!(!missing_thread_error(
            &json!({"code":-32600,"message":"no rollout found for thread id another"}),
            id
        ));
        assert!(!missing_thread_error(
            &json!({"code":-32603,"message":format!("no rollout found for thread id {id}")}),
            id
        ));
    }
    #[test]
    fn fast_selection_applies_only_to_the_current_turn() {
        for (fast, expected) in [
            (Some(true), Some("fast")),
            (Some(false), Some("default")),
            (None, None),
        ] {
            let mut params = json!({"threadId":"thread", "input":[]});
            set_turn_service_tier(&mut params, fast);
            assert_eq!(
                params.get("serviceTierForTurn").and_then(Value::as_str),
                expected
            );
            assert!(params.get("serviceTier").is_none());
        }
    }
    #[test]
    fn file_approval_keeps_unknown_null_and_preserves_known_targets() {
        let unknown = declined_approval(
            "room",
            "item/fileChange/requestApproval",
            &json!({"itemId":"id","grantRoot":"C:/work"}),
        )
        .unwrap();
        let payload = serde_json::to_value(unknown).unwrap();
        assert!(payload["change"].is_null());
        assert_eq!(payload["targets"], json!([]));
        let request = declined_approval("room", "item/fileChange/requestApproval", &json!({"fileChanges":{"a.md":{"type":"add"},"b.md":{"type":"update"},"c.md":{"type":"delete"}}})).unwrap();
        let payload = serde_json::to_value(request).unwrap();
        assert!(payload["change"].is_null());
        assert_eq!(
            payload["targets"],
            json!([{"path":"a.md","change":"create"},{"path":"b.md","change":"edit"},{"path":"c.md","change":"delete"}])
        );
        let started = json!({"item":{"type":"fileChange","id":"item1","changes":[{"path":"a.md","kind":{"type":"add"}},{"path":"b.md","kind":{"type":"update"}},{"path":"c.md","kind":{"type":"delete"}}]}});
        let (id, changes) = started_file_changes(&started).unwrap();
        assert_eq!(id, "item1");
        let request = declined_approval(
            "room",
            "item/fileChange/requestApproval",
            &json!({"itemId":id,"fileChanges":changes}),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(request).unwrap()["targets"],
            payload["targets"]
        );
    }
    #[test]
    fn file_change_event_only_identifies_archive_files() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("zettels")).unwrap();
        let path = temp.path().join("zettels/zet_20261115_120000.md");
        std::fs::write(&path, "문서").unwrap();
        let event = serde_json::json!({"params":{"item":{"changes":[{"path":path},{"path":"../outside.md"}]}}});
        assert_eq!(
            file_change_references(temp.path(), &event),
            vec!["zet_20261115_120000"]
        );
    }
    #[test]
    fn developer_instructions_are_separate_and_refreshed_per_request() {
        let settings = crate::permission_modes::codex_settings("Ask for approval").unwrap();
        let first = thread_params(Path::new("archive"), &settings, "시각1");
        let second = thread_params(Path::new("archive"), &settings, "시각2");
        assert_eq!(first["developerInstructions"], "지금 시각: 시각1");
        assert_eq!(second["developerInstructions"], "지금 시각: 시각2");
        assert!(first.get("input").is_none());
    }

    #[test]
    fn compaction_events_only_match_the_active_thread() {
        let event = json!({"params":{"threadId":"wanted","item":{"type":"contextCompaction"}}});
        assert!(is_compaction_event("item/started", &event, Some("wanted")));
        assert!(is_compaction_event(
            "item/completed",
            &event,
            Some("wanted")
        ));
        assert!(!is_compaction_event(
            "item/completed",
            &event,
            Some("other")
        ));
        assert!(!is_compaction_event("item/completed", &event, None));
        assert!(!is_compaction_event(
            "item/started",
            &json!({"params":{}}),
            None
        ));
    }

    #[test]
    fn declined_codex_approval_events_include_the_chat_id() {
        for method in [
            "item/commandExecution/requestApproval",
            "item/fileChange/requestApproval",
            "item/permissions/requestApproval",
        ] {
            let mut events = Vec::new();
            let mut emit = |name: &str, payload: Value| events.push((name.to_owned(), payload));
            let closed =
                emit_declined_request("chat_123", method, &json!({"command":"ls"}), &mut emit)
                    .unwrap();
            emit_declined_closed(closed, &mut emit);
            assert_eq!(events[0].0, "agent-approval-request");
            assert_eq!(events[1].0, "agent-approval-closed");
            assert_eq!(events[0].1["chatId"], "chat_123");
            assert_eq!(events[1].1["chatId"], "chat_123");
            assert_eq!(events[0].1["requestId"], events[1].1["requestId"]);
            assert_eq!(events[0].1["options"], json!(["deny"]));
        }
        assert!(declined_approval("chat_123", "unknown", &json!({})).is_none());
    }
    #[test]
    fn mismatched_or_missing_server_permissions_are_rejected() {
        let settings = crate::permission_modes::codex_settings("Approve for me").unwrap();
        let valid = json!({"approvalPolicy":"on-request","approvalsReviewer":"auto_review","sandbox":{"type":"workspaceWrite"}});
        assert!(effective_settings_match(&valid, &settings));
        let wrong = json!({"approvalPolicy":"on-request","approvalsReviewer":"user","sandbox":{"type":"workspaceWrite"}});
        assert!(!effective_settings_match(&wrong, &settings));
        assert!(!effective_settings_match(&json!({}), &settings));
    }
    #[test]
    fn interactive_approval_preserves_chat_and_supported_decisions() {
        let request = interactive_approval(
            "chat_123",
            "item/commandExecution/requestApproval",
            &json!({"availableDecisions":["accept","decline"],"command":"pwd"}),
        )
        .unwrap();
        assert_eq!(request.chat_id.as_deref(), Some("chat_123"));
        assert_eq!(
            request.options,
            vec![ApprovalChoice::AllowOnce, ApprovalChoice::Deny]
        );
        assert_eq!(approval_wire(ApprovalChoice::AllowOnce), "accept");
        assert_eq!(approval_wire(ApprovalChoice::Deny), "decline");
        assert!(
            interactive_approval("chat_123", "item/permissions/requestApproval", &json!({}))
                .is_none()
        );
    }
    #[test]
    fn only_codex_is_supported() {
        assert!(supported("codex"));
        assert!(!supported("claude"));
        assert!(!supported("gemini"));
    }
    #[tokio::test]
    #[ignore = "실제 Codex 로그인 상태에서 실행"]
    async fn live_two_turns() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("archive");
        std::fs::create_dir(&archive).unwrap();
        std::fs::write(archive.join("AGENTS.md"), "이 보관함에서 일합니다.").unwrap();
        let db = dir.path().join("test.db");
        let c = rusqlite::Connection::open(&db).unwrap();
        c.execute_batch(include_str!("../migrations/0009_chats.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0010_chat_codex.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0011_chat_effort.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0012_chat_groups.sql"))
            .unwrap();
        c.execute_batch(include_str!("../migrations/0013_chat_context.sql"))
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
        c.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES ('test','','a','a')",
            [],
        )
        .unwrap();
        drop(c);
        chat::append(
            &db,
            "test",
            chat::Role::User,
            "한 줄로 인사해 주세요",
            None,
            None,
        )
        .unwrap();
        let mut events = Vec::new();
        run(
            &db,
            &archive,
            "test",
            "한 줄로 인사해 주세요",
            None,
            Some("low"),
            &crate::permission_modes::codex_settings("Ask for approval").unwrap(),
            None,
            CancellationToken::new(),
            |name, v| events.push((name.to_owned(), v)),
        )
        .await
        .unwrap();
        assert!(events.iter().any(|(n, _)| n == "chat-stream"));
        assert!(events.iter().any(|(n, _)| n == "chat-done"));
        let first = chat::codex_thread(&db, "test").unwrap().unwrap();
        let first_usage = serde_json::to_value(chat_context::get(&db, "test").unwrap()).unwrap();
        println!("FIRST_CONTEXT_RECEIVED={}", first_usage["state"] == "ok");
        chat::append(
            &db,
            "test",
            chat::Role::User,
            "방금 내가 뭐라고 했지?",
            None,
            None,
        )
        .unwrap();
        let mut second = String::new();
        run(
            &db,
            &archive,
            "test",
            "방금 내가 뭐라고 했지?",
            None,
            None,
            &crate::permission_modes::codex_settings("Ask for approval").unwrap(),
            None,
            CancellationToken::new(),
            |n, v| {
                if n == "chat-stream" {
                    second.push_str(v["delta"].as_str().unwrap_or(""));
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(
            chat::codex_thread(&db, "test").unwrap().as_deref(),
            Some(first.as_str())
        );
        assert!(!second.is_empty());
        let second_usage = serde_json::to_value(chat_context::get(&db, "test").unwrap()).unwrap();
        println!("SECOND_CONTEXT_RECEIVED={}", second_usage["state"] == "ok");
        println!(
            "CONTEXT_INCREASED={}",
            second_usage["usedTokens"]
                .as_i64()
                .zip(first_usage["usedTokens"].as_i64())
                .is_some_and(|(after, before)| after > before)
        );
        assert_eq!(first_usage["state"], "ok");
        assert_eq!(second_usage["state"], "ok");
        let detail = serde_json::to_value(chat::read(&db, "test").unwrap()).unwrap();
        assert_eq!(detail["messages"].as_array().unwrap().len(), 4);
        assert_eq!(detail["messages"][1]["provider"], "codex");
        assert!(detail["messages"][1]["model"].as_str().is_some());
        assert_eq!(detail["messages"][1]["effort"], "low");
        println!("SECOND_ANSWER={second}");
    }
}
