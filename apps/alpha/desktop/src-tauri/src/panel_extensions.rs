//! WebView2 extensions are owned by the shared human-panel profile.
use serde::Serialize;
use tauri::Emitter;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExtensionInfo {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) icon_data_url: Option<String>,
    pub(crate) origin: &'static str,
}

#[cfg(windows)]
pub(crate) enum Operation {
    List,
    Install(std::path::PathBuf),
    SetEnabled(String, bool),
    Remove(String),
}

#[cfg(windows)]
pub(crate) async fn perform(
    app: &tauri::AppHandle,
    operation: Operation,
) -> Result<Vec<ExtensionInfo>, &'static str> {
    use std::sync::{Arc, Mutex};
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2BrowserExtensionList, ICoreWebView2Profile7, ICoreWebView2_13,
    };
    use webview2_com::{
        BrowserExtensionEnableCompletedHandler, BrowserExtensionRemoveCompletedHandler,
        ProfileAddBrowserExtensionCompletedHandler, ProfileGetBrowserExtensionsCompletedHandler,
    };
    use windows_core::{Interface, HSTRING, PWSTR};

    static OPERATIONS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _operation_guard = OPERATIONS.lock().await;
    let install_path = match &operation {
        Operation::Install(path) => Some(path.clone()),
        _ => None,
    };

    unsafe fn items(
        list: &ICoreWebView2BrowserExtensionList,
    ) -> Result<Vec<ExtensionInfo>, &'static str> {
        let mut count = 0;
        unsafe { list.Count(&mut count) }.map_err(|_| "unavailable")?;
        let mut result = Vec::new();
        for index in 0..count {
            let item = unsafe { list.GetValueAtIndex(index) }.map_err(|_| "unavailable")?;
            let mut id = PWSTR::null();
            let mut name = PWSTR::null();
            let mut enabled = windows_core::BOOL(0);
            unsafe { item.Id(&mut id) }.map_err(|_| "unavailable")?;
            let id = webview2_com::take_pwstr(id);
            unsafe { item.Name(&mut name) }.map_err(|_| "unavailable")?;
            let name = webview2_com::take_pwstr(name);
            unsafe { item.IsEnabled(&mut enabled) }.map_err(|_| "unavailable")?;
            result.push(ExtensionInfo {
                id,
                name,
                enabled: enabled.as_bool(),
                icon_data_url: None,
                origin: "unknown",
            });
        }
        Ok(result)
    }
    let view = crate::panel_browser::current_panel(app).ok_or("panel_unavailable")?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(tx)));
    view.with_webview(move |platform| {
        let result = unsafe {
            platform.controller().CoreWebView2()
                .and_then(|core| core.cast::<ICoreWebView2_13>())
                .and_then(|core| core.Profile())
                .and_then(|profile| profile.cast::<ICoreWebView2Profile7>())
        };
        let Ok(profile) = result else {
            if let Ok(mut tx) = sender.lock() { if let Some(tx) = tx.take() { let _ = tx.send(Err("unsupported")); } }
            return;
        };
        let on_error = sender.clone();
        let started = unsafe { match operation {
            Operation::Install(path) => {
                let tx = sender.clone();
                let handler = ProfileAddBrowserExtensionCompletedHandler::create(Box::new(move |status, extension| {
                    let value = status.map_err(|_| "install_failed").and_then(|_| {
                        let extension = extension.ok_or("install_failed")?;
                        let mut id = PWSTR::null(); let mut name = PWSTR::null(); let mut enabled = windows_core::BOOL(0);
                        extension.Id(&mut id).map_err(|_| "install_failed")?;
                        let id = webview2_com::take_pwstr(id);
                        extension.Name(&mut name).map_err(|_| "install_failed")?;
                        let name = webview2_com::take_pwstr(name);
                        extension.IsEnabled(&mut enabled).map_err(|_| "install_failed")?;
                        Ok(vec![ExtensionInfo { id, name, enabled: enabled.as_bool(), icon_data_url: None, origin: "unknown" }])
                    });
                    if let Ok(mut tx) = tx.lock() { if let Some(tx) = tx.take() { let _ = tx.send(value); } }
                    Ok(())
                }));
                profile.AddBrowserExtension(&HSTRING::from(path.as_path()), &handler)
            }
            other => {
                let tx = sender.clone();
                let handler = ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(move |status, list| {
                    let value = status.map_err(|_| "unavailable").and_then(|_| list.ok_or("unavailable"));
                    match value {
                        Err(error) => { if let Ok(mut tx) = tx.lock() { if let Some(tx) = tx.take() { let _ = tx.send(Err(error)); } } }
                        Ok(list) => match other {
                            Operation::List => { let value = items(&list); if let Ok(mut tx) = tx.lock() { if let Some(tx) = tx.take() { let _ = tx.send(value); } } }
                            Operation::SetEnabled(_, _) | Operation::Remove(_) => {
                                let (id, enabled) = match other { Operation::SetEnabled(id, enabled) => (id, Some(enabled)), Operation::Remove(id) => (id, None), _ => unreachable!() };
                                let item = items(&list)
                                    .and_then(|values| values.iter().position(|value| value.id == id).ok_or("not_found"))
                                    .and_then(|index| list.GetValueAtIndex(index as u32).map_err(|_| "unavailable"));
                                if let Ok(item) = item {
                                    let done = tx.clone();
                                    let started = if let Some(enabled) = enabled {
                                        let handler = BrowserExtensionEnableCompletedHandler::create(Box::new(move |status| { if let Ok(mut tx) = done.lock() { if let Some(tx) = tx.take() { let _ = tx.send(status.map(|_| Vec::new()).map_err(|_| "operation_failed")); } } Ok(()) }));
                                        item.Enable(enabled, &handler)
                                    } else {
                                        let handler = BrowserExtensionRemoveCompletedHandler::create(Box::new(move |status| { if let Ok(mut tx) = done.lock() { if let Some(tx) = tx.take() { let _ = tx.send(status.map(|_| Vec::new()).map_err(|_| "operation_failed")); } } Ok(()) }));
                                        item.Remove(&handler)
                                    };
                                    if started.is_err() { if let Ok(mut tx) = tx.lock() { if let Some(tx) = tx.take() { let _ = tx.send(Err("operation_failed")); } } }
                                } else if let Err(error) = item { if let Ok(mut tx) = tx.lock() { if let Some(tx) = tx.take() { let _ = tx.send(Err(error)); } } }
                            }
                            Operation::Install(_) => unreachable!(),
                        }
                    }
                    Ok(())
                }));
                profile.GetBrowserExtensions(&handler)
            }
        }};
        if started.is_err() { if let Ok(mut tx) = on_error.lock() { if let Some(tx) = tx.take() { let _ = tx.send(Err("unavailable")); } } }
    }).map_err(|_| "unavailable")?;
    let entries = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
        .await
        .map_err(|_| "timeout")?
        .map_err(|_| "unavailable")??;
    if entries.is_empty() {
        return Ok(entries);
    }
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        crate::panel_extension_catalog::enrich(&app, entries, install_path.as_deref())
    })
    .await
    .map_err(|_| "unavailable")
}

#[cfg(all(test, windows))]
pub(crate) async fn install_fixture(
    app: &tauri::AppHandle,
    root: &std::path::Path,
) -> Result<String, &'static str> {
    let folder = root.join("fixture-extension");
    std::fs::create_dir_all(&folder).map_err(|_| "fixture_io")?;
    std::fs::write(folder.join("manifest.json"), r#"{"manifest_version":3,"name":"momso fixture","version":"1.0.0","content_scripts":[{"matches":["http://127.0.0.1/*"],"js":["content.js"]}]}"#).map_err(|_| "fixture_io")?;
    std::fs::write(
        folder.join("content.js"),
        "document.documentElement.dataset.momsoFixture='active';",
    )
    .map_err(|_| "fixture_io")?;
    let installed = perform(app, Operation::Install(folder)).await?;
    let id = installed.first().ok_or("fixture_install")?.id.clone();
    if !perform(app, Operation::List)
        .await?
        .iter()
        .any(|item| item.id == id && item.enabled)
    {
        return Err("fixture_list");
    }
    perform(app, Operation::SetEnabled(id.clone(), false)).await?;
    if !perform(app, Operation::List)
        .await?
        .iter()
        .any(|item| item.id == id && !item.enabled)
    {
        return Err("fixture_disable");
    }
    perform(app, Operation::SetEnabled(id.clone(), true)).await?;
    if !perform(app, Operation::List)
        .await?
        .iter()
        .any(|item| item.id == id && item.enabled)
    {
        return Err("fixture_enable");
    }
    Ok(id)
}

#[cfg(all(test, windows))]
pub(crate) async fn remove_fixture(app: &tauri::AppHandle, id: String) -> Result<(), &'static str> {
    perform(app, Operation::Remove(id.clone())).await?;
    if perform(app, Operation::List)
        .await?
        .iter()
        .any(|item| item.id == id)
    {
        return Err("fixture_remove");
    }
    Ok(())
}

#[cfg(not(windows))]
async fn perform(_: &tauri::AppHandle, _: ()) -> Result<Vec<ExtensionInfo>, &'static str> {
    Err("unsupported")
}

#[tauri::command]
pub(crate) async fn panel_extensions_list(
    app: tauri::AppHandle,
) -> Result<Vec<ExtensionInfo>, &'static str> {
    #[cfg(windows)]
    {
        perform(&app, Operation::List).await
    }
    #[cfg(not(windows))]
    {
        perform(&app, ()).await
    }
}

#[tauri::command]
pub(crate) async fn panel_extensions_install(
    app: tauri::AppHandle,
) -> Result<Vec<ExtensionInfo>, &'static str> {
    use tauri_plugin_dialog::DialogExt;
    let chooser = app.clone();
    let path = tauri::async_runtime::spawn_blocking(move || {
        chooser
            .dialog()
            .file()
            .blocking_pick_folder()
            .and_then(|p| p.into_path().ok())
    })
    .await
    .map_err(|_| "unavailable")?
    .ok_or("cancelled")?;
    #[cfg(windows)]
    let result = {
        let result = perform(&app, Operation::Install(path.clone())).await;
        if let Ok(extensions) = &result {
            if let Some(extension) = extensions.first() {
                if crate::panel_extension_catalog::remember_path(&app, &extension.id, &path)
                    .is_err()
                {
                    let _ = app.emit("panel-extensions-changed", ());
                    return Err("installed_catalog_unavailable");
                }
            }
        }
        result
    };
    #[cfg(not(windows))]
    let result = {
        let _ = path;
        perform(&app, ()).await
    };
    if result.is_ok() {
        let _ = app.emit("panel-extensions-changed", ());
    }
    result
}

#[tauri::command]
pub(crate) async fn panel_extensions_set_enabled(
    app: tauri::AppHandle,
    extension_id: String,
    enabled: bool,
) -> Result<(), &'static str> {
    #[cfg(windows)]
    perform(&app, Operation::SetEnabled(extension_id, enabled)).await?;
    #[cfg(not(windows))]
    {
        let _ = (extension_id, enabled);
        perform(&app, ()).await?;
    }
    let _ = app.emit("panel-extensions-changed", ());
    Ok(())
}

#[tauri::command]
pub(crate) async fn panel_extensions_remove(
    app: tauri::AppHandle,
    extension_id: String,
) -> Result<(), &'static str> {
    #[cfg(windows)]
    perform(&app, Operation::Remove(extension_id.clone())).await?;
    #[cfg(not(windows))]
    {
        let _ = &extension_id;
        perform(&app, ()).await?;
    }
    let result = crate::panel_extension_catalog::forget(&app, &extension_id);
    let _ = app.emit("panel-extensions-changed", ());
    result
}
