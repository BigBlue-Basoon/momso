use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use tauri::{
    test::{get_ipc_response, mock_builder, mock_context, noop_assets, INVOKE_KEY},
    webview::InvokeRequest,
};

#[test]
fn every_registered_command_from_remote_panel_is_denied_before_dispatch() {
    let source = include_str!("lib.rs");
    let commands = source
        .rsplit(".invoke_handler(tauri::generate_handler![")
        .next()
        .unwrap()
        .split("])")
        .next()
        .unwrap();
    let names: Vec<_> = commands
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            let line = line.strip_suffix(',')?;
            Some(line.rsplit("::").next().unwrap())
        })
        .collect();
    assert!(names.len() > 100, "registered commands were not discovered");
    let calls = Arc::new(AtomicUsize::new(0));
    let dispatched = calls.clone();
    let app = mock_builder()
        .invoke_handler(move |_| {
            dispatched.fetch_add(1, Ordering::SeqCst);
            false
        })
        .build(mock_context(noop_assets()))
        .unwrap();
    let panel = tauri::WebviewWindowBuilder::new(&app, "panel-browser", Default::default())
        .build()
        .unwrap();
    let extension =
        tauri::WebviewWindowBuilder::new(&app, "panel-extension-popup", Default::default())
            .build()
            .unwrap();
    for name in &names {
        let response = get_ipc_response(
            &panel,
            InvokeRequest {
                cmd: (*name).into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "https://example.test/".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(serde_json::json!({})),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.into(),
            },
        );
        assert!(response.is_err(), "remote invocation succeeded: {name}");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "remote command dispatched: {name}"
        );
        let response = get_ipc_response(
            &extension,
            InvokeRequest {
                cmd: (*name).into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "chrome-extension://abcdefghijklmnopabcdefghijklmnop/popup.html"
                    .parse()
                    .unwrap(),
                body: tauri::ipc::InvokeBody::Json(serde_json::json!({})),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.into(),
            },
        );
        assert!(response.is_err(), "extension invocation succeeded: {name}");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "extension command dispatched: {name}"
        );
    }
    let _ = get_ipc_response(
        &panel,
        InvokeRequest {
            cmd: names[0].into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "http://tauri.localhost/".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(serde_json::json!({})),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.into(),
        },
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "local control did not reach the handler"
    );
    println!("checked {} registered commands", names.len());
}

#[test]
fn only_app_owned_webviews_receive_capabilities() {
    let capability: serde_json::Value =
        serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
    assert_eq!(
        capability["webviews"],
        serde_json::json!(["main", "panel-overlay"])
    );
    assert!(capability.get("windows").is_none());
    assert!(capability.get("remote").is_none());
}
