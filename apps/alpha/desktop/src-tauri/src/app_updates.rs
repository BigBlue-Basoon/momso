use serde::Serialize;
use std::time::Duration;
use tauri::{Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Mutex;

#[derive(Default)]
pub(crate) struct UpdateState(Mutex<Inner>);
#[derive(Default)]
struct Inner {
    pending: Option<Update>,
    downloaded: Option<Vec<u8>>,
    busy: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateInfo {
    state: &'static str,
    current_version: String,
    latest_version: Option<String>,
    notes: Option<String>,
    published_at: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VersionInfo {
    version: String,
    build_date: Option<&'static str>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
}
#[derive(Clone, Serialize)]
pub(crate) struct Stage {
    state: &'static str,
}

fn info(state: &'static str, version: &str) -> UpdateInfo {
    UpdateInfo {
        state,
        current_version: version.to_owned(),
        latest_version: None,
        notes: None,
        published_at: None,
    }
}
fn check_failed(version: &str) -> UpdateInfo {
    info("failed", version)
}
fn configured(key: &str, endpoint: &str) -> bool {
    !key.trim().is_empty()
        && endpoint
            .parse::<reqwest::Url>()
            .is_ok_and(|u| u.scheme() == "https" && u.host_str().is_some())
}
fn settings(app: &tauri::AppHandle) -> Option<(String, String)> {
    let config = app.config().plugins.0.get("updater")?;
    let key = config.get("pubkey")?.as_str()?.to_owned();
    let endpoint = config
        .get("endpoints")?
        .as_array()?
        .first()?
        .as_str()?
        .to_owned();
    configured(&key, &endpoint).then_some((key, endpoint))
}
fn newer(current: &str, latest: &str) -> bool {
    match (
        semver::Version::parse(current),
        semver::Version::parse(latest.trim_start_matches('v')),
    ) {
        (Ok(a), Ok(b)) => b > a,
        _ => false,
    }
}
async fn check(app: &tauri::AppHandle) -> UpdateInfo {
    let version = app.package_info().version.to_string();
    if settings(app).is_none() {
        return info("unavailable", &version);
    }
    let updater = match app
        .updater_builder()
        .timeout(Duration::from_secs(30))
        .build()
    {
        Ok(value) => value,
        Err(_) => return info("failed", &version),
    };
    match updater.check().await {
        Ok(Some(update)) if newer(&version, &update.version) => {
            let result = UpdateInfo {
                state: "available",
                current_version: version,
                latest_version: Some(update.version.clone()),
                notes: update.body.clone(),
                published_at: update
                    .raw_json
                    .get("pub_date")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
            };
            let state = app.state::<UpdateState>();
            let mut inner = state.0.lock().await;
            if !inner.busy {
                if inner
                    .pending
                    .as_ref()
                    .is_none_or(|p| p.version != update.version)
                {
                    inner.downloaded = None;
                }
                inner.pending = Some(update);
            }
            result
        }
        Ok(_) => {
            let state = app.state::<UpdateState>();
            let mut inner = state.0.lock().await;
            if !inner.busy {
                inner.pending = None;
                inner.downloaded = None;
            }
            info("up_to_date", &version)
        }
        Err(_) => check_failed(&version),
    }
}

pub(crate) fn start_monitor(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let result = check(&app).await;
            if result.state == "available" {
                let _ = app.emit("update-available", result);
            }
            tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
        }
    });
}
#[tauri::command]
pub(crate) fn app_version(app: tauri::AppHandle) -> VersionInfo {
    VersionInfo {
        version: app.package_info().version.to_string(),
        build_date: option_env!("MOMSO_BUILD_DATE").filter(|s| !s.trim().is_empty()),
    }
}
#[tauri::command]
pub(crate) async fn update_check(app: tauri::AppHandle) -> Result<UpdateInfo, String> {
    Ok(check(&app).await)
}

#[tauri::command]
pub(crate) async fn update_install(
    app: tauri::AppHandle,
    state: State<'_, UpdateState>,
) -> Result<Stage, String> {
    if settings(&app).is_none() {
        return Ok(Stage {
            state: "unavailable",
        });
    }
    let update = {
        let mut inner = state.0.lock().await;
        if inner.busy {
            return Ok(Stage { state: "busy" });
        }
        let Some(update) = inner.pending.clone() else {
            return Ok(Stage {
                state: "unavailable",
            });
        };
        inner.busy = true;
        inner.downloaded = None;
        update
    };
    let mut downloaded = 0u64;
    let result = update
        .download(
            |chunk, total| {
                downloaded = downloaded.saturating_add(chunk as u64);
                let _ = app.emit(
                    "update-progress",
                    Progress {
                        downloaded_bytes: downloaded,
                        total_bytes: total,
                    },
                );
            },
            || {},
        )
        .await;
    let mut inner = state.0.lock().await;
    inner.busy = false;
    match result {
        Ok(bytes) => {
            inner.downloaded = Some(bytes);
            let stage = Stage {
                state: "downloaded",
            };
            let _ = app.emit("update-done", stage.clone());
            Ok(stage)
        }
        Err(_) => {
            let stage = Stage { state: "failed" };
            let _ = app.emit("update-failed", stage.clone());
            Ok(stage)
        }
    }
}
#[tauri::command]
pub(crate) async fn update_restart(
    app: tauri::AppHandle,
    state: State<'_, UpdateState>,
    workspace: State<'_, crate::AppState>,
) -> Result<Stage, String> {
    if settings(&app).is_none() {
        return Ok(Stage {
            state: "unavailable",
        });
    }
    let (update, bytes) = {
        let mut inner = state.0.lock().await;
        if inner.busy {
            return Ok(Stage { state: "busy" });
        }
        let Some(update) = inner.pending.clone() else {
            return Ok(Stage {
                state: "unavailable",
            });
        };
        let Some(bytes) = inner.downloaded.take() else {
            return Ok(Stage {
                state: "unavailable",
            });
        };
        inner.busy = true;
        (update, bytes)
    };
    let lease = workspace.paths.archive.current.clone().read_owned().await;
    let database = workspace.paths.database_path.clone();
    let backups = workspace.paths.backups_path.clone();
    // On Windows, Tauri exits the application when the signed installer is launched.
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        install_after_backup(&database, &backups, || {
            update.install(bytes).map_err(|_| ())
        })
    })
    .await;
    state.0.lock().await.busy = false;
    match result {
        Ok(Ok(())) => {
            let stage = Stage {
                state: "install_started",
            };
            let _ = app.emit("update-done", stage.clone());
            Ok(stage)
        }
        _ => {
            let stage = Stage { state: "failed" };
            let _ = app.emit("update-failed", stage.clone());
            Ok(stage)
        }
    }
}

fn install_after_backup(
    database: &std::path::Path,
    backups: &std::path::Path,
    install: impl FnOnce() -> Result<(), ()>,
) -> Result<(), ()> {
    crate::storage::create_manual_backup(database, backups).map_err(|_| ())?;
    install()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installation_runs_only_after_a_successful_database_backup() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("momso.db");
        let backups = temp.path().join("backups");
        crate::storage::initialize(&db, &backups, "test").unwrap();
        let called = std::cell::Cell::new(false);
        install_after_backup(&db, &backups, || {
            called.set(true);
            Ok(())
        })
        .unwrap();
        assert!(called.get());
        assert!(std::fs::read_dir(&backups).unwrap().next().is_some());
        let called = std::cell::Cell::new(false);
        assert!(install_after_backup(temp.path(), &backups, || {
            called.set(true);
            Ok(())
        })
        .is_err());
        assert!(!called.get());
    }
    #[test]
    fn empty_settings_unavailable() {
        assert!(!configured("", ""));
        assert!(!configured("public", ""));
        assert!(!configured("", "https://example.invalid/latest.json"));
        assert!(!configured("public", "http://example.invalid/latest.json"));
    }
    #[test]
    fn configured_public_key_and_endpoint_are_available() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let updater = &config["plugins"]["updater"];
        let key = updater["pubkey"].as_str().unwrap();
        assert!(!key.is_empty());
        let endpoint = updater["endpoints"]
            .as_array()
            .unwrap()
            .first()
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(configured(key, endpoint));
    }
    #[test]
    fn version_comparison() {
        assert!(newer("0.1.0", "0.2.0"));
        assert!(!newer("0.2.0", "0.1.0"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(newer("0.1.0-alpha.1", "0.1.0"));
        assert!(!newer("0.1.0", "invalid"));
    }
    #[test]
    fn fake_metadata_file_detects_update() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("latest.json");
        std::fs::write(&path,r#"{"version":"0.2.0","notes":"local fixture","platforms":{"windows-x86_64":{"url":"https://example.invalid/update.exe","signature":"not-a-signature"}}}"#).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(newer("0.1.0", value["version"].as_str().unwrap()));
    }

    #[tokio::test]
    async fn official_updater_detects_local_fixture_without_signing() {
        use std::io::{Read, Write};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("latest.json");
        std::fs::write(&path, r#"{"version":"0.2.0","notes":"fixture","platforms":{"windows-x86_64":{"url":"https://example.invalid/update.exe","signature":"not-a-signature"}}}"#).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(10)))
                            .unwrap();
                        let mut request = Vec::new();
                        let mut chunk = [0u8; 1024];
                        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                            let count = stream.read(&mut chunk).unwrap();
                            assert!(count > 0 && request.len() + count <= 16384);
                            request.extend_from_slice(&chunk[..count]);
                        }
                        let body = std::fs::read(path).unwrap();
                        let mut response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                        response.extend_from_slice(&body);
                        stream.write_all(&response).unwrap();
                        return;
                    }
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && start.elapsed() < Duration::from_secs(10) =>
                    {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(e) => panic!("local fixture: {e}"),
                }
            }
        });
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().plugins.0.insert(
            "updater".into(),
            serde_json::json!({"pubkey":"", "endpoints":[], "dangerousInsecureTransportProtocol":true}),
        );
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .unwrap();
        let updater = app
            .handle()
            .updater_builder()
            .endpoints(vec![format!("http://127.0.0.1:{port}/latest.json")
                .parse()
                .unwrap()])
            .unwrap()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let update = updater.check().await.unwrap().unwrap();
        assert_eq!(update.version, "0.2.0");
        server.join().unwrap();
    }

    #[tokio::test]
    async fn missing_release_returns_failed_without_an_available_update() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().plugins.0.insert(
            "updater".into(),
            serde_json::json!({"pubkey":"", "endpoints":[], "dangerousInsecureTransportProtocol":true}),
        );
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .unwrap();
        let updater = app
            .handle()
            .updater_builder()
            .endpoints(vec![format!("http://127.0.0.1:{port}/latest.json")
                .parse()
                .unwrap()])
            .unwrap()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        assert!(updater.check().await.is_err());
        let result = check_failed("0.1.0");
        assert_eq!(result.state, "failed");
        assert!(result.latest_version.is_none());
        server.join().unwrap();
    }
}
