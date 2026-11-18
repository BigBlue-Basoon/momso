//! Guarded CDP worker for a panel. It is not exposed as an MCP endpoint.
use crate::panel_cdp::{PanelEvents, PanelTransport};
use crate::panel_navigation::NavigationLease;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::Duration;

enum Command {
    Call {
        method: String,
        params: Value,
        reply: mpsc::Sender<Result<Value, &'static str>>,
    },
    AllowOrigin {
        origin: String,
        reply: mpsc::Sender<Result<(), &'static str>>,
    },
    Stop,
}

enum Event {
    Message(&'static str, Value),
    Failed,
}

/// Request transport and browser event state. It owns the active navigation lease.
pub struct PanelWorker {
    commands: mpsc::Sender<Command>,
    worker: Option<std::thread::JoinHandle<()>>,
    blocked: Arc<Mutex<Option<String>>>,
    navigation: Arc<AtomicU64>,
    webauthn_pending: Arc<AtomicBool>,
    uninspectable_frame: Arc<AtomicBool>,
}

impl PanelWorker {
    pub fn start(app: tauri::AppHandle, origins: &[String]) -> Result<Self, &'static str> {
        let lease = crate::panel_browser::navigation_gate().activate(origins)?;
        let transport = PanelTransport::bind(&app)?;
        let (commands, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let blocked = Arc::new(Mutex::new(None));
        let navigation = Arc::new(AtomicU64::new(0));
        let webauthn_pending = Arc::new(AtomicBool::new(false));
        let uninspectable_frame = Arc::new(AtomicBool::new(false));
        let worker_blocked = blocked.clone();
        let worker_navigation = navigation.clone();
        let worker_webauthn = webauthn_pending.clone();
        let worker_opaque = uninspectable_frame.clone();
        let worker = std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => {
                    let _ = ready_tx.send(Err("unavailable"));
                    return;
                }
            };
            runtime.block_on(run(
                app,
                transport,
                lease,
                rx,
                ready_tx,
                worker_blocked,
                (worker_navigation, worker_webauthn, worker_opaque),
            ));
        });
        if !matches!(ready_rx.recv_timeout(Duration::from_secs(20)), Ok(Ok(()))) {
            let _ = commands.send(Command::Stop);
            let _ = worker.join();
            return Err("unavailable");
        }
        Ok(Self {
            commands,
            worker: Some(worker),
            blocked,
            navigation,
            webauthn_pending,
            uninspectable_frame,
        })
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value, &'static str> {
        let (reply, result) = mpsc::channel();
        self.commands
            .send(Command::Call {
                method: method.into(),
                params,
                reply,
            })
            .map_err(|_| "unavailable")?;
        match result.recv_timeout(Duration::from_secs(15)) {
            Ok(result) => result,
            Err(_) => {
                let _ = self.commands.send(Command::Stop);
                Err("unavailable")
            }
        }
    }

    /// This is host-side only; the caller must have already obtained user approval.
    pub fn allow_origin(&self, origin: &str) -> Result<(), &'static str> {
        let (reply, result) = mpsc::channel();
        self.commands
            .send(Command::AllowOrigin {
                origin: origin.into(),
                reply,
            })
            .map_err(|_| "unavailable")?;
        result
            .recv_timeout(Duration::from_secs(15))
            .map_err(|_| "unavailable")?
    }

    pub fn blocked(&self) -> Option<String> {
        let gate_blocked = crate::panel_browser::navigation_gate().blocked();
        if gate_blocked
            .as_deref()
            .is_some_and(crate::browser_runtime::google_login_target)
        {
            return gate_blocked;
        }
        self.blocked
            .lock()
            .ok()
            .and_then(|v| v.clone())
            .or(gate_blocked)
    }
    pub fn clear_blocked(&self) {
        if let Ok(mut blocked) = self.blocked.lock() {
            *blocked = None;
        }
        crate::panel_browser::navigation_gate().clear_blocked();
    }
    pub fn navigation(&self) -> u64 {
        self.navigation.load(Ordering::Acquire)
    }
    pub fn security_flags(&self) -> (Arc<AtomicBool>, Arc<AtomicBool>) {
        (
            self.webauthn_pending.clone(),
            self.uninspectable_frame.clone(),
        )
    }
    pub fn is_active(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
    }
}

impl Drop for PanelWorker {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

async fn forward_events(
    mut events: PanelEvents,
    name: &'static str,
    tx: tokio::sync::mpsc::Sender<Event>,
) {
    while let Some(event) = events.recv().await {
        if tx.send(Event::Message(name, event)).await.is_err() {
            return;
        }
    }
    let _ = tx.send(Event::Failed).await;
}

async fn initialize(
    transport: &PanelTransport,
) -> Result<
    (
        tokio::sync::mpsc::Receiver<Event>,
        Vec<tokio::task::JoinHandle<()>>,
    ),
    &'static str,
> {
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let mut tasks = Vec::new();
    for name in [
        "Fetch.requestPaused",
        "Page.frameAttached",
        "Page.frameDetached",
        "Page.frameNavigated",
        "Target.attachedToTarget",
        "Target.detachedFromTarget",
        "Runtime.bindingCalled",
    ] {
        let stream = transport.subscribe(name).await?;
        tasks.push(tokio::spawn(forward_events(stream, name, tx.clone())));
    }
    for (method, params) in [
        ("Page.enable", json!({})),
        ("Runtime.enable", json!({})),
        (
            "Runtime.addBinding",
            json!({"name":crate::browser_driver::WEBAUTHN_BINDING}),
        ),
        (
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source":crate::browser_driver::WEBAUTHN_WATCHER,"runImmediately":true}),
        ),
        ("Fetch.enable", json!({"patterns":[{"urlPattern":"*"}]})),
        (
            "Target.setAutoAttach",
            json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true}),
        ),
    ] {
        transport.call(method, params).await?;
    }
    Ok((rx, tasks))
}

async fn run(
    app: tauri::AppHandle,
    transport: PanelTransport,
    lease: NavigationLease,
    commands: mpsc::Receiver<Command>,
    ready: mpsc::Sender<Result<(), &'static str>>,
    blocked: Arc<Mutex<Option<String>>>,
    signals: (Arc<AtomicU64>, Arc<AtomicBool>, Arc<AtomicBool>),
) {
    use tauri::Manager;
    let (navigation, webauthn_pending, uninspectable_frame) = signals;
    let (mut events, tasks) =
        match tokio::time::timeout(Duration::from_secs(15), initialize(&transport)).await {
            Ok(Ok(value)) => value,
            _ => {
                let _ = tokio::time::timeout(
                    Duration::from_secs(1),
                    transport.call("Fetch.disable", json!({})),
                )
                .await;
                let _ = ready.send(Err("unavailable"));
                return;
            }
        };
    if ready.send(Ok(())).is_err() {
        return;
    }
    let transport = Arc::new(transport);
    let mut ticking = tokio::time::interval(Duration::from_millis(10));
    let mut frames = std::collections::HashSet::new();
    let mut opaque_sessions = std::collections::HashSet::new();
    let mut failed = false;
    loop {
        tokio::select! {
            _ = ticking.tick() => match commands.try_recv() {
                Ok(Command::Stop) | Err(mpsc::TryRecvError::Disconnected) => break,
                Ok(Command::Call { method, params, reply }) => {
                    let transport = transport.clone();
                    tokio::spawn(async move { let _ = reply.send(transport.call(&method, params).await); });
                }
                Ok(Command::AllowOrigin { origin, reply }) => { let _ = reply.send(lease.allow_origin(&origin)); }
                Err(mpsc::TryRecvError::Empty) => {}
            },
            event = events.recv() => match event {
                Some(Event::Message(name, event)) => {
                    let params = &event["params"];
                    let session = event.get("sessionId").and_then(Value::as_str);
                    match name {
                        "Page.frameAttached" => { if let Some(id) = params.get("frameId").and_then(Value::as_str) { frames.insert(id.to_owned()); } }
                        "Page.frameDetached" => { if let Some(id) = params.get("frameId").and_then(Value::as_str) { frames.remove(id); } }
                        "Page.frameNavigated" if params.pointer("/frame/parentId").is_none() && session.is_none() => { webauthn_pending.store(false, Ordering::Release); navigation.fetch_add(1, Ordering::AcqRel); }
                        "Runtime.bindingCalled" if params.get("name").and_then(Value::as_str) == Some(crate::browser_driver::WEBAUTHN_BINDING) => { webauthn_pending.store(true, Ordering::Release); }
                        "Target.detachedFromTarget" => { if let Some(sid) = params.get("sessionId").and_then(Value::as_str) { opaque_sessions.remove(sid); uninspectable_frame.store(!opaque_sessions.is_empty(), Ordering::Release); } }
                        "Target.attachedToTarget" => {
                            let Some(sid) = params.get("sessionId").and_then(Value::as_str) else { failed = true; break; };
                            let kind = params.pointer("/targetInfo/type").and_then(Value::as_str).unwrap_or("");
                            if kind == "page" {
                                let Some(target) = params.pointer("/targetInfo/targetId").and_then(Value::as_str) else { failed = true; break; };
                                if let Ok(mut blocked) = blocked.lock() { *blocked = Some("new_window".into()); }
                                if transport.call("Target.closeTarget", json!({"targetId":target})).await.is_err() { failed = true; break; }
                            } else {
                                if kind == "iframe" { opaque_sessions.insert(sid.to_owned()); uninspectable_frame.store(true, Ordering::Release); }
                                for (method, arguments) in [
                                    ("Target.setAutoAttach", json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true})),
                                    ("Fetch.enable", json!({"patterns":[{"urlPattern":"*"}]})),
                                    ("Runtime.runIfWaitingForDebugger", json!({})),
                                ] {
                                    if transport.call_in_session(Some(sid), method, arguments).await.is_err() { failed = true; break; }
                                }
                                if failed { break; }
                            }
                        }
                        "Fetch.requestPaused" => {
                            let Some(id) = params.get("requestId").and_then(Value::as_str).map(str::to_owned) else { failed = true; break; };
                            let url = params.pointer("/request/url").and_then(Value::as_str).unwrap_or("");
                            let permitted = crate::panel_browser::navigation_gate().permits_request(url);
                            if !permitted && params.get("resourceType").and_then(Value::as_str) == Some("Document") && !params.get("frameId").and_then(Value::as_str).is_some_and(|frame| frames.contains(frame)) {
                                if let Ok(mut blocked) = blocked.lock() { *blocked = Some(if crate::browser_runtime::google_login_target(url) { url.to_owned() } else { reqwest::Url::parse(url).ok().map(|url| url.origin().ascii_serialization()).unwrap_or_default() }); }
                            }
                            let method = if permitted { "Fetch.continueRequest" } else { "Fetch.failRequest" };
                            let args = if permitted { json!({"requestId":id}) } else { json!({"requestId":id,"errorReason":"BlockedByClient"}) };
                            if transport.call_in_session(session, method, args).await.is_err() { failed = true; break; }
                        }
                        _ => {}
                    }
                }
                _ => { failed = true; break; }
            }
        }
    }
    if failed {
        if let Some(view) = app.get_webview("panel-browser") {
            let _ = view.close();
        }
    } else {
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            transport.call("Fetch.disable", json!({})),
        )
        .await;
    }
    for task in tasks {
        task.abort();
    }
}
