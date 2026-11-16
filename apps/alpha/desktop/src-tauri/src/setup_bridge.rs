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
            let line = tokio::select! { _ = worker.cancel.cancelled() => break, v = lines.next_line() => match v { Ok(Some(v)) if v.len() <= 4096 => v, _ => break } };
            let result = match serde_json::from_str::<serde_json::Value>(&line) {
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
        }
    });
    Ok(Guard {
        chat_id: chat_id.to_owned(),
        entry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::windows::named_pipe::ClientOptions;
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
                            serde_json::json!({"method":"object_store_r2_connect", "account":"0123456789abcdef0123456789abcdef"})
                        );
                        serde_json::json!({"status":"cancelled"})
                    }
                    Some("import_conversations") => {
                        assert_eq!(value, serde_json::json!({"method":"import_conversations","provider":"claude"}));
                        serde_json::json!({"status":"success","imported":2,"duplicate":1,"contents":"private conversation","path":"private path"})
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
            let reply = crate::momso_mcp::test_response(&serde_json::json!({"id":2,"method":"tools/call","params":{"name":"momso_github_connect","arguments":{}}}), &mut ready, &mut client).unwrap();
            assert!(reply.to_string().contains("denied"));
            let cancelled = crate::momso_mcp::test_response(&serde_json::json!({"id":4,"method":"tools/call","params":{"name":"momso_object_store_r2_connect","arguments":{"account":"0123456789abcdef0123456789abcdef"}}}), &mut ready, &mut client).unwrap();
            assert_eq!(cancelled["result"]["content"][0]["text"], "{\"status\":\"cancelled\"}");
            assert!(!cancelled.to_string().contains(secret));
            let invalid = crate::momso_mcp::test_response(&serde_json::json!({"id":5,"method":"tools/call","params":{"name":"momso_import_conversations","arguments":{"provider":"claude","path":"private path"}}}), &mut ready, &mut client).unwrap();
            assert!(invalid.get("error").is_some());
            let imported = crate::momso_mcp::test_response(&serde_json::json!({"id":6,"method":"tools/call","params":{"name":"momso_import_conversations","arguments":{"provider":"claude"}}}), &mut ready, &mut client).unwrap();
            let output = imported.to_string();
            assert!(output.contains("imported"));
            assert!(!output.contains("private conversation") && !output.contains("private path"));
        }).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }
}
