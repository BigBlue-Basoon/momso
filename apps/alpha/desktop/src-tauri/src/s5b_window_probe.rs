//! Isolated native-window probe; compiled only for the explicit debug test feature.
use std::path::PathBuf;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

pub fn run() {
    let root = PathBuf::from(std::env::var_os("MOMSO_S5B_ROOT").expect("isolated root"));
    let id = std::env::var("MOMSO_S5B_IDENTIFIER").expect("isolated identifier");
    assert!(id.starts_with("app.momso.s5b.fixture."));
    assert!(root.is_absolute() && root.starts_with(std::env::temp_dir()));
    let panel = std::env::var_os("MOMSO_S5B_PANEL").is_some();
    let mut context = tauri::generate_context!();
    context.config_mut().identifier = id;
    context.config_mut().app.windows.clear();
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if args.iter().any(|arg| arg == "--probe-finish") {
                app.exit(0);
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
            let window = WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::External("about:blank".parse()?),
            )
            .title("momso-s5b-fixture")
            .data_directory(root.join("main-webview"))
            .build()?;
            let app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let outcome = if panel {
                    crate::panel_browser::panel_browser_open_agent(
                        app.clone(),
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
                let result = format!(
                    "panel={panel} outcome={outcome:?} webview_window={single_webview} hwnd={:?}",
                    window.hwnd()
                );
                let _ = std::fs::write(root.join("ready.txt"), result);
            });
            Ok(())
        })
        .run(context)
        .expect("isolated native-window probe");
}
