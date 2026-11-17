//! Per-chat host-owned browser sessions. Browser content is untrusted data.
use crate::{
    approval::ApprovalRequest,
    browser_driver::{self, DriverError, PageState, Session},
    browser_gateway::Policy,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

pub(crate) type Notify = Arc<dyn Fn(&str, Value) + Send + Sync>;
pub(crate) type Ask = Arc<
    dyn Fn(ApprovalRequest, CancellationToken) -> Pin<Box<dyn Future<Output = bool> + Send>>
        + Send
        + Sync,
>;
pub(crate) struct RequestContext<'a> {
    pub chat_id: &'a str,
    pub provider: &'a str,
    pub skill: Option<&'a str>,
    pub selected: &'a Result<Option<Policy>, &'static str>,
    pub root: PathBuf,
    pub visible: bool,
    pub cancel: CancellationToken,
    pub notify: Notify,
    pub ask: Ask,
}
struct Entry {
    policy: Policy,
    skill: String,
    action: String,
    grant: Mutex<Option<Grant>>,
    serial: AsyncMutex<()>,
    session: Arc<Mutex<Option<Session>>>,
}
#[derive(Clone)]
struct Grant {
    origin: String,
    action: String,
    description: String,
    element_id: String,
    element_name: String,
}
impl Grant {
    fn permits(&self, origin: &str, method: &str, id: &str, name: &str) -> bool {
        self.origin == origin
            && (method != "browser_click"
                || (id == self.element_id
                    && name == self.element_name
                    && name.contains(&self.action)))
    }
}
struct ScopeRequest<'a> {
    chat_id: &'a str,
    provider: &'a str,
    origin: &'a str,
    skill: &'a str,
    action: &'a str,
    description: &'a str,
    element_id: &'a str,
    element_name: &'a str,
}
async fn request_scope(
    policy: &Policy,
    scope: ScopeRequest<'_>,
    ask: &Ask,
    cancel: CancellationToken,
) -> Result<Grant, &'static str> {
    if !policy.allows_action(scope.action) {
        return Err("action_unavailable");
    }
    let request = crate::approval::browser_scope_request(
        scope.chat_id,
        scope.provider,
        scope.origin,
        scope.skill,
        scope.action,
        &format!("{} · 대상: {}", scope.description, scope.element_name),
    );
    if !ask(request, cancel.clone()).await || cancel.is_cancelled() {
        return Err("denied");
    }
    Ok(Grant {
        origin: scope.origin.to_owned(),
        action: scope.action.to_owned(),
        description: scope.description.to_owned(),
        element_id: scope.element_id.to_owned(),
        element_name: scope.element_name.to_owned(),
    })
}
#[derive(Default)]
pub(crate) struct BrowserManager {
    entries: AsyncMutex<HashMap<String, Arc<Entry>>>,
}

fn limited(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}
fn page_value(page: PageState, known: &[&str]) -> Value {
    let clean = |value: &str, limit| {
        limited(
            &crate::chat_archive_redact::redact_with_known(value, known).text,
            limit,
        )
    };
    let elements: Vec<_> = page.elements.into_iter().take(40).map(|e| json!({"id":limited(&e.id,128),"role":clean(&e.role,24),"name":clean(&e.name,128),"kind":e.kind})).collect();
    json!({"status":"page","origin":page.origin,"title":clean(&page.title,256),"text":clean(&page.text,4000),"elements":elements,"dataProvenance":page.data_provenance})
}
fn human(reason: &'static str, chat_id: &str, notify: &Notify) -> Value {
    let reason = match reason {
        "login" | "password" | "one_time_code" | "captcha" | "verification" => reason,
        _ => "verification",
    };
    notify(
        "browser-human-required",
        json!({"chatId":chat_id,"reason":reason}),
    );
    json!({"status":"human_required"})
}
fn safe_page(page: PageState, chat_id: &str, notify: &Notify, known: &[&str]) -> Value {
    if page.elements.iter().any(|e| e.kind == "password") {
        return human("password", chat_id, notify);
    }
    if page.elements.iter().any(|e| e.kind == "one_time_code") {
        return human("one_time_code", chat_id, notify);
    }
    page_value(page, known)
}
fn error(err: DriverError, chat_id: &str, notify: &Notify) -> Value {
    match err {
        DriverError::Blocked(_) => json!({"status":"blocked"}),
        DriverError::HumanRequired(reason) => human(reason, chat_id, notify),
        DriverError::Refused("password") => human("password", chat_id, notify),
        DriverError::Refused("one_time_code") => human("one_time_code", chat_id, notify),
        DriverError::Refused("cancelled") => json!({"status":"denied"}),
        DriverError::TargetChanged => json!({"status":"target_changed"}),
        _ => json!({"status":"unavailable"}),
    }
}
fn valid(method: &str, args: &Value) -> bool {
    let Some(a) = args.as_object() else {
        return false;
    };
    let id = || {
        a.get("elementId")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty() && s.len() <= 256)
    };
    match method {
        "browser_open" | "browser_read" | "browser_close" => a.is_empty(),
        "browser_navigate" => {
            a.len() == 1
                && a.get("url")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty() && s.len() <= 4096)
        }
        "browser_click" => a.len() == 1 && id(),
        "browser_request_action" => {
            a.len() == 3
                && ["action", "description"].iter().all(|key| {
                    a.get(*key).and_then(Value::as_str).is_some_and(|s| {
                        !s.trim().is_empty()
                            && s.chars().count() <= if *key == "action" { 64 } else { 200 }
                            && !s.chars().any(char::is_control)
                    })
                })
                && id()
        }
        "browser_fill" => {
            a.len() == 2
                && id()
                && a.get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.len() <= 16384)
        }
        _ => false,
    }
}
async fn call<T: Send + 'static>(
    entry: &Arc<Entry>,
    f: impl FnOnce(&mut Session) -> Result<T, DriverError> + Send + 'static,
) -> Result<T, DriverError> {
    let session = entry.session.clone();
    tokio::task::spawn_blocking(move || {
        let mut guard = session.lock().map_err(|_| DriverError::Unavailable)?;
        f(guard.as_mut().ok_or(DriverError::Unavailable)?)
    })
    .await
    .map_err(|_| DriverError::Unavailable)?
}

impl BrowserManager {
    pub(crate) async fn is_open(&self, chat_id: &str) -> bool {
        let entry = self.entries.lock().await.get(chat_id).cloned();
        let Some(entry) = entry else {
            return false;
        };
        let session = entry.session.clone();
        let active = tokio::task::spawn_blocking(move || {
            session
                .lock()
                .ok()
                .and_then(|s| s.as_ref().map(Session::is_active))
                .unwrap_or(false)
        })
        .await
        .unwrap_or(false);
        if !active {
            let mut entries = self.entries.lock().await;
            if entries
                .get(chat_id)
                .is_some_and(|current| Arc::ptr_eq(current, &entry))
            {
                entries.remove(chat_id);
            }
        }
        active
    }
    pub(crate) async fn close(&self, chat_id: &str, notify: &Notify) {
        let entry = self.entries.lock().await.remove(chat_id);
        if let Some(entry) = entry {
            let _serial = entry.serial.lock().await;
            let session = entry.session.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(mut guard) = session.lock() {
                    if let Some(session) = guard.take() {
                        session.close();
                    }
                }
            })
            .await;
            notify(
                "browser-session-updated",
                json!({"chatId":chat_id,"open":false}),
            );
        }
    }
    pub(crate) async fn dispatch(&self, context: RequestContext<'_>, input: Value) -> Value {
        let RequestContext {
            chat_id,
            provider,
            skill,
            selected,
            root,
            visible,
            cancel,
            notify,
            ask,
        } = context;
        let Some(method) = input.get("method").and_then(Value::as_str) else {
            return json!({"status":"unavailable"});
        };
        let args = input.get("arguments").cloned().unwrap_or(Value::Null);
        if input.as_object().is_none_or(|o| o.len() != 2) || !valid(method, &args) {
            return json!({"status":"unavailable"});
        }
        if method == "browser_close" {
            self.close(chat_id, &notify).await;
            return json!({"status":"closed"});
        }
        if cancel.is_cancelled() {
            return json!({"status":"denied"});
        }
        let existing = self.entries.lock().await.get(chat_id).cloned();
        if let Err("browser_policy_modified") = selected {
            return json!({"status":"policy_modified"});
        }
        if selected.is_err() {
            return json!({"status":"policy_unavailable"});
        }
        if let (Some(entry), Ok(Some(policy))) = (&existing, selected) {
            if &entry.policy != policy || skill != Some(entry.skill.as_str()) {
                return json!({"status":"policy_unavailable"});
            }
        }
        if method == "browser_open" {
            if existing.is_some() {
                return json!({"status":"ok"});
            }
            let (Some(skill), Ok(Some(policy))) = (skill, selected) else {
                return json!({"status":"policy_unavailable"});
            };
            let policy = policy.clone();
            let origins = policy.origins().to_vec();
            let token = cancel.clone();
            let profile = root
                .join("browser-profiles")
                .join(format!("{:x}", Sha256::digest(chat_id.as_bytes())));
            let result = tokio::task::spawn_blocking(move || {
                let session = browser_driver::launch(&profile, &origins, visible)?;
                if token.is_cancelled() {
                    return Err(DriverError::Unavailable);
                }
                Ok(session)
            })
            .await;
            let session = match result {
                Ok(Ok(session)) => session,
                _ => return json!({"status":"driver_unavailable"}),
            };
            if cancel.is_cancelled() {
                drop(session);
                return json!({"status":"denied"});
            }
            let action = crate::skills::BUNDLED_SKILLS
                .iter()
                .find(|(name, _)| *name == skill)
                .and_then(|(_, text)| crate::skills::parse(skill, text).ok())
                .map(|(head, _)| head.title)
                .unwrap_or_else(|| "브라우저 화면".into());
            let entry = Arc::new(Entry {
                policy,
                skill: skill.to_owned(),
                action,
                grant: Mutex::new(None),
                serial: AsyncMutex::new(()),
                session: Arc::new(Mutex::new(Some(session))),
            });
            let mut entries = self.entries.lock().await;
            if cancel.is_cancelled() {
                return json!({"status":"denied"});
            }
            if entries.contains_key(chat_id) {
                return json!({"status":"unavailable"});
            }
            entries.insert(chat_id.to_owned(), entry);
            drop(entries);
            notify(
                "browser-session-updated",
                json!({"chatId":chat_id,"open":true}),
            );
            return json!({"status":"ok"});
        }
        let Some(entry) = existing else {
            return json!({"status":"policy_unavailable"});
        };
        let _serial = entry.serial.lock().await;
        if cancel.is_cancelled() {
            return json!({"status":"denied"});
        }
        if method == "browser_navigate" {
            if !entry.policy.allows_read_only(method) {
                return json!({"status":"read_only"});
            }
            let url = args["url"].as_str().unwrap();
            if !entry.policy.allows(url) {
                return json!({"status":"blocked"});
            }
            let data_root = root.clone();
            let known = match tokio::task::spawn_blocking(move || {
                crate::chat_archive_store::known_values(&data_root, &data_root.join("momso.db"))
            })
            .await
            {
                Ok(Ok(known)) => known,
                _ => return json!({"status":"unavailable"}),
            };
            let refs: Vec<&str> = known.iter().map(String::as_str).collect();
            let url = url.to_owned();
            return match call(&entry, move |s| s.navigate(&url)).await {
                Ok(page) if entry.policy.allows(&page.origin) => {
                    safe_page(page, chat_id, &notify, &refs)
                }
                Ok(_) => json!({"status":"blocked"}),
                Err(e) => error(e, chat_id, &notify),
            };
        }
        let page = match call(&entry, |s| s.read()).await {
            Ok(page) if entry.policy.allows(&page.origin) => page,
            Ok(_) => return json!({"status":"blocked"}),
            Err(e) => return error(e, chat_id, &notify),
        };
        if page.elements.iter().any(|e| e.kind == "password") {
            return human("password", chat_id, &notify);
        }
        if page.elements.iter().any(|e| e.kind == "one_time_code") {
            return human("one_time_code", chat_id, &notify);
        }
        if method == "browser_read" {
            return if entry.policy.allows_read_only(method) {
                let data_root = root.clone();
                let known = match tokio::task::spawn_blocking(move || {
                    crate::chat_archive_store::known_values(&data_root, &data_root.join("momso.db"))
                })
                .await
                {
                    Ok(Ok(known)) => known,
                    _ => return json!({"status":"unavailable"}),
                };
                let refs: Vec<&str> = known.iter().map(String::as_str).collect();
                page_value(page, &refs)
            } else {
                json!({"status":"read_only"})
            };
        }
        if method == "browser_request_action" {
            let action = args["action"].as_str().unwrap();
            let description = args["description"].as_str().unwrap();
            let Some(target) = page.elements.iter().find(|element| {
                element.id == args["elementId"].as_str().unwrap()
                    && element.kind == "normal"
                    && element.name.contains(action)
            }) else {
                return json!({"status":"action_mismatch"});
            };
            match entry.grant.lock() {
                Ok(mut grant) => *grant = None,
                Err(_) => return json!({"status":"unavailable"}),
            }
            let approved = match request_scope(
                &entry.policy,
                ScopeRequest {
                    chat_id,
                    provider,
                    origin: &page.origin,
                    skill: &entry.action,
                    action,
                    description,
                    element_id: &target.id,
                    element_name: &target.name,
                },
                &ask,
                cancel.clone(),
            )
            .await
            {
                Ok(grant) => grant,
                Err(status) => return json!({"status":status}),
            };
            let Ok(mut grant) = entry.grant.lock() else {
                return json!({"status":"unavailable"});
            };
            *grant = Some(approved);
            return json!({"status":"approved"});
        }
        let grant = match entry.grant.lock() {
            Ok(grant) => grant.clone(),
            Err(_) => return json!({"status":"unavailable"}),
        };
        let Some(grant) = grant.filter(|grant| grant.origin == page.origin) else {
            return json!({"status":"read_only"});
        };
        let id = args["elementId"].as_str().unwrap();
        let Some(element) = page
            .elements
            .iter()
            .find(|e| e.id == id && e.kind == "normal")
        else {
            return json!({"status":"target_changed"});
        };
        let expected = element.name.clone();
        if !grant.permits(&page.origin, method, id, &expected) {
            return json!({"status":"action_mismatch"});
        }
        let id = id.to_owned();
        let action = if method == "browser_fill" {
            format!(
                "{} · {} · 입력: {}",
                entry.action,
                grant.description,
                args["text"].as_str().unwrap()
            )
        } else {
            format!("{} · {}", entry.action, grant.description)
        };
        let request = crate::approval::browser_request(
            chat_id,
            provider,
            &page.origin,
            &action,
            entry.policy.effect(&expected),
            &expected,
        );
        if !ask(request, cancel.clone()).await || cancel.is_cancelled() {
            return json!({"status":"denied"});
        }
        let origin = page.origin.clone();
        let operation_cancel = cancel.clone();
        let result = if method == "browser_click" {
            call(&entry, move |s| {
                if operation_cancel.is_cancelled() {
                    Err(DriverError::Refused("cancelled"))
                } else {
                    s.click_checked(&id, &expected, Some(&origin))
                }
            })
            .await
        } else {
            let text = args["text"].as_str().unwrap().to_owned();
            call(&entry, move |s| {
                if operation_cancel.is_cancelled() {
                    Err(DriverError::Refused("cancelled"))
                } else {
                    s.fill_checked(&id, &text, Some(&expected), Some(&origin))
                }
            })
            .await
        };
        match result {
            Ok(()) => {
                // A completed action cannot authorize another final click.
                if method == "browser_click" {
                    if let Ok(mut grant) = entry.grant.lock() {
                        *grant = None;
                    }
                }
                json!({"status":"ok"})
            }
            Err(e) => error(e, chat_id, &notify),
        }
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn browser_page_hides_known_key_from_ai_response() {
        let secret = "fixture-opaque-known-secret";
        let page = PageState {
            origin: "https://example.test".into(),
            title: format!("title {secret}"),
            text: format!("body {secret}"),
            elements: vec![crate::browser_driver::Element {
                id: "fixture-id".into(),
                role: "button".into(),
                name: format!("button {secret}"),
                kind: "normal",
            }],
            data_provenance: "untrusted_page_data",
        };
        let result = serde_json::to_string(&page_value(page, &[secret])).unwrap();
        assert!(!result.contains(secret));
        assert!(result.contains("fixture-id"));
    }

    #[tokio::test]
    async fn scope_approval_is_policy_bound_and_denial_keeps_read_only() {
        let policy = Policy::test_loopback("http://127.0.0.1:1234".into());
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let deny: Ask = Arc::new(move |request, _| {
            assert_eq!(request.kind, "browser_action_scope");
            assert_eq!(request.targets[0].path, "http://127.0.0.1:1234");
            counted.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { false })
        });
        let scope = || ScopeRequest {
            chat_id: "chat",
            provider: "claude",
            origin: "http://127.0.0.1:1234",
            skill: "fixture",
            action: "게시",
            description: "내일 수업 게시",
            element_id: "button-1",
            element_name: "게시",
        };
        assert!(matches!(
            request_scope(&policy, scope(), &deny, CancellationToken::new()).await,
            Err("denied")
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let allow: Ask = Arc::new(|_, _| Box::pin(async { true }));
        let grant = request_scope(&policy, scope(), &allow, CancellationToken::new())
            .await
            .unwrap();
        assert!(grant.permits("http://127.0.0.1:1234", "browser_click", "button-1", "게시"));
        assert!(!grant.permits("http://127.0.0.1:1234", "browser_click", "button-2", "게시"));
        assert!(!grant.permits("http://127.0.0.1:1234", "browser_click", "button-1", "삭제"));
        assert!(!grant.permits("https://other.example", "browser_click", "button-1", "게시"));
        let mut bad = scope();
        bad.action = "무제한";
        assert!(matches!(
            request_scope(&policy, bad, &allow, CancellationToken::new()).await,
            Err("action_unavailable")
        ));
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::approval::{ApprovalBroker, ApprovalChoice};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        thread,
        time::Duration,
    };
    use tokio::sync::mpsc;

    struct Site {
        url: String,
        posted: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }
    impl Site {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let posted = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let posts = posted.clone();
            let done = stop.clone();
            let worker = thread::spawn(move || {
                while !done.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                            let mut data = [0u8; 8192];
                            let n = stream.read(&mut data).unwrap_or(0);
                            let request = String::from_utf8_lossy(&data[..n]);
                            let body = if request.starts_with("POST /posted ") {
                                posts.fetch_add(1, Ordering::SeqCst);
                                "ok"
                            } else if request.starts_with("GET /login ") {
                                "<title>Login</title><input type='password'>"
                            } else {
                                "<title>Class</title><input placeholder='Lesson' oninput=\"fetch('/posted',{method:'POST'})\"><button onclick=\"fetch('/posted',{method:'POST'})\">게시</button>"
                            };
                            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
                            let _ = stream.write_all(response.as_bytes());
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                url,
                posted,
                stop,
                worker: Some(worker),
            }
        }
    }
    impl Drop for Site {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires a locally installed Edge or Chrome browser"]
    async fn edge_mcp_approval_human_handoff_policy_and_close() {
        for choice in [ApprovalChoice::AllowOnce, ApprovalChoice::Deny] {
            let site = Site::start();
            let other = Site::start();
            let build_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(4)
                .unwrap()
                .join("target");
            let temp = tempfile::Builder::new()
                .prefix("c19-browser-test-")
                .tempdir_in(build_root)
                .unwrap();
            let manager = Arc::new(BrowserManager::default());
            let worker_manager = manager.clone();
            let policy = Ok(Some(Policy::test_loopback(site.url.clone())));
            let broker = ApprovalBroker::default();
            let worker_broker = broker.clone();
            let (approved_tx, mut approved_rx) = mpsc::unbounded_channel::<String>();
            let observed = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
            let worker_observed = observed.clone();
            let continuation = Arc::new(AtomicBool::new(false));
            let worker_continuation = continuation.clone();
            let root = temp.path().to_path_buf();
            crate::storage::initialize(&root.join("momso.db"), &root.join("backups"), "fixture")
                .unwrap();
            let handler: crate::setup_bridge::Handler = Arc::new(move |input| {
                let manager = worker_manager.clone();
                let broker = worker_broker.clone();
                let tx = approved_tx.clone();
                let log = worker_observed.clone();
                let root = root.clone();
                let policy = policy.clone();
                let continuation = worker_continuation.clone();
                Box::pin(async move {
                    let notify: Notify = Arc::new(move |name, payload| {
                        log.lock().unwrap().push((name.to_owned(), payload))
                    });
                    let ask: Ask = Arc::new(move |request, token| {
                        let broker = broker.clone();
                        let tx = tx.clone();
                        Box::pin(async move {
                            matches!(
                                broker
                                    .request_with(
                                        request,
                                        token,
                                        |card| {
                                            let _ = tx.send(card.request_id.clone());
                                            true
                                        },
                                        |_| {}
                                    )
                                    .await,
                                Ok(ApprovalChoice::AllowOnce)
                            )
                        })
                    });
                    let continuing = continuation.load(Ordering::SeqCst);
                    let selected = if continuing { Ok(None) } else { policy };
                    let result = manager
                        .dispatch(
                            RequestContext {
                                chat_id: "chat",
                                provider: "claude",
                                skill: if continuing { None } else { Some("fixture") },
                                selected: &selected,
                                root,
                                visible: false,
                                cancel: CancellationToken::new(),
                                notify,
                                ask,
                            },
                            input,
                        )
                        .await;
                    if result["status"] == "human_required" {
                        continuation.store(true, Ordering::SeqCst);
                    }
                    result
                })
            });
            let chat_id = format!("test-{}", uuid::Uuid::new_v4());
            let guard =
                crate::setup_bridge::start(&chat_id, CancellationToken::new(), handler).unwrap();
            crate::setup_bridge::bind(&chat_id, Some(std::process::id()));
            let pipe = crate::setup_bridge::name(&chat_id).unwrap();
            let url = site.url.clone();
            let forbidden = other.url.clone();
            let cli = tokio::task::spawn_blocking(move || {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(pipe)
                    .unwrap();
                let mut client = crate::momso_mcp::test_bridge(file);
                let mut ready = true;
                let mut call = |name: &str, args: Value| {
                    let result=crate::momso_mcp::test_response(&json!({"id":1,"method":"tools/call","params":{"name":name,"arguments":args}}),&mut ready,&mut client).unwrap();
                    if result.get("error").is_some() {
                        return json!({"status":"invalid_parameters"});
                    }
                    serde_json::from_str::<Value>(
                        result["result"]["content"][0]["text"].as_str().unwrap(),
                    )
                    .unwrap()
                };
                assert_eq!(call("browser_open", json!({}))["status"], "ok");
                assert_eq!(
                    call("browser_navigate", json!({"url":format!("{url}/login")}))["status"],
                    "human_required"
                );
                assert_eq!(call("browser_open", json!({}))["status"], "ok");
                assert_eq!(
                    call("browser_navigate", json!({"url":forbidden}))["status"],
                    "blocked"
                );
                let page = call("browser_navigate", json!({"url":format!("{url}/work")}));
                assert_eq!(page["kind"], "untrusted_page_data");
                let input = page["elements"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["name"] == "Lesson")
                    .unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                assert_eq!(
                    call("browser_fill", json!({"elementId":input,"text":"sample"}))["status"],
                    "read_only"
                );
                let id = page["elements"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["name"] == "게시")
                    .unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                assert_eq!(
                    call(
                        "browser_request_action",
                        json!({"action":"게시","description":"가짜 수업 게시", "elementId":id})
                    )["status"],
                    "approved"
                );
                assert_eq!(
                    call("browser_fill", json!({"elementId":input,"text":"sample"}))["status"],
                    "denied"
                );
                assert_eq!(
                    call("browser_click", json!({"elementId":id,"approved":true}))["status"],
                    "invalid_parameters"
                );
                let result = call("browser_click", json!({"elementId":id}));
                assert_eq!(call("browser_close", json!({}))["status"], "closed");
                result
            });
            let scope_request = tokio::time::timeout(Duration::from_secs(45), approved_rx.recv())
                .await
                .unwrap()
                .unwrap();
            broker
                .respond(&scope_request, ApprovalChoice::AllowOnce)
                .await
                .unwrap();
            let fill_request = tokio::time::timeout(Duration::from_secs(20), approved_rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(site.posted.load(Ordering::SeqCst), 0);
            broker
                .respond(&fill_request, ApprovalChoice::Deny)
                .await
                .unwrap();
            let request = tokio::time::timeout(Duration::from_secs(20), approved_rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(site.posted.load(Ordering::SeqCst), 0);
            assert!(!cli.is_finished());
            broker.respond(&request, choice).await.unwrap();
            let result = tokio::time::timeout(Duration::from_secs(30), cli)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                result["status"],
                if choice == ApprovalChoice::AllowOnce {
                    "ok"
                } else {
                    "denied"
                }
            );
            assert_eq!(
                site.posted.load(Ordering::SeqCst),
                usize::from(choice == ApprovalChoice::AllowOnce)
            );
            assert_eq!(other.posted.load(Ordering::SeqCst), 0);
            assert!(!manager.is_open("chat").await);
            let events = observed.lock().unwrap();
            assert!(events.iter().any(
                |(name, value)| name == "browser-human-required" && value["reason"] == "login"
            ));
            assert!(events
                .iter()
                .any(|(name, value)| name == "browser-session-updated" && value["open"] == false));
            drop(events);
            drop(guard);
        }
    }
}
