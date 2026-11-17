use serde::Serialize;
use std::time::Duration;
use tauri::{Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Mutex;

#[cfg(test)]
#[path = "app_updates_local_test.rs"]
mod local_test;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

fn updater_reason(error: &tauri_plugin_updater::Error) -> &'static str {
    use tauri_plugin_updater::Error;
    match error {
        Error::Reqwest(e) if e.is_timeout() => "timeout",
        Error::Reqwest(e) if e.is_connect() => "connection_failed",
        Error::Reqwest(_) => "network_failed",
        Error::Network(_) => "download_http_error",
        Error::ReleaseNotFound => "release_unavailable",
        Error::Serialization(_) | Error::Semver(_) => "invalid_release_info",
        Error::TargetNotFound(_)
        | Error::TargetsNotFound(_)
        | Error::UnsupportedArch
        | Error::UnsupportedOs => "platform_unavailable",
        Error::Minisign(_)
        | Error::Base64(_)
        | Error::SignatureUtf8(_)
        | Error::SignedVersionMismatch { .. }
        | Error::MissingSignedVersion => "signature_invalid",
        Error::EmptyEndpoints | Error::UrlParse(_) | Error::InsecureTransportProtocol => {
            "configuration_invalid"
        }
        _ => "unknown_failure",
    }
}

fn info(state: &'static str, version: &str) -> UpdateInfo {
    UpdateInfo {
        state,
        reason: None,
        current_version: version.to_owned(),
        latest_version: None,
        notes: None,
        published_at: None,
    }
}
fn check_failed(version: &str, reason: &'static str) -> UpdateInfo {
    let mut result = info("failed", version);
    result.reason = Some(reason);
    result
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
        Err(error) => return check_failed(&version, updater_reason(&error)),
    };
    match updater.check().await {
        Ok(Some(update)) if newer(&version, &update.version) => {
            let result = UpdateInfo {
                state: "available",
                reason: None,
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
        Err(error) => check_failed(&version, updater_reason(&error)),
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
            reason: Some("configuration_unavailable"),
        });
    }
    let update = {
        let mut inner = state.0.lock().await;
        if inner.busy {
            return Ok(Stage {
                state: "busy",
                reason: None,
            });
        }
        let Some(update) = inner.pending.clone() else {
            return Ok(Stage {
                state: "unavailable",
                reason: Some("update_not_selected"),
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
                reason: None,
            };
            let _ = app.emit("update-done", stage.clone());
            Ok(stage)
        }
        Err(error) => {
            let stage = Stage {
                state: "failed",
                reason: Some(updater_reason(&error)),
            };
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
            reason: Some("configuration_unavailable"),
        });
    }
    let (update, bytes) = {
        let mut inner = state.0.lock().await;
        if inner.busy {
            return Ok(Stage {
                state: "busy",
                reason: None,
            });
        }
        let Some(update) = inner.pending.clone() else {
            return Ok(Stage {
                state: "unavailable",
                reason: Some("update_not_selected"),
            });
        };
        let Some(bytes) = inner.downloaded.take() else {
            return Ok(Stage {
                state: "unavailable",
                reason: Some("update_not_downloaded"),
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
                reason: None,
            };
            let _ = app.emit("update-done", stage.clone());
            Ok(stage)
        }
        result => {
            let reason = match result {
                Ok(Err(reason)) => reason,
                Err(_) => "worker_failed",
                Ok(Ok(())) => unreachable!(),
            };
            let stage = Stage {
                state: "failed",
                reason: Some(reason),
            };
            let _ = app.emit("update-failed", stage.clone());
            Ok(stage)
        }
    }
}

fn install_after_backup(
    database: &std::path::Path,
    backups: &std::path::Path,
    install: impl FnOnce() -> Result<(), ()>,
) -> Result<(), &'static str> {
    crate::storage::create_manual_backup(database, backups).map_err(|_| "backup_failed")?;
    install().map_err(|_| "installation_failed")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn read_fixture_headers(stream: &mut std::net::TcpStream) -> std::io::Result<()> {
        use std::io::Read;
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 1024];
        while !bytes.windows(4).any(|v| v == b"\r\n\r\n") {
            let n = stream.read(&mut chunk)?;
            if n == 0 || bytes.len() + n > 16384 {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            bytes.extend_from_slice(&chunk[..n]);
        }
        Ok(())
    }
    #[test]
    fn fixture_waits_for_split_request_headers() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_fixture_headers(&mut stream).unwrap();
            sender.send(()).unwrap();
        });
        let mut client = std::net::TcpStream::connect(addr).unwrap();
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
            .unwrap();
        assert!(receiver.recv_timeout(Duration::from_millis(100)).is_err());
        client.write_all(b"\r\n").unwrap();
        assert!(receiver.recv_timeout(Duration::from_secs(2)).is_ok());
        server.join().unwrap();
    }
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
        assert_eq!(
            install_after_backup(temp.path(), &backups, || {
                called.set(true);
                Ok(())
            }),
            Err("backup_failed")
        );
        assert!(!called.get());
        assert_eq!(
            install_after_backup(&db, &backups, || Err(())),
            Err("installation_failed")
        );
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
        use std::io::Write;
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
                        stream.set_nonblocking(false).unwrap();
                        read_fixture_headers(&mut stream).unwrap();
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
        let result = check_failed("0.1.0", "release_unavailable");
        assert_eq!(result.state, "failed");
        assert!(result.latest_version.is_none());
        server.join().unwrap();
    }

    #[test]
    fn updater_reasons_distinguish_release_platform_and_signature() {
        use tauri_plugin_updater::Error;
        assert_eq!(
            updater_reason(&Error::ReleaseNotFound),
            "release_unavailable"
        );
        assert_eq!(
            updater_reason(&Error::UnsupportedOs),
            "platform_unavailable"
        );
        assert_eq!(
            updater_reason(&Error::MissingSignedVersion),
            "signature_invalid"
        );
        assert_eq!(
            updater_reason(&Error::EmptyEndpoints),
            "configuration_invalid"
        );
        let result = serde_json::to_value(check_failed("0.1.0", "release_unavailable")).unwrap();
        assert_eq!(result["reason"], "release_unavailable");
    }
}
