//! Per-chat broker for the app's authenticated setup pipe.
#![allow(dead_code)]
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex, OnceLock, Weak,
    },
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

struct Entry {
    name: String,
    pid: AtomicU32,
    cancel: CancellationToken,
}
static ENTRIES: OnceLock<Mutex<HashMap<String, Weak<Entry>>>> = OnceLock::new();
fn entries() -> &'static Mutex<HashMap<String, Weak<Entry>>> {
    ENTRIES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) struct Guard {
    chat_id: String,
    entry: Arc<Entry>,
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.entry.cancel.cancel();
        if let Ok(mut all) = entries().lock() {
            if all
                .get(&self.chat_id)
                .and_then(Weak::upgrade)
                .is_some_and(|v| Arc::ptr_eq(&v, &self.entry))
            {
                all.remove(&self.chat_id);
            }
        }
    }
}

pub(crate) fn name(chat_id: &str) -> Option<String> {
    entries()
        .lock()
        .ok()?
        .get(chat_id)?
        .upgrade()
        .map(|v| v.name.clone())
}
pub(crate) fn bind(chat_id: &str, pid: Option<u32>) {
    if let (Some(entry), Some(pid)) = (
        entries()
            .lock()
            .ok()
            .and_then(|m| m.get(chat_id).and_then(Weak::upgrade)),
        pid,
    ) {
        entry.pid.store(pid, Ordering::Release);
    }
}

pub(crate) type Handler = Arc<
    dyn Fn(serde_json::Value) -> Pin<Box<dyn Future<Output = serde_json::Value> + Send>>
        + Send
        + Sync,
>;

pub(crate) fn start(
    chat_id: &str,
    cancel: CancellationToken,
    handler: Handler,
) -> std::io::Result<Guard> {
    let (name, server) = crate::setup_pipe::create()?;
    let entry = Arc::new(Entry {
        name,
        pid: AtomicU32::new(0),
        cancel: cancel.child_token(),
    });
    {
        let mut all = entries()
            .lock()
            .map_err(|_| std::io::Error::other("unavailable"))?;
        if all.get(chat_id).and_then(Weak::upgrade).is_some() {
            return Err(std::io::Error::other("already active"));
        }
        all.insert(chat_id.to_owned(), Arc::downgrade(&entry));
    }
    let worker = entry.clone();
    tauri::async_runtime::spawn(async move {
        let pid = loop {
            let pid = worker.pid.load(Ordering::Acquire);
            if pid != 0 {
                break pid;
            }
            tokio::select! { _ = worker.cancel.cancelled() => return, _ = tokio::time::sleep(Duration::from_millis(20)) => {} }
        };
        let Ok(executable) = std::env::current_exe() else {
            return;
        };
        let pipe = tokio::select! { _ = worker.cancel.cancelled() => return, result = crate::setup_pipe::accept(server, pid, &executable) => match result { Ok(v)=>v, Err(_)=>return } };
        let (read, mut write) = tokio::io::split(pipe);
        let mut lines = BufReader::new(read).lines();
        loop {
            let line = tokio::select! { _ = worker.cancel.cancelled() => break, v = lines.next_line() => match v { Ok(Some(v)) if v.len() <= 65_536 => v, _ => break } };
            let result = match serde_json::from_str::<serde_json::Value>(&line) {
                Ok(v)
                    if v.get("method").and_then(serde_json::Value::as_str)
                        == Some("browser_open") =>
                {
                    let pending = handler(v);
                    tokio::pin!(pending);
                    tokio::select! {
                        result = &mut pending => result,
                        _ = worker.cancel.cancelled() => tokio::time::timeout(Duration::from_millis(250), &mut pending).await.unwrap_or_else(|_| serde_json::json!({"status":"panel_closed"})),
                    }
                }
                Ok(v) => {
                    tokio::select! { _ = worker.cancel.cancelled() => break, result = handler(v) => result }
                }
                _ => serde_json::json!({"status":"unavailable"}),
            };
            if write
                .write_all(format!("{result}\n").as_bytes())
                .await
                .is_err()
            {
                break;
            }
            if worker.cancel.is_cancelled() {
                break;
            }
        }
    });
    Ok(Guard {
        chat_id: chat_id.to_owned(),
        entry,
    })
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    #[ignore = "requires an explicitly authorized real Codex account and a built app executable"]
    async fn real_codex_calls_document_list_ephemerally() {
        use super::*;
        let executable = std::env::var_os("MOMSO_T71B_CODEX").expect("Codex executable");
        let app =
            std::path::PathBuf::from(std::env::var_os("MOMSO_T71B_APP").expect("app executable"));
        let list = crate::process::hidden_command(&executable)
            .args(["mcp", "list", "--json"])
            .output()
            .unwrap();
        assert!(
            list.status.success(),
            "could not inspect installed MCP names"
        );
        let servers: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        crate::archive::initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        crate::archive_zet::create(
            &root,
            &db,
            crate::archive_zet::ZetCreate {
                title: "fixture-momso-list-unique".into(),
                summary: "fixture".into(),
                kind: "note".into(),
                body: "fixture".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        let chat = format!("test-{}", uuid::Uuid::new_v4());
        let handler: Handler = Arc::new(move |input| {
            let root = root.clone();
            let db = db.clone();
            Box::pin(async move {
                crate::momso_document_read::dispatch(
                    &root,
                    &db,
                    input["method"].as_str().unwrap_or(""),
                    &input["arguments"],
                )
            })
        });
        let _guard = start(&chat, CancellationToken::new(), handler).unwrap();
        let mut command = crate::process::hidden_async_command(&executable);
        for server in servers.as_array().unwrap() {
            let name = server["name"].as_str().unwrap();
            assert!(name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'));
            command
                .arg("-c")
                .arg(format!("mcp_servers.{name}.enabled=false"));
        }
        for option in crate::momso_mcp::codex_config(&app, &chat).unwrap() {
            command.arg("-c").arg(option);
        }
        command.args(["exec", "--ephemeral", "--skip-git-repo-check", "--json", "-s", "read-only", "Use the momso_document_list MCP tool, then report the document title exactly. Do not run shell commands or use any other tool."])
            .current_dir(temp.path()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
        let child = command.spawn().unwrap();
        bind(&chat, child.id());
        let output = tokio::time::timeout(Duration::from_secs(180), child.wait_with_output())
            .await
            .expect("Codex timed out")
            .unwrap();
        let lines = String::from_utf8_lossy(&output.stdout);
        let mut called = false;
        let mut answer = None;
        for line in lines.lines() {
            if let Ok(event) = serde_json::from_str::<serde_json::Value>(line) {
                let item = &event["item"];
                if item["type"] == "mcp_tool_call"
                    && item["tool"] == "momso_document_list"
                    && item["status"] == "completed"
                {
                    called = true;
                }
                if item["type"] == "agent_message" {
                    answer = item["text"].as_str().map(str::to_owned);
                }
            }
        }
        println!("REAL_CODEX_MCP_CALLED={called}");
        println!(
            "REAL_CODEX_ANSWER={}",
            answer.as_deref().unwrap_or("<no answer>")
        );
        assert!(
            output.status.success() && called && answer.is_some(),
            "real Codex did not complete the document tool call; status={}",
            output.status
        );
    }
    #[test]
    fn fake_document_provider_child() {
        let Some(pipe) = std::env::var_os("MOMSO_TEST_DOCUMENT_PIPE") else {
            return;
        };
        let id = std::env::var("MOMSO_TEST_DOCUMENT_ID").unwrap();
        let provider = std::env::var("MOMSO_TEST_DOCUMENT_PROVIDER").unwrap();
        let config = std::env::var("MOMSO_TEST_DOCUMENT_CONFIG").unwrap();
        assert!(["codex", "claude", "gemini"].contains(&provider.as_str()));
        assert!(config.contains(crate::momso_mcp::NAME) && config.contains("momso-setup-"));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(pipe)
            .unwrap();
        let mut client = crate::momso_mcp::test_bridge(file);
        let mut ready = true;
        let list = crate::momso_mcp::test_response(
            &serde_json::json!({"id":1,"method":"tools/list"}),
            &mut ready,
            &mut client,
        )
        .unwrap();
        let tools = list["result"]["tools"].as_array().unwrap();
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "momso_document_search"));
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "momso_document_read"));
        let found = crate::momso_mcp::test_response(&serde_json::json!({"id":2,"method":"tools/call","params":{"name":"momso_document_search","arguments":{"query":"균형"}}}), &mut ready, &mut client).unwrap();
        assert!(found.to_string().contains(&id));
        let read = crate::momso_mcp::test_response(&serde_json::json!({"id":3,"method":"tools/call","params":{"name":"momso_document_read","arguments":{"id":id}}}), &mut ready, &mut client).unwrap();
        assert!(read.to_string().contains("호흡과 균형"));
    }

    #[tokio::test]
    async fn fake_providers_use_authenticated_document_tools() {
        use super::*;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        crate::archive::initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let id = crate::archive_zet::create(
            &root,
            &db,
            crate::archive_zet::ZetCreate {
                title: "수업".into(),
                summary: "요약".into(),
                kind: "note".into(),
                body: "호흡과 균형".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        for provider in ["codex", "claude", "gemini"] {
            let chat = format!("test-{}", uuid::Uuid::new_v4());
            let handler_root = root.clone();
            let handler_db = db.clone();
            let handler: Handler = Arc::new(move |input| {
                let root = handler_root.clone();
                let db = handler_db.clone();
                Box::pin(async move {
                    crate::momso_document_read::dispatch(
                        &root,
                        &db,
                        input["method"].as_str().unwrap_or(""),
                        &input["arguments"],
                    )
                })
            });
            let guard = start(&chat, CancellationToken::new(), handler).unwrap();
            let pipe = name(&chat).unwrap();
            let executable = std::env::current_exe().unwrap();
            let config = match provider {
                "codex" => serde_json::to_string(
                    &crate::momso_mcp::codex_config(&executable, &chat).unwrap(),
                )
                .unwrap(),
                "claude" => crate::momso_mcp::claude_config(&executable, &chat).to_string(),
                _ => crate::momso_mcp::acp_config(&executable, &chat).to_string(),
            };
            let child = crate::process::hidden_async_command(executable)
                .args([
                    "--exact",
                    "setup_bridge::tests::fake_document_provider_child",
                    "--test-threads=1",
                ])
                .env("MOMSO_TEST_DOCUMENT_PIPE", pipe)
                .env("MOMSO_TEST_DOCUMENT_ID", &id)
                .env("MOMSO_TEST_DOCUMENT_PROVIDER", provider)
                .env("MOMSO_TEST_DOCUMENT_CONFIG", config)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            bind(&chat, child.id());
            let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
                .await
                .unwrap()
                .unwrap();
            assert!(
                output.status.success(),
                "fake provider {provider} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            drop(guard);
        }
    }

    #[tokio::test]
    async fn authenticated_document_tools_search_and_read() {
        use super::*;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        crate::archive::initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let id = crate::archive_zet::create(
            &root,
            &db,
            crate::archive_zet::ZetCreate {
                title: "수업".into(),
                summary: "요약".into(),
                kind: "note".into(),
                body: "호흡과 균형".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        let chat_id = format!("test-{}", uuid::Uuid::new_v4());
        let handler: Handler = Arc::new(move |input| {
            let root = root.clone();
            let db = db.clone();
            Box::pin(async move {
                crate::momso_document_read::dispatch(
                    &root,
                    &db,
                    input["method"].as_str().unwrap_or(""),
                    &input["arguments"],
                )
            })
        });
        let _guard = start(&chat_id, CancellationToken::new(), handler).unwrap();
        bind(&chat_id, Some(std::process::id()));
        let pipe = name(&chat_id).unwrap();
        let executable = std::env::current_exe().unwrap();
        assert!(
            crate::momso_mcp::codex_config(&executable, &chat_id).unwrap()[1]
                .contains("momso-setup-")
        );
        assert_eq!(
            crate::momso_mcp::claude_config(&executable, &chat_id)["mcpServers"]
                [crate::momso_mcp::NAME]["args"][2],
            pipe
        );
        assert_eq!(
            crate::momso_mcp::acp_config(&executable, &chat_id)["args"][2],
            pipe
        );
        tokio::task::spawn_blocking(move || {
            let file = std::fs::OpenOptions::new().read(true).write(true).open(pipe).unwrap();
            let mut client = crate::momso_mcp::test_bridge(file);
            let mut ready = true;
            let listing = crate::momso_mcp::test_response(&serde_json::json!({"id":1,"method":"tools/list"}), &mut ready, &mut client).unwrap().to_string();
            assert!(listing.contains("momso_document_search") && listing.contains("momso_document_read"));
            let found = crate::momso_mcp::test_response(&serde_json::json!({"id":2,"method":"tools/call","params":{"name":"momso_document_search","arguments":{"query":"균형"}}}), &mut ready, &mut client).unwrap();
            assert!(found.to_string().contains(&id));
            let read = crate::momso_mcp::test_response(&serde_json::json!({"id":3,"method":"tools/call","params":{"name":"momso_document_read","arguments":{"id":id}}}), &mut ready, &mut client).unwrap();
            assert!(read.to_string().contains("호흡과 균형"));
        }).await.unwrap();
    }
    use super::*;
    use tokio::net::windows::named_pipe::ClientOptions;
    #[tokio::test]
    async fn cancelled_panel_open_returns_closed_before_pipe_shutdown() {
        let id = format!("test-{}", uuid::Uuid::new_v4());
        let cancel = CancellationToken::new();
        let handler_cancel = cancel.clone();
        let (ready_tx, mut ready_rx) = tokio::sync::mpsc::unbounded_channel();
        let handler: Handler = Arc::new(move |_| {
            let cancel = handler_cancel.clone();
            let ready = ready_tx.clone();
            Box::pin(async move {
                let _ = ready.send(());
                cancel.cancelled().await;
                serde_json::json!({"status":"panel_closed"})
            })
        });
        let _guard = start(&id, cancel.clone(), handler).unwrap();
        bind(&id, Some(std::process::id()));
        let pipe = ClientOptions::new().open(name(&id).unwrap()).unwrap();
        let (read, mut write) = tokio::io::split(pipe);
        write
            .write_all(b"{\"method\":\"browser_open\",\"arguments\":{}}\n")
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), ready_rx.recv())
            .await
            .unwrap()
            .unwrap();
        cancel.cancel();
        let reply = tokio::time::timeout(
            Duration::from_secs(5),
            BufReader::new(read).lines().next_line(),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&reply).unwrap()["status"],
            "panel_closed"
        );
    }
    #[tokio::test]
    async fn only_registered_session_receives_status_and_drop_clears_it() {
        let id = format!("test-{}", uuid::Uuid::new_v4());
        let handler: Handler =
            Arc::new(|_| Box::pin(async { serde_json::json!({"status":"unavailable"}) }));
        let guard = start(&id, CancellationToken::new(), handler).unwrap();
        let pipe_name = name(&id).unwrap();
        let exe = std::env::current_exe().unwrap();
        assert_eq!(
            crate::momso_mcp::acp_config(&exe, &id)["args"][2],
            pipe_name
        );
        assert_eq!(
            crate::momso_mcp::claude_config(&exe, &id)["mcpServers"][crate::momso_mcp::NAME]
                ["args"][2],
            pipe_name
        );
        assert_eq!(
            crate::momso_mcp::acp_config(&exe, "another-chat")["args"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(crate::momso_mcp::codex_config(&exe, &id).unwrap()[1].contains("momso-setup-"));
        bind(&id, Some(std::process::id()));
        let pipe = ClientOptions::new().open(name(&id).unwrap()).unwrap();
        let (r, mut w) = tokio::io::split(pipe);
        w.write_all(b"{\"method\":\"status\"}\n").await.unwrap();
        let mut lines = BufReader::new(r).lines();
        let reply = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&reply).unwrap()["status"],
            "unavailable"
        );
        drop(guard);
        assert!(name(&id).is_none());
    }

    #[tokio::test]
    async fn authenticated_mcp_forwards_only_valid_nonsecret_requests() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let handler: Handler = Arc::new(move |value| {
            let seen = seen.clone();
            Box::pin(async move {
                seen.fetch_add(1, Ordering::SeqCst);
                match value["method"].as_str() {
                    Some("github_connect") => {
                        assert_eq!(value, serde_json::json!({"method":"github_connect"}));
                        serde_json::json!({"status":"denied"})
                    }
                    Some("object_store_r2_connect") => {
                        assert_eq!(
                            value,
                            serde_json::json!({"method":"object_store_r2_connect"})
                        );
                        serde_json::json!({"status":"denied"})
                    }
                    Some("import_conversations") => {
                        assert_eq!(
                            value,
                            serde_json::json!({"method":"import_conversations","provider":"claude"})
                        );
                        serde_json::json!({"status":"success","imported":2,"duplicate":1,"contents":"private conversation","path":"private path"})
                    }
                    Some("browser_read") => {
                        serde_json::json!({"status":"page","origin":"https://example.test","title":"수업","text":"x".repeat(8000),"elements":[{"id":"a","role":"button","name":"게시","kind":"normal"}]})
                    }
                    Some("browser_navigate") => {
                        serde_json::json!({"status":"blocked","blockedHost":"outside.example","private":"never return"})
                    }
                    Some("browser_request_host") => {
                        assert_eq!(value["arguments"]["host"], "outside.example");
                        serde_json::json!({"status":"approved"})
                    }
                    Some("yoga_term_add") => {
                        assert_eq!(
                            value,
                            serde_json::json!({"method":"yoga_term_add","term":"가상 표기","baseId":"yoga-036"})
                        );
                        serde_json::json!({"status":"denied"})
                    }
                    Some("yoga_term_delete") => {
                        assert_eq!(value.as_object().unwrap().len(), 2);
                        serde_json::json!({"status":"denied"})
                    }
                    _ => panic!("unexpected setup request"),
                }
            })
        });
        let id = format!("test-{}", uuid::Uuid::new_v4());
        let _guard = start(&id, CancellationToken::new(), handler).unwrap();
        bind(&id, Some(std::process::id()));
        let name = name(&id).unwrap();
        tokio::task::spawn_blocking(move || {
            let file = std::fs::OpenOptions::new().read(true).write(true).open(name).unwrap();
            let mut client = crate::momso_mcp::test_bridge(file);
            let mut ready = true;
            let secret = "fake-secret-not-forwarded";
            let bad = crate::momso_mcp::test_response(&serde_json::json!({"id":1,"method":"tools/call","params":{"name":"momso_github_connect","arguments":{"secret":secret}}}), &mut ready, &mut client).unwrap();
            assert!(!bad.to_string().contains(secret));
            let bad_storage = crate::momso_mcp::test_response(&serde_json::json!({"id":3,"method":"tools/call","params":{"name":"momso_object_store_r2_connect","arguments":{"account":"0123456789abcdef0123456789abcdef","secret":secret}}}), &mut ready, &mut client).unwrap();
            assert!(!bad_storage.to_string().contains(secret));
            let r2 = crate::momso_mcp::test_response(&serde_json::json!({"id":4,"method":"tools/call","params":{"name":"momso_object_store_r2_connect","arguments":{}}}), &mut ready, &mut client).unwrap();
            assert!(r2.to_string().contains("denied"));
            let reply = crate::momso_mcp::test_response(&serde_json::json!({"id":2,"method":"tools/call","params":{"name":"momso_github_connect","arguments":{}}}), &mut ready, &mut client).unwrap();
            assert!(reply.to_string().contains("denied"));
            let invalid = crate::momso_mcp::test_response(&serde_json::json!({"id":5,"method":"tools/call","params":{"name":"momso_import_conversations","arguments":{"provider":"claude","path":"private path"}}}), &mut ready, &mut client).unwrap();
            assert!(invalid.get("error").is_some());
            let imported = crate::momso_mcp::test_response(&serde_json::json!({"id":6,"method":"tools/call","params":{"name":"momso_import_conversations","arguments":{"provider":"claude"}}}), &mut ready, &mut client).unwrap();
            let output = imported.to_string();
            assert!(output.contains("imported"));
            assert!(!output.contains("private conversation") && !output.contains("private path"));
            let widened = crate::momso_mcp::test_response(&serde_json::json!({"id":7,"method":"tools/call","params":{"name":"browser_open","arguments":{"origins":["https://evil.test"]}}}), &mut ready, &mut client).unwrap();
            assert!(widened.get("error").is_some());
            let page = crate::momso_mcp::test_response(&serde_json::json!({"id":8,"method":"tools/call","params":{"name":"browser_read","arguments":{}}}), &mut ready, &mut client).unwrap();
            let body = page["result"]["content"][0]["text"].as_str().unwrap();
            assert!(body.contains("untrusted_page_data") && body.contains("https://example.test") && body.len() > 8000);
            let blocked = crate::momso_mcp::test_response(&serde_json::json!({"id":11,"method":"tools/call","params":{"name":"browser_navigate","arguments":{"url":"https://outside.example/"}}}), &mut ready, &mut client).unwrap();
            let body = blocked["result"]["content"][0]["text"].as_str().unwrap();
            assert!(body.contains("outside.example") && !body.contains("never return"));
            let host = crate::momso_mcp::test_response(&serde_json::json!({"id":12,"method":"tools/call","params":{"name":"browser_request_host","arguments":{"host":"outside.example"}}}), &mut ready, &mut client).unwrap();
            assert!(host.to_string().contains("approved"));
            let bad_term = crate::momso_mcp::test_response(&serde_json::json!({"id":9,"method":"tools/call","params":{"name":"momso_yoga_term_add","arguments":{"term":"가상 표기","confirmedByUser":true}}}), &mut ready, &mut client).unwrap();
            assert!(bad_term.get("error").is_some());
            let deletion = crate::momso_mcp::test_response(&serde_json::json!({"id":10,"method":"tools/call","params":{"name":"momso_yoga_term_delete","arguments":{"id":"user-fake"}}}), &mut ready, &mut client).unwrap();
            assert!(deletion.to_string().contains("denied"));
            let term = crate::momso_mcp::test_response(&serde_json::json!({"id":10,"method":"tools/call","params":{"name":"momso_yoga_term_add","arguments":{"term":"가상 표기","baseId":"yoga-036"}}}), &mut ready, &mut client).unwrap();
            assert_eq!(term["result"]["content"][0]["text"], "{\"status\":\"denied\"}");
        }).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 8);
    }
}
