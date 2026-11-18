//! Lifecycle diagnostics contain event kinds and codes, never page content or paths.

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
