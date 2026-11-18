//! In-process transport for the dedicated panel WebView2; no listener or debug port.
//! This transport is not an MCP tool and does not grant browser access to AI.
use serde_json::Value;
use tauri::Manager;

const PANEL_LABEL: &str = crate::panel_browser::LABEL;

fn lookup_panel<T>(get: impl FnOnce(&str) -> Option<T>) -> Result<T, &'static str> {
    get(PANEL_LABEL).ok_or("unavailable")
}

/// The handle is obtained only from the dedicated panel label, never from an input label.
pub struct PanelTransport {
    view: tauri::Webview,
}

/// A bounded event stream; dropping it removes the WebView2 event handler.
pub struct PanelEvents {
    view: tauri::Webview,
    event: String,
    token: i64,
    receiver: tokio::sync::mpsc::Receiver<Value>,
    overflow: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl PanelEvents {
    pub async fn recv(&mut self) -> Option<Value> {
        if self.overflow.load(std::sync::atomic::Ordering::Acquire) {
            return None;
        }
        let event = self.receiver.recv().await;
        if self.overflow.load(std::sync::atomic::Ordering::Acquire) {
            None
        } else {
            event
        }
    }
}

impl Drop for PanelEvents {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            use windows_core::HSTRING;
            let event = self.event.clone();
            let token = self.token;
            let _ = self.view.with_webview(move |platform| unsafe {
                if let Ok(core) = platform.controller().CoreWebView2() {
                    if let Ok(receiver) =
                        core.GetDevToolsProtocolEventReceiver(&HSTRING::from(event))
                    {
                        let _ = receiver.remove_DevToolsProtocolEventReceived(token);
                    }
                }
            });
        }
        #[cfg(not(windows))]
        let _ = (&self.view, &self.event, self.token);
    }
}

impl PanelTransport {
    /// Bind only the app-owned panel, and fail if it is not open.
    pub fn bind(app: &tauri::AppHandle) -> Result<Self, &'static str> {
        let view = lookup_panel(|label| app.get_webview(label))?;
        Ok(Self { view })
    }

    /// Dispatch on the WebView UI thread and receive the asynchronous CDP result.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, &'static str> {
        self.call_in_session(None, method, params).await
    }

    /// Call an attached target session when the WebView2 runtime supports it.
    pub async fn call_in_session(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, &'static str> {
        let session = session.map(str::to_owned);
        let method = method.to_owned();
        let params = serde_json::to_string(&params).map_err(|_| "unavailable")?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.view
            .with_webview(move |platform| {
                #[cfg(windows)]
                {
                    use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
                    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_11;
                    use windows_core::{Interface, HSTRING};
                    let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
                    let done = tx.clone();
                    let callback = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                        move |status, result| {
                            let result = status.map_err(|_| "unavailable").and_then(|_| {
                                serde_json::from_str(&result).map_err(|_| "unavailable")
                            });
                            if let Ok(mut sender) = done.lock() {
                                if let Some(sender) = sender.take() {
                                    let _ = sender.send(result);
                                }
                            }
                            Ok(())
                        },
                    ));
                    let started = unsafe {
                        platform.controller().CoreWebView2().and_then(|core| {
                            if let Some(session) = session {
                                core.cast::<ICoreWebView2_11>()?
                                    .CallDevToolsProtocolMethodForSession(
                                        &HSTRING::from(session),
                                        &HSTRING::from(method),
                                        &HSTRING::from(params),
                                        &callback,
                                    )
                            } else {
                                core.CallDevToolsProtocolMethod(
                                    &HSTRING::from(method),
                                    &HSTRING::from(params),
                                    &callback,
                                )
                            }
                        })
                    };
                    if started.is_err() {
                        if let Ok(mut sender) = tx.lock() {
                            if let Some(sender) = sender.take() {
                                let _ = sender.send(Err("unavailable"));
                            }
                        }
                    }
                }
                #[cfg(not(windows))]
                {
                    let _ = (platform, session, method, params);
                    let _ = tx.send(Err("unavailable"));
                }
            })
            .map_err(|_| "unavailable")?;
        tokio::time::timeout(std::time::Duration::from_secs(15), rx)
            .await
            .map_err(|_| "unavailable")?
            .map_err(|_| "unavailable")?
    }

    /// Subscribe to a CDP event on the panel; no network endpoint is opened.
    pub async fn subscribe(&self, event: &str) -> Result<PanelEvents, &'static str> {
        let event_name = event.to_owned();
        let requested = event_name.clone();
        let (sender, receiver) = tokio::sync::mpsc::channel(256);
        let overflow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let callback_overflow = overflow.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.view.with_webview(move |platform| {
            #[cfg(windows)]
            {
                use webview2_com::{DevToolsProtocolEventReceivedEventHandler, take_pwstr};
                use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2DevToolsProtocolEventReceivedEventArgs2;
                use windows_core::{HSTRING, Interface, PWSTR};
                let handler = DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                    let mut delivered = false;
                    if let Some(args) = args {
                        let mut raw = PWSTR::null();
                        if unsafe { args.ParameterObjectAsJson(&mut raw) }.is_ok() {
                            let text = take_pwstr(raw);
                            if text.len() <= 65536 {
                              if let Ok(params) = serde_json::from_str::<Value>(&text) {
                                let session = args.cast::<ICoreWebView2DevToolsProtocolEventReceivedEventArgs2>().ok().and_then(|args| {
                                    let mut raw = PWSTR::null();
                                    unsafe { args.SessionId(&mut raw) }.ok().map(|_| take_pwstr(raw))
                                }).filter(|s| !s.is_empty());
                                delivered = sender.try_send(serde_json::json!({"params":params,"sessionId":session})).is_ok();
                              }
                            }
                        }
                    }
                    if !delivered {
                        callback_overflow.store(true, std::sync::atomic::Ordering::Release);
                        let _ = sender.try_send(Value::Null);
                    }
                    Ok(())
                }));
                let result = unsafe { platform.controller().CoreWebView2().and_then(|core| core.GetDevToolsProtocolEventReceiver(&HSTRING::from(requested.as_str()))).and_then(|events| {
                    let mut token = 0;
                    events.add_DevToolsProtocolEventReceived(&handler, &mut token)?;
                    Ok(token)
                }) }.map_err(|_| "unavailable");
                if let Err(Ok(token)) = tx.send(result) {
                    unsafe { if let Ok(core) = platform.controller().CoreWebView2() { if let Ok(events) = core.GetDevToolsProtocolEventReceiver(&HSTRING::from(requested.as_str())) { let _ = events.remove_DevToolsProtocolEventReceived(token); } } }
                }
            }
            #[cfg(not(windows))]
            { let _ = (platform, requested, sender, callback_overflow); let _ = tx.send(Err("unavailable")); }
        }).map_err(|_| "unavailable")?;
        let token = tokio::time::timeout(std::time::Duration::from_secs(15), rx)
            .await
            .map_err(|_| "unavailable")?
            .map_err(|_| "unavailable")??;
        Ok(PanelEvents {
            view: self.view.clone(),
            event: event_name,
            token,
            receiver,
            overflow,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[cfg(windows)]
    pub(crate) fn process_tree(
        roots: &[u32],
    ) -> Result<std::collections::HashSet<u32>, &'static str> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        };
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err("unavailable");
        }
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut edges = Vec::new();
        let mut success = unsafe { Process32FirstW(snapshot, &mut entry) };
        if success == 0 {
            unsafe {
                CloseHandle(snapshot);
            }
            return Err("unavailable");
        }
        while success != 0 {
            edges.push((entry.th32ParentProcessID, entry.th32ProcessID));
            success = unsafe { Process32NextW(snapshot, &mut entry) };
        }
        unsafe {
            CloseHandle(snapshot);
        }
        let mut result: std::collections::HashSet<u32> = roots.iter().copied().collect();
        loop {
            let before = result.len();
            for (parent, child) in &edges {
                if result.contains(parent) {
                    result.insert(*child);
                }
            }
            if result.len() == before {
                return Ok(result);
            }
        }
    }

    #[test]
    fn transport_binding_has_only_the_panel_label() {
        let only_main = |label: &str| (label == "main").then_some("main handle");
        assert_eq!(lookup_panel(only_main), Err("unavailable"));
        let panel = |label: &str| (label == "panel-browser").then_some("panel handle");
        assert_eq!(lookup_panel(panel), Ok("panel handle"));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires the real WebView2 runtime and an interactive Windows desktop"]
    fn hidden_webview2_can_use_portless_cdp_against_local_fixture() {
        run_local_fixture(false, false, false);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires the real WebView2 runtime and an interactive Windows desktop"]
    fn blocked_origin_receives_no_request_content() {
        run_local_fixture(true, false, false);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires the real WebView2 runtime and an interactive Windows desktop"]
    fn panel_mcp_scope_login_and_close_through_product_path() {
        run_local_fixture(true, true, false);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires the real WebView2 runtime and an interactive Windows desktop"]
    fn panel_takeover_cancels_approval_and_requires_new_request() {
        run_local_fixture(true, true, true);
    }

    #[cfg(windows)]
    fn run_local_fixture(allow_tcp_without_data: bool, run_mcp: bool, takeover: bool) {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use tauri::{LogicalPosition, LogicalSize, WebviewUrl};
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("profile");
        let data_root = temp.path().join("data");
        std::fs::create_dir_all(&data_root).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop_server = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let server_stopped = stop_server.clone();
        let server = std::thread::spawn(move || {
            let body = concat!("<html><title>fixture</title><body><button>로그인</button><input id='name'><button id='send' onclick=\"document.body.dataset.clicked='yes'\">send</button><button id='publish'>게시</button><button onclick=\"window.open('/momso-test-google-login?state=fixture')\">게시 Google popup</button><a href='https://accounts.google.com/signin'>Google</a><p>", "sk-", "proj-fakefixtureabcdefghijklmnop123456", "</p></body></html>");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
            while std::time::Instant::now() < deadline
                && !server_stopped.load(std::sync::atomic::Ordering::Acquire)
            {
                if let Ok((mut stream, _)) = listener.accept() {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(300)));
                    let mut input = [0; 4096];
                    if matches!(stream.read(&mut input), Ok(count) if count > 0)
                        && write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).is_ok() { continue; }
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let app = tauri::Builder::<tauri::Wry>::default().any_thread().setup(move |app| {
            if run_mcp {
                let db = data_root.join("momso.db");
                crate::storage::initialize(&db, &data_root.join("backups"), "fixture")?;
                rusqlite::Connection::open(&db)?.execute("INSERT INTO chats(id,title,created_at,updated_at) VALUES ('panel-mcp-fixture','시험 채팅방','fixture','fixture')", [])?;
                let location = crate::archive_location::ArchiveLocation::load(db.clone(), &data_root, vec![])?;
                let manager = std::sync::Arc::new(crate::browser_runtime::BrowserManager::default());
                manager.bind_app(app.handle().clone());
                app.manage(crate::AppState {
                    paths: crate::runtime::RuntimePaths { data_root: data_root.clone(), database_path: db, objects_path: data_root.join("objects"), exports_path: data_root.join("exports"), backups_path: data_root.join("backups"), logs_path: data_root.join("logs"), archive: location, archive_lease: None },
                    approvals: Default::default(), setup_secrets: Default::default(), usage_monitor: Default::default(), usage_cache: Default::default(), chat_active: Default::default(), browser: manager, selections: Default::default(),
                });
            }
            let window = tauri::window::WindowBuilder::new(app, "main").visible(false).build()?;
            let main = tauri::webview::WebviewBuilder::new("main", WebviewUrl::External("about:blank".parse()?)).data_directory(profile.with_file_name("main-profile"));
            window.add_child(main, LogicalPosition::new(0.0, 0.0), LogicalSize::new(1.0, 1.0))?;
            if PanelTransport::bind(app.handle()).is_ok() { return Err(std::io::Error::other("main was incorrectly bound").into()); }
            let mut view = tauri::webview::WebviewBuilder::new(PANEL_LABEL, WebviewUrl::External("about:blank".parse()?))
                .data_directory(profile.clone())
                .on_navigation(|url| crate::panel_browser::navigation_gate().permits_navigation(url.as_str()))
                .on_new_window(|_, _| { let _ = crate::panel_browser::navigation_gate().permits_popup(); tauri::webview::NewWindowResponse::Deny });
            if let Ok(args) = std::env::var("MOMSO_TEST_PANEL_BROWSER_ARGS") { view = view.additional_browser_args(&args); }
            window.add_child(view, LogicalPosition::new(0.0, 0.0), LogicalSize::new(640.0, 480.0))?;
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Runtime::new().unwrap();
                let result = runtime.block_on(async {
                    let transport = PanelTransport::bind(&handle)?;
                    let mut events = transport.subscribe("Runtime.consoleAPICalled").await?;
                    transport.call("Runtime.enable", serde_json::json!({})).await?;
                    transport.call("Page.navigate", serde_json::json!({"url":format!("http://{address}/")})).await?;
                    let mut ready = false;
                    for _ in 0..100 {
                        let result = transport.call("Runtime.evaluate", serde_json::json!({"expression":"document.title","returnByValue":true})).await?;
                        if result.pointer("/result/value").and_then(Value::as_str) == Some("fixture") { ready = true; break; }
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                    if !ready { return Err("unavailable"); }
                    let result = transport.call("Runtime.evaluate", serde_json::json!({"expression":"document.querySelector('#name').value='sample';document.querySelector('#send').click();JSON.stringify({title:document.title,value:document.querySelector('#name').value,clicked:document.body.dataset.clicked})","returnByValue":true})).await?;
                    let value = result.pointer("/result/value").and_then(Value::as_str).ok_or("unavailable")?;
                    let value: Value = serde_json::from_str(value).map_err(|_| "unavailable")?;
                    if value != serde_json::json!({"title":"fixture","value":"sample","clicked":"yes"}) { return Err("unavailable"); }
                    transport.call("Runtime.evaluate", serde_json::json!({"expression":"console.log('fixture-event')"})).await?;
                    let event = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv()).await.map_err(|_| "unavailable")?.ok_or("unavailable")?;
                    if event.pointer("/params/type").and_then(Value::as_str) != Some("log") { return Err("unavailable"); }
                    drop(events);
                    // With no AI worker, the human panel must not enable Fetch interception.
                    let human_probe = TcpListener::bind("127.0.0.1:0").map_err(|_| "unavailable")?;
                    human_probe.set_nonblocking(true).map_err(|_| "unavailable")?;
                    let human_url = format!("http://{}/human", human_probe.local_addr().map_err(|_| "unavailable")?);
                    let mut human_paused = transport.subscribe("Fetch.requestPaused").await?;
                    let expression = format!("fetch({}).catch(()=>{{}});void 0", serde_json::to_string(&human_url).map_err(|_| "unavailable")?);
                    transport.call("Runtime.evaluate", serde_json::json!({"expression":expression})).await?;
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(4);
                    let mut human_requested = false;
                    while std::time::Instant::now() < until {
                        if let Ok((mut stream, _)) = human_probe.accept() {
                            stream.set_nonblocking(false).map_err(|_| "unavailable")?;
                            stream.set_read_timeout(Some(std::time::Duration::from_millis(300))).map_err(|_| "unavailable")?;
                            let mut input = [0; 1024];
                            if matches!(stream.read(&mut input), Ok(count) if count > 0) {
                                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                                human_requested = true;
                                break;
                            }
                        } else { tokio::time::sleep(std::time::Duration::from_millis(20)).await; }
                    }
                    if !human_requested || tokio::time::timeout(std::time::Duration::from_millis(200), human_paused.recv()).await.is_ok() { return Err("human_mode_intercepted"); }
                    drop(human_paused);
                    drop(human_probe);
                    let forbidden = TcpListener::bind("127.0.0.1:0").map_err(|_| "unavailable")?;
                    forbidden.set_nonblocking(true).map_err(|_| "unavailable")?;
                    let forbidden_url = format!("http://{}/", forbidden.local_addr().map_err(|_| "unavailable")?);
                    let case = if run_mcp { "fetch".into() } else { std::env::var("MOMSO_TEST_PANEL_CASE").unwrap_or_else(|_| "navigation".into()) };
                    let mut allowed = vec![format!("http://{address}")];
                    let mut redirect_url = None;
                    let mut redirect_server = None;
                    if case == "redirect" {
                        let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| "unavailable")?;
                        listener.set_nonblocking(true).map_err(|_| "unavailable")?;
                        let origin = format!("http://{}", listener.local_addr().map_err(|_| "unavailable")?);
                        redirect_url = Some(format!("{origin}/"));
                        allowed.push(origin);
                        let location = forbidden_url.clone();
                        redirect_server = Some(std::thread::spawn(move || {
                            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                            while std::time::Instant::now() < until {
                                if let Ok((mut stream, _)) = listener.accept() {
                                    let _ = stream.set_nonblocking(false);
                                    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(300)));
                                    let mut input = [0; 1024];
                                    if matches!(stream.read(&mut input), Ok(count) if count > 0) {
                                        let _ = write!(stream, "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                                        return;
                                    }
                                } else { std::thread::sleep(std::time::Duration::from_millis(10)); }
                            }
                        }));
                    }
                    let mut paused = transport.subscribe("Fetch.requestPaused").await?;
                    let worker = crate::panel_worker::PanelWorker::start(handle.clone(), &allowed)?;
                    if run_mcp {
                        worker.call("Runtime.evaluate", serde_json::json!({"expression":"globalThis.__momso_webauthn_request('fixture')"}))?;
                        let (webauthn, _) = worker.security_flags();
                        for _ in 0..100 {
                            if webauthn.load(std::sync::atomic::Ordering::Acquire) { break; }
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                        if !webauthn.load(std::sync::atomic::Ordering::Acquire) { return Err("webauthn_guard_missing"); }
                    }
                    if std::env::var_os("MOMSO_TEST_PANEL_DISABLE_FETCH").is_some() {
                        worker.call("Fetch.disable", serde_json::json!({}))?;
                    }
                    if std::env::var_os("MOMSO_TEST_PANEL_BLOCK_URLS").is_some() {
                        worker.call("Network.enable", serde_json::json!({}))?;
                        worker.call("Network.setBlockedURLs", serde_json::json!({"urlPatterns":[{"urlPattern":format!("http://{address}/*"),"block":false},{"urlPattern":"*://*:*/*","block":true}]}))?;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    if forbidden.accept().is_ok() { return Err("fixture_contaminated"); }
                    let quoted = serde_json::to_string(&forbidden_url).map_err(|_| "unavailable")?;
                    let expression = match case.as_str() {
                        "navigation" => format!("location.href={quoted}"),
                        "fetch" => format!("fetch({quoted}).catch(()=>{{}});void 0"),
                        "image" => format!("let x=document.createElement('img');x.src={quoted};document.body.append(x);void 0"),
                        "script" => format!("let x=document.createElement('script');x.src={quoted};document.body.append(x);void 0"),
                        "redirect" => format!("location.href={}", serde_json::to_string(&redirect_url.ok_or("unavailable")?).map_err(|_| "unavailable")?),
                        "popup" => format!("window.open({quoted});void 0"),
                        _ => return Err("invalid_fixture_case"),
                    };
                    worker.call("Runtime.evaluate", serde_json::json!({"expression":expression,"returnByValue":true,"userGesture":true}))?;
                    if case == "navigation" || case == "redirect" || case == "popup" {
                        for _ in 0..100 {
                            if worker.blocked().is_some() { break; }
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                        let blocked_origin = reqwest::Url::parse(&forbidden_url).map_err(|_| "unavailable")?.origin().ascii_serialization();
                        let expected = if case == "popup" { "new_window" } else { blocked_origin.as_str() };
                        if worker.blocked().as_deref() != Some(expected) { return Err("wrong_block_event"); }
                    } else if std::env::var_os("MOMSO_TEST_PANEL_BLOCK_URLS").is_none() {
                        let event = tokio::time::timeout(std::time::Duration::from_secs(4), paused.recv()).await.map_err(|_| "no_fetch_pause")?.ok_or("no_fetch_pause")?;
                        if event.pointer("/params/request/url").and_then(Value::as_str) != Some(forbidden_url.as_str()) { return Err("wrong_fetch_event"); }
                    }
                    let mut streams = Vec::new();
                    let mut data_connections = 0_u32;
                    let mut requests = 0_u32;
                    let until = std::time::Instant::now() + std::time::Duration::from_millis(1000);
                    while std::time::Instant::now() < until {
                        if let Ok((stream, _)) = forbidden.accept() {
                            streams.push(stream);
                        } else { tokio::time::sleep(std::time::Duration::from_millis(20)).await; }
                    }
                    let connections = streams.len();
                    for mut stream in streams {
                            stream.set_nonblocking(false).map_err(|_| "unavailable")?;
                            stream.set_read_timeout(Some(std::time::Duration::from_secs(1))).map_err(|_| "unavailable")?;
                            let mut received = Vec::new();
                            let mut buffer = [0; 1024];
                            while received.len() < 8192 {
                                match stream.read(&mut buffer) {
                                    Ok(0) | Err(_) => break,
                                    Ok(count) => { received.extend_from_slice(&buffer[..count]); if received.contains(&b'\n') { break; } }
                                }
                            }
                            if !received.is_empty() {
                                data_connections += 1;
                                if received.contains(&b'\n') && [b"GET ".as_slice(), b"POST ", b"HEAD ", b"OPTIONS ", b"PUT ", b"DELETE ", b"CONNECT "].iter().any(|prefix| received.starts_with(prefix)) { requests += 1; }
                            }
                    }
                    println!("forbidden fixture: case={case}, tcp={connections}, data_connections={data_connections}, http_lines={requests}");
                    if data_connections > 0 { return Err("forbidden_data"); }
                    if connections > 0 && !allow_tcp_without_data { return Err("forbidden_tcp_connection"); }
                    if let Some(server) = redirect_server { server.join().map_err(|_| "unavailable")?; }
                    drop(worker);
                    if case == "fetch" {
                        let mut session = crate::browser_driver::Session::attach_panel(handle.clone(), &[format!("http://{address}")], &[], &[], None).map_err(|_| "driver_attach_failed")?;
                        if !matches!(session.read(), Err(crate::browser_driver::DriverError::Refused("panel_page_changed"))) { return Err("existing_page_was_read"); }
                        let page = session.navigate(&format!("http://{address}/")).map_err(|_| "driver_navigate_failed")?;
                        if page.title != "fixture" { return Err("driver_wrong_page"); }
                        let input = page.elements.iter().find(|item| item.role == "input").ok_or("missing_input")?;
                        session.fill(&input.id, "example").map_err(|_| "driver_fill_failed")?;
                        let button = page.elements.iter().find(|item| item.name == "send").ok_or("missing_button")?;
                        session.click(&button.id, "send").map_err(|_| "driver_click_failed")?;
                        if !matches!(session.navigate(&forbidden_url), Err(crate::browser_driver::DriverError::Blocked(_))) { return Err("driver_failed_to_block"); }
                        session.close();
                        let origin = format!("http://{address}");
                        let mut login = crate::browser_driver::Session::attach_panel(handle.clone(), std::slice::from_ref(&origin), std::slice::from_ref(&origin), &[], None).map_err(|_| "login_attach_failed")?;
                        if !matches!(login.navigate(&format!("{origin}/login")), Err(crate::browser_driver::DriverError::HumanRequired("login"))) { return Err("login_guard_failed"); }
                        login.close();
                        let mut outside = crate::browser_driver::Session::attach_panel(handle.clone(), std::slice::from_ref(&forbidden_url), &[], &[], None).map_err(|_| "outside_attach_failed")?;
                        if !matches!(outside.read(), Err(crate::browser_driver::DriverError::Blocked(_))) { return Err("outside_read_not_blocked"); }
                        outside.close();
                    }
                    let view = handle.get_webview(PANEL_LABEL).ok_or("unavailable")?;
                    let (pid_tx, pid_rx) = tokio::sync::oneshot::channel();
                    view.with_webview(move |platform| {
                        let mut pid = 0;
                        let result = unsafe { platform.controller().CoreWebView2().and_then(|core| core.BrowserProcessId(&mut pid)) }.map(|_| pid).map_err(|_| "unavailable");
                        let _ = pid_tx.send(result);
                    }).map_err(|_| "unavailable")?;
                    let pid = pid_rx.await.map_err(|_| "unavailable")??;
                    let processes = process_tree(&[std::process::id(), pid])?;
                    let output = crate::hidden_process::hidden_command("netstat").arg("-ano").output().map_err(|_| "unavailable")?;
                    if !output.status.success() { return Err("unavailable"); }
                    let output = String::from_utf8_lossy(&output.stdout);
                    if output.lines().any(|line| {
                        let columns: Vec<_> = line.split_whitespace().collect();
                        if columns.len() < 5 || columns[0] != "TCP" || columns[columns.len() - 2] != "LISTENING" { return false; }
                        let Ok(owner) = columns[columns.len() - 1].parse::<u32>() else { return false; };
                        if !processes.contains(&owner) { return false; }
                        let fixture = columns[1] == address.to_string() || columns[1] == forbidden_url.trim_start_matches("http://").trim_end_matches('/');
                        !(owner == std::process::id() && fixture)
                    }) { return Err("unexpected_listener"); }
                    println!("controlled process tree: processes={}, listening_tcp=0 (fixture listeners excluded)", processes.len());
                    if run_mcp {
                        run_product_mcp(&handle, data_root.clone(), format!("http://{address}"), takeover).await?;
                    }
                    Ok(())
                });
                let _ = tx.send(result);
                handle.exit(0);
            });
            Ok(())
        }).build(tauri::test::mock_context(tauri::test::noop_assets())).unwrap();
        app.run_return(|_, _| {});
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(120))
                .unwrap(),
            Ok(())
        );
        stop_server.store(true, std::sync::atomic::Ordering::Release);
        server.join().unwrap();
    }

    #[cfg(windows)]
    async fn run_product_mcp(
        app: &tauri::AppHandle,
        root: std::path::PathBuf,
        site: String,
        takeover: bool,
    ) -> Result<(), &'static str> {
        use crate::approval::ApprovalChoice;
        use tauri::{Emitter, Listener, Manager};
        use tokio_util::sync::CancellationToken;

        app.get_webview(PANEL_LABEL)
            .ok_or("panel_missing")?
            .close()
            .map_err(|_| "panel_close_failed")?;
        let until = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.get_webview(PANEL_LABEL).is_some() && tokio::time::Instant::now() < until {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        if app.get_webview(PANEL_LABEL).is_some() {
            return Err("panel_close_failed");
        }
        let ui_app = app.clone();
        let ui_profile = root.join("simulated-ui-panel");
        let open_listener = app.listen("panel-agent-open-request", move |_| {
            let app = ui_app.clone();
            let profile = ui_profile.clone();
            tauri::async_runtime::spawn(async move {
                if !takeover {
                    tokio::time::sleep(std::time::Duration::from_secs(21)).await;
                }
                let main_app = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if let Some(window) = main_app.get_window("main") {
                        let builder = tauri::webview::WebviewBuilder::new(
                            PANEL_LABEL,
                            tauri::WebviewUrl::External("about:blank".parse().unwrap()),
                        )
                        .data_directory(profile)
                        .on_navigation(|url| {
                            crate::panel_browser::navigation_gate().permits_navigation(url.as_str())
                        })
                        .on_new_window(|url, _| {
                            let _ = crate::panel_browser::deny_agent_popup(&url);
                            tauri::webview::NewWindowResponse::Deny
                        });
                        let _ = window.add_child(
                            builder,
                            tauri::LogicalPosition::new(0.0, 0.0),
                            tauri::LogicalSize::new(640.0, 480.0),
                        );
                    }
                });
            });
        });
        let state = app.state::<crate::AppState>();
        let manager = state.browser.clone();
        let broker = state.approvals.clone();
        let panel_events = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let recorded_events = panel_events.clone();
        let panel_listener = app.listen("panel-agent-control-changed", move |event| {
            if let Ok(value) = serde_json::from_str::<Value>(event.payload()) {
                if let Ok(mut events) = recorded_events.lock() {
                    events.push(value);
                }
            }
        });
        let turn = CancellationToken::new();
        state
            .chat_active
            .lock()
            .await
            .insert("panel-mcp-fixture".into(), turn.clone());
        let (cards_tx, mut cards_rx) =
            tokio::sync::mpsc::unbounded_channel::<(String, String, String)>();
        let cards_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let closed_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let closed_observed = closed_count.clone();
        let closed_listener = app.listen("agent-approval-closed", move |event| {
            if serde_json::from_str::<Value>(event.payload())
                .is_ok_and(|v| v["reason"] == "provider_cancelled")
            {
                closed_observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });
        let count = cards_count.clone();
        let listener = app.listen("agent-approval-request", move |event| {
            if let Ok(value) = serde_json::from_str::<Value>(event.payload()) {
                if let Some(id) = value.get("requestId").and_then(Value::as_str) {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let _ = cards_tx.send((
                        id.to_owned(),
                        value["kind"].as_str().unwrap_or_default().to_owned(),
                        value["description"].as_str().unwrap_or_default().to_owned(),
                    ));
                }
            }
        });
        let responder_app = app.clone();
        let responder = tokio::spawn(async move {
            while let Some((id, kind, description)) = cards_rx.recv().await {
                if kind == "browser_action" && !description.contains("Google popup") {
                    if takeover {
                        let _ = crate::panel_agent_take_over(
                            responder_app.clone(),
                            responder_app.state(),
                            "panel-mcp-fixture".into(),
                        )
                        .await;
                        continue;
                    }
                    if let Ok(transport) = PanelTransport::bind(&responder_app) {
                        let _ = transport.call("Runtime.evaluate", serde_json::json!({"expression":"document.querySelector('#publish').textContent='Changed'"})).await;
                    }
                }
                let _ = broker.respond(&id, ApprovalChoice::AllowOnce).await;
            }
        });
        let mut fixture_policy = crate::browser_gateway::Policy::test_loopback(site.clone());
        fixture_policy.add_test_login_path(format!("{site}/account"));
        let policy = Ok(Some(fixture_policy));
        let handler_app = app.clone();
        let resume_site = site.clone();
        let human_login_phase = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handler: crate::setup_bridge::Handler = std::sync::Arc::new(move |input| {
            let app = handler_app.clone();
            let manager = manager.clone();
            let policy = policy.clone();
            let root = root.clone();
            let resume_site = resume_site.clone();
            let human_login_phase = human_login_phase.clone();
            Box::pin(async move {
                let event_app = app.clone();
                let notify: crate::browser_runtime::Notify =
                    std::sync::Arc::new(move |name, payload| {
                        let _ = event_app.emit(name, payload);
                    });
                let approval_app = app.clone();
                let broker = app.state::<crate::AppState>().approvals.clone();
                let ask: crate::browser_runtime::Ask =
                    std::sync::Arc::new(move |request, token| {
                        let app = approval_app.clone();
                        let broker = broker.clone();
                        Box::pin(async move {
                            matches!(
                                broker.request(&app, request, token).await,
                                Ok(ApprovalChoice::AllowOnce)
                            )
                        })
                    });
                let opening = input["method"] == "browser_open";
                let reading = input["method"] == "browser_read";
                let requesting_login = input["method"] == "browser_request_human_login";
                let result = manager
                    .dispatch(
                        crate::browser_runtime::RequestContext {
                            chat_id: "panel-mcp-fixture",
                            provider: "claude",
                            skill: Some("fixture"),
                            selected: &policy,
                            root: root.clone(),
                            visible: false,
                            cancel: CancellationToken::new(),
                            notify: notify.clone(),
                            ask: ask.clone(),
                        },
                        input,
                    )
                    .await;
                if opening && result["status"] == "ok" {
                    let other = manager
                        .dispatch(
                            crate::browser_runtime::RequestContext {
                                chat_id: "other-chat",
                                provider: "claude",
                                skill: Some("fixture"),
                                selected: &policy,
                                root,
                                visible: false,
                                cancel: CancellationToken::new(),
                                notify,
                                ask,
                            },
                            serde_json::json!({"method":"browser_open","arguments":{}}),
                        )
                        .await;
                    if other["status"] != "panel_busy" {
                        return serde_json::json!({"status":"unavailable"});
                    }
                }
                if result["status"] == "external_login_required" {
                    let _ = crate::panel_agent_resume(
                        app.clone(),
                        app.state(),
                        "panel-mcp-fixture".into(),
                    )
                    .await;
                } else if requesting_login && result["status"] == "human_required" {
                    human_login_phase.store(1, std::sync::atomic::Ordering::SeqCst);
                } else if reading
                    && result["status"] == "human_required"
                    && human_login_phase.load(std::sync::atomic::Ordering::SeqCst) == 1
                {
                    let _ = crate::panel_agent_resume(
                        app.clone(),
                        app.state(),
                        "panel-mcp-fixture".into(),
                    )
                    .await;
                    human_login_phase.store(2, std::sync::atomic::Ordering::SeqCst);
                } else if result["status"] == "human_required" {
                    if let Ok(transport) = PanelTransport::bind(&app) {
                        let _ = transport
                            .call(
                                "Page.navigate",
                                serde_json::json!({"url":format!("{resume_site}/")}),
                            )
                            .await;
                        for _ in 0..60 {
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                            if crate::panel_agent_resume(
                                app.clone(),
                                app.state(),
                                "panel-mcp-fixture".into(),
                            )
                            .await
                            .is_ok()
                            {
                                break;
                            }
                        }
                    }
                    let _ = crate::panel_browser::panel_browser_close(app).await;
                } else if takeover && reading && result["status"] == "human_takeover" {
                    let _ = crate::panel_agent_resume(
                        app.clone(),
                        app.state(),
                        "panel-mcp-fixture".into(),
                    )
                    .await;
                    app.state::<crate::AppState>()
                        .chat_active
                        .lock()
                        .await
                        .insert("panel-mcp-fixture".into(), CancellationToken::new());
                }
                result
            })
        });
        let chat = format!("panel-mcp-{}", uuid::Uuid::new_v4());
        let bridge = crate::setup_bridge::start(&chat, CancellationToken::new(), handler)
            .map_err(|_| "mcp_start_failed")?;
        crate::setup_bridge::bind(&chat, Some(std::process::id()));
        let pipe = crate::setup_bridge::name(&chat).ok_or("mcp_pipe_missing")?;
        let expected_site = format!("{site}/");
        let result = tokio::task::spawn_blocking(move || -> Result<(), &'static str> {
            let file = std::fs::OpenOptions::new().read(true).write(true).open(pipe).map_err(|_| "mcp_connect_failed")?;
            let mut client = crate::momso_mcp::test_bridge(file);
            let mut ready = true;
            let mut call = |name: &str, args: Value| -> Result<Value, &'static str> {
                let result = crate::momso_mcp::test_response(&serde_json::json!({"id":1,"method":"tools/call","params":{"name":name,"arguments":args}}), &mut ready, &mut client).ok_or("mcp_response_missing")?;
                let text = result.pointer("/result/content/0/text").and_then(Value::as_str).ok_or("mcp_response_invalid")?;
                serde_json::from_str(text).map_err(|_| "mcp_response_invalid")
            };
            if call("browser_open", serde_json::json!({}))?["status"] != "ok" { return Err("mcp_open_failed"); }
            if call("browser_navigate", serde_json::json!({"url":format!("{site}/")}))?["status"] != "page" { return Err("mcp_navigate_failed"); }
            if call("browser_request_human_login", serde_json::json!({"reason":"login"}))?["status"] != "human_required" { return Err("mcp_login_request_failed"); }
            if call("browser_read", serde_json::json!({}))?["status"] != "human_required" { return Err("mcp_login_not_paused"); }
            if call("browser_read", serde_json::json!({}))?["status"] != "page" { return Err("mcp_login_resume_failed"); }
            let external = call("browser_navigate", serde_json::json!({"url":"https://accounts.google.com/signin?state=fixture"}))?;
            if external != serde_json::json!({"status":"external_login_required"}) { return Err("mcp_external_login_signal_failed"); }
            let page = call("browser_read", serde_json::json!({}))?;
            if page["kind"] != "untrusted_page_data" || page.to_string().contains(concat!("sk-", "proj-fakefixtureabcdefghijklmnop123456")) { return Err("mcp_read_or_redaction_failed"); }
            let google = page["elements"].as_array().and_then(|elements| elements.iter().find(|e| e["name"] == "Google")).and_then(|e| e["id"].as_str()).ok_or("mcp_google_link_missing")?.to_owned();
            if call("browser_click", serde_json::json!({"elementId":google}))?["status"] != "external_login_required" { return Err("mcp_google_link_signal_failed"); }
            let popup = page["elements"].as_array().and_then(|elements| elements.iter().find(|e| e["name"] == "게시 Google popup")).and_then(|e| e["id"].as_str()).ok_or("mcp_google_popup_missing")?.to_owned();
            if call("browser_request_action", serde_json::json!({"action":"게시","description":"popup fixture","elementId":popup}))?["status"] != "approved" { return Err("mcp_google_popup_scope_failed"); }
            if call("browser_click", serde_json::json!({"elementId":popup}))?["status"] != "external_login_required" { return Err("mcp_google_popup_signal_failed"); }
            let id = page["elements"].as_array().and_then(|elements| elements.iter().find(|e| e["name"] == "게시")).and_then(|e| e["id"].as_str()).ok_or("mcp_target_missing")?.to_owned();
            if call("browser_request_action", serde_json::json!({"action":"게시","description":"fixture","elementId":id}))?["status"] != "approved" { return Err("mcp_action_scope_failed"); }
            if takeover {
                if call("browser_click", serde_json::json!({"elementId":id}))?["status"] != "human_takeover" { return Err("mcp_takeover_failed"); }
                if call("browser_read", serde_json::json!({}))?["status"] != "human_takeover" { return Err("mcp_takeover_not_retained"); }
                if call("browser_open", serde_json::json!({}))?["status"] != "ok" { return Err("mcp_resume_failed"); }
                return Ok(());
            }
            if call("browser_click", serde_json::json!({"elementId":id}))?["status"] != "target_changed" { return Err("mcp_changed_target_failed"); }
            if call("browser_navigate", serde_json::json!({"url":"https://outside.example/"}))?["status"] != "blocked" { return Err("mcp_scope_block_failed"); }
            if call("browser_request_host", serde_json::json!({"host":"outside.example"}))?["status"] != "approved" { return Err("mcp_scope_approval_failed"); }
            if call("browser_navigate", serde_json::json!({"url":format!("{site}/account")}))?["status"] != "human_required" { return Err("mcp_login_guard_failed"); }
            if call("browser_read", serde_json::json!({}))?["status"] != "panel_closed" { return Err("mcp_close_failed"); }
            Ok(())
        }).await.map_err(|_| "mcp_task_failed")?;
        drop(bridge);
        app.unlisten(listener);
        app.unlisten(open_listener);
        responder.abort();
        result?;
        if !turn.is_cancelled() {
            return Err("mcp_turn_not_cancelled");
        }
        if takeover && closed_count.load(std::sync::atomic::Ordering::SeqCst) < 1 {
            return Err("approval_close_missing");
        }
        app.unlisten(closed_listener);
        app.unlisten(panel_listener);
        let events = panel_events.lock().map_err(|_| "panel_event_lock_failed")?;
        if !events.iter().any(|v| {
            v["state"] == "login"
                && v["reason"] == "google_login_external"
                && v["provider"] == "claude"
                && v["chatTitle"] == "시험 채팅방"
                && v["loginUrl"] == "https://accounts.google.com/signin?state=fixture"
                && v["siteUrl"] == expected_site
        }) {
            return Err("panel_login_event_missing");
        }
        if !takeover {
            let last_login = events
                .iter()
                .rposition(|v| v["state"] == "login" && v["reason"] == "login_page")
                .ok_or("panel_login_page_missing")?;
            if !events.iter().skip(last_login + 1).any(|v| {
                v["state"] == "started"
                    && v["provider"] == "claude"
                    && v["chatTitle"] == "시험 채팅방"
            }) {
                return Err("panel_resume_event_missing");
            }
        }
        if cards_count.load(std::sync::atomic::Ordering::SeqCst) < 2 {
            return Err("approval_cards_missing");
        }
        Ok(())
    }
}
