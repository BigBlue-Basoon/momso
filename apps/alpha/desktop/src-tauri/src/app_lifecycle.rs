//! Lifecycle diagnostics contain event kinds and codes, never page content or paths.

#[cfg(windows)]
use tauri::Manager;

#[cfg(windows)]
const RECOVERY_INTERVAL_SECS: u64 = 120;

#[cfg(windows)]
static RECOVERY_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
fn recovery_file(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    #[cfg(all(debug_assertions, feature = "g61-test-probe"))]
    if app
        .config()
        .identifier
        .starts_with("app.momso.s5b.fixture.")
    {
        let root = std::path::PathBuf::from(std::env::var_os("MOMSO_S5B_ROOT")?);
        if root.is_absolute() && root.starts_with(std::env::temp_dir()) {
            return Some(root.join("webview-recovery"));
        }
        return None;
    }
    Some(app.path().app_config_dir().ok()?.join("webview-recovery"))
}

#[cfg(windows)]
fn recovery_record(text: &str) -> Option<(u64, bool, bool)> {
    let mut parts = text.split_whitespace();
    let time = parts.next()?.parse().ok()?;
    let hidden = match parts.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let pending = match parts.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((time, hidden, pending))
}

#[cfg(windows)]
fn should_recover(label: &str, kind: Option<i32>) -> bool {
    label == "main"
        && kind
            == Some(
                webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED.0,
            )
}

#[cfg(windows)]
fn now_secs() -> Option<u64> {
    Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs(),
    )
}

#[cfg(windows)]
fn request_webview_recovery(app: tauri::AppHandle) {
    use std::sync::atomic::Ordering;
    if RECOVERY_REQUESTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        // Leave the WebView2 event handler before touching window or application state.
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let Some(path) = recovery_file(&app) else {
            log::error!("lifecycle webview_recovery_stopped reason=state_unavailable");
            return;
        };
        let Some(now) = now_secs() else {
            log::error!("lifecycle webview_recovery_stopped reason=clock_unavailable");
            return;
        };
        match std::fs::read_to_string(&path) {
            Ok(previous) => {
                let Some((last, _, _)) = recovery_record(&previous) else {
                    log::error!("lifecycle webview_recovery_stopped reason=invalid_state");
                    return;
                };
                if now.saturating_sub(last) < RECOVERY_INTERVAL_SECS {
                    log::error!("lifecycle webview_recovery_stopped reason=repeated_failure");
                    return;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                log::error!("lifecycle webview_recovery_stopped reason=state_read_failed");
                return;
            }
        }
        let Some(window) = app.get_window("main") else {
            log::error!("lifecycle webview_recovery_stopped reason=window_missing");
            return;
        };
        let hidden = match window.is_visible() {
            Ok(visible) => !visible,
            Err(_) => {
                log::error!("lifecycle webview_recovery_stopped reason=visibility_unavailable");
                return;
            }
        };
        if path
            .parent()
            .is_none_or(|parent| std::fs::create_dir_all(parent).is_err())
            || std::fs::write(&path, format!("{now} {} 1", u8::from(hidden))).is_err()
        {
            log::error!("lifecycle webview_recovery_stopped reason=state_write_failed");
            return;
        }
        log::warn!("lifecycle webview_recovery_restart hidden={hidden}");
        app.request_restart();
    });
}

#[cfg(windows)]
pub(crate) fn restore_recovery_visibility(app: &tauri::AppHandle) {
    let Some(path) = recovery_file(app) else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Some((time, hidden, true)) = recovery_record(&text) else {
        return;
    };
    if now_secs().is_none_or(|now| now.saturating_sub(time) >= RECOVERY_INTERVAL_SECS) {
        return;
    }
    if hidden {
        if let Some(window) = app.get_window("main") {
            if let Err(error) = window.hide() {
                log::warn!("lifecycle webview_recovery_hide_failed error={error}");
            }
        }
    }
    if std::fs::write(path, format!("{time} {} 0", u8::from(hidden))).is_err() {
        log::warn!("lifecycle webview_recovery_state_consume_failed");
    }
}

#[cfg(not(windows))]
pub(crate) fn restore_recovery_visibility(_app: &tauri::AppHandle) {}

pub(crate) fn log_run_event(event: &tauri::RunEvent) {
    match event {
        tauri::RunEvent::ExitRequested { code, .. } => log::info!(
            "lifecycle exit_requested pid={} code={code:?}",
            std::process::id()
        ),
        tauri::RunEvent::Exit => log::info!(
            "lifecycle exit pid={} code=not_provided",
            std::process::id()
        ),
        _ => {}
    }
}

#[cfg(windows)]
pub(crate) fn observe_webview(view: &tauri::Webview) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2ProcessFailedEventArgs2, COREWEBVIEW2_PROCESS_FAILED_KIND,
        COREWEBVIEW2_PROCESS_FAILED_REASON,
    };
    use windows_core::Interface;
    let label = view.label().to_owned();
    let app = view.app_handle().clone();
    let failed_label = label.clone();
    if let Err(error) = view.with_webview(move |platform| unsafe {
        let callback_label = label.clone();
        let handler = webview2_com::ProcessFailedEventHandler::create(Box::new(move |_, args| {
            let mut kind = None;
            let mut reason = None;
            let mut code = None;
            if let Some(args) = args {
                let mut value = COREWEBVIEW2_PROCESS_FAILED_KIND(0);
                if args.ProcessFailedKind(&mut value).is_ok() { kind = Some(value.0); }
                if let Ok(args) = args.cast::<ICoreWebView2ProcessFailedEventArgs2>() {
                    let mut value = COREWEBVIEW2_PROCESS_FAILED_REASON(0);
                    if args.Reason(&mut value).is_ok() { reason = Some(value.0); }
                    let mut value = 0;
                    if args.ExitCode(&mut value).is_ok() { code = Some(value); }
                }
            }
            log::error!("lifecycle webview_process_failed label={callback_label} kind={kind:?} reason={reason:?} code={code:?}");
            if should_recover(&callback_label, kind) {
                request_webview_recovery(app.clone());
            }
            Ok(())
        }));
        let mut token = 0;
        if let Err(error) = platform.controller().CoreWebView2().and_then(|core| core.add_ProcessFailed(&handler, &mut token)) {
            log::warn!("lifecycle webview_process_listener_failed label={label} error={error}");
        }
    }) {
        log::warn!("lifecycle webview_access_failed label={failed_label} error={error}");
    }
}

#[cfg(not(windows))]
pub(crate) fn observe_webview(_view: &tauri::Webview) {}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn recovery_only_targets_main_browser_exit() {
        assert!(should_recover("main", Some(0)));
        assert!(!should_recover("main", Some(1)));
        assert!(!should_recover("panel-browser", Some(0)));
        assert!(!should_recover("main", None));
    }

    #[test]
    fn recovery_record_rejects_invalid_or_ambiguous_state() {
        assert_eq!(recovery_record("42 1 1"), Some((42, true, true)));
        assert_eq!(recovery_record("42 0 0"), Some((42, false, false)));
        for value in ["", "bad 1 1", "42 2 1", "42 0 2", "42 0 1 extra"] {
            assert_eq!(recovery_record(value), None);
        }
    }
}
