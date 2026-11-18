//! Isolated, human-operated panel. Remote pages receive no application capability.
use tauri::{
    webview::{DownloadEvent, NewWindowResponse, PageLoadEvent, WebviewBuilder},
    Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder,
};

pub(crate) const LABEL: &str = "panel-browser";

pub(crate) fn navigation_gate() -> &'static crate::panel_navigation::PanelNavigation {
    static GATE: std::sync::OnceLock<crate::panel_navigation::PanelNavigation> =
        std::sync::OnceLock::new();
    GATE.get_or_init(Default::default)
}

const POPUP_PREFIX: &str = "panel-popup-";
const CACHE_LIMIT: u64 = 500 * 1024 * 1024;

#[cfg(windows)]
struct PopupInfo {
    host: Option<String>,
    opener_host: Option<String>,
    title: Option<String>,
}

#[cfg(windows)]
type PopupEntry = std::sync::Arc<std::sync::Mutex<PopupInfo>>;
#[cfg(windows)]
static POPUPS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, PopupEntry>>,
> = std::sync::OnceLock::new();

#[cfg(windows)]
fn popup_event(
    app: &tauri::AppHandle,
    id: &str,
    info: &PopupEntry,
    state: &str,
    reason: Option<&str>,
) {
    if let Ok(info) = info.lock() {
        let _ = app.emit_to(
            "main",
            "panel-browser-popup",
            serde_json::json!({
                "id": id, "state": state, "host": info.host, "openerHost": info.opener_host,
                "title": info.title, "reason": reason
            }),
        );
    }
}

#[cfg(windows)]
fn close_popup_event(app: &tauri::AppHandle, label: &str, reason: &str) {
    let entry = POPUPS
        .get_or_init(Default::default)
        .lock()
        .ok()
        .and_then(|mut popups| popups.remove(label));
    if let Some(info) = entry {
        popup_event(app, label, &info, "closed", Some(reason));
    }
}

fn cache_exceeds_limit(profile: &std::path::Path, limit: u64) -> bool {
    let cache = profile.join("EBWebView").join("Default").join("Cache");
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(cache)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry
                .metadata()
                .map(|metadata| {
                    if entry.file_type().is_symlink() {
                        return false;
                    }
                    #[cfg(windows)]
                    {
                        use std::os::windows::fs::MetadataExt;
                        if metadata.file_attributes() & 0x400 != 0 {
                            return false;
                        }
                    }
                    true
                })
                .unwrap_or(false)
        })
        .flatten()
    {
        if entry.file_type().is_file() {
            total = total.saturating_add(entry.metadata().map(|value| value.len()).unwrap_or(0));
            if total > limit {
                return true;
            }
        }
    }
    false
}

pub(crate) fn start_cache_maintenance(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let Ok(root) = app.path().app_data_dir() else {
            return;
        };
        let profile = root.join("panel-browser-profile");
        let over_limit = tauri::async_runtime::spawn_blocking(move || {
            cache_exceeds_limit(&profile, CACHE_LIMIT)
        })
        .await
        .unwrap_or(false);
        if over_limit {
            if let Err(error) = clear_profile_data(app, true).await {
                log::warn!("Panel disk cache cleanup failed: {error}");
            }
        }
    });
}

fn allowed_address(address: &str) -> Result<reqwest::Url, &'static str> {
    #[cfg(debug_assertions)]
    if std::env::var_os("MOMSO_PANEL_ALLOW_LOOPBACK").is_some_and(|v| v == "1") {
        if let Ok(url) = reqwest::Url::parse(address) {
            if development_loopback(&url) {
                return Ok(url);
            }
        }
    }
    crate::external_url::validate(address)?;
    reqwest::Url::parse(address).map_err(|_| "invalid_url")
}

#[cfg(debug_assertions)]
fn development_loopback(url: &reqwest::Url) -> bool {
    url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.username().is_empty()
        && url.password().is_none()
}

fn is_google_login(url: &reqwest::Url) -> bool {
    url.host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("accounts.google.com"))
}

pub(crate) fn deny_agent_popup(url: &reqwest::Url) -> bool {
    if navigation_gate().permits_popup() {
        return false;
    }
    if crate::browser_runtime::google_login_target(url.as_str()) {
        navigation_gate().mark_external_login(url.as_str());
    }
    true
}

pub(crate) fn current_site_url(app: &tauri::AppHandle) -> Option<String> {
    app.get_webview(LABEL)
        .and_then(|view| view.url().ok())
        .filter(|url| allowed_address(url.as_str()).is_ok() && !is_google_login(url))
        .map(|url| url.to_string())
}

fn google_notice(app: &tauri::AppHandle, url: &reqwest::Url, site: Option<String>) -> bool {
    if !is_google_login(url) {
        return false;
    }
    let _ = app.emit_to(
        "main",
        "panel-browser-google-login",
        serde_json::json!({"host": "accounts.google.com", "siteUrl": site}),
    );
    true
}

fn human_browser_command(
    executable: &std::path::Path,
    profile: &std::path::Path,
    url: &reqwest::Url,
) -> std::process::Command {
    let mut command = crate::process::hidden_command(executable);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .args([
            "--no-first-run",
            "--no-default-browser-check",
            "--new-window",
        ])
        .arg(url.as_str())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for key in [
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "MOMSO_CODEX_API_KEY",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
    ] {
        command.env_remove(key);
    }
    command
}

#[tauri::command]
pub(crate) async fn panel_browser_open_human(
    app: tauri::AppHandle,
    url: String,
) -> Result<(), String> {
    let url = allowed_address(&url).map_err(str::to_owned)?;
    let executable = crate::browser_driver::executable().ok_or("browser_missing")?;
    let profile = app
        .path()
        .app_data_dir()
        .map_err(|_| "unavailable")?
        .join("human-browser-profile");
    std::fs::create_dir_all(&profile).map_err(|_| "unavailable")?;
    human_browser_command(&executable, &profile, &url)
        .spawn()
        .map_err(|_| "unavailable")?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn panel_browser_open(
    app: tauri::AppHandle,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let url = allowed_address(&url).map_err(str::to_owned)?;
    if google_notice(&app, &url, None) {
        return Err("google_login_required".into());
    }
    open_panel(app, url, x, y, width, height).await
}

/// Open an empty panel for an already-approved agent session; no site loads yet.
#[tauri::command]
pub(crate) async fn panel_browser_open_agent(
    app: tauri::AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let url = reqwest::Url::parse("about:blank").map_err(|_| "unavailable")?;
    open_panel(app, url, x, y, width, height).await
}

async fn open_panel(
    app: tauri::AppHandle,
    url: reqwest::Url,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    if ![x, y, width, height].iter().all(|v| v.is_finite()) || width <= 0.0 || height <= 0.0 {
        return Err("invalid_bounds".into());
    }
    if app.get_webview(LABEL).is_some() {
        return Err("already_open".into());
    }
    let profile = app
        .path()
        .app_data_dir()
        .map_err(|_| "unavailable")?
        .join("panel-browser-profile");
    std::fs::create_dir_all(&profile).map_err(|_| "unavailable")?;
    let window = app.get_window("main").ok_or("unavailable")?;
    let nav_app = app.clone();
    let popup_app = app.clone();
    let current_site = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let nav_site = current_site.clone();
    let popup_site = current_site.clone();
    let loaded_site = current_site;
    let panel_view = window
        .add_child(
            WebviewBuilder::new(LABEL, WebviewUrl::External(url))
                .data_directory(profile)
                .focused(false)
                .on_navigation(move |url| navigation_gate().permits_navigation(url.as_str()) && allowed_address(url.as_str()).is_ok() && !google_notice(&nav_app, url, nav_site.lock().ok().and_then(|v| v.clone())))
                .on_new_window(move |url, features| {
                    if deny_agent_popup(&url) || allowed_address(url.as_str()).is_err() || google_notice(&popup_app, &url, popup_site.lock().ok().and_then(|v| v.clone())) {
                        return NewWindowResponse::Deny;
                    }
                    #[cfg(windows)]
                    {
                        let Some(owner) = popup_app.get_window("main").and_then(|window| window.hwnd().ok()) else { return NewWindowResponse::Deny };
                        let info = std::sync::Arc::new(std::sync::Mutex::new(PopupInfo {
                            host: url.host_str().map(str::to_owned),
                            opener_host: popup_site.lock().ok().and_then(|site| site.as_ref().and_then(|s| reqwest::Url::parse(s).ok()).and_then(|u| u.host_str().map(str::to_owned))),
                            title: None,
                        }));
                        let page_info = info.clone();
                        let title_info = info.clone();
                        let popup_nav_app = popup_app.clone();
                        let popup_nav_site = popup_site.clone();
                        let builder = WebviewWindowBuilder::new(&popup_app, format!("{POPUP_PREFIX}{}", uuid::Uuid::new_v4()), WebviewUrl::External("about:blank".parse().expect("static URL")))
                            .window_features(features)
                            .owner_raw(owner)
                            .title(url.origin().ascii_serialization())
                            .on_navigation(move |url| allowed_address(url.as_str()).is_ok() && !google_notice(&popup_nav_app, url, popup_nav_site.lock().ok().and_then(|v| v.clone())))
                            .on_page_load(move |window, payload| {
                                if let Ok(mut info) = page_info.lock() { info.host = payload.url().host_str().map(str::to_owned); }
                                let _ = window.set_title(&payload.url().origin().ascii_serialization());
                            })
                            .on_document_title_changed(move |_, title| { if let Ok(mut info) = title_info.lock() { info.title = Some(title); } })
                            .on_new_window(|_, _| NewWindowResponse::Deny)
                            .on_download(|webview, event| match event {
                                DownloadEvent::Requested { url, destination } => crate::panel_download::start(webview.app_handle(), url.as_str(), destination),
                                DownloadEvent::Finished { url, path, success } => { crate::panel_download::finish(webview.app_handle().clone(), url.to_string(), path, success); true }
                                _ => false,
                            });
                        #[cfg(debug_assertions)]
                        let builder = if std::env::var_os("MOMSO_PANEL_TEST_HIDE_POPUPS").is_some_and(|value| value == "1") { builder.visible(false) } else { builder };
                        match builder.build() {
                            Ok(window) => {
                                crate::app_lifecycle::observe_webview(window.as_ref());
                                let label = window.label().to_owned();
                                POPUPS.get_or_init(Default::default).lock().expect("popup registry").insert(label.clone(), info.clone());
                                popup_event(&popup_app, &label, &info, "opened", None);
                                let window_app = popup_app.clone();
                                let window_label = label.clone();
                                window.on_window_event(move |event| {
                                    if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                                        close_popup_event(&window_app, &window_label, "user");
                                    } else if matches!(event, tauri::WindowEvent::Destroyed) {
                                        close_popup_event(&window_app, &window_label, "unknown");
                                    }
                                });
                                let close = window.clone();
                                let failed = window.clone();
                                let close_app = popup_app.clone();
                                let close_label = label;
                                if window.with_webview(move |platform| unsafe {
                                    let mut token = 0;
                                    let handler = webview2_com::WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
                                        close_popup_event(&close_app, &close_label, "site");
                                        let _ = close.destroy();
                                        Ok(())
                                    }));
                                    let result = platform.controller().CoreWebView2()
                                        .and_then(|core| core.add_WindowCloseRequested(&handler, &mut token));
                                    if let Err(error) = result {
                                        log::warn!("Panel popup close handler failed: {error}");
                                        let _ = failed.destroy();
                                    }
                                }).is_err() {
                                    let _ = window.destroy();
                                    return NewWindowResponse::Deny;
                                }
                                NewWindowResponse::Create { window }
                            }
                            Err(_) => NewWindowResponse::Deny,
                        }
                    }
                    #[cfg(not(windows))]
                    { let _ = features; NewWindowResponse::Deny }
                })
                .on_download(|webview, event| {
                    match event {
                        DownloadEvent::Requested { url, destination } => crate::panel_download::start(webview.app_handle(), url.as_str(), destination),
                        DownloadEvent::Finished { url, path, success } => {
                            crate::panel_download::finish(webview.app_handle().clone(), url.to_string(), path, success);
                            true
                        }
                        _ => false,
                    }
                })
                .on_document_title_changed(|webview, title| {
                    let _ = webview.app_handle().emit_to("main", "panel-browser-title", serde_json::json!({"title": title}));
                })
                .on_page_load(move |webview, payload| {
                    let url = payload.url();
                    if let Ok(mut site) = loaded_site.lock() { *site = Some(url.to_string()); }
                    let _ = webview.app_handle().emit_to("main", "panel-browser-page", serde_json::json!({
                        "url": url.as_str(), "host": url.host_str(), "secure": url.scheme() == "https",
                        "loading": matches!(payload.event(), PageLoadEvent::Started)
                    }));
                    emit_history(&webview);
                }),
            LogicalPosition::new(x, y),
            LogicalSize::new(width, height),
        )
        .map_err(|_| "unavailable".to_owned())?;
    crate::app_lifecycle::observe_webview(&panel_view);
    Ok(())
}

fn panel(app: &tauri::AppHandle) -> Result<tauri::Webview, String> {
    app.get_webview(LABEL).ok_or_else(|| "unavailable".into())
}

#[tauri::command]
pub(crate) async fn panel_browser_close(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(state) = app.try_state::<crate::AppState>() {
        if let Ok(Some(chat_id)) = state.browser.panel_ownership.panel_closed() {
            if let Some(token) = state.chat_active.lock().await.get(&chat_id).cloned() {
                token.cancel();
            }
            let event_app = app.clone();
            let notify: crate::browser_runtime::Notify =
                std::sync::Arc::new(move |name, payload| {
                    let _ = event_app.emit(name, payload);
                });
            state.browser.close(&chat_id, &notify).await;
        }
    }
    for (label, popup) in app.webview_windows() {
        if label.starts_with(POPUP_PREFIX) {
            #[cfg(windows)]
            close_popup_event(&app, &label, "panel_closed");
            let _ = popup.destroy();
        }
    }
    panel(&app)?.close().map_err(|_| "unavailable".into())
}

#[tauri::command]
pub(crate) async fn panel_browser_clear_data(
    app: tauri::AppHandle,
) -> Result<&'static str, String> {
    clear_profile_data(app, false).await
}

async fn clear_profile_data(
    app: tauri::AppHandle,
    disk_cache_only: bool,
) -> Result<&'static str, String> {
    let (view, temporary) = if let Some(view) = app.get_webview(LABEL) {
        (view, false)
    } else {
        let profile = app
            .path()
            .app_data_dir()
            .map_err(|_| "unavailable")?
            .join("panel-browser-profile");
        std::fs::create_dir_all(&profile).map_err(|_| "unavailable")?;
        let window = app.get_window("main").ok_or("unavailable")?;
        let view = window
            .add_child(
                WebviewBuilder::new(
                    "panel-browser-clear",
                    WebviewUrl::External("about:blank".parse().map_err(|_| "unavailable")?),
                )
                .data_directory(profile)
                .focused(false)
                .on_navigation(|_| false),
                LogicalPosition::new(-10000.0, -10000.0),
                LogicalSize::new(1.0, 1.0),
            )
            .map_err(|_| "unavailable")?;
        crate::app_lifecycle::observe_webview(&view);
        (view, true)
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    let submitted = view.with_webview(move |platform| {
        #[cfg(windows)]
        {
            use webview2_com::{
                ClearBrowsingDataCompletedHandler,
                Microsoft::Web::WebView2::Win32::{
                    ICoreWebView2Profile2, ICoreWebView2_13,
                    COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE,
                },
            };
            use windows_core::Interface;
            let callback_tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
            let inner_tx = callback_tx.clone();
            let result = unsafe {
                platform
                    .controller()
                    .CoreWebView2()
                    .and_then(|core| core.cast::<ICoreWebView2_13>())
                    .and_then(|core| core.Profile())
                    .and_then(|profile| profile.cast::<ICoreWebView2Profile2>())
                    .and_then(|profile| {
                        let handler =
                            ClearBrowsingDataCompletedHandler::create(Box::new(move |result| {
                                if let Some(sender) =
                                    inner_tx.lock().ok().and_then(|mut slot| slot.take())
                                {
                                    let _ = sender.send(result.is_ok());
                                }
                                Ok(())
                            }));
                        if disk_cache_only {
                            profile.ClearBrowsingData(
                                COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE,
                                &handler,
                            )
                        } else {
                            profile.ClearBrowsingDataAll(&handler)
                        }
                    })
            };
            if result.is_err() {
                if let Some(sender) = callback_tx.lock().ok().and_then(|mut slot| slot.take()) {
                    let _ = sender.send(false);
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = platform;
            let _ = disk_cache_only;
            let _ = tx.send(false);
        }
    });
    if submitted.is_err() {
        if temporary {
            let _ = view.close();
        }
        return Err("unavailable".into());
    }
    let result = match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
        Ok(Ok(true)) => Ok("cleared"),
        _ => Err("unavailable".into()),
    };
    if temporary {
        let _ = view.close();
    }
    result
}

#[tauri::command]
pub(crate) async fn panel_browser_visibility(
    app: tauri::AppHandle,
    visible: bool,
) -> Result<(), String> {
    let main_visible = app
        .get_window("main")
        .is_some_and(|window| window.is_visible().unwrap_or(false));
    for (label, popup) in app.webview_windows() {
        if label.starts_with(POPUP_PREFIX) {
            #[cfg(windows)]
            if visible && main_visible {
                if let Ok(hwnd) = popup.hwnd() {
                    unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                            hwnd.0 as _,
                            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE,
                        );
                    }
                }
            } else {
                let _ = popup.hide();
            }
            #[cfg(not(windows))]
            let _ = if visible && main_visible {
                popup.show()
            } else {
                popup.hide()
            };
        }
    }
    let view = panel(&app)?;
    if !visible {
        return view.hide().map_err(|_| "unavailable".into());
    }
    #[cfg(windows)]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        view.with_webview(move |platform| {
            let result = unsafe {
                let controller = platform.controller();
                let mut hwnd = Default::default();
                controller.ParentWindow(&mut hwnd).and_then(|_| {
                    windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                        hwnd.0 as _,
                        windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE,
                    );
                    controller.SetIsVisible(true)
                })
            };
            let _ = tx.send(result.is_ok());
        })
        .map_err(|_| "unavailable")?;
        match rx.await {
            Ok(true) => Ok(()),
            _ => Err("unavailable".into()),
        }
    }
    #[cfg(not(windows))]
    view.show().map_err(|_| "unavailable".into())
}

#[tauri::command]
pub(crate) async fn panel_browser_bounds(
    app: tauri::AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    if ![x, y, width, height].iter().all(|v| v.is_finite()) || width <= 0.0 || height <= 0.0 {
        return Err("invalid_bounds".into());
    }
    let view = panel(&app)?;
    view.set_position(LogicalPosition::new(x, y))
        .map_err(|_| "unavailable")?;
    view.set_size(LogicalSize::new(width, height))
        .map_err(|_| "unavailable".into())
}

#[tauri::command]
pub(crate) async fn panel_browser_navigate(
    app: tauri::AppHandle,
    url: String,
) -> Result<(), String> {
    let url = allowed_address(&url).map_err(str::to_owned)?;
    let site = app
        .get_webview(LABEL)
        .and_then(|view| view.url().ok())
        .filter(|current| allowed_address(current.as_str()).is_ok() && !is_google_login(current))
        .map(|current| current.to_string());
    if google_notice(&app, &url, site) {
        return Err("google_login_required".into());
    }
    panel(&app)?.navigate(url).map_err(|_| "unavailable".into())
}

fn emit_history(view: &tauri::Webview) {
    let app = view.app_handle().clone();
    let _ = view.with_webview(move |platform| {
        #[cfg(windows)]
        unsafe {
            if let Ok(core) = platform.controller().CoreWebView2() {
                let mut back = Default::default();
                let mut forward = Default::default();
                if core.CanGoBack(&mut back).is_ok() && core.CanGoForward(&mut forward).is_ok() {
                    let _ = app.emit_to("main", "panel-browser-history", serde_json::json!({"canGoBack": back.as_bool(), "canGoForward": forward.as_bool()}));
                }
            }
        }
        #[cfg(not(windows))]
        let _ = (platform, app);
    });
}

#[tauri::command]
pub(crate) async fn panel_browser_action(
    app: tauri::AppHandle,
    action: String,
) -> Result<(), String> {
    let view = panel(&app)?;
    if action == "reload" {
        return view.reload().map_err(|_| "unavailable".into());
    }
    if !matches!(action.as_str(), "back" | "forward" | "stop") {
        return Err("invalid_action".into());
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    view.with_webview(move |platform| {
        #[cfg(windows)]
        let result = unsafe {
            platform
                .controller()
                .CoreWebView2()
                .and_then(|core| match action.as_str() {
                    "back" => core.GoBack(),
                    "forward" => core.GoForward(),
                    _ => core.Stop(),
                })
                .map_err(|_| ())
        };
        #[cfg(not(windows))]
        let result: Result<(), ()> = {
            let _ = (platform, action);
            Err(())
        };
        let _ = tx.send(result);
    })
    .map_err(|_| "unavailable")?;
    rx.await
        .map_err(|_| "unavailable")?
        .map_err(|_| "unavailable".into())
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    #[ignore = "requires WebView2 and an interactive Windows desktop"]
    fn agent_panel_preserves_foreground_for_hidden_and_background_windows() {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetGUIThreadInfo, ShowWindow, GUITHREADINFO, SW_SHOWNOACTIVATE,
        };

        if std::env::var_os("MOMSO_FOCUS_PROBE_CHILD").is_none() {
            let temp = tempfile::tempdir().unwrap();
            let local = temp.path().join("AppData/Local");
            let roaming = temp.path().join("AppData/Roaming");
            std::fs::create_dir_all(&local).unwrap();
            std::fs::create_dir_all(&roaming).unwrap();
            let output = crate::process::hidden_command(std::env::current_exe().unwrap())
                .args(["--exact", "panel_browser::tests::agent_panel_preserves_foreground_for_hidden_and_background_windows", "--ignored", "--nocapture"])
                .env("MOMSO_FOCUS_PROBE_CHILD", "1")
                .env("MOMSO_PANEL_ALLOW_LOOPBACK", "1")
                .env("USERPROFILE", temp.path()).env("LOCALAPPDATA", local).env("APPDATA", roaming)
                .creation_flags(0x08000000).output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let app = tauri::Builder::<tauri::Wry>::default().any_thread().setup(move |app| {
            let window = tauri::window::WindowBuilder::new(app, "main")
                .visible(false).skip_taskbar(true).position(-20000.0, -20000.0).build()?;
            let hwnd = window.hwnd()?.0 as isize;
            let app = app.handle().clone();
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Runtime::new().unwrap();
                let result: Result<(), String> = runtime.block_on(async {
                    let foreground = unsafe { GetForegroundWindow() } as usize;
                    if foreground == 0 { return Err("foreground unavailable".into()); }
                    let focus = || {
                        let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
                        if unsafe { GetGUIThreadInfo(0, &mut info) } == 0 { return Err("keyboard focus unavailable".to_owned()); }
                        Ok(info.hwndFocus as usize)
                    };
                    let original_focus = focus()?;
                    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
                    let local_url = format!("http://{}/", listener.local_addr().map_err(|e| e.to_string())?);
                    for background_visible in [false, true] {
                        if background_visible {
                            let (shown_tx, shown_rx) = tokio::sync::oneshot::channel();
                            app.run_on_main_thread(move || {
                                unsafe { ShowWindow(hwnd as _, SW_SHOWNOACTIVATE); }
                                let _ = shown_tx.send(());
                            }).map_err(|e| e.to_string())?;
                            shown_rx.await.map_err(|e| e.to_string())?;
                        }
                        super::panel_browser_open_agent(app.clone(), 0.0, 0.0, 100.0, 100.0).await?;
                        super::panel_browser_bounds(app.clone(), 5.0, 5.0, 120.0, 120.0).await?;
                        super::panel_browser_visibility(app.clone(), false).await?;
                        super::panel_browser_visibility(app.clone(), true).await?;
                        super::panel_browser_navigate(app.clone(), local_url.clone()).await?;
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                        if unsafe { GetForegroundWindow() } as usize != foreground {
                            return Err(format!("foreground changed: background_visible={background_visible}"));
                        }
                        if focus()? != original_focus { return Err(format!("keyboard focus changed: background_visible={background_visible}")); }
                        super::panel_browser_close(app.clone()).await?;
                    }
                    Ok(())
                });
                let _ = tx.send(result);
                app.exit(0);
            });
            Ok(())
        }).build(tauri::test::mock_context(tauri::test::noop_assets())).unwrap();
        app.run(|_, _| {});
        rx.recv_timeout(std::time::Duration::from_secs(30))
            .unwrap()
            .unwrap();
    }

    #[test]
    fn cache_limit_counts_only_disk_cache() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("EBWebView/Default/Cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("fixture"), b"abcd").unwrap();
        std::fs::write(temp.path().join("cookies"), b"untouched cookie").unwrap();
        assert!(!super::cache_exceeds_limit(temp.path(), 4));
        assert!(super::cache_exceeds_limit(temp.path(), 3));
        assert!(temp.path().join("cookies").exists());
    }
    use super::*;

    #[test]
    fn panel_navigation_rejects_internal_schemes_and_hosts() {
        for address in [
            "asset://localhost/a",
            "tauri://localhost",
            "file:///C:/x",
            "http://localhost:1420",
            "http://127.0.0.1:1420",
            "https://tauri.localhost/",
            "https://user@example.com",
        ] {
            assert!(allowed_address(address).is_err(), "{address}");
        }
        assert!(allowed_address("https://example.com/").is_ok());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn development_loopback_is_literal_http_without_credentials() {
        for (url, expected) in [
            ("http://127.0.0.1:4567/", true),
            ("https://127.0.0.1/", false),
            ("http://localhost/", false),
            ("http://user@127.0.0.1/", false),
        ] {
            assert_eq!(
                development_loopback(&reqwest::Url::parse(url).unwrap()),
                expected
            );
        }
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn release_never_allows_loopback() {
        assert!(allowed_address("http://127.0.0.1:4567/").is_err());
    }

    #[test]
    fn google_login_detection_uses_exact_host() {
        assert!(is_google_login(
            &allowed_address("https://accounts.google.com/o/oauth2/auth").unwrap()
        ));
        assert!(!is_google_login(
            &allowed_address("https://accounts.google.com.example.com/").unwrap()
        ));
    }

    #[test]
    fn human_browser_has_dedicated_profile_and_no_debugging_port() {
        let url = allowed_address("https://example.com/").unwrap();
        let command = human_browser_command(
            std::path::Path::new("browser.exe"),
            std::path::Path::new("human-profile"),
            &url,
        );
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        assert!(args
            .iter()
            .any(|arg| arg == "--user-data-dir=human-profile"));
        assert!(args.iter().all(|arg| !arg.contains("remote-debugging")));
        assert_eq!(args.last().unwrap(), "https://example.com/");
    }
}
