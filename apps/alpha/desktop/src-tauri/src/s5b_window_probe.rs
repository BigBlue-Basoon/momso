//! Isolated native-window probe; compiled only for the explicit debug test feature.
use std::path::PathBuf;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

#[tauri::command]
fn overlay_probe_report(
    webview: tauri::Webview,
    value: serde_json::Value,
) -> Result<(), &'static str> {
    if webview.label() != crate::panel_overlay::WEBVIEW {
        return Err("unavailable");
    }
    let root = PathBuf::from(std::env::var_os("MOMSO_S5B_ROOT").ok_or("unavailable")?);
    if !root.is_absolute() || !root.starts_with(std::env::temp_dir()) {
        return Err("unavailable");
    }
    std::fs::write(root.join("overlay-invoke.json"), value.to_string()).map_err(|_| "unavailable")
}

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
    if std::env::var_os("MOMSO_S13_OVERLAY").is_some() {
        context.config_mut().build.dev_url = Some(url.clone());
    }
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![crate::panel_overlay::panel_overlay_state, overlay_probe_report])
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if crate::tray_lifecycle::is_test_exit(&args, true) {
                crate::tray_lifecycle::finish_exit(app.clone());
            } else if args.iter().any(|arg| arg == "--probe-reload") {
                if let Some(webview) = app.get_webview("main") {
                    let _ = webview.eval("window.location.reload()");
                }
            } else if args.iter().any(|arg| arg == "--probe-other-chat") {
                let app = app.clone();
                tauri::async_runtime::spawn(async move { let _ = crate::panel_browser::panel_browser_select_chat(app, "s13-other".into()).await; });
            } else if args.iter().any(|arg| arg == "--probe-hide-panel") {
                let app = app.clone();
                tauri::async_runtime::spawn(async move { let _ = crate::panel_browser::panel_browser_visibility(app, "s5b-fixture".into(), false).await; });
            } else {
                crate::tray_lifecycle::show_main(app);
            }
        }))
        .on_window_event(|window, event| {
            if window.label() == "main" {
                crate::panel_overlay::main_window_event(window, event);
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    crate::tray_lifecycle::hide_on_close(window, api);
                }
            }
        })
        .setup(move |app| {
            let _ = std::fs::write(root.join("app-pid.txt"), std::process::id().to_string());
            let load_file = root.join("loads.txt");
            let load_count = std::sync::atomic::AtomicUsize::new(0);
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.clone()))
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
                let overlay = if std::env::var_os("MOMSO_S28_POPUP").is_some() {
                    let extension = root.join("fixture-extension");
                    std::fs::create_dir_all(&extension).expect("fixture extension directory");
                    std::fs::write(extension.join("manifest.json"), r#"{"manifest_version":3,"name":"momso popup fixture","version":"1.0.0","host_permissions":["http://127.0.0.1/*"],"action":{"default_popup":"popup.html"}}"#).expect("fixture manifest");
                    std::fs::write(extension.join("popup.html"), "<!doctype html><title>extension popup</title><script src='popup.js'></script>").expect("fixture popup");
                    let report_url = format!("{}s28-result", url);
                    let js = format!(r#"(async()=>{{let ipc='absent';try{{if(window.__TAURI_INTERNALS__){{await window.__TAURI_INTERNALS__.invoke('panel_overlay_state');ipc='allowed'}}}}catch(_){{ipc='denied'}}await fetch({report_url:?}+'?runtime='+encodeURIComponent(chrome.runtime.id)+'&ipc='+ipc)}})()"#);
                    std::fs::write(extension.join("popup.js"), js).expect("fixture script");
                    let installed = crate::panel_extensions::perform(&app, crate::panel_extensions::Operation::Install(extension.clone())).await.expect("fixture extension installation");
                    let id = installed.first().expect("fixture extension id").id.clone();
                    crate::panel_extension_catalog::remember_path(&app, &id, &extension).expect("fixture extension catalog");
                    let input = serde_json::from_value(serde_json::json!({"kind":"extension","chatId":"s5b-fixture","x":80,"y":70,"width":320,"height":220})).expect("fixture bounds");
                    Some(crate::panel_overlay::panel_extension_popup_open(app.clone(), window.as_ref().clone(), id, input).await)
                } else if std::env::var_os("MOMSO_S13_OVERLAY").is_some() {
                    let input = serde_json::from_value(serde_json::json!({"kind":"fixture","chatId":"s5b-fixture","payload":{"site":"example.test","panel":{"width":320}},"x":80,"y":70,"width":320,"height":220})).expect("fixture bounds");
                    Some(crate::panel_overlay::panel_overlay_open(app.clone(), window.as_ref().clone(), input).await)
                } else { None };
                let single_webview = app.get_webview_window("main").is_some();
                let hwnd = format!("{:?}", window.hwnd());
                let _ = window.with_webview(move |platform| unsafe {
                    let mut browser_pid = 0;
                    let pid_result = platform.controller().CoreWebView2().and_then(|core| core.BrowserProcessId(&mut browser_pid));
                    let result = format!(
                        "panel={panel} outcome={outcome:?} overlay={overlay:?} webview_window={single_webview} hwnd={hwnd} app_pid={} browser_pid={browser_pid} browser_result={pid_result:?}",
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
