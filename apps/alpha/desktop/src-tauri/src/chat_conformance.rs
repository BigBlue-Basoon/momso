use crate::{
    approval::ApprovalChoice, chat, chat_claude, chat_codex, chat_codex::ApprovalChannel,
    chat_gemini, permission_modes, storage,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex},
};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

const CODEX: &str = r#"
$null = [Console]::ReadLine()
'{"id":1,"result":{}}'
$null = [Console]::ReadLine()
$null = [Console]::ReadLine()
'{"id":2,"result":{"thread":{"id":"fixture-thread"},"approvalPolicy":"on-request","approvalsReviewer":"user","sandbox":{"type":"workspaceWrite"},"model":"fixture-model","reasoningEffort":"low"}}'
$turn = [Console]::ReadLine() | ConvertFrom-Json
if ($turn.params.PSObject.Properties.Name -contains 'effort') { exit 25 }
if ($turn.params.input[0].text.Contains('C15_PRIVATE_DICTIONARY_MARKER')) { exit 25 }
if (-not $turn.params.input[0].text.Contains('history-marker') -or -not $turn.params.input[0].text.Contains('attachment-marker.txt')) { exit 21 }
if (-not $turn.params.input[0].text.StartsWith('$sample ')) { exit 22 }
'{"method":"thread/tokenUsage/updated","params":{"threadId":"fixture-thread","tokenUsage":{"last":{"totalTokens":12},"modelContextWindow":1000}}}'
'{"method":"item/agentMessage/delta","params":{"delta":"hello"}}'
if ($env:SCENE -eq 'stop') { [Threading.Thread]::Sleep(30000); exit 0 }
if ($env:SCENE -eq 'disconnect') { exit 0 }
if ($env:SCENE -eq 'error') { '{"method":"turn/completed","params":{"turn":{"status":"failed","error":{"codexErrorInfo":"other"}}}}'; exit 0 }
if ($env:SCENE -eq 'auth') { '{"method":"turn/completed","params":{"turn":{"status":"failed","error":{"codexErrorInfo":"unauthorized","message":"fake-secret-must-not-appear"}}}}'; exit 0 }
if ($env:SCENE -eq 'length') { '{"method":"turn/completed","params":{"turn":{"status":"failed","error":{"codexErrorInfo":"contextWindowExceeded"}}}}'; exit 0 }
if ($env:SCENE -in @('quota','rate','model','server')) {
  $code = switch ($env:SCENE) { 'quota' { 'usageLimitExceeded' } 'rate' { 'rateLimitExceeded' } 'model' { 'modelNotFound' } 'server' { 'serverOverloaded' } }
  @{method='turn/completed';params=@{turn=@{status='failed';error=@{codexErrorInfo=$code;message='fake-secret-must-not-appear'}}}} | ConvertTo-Json -Depth 10 -Compress
  exit 0
}
foreach ($kind in @('add','update','delete')) {
  $started = @{method='item/started';params=@{threadId='fixture-thread';item=@{id='change';type='fileChange';changes=@(@{path='a.md';kind=@{type=$kind}})}}} | ConvertTo-Json -Depth 12 -Compress
  [Console]::WriteLine($started)
  '{"id":91,"method":"item/fileChange/requestApproval","params":{"itemId":"change"}}'
  $reply = [Console]::ReadLine() | ConvertFrom-Json
  $expected = if ($env:SCENE -eq 'deny') { 'decline' } else { 'accept' }
  if ($reply.result.decision -ne $expected) { exit 23 }
}
'{"id":92,"method":"item/commandExecution/requestApproval","params":{"command":"echo test"}}'
$reply = [Console]::ReadLine() | ConvertFrom-Json
$expected = if ($env:SCENE -eq 'deny') { 'decline' } else { 'accept' }
if ($reply.result.decision -ne $expected) { exit 24 }
if ($env:SCENE -eq 'complete') {
  [IO.File]::WriteAllText((Join-Path 'zettels' ($env:ZET_EDIT+'.md')), 'edited')
  [IO.File]::WriteAllText((Join-Path 'zettels' ($env:ZET_CREATE+'.md')), 'created')
}
'{"method":"turn/completed","params":{"turn":{"status":"completed"}}}'
"#;
const CLAUDE: &str = r#"
$null = [Console]::ReadLine()
'{"type":"control_response","response":{"request_id":"momso-init","subtype":"success"}}'
$turn = [Console]::ReadLine() | ConvertFrom-Json
if ($turn.message.content.Contains('C15_PRIVATE_DICTIONARY_MARKER')) { exit 25 }
if (-not $turn.message.content.Contains('history-marker') -or -not $turn.message.content.Contains('attachment-marker.txt')) { exit 21 }
if (-not $turn.message.content.StartsWith('/sample ')) { exit 22 }
'{"type":"system","subtype":"init","session_id":"fixture-session"}'
'{"type":"assistant","message":{"model":"fixture-model","content":[]}}'
'{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hello"}}}'
if ($env:SCENE -eq 'stop') { [Threading.Thread]::Sleep(30000); exit 0 }
if ($env:SCENE -eq 'disconnect') { exit 0 }
if ($env:SCENE -eq 'error') { '{"type":"result","subtype":"error_during_execution","is_error":true}'; exit 0 }
if ($env:SCENE -eq 'auth') { '{"type":"assistant","error":"authentication_failed","message":{"content":"fake-secret-must-not-appear"}}'; exit 0 }
if ($env:SCENE -in @('quota','rate','billing','model','server')) {
  if ($env:SCENE -eq 'quota') { '{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","errorCode":"credits_required","resetsAt":1800000000}}' }
  if ($env:SCENE -eq 'rate') { '{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1800000000}}' }
  $code = switch ($env:SCENE) { 'quota' { 'rate_limit' } 'rate' { 'rate_limit' } 'billing' { 'billing_error' } 'model' { 'model_not_found' } 'server' { 'overloaded' } }
  @{type='assistant';error=$code;message=@{content='fake-secret-must-not-appear'}} | ConvertTo-Json -Depth 10 -Compress
  exit 0
}
if ($env:SCENE -in @('length','turn_limit','refusal')) {
  $reason = switch ($env:SCENE) { 'length' { 'max_tokens' } 'turn_limit' { 'max_turn_requests' } 'refusal' { 'refusal' } }
  if ($env:SCENE -eq 'turn_limit') { '{"type":"result","subtype":"error_max_turns","is_error":true}'; exit 0 }
  @{type='result';is_error=$false;stop_reason=$reason} | ConvertTo-Json -Compress
  $null = [Console]::ReadLine()
  '{"type":"control_response","response":{"request_id":"momso-context","subtype":"success","response":{"model":"fixture-model","totalTokens":12,"maxTokens":1000}}}'
  exit 0
}
foreach ($sceneTool in @('Write','Edit','Delete','Bash')) {
  $tool = if ($sceneTool -eq 'Delete') { 'Bash' } else { $sceneTool }
  $toolInput = if ($sceneTool -eq 'Delete') { @{command='rm a.md'} } elseif ($tool -eq 'Bash') { @{command='echo test'} } else { @{file_path='a.md'} }
  $req = @{type='control_request';request_id='r1';request=@{subtype='can_use_tool';tool_name=$tool;input=$toolInput}} | ConvertTo-Json -Depth 12 -Compress
  [Console]::WriteLine($req)
  $reply = [Console]::ReadLine() | ConvertFrom-Json
  $expected = if ($env:SCENE -eq 'deny') { 'deny' } else { 'allow' }
  if ($reply.response.response.behavior -ne $expected) { exit 23 }
}
if ($env:SCENE -eq 'complete') {
  [IO.File]::WriteAllText((Join-Path 'zettels' ($env:ZET_EDIT+'.md')), 'edited')
  [IO.File]::WriteAllText((Join-Path 'zettels' ($env:ZET_CREATE+'.md')), 'created')
}
if ($env:SCENE -eq 'reported') { '{"type":"result","is_error":false,"effort":"high","fast_mode_state":"on"}' }
else { '{"type":"result","is_error":false}' }
$null = [Console]::ReadLine()
'{"type":"control_response","response":{"request_id":"momso-context","subtype":"success","response":{"model":"fixture-model","totalTokens":12,"maxTokens":1000}}}'
"#;
const GEMINI: &str = r#"
while ($line = [Console]::ReadLine()) {
  $v = $line | ConvertFrom-Json
  switch ($v.method) {
    'initialize' { '{"jsonrpc":"2.0","id":1,"result":{"agentCapabilities":{"sessionCapabilities":{"resume":{}}}}}' }
    'authenticate' { '{"jsonrpc":"2.0","id":2,"result":{}}' }
    'session/new' { '{"jsonrpc":"2.0","id":3,"result":{"sessionId":"fixture-session","configOptions":[{"id":"mode","currentValue":"default"},{"id":"model","currentValue":"fixture-model"}]}}' }
    'session/set_config_option' { if ($v.params.configId -eq 'thought_level') { exit 25 }; exit 26 }
    'session/prompt' {
      if ($v.params.prompt[0].text.Contains('C15_PRIVATE_DICTIONARY_MARKER')) { exit 25 }
      if (-not $v.params.prompt[0].text.Contains('history-marker') -or -not $v.params.prompt[0].text.Contains('attachment-marker.txt')) { exit 21 }
      if (-not $v.params.prompt[0].text.StartsWith('/sample ')) { exit 22 }
      '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}}}'
      if ($env:SCENE -eq 'stop') {
        $null = [Console]::ReadLine()
        '{"jsonrpc":"2.0","id":7,"result":{"stopReason":"cancelled"}}'
        exit 0
      }
      if ($env:SCENE -eq 'disconnect') { exit 0 }
      if ($env:SCENE -eq 'error') { '{"jsonrpc":"2.0","id":7,"error":{"code":401,"message":"authentication failed"}}'; exit 0 }
      if ($env:SCENE -eq 'auth') { '{"jsonrpc":"2.0","id":7,"error":{"code":-32000,"message":"fake-secret-must-not-appear"}}'; exit 0 }
      if ($env:SCENE -in @('length','turn_limit','refusal')) {
        $reason = switch ($env:SCENE) { 'length' { 'max_tokens' } 'turn_limit' { 'max_turn_requests' } 'refusal' { 'refusal' } }
        @{jsonrpc='2.0';id=7;result=@{stopReason=$reason}} | ConvertTo-Json -Compress
        exit 0
      }
      foreach ($kind in @('create','edit','delete','execute')) {
        $actual = if ($kind -eq 'create') { 'edit' } else { $kind }
        $content = if ($kind -eq 'create') { @(@{type='diff';path='a.md';oldText=$null;newText='new'}) } elseif ($kind -eq 'edit') { @(@{type='diff';path='a.md';oldText='old';newText='new'}) } else { @() }
        $call = @{kind=$actual;title='test';locations=@(@{path='a.md'});content=$content}
        $req = @{jsonrpc='2.0';id=99;method='session/request_permission';params=@{toolCall=$call;options=@(@{kind='allow_once';optionId='yes'},@{kind='reject_once';optionId='no'})}} | ConvertTo-Json -Depth 15 -Compress
        [Console]::WriteLine($req)
        $reply = [Console]::ReadLine() | ConvertFrom-Json
        $expected = if ($env:SCENE -eq 'deny') { 'no' } else { 'yes' }
        if ($reply.result.outcome.optionId -ne $expected) { exit 23 }
      }
      if ($env:SCENE -eq 'complete') {
        [IO.File]::WriteAllText((Join-Path 'zettels' ($env:ZET_EDIT+'.md')), 'edited')
        [IO.File]::WriteAllText((Join-Path 'zettels' ($env:ZET_CREATE+'.md')), 'created')
      }
      '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"usage_update","used":12,"size":1000}}}'
      '{"jsonrpc":"2.0","id":7,"result":{"stopReason":"end_turn"}}'
      exit 0
    }
  }
}
"#;

fn command(path: &Path, scene: &str) -> Command {
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(path)
        .env("SCENE", scene)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

#[tokio::test]
async fn same_fake_scene_compares_stream_completion_stop_and_approvals() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    storage::initialize(&db, &dir.path().join("backups"), "test").unwrap();
    let hash = format!("{:x}", Sha256::digest(b"fake file"));
    let object_dir = dir.path().join("objects").join("sha256").join(&hash[..2]);
    std::fs::create_dir_all(&object_dir).unwrap();
    std::fs::create_dir_all(dir.path().join("zettels")).unwrap();
    std::fs::create_dir_all(dir.path().join("receipts")).unwrap();
    std::fs::create_dir_all(dir.path().join("dictionary")).unwrap();
    std::fs::write(dir.path().join("dictionary/yoga.json"), "C15_PRIVATE_DICTIONARY_MARKER").unwrap();
    std::fs::write(object_dir.join(&hash), "fake file").unwrap();
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,media_type,captured_by,created_at) VALUES('fixture','other',?1,9,?2,'attachment-marker.txt','text/plain','test','test')", rusqlite::params![hash,format!("sha256/{}/{hash}", &hash[..2])]).unwrap();
    drop(connection);
    let mut outcomes = Vec::new();
    for (index, (provider, script)) in [("codex", CODEX), ("claude", CLAUDE), ("gemini", GEMINI)]
        .into_iter()
        .enumerate()
    {
        let script_path = dir.path().join(format!("{provider}.ps1"));
        std::fs::write(&script_path, script).unwrap();
        for scene in [
            "complete",
            "deny",
            "reported",
            "stop",
            "error",
            "auth",
            "length",
            "turn_limit",
            "refusal",
            "quota",
            "rate",
            "billing",
            "model",
            "server",
            "disconnect",
        ] {
            // Codex exposes context overflow; the other two situations have no
            // equivalent structured turn result in the protocol verified here.
            if provider == "codex" && matches!(scene, "turn_limit" | "refusal") {
                continue;
            }
            if matches!(scene, "quota" | "rate" | "billing" | "model" | "server")
                && (provider == "gemini" || (provider == "codex" && scene == "billing"))
            {
                continue;
            }
            let id = serde_json::to_value(chat::create(&db).unwrap()).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            chat::append(&db, &id, chat::Role::User, "history-marker", None, None).unwrap();
            let previous_provider = if provider == "claude" {
                "codex"
            } else {
                "claude"
            };
            chat::append(
                &db,
                &id,
                chat::Role::Assistant,
                "previous-answer",
                Some(previous_provider.into()),
                None,
            )
            .unwrap();
            chat::rename(&db, &id, "manual-title").unwrap();
            let handoff =
                crate::chat_handoff::prepare(&db, &id, provider, false, "question").unwrap();
            assert_eq!(handoff.not_handed_over, 0);
            rusqlite::Connection::open(&db).unwrap().execute("INSERT INTO chat_attachments(receipt_id,chat_id,asset_id) VALUES (?1,?2,'fixture')", rusqlite::params![format!("receipt-{id}"),id]).unwrap();
            let user = chat::append_for_send(&db, &id, "question", Some("sample".into())).unwrap();
            let attachments =
                chat::message_attachment_paths(&db, dir.path(), &id, &user.id).unwrap();
            let prompt = format!(
                "{}\n{}",
                handoff.prompt,
                serde_json::to_string(&attachments).unwrap()
            );
            let edit_id = format!("zet_20261115_1200{:02}", index * 2);
            let create_id = format!("zet_20261115_1200{:02}", index * 2 + 1);
            if scene == "complete" {
                std::fs::write(
                    dir.path().join("zettels").join(format!("{edit_id}.md")),
                    "old",
                )
                .unwrap();
            }
            let tracking = crate::chat_file_changes::Tracking::start(dir.path(), &id);
            let before = crate::chat_file_changes::capture(dir.path()).unwrap();
            let mut fake = command(&script_path, scene);
            fake.current_dir(dir.path())
                .env("ZET_EDIT", &edit_id)
                .env("ZET_CREATE", &create_id);
            let requests = Arc::new(Mutex::new(Vec::new()));
            let observed = requests.clone();
            let channel: ApprovalChannel = Box::new(move |request, _| {
                let observed = observed.clone();
                Box::pin(async move {
                    observed
                        .lock()
                        .unwrap()
                        .push(serde_json::to_value(request).unwrap());
                    Ok(if scene == "deny" {
                        ApprovalChoice::Deny
                    } else {
                        ApprovalChoice::AllowOnce
                    })
                })
            });
            let token = CancellationToken::new();
            let cancel = token.clone();
            let mut events = Vec::<(String, Value)>::new();
            let mut capture = crate::chat_finish::Capture::default();
            let result = {
                let mut emit = |name: &str, value: Value| {
                    if scene == "stop" && name == "chat-stream" {
                        cancel.cancel();
                    }
                    capture.observe(name, &value);
                    events.push((name.into(), value));
                };
                match provider {
                    "codex" => {
                        chat_codex::run_command(
                            fake,
                            &db,
                            dir.path(),
                            &id,
                            &prompt,
                            None,
                            None,
                            Some("sample"),
                            if scene == "reported" {
                                Some(true)
                            } else {
                                None
                            },
                            None,
                            &permission_modes::codex_settings("Ask for approval").unwrap(),
                            Some(channel),
                            token,
                            None,
                            None,
                            &mut emit,
                        )
                        .await
                    }
                    "claude" => {
                        chat_claude::run_command_ref(
                            fake,
                            &db,
                            dir.path(),
                            &id,
                            &prompt,
                            None,
                            None,
                            false,
                            Some("sample"),
                            None,
                            &Some(channel),
                            None,
                            token,
                            &mut emit,
                        )
                        .await
                    }
                    _ => {
                        chat_gemini::run_command(
                            fake,
                            &db,
                            dir.path(),
                            &id,
                            &prompt,
                            None,
                            None,
                            "default",
                            Some("sample"),
                            "fake-test-only".into(),
                            Some(channel),
                            token,
                            &mut emit,
                        )
                        .await
                    }
                }
            };
            let stream = events
                .iter()
                .find(|(name, _)| name == "chat-stream")
                .unwrap();
            assert_eq!(stream.1["chatId"], id);
            assert_eq!(stream.1["delta"], "hello");
            assert!(!events.iter().any(|(name, _)| name == "chat-delta"));
            if matches!(scene, "quota" | "rate" | "billing" | "model" | "server") {
                assert!(result.is_err());
                let info = capture.provider_error.as_ref().unwrap();
                assert_eq!(
                    info["kind"],
                    match scene {
                        "quota" => "usageLimit",
                        "rate" => "rateLimited",
                        "billing" => "billing",
                        "model" => "modelUnavailable",
                        _ => "serviceUnavailable",
                    }
                );
                if provider == "claude" && matches!(scene, "quota" | "rate") {
                    assert_eq!(info["retryAt"], 1800000000_i64);
                } else {
                    assert!(info["retryAt"].is_null());
                }
                assert!(!info.to_string().contains("fake-secret-must-not-appear"));
                assert!(capture
                    .persist_partial(&db, &id, provider, false, 0)
                    .unwrap()
                    .is_some());
                let detail = serde_json::to_value(chat::read(&db, &id).unwrap()).unwrap();
                assert_eq!(
                    detail["messages"].as_array().unwrap().last().unwrap()["text"],
                    "hello"
                );
                continue;
            }
            if matches!(scene, "length" | "turn_limit" | "refusal") {
                let expected = match scene {
                    "length" => "length_limit",
                    "turn_limit" => "turn_limit",
                    _ => "refusal",
                };
                if result.is_err() {
                    assert!(capture
                        .persist_partial(&db, &id, provider, false, 0)
                        .unwrap()
                        .is_some());
                } else {
                    let done = &events
                        .iter()
                        .find(|(name, _)| name == "chat-done")
                        .unwrap()
                        .1;
                    assert_eq!(done["finishReason"], expected);
                }
                let detail = serde_json::to_value(chat::read(&db, &id).unwrap()).unwrap();
                let reply = detail["messages"].as_array().unwrap().last().unwrap();
                assert_eq!(reply["text"], "hello");
                assert_eq!(reply["finishReason"], expected);
                continue;
            }
            if scene == "stop" || scene == "error" || scene == "auth" || scene == "disconnect" {
                assert!(result.is_err());
                if scene == "disconnect" {
                    let payload = crate::chat_finish::provider_error_payload(
                        &id,
                        result.as_ref().unwrap_err(),
                    );
                    assert_eq!(payload["error"]["kind"], "connectionClosed");
                }
                if scene == "auth" {
                    assert_eq!(result.as_ref().unwrap_err(), "__momso_auth_failed");
                    assert!(!format!("{events:?}").contains("fake-secret-must-not-appear"));
                    let payload = crate::chat_finish::provider_error_payload(
                        &id,
                        result.as_ref().unwrap_err(),
                    );
                    assert_eq!(payload["status"], "auth_failed");
                    assert_eq!(payload["reason"], "");
                    assert!(!payload.to_string().contains("fake-secret-must-not-appear"));
                }
                assert!(!events.iter().any(|(name, _)| name == "chat-done"));
                assert!(capture
                    .persist_partial(&db, &id, provider, scene == "stop", 0)
                    .unwrap()
                    .is_some());
                let detail = serde_json::to_value(chat::read(&db, &id).unwrap()).unwrap();
                let reply = detail["messages"].as_array().unwrap().last().unwrap();
                assert_eq!(reply["text"], "hello");
                assert_eq!(
                    reply["finishReason"],
                    if scene == "stop" {
                        "cancelled"
                    } else {
                        "error"
                    }
                );
                continue;
            }
            result.unwrap();
            let after = crate::chat_file_changes::capture(dir.path()).unwrap();
            let receipt = crate::archive_receipt::ReceiptCreate {
                tool: provider.into(),
                model: "fixture-model".into(),
                chat: Some(id.clone()),
                read: vec![],
                wrote: vec![],
                prompt: "question".into(),
                changes: String::new(),
            };
            let recorded = crate::chat_file_changes::record_tracked(
                dir.path(),
                &db,
                receipt,
                &before,
                &after,
                &Default::default(),
                tracking,
            )
            .unwrap();
            assert_eq!(recorded, scene == "complete");
            if recorded {
                let receipt_text = std::fs::read_dir(dir.path().join("receipts"))
                    .unwrap()
                    .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
                    .find(|text| text.contains(&id))
                    .unwrap();
                let (receipt, _) = crate::archive_receipt::parse(&receipt_text).unwrap();
                assert_eq!(receipt.tool, provider);
                assert_eq!(
                    receipt.write_actions.get(&edit_id).map(String::as_str),
                    Some("edited")
                );
                assert_eq!(
                    receipt.write_actions.get(&create_id).map(String::as_str),
                    Some("created")
                );
            }
            let done = &events
                .iter()
                .find(|(name, _)| name == "chat-done")
                .unwrap()
                .1;
            assert_eq!(done["chatId"], id);
            assert!(done["messageId"].is_string());
            assert_eq!(done["finishReason"], "completed");
            for field in ["actualEffort", "fastState", "stopReason"] {
                assert!(done.get(field).is_some(), "{provider}: missing {field}");
                let expected = match (provider, scene, field) {
                    ("claude", "reported", "actualEffort") => Value::String("high".into()),
                    ("claude", "reported", "fastState") => Value::String("on".into()),
                    _ => Value::Null,
                };
                assert_eq!(done[field], expected, "{provider}: unexpected {field}");
            }
            let asks = requests.lock().unwrap();
            assert_eq!(asks.len(), 4);
            for request in asks.iter() {
                assert_eq!(request["chatId"], id);
                assert_eq!(request["provider"], provider);
                assert!(request["requestId"].is_string());
                assert!(request["kind"].is_string());
                assert!(request["description"].is_string());
                assert!(request.get("change").is_some());
                assert!(request["targets"].is_array());
                assert!(request.get("reason").is_some());
                assert!(request["options"].is_array());
            }
            let detail = serde_json::to_value(chat::read(&db, &id).unwrap()).unwrap();
            assert_eq!(detail["title"], "manual-title");
            let reply = detail["messages"].as_array().unwrap().last().unwrap();
            assert_eq!(reply["text"], "hello");
            assert_eq!(reply["finishReason"], "completed");
            if provider == "claude" && scene == "reported" {
                assert_eq!(reply["actualEffort"], "high");
            } else {
                assert!(reply["actualEffort"].is_null());
            }
            let context = &events
                .iter()
                .find(|(name, _)| name == "chat-context-updated")
                .unwrap()
                .1;
            assert_eq!(context["chatId"], id);
            assert_eq!(context["context"]["usedTokens"], 12);
            assert_eq!(context["context"]["limitTokens"], 1000);
            if scene == "complete" {
                outcomes.push(
                    asks.iter()
                        .map(|r| (r["kind"].clone(), r["change"].clone()))
                        .collect::<Vec<_>>(),
                );
            }
        }
    }
    assert_eq!(outcomes[0][0].1, "create");
    assert_eq!(outcomes[0][1].1, "edit");
    assert_eq!(outcomes[0][2].1, "delete");
    assert_eq!(outcomes[1][2].0, "command");
    assert_eq!(outcomes[1][2].1, "delete");
    assert_eq!(outcomes[2][2].1, "delete");
}
