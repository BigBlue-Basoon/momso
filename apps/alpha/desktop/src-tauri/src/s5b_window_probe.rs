//! Isolated native-window probe; compiled only for the explicit debug test feature.
use std::path::PathBuf;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

pub fn run() {
    let root = PathBuf::from(std::env::var_os("MOMSO_S5B_ROOT").expect("isolated root"));
    let id = std::env::var("MOMSO_S5B_IDENTIFIER").expect("isolated identifier");
    assert!(id.starts_with("app.momso.s5b.fixture."));
    assert!(root.is_absolute() && root.starts_with(std::env::temp_dir()));
    let panel = std::env::var_os("MOMSO_S5B_PANEL").is_some();
    let url = std::env::var("MOMSO_S5B_URL").expect("local fixture URL");
    let url: tauri::Url = url.parse().expect("valid fixture URL");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    let mut context = tauri::generate_context!();
    context.config_mut().identifier = id;
    context.config_mut().app.windows.clear();
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if crate::tray_lifecycle::is_test_exit(&args, true) {
                crate::tray_lifecycle::finish_exit(app.clone());
            } else if args.iter().any(|arg| arg == "--probe-reload") {
                if let Some(webview) = app.get_webview("main") {
                    let _ = webview.eval("window.location.reload()");
                }
            } else {
                crate::tray_lifecycle::show_main(app);
            }
        }))
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    crate::tray_lifecycle::hide_on_close(window, api);
                }
            }
        })
        .setup(move |app| {
            let _ = std::fs::write(root.join("app-pid.txt"), std::process::id().to_string());
            let load_file = root.join("loads.txt");
            let load_count = std::sync::atomic::AtomicUsize::new(0);
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("momso-s5b-fixture")
                .data_directory(root.join("main-webview"))
                .on_page_load(move |_, payload| {
                    if payload.event() == tauri::webview::PageLoadEvent::Finished {
                        let count =
                            load_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                        let _ = std::fs::write(&load_file, count.to_string());
                    }
                })
                .build()?;
            crate::app_lifecycle::restore_recovery_visibility(app.handle());
            crate::app_lifecycle::observe_webview(window.as_ref());
            let app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let outcome = if panel {
                    let _ = crate::panel_browser::panel_browser_select_chat(
                        app.clone(),
                        "s5b-fixture".into(),
                    )
                    .await;
                    crate::panel_browser::panel_browser_open_agent(
                        app.clone(),
                        "s5b-fixture".into(),
                        0.0,
                        0.0,
                        300.0,
                        200.0,
                    )
                    .await
                } else {
                    Ok(())
                };
                let single_webview = app.get_webview_window("main").is_some();
                let hwnd = format!("{:?}", window.hwnd());
                let _ = window.with_webview(move |platform| unsafe {
                    let mut browser_pid = 0;
                    let pid_result = platform.controller().CoreWebView2().and_then(|core| core.BrowserProcessId(&mut browser_pid));
                    let result = format!(
                        "panel={panel} outcome={outcome:?} webview_window={single_webview} hwnd={hwnd} app_pid={} browser_pid={browser_pid} browser_result={pid_result:?}",
                        std::process::id()
                    );
                    let _ = std::fs::write(root.join("ready.txt"), result);
                });
            });
            Ok(())
        })
        .run(context)
        .expect("isolated native-window probe");
}
