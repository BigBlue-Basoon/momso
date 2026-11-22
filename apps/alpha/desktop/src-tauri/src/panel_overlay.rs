//! App-owned popover above the external browser webview.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::{Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl};

const WINDOW: &str = "panel-overlay-window";
pub(crate) const WEBVIEW: &str = "panel-overlay";
const EXTENSION_WEBVIEW: &str = "panel-extension-popup";
static CLOSING: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OverlayInput {
    kind: String,
    chat_id: String,
    #[serde(default)]
    payload: Option<serde_json::Value>,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Clone, serde::Serialize)]
struct OpenOverlay {
    id: uuid::Uuid,
    #[serde(flatten)]
    input: OverlayInput,
}

fn state() -> &'static Mutex<Option<OpenOverlay>> {
    static STATE: OnceLock<Mutex<Option<OpenOverlay>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

fn open_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn geometry_valid(input: &OverlayInput) -> bool {
    [input.x, input.y, input.width, input.height]
        .iter()
        .all(|v| v.is_finite())
        && input.width > 0.0
        && input.height > 0.0
}

fn local_navigation(base: &tauri::Url, url: &tauri::Url) -> bool {
    url.scheme() == base.scheme()
        && url.host_str() == base.host_str()
        && url.port_or_known_default() == base.port_or_known_default()
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(url.path(), "/" | "/index.html")
}

fn extension_navigation(extension_id: &str, url: &tauri::Url) -> bool {
    url.scheme() == "chrome-extension"
        && url.host_str() == Some(extension_id)
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
}

fn route_extension_link(app: tauri::AppHandle, chat: String, url: &tauri::Url) {
    if !matches!(url.scheme(), "http" | "https") {
        return;
    }
    let address = url.to_string();
    tauri::async_runtime::spawn(async move {
        let _ = crate::panel_browser::panel_browser_navigate(app, chat, address).await;
    });
}

fn panel_profile(app: &tauri::AppHandle) -> Result<std::path::PathBuf, &'static str> {
    #[cfg(all(windows, debug_assertions, feature = "g61-test-probe"))]
    if app
        .config()
        .identifier
        .starts_with("app.momso.s5b.fixture.")
    {
        let root =
            std::path::PathBuf::from(std::env::var_os("MOMSO_S5B_ROOT").ok_or("unavailable")?);
        if !root.is_absolute() || !root.starts_with(std::env::temp_dir()) {
            return Err("unavailable");
        }
        return Ok(root.join("panel-browser-profile"));
    }
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|_| "unavailable")?
        .join("panel-browser-profile"))
}

#[cfg(windows)]
fn foreground_matches(app: &tauri::AppHandle, labels: &[&str]) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetAncestor, GetForegroundWindow, GA_ROOT};
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return false;
    }
    let root = unsafe { GetAncestor(foreground, GA_ROOT) } as usize;
    labels.iter().any(|label| {
        app.get_window(label)
            .and_then(|window| window.hwnd().ok())
            .is_some_and(|hwnd| hwnd.0 as usize == root)
    })
}

#[cfg(windows)]
fn foreground_inside_overlay(app: &tauri::AppHandle) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetForegroundWindow, GetWindow, GA_ROOT, GW_OWNER,
    };
    let Some(overlay) = app.get_window(WINDOW).and_then(|window| window.hwnd().ok()) else {
        return false;
    };
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return false;
    }
    let mut window = unsafe { GetAncestor(foreground, GA_ROOT) };
    for _ in 0..32 {
        if window.is_null() {
            break;
        }
        if std::ptr::eq(window, overlay.0) {
            return true;
        }
        window = unsafe { GetWindow(window, GW_OWNER) };
    }
    false
}

#[cfg(not(windows))]
fn foreground_inside_overlay(app: &tauri::AppHandle) -> bool {
    foreground_matches(app, &[WINDOW])
}

#[cfg(windows)]
fn show_without_activation(window: &tauri::Window) -> Result<(), &'static str> {
    let hwnd = window.hwnd().map_err(|_| "unavailable")?;
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
            hwnd.0 as _,
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE,
        );
    }
    Ok(())
}

#[cfg(not(windows))]
fn show_without_activation(window: &tauri::Window) -> Result<(), &'static str> {
    window.show().map_err(|_| "unavailable")
}

#[cfg(not(windows))]
fn foreground_matches(app: &tauri::AppHandle, labels: &[&str]) -> bool {
    labels.iter().any(|label| {
        app.get_window(label)
            .is_some_and(|window| window.is_focused().unwrap_or(false))
    })
}

fn dismiss_matching(
    app: tauri::AppHandle,
    reason: &'static str,
    restore_focus: bool,
    id: Option<uuid::Uuid>,
) {
    let Ok(mut guard) = state().lock() else {
        return;
    };
    if id.is_some_and(|id| guard.as_ref().is_none_or(|open| open.id != id)) {
        return;
    }
    let Some(current) = guard.take() else {
        return;
    };
    CLOSING.fetch_add(1, Ordering::SeqCst);
    drop(guard);
    let can_restore_focus = restore_focus && foreground_matches(&app, &[WINDOW, "main"]);
    if let Some(window) = app.get_window(WINDOW) {
        if let Err(error) = window.destroy() {
            log::warn!("panel_overlay_destroy_failed error={error}");
        }
    }
    CLOSING.fetch_sub(1, Ordering::SeqCst);
    let _ = app.emit_to(
        tauri::EventTarget::webview("main"),
        "panel-overlay-closed",
        serde_json::json!({"id":current.id,"reason":reason}),
    );
    if can_restore_focus && foreground_matches(&app, &[WINDOW, "main"]) {
        if let Some(main) = app.get_window("main") {
            if main.is_visible().unwrap_or(false) {
                let _ = main.set_focus();
                if let Some(view) = app.get_webview("main") {
                    let _ = view.set_focus();
                }
            }
        }
    }
}

pub(crate) fn dismiss(app: tauri::AppHandle, reason: &'static str, restore_focus: bool) {
    dismiss_matching(app, reason, restore_focus, None);
}

pub(crate) fn main_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if matches!(
        event,
        tauri::WindowEvent::Moved(_)
            | tauri::WindowEvent::Resized(_)
            | tauri::WindowEvent::ScaleFactorChanged { .. }
            | tauri::WindowEvent::CloseRequested { .. }
    ) {
        let Some(id) = state()
            .lock()
            .ok()
            .and_then(|value| value.as_ref().map(|open| open.id))
        else {
            return;
        };
        let app = window.app_handle().clone();
        tauri::async_runtime::spawn(async move {
            dismiss_matching(app, "window_changed", false, Some(id));
        });
    }
}

#[tauri::command]
pub(crate) async fn panel_overlay_open(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    input: OverlayInput,
) -> Result<serde_json::Value, &'static str> {
    open_overlay(app, webview, input, None).await
}

#[tauri::command]
pub(crate) async fn panel_extension_popup_open(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    extension_id: String,
    input: OverlayInput,
) -> Result<serde_json::Value, &'static str> {
    if webview.label() != "main" {
        return Err("unavailable");
    }
    let popup =
        crate::panel_extension_catalog::panel_extensions_popup(app.clone(), extension_id.clone())
            .await?;
    let url = tauri::Url::parse(popup["url"].as_str().ok_or("invalid_popup")?)
        .map_err(|_| "invalid_popup")?;
    if !extension_navigation(&extension_id, &url) {
        return Err("invalid_popup");
    }
    open_overlay(app, webview, input, Some((extension_id, url))).await
}

async fn open_overlay(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    input: OverlayInput,
    extension: Option<(String, tauri::Url)>,
) -> Result<serde_json::Value, &'static str> {
    if webview.label() != "main" {
        return Err("unavailable");
    }
    let _open = open_lock().lock().await;
    if !geometry_valid(&input) {
        return Err("invalid_bounds");
    }
    if crate::panel_chat::active_chat().as_deref() != Some(input.chat_id.as_str()) {
        return Err("panel_not_active");
    }
    let main = app.get_window("main").ok_or("unavailable")?;
    if !main.is_visible().map_err(|_| "unavailable")? {
        return Err("unavailable");
    }
    let base = webview.url().map_err(|_| "unavailable")?;
    let activate = foreground_matches(&app, &["main"]);
    if !matches!(base.scheme(), "tauri" | "http" | "https") {
        return Err("unavailable");
    }
    let position = main.inner_position().map_err(|_| "unavailable")?;
    let scale = main.scale_factor().map_err(|_| "unavailable")?;
    dismiss(app.clone(), "replaced", false);
    for _ in 0..80 {
        if CLOSING.load(Ordering::SeqCst) == 0 && app.get_window(WINDOW).is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    if CLOSING.load(Ordering::SeqCst) != 0 || app.get_window(WINDOW).is_some() {
        return Err("unavailable");
    }
    let id = uuid::Uuid::new_v4();
    *state().lock().map_err(|_| "unavailable")? = Some(OpenOverlay {
        id,
        input: input.clone(),
    });
    let result = (|| {
        let window = tauri::window::WindowBuilder::new(&app, WINDOW)
            .parent(&main)
            .map_err(|_| "unavailable")?
            .title("momso")
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .skip_taskbar(true)
            .resizable(false)
            .visible(false)
            .position(
                position.x as f64 / scale + input.x,
                position.y as f64 / scale + input.y,
            )
            .inner_size(input.width, input.height)
            .build()
            .map_err(|_| "unavailable")?;
        let navigation_base = base.clone();
        #[cfg(all(windows, debug_assertions, feature = "g61-test-probe"))]
        let is_extension = extension.is_some();
        let builder = if let Some((extension_id, url)) = extension {
            let profile = panel_profile(&app)?;
            std::fs::create_dir_all(&profile).map_err(|_| "unavailable")?;
            let nav_app = app.clone();
            let nav_chat = input.chat_id.clone();
            let new_app = app.clone();
            let new_chat = input.chat_id.clone();
            tauri::webview::WebviewBuilder::new(EXTENSION_WEBVIEW, WebviewUrl::External(url))
                .data_directory(profile)
                .browser_extensions_enabled(true)
                .focused(activate)
                .on_navigation(move |url| {
                    if extension_navigation(&extension_id, url) {
                        true
                    } else {
                        route_extension_link(nav_app.clone(), nav_chat.clone(), url);
                        false
                    }
                })
                .on_new_window(move |url, _| {
                    route_extension_link(new_app.clone(), new_chat.clone(), &url);
                    tauri::webview::NewWindowResponse::Deny
                })
        } else {
            tauri::webview::WebviewBuilder::new(WEBVIEW, WebviewUrl::App("index.html".into()))
                .transparent(true)
                .focused(activate)
                .on_navigation(move |url| local_navigation(&navigation_base, url))
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        };
        #[cfg(all(windows, debug_assertions, feature = "g61-test-probe"))]
        let builder = if app
            .config()
            .identifier
            .starts_with("app.momso.s5b.fixture.")
            && !is_extension
        {
            let root = std::env::var_os("MOMSO_S5B_ROOT")
                .map(std::path::PathBuf::from)
                .ok_or("unavailable")?;
            if !root.is_absolute() || !root.starts_with(std::env::temp_dir()) {
                return Err("unavailable");
            }
            builder.data_directory(root.join("main-webview"))
        } else {
            builder
        };
        let overlay_view = match window.add_child(
            builder,
            LogicalPosition::new(0.0, 0.0),
            LogicalSize::new(input.width, input.height),
        ) {
            Ok(view) => view,
            Err(_) => {
                let _ = window.destroy();
                return Err("unavailable");
            }
        };
        if state()
            .lock()
            .ok()
            .and_then(|value| value.as_ref().map(|open| open.id))
            != Some(id)
            || main.inner_position().ok() != Some(position)
            || main.scale_factor().ok() != Some(scale)
            || crate::panel_chat::active_chat().as_deref() != Some(input.chat_id.as_str())
            || !main.is_visible().unwrap_or(false)
        {
            let _ = window.destroy();
            return Err("unavailable");
        }
        let listener_app = app.clone();
        window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Focused(false)) {
                let app = listener_app.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    if !foreground_inside_overlay(&app) {
                        dismiss_matching(app, "outside", false, Some(id));
                    }
                });
            } else if matches!(event, tauri::WindowEvent::Destroyed) {
                let app = listener_app.clone();
                tauri::async_runtime::spawn(async move {
                    dismiss_matching(app, "closed", false, Some(id));
                });
            }
        });
        if activate && foreground_matches(&app, &["main"]) {
            window.show().map_err(|_| "unavailable")?;
            window.set_focus().map_err(|_| "unavailable")?;
            overlay_view.set_focus().map_err(|_| "unavailable")?;
        } else {
            show_without_activation(&window)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        dismiss_matching(app.clone(), "failed", false, Some(id));
        return Err(error);
    }
    let chat = input.chat_id.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            if state()
                .lock()
                .ok()
                .and_then(|value| value.clone())
                .is_none_or(|v| v.id != id)
            {
                break;
            }
            if crate::panel_chat::active_chat().as_deref() != Some(chat.as_str()) {
                dismiss_matching(app, "chat_changed", false, Some(id));
                break;
            }
            if !app
                .get_window("main")
                .is_some_and(|window| window.is_visible().unwrap_or(false))
            {
                dismiss_matching(app, "window_changed", false, Some(id));
                break;
            }
        }
    });
    Ok(serde_json::json!({"id":id}))
}

#[tauri::command]
pub(crate) fn panel_overlay_state(
    webview: tauri::Webview,
) -> Result<serde_json::Value, &'static str> {
    if webview.label() != WEBVIEW && webview.label() != "main" {
        return Err("unavailable");
    }
    state()
        .lock()
        .map(|value| serde_json::json!(value.as_ref()))
        .map_err(|_| "unavailable")
}

#[tauri::command]
pub(crate) async fn panel_overlay_close(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    id: uuid::Uuid,
) -> Result<(), &'static str> {
    if webview.label() != WEBVIEW && webview.label() != "main" {
        return Err("unavailable");
    }
    dismiss_matching(app, "closed", true, Some(id));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_must_be_finite_and_nonempty() {
        let mut input = OverlayInput {
            kind: "fixture".into(),
            chat_id: "chat".into(),
            payload: None,
            x: 0.0,
            y: 0.0,
            width: 320.0,
            height: 220.0,
        };
        assert!(geometry_valid(&input));
        input.width = 0.0;
        assert!(!geometry_valid(&input));
        input.width = 100.0;
        input.x = f64::NAN;
        assert!(!geometry_valid(&input));
    }

    #[test]
    fn overlay_payload_round_trips_and_old_calls_remain_valid() {
        let old = serde_json::json!({"kind":"saved-logins","chatId":"chat","x":1,"y":2,"width":300,"height":200});
        let old_input: OverlayInput = serde_json::from_value(old.clone()).unwrap();
        assert!(old_input.payload.is_none());
        let payload = serde_json::json!({"site":"example.test","url":"https://example.test/path","panel":{"x":10,"width":300}});
        let mut next = old;
        next["payload"] = payload.clone();
        let input: OverlayInput = serde_json::from_value(next).unwrap();
        let output = serde_json::to_value(OpenOverlay {
            id: uuid::Uuid::nil(),
            input,
        })
        .unwrap();
        assert_eq!(output["kind"], "saved-logins");
        assert_eq!(output["payload"], payload);
    }

    #[test]
    fn navigation_stays_on_app_origin_and_entry_page() {
        let base = tauri::Url::parse("http://localhost:1420/").unwrap();
        assert!(local_navigation(
            &base,
            &tauri::Url::parse("http://localhost:1420/index.html").unwrap()
        ));
        for value in [
            "https://example.test/",
            "http://localhost:1421/",
            "http://localhost:1420/other",
            "http://user@localhost:1420/",
        ] {
            assert!(!local_navigation(&base, &tauri::Url::parse(value).unwrap()));
        }
    }

    #[test]
    fn extension_popup_stays_on_its_own_extension_origin() {
        let id = "abcdefghijklmnopabcdefghijklmnop";
        assert!(extension_navigation(
            id,
            &tauri::Url::parse(&format!("chrome-extension://{id}/popup.html")).unwrap()
        ));
        for value in [
            "https://example.test/",
            "chrome-extension://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/popup.html",
            "file:///C:/test.html",
            "chrome-extension://user@abcdefghijklmnopabcdefghijklmnop/popup.html",
        ] {
            assert!(!extension_navigation(
                id,
                &tauri::Url::parse(value).unwrap()
            ));
        }
    }
}
