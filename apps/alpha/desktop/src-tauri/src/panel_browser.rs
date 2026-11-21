//! Isolated, human-operated panel. Remote pages receive no application capability.
use tauri::{
    webview::{DownloadEvent, NewWindowResponse, PageLoadEvent, WebviewBuilder},
    Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder,
};

pub(crate) const LABEL: &str = "panel-browser";
static PANEL_SWITCH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) fn switch_lock() -> &'static tokio::sync::Mutex<()> {
    &PANEL_SWITCH
}

fn ensure_chat(chat_id: &str) -> Result<(), String> {
    let active = crate::panel_chat::active_chat();
    if chat_is_active(active.as_deref(), chat_id) {
        Ok(())
    } else {
        Err("panel_not_active".into())
    }
}

fn chat_is_active(active: Option<&str>, chat_id: &str) -> bool {
    active == Some(chat_id) || (cfg!(test) && active.is_none() && chat_id.is_empty())
}

pub(crate) fn current_panel(app: &tauri::AppHandle) -> Option<tauri::Webview> {
    let label = crate::panel_chat::active_label().or_else(|| {
        if cfg!(test) {
            Some(LABEL.to_owned())
        } else {
            None
        }
    })?;
    app.get_webview(&label)
}

/// The frontend calls this on every chat switch, before exposing its panel.
#[tauri::command]
pub(crate) async fn panel_browser_select_chat(
    app: tauri::AppHandle,
    chat_id: String,
) -> Result<serde_json::Value, String> {
    let _switch = PANEL_SWITCH.lock().await;
    crate::panel_chat::validate(&chat_id)?;
    let previous = crate::panel_chat::active_chat();
    if let Some(old) = previous.as_deref().filter(|old| *old != chat_id) {
        if let Some(state) = app.try_state::<crate::AppState>() {
            state.browser.panel_ownership.take_over_chat(old)?;
            let event_app = app.clone();
            let notify: crate::browser_runtime::Notify =
                std::sync::Arc::new(move |name, payload| {
                    let _ = event_app.emit(name, payload);
                });
            state.browser.close(old, &notify).await;
        }
        if let Some(view) = current_panel(&app) {
            view.hide().map_err(|_| "unavailable")?;
        }
    }
    let (old, label) = crate::panel_chat::select(&chat_id).map_err(str::to_owned)?;
    for candidate in crate::panel_chat::labels() {
        if let Some(view) = app.get_webview(&candidate) {
            if candidate == label {
                view.show().map_err(|_| "unavailable")?;
            } else {
                view.hide().map_err(|_| "unavailable")?;
            }
        }
    }
    let main_visible = app
        .get_window("main")
        .is_some_and(|window| window.is_visible().unwrap_or(false));
    for (popup_label, popup) in app.webview_windows() {
        if popup_label.starts_with(POPUP_PREFIX) {
            if main_visible && popup_belongs_to_active_chat(&popup_label) {
                let _ = popup.show();
            } else {
                let _ = popup.hide();
            }
        }
    }
    if let (Some(old), Some(state)) = (
        old.filter(|old| old != &chat_id),
        app.try_state::<crate::AppState>(),
    ) {
        state.browser.panel_ownership.resume(&old)?;
    }
    Ok(serde_json::json!({"chatId":chat_id,"hasPanel":app.get_webview(&label).is_some()}))
}

#[tauri::command]
pub(crate) async fn panel_browser_create_draft(
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let id = format!("draft-{}", uuid::Uuid::new_v4());
    panel_browser_select_chat(app, id).await
}

#[tauri::command]
pub(crate) async fn panel_browser_adopt_draft(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    draft_id: String,
    chat_id: String,
) -> Result<serde_json::Value, String> {
    let db = state.paths.database_path.clone();
    let target = chat_id.clone();
    tauri::async_runtime::spawn_blocking(move || crate::chat::read(&db, &target))
        .await
        .map_err(|_| "unavailable")?
        .map_err(|_| "chat_not_found")?;
    let _switch = PANEL_SWITCH.lock().await;
    crate::panel_chat::adopt(&draft_id, &chat_id).map_err(str::to_owned)?;
    Ok(serde_json::json!({"chatId":chat_id,"hasPanel":current_panel(&app).is_some()}))
}

pub(crate) fn navigation_gate() -> &'static crate::panel_navigation::PanelNavigation {
    static GATE: std::sync::OnceLock<crate::panel_navigation::PanelNavigation> =
        std::sync::OnceLock::new();
    GATE.get_or_init(Default::default)
}

const POPUP_PREFIX: &str = "panel-popup-";
const CACHE_LIMIT: u64 = 500 * 1024 * 1024;

struct OAuthPanelState {
    id: uuid::Uuid,
    state: String,
    external: tokio::sync::mpsc::UnboundedSender<()>,
    revoked: tokio_util::sync::CancellationToken,
}

fn oauth_panel_state() -> &'static std::sync::Mutex<Option<OAuthPanelState>> {
    static STATE: std::sync::OnceLock<std::sync::Mutex<Option<OAuthPanelState>>> =
        std::sync::OnceLock::new();
    STATE.get_or_init(|| std::sync::Mutex::new(None))
}

struct OAuthPanelGuard(uuid::Uuid);
impl Drop for OAuthPanelGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = oauth_panel_state().lock() {
            if state.as_ref().is_some_and(|active| active.id == self.0) {
                *state = None;
            }
        }
    }
}

fn oauth_callback_allowed(url: &reqwest::Url) -> bool {
    if url.scheme() != "http"
        || url.host_str() != Some("localhost")
        || url.port() != Some(8976)
        || url.path() != "/oauth/callback"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let pairs: Vec<_> = url.query_pairs().collect();
    let states: Vec<_> = pairs.iter().filter(|(key, _)| key == "state").collect();
    let codes: Vec<_> = pairs.iter().filter(|(key, _)| key == "code").collect();
    let errors: Vec<_> = pairs.iter().filter(|(key, _)| key == "error").collect();
    let result = (codes.len() == 1 && !codes[0].1.is_empty() && errors.is_empty())
        || (errors.len() == 1 && !errors[0].1.is_empty() && codes.is_empty());
    states.len() == 1
        && result
        && oauth_panel_state().lock().is_ok_and(|active| {
            active.as_ref().is_some_and(|active| {
                !active.state.is_empty()
                    && !active.revoked.is_cancelled()
                    && states[0].1 == active.state
            })
        })
}

fn oauth_external_login() -> bool {
    oauth_panel_state().lock().is_ok_and(|active| {
        active.as_ref().is_some_and(|active| {
            !active.revoked.is_cancelled() && active.external.send(()).is_ok()
        })
    })
}

pub(crate) struct OAuthPanel {
    _guard: OAuthPanelGuard,
    _lease: crate::panel_ownership::PanelLease,
    fallback: tokio::task::JoinHandle<()>,
}
impl Drop for OAuthPanel {
    fn drop(&mut self) {
        self.fallback.abort();
    }
}

pub(crate) async fn open_oauth_panel(
    app: tauri::AppHandle,
    chat_id: &str,
    url: reqwest::Url,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<OAuthPanel, &'static str> {
    let token = url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.is_empty())
        .ok_or("invalid_url")?;
    open_login_panel(app, chat_id, url, cancel, token).await
}

pub(crate) async fn open_device_login_panel(
    app: tauri::AppHandle,
    url: reqwest::Url,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<OAuthPanel, &'static str> {
    if !valid_device_login_url(&url) {
        return Err("invalid_url");
    }
    open_login_panel(app, "", url, cancel, String::new()).await
}

fn valid_device_login_url(url: &reqwest::Url) -> bool {
    let allowed = matches!(
        (url.host_str(), url.path()),
        (Some("auth.openai.com"), "/codex/device") | (Some("github.com"), "/login/device")
    );
    url.scheme() == "https"
        && allowed
        && url.port().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

fn login_panel_chat(
    active: Option<String>,
    requested: &str,
) -> Result<Option<String>, &'static str> {
    let Some(selected) = active else {
        return Ok(None);
    };
    if !requested.is_empty() && requested != selected {
        return Err("panel_not_active");
    }
    Ok(Some(selected))
}

async fn open_login_panel(
    app: tauri::AppHandle,
    chat_id: &str,
    url: reqwest::Url,
    cancel: tokio_util::sync::CancellationToken,
    token: String,
) -> Result<OAuthPanel, &'static str> {
    let state = app.state::<crate::AppState>();
    let lease = state
        .browser
        .panel_ownership
        .claim(&format!("oauth-{}", uuid::Uuid::new_v4()))?;
    let revoked = lease.cancellation();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let id = uuid::Uuid::new_v4();
    {
        let mut active = oauth_panel_state().lock().map_err(|_| "unavailable")?;
        if active.is_some() {
            return Err("panel_busy");
        }
        *active = Some(OAuthPanelState {
            id,
            state: token,
            external: sender,
            revoked: revoked.clone(),
        });
    }
    let guard = OAuthPanelGuard(id);
    if cancel.is_cancelled() || revoked.is_cancelled() {
        return Err("cancelled");
    }
    // Settings and first-run login can start before any chat has a panel.
    let selected_chat = login_panel_chat(crate::panel_chat::active_chat(), chat_id)?;
    if selected_chat.is_none() {
        panel_browser_open_human(app, url.to_string())
            .await
            .map_err(|_| "panel_unavailable")?;
        let fallback = tokio::spawn(async move {
            tokio::select! {
                _ = revoked.cancelled() => cancel.cancel(),
                _ = cancel.cancelled() => {},
            }
        });
        return Ok(OAuthPanel {
            _guard: guard,
            _lease: lease,
            fallback,
        });
    }
    let selected_chat = selected_chat.ok_or("panel_unavailable")?;
    app.emit_to(
        "main",
        "panel-agent-open-request",
        serde_json::json!({"chatId": selected_chat}),
    )
    .map_err(|_| "unavailable")?;
    let panel = async {
        loop {
            if crate::panel_chat::active_chat().as_deref() != Some(selected_chat.as_str()) {
                break Err("panel_not_active");
            }
            if current_panel(&app).is_some() {
                break Ok::<(), &'static str>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    };
    tokio::select! {
        _ = cancel.cancelled() => return Err("cancelled"),
        _ = revoked.cancelled() => return Err("cancelled"),
        result = tokio::time::timeout(std::time::Duration::from_secs(20), panel) => result.map_err(|_| "panel_unavailable")??,
    }
    if !navigation_gate().permits_navigation(url.as_str()) {
        return Err("panel_busy");
    }
    panel_browser_navigate(app.clone(), selected_chat, url.to_string())
        .await
        .map_err(|_| "panel_unavailable")?;
    let fallback = tokio::spawn(async move {
        tokio::select! {
            value = receiver.recv() => if value.is_some() {
                if panel_browser_open_human(app, url.to_string()).await.is_ok() {
                    revoked.cancelled().await;
                    cancel.cancel();
                } else { cancel.cancel(); }
            },
            _ = revoked.cancelled() => cancel.cancel(),
        }
    });
    Ok(OAuthPanel {
        _guard: guard,
        _lease: lease,
        fallback,
    })
}

#[cfg(windows)]
struct PopupInfo {
    chat_id: Option<String>,
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
        if crate::panel_chat::resolve(info.chat_id.clone()) != crate::panel_chat::active_chat() {
            return;
        }
        let _ = app.emit_to(
            "main",
            "panel-browser-popup",
            serde_json::json!({
                "chatId": crate::panel_chat::resolve(info.chat_id.clone()), "id": id, "state": state, "host": info.host, "openerHost": info.opener_host,
                "title": info.title, "reason": reason
            }),
        );
    }
}

#[cfg(windows)]
fn popup_belongs_to_active_chat(label: &str) -> bool {
    POPUPS
        .get_or_init(Default::default)
        .lock()
        .ok()
        .and_then(|popups| popups.get(label).cloned())
        .and_then(|info| {
            info.lock()
                .ok()
                .map(|info| crate::panel_chat::resolve(info.chat_id.clone()))
        })
        == Some(crate::panel_chat::active_chat())
}

#[cfg(not(windows))]
fn popup_belongs_to_active_chat(_label: &str) -> bool {
    false
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
    current_panel(app)
        .and_then(|view| view.url().ok())
        .filter(|url| allowed_address(url.as_str()).is_ok() && !is_google_login(url))
        .map(|url| url.to_string())
}

fn google_notice(
    app: &tauri::AppHandle,
    url: &reqwest::Url,
    site: Option<String>,
    chat_id: Option<String>,
) -> bool {
    if !is_google_login(url) {
        return false;
    }
    if oauth_external_login() {
        return true;
    }
    let _ = app.emit_to(
        "main",
        "panel-browser-google-login",
        serde_json::json!({"chatId": crate::panel_chat::resolve(chat_id), "host": "accounts.google.com", "siteUrl": site}),
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
    chat_id: String,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    ensure_chat(&chat_id)?;
    let url = allowed_address(&url).map_err(str::to_owned)?;
    open_panel(app, &chat_id, url, (x, y, width, height), true).await
}

/// Open an empty panel for an already-approved agent session; no site loads yet.
#[tauri::command]
pub(crate) async fn panel_browser_open_agent(
    app: tauri::AppHandle,
    chat_id: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let url = reqwest::Url::parse("about:blank").map_err(|_| "unavailable")?;
    open_panel(app, &chat_id, url, (x, y, width, height), false).await
}

#[cfg(any(windows, test))]
fn human_panel_may_focus(main: usize, foreground: usize, root: usize) -> bool {
    main != 0 && foreground != 0 && (main == foreground || main == root)
}

async fn open_panel(
    app: tauri::AppHandle,
    chat_id: &str,
    url: reqwest::Url,
    bounds: (f64, f64, f64, f64),
    human_requested: bool,
) -> Result<(), String> {
    let _switch = PANEL_SWITCH.lock().await;
    ensure_chat(chat_id)?;
    if human_requested && google_notice(&app, &url, None, Some(chat_id.to_owned())) {
        return Err("google_login_required".into());
    }
    let (x, y, width, height) = bounds;
    if ![x, y, width, height].iter().all(|v| v.is_finite()) || width <= 0.0 || height <= 0.0 {
        return Err("invalid_bounds".into());
    }
    let label = crate::panel_chat::active_label()
        .or_else(|| {
            if cfg!(test) {
                Some(LABEL.to_owned())
            } else {
                None
            }
        })
        .ok_or("chat_required")?;
    if app.get_webview(&label).is_some() {
        return Err("already_open".into());
    }
    #[cfg(test)]
    let fixture_profile =
        std::env::var_os("MOMSO_CHAT_PANEL_PROFILE").map(std::path::PathBuf::from);
    #[cfg(not(test))]
    let fixture_profile: Option<std::path::PathBuf> = None;
    #[cfg(all(windows, debug_assertions, feature = "g61-test-probe"))]
    let fixture_profile = if fixture_profile.is_some() {
        fixture_profile
    } else if let Some(root) = std::env::var_os("MOMSO_S5B_ROOT") {
        let root = std::path::PathBuf::from(root);
        if !root.is_absolute() || !root.starts_with(std::env::temp_dir()) {
            return Err("unavailable".into());
        }
        Some(root.join("panel-browser-profile"))
    } else {
        None
    };
    let profile = match fixture_profile {
        Some(path) => path,
        None => app
            .path()
            .app_data_dir()
            .map_err(|_| "unavailable")?
            .join("panel-browser-profile"),
    };
    std::fs::create_dir_all(&profile).map_err(|_| "unavailable")?;
    let window = app.get_window("main").ok_or("unavailable")?;
    let nav_app = app.clone();
    let popup_app = app.clone();
    let current_site = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let nav_site = current_site.clone();
    let popup_site = current_site.clone();
    let loaded_site = current_site;
    let owner = crate::panel_chat::active_chat();
    let navigation_owner = owner.clone();
    let download_owner = owner.clone();
    let popup_chat_owner = owner.clone();
    let title_owner = owner.clone();
    let page_owner = owner;
    let panel_view = window
        .add_child(
            WebviewBuilder::new(label, WebviewUrl::External(url))
                .data_directory(profile)
                // Creating an agent panel must not activate a background window.
                .focused(false)
                .on_navigation(move |url| crate::panel_chat::resolve(navigation_owner.clone()) == crate::panel_chat::active_chat() && navigation_gate().permits_navigation(url.as_str()) && (allowed_address(url.as_str()).is_ok() || oauth_callback_allowed(url)) && !google_notice(&nav_app, url, nav_site.lock().ok().and_then(|v| v.clone()), navigation_owner.clone()))
                .on_new_window(move |url, features| {
                    if crate::panel_chat::resolve(popup_chat_owner.clone()) != crate::panel_chat::active_chat() {
                        return NewWindowResponse::Deny;
                    }
                    if deny_agent_popup(&url) || allowed_address(url.as_str()).is_err() || google_notice(&popup_app, &url, popup_site.lock().ok().and_then(|v| v.clone()), popup_chat_owner.clone()) {
                        return NewWindowResponse::Deny;
                    }
                    #[cfg(windows)]
                    {
                        let Some(owner) = popup_app.get_window("main").and_then(|window| window.hwnd().ok()) else { return NewWindowResponse::Deny };
                        let info = std::sync::Arc::new(std::sync::Mutex::new(PopupInfo {
                            chat_id: popup_chat_owner.clone(),
                            host: url.host_str().map(str::to_owned),
                            opener_host: popup_site.lock().ok().and_then(|site| site.as_ref().and_then(|s| reqwest::Url::parse(s).ok()).and_then(|u| u.host_str().map(str::to_owned))),
                            title: None,
                        }));
                        let page_info = info.clone();
                        let title_info = info.clone();
                        let popup_nav_app = popup_app.clone();
                        let popup_nav_site = popup_site.clone();
                        let popup_nav_owner = popup_chat_owner.clone();
                        let popup_download_owner = popup_chat_owner.clone();
                        let builder = WebviewWindowBuilder::new(&popup_app, format!("{POPUP_PREFIX}{}", uuid::Uuid::new_v4()), WebviewUrl::External("about:blank".parse().expect("static URL")))
                            .window_features(features)
                            .owner_raw(owner)
                            .title(url.origin().ascii_serialization())
                            .on_navigation(move |url| crate::panel_chat::resolve(popup_nav_owner.clone()) == crate::panel_chat::active_chat() && allowed_address(url.as_str()).is_ok() && !google_notice(&popup_nav_app, url, popup_nav_site.lock().ok().and_then(|v| v.clone()), popup_nav_owner.clone()))
                            .on_page_load(move |window, payload| {
                                if let Ok(mut info) = page_info.lock() { info.host = payload.url().host_str().map(str::to_owned); }
                                let _ = window.set_title(&payload.url().origin().ascii_serialization());
                            })
                            .on_document_title_changed(move |_, title| { if let Ok(mut info) = title_info.lock() { info.title = Some(title); } })
                            .on_new_window(|_, _| NewWindowResponse::Deny)
                            .on_download(move |webview, event| match event {
                                DownloadEvent::Requested { url, destination } => crate::panel_download::start(webview.app_handle(), url.as_str(), destination, popup_download_owner.clone()),
                                DownloadEvent::Finished { url, path, success } => { crate::panel_download::finish(webview.app_handle().clone(), url.to_string(), path, success, popup_download_owner.clone()); true }
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
                .on_download(move |webview, event| {
                    match event {
                        DownloadEvent::Requested { url, destination } => crate::panel_chat::resolve(download_owner.clone()) == crate::panel_chat::active_chat() && crate::panel_download::start(webview.app_handle(), url.as_str(), destination, download_owner.clone()),
                        DownloadEvent::Finished { url, path, success } => {
                            crate::panel_download::finish(webview.app_handle().clone(), url.to_string(), path, success, download_owner.clone());
                            true
                        }
                        _ => false,
                    }
                })
                .on_document_title_changed(move |webview, title| {
                    if crate::panel_chat::resolve(title_owner.clone()) == crate::panel_chat::active_chat() {
                        let _ = webview.app_handle().emit_to("main", "panel-browser-title", serde_json::json!({"chatId":crate::panel_chat::resolve(title_owner.clone()),"title": title}));
                    }
                })
                .on_page_load(move |webview, payload| {
                    let url = payload.url();
                    if url.scheme() == "http" && url.host_str() == Some("localhost") && url.port() == Some(8976) && url.path() == "/oauth/callback" { return; }
                    if let Ok(mut site) = loaded_site.lock() { *site = Some(url.to_string()); }
                    if crate::panel_chat::resolve(page_owner.clone()) == crate::panel_chat::active_chat() {
                        let _ = webview.app_handle().emit_to("main", "panel-browser-page", serde_json::json!({
                            "chatId":crate::panel_chat::resolve(page_owner.clone()),"url": url.as_str(), "host": url.host_str(), "secure": url.scheme() == "https",
                            "loading": matches!(payload.event(), PageLoadEvent::Started)
                        }));
                        emit_history(&webview, page_owner.clone());
                    }
                }),
            LogicalPosition::new(x, y),
            LogicalSize::new(width, height),
        )
        .map_err(|_| if cfg!(test) { "test_webview_unavailable".to_owned() } else { "unavailable".to_owned() })?;
    crate::app_lifecycle::observe_webview(&panel_view);
    #[cfg(windows)]
    if human_requested {
        if let Ok(hwnd) = window.hwnd() {
            let main = hwnd.0 as usize;
            let _ = panel_view.with_webview(move |platform| {
                use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC;
                use windows_sys::Win32::UI::WindowsAndMessaging::{GetAncestor, GetForegroundWindow, GA_ROOT};
                let foreground = unsafe { GetForegroundWindow() };
                let root = unsafe { GetAncestor(foreground, GA_ROOT) };
                if human_panel_may_focus(main, foreground as usize, root as usize) {
                    let _ = unsafe { platform.controller().MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC) };
                }
            });
        }
    }
    #[cfg(not(windows))]
    if human_requested && window.is_focused().unwrap_or(false) {
        let _ = panel_view.set_focus();
    }
    Ok(())
}

fn panel(app: &tauri::AppHandle) -> Result<tauri::Webview, String> {
    current_panel(app).ok_or_else(|| "unavailable".into())
}

#[tauri::command]
pub(crate) async fn panel_browser_close(
    app: tauri::AppHandle,
    chat_id: String,
) -> Result<(), String> {
    let _switch = PANEL_SWITCH.lock().await;
    ensure_chat(&chat_id)?;
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
        if label.starts_with(POPUP_PREFIX) && popup_belongs_to_active_chat(&label) {
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
    let (view, temporary) = if let Some(view) = current_panel(&app) {
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
    chat_id: String,
    visible: bool,
) -> Result<(), String> {
    let _switch = PANEL_SWITCH.lock().await;
    ensure_chat(&chat_id)?;
    let main_visible = app
        .get_window("main")
        .is_some_and(|window| window.is_visible().unwrap_or(false));
    for (label, popup) in app.webview_windows() {
        if label.starts_with(POPUP_PREFIX) && popup_belongs_to_active_chat(&label) {
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
    chat_id: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let _switch = PANEL_SWITCH.lock().await;
    ensure_chat(&chat_id)?;
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
    chat_id: String,
    url: String,
) -> Result<(), String> {
    let _switch = PANEL_SWITCH.lock().await;
    ensure_chat(&chat_id)?;
    let url = allowed_address(&url).map_err(str::to_owned)?;
    let site = current_panel(&app)
        .and_then(|view| view.url().ok())
        .filter(|current| allowed_address(current.as_str()).is_ok() && !is_google_login(current))
        .map(|current| current.to_string());
    if google_notice(&app, &url, site, Some(chat_id.clone())) {
        return Err("google_login_required".into());
    }
    panel(&app)?.navigate(url).map_err(|_| "unavailable".into())
}

fn emit_history(view: &tauri::Webview, owner: Option<String>) {
    let app = view.app_handle().clone();
    let _ = view.with_webview(move |platform| {
        #[cfg(windows)]
        unsafe {
            if let Ok(core) = platform.controller().CoreWebView2() {
                let mut back = Default::default();
                let mut forward = Default::default();
                if crate::panel_chat::resolve(owner.clone()) == crate::panel_chat::active_chat() && core.CanGoBack(&mut back).is_ok() && core.CanGoForward(&mut forward).is_ok() {
                    let _ = app.emit_to("main", "panel-browser-history", serde_json::json!({"chatId":crate::panel_chat::resolve(owner),"canGoBack": back.as_bool(), "canGoForward": forward.as_bool()}));
                }
            }
        }
        #[cfg(not(windows))]
        let _ = (platform, app, owner);
    });
}

#[tauri::command]
pub(crate) async fn panel_browser_action(
    app: tauri::AppHandle,
    chat_id: String,
    action: String,
) -> Result<(), String> {
    let _switch = PANEL_SWITCH.lock().await;
    ensure_chat(&chat_id)?;
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
    fn ai_panel_navigation_click_then_read_real_webview() {
        use std::{
            io::{Read, Write},
            sync::{
                atomic::{AtomicUsize, Ordering},
                Arc,
            },
            time::Duration,
        };
        if std::env::var_os("MOMSO_CLICK_PROBE").is_none() {
            let temp = tempfile::tempdir().unwrap();
            let local = temp.path().join("local");
            let roaming = temp.path().join("roaming");
            std::fs::create_dir_all(&local).unwrap();
            std::fs::create_dir_all(&roaming).unwrap();
            let output = crate::hidden_process::hidden_command(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "panel_browser::tests::ai_panel_navigation_click_then_read_real_webview",
                    "--ignored",
                    "--nocapture",
                ])
                .env("MOMSO_CLICK_PROBE", "1")
                .env("MOMSO_CLICK_ROOT", temp.path())
                .env(
                    "MOMSO_CHAT_PANEL_PROFILE",
                    temp.path().join("panel-profile"),
                )
                .env("MOMSO_PANEL_ALLOW_LOOPBACK", "1")
                .env("MOMSO_TEST_FAKE_CREDENTIAL_STORE", "1")
                .env("USERPROFILE", temp.path())
                .env("LOCALAPPDATA", local)
                .env("APPDATA", roaming)
                .output()
                .unwrap();
            let result = std::fs::read_to_string(temp.path().join("result.txt"))
                .unwrap_or_else(|_| "result missing".into());
            if let Ok(detail) = std::fs::read_to_string(temp.path().join("public-observation.txt"))
            {
                println!("{detail}");
            }
            assert!(
                output.status.success() && result == "ok",
                "result={result}; stdout={}; stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        struct FixtureLogger;
        impl log::Log for FixtureLogger {
            fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
                metadata.level() <= log::Level::Warn
                    && (metadata.target().ends_with("panel_worker")
                        || metadata.target().ends_with("panel_cdp")
                        || metadata.target().ends_with("browser_runtime"))
            }
            fn log(&self, record: &log::Record<'_>) {
                if self.enabled(record.metadata()) {
                    eprintln!("{}", record.args());
                }
            }
            fn flush(&self) {}
        }
        static LOGGER: FixtureLogger = FixtureLogger;
        let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Warn));
        let public_site = std::env::var_os("MOMSO_CLICK_PUBLIC_SITE").is_some();
        let root = std::path::PathBuf::from(std::env::var_os("MOMSO_CLICK_ROOT").unwrap());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().take(40) {
                let Ok(mut stream) = stream else { break };
                let mut input = [0; 8192];
                if let Ok(n) = stream.read(&mut input) {
                    let request = String::from_utf8_lossy(&input[..n]);
                    let body = if request.starts_with("GET /next ") {
                        "<html><title>next page</title><body>next content</body></html>"
                    } else {
                        "<html><title>start page</title><body><button onclick=\"location.href='/next'\">pricing</button></body></html>"
                    };
                    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        });
        let db = root.join("momso.db");
        crate::storage::initialize(&db, &root.join("backups"), "fixture").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut context = tauri::generate_context!();
        context.config_mut().identifier = "app.momso.click.fixture".into();
        context.config_mut().app.windows.clear();
        let app = tauri::Builder::<tauri::Wry>::default().any_thread().setup(move |app| {
            tauri::window::WindowBuilder::new(app, "main").visible(false).skip_taskbar(true).position(-20000.0, -20000.0).build()?;
            let app = app.handle().clone();
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Runtime::new().unwrap();
                let result: Result<(), String> = runtime.block_on(async {
                    let chat = "click-fixture";
                    super::panel_browser_select_chat(app.clone(), chat.into()).await?;
                    super::panel_browser_open_agent(app.clone(), chat.into(), 0.0, 0.0, 300.0, 200.0).await?;
                    let manager = crate::browser_runtime::BrowserManager::default();
                    manager.bind_app(app.clone());
                    let policy = Ok(Some(crate::browser_gateway::Policy::general()));
                    let asks = Arc::new(AtomicUsize::new(0));
                    let count = asks.clone();
                    let ask: crate::browser_runtime::Ask = Arc::new(move |_, _| { count.fetch_add(1, Ordering::SeqCst); Box::pin(async { true }) });
                    let notify: crate::browser_runtime::Notify = Arc::new(|_, _| {});
                    let call = |input| manager.dispatch(crate::browser_runtime::RequestContext { chat_id: chat, provider: "fixture", skill: None, selected: &policy, root: root.clone(), visible: false, cancel: tokio_util::sync::CancellationToken::new(), notify: notify.clone(), ask: ask.clone() }, input);
                    let opened = call(serde_json::json!({"method":"browser_open","arguments":{}})).await;
                    if opened["status"] != "ok" { return Err(format!("open: {}", opened["status"])); }
                    let start_url = if public_site { "https://bodycodi.com/".to_owned() } else { format!("{base}/first") };
                    let page = call(serde_json::json!({"method":"browser_navigate","arguments":{"url":start_url}})).await;
                    if page["status"] != "page" { return Err(format!("navigate: {}", page["status"])); }
                    let previous_title = page["title"].clone();
                    let previous_text = page["text"].clone();
                    let id = page["elements"].as_array().and_then(|items| items.iter().find(|item| if public_site { item["name"].as_str().is_some_and(|name| name.contains("요금")) } else { item["name"] == "pricing" })).and_then(|item| item["id"].as_str()).ok_or("missing pricing menu")?.to_owned();
                    if public_site {
                        let transport = crate::panel_cdp::PanelTransport::bind(&app).map_err(str::to_owned)?;
                        let tree = transport.call("Page.getFrameTree", serde_json::json!({})).await.map_err(str::to_owned)?;
                        let frame = tree.pointer("/frameTree/frame/id").and_then(serde_json::Value::as_str).ok_or("frame unavailable")?;
                        let world = transport.call("Page.createIsolatedWorld", serde_json::json!({"frameId":frame,"worldName":"momso-browser-driver"})).await.map_err(str::to_owned)?;
                        let context = world["executionContextId"].as_i64().ok_or("world unavailable")?;
                        let quoted = serde_json::to_string(&id).map_err(|_| "id unavailable")?;
                        let expression = format!("(()=>{{const e=window.__momso_driver_state?.byId.get({quoted});if(!e)return 'unknown';if(e.tagName!=='A')return 'button';const t=e.getAttribute('target');return t==='_blank'?'link_blank':t?'link_other':'link_same'}})()");
                        let observation = transport.call("Runtime.evaluate", serde_json::json!({"expression":expression,"contextId":context,"returnByValue":true})).await.map_err(str::to_owned)?;
                        let target = observation.pointer("/result/value").and_then(serde_json::Value::as_str).unwrap_or("unknown");
                        std::fs::write(root.join("public-observation.txt"), format!("public_target={target}")).map_err(|_| "observation unavailable")?;
                    }
                    let clicked = call(serde_json::json!({"method":"browser_click","arguments":{"elementId":id}})).await;
                    if clicked["status"] != "ok" && clicked["status"] != "page" { return Err(format!("click: {}", clicked["status"])); }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let page = call(serde_json::json!({"method":"browser_read","arguments":{}})).await;
                    if page["status"] != "page" || (!public_site && page["title"] != "next page") { return Err(format!("read: {}", page["status"])); }
                    if public_site && page["title"] == previous_title && page["text"] == previous_text { return Err("pricing view did not change".into()); }
                    if asks.load(Ordering::SeqCst) != 1 { return Err("approval count".into()); }
                    if super::current_panel(&app).is_none() { return Err("panel closed".into()); }
                    if !public_site {
                        crate::panel_worker::fail_next_guard_for_test();
                        for _ in 0..100 {
                            if !manager.is_open(chat).await { break; }
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                        if manager.is_open(chat).await { return Err("guard did not stop".into()); }
                        if super::current_panel(&app).is_none() { return Err("guard failure closed panel".into()); }
                        super::panel_browser_navigate(app.clone(), chat.into(), format!("{base}/after-guard")).await?;
                    }
                    Ok(())
                });
                let text = match result { Ok(()) => "ok".to_owned(), Err(error) => format!("error: {error}") };
                let _ = std::fs::write(std::env::var_os("MOMSO_CLICK_ROOT").unwrap().to_string_lossy().to_string() + "\\result.txt", text);
                let _ = tx.send(());
                app.exit(0);
            });
            Ok(())
        }).build(context).unwrap();
        app.run(|_, _| {});
        rx.recv_timeout(Duration::from_secs(90)).unwrap();
    }

    #[test]
    fn login_routes_without_chat_and_respects_selected_chat() {
        assert_eq!(super::login_panel_chat(None, "").unwrap(), None);
        assert_eq!(
            super::login_panel_chat(Some("draft".into()), "")
                .unwrap()
                .as_deref(),
            Some("draft")
        );
        assert_eq!(
            super::login_panel_chat(Some("chat".into()), "other"),
            Err("panel_not_active")
        );
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "requires WebView2 and an interactive Windows desktop"]
    fn isolated_chat_panels_keep_pages_and_share_cookies() {
        use std::{
            io::{Read, Write},
            sync::{Arc, Mutex},
            time::{Duration, Instant},
        };
        if std::env::var_os("MOMSO_CHAT_PANEL_PROBE").is_none() {
            let temp = tempfile::tempdir().unwrap();
            let local = temp.path().join("local");
            let roaming = temp.path().join("roaming");
            std::fs::create_dir_all(&local).unwrap();
            std::fs::create_dir_all(&roaming).unwrap();
            let output = crate::hidden_process::hidden_command(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "panel_browser::tests::isolated_chat_panels_keep_pages_and_share_cookies",
                    "--ignored",
                    "--nocapture",
                ])
                .env("MOMSO_CHAT_PANEL_PROBE", "1")
                .env("MOMSO_CHAT_PANEL_RESULT", temp.path().join("result.txt"))
                .env(
                    "MOMSO_CHAT_PANEL_PROFILE",
                    temp.path().join("panel-profile"),
                )
                .env("MOMSO_PANEL_ALLOW_LOOPBACK", "1")
                .env("USERPROFILE", temp.path())
                .env("LOCALAPPDATA", local)
                .env("APPDATA", roaming)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                std::fs::read_to_string(temp.path().join("result.txt")).unwrap(),
                "ok"
            );
            return;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let server_seen = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(8) {
                let Ok(mut stream) = stream else { break };
                let mut buf = [0; 8192];
                if let Ok(n) = stream.read(&mut buf) {
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    server_seen.lock().unwrap().push(request);
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: text/plain\r\nSet-Cookie: fixture_chat=shared; Path=/\r\nConnection: close\r\n\r\nok");
                }
            }
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let mut context = tauri::generate_context!();
        context.config_mut().identifier = "app.momso.panel.fixture".into();
        context.config_mut().app.windows.clear();
        let app = tauri::Builder::<tauri::Wry>::default()
            .any_thread()
            .setup(move |app| {
                tauri::window::WindowBuilder::new(app, "main")
                    .visible(false)
                    .skip_taskbar(true)
                    .position(-20000.0, -20000.0)
                    .build()?;
                let app = app.handle().clone();
                std::thread::spawn(move || {
                    let runtime = tokio::runtime::Runtime::new().unwrap();
                    let result: Result<(), String> = runtime.block_on(async {
                        let draft = super::panel_browser_create_draft(app.clone()).await?["chatId"]
                            .as_str()
                            .ok_or("draft id missing")?
                            .to_owned();
                        super::panel_browser_open(
                            app.clone(),
                            draft.clone(),
                            format!("{base}/first"),
                            0.0,
                            0.0,
                            300.0,
                            200.0,
                        )
                        .await
                        .map_err(|e| format!("open first: {e}"))?;
                        let deadline = Instant::now() + Duration::from_secs(15);
                        while !seen
                            .lock()
                            .unwrap()
                            .iter()
                            .any(|r| r.starts_with("GET /first "))
                        {
                            if Instant::now() > deadline {
                                return Err("first page did not load".into());
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        crate::panel_chat::adopt(&draft, "first").map_err(str::to_owned)?;
                        super::panel_browser_navigate(
                            app.clone(),
                            "first".into(),
                            format!("{base}/first-again"),
                        )
                        .await?;
                        let deadline = Instant::now() + Duration::from_secs(15);
                        while !seen
                            .lock()
                            .unwrap()
                            .iter()
                            .any(|r| r.starts_with("GET /first-again "))
                        {
                            if Instant::now() > deadline {
                                return Err("adopted page did not load".into());
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                        super::panel_browser_select_chat(app.clone(), "second".into())
                            .await
                            .map_err(|e| format!("select second: {e}"))?;
                        super::panel_browser_open(
                            app.clone(),
                            "second".into(),
                            format!("{base}/second"),
                            0.0,
                            0.0,
                            300.0,
                            200.0,
                        )
                        .await
                        .map_err(|e| format!("open second: {e}"))?;
                        let deadline = Instant::now() + Duration::from_secs(15);
                        loop {
                            let second = seen
                                .lock()
                                .unwrap()
                                .iter()
                                .find(|r| r.starts_with("GET /second "))
                                .cloned();
                            if let Some(request) = second {
                                if !request.to_lowercase().contains("fixture_chat=shared") {
                                    return Err("shared cookie missing".into());
                                }
                                break;
                            }
                            if Instant::now() > deadline {
                                return Err("second page did not load".into());
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                        super::panel_browser_select_chat(app.clone(), "first".into())
                            .await
                            .map_err(|e| format!("return first: {e}"))?;
                        let first = super::current_panel(&app).ok_or("first panel missing")?;
                        if first.url().map_err(|e| e.to_string())?.path() != "/first-again" {
                            return Err("first page changed".into());
                        }
                        if super::panel_browser_navigate(
                            app.clone(),
                            "second".into(),
                            format!("{base}/wrong"),
                        )
                        .await
                        .is_ok()
                        {
                            return Err("inactive chat controlled active panel".into());
                        }
                        Ok(())
                    });
                    let text = match &result {
                        Ok(()) => "ok".to_owned(),
                        Err(error) => format!("error: {error}"),
                    };
                    let _ =
                        std::fs::write(std::env::var_os("MOMSO_CHAT_PANEL_RESULT").unwrap(), text);
                    let _ = tx.send(result);
                    app.exit(0);
                });
                Ok(())
            })
            .build(context)
            .unwrap();
        app.run(|_, _| {});
        rx.recv_timeout(Duration::from_secs(45)).unwrap().unwrap();
    }

    #[test]
    fn commands_from_inactive_chat_cannot_target_visible_panel() {
        assert!(super::chat_is_active(Some("chat_a"), "chat_a"));
        assert!(!super::chat_is_active(Some("chat_b"), "chat_a"));
        assert!(!super::chat_is_active(None, "chat_a"));
    }

    #[test]
    fn human_panel_focus_requires_own_foreground_window() {
        assert!(super::human_panel_may_focus(10, 10, 10));
        assert!(super::human_panel_may_focus(10, 11, 10));
        assert!(!super::human_panel_may_focus(10, 20, 20));
        assert!(!super::human_panel_may_focus(10, 0, 0));
        assert!(!super::human_panel_may_focus(0, 10, 10));
    }
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
                        super::panel_browser_open_agent(app.clone(), String::new(), 0.0, 0.0, 100.0, 100.0).await?;
                        super::panel_browser_bounds(app.clone(), String::new(), 5.0, 5.0, 120.0, 120.0).await?;
                        super::panel_browser_visibility(app.clone(), String::new(), false).await?;
                        super::panel_browser_visibility(app.clone(), String::new(), true).await?;
                        super::panel_browser_navigate(app.clone(), String::new(), local_url.clone()).await?;
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                        if unsafe { GetForegroundWindow() } as usize != foreground {
                            return Err(format!("foreground changed: background_visible={background_visible}"));
                        }
                        if focus()? != original_focus { return Err(format!("keyboard focus changed: background_visible={background_visible}")); }
                        super::panel_browser_close(app.clone(), String::new()).await?;
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
    fn device_login_only_accepts_fixed_provider_addresses() {
        for valid in [
            "https://auth.openai.com/codex/device",
            "https://github.com/login/device",
        ] {
            assert!(super::valid_device_login_url(
                &reqwest::Url::parse(valid).unwrap()
            ));
        }
        for invalid in [
            "http://github.com/login/device",
            "https://github.com.evil.test/login/device",
            "https://github.com/login/device?code=secret",
            "https://auth.openai.com/codex/device#fragment",
        ] {
            assert!(!super::valid_device_login_url(
                &reqwest::Url::parse(invalid).unwrap()
            ));
        }
    }
    #[tokio::test]
    async fn oauth_panel_allows_only_its_callback_and_requests_external_login() {
        use super::*;
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let id = uuid::Uuid::new_v4();
        let revoked = tokio_util::sync::CancellationToken::new();
        *oauth_panel_state().lock().unwrap() = Some(OAuthPanelState {
            id,
            state: "fixture-state".into(),
            external: sender,
            revoked: revoked.clone(),
        });
        let guard = OAuthPanelGuard(id);
        let good = reqwest::Url::parse(
            "http://localhost:8976/oauth/callback?code=fixture&state=fixture-state",
        )
        .unwrap();
        assert!(oauth_callback_allowed(&good));
        assert!(oauth_callback_allowed(
            &reqwest::Url::parse(
                "http://localhost:8976/oauth/callback?error=access_denied&state=fixture-state"
            )
            .unwrap()
        ));
        for value in [
            "http://localhost:8976/oauth/callback?code=x&state=wrong",
            "http://localhost:8977/oauth/callback?code=x&state=fixture-state",
            "http://127.0.0.1:8976/oauth/callback?code=x&state=fixture-state",
            "http://localhost:8976/oauth/callback?code=x&state=fixture-state&state=wrong",
            "http://localhost:8976/other?code=x&state=fixture-state",
        ] {
            assert!(!oauth_callback_allowed(
                &reqwest::Url::parse(value).unwrap()
            ));
        }
        assert!(oauth_external_login());
        assert_eq!(receiver.recv().await, Some(()));
        oauth_panel_state()
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .state
            .clear();
        assert!(!oauth_callback_allowed(
            &reqwest::Url::parse("http://localhost:8976/oauth/callback?code=x&state=").unwrap()
        ));
        revoked.cancel();
        assert!(!oauth_callback_allowed(&good));
        assert!(!oauth_external_login());
        drop(guard);
        assert!(!oauth_callback_allowed(&good));
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
