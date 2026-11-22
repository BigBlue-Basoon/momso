//! Explicitly gated live checks; never run during the normal test suite.
use crate::{chat, chat_context};
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

const PROMPT: &str = "Reply with exactly OK.";

#[cfg(windows)]
#[tokio::test]
#[ignore = "requires installed agy; sends a dummy key only to a local fake Gemini API"]
async fn gemini_local_api_one_turn() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let requests = Arc::new(AtomicUsize::new(0));
    let tool_response = Arc::new(AtomicBool::new(false));
    let text_seen = Arc::new(AtomicBool::new(false));
    let image_seen = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let count = requests.clone();
    let response_seen = tool_response.clone();
    let saw_text = text_seen.clone();
    let saw_image = image_seen.clone();
    let mut tool_injected = false;
    let case = std::env::var("MOMSO_LOCAL_API_CASE").unwrap_or_else(|_| "ok".into());
    assert!(matches!(
        case.as_str(),
        "ok" | "auth" | "quota" | "quota_header" | "server" | "disconnect" | "tool" | "attachments"
    ));
    let server_case = case.clone();
    let worker = std::thread::spawn(move || {
        use sha2::{Digest, Sha256};
        let mut request_counts = std::collections::HashMap::<[u8; 32], usize>::new();
        while !stop.load(Ordering::SeqCst) {
            let Ok((mut socket, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut input = Vec::new();
            let mut buffer = [0_u8; 4096];
            while !input.windows(4).any(|w| w == b"\r\n\r\n") {
                let Ok(n) = socket.read(&mut buffer) else {
                    break;
                };
                if n == 0 {
                    break;
                }
                input.extend_from_slice(&buffer[..n]);
                if input.len() > 65536 {
                    break;
                }
            }
            if let Some(end) = input.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&input[..end]);
                let body_len = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                if body_len <= 1_000_000 {
                    let mut remaining =
                        body_len.saturating_sub(input.len().saturating_sub(end + 4));
                    while remaining > 0 {
                        let Ok(n) = socket.read(&mut buffer[..remaining.min(4096)]) else {
                            break;
                        };
                        if n == 0 {
                            break;
                        }
                        input.extend_from_slice(&buffer[..n]);
                        remaining -= n;
                    }
                }
            }
            if input.starts_with(b"POST ")
                && input
                    .windows(b":streamGenerateContent".len())
                    .any(|w| w == b":streamGenerateContent")
            {
                count.fetch_add(1, Ordering::SeqCst);
                if let Some(end) = input.windows(4).position(|w| w == b"\r\n\r\n") {
                    let digest: [u8; 32] = Sha256::digest(&input[end + 4..]).into();
                    *request_counts.entry(digest).or_default() += 1;
                }
                if server_case == "attachments" {
                    if input
                        .windows(b"C50_SAMPLE_TEXT".len())
                        .any(|w| w == b"C50_SAMPLE_TEXT")
                    {
                        saw_text.store(true, Ordering::SeqCst);
                    }
                    if input
                        .windows(b"inlineData".len())
                        .any(|w| w == b"inlineData")
                        && input.windows(b"image/png".len()).any(|w| w == b"image/png")
                        && input
                            .windows(b"iVBORw0KGgoAAAANSUhEUg".len())
                            .any(|w| w == b"iVBORw0KGgoAAAANSUhEUg")
                    {
                        saw_image.store(true, Ordering::SeqCst);
                    }
                }
                if server_case == "tool"
                    && count.load(Ordering::SeqCst) > 1
                    && input
                        .windows(b"functionResponse".len())
                        .any(|w| w == b"functionResponse")
                    && input
                        .windows(b"momso_version".len())
                        .any(|w| w == b"momso_version")
                {
                    response_seen.store(true, Ordering::SeqCst);
                }
            }
            if server_case == "disconnect" {
                continue;
            }
            let (status, content_type, body) = match server_case.as_str() {
                "auth" => ("401 Unauthorized", "application/json", "{\"error\":{\"code\":401,\"status\":\"UNAUTHENTICATED\",\"message\":\"invalid test key\"}}".to_owned()),
                "quota" => ("429 Too Many Requests", "application/json", "{\"error\":{\"code\":429,\"status\":\"RESOURCE_EXHAUSTED\",\"message\":\"test quota\",\"details\":[{\"@type\":\"type.googleapis.com/google.rpc.RetryInfo\",\"retryDelay\":\"120s\"}]}}".to_owned()),
                "quota_header" => ("429 Too Many Requests", "application/json", "{\"error\":{\"code\":429,\"status\":\"RESOURCE_EXHAUSTED\",\"message\":\"test quota\"}}".to_owned()),
                "server" => ("503 Service Unavailable", "application/json", "{\"error\":{\"code\":503,\"status\":\"UNAVAILABLE\",\"message\":\"test server unavailable\"}}".to_owned()),
                "tool" if input.starts_with(b"POST ") && input.windows(b":streamGenerateContent".len()).any(|w| w == b":streamGenerateContent") => {
                    let body = input.windows(4).position(|w| w == b"\r\n\r\n").map(|i| &input[i+4..]).unwrap_or_default();
                    let request: serde_json::Value = serde_json::from_slice(body).unwrap_or_else(|_| {
                        let headers = String::from_utf8_lossy(&input[..input.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(0)]);
                        let relevant: Vec<_> = headers.lines().filter(|line| {
                            let name = line.split(':').next().unwrap_or("");
                            ["content-length", "content-encoding", "transfer-encoding"].iter().any(|v| name.eq_ignore_ascii_case(v))
                        }).collect();
                        panic!("model request body not JSON: {} bytes, {:?}", body.len(), relevant);
                    });
                    fn tool_name(value: &serde_json::Value) -> Option<&str> {
                        match value {
                            serde_json::Value::Object(map) => map.get("name").and_then(serde_json::Value::as_str).filter(|name| name.ends_with("momso_version")).or_else(|| map.values().find_map(tool_name)),
                            serde_json::Value::Array(values) => values.iter().find_map(tool_name),
                            _ => None,
                        }
                    }
                    let name = request.get("tools").and_then(tool_name);
                    if let Some(name) = name.filter(|_| !tool_injected) {
                        tool_injected = true;
                        let chunk = serde_json::json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":name,"args":{}}}]},"finishReason":"STOP"}]});
                        ("200 OK", "text/event-stream", format!("data: {chunk}\n\n"))
                    } else {
                        println!("LOCAL_API_TOOL_REQUEST has_prompt={} has_tools={}", body.windows(PROMPT.len()).any(|w| w == PROMPT.as_bytes()), request.get("tools").is_some());
                        ("200 OK", "text/event-stream", "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"OK\"}]},\"finishReason\":\"STOP\"}]}\n\n".to_owned())
                    }
                }
                _ => ("200 OK", "text/event-stream", "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"OK\"}]},\"finishReason\":\"STOP\"}]}\n\n".to_owned()),
            };
            let retry = if server_case == "quota" || server_case == "quota_header" {
                "Retry-After: 120\r\n"
            } else {
                ""
            };
            let header = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{retry}Connection: close\r\n\r\n", body.len());
            let _ = socket.write_all(header.as_bytes());
            let _ = socket.write_all(body.as_bytes());
        }
        let mut counts: Vec<_> = request_counts.into_values().collect();
        counts.sort_unstable();
        println!("LOCAL_API_SAME_REQUEST_COUNTS={counts:?}");
    });
    let (dir, db, archive, id) = fixture();
    let mut blocks = Vec::new();
    if case == "attachments" {
        use base64::Engine as _;
        use sha2::{Digest, Sha256};
        let image = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=").unwrap();
        let items = [
            ("sample.txt", "text/plain", b"C50_SAMPLE_TEXT".as_slice()),
            ("sample.png", "image/png", image.as_slice()),
        ];
        let mut attachments = Vec::new();
        for (name, media_type, bytes) in items {
            let hash = format!("{:x}", Sha256::digest(bytes));
            let relative = format!("objects/sha256/{}/{}", &hash[..2], hash);
            let path = archive.join(&relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
            attachments.push(chat::MessageAttachment {
                path: relative,
                name: name.into(),
                media_type: Some(media_type.into()),
            });
        }
        blocks = crate::chat_gemini::attachment_blocks(&archive, &attachments).unwrap();
    }
    let executable = crate::gemini_access::executable().expect("agy not installed");
    let mut cmd = crate::process::hidden_async_command(executable);
    cmd.args([
        "--settings",
        "{\"modelProvider\":\"gemini\",\"permissions\":{\"deny\":[\"write_file(objects/)\"]}}",
        "acp",
        "--gemini-dir",
    ])
    .arg(dir.path().join("gemini"))
    .args(["--log-file", "NUL"])
    .env("GOOGLE_GEMINI_BASE_URL", format!("http://{address}"))
    .env("HOME", dir.path())
    .env("USERPROFILE", dir.path())
    .env("APPDATA", dir.path())
    .env("LOCALAPPDATA", dir.path());
    let mut events = Vec::new();
    let result = tokio::time::timeout(
        Duration::from_secs(if std::env::var_os("MOMSO_LOCAL_API_TRACE").is_some() {
            25
        } else {
            90
        }),
        crate::chat_gemini::run_command(
            cmd,
            &db,
            &archive,
            &id,
            PROMPT,
            blocks,
            None,
            None,
            "default",
            None,
            "local-test-key".into(),
            None,
            CancellationToken::new(),
            &mut |name, value| events.push((name.to_owned(), value)),
        ),
    )
    .await;
    stopped.store(true, Ordering::SeqCst);
    worker.join().unwrap();
    println!(
        "LOCAL_API_REQUEST_COUNT={}",
        requests.load(Ordering::SeqCst)
    );
    assert!(requests.load(Ordering::SeqCst) > 0);
    let result = result.expect("local API test timed out");
    if case == "ok" || case == "tool" || case == "attachments" {
        assert!(result.is_ok(), "product Gemini request failed");
        if case == "tool" {
            assert!(
                requests.load(Ordering::SeqCst) >= 2,
                "tool result did not return to API"
            );
            assert!(
                tool_response.load(Ordering::SeqCst),
                "tool result is missing from API request"
            );
        }
        if case == "attachments" {
            assert!(
                text_seen.load(Ordering::SeqCst),
                "text attachment missing from API request"
            );
            assert!(
                image_seen.load(Ordering::SeqCst),
                "image attachment missing from API request"
            );
        }
        assert!(events.iter().any(|(name, value)| name == "chat-stream"
            && value["delta"].as_str().is_some_and(|v| v.contains("OK"))));
        assert!(events.iter().any(|(name, _)| name == "chat-done"));
    } else {
        if result.is_ok() {
            println!("LOCAL_API_{case}_COMPLETED=true");
        }
        let error = result.expect_err("fake API failure was not reported");
        println!("LOCAL_API_{case}_RESULT={error}");
        if case == "auth" {
            assert_eq!(error, "__momso_auth_failed");
            assert!(!events.iter().any(|(name, _)| name == "chat-stream"));
        } else if case == "quota" {
            assert_eq!(error, "__momso_gemini_rate_limited:120");
            assert!(events
                .iter()
                .any(|(name, value)| name == "chat-provider-retrying"
                    && value["retryAfterSeconds"] == 120));
            let payload = crate::chat_finish::provider_error_payload(&id, &error);
            assert_eq!(payload["error"]["kind"], "rateLimited");
            assert!(payload["error"]["retryAt"].is_number());
        } else if case == "quota_header" {
            assert_eq!(error, "__momso_gemini_rate_limited:120");
            assert!(
                crate::chat_finish::provider_error_payload(&id, &error)["error"]["retryAt"]
                    .is_number()
            );
        } else if case == "server" {
            assert_eq!(error, "__momso_gemini_service_unavailable");
            assert_eq!(
                crate::chat_finish::provider_error_payload(&id, &error)["error"]["kind"],
                "serviceUnavailable"
            );
        } else if case == "disconnect" {
            assert_eq!(error, "AI 연결이 끊겼습니다");
            assert_eq!(
                crate::chat_finish::provider_error_payload(&id, &error)["error"]["kind"],
                "connectionClosed"
            );
        }
        assert!(!events.iter().any(|(name, _)| name == "chat-done"));
    }
}

fn enabled() -> bool {
    let enabled = std::env::var_os("MOMSO_RUN_LIVE_PROVIDER_SMOKE").is_some();
    assert!(
        !fake_enabled() || cfg!(windows),
        "fake_cli_not_supported_on_this_platform"
    );
    enabled
}

#[cfg(windows)]
fn fake_command(dir: &std::path::Path, provider: &str, scenario: &str) -> tokio::process::Command {
    let source = match provider {
        "codex" => {
            r#"
$null=[Console]::In.ReadLine()
[Console]::WriteLine('{"id":1,"result":{}}')
$null=[Console]::In.ReadLine()
$null=[Console]::In.ReadLine()
[Console]::WriteLine('{"id":2,"result":{"thread":{"id":"fixture-thread"},"approvalPolicy":"on-request","approvalsReviewer":"user","sandbox":{"type":"workspaceWrite"},"model":"fixture-model","reasoningEffort":"low"}}')
$turn=[Console]::In.ReadLine() | ConvertFrom-Json
if ($turn.method -ne 'turn/start' -or $turn.params.effort -ne 'low') { exit 12 }
if ($env:MOMSO_ENGINE_SCENARIO -in @('allow','deny')) {
 [Console]::WriteLine('{"id":99,"method":"item/fileChange/requestApproval","params":{"grantRoot":"sample.txt","fileChanges":{"sample.txt":{"type":"add"}}}}')
 $reply=[Console]::In.ReadLine() | ConvertFrom-Json
 $expected=if ($env:MOMSO_ENGINE_SCENARIO -eq 'allow') {'accept'} else {'decline'}
 if ($reply.result.decision -ne $expected) { exit 11 }
}
if ($env:MOMSO_ENGINE_SCENARIO -eq 'deny') {
 [Console]::WriteLine('{"method":"item/agentMessage/delta","params":{"delta":"DENIED"}}')
} else { [Console]::WriteLine('{"method":"item/agentMessage/delta","params":{"delta":"OK"}}') }
if ($env:MOMSO_ENGINE_SCENARIO -eq 'stop') { [Threading.Thread]::Sleep(30000) }
[Console]::WriteLine('{"method":"turn/completed","params":{"turn":{"status":"completed"}}}')
"#
        }
        "claude" => {
            r#"
$null=[Console]::In.ReadLine()
[Console]::WriteLine('{"type":"control_response","response":{"request_id":"momso-init","subtype":"success"}}')
$null=[Console]::In.ReadLine()
[Console]::WriteLine('{"type":"system","subtype":"init","session_id":"fake-session"}')
[Console]::WriteLine('{"type":"assistant","message":{"model":"fake-haiku","content":[]}}')
if ($env:MOMSO_ENGINE_SCENARIO -in @('allow','deny')) {
 [Console]::WriteLine('{"type":"control_request","request_id":"approve","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"sample.txt","content":"x"}}}')
 $reply=[Console]::In.ReadLine() | ConvertFrom-Json
 $expected=if ($env:MOMSO_ENGINE_SCENARIO -eq 'allow') {'allow'} else {'deny'}
 if ($reply.response.response.behavior -ne $expected) { exit 11 }
}
if ($env:MOMSO_ENGINE_SCENARIO -eq 'deny') {
 [Console]::WriteLine('{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"DENIED"}}}')
} else { [Console]::WriteLine('{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"OK"}}}') }
if ($env:MOMSO_ENGINE_SCENARIO -eq 'stop') { [Threading.Thread]::Sleep(30000) }
[Console]::WriteLine('{"type":"result","is_error":false}')
$null=[Console]::In.ReadLine()
[Console]::WriteLine('{"type":"control_response","response":{"request_id":"momso-context","subtype":"success","response":{"model":"fake-haiku","totalTokens":12,"maxTokens":1000}}}')
"#
        }
        "gemini" => {
            r#"
while ($line=[Console]::In.ReadLine()) {
 $v=$line | ConvertFrom-Json
 switch ($v.method) {
 'initialize' { '{"jsonrpc":"2.0","id":1,"result":{"agentCapabilities":{"sessionCapabilities":{"resume":{}}}}}' }
 'authenticate' { '{"jsonrpc":"2.0","id":2,"result":{}}' }
 'session/new' { '{"jsonrpc":"2.0","id":3,"result":{"sessionId":"fake-session","configOptions":[{"id":"mode","currentValue":"default"},{"id":"model","currentValue":"fake"},{"id":"thought_level","currentValue":"low"}]}}' }
 'session/resume' { '{"jsonrpc":"2.0","id":3,"result":{"sessionId":"fake-session","configOptions":[{"id":"mode","currentValue":"default"},{"id":"model","currentValue":"fake"},{"id":"thought_level","currentValue":"low"}]}}' }
 'session/prompt' {
  if ($env:MOMSO_ENGINE_SCENARIO -in @('allow','deny')) {
   '{"jsonrpc":"2.0","id":99,"method":"session/request_permission","params":{"toolCall":{"toolCallId":"edit-1","kind":"edit","title":"Write","locations":[{"path":"sample.txt"}]},"options":[{"kind":"allow_once","optionId":"yes"},{"kind":"reject_once","optionId":"no"}]}}'
   $reply=[Console]::In.ReadLine() | ConvertFrom-Json
   $expected=if ($env:MOMSO_ENGINE_SCENARIO -eq 'allow') {'yes'} else {'no'}
   if ($reply.result.outcome.optionId -ne $expected) { exit 11 }
  }
  if ($env:MOMSO_ENGINE_SCENARIO -eq 'deny') {
   '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"DENIED"}}}}'
  } else { '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"OK"}}}}' }
  if ($env:MOMSO_ENGINE_SCENARIO -eq 'stop') { [Threading.Thread]::Sleep(30000) }
  '{"jsonrpc":"2.0","id":7,"result":{"stopReason":"end_turn"}}'
 }
 }
}
"#
        }
        _ => unreachable!(),
    };
    let script = dir.join(format!("{provider}-fake.ps1"));
    std::fs::write(&script, source).unwrap();
    let mut command = tokio::process::Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(script)
        .env("MOMSO_ENGINE_SCENARIO", scenario);
    command
}

#[cfg(windows)]
fn fake_approval(
    id: &str,
    scenario: &str,
) -> (
    Option<crate::chat_codex::ApprovalChannel>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use crate::approval::ApprovalChoice;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let seen = Arc::new(AtomicBool::new(false));
    if !matches!(scenario, "allow" | "deny") {
        return (None, seen);
    }
    let choice = if scenario == "allow" {
        ApprovalChoice::AllowOnce
    } else {
        ApprovalChoice::Deny
    };
    let expected_id = id.to_owned();
    let marked = seen.clone();
    let channel: crate::chat_codex::ApprovalChannel = Box::new(move |request, _| {
        assert_eq!(request.chat_id.as_deref(), Some(expected_id.as_str()));
        assert!(request
            .targets
            .iter()
            .any(|target| target.path == "sample.txt"));
        assert!(request.options.contains(&choice));
        marked.store(true, Ordering::SeqCst);
        Box::pin(async move { Ok(choice) })
    });
    (Some(channel), seen)
}

#[cfg(windows)]
fn scenario() -> String {
    let value = std::env::var("MOMSO_ENGINE_SCENARIO").unwrap_or_else(|_| "normal".into());
    assert!(matches!(
        value.as_str(),
        "normal" | "allow" | "deny" | "stop"
    ));
    value
}

fn fake_enabled() -> bool {
    std::env::var_os("MOMSO_ENGINE_FAKE").is_some()
}

#[cfg(windows)]
async fn fake_round(
    provider: &str,
    dir: &std::path::Path,
    db: &std::path::Path,
    archive: &std::path::Path,
    id: &str,
    scenario: &str,
) {
    use std::sync::atomic::Ordering;
    let (approval, seen) = fake_approval(id, scenario);
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let mut capture = crate::chat_finish::Capture::default();
    let mut events = Vec::new();
    let mut emit = |name: &str, value: serde_json::Value| {
        capture.observe(name, &value);
        if scenario == "stop" && name == "chat-stream" {
            trigger.cancel();
        }
        events.push((name.to_owned(), value));
    };
    let result = match provider {
        "codex" => {
            use std::process::Stdio;
            let mut cmd = fake_command(dir, provider, scenario);
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            crate::chat_codex::run_command(
                cmd,
                db,
                archive,
                id,
                PROMPT,
                &[],
                None,
                Some("low"),
                None,
                None,
                None,
                &crate::permission_modes::codex_settings("Ask for approval").unwrap(),
                approval,
                cancel,
                chat::codex_thread(db, id).unwrap(),
                None,
                None,
                &mut emit,
            )
            .await
        }
        "claude" => {
            crate::chat_claude::run_command_ref(
                fake_command(dir, provider, scenario),
                db,
                archive,
                id,
                PROMPT,
                &[],
                Some("haiku"),
                Some("low"),
                false,
                None,
                None,
                &approval,
                None,
                cancel,
                &mut emit,
            )
            .await
        }
        "gemini" => {
            crate::chat_gemini::run_command(
                fake_command(dir, provider, scenario),
                db,
                archive,
                id,
                PROMPT,
                Vec::new(),
                None,
                Some("low"),
                "default",
                None,
                "fake-key".into(),
                approval,
                cancel,
                &mut emit,
            )
            .await
        }
        _ => unreachable!(),
    };
    if scenario == "stop" {
        assert!(result.is_err());
        assert!(capture
            .persist_partial(db, id, provider, true, 0)
            .unwrap()
            .is_some());
        let detail = serde_json::to_value(chat::read(db, id).unwrap()).unwrap();
        assert!(detail["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "assistant"
                && message["text"] == "OK"
                && message["finishReason"] == "cancelled"));
        chat::append(db, id, chat::Role::User, PROMPT, None, None).unwrap();
        Box::pin(fake_round(provider, dir, db, archive, id, "normal")).await;
        return;
    }
    assert!(result.is_ok(), "fake_provider_failed");
    if matches!(scenario, "allow" | "deny") {
        assert!(seen.load(Ordering::SeqCst));
    }
    if scenario == "deny" {
        let detail = serde_json::to_value(chat::read(db, id).unwrap()).unwrap();
        assert!(detail["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "assistant" && message["text"] == "DENIED"));
    }
    verify(db, id, &events);
}

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, String) {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("archive");
    std::fs::create_dir(&archive).unwrap();
    std::fs::write(archive.join("AGENTS.md"), "Local test.").unwrap();
    std::fs::write(archive.join("CLAUDE.md"), "Local test.").unwrap();
    let db = dir.path().join("chat.sqlite");
    crate::storage::initialize(&db, &dir.path().join("backup"), "test").unwrap();
    let value = serde_json::to_value(chat::create(&db).unwrap()).unwrap();
    let id = value["id"].as_str().unwrap().to_owned();
    chat::append(&db, &id, chat::Role::User, PROMPT, None, None).unwrap();
    // A manual title avoids a second, title-generation model request.
    chat::rename(&db, &id, "Smoke").unwrap();
    (dir, db, archive, id)
}

fn verify(db: &std::path::Path, id: &str, events: &[(String, serde_json::Value)]) {
    assert!(events.iter().any(|(name, _)| name == "chat-stream"));
    assert!(events.iter().any(|(name, _)| name == "chat-done"));
    let detail = serde_json::to_value(chat::read(db, id).unwrap()).unwrap();
    let answer = detail["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "assistant")
        .unwrap();
    if let Some(effort) = answer["actualEffort"].as_str() {
        assert_eq!(effort, "low", "effective_effort_mismatch");
    }
    assert_eq!(answer["effort"], "low", "requested_effort_missing");
    let context = serde_json::to_value(chat_context::get(db, id).unwrap()).unwrap();
    println!("MODEL_REPORTED={}", answer["model"].is_string());
    println!("EFFORT_REPORTED={}", answer["actualEffort"].is_string());
    println!("EFFORT_REQUESTED={}", answer["effort"].is_string());
    println!("CONTEXT_REPORTED={}", context["usedTokens"].is_number());
}

#[tokio::test]
#[ignore = "sends one real Codex request"]
async fn codex_one_turn() {
    if !enabled() {
        return;
    }
    let (dir, db, archive, id) = fixture();
    let mut events = Vec::new();
    #[cfg(windows)]
    if fake_enabled() {
        fake_round("codex", dir.path(), &db, &archive, &id, &scenario()).await;
        return;
    }
    let _keep_dir = dir;
    let result = crate::chat_codex::run(
        &db,
        &archive,
        &id,
        PROMPT,
        None,
        Some("low"),
        &crate::permission_modes::codex_settings("Ask for approval").unwrap(),
        None,
        CancellationToken::new(),
        |name, value| events.push((name.to_owned(), value)),
    )
    .await;
    if let Ok(Some(thread)) = chat::codex_thread(&db, &id) {
        if crate::chat_codex::archive_thread(&thread).await.is_err() {
            println!("SMOKE_THREAD_NOT_ARCHIVED={thread}");
            panic!("thread_archive_failed");
        }
    }
    assert!(result.is_ok(), "provider_failed");
    verify(&db, &id, &events);
}

#[tokio::test]
#[ignore = "sends one real Claude request"]
async fn claude_one_turn() {
    if !enabled() {
        return;
    }
    let (dir, db, archive, id) = fixture();
    let mut events = Vec::new();
    #[cfg(windows)]
    if fake_enabled() {
        fake_round("claude", dir.path(), &db, &archive, &id, &scenario()).await;
        return;
    }
    let _keep_dir = dir;
    let result = crate::chat_claude::run(
        &db,
        &archive,
        &id,
        PROMPT,
        Some("haiku"),
        Some("low"),
        "default",
        false,
        None,
        CancellationToken::new(),
        |name, value| events.push((name.to_owned(), value)),
    )
    .await;
    assert!(result.is_ok(), "provider_failed");
    verify(&db, &id, &events);
}

#[tokio::test]
#[ignore = "sends one real Gemini request"]
async fn gemini_one_turn() {
    if !enabled() {
        return;
    }
    let name = crate::credentials::ai_key_name("gemini").unwrap();
    let key = if fake_enabled() {
        Some("fake-key".to_owned())
    } else {
        crate::credentials::get_secret(name).unwrap()
    };
    let Some(key) = key else {
        println!("SKIPPED=api_key_missing");
        return;
    };
    let (dir, db, archive, id) = fixture();
    let mut events = Vec::new();
    #[cfg(windows)]
    if fake_enabled() {
        fake_round("gemini", dir.path(), &db, &archive, &id, &scenario()).await;
        return;
    }
    let _keep_dir = dir;
    let result = crate::chat_gemini::run(
        &db,
        &archive,
        &id,
        PROMPT,
        &[],
        None,
        Some("low"),
        "default",
        None,
        key,
        None,
        CancellationToken::new(),
        None,
        &mut |name, value| events.push((name.to_owned(), value)),
    )
    .await;
    assert!(result.is_ok(), "provider_failed");
    verify(&db, &id, &events);
}
