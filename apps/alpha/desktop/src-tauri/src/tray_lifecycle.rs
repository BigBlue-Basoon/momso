use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};

const OPEN: &str = "momso_tray_open";
const QUIT: &str = "momso_tray_quit";
static QUITTING: AtomicBool = AtomicBool::new(false);

struct PendingExit {
    id: String,
    reply: Option<tokio::sync::oneshot::Sender<bool>>,
}

#[derive(Default)]
pub(crate) struct ExitBroker(Mutex<Option<PendingExit>>);

impl ExitBroker {
    fn start(&self) -> Option<(String, tokio::sync::oneshot::Receiver<bool>)> {
        let mut pending = self.0.lock().ok()?;
        if pending.is_some() {
            return None;
        }
        let id = uuid::Uuid::new_v4().to_string();
        let (reply, receiver) = tokio::sync::oneshot::channel();
        *pending = Some(PendingExit {
            id: id.clone(),
            reply: Some(reply),
        });
        Some((id, receiver))
    }

    fn respond(&self, id: &str, approved: bool) -> &'static str {
        let Ok(mut pending) = self.0.lock() else {
            return "unavailable";
        };
        let Some(value) = pending.as_mut().filter(|value| value.id == id) else {
            return "stale";
        };
        let Some(reply) = value.reply.take() else {
            return "stale";
        };
        if reply.send(approved).is_ok() {
            "accepted"
        } else {
            "stale"
        }
    }

    fn cancel(&self, id: &str) {
        if let Ok(mut pending) = self.0.lock() {
            if pending.as_ref().is_some_and(|value| value.id == id) {
                pending.take();
            }
        }
    }
}

#[tauri::command]
pub(crate) fn app_exit_respond(
    broker: tauri::State<'_, ExitBroker>,
    request_id: String,
    decision: String,
) -> serde_json::Value {
    let approved = match decision.as_str() {
        "exit" => true,
        "stay" => false,
        _ => return serde_json::json!({"state":"invalid"}),
    };
    serde_json::json!({"state":broker.respond(&request_id, approved)})
}

pub(crate) fn hide_on_close<R: tauri::Runtime>(
    window: &tauri::Window<R>,
    api: &tauri::CloseRequestApi,
) {
    api.prevent_close();
    if let Err(error) = window.hide() {
        log::warn!("lifecycle main_window_hide_failed error={error}");
    } else {
        log::info!("lifecycle main_window_hidden");
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Open,
    Quit,
}

fn menu_action(id: &str) -> Option<Action> {
    match id {
        OPEN => Some(Action::Open),
        QUIT => Some(Action::Quit),
        _ => None,
    }
}

fn is_open_click(button: MouseButton, state: MouseButtonState) -> bool {
    button == MouseButton::Left && state == MouseButtonState::Up
}

fn begin_exit(flag: &AtomicBool) -> bool {
    !flag.swap(true, Ordering::SeqCst)
}

pub(crate) fn show_main(app: &tauri::AppHandle) {
    let Some(window) = app.get_window("main") else {
        log::warn!("lifecycle main_window_show_failed reason=window_missing");
        return;
    };
    if let Err(error) = window.unminimize() {
        log::warn!("lifecycle main_window_unminimize_failed error={error}");
    }
    if let Err(error) = window.show() {
        log::warn!("lifecycle main_window_show_failed error={error}");
        return;
    }
    if let Err(error) = window.set_focus() {
        log::warn!("lifecycle main_window_focus_failed error={error}");
    }
}

pub(crate) fn is_test_exit(args: &[String], enabled: bool) -> bool {
    enabled && args.iter().any(|arg| arg == "--momso-test-exit")
}

pub(crate) fn request_exit(app: tauri::AppHandle) {
    let (id, receiver) = {
        let Some(broker) = app.try_state::<ExitBroker>() else {
            log::warn!("lifecycle tray_exit_request_failed reason=broker_unavailable");
            return;
        };
        show_main(&app);
        let Some((id, receiver)) = broker.start() else {
            return;
        };
        let event = serde_json::json!({"requestId":id});
        if app
            .emit_to(
                tauri::EventTarget::webview("main"),
                "app-exit-requested",
                event,
            )
            .is_err()
        {
            broker.cancel(&id);
            log::warn!("lifecycle tray_exit_request_failed reason=event_unavailable");
            return;
        }
        (id, receiver)
    };
    tauri::async_runtime::spawn(async move {
        let answer = tokio::time::timeout(std::time::Duration::from_secs(300), receiver).await;
        let reason = match answer {
            Ok(Ok(true)) => "approved",
            Ok(Ok(false)) => "stayed",
            _ => "timeout",
        };
        if let Some(broker) = app.try_state::<ExitBroker>() {
            broker.cancel(&id);
        }
        let _ = app.emit_to(
            tauri::EventTarget::webview("main"),
            "app-exit-request-closed",
            serde_json::json!({"requestId":id,"reason":reason}),
        );
        if reason == "approved" {
            finish_exit(app);
        }
    });
}

pub(crate) fn finish_exit(app: tauri::AppHandle) {
    if !begin_exit(&QUITTING) {
        return;
    }
    log::info!("lifecycle tray_exit_requested");
    let paths = app
        .try_state::<crate::AppState>()
        .map(|state| state.paths.clone());
    let resources = app.path().resource_dir();
    tauri::async_runtime::spawn(async move {
        if let (Some(paths), Ok(resources)) = (paths, resources) {
            let close_result =
                crate::archive_auto_backup::close_all(app.clone(), paths, resources).await;
            if let Ok((crate::archive_auto_backup::CloseResult::Finished(Some(error)), _)) =
                close_result
            {
                if error != "redaction_pending" {
                    crate::report_chat_archive(&app, None, Err(error));
                }
            }
        }
        log::info!("lifecycle tray_exit_cleanup_finished");
        app.exit(0);
    });
}

pub(crate) fn install(app: &tauri::AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, OPEN, "열기", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT, "종료", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("app icon".into()))?;
    TrayIconBuilder::with_id("momso")
        .icon(icon)
        .tooltip("momso")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match menu_action(event.id().as_ref()) {
            Some(Action::Open) => show_main(app),
            Some(Action::Quit) => request_exit(app.clone()),
            None => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button,
                button_state,
                ..
            } = event
            {
                if is_open_click(button, button_state) {
                    show_main(tray.app_handle());
                }
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exit_requires_explicit_opt_in_and_argument() {
        let args = vec!["momso.exe".to_owned(), "--momso-test-exit".to_owned()];
        assert!(!is_test_exit(&args, false));
        assert!(!is_test_exit(&args[..1], true));
        assert!(is_test_exit(&args, true));
    }

    #[test]
    fn tray_actions_open_on_left_release_and_only_quit_from_quit_menu() {
        assert_eq!(menu_action(OPEN), Some(Action::Open));
        assert_eq!(menu_action(QUIT), Some(Action::Quit));
        assert_eq!(menu_action("unrelated"), None);
        assert!(is_open_click(MouseButton::Left, MouseButtonState::Up));
        assert!(!is_open_click(MouseButton::Left, MouseButtonState::Down));
        assert!(!is_open_click(MouseButton::Right, MouseButtonState::Up));
    }

    #[test]
    fn repeated_quit_requests_start_cleanup_once() {
        let flag = AtomicBool::new(false);
        assert!(begin_exit(&flag));
        assert!(!begin_exit(&flag));
    }

    #[tokio::test]
    async fn exit_request_waits_for_matching_response_and_timeout_is_safe() {
        let broker = ExitBroker::default();
        let (id, mut receiver) = broker.start().unwrap();
        assert!(broker.start().is_none());
        assert_eq!(broker.respond("other", true), "stale");
        assert!(receiver.try_recv().is_err());
        assert_eq!(broker.respond(&id, false), "accepted");
        assert!(!receiver.await.unwrap());
        assert_eq!(broker.respond(&id, true), "stale");
        assert!(broker.start().is_none());
        broker.cancel(&id);
        let (expired, receiver) = broker.start().unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), receiver)
                .await
                .is_err()
        );
        broker.cancel(&expired);
        assert_eq!(broker.respond(&expired, true), "stale");
        let (approved, receiver) = broker.start().unwrap();
        assert_eq!(broker.respond(&approved, true), "accepted");
        assert!(receiver.await.unwrap());
        assert_eq!(broker.respond(&approved, true), "stale");
    }

    #[test]
    fn main_close_keeps_window_and_event_loop_alive() {
        use std::sync::Arc;
        use std::time::Duration;
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let window = tauri::window::WindowBuilder::new(&app, "main")
            .build()
            .unwrap();
        let observed = Arc::new(AtomicBool::new(false));
        let result = observed.clone();
        let handle = app.handle().clone();
        let helper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            window.close().unwrap();
            std::thread::sleep(Duration::from_secs(2));
            result.store(handle.get_window("main").is_some(), Ordering::SeqCst);
            window.destroy().unwrap();
        });
        app.run(|handle, event| {
            if let tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } = event
            {
                if label == "main" {
                    hide_on_close(&handle.get_window("main").unwrap(), &api);
                }
            }
        });
        helper.join().unwrap();
        assert!(observed.load(Ordering::SeqCst));
    }
}
