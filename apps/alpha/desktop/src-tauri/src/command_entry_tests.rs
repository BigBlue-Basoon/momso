use super::*;
use serde_json::{json, Value};
use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::webview::InvokeRequest;

#[cfg(windows)]
#[test]
fn approved_and_manual_documents_reappear_in_mcp_and_recent_list() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let database_path = root.join("momso.db");
    let backups_path = root.join("backups");
    std::fs::create_dir_all(&backups_path).unwrap();
    storage::initialize(&database_path, &backups_path, "test").unwrap();
    let archive =
        archive_location::ArchiveLocation::load(database_path.clone(), root, vec![]).unwrap();
    let archive_root = root.join("momso");
    archive::initialize(&archive_root, root).unwrap();
    let paths = RuntimePaths {
        data_root: root.to_path_buf(),
        database_path: database_path.clone(),
        objects_path: archive_root.join("objects"),
        exports_path: root.join("exports"),
        backups_path,
        logs_path: root.join("logs"),
        archive,
        archive_lease: None,
    };
    let broker = approval::ApprovalBroker::default();
    let app = mock_builder()
        .manage(AppState {
            paths,
            approvals: broker.clone(),
            setup_secrets: Default::default(),
            usage_monitor: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            usage_cache: Arc::new(usage_status::UsageCache::default()),
            chat_active: Default::default(),
            browser: Default::default(),
            selections: Default::default(),
        })
        .invoke_handler(tauri::generate_handler![zet_create, zet_list])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let manual = invoke(
        &window,
        "zet_create",
        json!({"input":{"title":"Manual fixture","summary":"note","kind":"note","body":"manual text","assets":[],"edges":[],"folder":"Classes/November"}}),
    );
    let manual_id = manual.as_str().unwrap();
    let ai_id = "zet_20990101_120000";
    let responder = broker.clone();
    let request = approval::ApprovalRequest {
        request_id: "fixture-approval".into(),
        chat_id: Some("fixture-chat".into()),
        provider: "codex".into(),
        kind: "fileChange".into(),
        description: String::new(),
        command: None,
        sandbox_escalation: None,
        change: Some(approval::ChangeKind::Create),
        targets: vec![approval::ApprovalTarget {
            path: format!("zettels/{ai_id}.md"),
            change: Some(approval::ChangeKind::Create),
        }],
        reason: None,
        options: vec![
            approval::ApprovalChoice::AllowOnce,
            approval::ApprovalChoice::Deny,
        ],
        folder: None,
        original_file_name: None,
        setup: None,
    };
    let approved = tauri::async_runtime::block_on(broker.request_with(
        request,
        tokio_util::sync::CancellationToken::new(),
        move |card| {
            let id = card.request_id.clone();
            tauri::async_runtime::spawn(async move {
                responder
                    .respond(&id, approval::ApprovalChoice::AllowOnce)
                    .await
                    .unwrap();
            });
            true
        },
        |_| {},
    ))
    .unwrap();
    assert_eq!(approved, approval::ApprovalChoice::AllowOnce);
    std::fs::write(archive_root.join("zettels").join(format!("{ai_id}.md")), format!("---\nid: {ai_id}\ntitle: Approved fixture\nsummary: note\nkind: note\nstatus: draft\ncreated_at: '2099-01-01T12:00:00+09:00'\nupdated_at: '2099-01-01T12:00:00+09:00'\nassets: []\nedges: []\n---\napproved unique text\n")).unwrap();
    let manual_path = archive_root.join("zettels").join(format!("{manual_id}.md"));
    let ai_path = archive_root.join("zettels").join(format!("{ai_id}.md"));
    let original_manual = std::fs::read(&manual_path).unwrap();
    let original_ai = std::fs::read(&ai_path).unwrap();
    let count: i64 = storage::open_writable_connection(&database_path)
        .unwrap()
        .query_row("SELECT count(*) FROM archive_zets", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1, "AI file is not yet indexed");
    let partial = momso_document_read::dispatch(
        &archive_root,
        &database_path,
        "momso_document_search",
        &json!({"query":"approved unique"}),
    );
    assert_eq!(partial["documents"][0]["id"], ai_id);
    storage::open_writable_connection(&database_path)
        .unwrap()
        .execute("DELETE FROM archive_zets", [])
        .unwrap();
    let all = momso_document_read::dispatch(
        &archive_root,
        &database_path,
        "momso_document_list",
        &json!({}),
    );
    assert_eq!(all["status"], "ok");
    assert_eq!(all["documents"].as_array().unwrap().len(), 2);
    assert_eq!(all["documents"][0]["id"], ai_id);
    assert!(all["documents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|doc| doc["id"] == manual_id));
    let search = momso_document_read::dispatch(
        &archive_root,
        &database_path,
        "momso_document_search",
        &json!({"query":"approved unique"}),
    );
    assert_eq!(search["documents"][0]["id"], ai_id);
    let recent = invoke(&window, "zet_list", json!({}));
    assert_eq!(recent.as_array().unwrap().len(), 2);
    assert_eq!(recent[0]["id"], ai_id);
    assert_eq!(
        recent
            .as_array()
            .unwrap()
            .iter()
            .find(|doc| doc["id"] == manual_id)
            .unwrap()["folder"],
        "Classes/November"
    );
    assert_eq!(recent[0]["folder"], "");
    assert_eq!(std::fs::read(manual_path).unwrap(), original_manual);
    assert_eq!(std::fs::read(ai_path).unwrap(), original_ai);
}

pub(crate) fn invoke(window: &tauri::WebviewWindow<MockRuntime>, cmd: &str, body: Value) -> Value {
    get_ipc_response(
        window,
        InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.into(),
        },
    )
    .unwrap_or_else(|error| panic!("{cmd}: {error}"))
    .deserialize()
    .unwrap()
}

#[cfg(windows)]
#[test]
fn fake_claude_r2_child() {
    use std::io::{BufRead, Write};
    let Some(pipe) = std::env::var_os("MOMSO_TEST_R2_PIPE") else {
        return;
    };
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut line = String::new();
    input.read_line(&mut line).unwrap();
    println!("\n{{\"type\":\"control_response\",\"response\":{{\"request_id\":\"momso-init\",\"subtype\":\"success\"}}}}");
    std::io::stdout().flush().unwrap();
    line.clear();
    input.read_line(&mut line).unwrap();
    assert!(line.contains("/object-store-connect"));
    println!("{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"fake-r2-session\"}}");
    std::io::stdout().flush().unwrap();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe)
        .unwrap();
    let mut client = crate::momso_mcp::test_bridge(file);
    let mut ready = true;
    let tools = crate::momso_mcp::test_response(
        &json!({"id":0,"method":"tools/list"}),
        &mut ready,
        &mut client,
    )
    .unwrap();
    assert!(tools.to_string().contains("momso_object_store_r2_connect"));
    let result = crate::momso_mcp::test_response(
        &json!({"id":1,"method":"tools/call","params":{"name":"momso_object_store_r2_connect","arguments":{}}}),
        &mut ready, &mut client,
    ).unwrap();
    assert!(result.to_string().contains("success"), "{result}");
    let marker = std::env::var_os("MOMSO_TEST_R2_FINISHED").unwrap();
    for _ in 0..1200 {
        if std::path::Path::new(&marker).is_file() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        std::path::Path::new(&marker).is_file(),
        "backup and purge did not finish"
    );
    println!("{{\"type\":\"stream_event\",\"event\":{{\"type\":\"content_block_delta\",\"delta\":{{\"type\":\"text_delta\",\"text\":\"OK\"}}}}}}");
    println!("{{\"type\":\"result\",\"is_error\":false}}");
    std::io::stdout().flush().unwrap();
    line.clear();
    input.read_line(&mut line).unwrap();
    println!("{{\"type\":\"control_response\",\"response\":{{\"request_id\":\"momso-context\",\"subtype\":\"success\",\"response\":{{\"model\":\"fake\",\"totalTokens\":1,\"maxTokens\":1000}}}}}}");
    std::io::stdout().flush().unwrap();
}

#[test]
fn backup_overview_never_launches_neon_or_a_browser() {
    if std::env::var_os("MOMSO_TEST_OVERVIEW_CHILD").is_some() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let database_path = root.join("momso.db");
        let backups_path = root.join("backups");
        std::fs::create_dir_all(&backups_path).unwrap();
        storage::initialize(&database_path, &backups_path, "test").unwrap();
        let database_for_test = database_path.clone();
        let archive =
            archive_location::ArchiveLocation::load(database_path.clone(), root, vec![]).unwrap();
        let objects_path = root.join("momso/objects");
        std::fs::create_dir_all(&objects_path).unwrap();
        let paths = RuntimePaths {
            data_root: root.to_path_buf(),
            database_path,
            objects_path,
            exports_path: root.join("exports"),
            backups_path,
            logs_path: root.join("logs"),
            archive,
            archive_lease: None,
        };
        let app = mock_builder()
            .manage(AppState {
                paths,
                approvals: Default::default(),
                setup_secrets: Default::default(),
                usage_monitor: Default::default(),
                usage_cache: Default::default(),
                chat_active: Default::default(),
                browser: Default::default(),
                selections: Default::default(),
            })
            .invoke_handler(tauri::generate_handler![
                user_owned_backup_overview,
                ai_models,
                object_store::object_store_status
            ])
            .build(mock_context(noop_assets()))
            .unwrap();
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let value = invoke(&window, "user_owned_backup_overview", json!({}));
        assert_eq!(value["recoveryKeyStored"], false);
        assert_eq!(value["recoveryKeyConfirmed"], false);
        assert_eq!(value["syncState"], "unavailable");
        assert!(value.get("neonOrganizations").is_none());
        assert!(value["profile"].get("neonProjectId").is_none());
        let connection = storage::open_writable_connection(&database_for_test).unwrap();
        connection.execute("UPDATE momso_backup_profiles SET neon_project_id='legacy-project', neon_branch_id='legacy-branch', neon_database_name='legacy-db', neon_role_name='legacy-role'", []).unwrap();
        drop(connection);
        let second = invoke(&window, "user_owned_backup_overview", json!({}));
        assert!(second["profile"].get("neonProjectId").is_none());
        let connection = storage::open_writable_connection(&database_for_test).unwrap();
        let legacy: String = connection
            .query_row(
                "SELECT neon_project_id FROM momso_backup_profiles",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy, "legacy-project");
        assert!(invoke(&window, "ai_models", json!({})).is_array());
        assert_eq!(
            invoke(&window, "object_store_status", json!({}))["state"],
            "not_connected"
        );
        tauri::async_runtime::block_on(async {
            for provider in ["codex", "claude", "gemini"] {
                let _ = provider_auth::read_status(provider).await;
            }
            let _ = usage_status::fetch_codex().await;
            let _ = usage_status::fetch_claude(root).await;
        });
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("browser-opened");
    std::fs::write(
        temp.path().join("neon.cmd"),
        "@echo off\r\necho opened > \"%MOMSO_TEST_BROWSER_MARKER%\"\r\n",
    )
    .unwrap();
    for tool in [
        "gh", "wrangler", "codex", "claude", "agy", "aws", "gcloud", "ncloud",
    ] {
        std::fs::write(temp.path().join(format!("{tool}.cmd")), "@echo off\r\nif /i \"%1\"==\"login\" echo opened > \"%MOMSO_TEST_BROWSER_MARKER%\"\r\nif /i \"%2\"==\"login\" echo opened > \"%MOMSO_TEST_BROWSER_MARKER%\"\r\nexit /b 1\r\n").unwrap();
    }
    let path = std::env::join_paths([
        temp.path().to_path_buf(),
        std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32"),
    ])
    .unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "command_entry_tests::backup_overview_never_launches_neon_or_a_browser",
            "--test-threads=1",
        ])
        .env("MOMSO_TEST_OVERVIEW_CHILD", "1")
        .env("MOMSO_TEST_FAKE_CREDENTIAL_STORE", "1")
        .env("MOMSO_TEST_BROWSER_MARKER", &marker)
        .env("PATH", path)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(!marker.exists());
}

#[test]
fn disconnect_object_store_keeps_local_and_remote_fixture() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let database_path = root.join("momso.db");
    let backups_path = root.join("backups");
    std::fs::create_dir_all(&backups_path).unwrap();
    storage::initialize(&database_path, &backups_path, "test").unwrap();
    let archive =
        archive_location::ArchiveLocation::load(database_path.clone(), root, vec![]).unwrap();
    let archive_root = root.join("momso");
    std::fs::create_dir_all(archive_root.join("objects")).unwrap();
    let local = archive_root.join("objects/source");
    let remote = root.join("remote-fixture");
    std::fs::write(&local, b"local fixture").unwrap();
    std::fs::write(&remote, b"remote fixture").unwrap();
    object_store::save_config(
        root,
        &object_store::Config {
            provider: "cloudflare".into(),
            endpoint: "https://fixture.r2.cloudflarestorage.com".into(),
            region: "wrangleroauth".into(),
            bucket: "momso-fixture".into(),
            credential_id: uuid::Uuid::new_v4().to_string(),
            last_backup_at: None,
            write_safety: Some(object_store::WriteSafety::CheckThenWrite),
        },
    )
    .unwrap();
    let paths = RuntimePaths {
        data_root: root.to_path_buf(),
        database_path,
        objects_path: archive_root.join("objects"),
        exports_path: root.join("exports"),
        backups_path,
        logs_path: root.join("logs"),
        archive,
        archive_lease: None,
    };
    let app = mock_builder()
        .manage(AppState {
            paths,
            approvals: Default::default(),
            setup_secrets: Default::default(),
            usage_monitor: Default::default(),
            usage_cache: Default::default(),
            chat_active: Default::default(),
            browser: Default::default(),
            selections: Default::default(),
        })
        .invoke_handler(tauri::generate_handler![
            object_store::object_store_disconnect
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    invoke(&window, "object_store_disconnect", json!({}));
    assert!(!root.join("object-store.json").exists());
    assert_eq!(std::fs::read(local).unwrap(), b"local fixture");
    assert_eq!(std::fs::read(remote).unwrap(), b"remote fixture");
}

#[test]
fn oauth_r2_commands_import_backup_purge_and_retry() {
    oauth_r2_flow(false, false, false);
}

#[cfg(windows)]
#[test]
fn claude_r2_login_approval_backup_and_remote_purge() {
    oauth_r2_flow(true, false, false);
}

#[test]
fn auto_upload_r2_batches_retries_disables_and_flushes() {
    if std::env::var_os("MOMSO_AUTO_UPLOAD_CHILD").is_some() {
        oauth_r2_flow(false, true, false);
        return;
    }
    let status = crate::process::hidden_command(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "command_entry_tests::auto_upload_r2_batches_retries_disables_and_flushes",
            "--test-threads=1",
        ])
        .env("MOMSO_AUTO_UPLOAD_CHILD", "1")
        .env("MOMSO_TEST_FAKE_CREDENTIAL_STORE", "1")
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn simultaneous_automatic_backups_and_close_timeout_resume() {
    if std::env::var_os("MOMSO_AUTO_BOTH_CHILD").is_some() {
        oauth_r2_flow(false, false, true);
        return;
    }
    let status = crate::process::hidden_command(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "command_entry_tests::simultaneous_automatic_backups_and_close_timeout_resume",
            "--test-threads=1",
        ])
        .env("MOMSO_AUTO_BOTH_CHILD", "1")
        .env("MOMSO_TEST_FAKE_CREDENTIAL_STORE", "1")
        .status()
        .unwrap();
    assert!(status.success());
}

fn oauth_r2_flow(with_claude: bool, automatic_upload: bool, concurrent: bool) {
    use std::{
        collections::BTreeMap,
        io::{Read, Write},
        net::TcpListener,
        sync::{
            atomic::{AtomicBool, Ordering},
            Mutex,
        },
    };
    #[cfg(windows)]
    struct ClaudeCleanup {
        marker: std::path::PathBuf,
        handle: Option<std::thread::JoinHandle<()>>,
    }
    #[cfg(windows)]
    impl Drop for ClaudeCleanup {
        fn drop(&mut self) {
            let _ = std::fs::write(&self.marker, b"done");
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }
    struct Cleanup {
        root: std::path::PathBuf,
        credential: String,
        stop: Arc<AtomicBool>,
        release: Arc<AtomicBool>,
        server: Option<std::thread::JoinHandle<()>>,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            object_store::unregister_test_config(&self.root);
            if let Ok(_guard) = credentials::prevent_secret_changes() {
                let _ = credentials::remove_secret_unlocked(&self.credential);
            }
            self.stop.store(true, Ordering::Relaxed);
            self.release.store(true, Ordering::SeqCst);
            if let Some(server) = self.server.take() {
                let _ = server.join();
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let db = root.join("momso.db");
    let backups = root.join("backups");
    std::fs::create_dir_all(&backups).unwrap();
    storage::initialize(&db, &backups, "test").unwrap();
    let profile = backup_profile::ensure(&db).unwrap();
    let (identity, _) = credentials::get_or_create_backup_identity(&profile.workspace_id).unwrap();
    storage::open_writable_connection(&db).unwrap().execute("UPDATE momso_backup_profiles SET recovery_key_confirmed_at='now' WHERE workspace_id=?1", [&profile.workspace_id]).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let objects = Arc::new(Mutex::new(BTreeMap::<String, Vec<u8>>::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let fail = Arc::new(AtomicBool::new(false));
    let fail_upload = Arc::new(AtomicBool::new(false));
    let pause_upload = Arc::new(AtomicBool::new(false));
    let upload_started = Arc::new(AtomicBool::new(false));
    let release_upload = Arc::new(AtomicBool::new(false));
    let bucket_created = Arc::new(AtomicBool::new(false));
    let (remote, stopped, fail_delete) = (objects.clone(), stop.clone(), fail.clone());
    let created = bucket_created.clone();
    let fail_next_upload = fail_upload.clone();
    let pause = pause_upload.clone();
    let started = upload_started.clone();
    let release = release_upload.clone();
    let server = std::thread::spawn(move || {
        let base = format!(
            "/client/v4/accounts/{}/r2/buckets/momso-basoon/objects",
            "a".repeat(32)
        );
        while !stopped.load(Ordering::Relaxed) {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(15)))
                .unwrap();
            let mut raw = Vec::new();
            let mut buffer = [0u8; 8192];
            let end = loop {
                let n = stream.read(&mut buffer).unwrap();
                assert_ne!(n, 0);
                raw.extend_from_slice(&buffer[..n]);
                if let Some(i) = raw.windows(4).position(|v| v == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let head = String::from_utf8(raw[..end].to_vec()).unwrap();
            let mut parts = head.lines().next().unwrap().split_whitespace();
            let method = parts.next().unwrap();
            let path = parts.next().unwrap();
            if !path.starts_with("/oauth/callback?") {
                assert!(head
                    .lines()
                    .any(|line| line.eq_ignore_ascii_case("authorization: Bearer test-token")));
            }
            let len = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|n| n.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            while raw.len() - end < len {
                let n = stream.read(&mut buffer).unwrap();
                assert_ne!(n, 0);
                raw.extend_from_slice(&buffer[..n]);
            }
            if method == "PUT" && pause.swap(false, Ordering::SeqCst) {
                started.store(true, Ordering::SeqCst);
                while !release.load(Ordering::SeqCst) && !stopped.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
            if fail_next_upload.swap(false, Ordering::SeqCst) {
                stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                continue;
            }
            let mut map = remote.lock().unwrap();
            let bucket_base = format!("/client/v4/accounts/{}/r2/buckets", "a".repeat(32));
            let (status, body) = if path == "/oauth/callback?code=fixture&state=fixture"
                && method == "GET"
            {
                ("200 OK", b"ok".to_vec())
            } else if path == format!("{bucket_base}/momso-basoon") && method == "GET" {
                if created.load(Ordering::SeqCst) {
                    ("200 OK", b"{}".to_vec())
                } else {
                    ("404 Not Found", vec![])
                }
            } else if path == bucket_base && method == "POST" {
                let value: Value = serde_json::from_slice(&raw[end..end + len]).unwrap();
                assert_eq!(value, json!({"name":"momso-basoon"}));
                created.store(true, Ordering::SeqCst);
                (
                    "200 OK",
                    serde_json::to_vec(&json!({"success":true,"result":{"name":"momso-basoon"}}))
                        .unwrap(),
                )
            } else if method == "GET" && path.starts_with(&format!("{base}?")) {
                let part = if path.contains("data%2F") {
                    "/data/"
                } else {
                    "/manifests/"
                };
                let entries: Vec<_> = map
                    .keys()
                    .filter(|key| key.contains(part))
                    .map(|key| json!({"key":key}))
                    .collect();
                ("200 OK", serde_json::to_vec(&json!({"success":true,"result":entries,"result_info":{"is_truncated":false}})).unwrap())
            } else {
                let key = path
                    .strip_prefix(&format!("{base}/"))
                    .expect("local R2 object path");
                match method {
                    "PUT" => {
                        map.insert(key.into(), raw[end..end + len].to_vec());
                        ("200 OK", b"{}".to_vec())
                    }
                    "GET" => map
                        .get(key)
                        .map(|v| ("200 OK", v.clone()))
                        .unwrap_or(("404 Not Found", vec![])),
                    "DELETE" if fail_delete.swap(false, Ordering::SeqCst) => {
                        ("403 Forbidden", vec![])
                    }
                    "DELETE" => {
                        map.remove(key);
                        ("200 OK", b"{}".to_vec())
                    }
                    _ => panic!("unexpected method"),
                }
            };
            if write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .is_ok()
            {
                let _ = stream.write_all(&body);
            }
        }
    });
    let _cleanup = Cleanup {
        root: root.to_path_buf(),
        credential: format!("backup:{}:age-identity", profile.workspace_id),
        stop,
        release: release_upload.clone(),
        server: Some(server),
    };
    let archive = archive_location::ArchiveLocation::load(db.clone(), root, vec![]).unwrap();
    let archive_root = root.join("momso");
    archive::initialize(&archive_root, root).unwrap();
    let git_remote = if concurrent {
        let tool = crate::process::resolve_tool("git.exe").unwrap();
        let bare = root.join("fake-github.git");
        archive_remote::git(&tool, root, &["init", "--bare", bare.to_str().unwrap()]).unwrap();
        Some(archive_remote::TestRemote {
            tool,
            target: bare.to_string_lossy().into_owned(),
        })
    } else {
        None
    };
    #[cfg(windows)]
    let mut claude_completion = None;
    #[cfg(windows)]
    if with_claude {
        assert_eq!(
            serde_json::to_value(archive_remote::username_set_db(&db, "basoon".into())).unwrap()
                ["state"],
            "ok"
        );
        let chat_id = serde_json::to_value(chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let endpoint = format!("http://{address}/");
        let test_root = root.to_path_buf();
        let test_db = db.clone();
        let handler_chat = chat_id.clone();
        let broker = approval::ApprovalBroker::default();
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let handler: setup_bridge::Handler = Arc::new(move |input| {
            let root = test_root.clone();
            let database = test_db.clone();
            let endpoint = endpoint.clone();
            let chat_id = handler_chat.clone();
            let broker = broker.clone();
            let ready = ready_tx.clone();
            Box::pin(async move {
                assert_eq!(input, json!({"method":"object_store_r2_connect"}));
                let result = r2_oauth::fake_setup_for_flow_test(
                    &root,
                    &database,
                    &endpoint,
                    &chat_id,
                    &broker,
                    tokio_util::sync::CancellationToken::new(),
                )
                .await;
                let _ = ready.send(result);
                json!({"status":setup_actions::r2_tool_status(result)})
            })
        });
        let marker = root.join("fake-claude-finished");
        let child_marker = marker.clone();
        let child_db = db.clone();
        let child_archive = archive_root.clone();
        let handle = std::thread::spawn(move || {
            tauri::async_runtime::block_on(async move {
                let guard = setup_bridge::start(
                    &chat_id,
                    tokio_util::sync::CancellationToken::new(),
                    handler,
                )
                .unwrap();
                let pipe = setup_bridge::name(&chat_id).unwrap();
                let mut command = process::hidden_async_command(std::env::current_exe().unwrap());
                command
                    .args([
                        "--exact",
                        "command_entry_tests::fake_claude_r2_child",
                        "--nocapture",
                        "--test-threads=1",
                    ])
                    .env("MOMSO_TEST_R2_PIPE", pipe)
                    .env("MOMSO_TEST_R2_FINISHED", child_marker);
                let mut emit = |_: &str, _: Value| {};
                chat_claude::run_command_ref(
                    command,
                    &child_db,
                    &child_archive,
                    &chat_id,
                    "원자료 저장소 연결",
                    &[],
                    Some("haiku"),
                    Some("low"),
                    false,
                    Some("object-store-connect"),
                    None,
                    &None,
                    None,
                    tokio_util::sync::CancellationToken::new(),
                    &mut emit,
                )
                .await
                .unwrap();
                drop(guard);
            })
        });
        claude_completion = Some(ClaudeCleanup {
            marker,
            handle: Some(handle),
        });
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(90))
            .unwrap()
            .unwrap();
        assert!(bucket_created.load(Ordering::SeqCst));
        let config: object_store::Config =
            serde_json::from_slice(&std::fs::read(root.join("object-store.json")).unwrap())
                .unwrap();
        assert_eq!(config.bucket, "momso-basoon");
        object_store::register_test_config(root, config);
    } else {
        object_store::register_test_config(
            root,
            object_store::Config {
                provider: "cloudflare".into(),
                endpoint: format!("http://{address}"),
                region: "wrangleroauth".into(),
                bucket: "momso-basoon".into(),
                credential_id: uuid::Uuid::new_v4().to_string(),
                last_backup_at: None,
                write_safety: Some(object_store::WriteSafety::CheckThenWrite),
            },
        );
    }
    #[cfg(not(windows))]
    {
        let _ = with_claude;
        object_store::register_test_config(
            root,
            object_store::Config {
                provider: "cloudflare".into(),
                endpoint: format!("http://{address}"),
                region: "wrangleroauth".into(),
                bucket: "momso-basoon".into(),
                credential_id: uuid::Uuid::new_v4().to_string(),
                last_backup_at: None,
                write_safety: Some(object_store::WriteSafety::CheckThenWrite),
            },
        );
    }
    let paths = RuntimePaths {
        data_root: root.to_path_buf(),
        database_path: db.clone(),
        objects_path: archive_root.join("objects"),
        exports_path: root.join("exports"),
        backups_path: backups,
        logs_path: root.join("logs"),
        archive,
        archive_lease: None,
    };
    let app = mock_builder()
        .manage(AppState {
            paths: paths.clone(),
            approvals: Default::default(),
            setup_secrets: Default::default(),
            usage_monitor: Default::default(),
            usage_cache: Default::default(),
            chat_active: Default::default(),
            browser: Default::default(),
            selections: Default::default(),
        })
        .manage(object_store_auto_upload::AutoUpload::default())
        .manage(archive_auto_backup::AutoBackup::default());
    let app = if let Some(remote) = git_remote.clone() {
        app.manage(remote)
    } else {
        app
    };
    let app = app
        .invoke_handler(tauri::generate_handler![
            plan_source_asset_import,
            apply_source_asset_import,
            chat_create,
            chat_attach_asset,
            object_store_transfer::object_store_backup,
            object_store_auto_upload::object_store_auto_upload_get,
            object_store_auto_upload::object_store_auto_upload_set,
            object_store_state::app_state_backup,
            source_delete::source_asset_delete_preview,
            source_delete::source_asset_trash,
            source_delete::source_asset_purge,
            source_delete::source_asset_deleted_list
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let mut imported = Vec::new();
    for (name, content) in [
        ("target.txt", "target fixture"),
        ("other.txt", "other fixture"),
    ] {
        let file = root.join(name);
        std::fs::write(&file, content).unwrap();
        app.state::<AppState>().selections.remember(
            std::slice::from_ref(&file),
            selection_grants::Purpose::Source,
        );
        let proposal = json!({"sourcePath":file,"assetKind":"raw_transcript","mediaType":"text/plain","recordLink":null,"actor":"fixture"});
        let plan = invoke(
            &window,
            "plan_source_asset_import",
            json!({"proposal":proposal}),
        );
        let asset = invoke(
            &window,
            "apply_source_asset_import",
            json!({"input":{"proposal":proposal,"planDigest":plan["planDigest"],"importConfirmed":true}}),
        );
        imported.push(asset);
    }
    let chat = invoke(&window, "chat_create", json!({}));
    crate::chat::append(
        &db,
        chat["id"].as_str().unwrap(),
        crate::chat::Role::User,
        "hello",
        None,
        None,
    )
    .unwrap();
    let attachment = invoke(
        &window,
        "chat_attach_asset",
        json!({"chatId":chat["id"],"assetId":imported[0]["id"]}),
    );
    assert!(attachment.is_string());
    if concurrent {
        while !crate::chat_redaction_recheck::step(&archive_root, &paths)
            .unwrap()
            .available
        {}
        let remote = git_remote.clone().unwrap();
        let show_chat = |id: &str| {
            archive_remote::git(
                &remote.tool,
                root,
                &[
                    "--git-dir",
                    &remote.target,
                    "show",
                    &format!("refs/heads/main:chats/{id}.json"),
                ],
            )
            .is_ok()
        };
        let chat_id = chat["id"].as_str().unwrap().to_owned();
        pause_upload.store(true, Ordering::SeqCst);
        release_upload.store(false, Ordering::SeqCst);
        upload_started.store(false, Ordering::SeqCst);
        tauri::async_runtime::block_on(async {
            let handle = app.handle().clone();
            let upload = tauri::async_runtime::spawn(async move {
                object_store_auto_upload::attempt(handle, true).await;
            });
            tokio::time::timeout(std::time::Duration::from_secs(90), async {
                while !upload_started.load(Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
            let github = tokio::time::timeout(
                std::time::Duration::from_secs(90),
                archive_auto_backup::attempt(app.handle().clone(), true),
            )
            .await;
            let saved_while_upload_waited = show_chat(&chat_id);
            let github_status = serde_json::to_value(
                archive_auto_backup::archive_auto_backup_get(app.state(), app.state()).unwrap(),
            )
            .unwrap();
            release_upload.store(true, Ordering::SeqCst);
            upload.await.unwrap();
            assert!(github.is_ok());
            assert!(
                saved_while_upload_waited,
                "GitHub backup must finish while R2 is held: {github_status}"
            );
        });
        assert!(!objects.lock().unwrap().is_empty());
        let before = objects.lock().unwrap().len();
        std::fs::write(
            archive_root.join("objects/fixture-close-retry"),
            b"pending after close",
        )
        .unwrap();
        let next_chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        crate::chat::append(
            &db,
            &next_chat,
            crate::chat::Role::User,
            "hello",
            None,
            None,
        )
        .unwrap();
        pause_upload.store(true, Ordering::SeqCst);
        upload_started.store(false, Ordering::SeqCst);
        release_upload.store(false, Ordering::SeqCst);
        let elapsed = std::time::Instant::now();
        let close_result = tauri::async_runtime::block_on(archive_auto_backup::close_all(
            app.handle().clone(),
            paths.clone(),
            root.to_path_buf(),
        ));
        assert!(upload_started.load(Ordering::SeqCst));
        assert!(elapsed.elapsed() >= std::time::Duration::from_secs(19));
        assert!(elapsed.elapsed() < std::time::Duration::from_secs(24));
        assert!(close_result.is_err() || matches!(close_result, Ok((_, false))));
        release_upload.store(true, Ordering::SeqCst);
        drop(window);
        drop(app);
        let restarted = mock_builder()
            .manage(AppState {
                paths: paths.clone(),
                approvals: Default::default(),
                setup_secrets: Default::default(),
                usage_monitor: Default::default(),
                usage_cache: Default::default(),
                chat_active: Default::default(),
                browser: Default::default(),
                selections: Default::default(),
            })
            .manage(object_store_auto_upload::AutoUpload::default())
            .manage(archive_auto_backup::AutoBackup::default())
            .manage(remote.clone())
            .build(mock_context(noop_assets()))
            .unwrap();
        tauri::async_runtime::block_on(async {
            for _ in 0..30 {
                archive_auto_backup::attempt(restarted.handle().clone(), true).await;
                object_store_auto_upload::attempt(restarted.handle().clone(), true).await;
                let state = serde_json::to_value(
                    object_store_auto_upload::object_store_auto_upload_get(
                        restarted.state(),
                        restarted.state(),
                    )
                    .unwrap(),
                )
                .unwrap();
                if show_chat(&next_chat)
                    && objects.lock().unwrap().len() > before
                    && state["state"] == "idle"
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        });
        assert!(show_chat(&next_chat));
        assert!(objects.lock().unwrap().len() > before);
        assert_eq!(
            serde_json::to_value(
                object_store_auto_upload::object_store_auto_upload_get(
                    restarted.state(),
                    restarted.state()
                )
                .unwrap()
            )
            .unwrap()["state"],
            "idle"
        );
        return;
    }
    if automatic_upload {
        let get = || invoke(&window, "object_store_auto_upload_get", json!({}));
        assert_eq!(get()["enabled"], true);
        tauri::async_runtime::block_on(object_store_auto_upload::attempt(
            app.handle().clone(),
            false,
        ));
        assert_eq!(get()["state"], "pending");
        assert!(objects.lock().unwrap().is_empty());
        let receive_marker = archive_root.join(".git/momso-first-receive-in-progress");
        std::fs::write(&receive_marker, b"fixture").unwrap();
        object_store_auto_upload::expedite(&app.state::<object_store_auto_upload::AutoUpload>());
        tauri::async_runtime::block_on(object_store_auto_upload::attempt(
            app.handle().clone(),
            false,
        ));
        assert_eq!(get()["reason"], "first_receive_in_progress");
        assert!(objects.lock().unwrap().is_empty());
        std::fs::remove_file(receive_marker).unwrap();
        object_store_auto_upload::expedite(&app.state::<object_store_auto_upload::AutoUpload>());
        fail_upload.store(true, Ordering::SeqCst);
        tauri::async_runtime::block_on(object_store_auto_upload::attempt(
            app.handle().clone(),
            false,
        ));
        assert_eq!(get()["state"], "retry_pending");
        assert_eq!(get()["consecutiveFailures"], 2);
        object_store_auto_upload::expedite(&app.state::<object_store_auto_upload::AutoUpload>());
        tauri::async_runtime::block_on(object_store_auto_upload::attempt(
            app.handle().clone(),
            false,
        ));
        assert_eq!(get()["state"], "idle");
        assert!(!objects.lock().unwrap().is_empty());
        assert_eq!(
            invoke(
                &window,
                "object_store_auto_upload_set",
                json!({"enabled":false})
            )["state"],
            "disabled"
        );
        let before = objects.lock().unwrap().len();
        std::fs::write(
            archive_root.join("objects/fixture-after-disable"),
            b"pending fixture",
        )
        .unwrap();
        tauri::async_runtime::block_on(object_store_auto_upload::attempt(
            app.handle().clone(),
            true,
        ));
        assert_eq!(objects.lock().unwrap().len(), before);
        assert_eq!(
            invoke(
                &window,
                "object_store_auto_upload_set",
                json!({"enabled":true})
            )["state"],
            "pending"
        );
        assert!(tauri::async_runtime::block_on(
            object_store_auto_upload::close_upload(app.handle().clone())
        ));
        assert_eq!(get()["state"], "idle");
        assert!(objects.lock().unwrap().len() > before);
    } else {
        let backup = invoke(&window, "object_store_backup", json!({}));
        assert_eq!(backup["completedCount"], 2);
    }
    assert_eq!(
        invoke(&window, "app_state_backup", json!({}))["state"],
        "complete"
    );
    let mut target_keys = Vec::new();
    let mut other_keys = Vec::new();
    let mut state_keys = Vec::new();
    for (key, value) in objects
        .lock()
        .unwrap()
        .iter()
        .filter(|(key, _)| key.contains("/manifests/"))
    {
        let decoded = object_store_transfer::decrypt_bytes(value, &identity, 1024 * 1024).unwrap();
        let manifest: object_store_transfer::Manifest = serde_json::from_slice(&decoded).unwrap();
        let dest = if manifest.kind == "app_state" {
            &mut state_keys
        } else if manifest.sha256 == imported[0]["contentSha256"].as_str().unwrap() {
            &mut target_keys
        } else {
            &mut other_keys
        };
        dest.push(key.clone());
        dest.extend(
            (0..manifest.chunks.len()).map(|i| object_store_transfer::chunk_key(&manifest, i)),
        );
    }
    assert!(!target_keys.is_empty() && !other_keys.is_empty() && !state_keys.is_empty());
    let id = imported[0]["id"].as_str().unwrap();
    let preview = invoke(
        &window,
        "source_asset_delete_preview",
        json!({"assetId":id}),
    );
    let trash = invoke(
        &window,
        "source_asset_trash",
        json!({"assetId":id,"planDigest":preview["planDigest"]}),
    );
    assert_eq!(trash["state"], "trashed");
    let preview = invoke(
        &window,
        "source_asset_delete_preview",
        json!({"assetId":id}),
    );
    fail.store(true, Ordering::SeqCst);
    let result = invoke(
        &window,
        "source_asset_purge",
        json!({"assetId":id,"planDigest":preview["planDigest"],"confirmed":true}),
    );
    assert_eq!(result["state"], "remote_retry_pending");
    let listed = invoke(
        &window,
        "source_asset_deleted_list",
        json!({"before":null,"limit":10}),
    );
    assert_eq!(listed["items"][0]["originalFileName"], "target.txt");
    assert_eq!(listed["items"][0]["deletionState"], "remote_retry_pending");
    let preview = invoke(
        &window,
        "source_asset_delete_preview",
        json!({"assetId":id}),
    );
    let result = invoke(
        &window,
        "source_asset_purge",
        json!({"assetId":id,"planDigest":preview["planDigest"],"confirmed":true}),
    );
    assert_ne!(result["state"], "remote_retry_pending");
    assert_eq!(
        invoke(
            &window,
            "source_asset_deleted_list",
            json!({"before":null,"limit":10})
        )["items"],
        json!([])
    );
    let map = objects.lock().unwrap();
    assert!(target_keys.iter().all(|key| !map.contains_key(key)));
    assert!(other_keys.iter().all(|key| map.contains_key(key)));
    assert!(state_keys.iter().all(|key| map.contains_key(key)));
    drop(map);
    if automatic_upload {
        tauri::async_runtime::block_on(object_store_auto_upload::attempt(
            app.handle().clone(),
            true,
        ));
        let map = objects.lock().unwrap();
        for (key, value) in map.iter().filter(|(key, _)| key.contains("/manifests/")) {
            let decoded =
                object_store_transfer::decrypt_bytes(value, &identity, 1024 * 1024).unwrap();
            let manifest: object_store_transfer::Manifest =
                serde_json::from_slice(&decoded).unwrap();
            assert_ne!(
                manifest.sha256,
                imported[0]["contentSha256"].as_str().unwrap(),
                "purged source must not be uploaded again: {key}"
            );
        }
    }
    #[cfg(windows)]
    if let Some(mut completion) = claude_completion {
        std::fs::write(&completion.marker, b"done").unwrap();
        completion.handle.take().unwrap().join().unwrap();
    }
}

#[test]
fn webview_import_rejects_unselected_and_consumes_selected_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let database_path = root.join("momso.db");
    let backups_path = root.join("backups");
    std::fs::create_dir_all(&backups_path).unwrap();
    storage::initialize(&database_path, &backups_path, "test").unwrap();
    let archive =
        archive_location::ArchiveLocation::load(database_path.clone(), root, vec![]).unwrap();
    let archive_root = root.join("momso");
    archive::initialize(&archive_root, root).unwrap();
    let paths = RuntimePaths {
        data_root: root.to_path_buf(),
        database_path,
        objects_path: archive_root.join("objects"),
        exports_path: root.join("exports"),
        backups_path,
        logs_path: root.join("logs"),
        archive,
        archive_lease: None,
    };
    let app = mock_builder()
        .manage(AppState {
            paths,
            approvals: approval::ApprovalBroker::default(),
            setup_secrets: setup_secret::SecretBroker::default(),
            usage_monitor: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            usage_cache: Arc::new(usage_status::UsageCache::default()),
            chat_active: Default::default(),
            browser: Default::default(),
            selections: Default::default(),
        })
        .invoke_handler(tauri::generate_handler![
            plan_source_asset_import,
            apply_source_asset_import,
            attachment_text_read,
            chat_import_export,
            chat_import_export_preview,
            chat_import_export_progress,
            load_local_ai_conversations,
            archive_location::archive_location_set,
            object_store_restore::object_store_recover_original_copy,
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let file = root.join("unselected-fixture.txt");
    std::fs::write(&file, "synthetic-private-fixture").unwrap();
    let proposal = json!({"sourcePath":file.to_str().unwrap(),"assetKind":"raw_transcript","mediaType":"text/plain","recordLink":null,"actor":"fixture"});
    let reject = |cmd: &str, body: Value, expected: &str| {
        let result = get_ipc_response(
            &window,
            InvokeRequest {
                cmd: cmd.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "http://tauri.localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.into(),
            },
        );
        let error = result
            .expect_err("the unselected or unsafe path must be rejected")
            .to_string();
        assert!(error.contains(expected), "{cmd}: {error}");
    };
    let unselected = file.to_str().unwrap();
    reject(
        "apply_source_asset_import",
        json!({"input":{"proposal":proposal,"planDigest":"unused","importConfirmed":true}}),
        "selection_required",
    );
    for (cmd, body) in [
        (
            "chat_import_export",
            json!({"path":unselected,"provider":"chatgpt"}),
        ),
        (
            "chat_import_export_preview",
            json!({"path":unselected,"provider":"chatgpt"}),
        ),
        (
            "chat_import_export_progress",
            json!({"path":unselected,"provider":"chatgpt","requestId":"fixture"}),
        ),
        (
            "archive_location_set",
            json!({"path":root.to_str().unwrap()}),
        ),
    ] {
        reject(cmd, body, "selection_required");
    }
    reject(
        "load_local_ai_conversations",
        json!({"provider":"codex","conversationIds":[unselected]}),
        "invalid_conversation_id",
    );
    let too_long = "a".repeat(33000);
    for path in [
        "../outside",
        "C:\\outside",
        "\\\\server\\share\\file",
        "CON",
        "folder/NUL",
        "folder/end.",
        "folder/end ",
        too_long.as_str(),
    ] {
        let bad_proposal = json!({"sourcePath":path,"assetKind":"raw_transcript","mediaType":"text/plain","recordLink":null,"actor":"fixture"});
        reject(
            "plan_source_asset_import",
            json!({"proposal":bad_proposal}),
            "selection_required",
        );
        reject(
            "apply_source_asset_import",
            json!({"input":{"proposal":bad_proposal,"planDigest":"unused","importConfirmed":true}}),
            "selection_required",
        );
        reject(
            "chat_import_export",
            json!({"path":path,"provider":"chatgpt"}),
            "selection_required",
        );
        reject(
            "chat_import_export_preview",
            json!({"path":path,"provider":"chatgpt"}),
            "selection_required",
        );
        reject(
            "chat_import_export_progress",
            json!({"path":path,"provider":"chatgpt","requestId":"fixture"}),
            "selection_required",
        );
        reject(
            "archive_location_set",
            json!({"path":path}),
            "selection_required",
        );
        reject(
            "load_local_ai_conversations",
            json!({"provider":"codex","conversationIds":[path]}),
            "invalid_conversation_id",
        );
        reject(
            "object_store_recover_original_copy",
            json!({"relativePath":path,"expectedSha256":"a".repeat(64)}),
            "invalid_id",
        );
    }
    let denied = get_ipc_response(
        &window,
        InvokeRequest {
            cmd: "plan_source_asset_import".into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(json!({"proposal":proposal})),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.into(),
        },
    );
    assert!(denied.is_err());
    app.state::<AppState>().selections.remember(
        std::slice::from_ref(&file),
        selection_grants::Purpose::Source,
    );
    let plan = invoke(
        &window,
        "plan_source_asset_import",
        json!({"proposal":proposal}),
    );
    reject(
        "apply_source_asset_import",
        json!({"input":{"proposal":proposal,"planDigest":"fixture-wrong","importConfirmed":true}}),
        "changed",
    );
    let imported = invoke(
        &window,
        "apply_source_asset_import",
        json!({"input":{"proposal":proposal,"planDigest":plan["planDigest"],"importConfirmed":true}}),
    );
    let opened = invoke(
        &window,
        "attachment_text_read",
        json!({"assetId":imported["id"],"cursor":0}),
    );
    assert_eq!(opened["content"], "synthetic-private-fixture");
    assert!(app
        .state::<AppState>()
        .selections
        .check(
            file.to_str().unwrap(),
            selection_grants::Purpose::Source,
            false
        )
        .is_err());
}

#[test]
fn source_purge_without_store_disappears_through_desktop_commands() {
    source_purge_without_remote_delete(false);
}

#[test]
fn oauth_r2_purge_without_recovery_key_reports_retry() {
    source_purge_without_remote_delete(true);
}

fn source_purge_without_remote_delete(oauth_r2: bool) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    if oauth_r2 {
        object_store::save_config(
            root,
            &object_store::Config {
                provider: "cloudflare".into(),
                endpoint: "https://fixture.r2.cloudflarestorage.com".into(),
                region: "wrangleroauth".into(),
                bucket: "momso-fixture".into(),
                credential_id: uuid::Uuid::new_v4().to_string(),
                last_backup_at: None,
                write_safety: Some(object_store::WriteSafety::CheckThenWrite),
            },
        )
        .unwrap();
    }
    let database_path = root.join("momso.db");
    let backups_path = root.join("backups");
    std::fs::create_dir_all(&backups_path).unwrap();
    storage::initialize(&database_path, &backups_path, "test").unwrap();
    let archive =
        archive_location::ArchiveLocation::load(database_path.clone(), root, vec![]).unwrap();
    let archive_root = root.join("momso");
    for folder in ["objects", "zettels", "receipts", ".git"] {
        std::fs::create_dir_all(archive_root.join(folder)).unwrap();
    }
    let contents = b"synthetic private lesson";
    let hash = object_store::digest(contents);
    let object = archive_root
        .join("objects/sha256")
        .join(&hash[..2])
        .join(&hash);
    std::fs::create_dir_all(object.parent().unwrap()).unwrap();
    std::fs::write(&object, contents).unwrap();
    let connection = storage::open_writable_connection(&database_path).unwrap();
    connection.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset-1','other',?1,?2,?3,'lesson.wav','test','now')", rusqlite::params![hash,contents.len() as i64,format!("sha256/{}/{hash}",&hash[..2])]).unwrap();
    drop(connection);
    let paths = RuntimePaths {
        data_root: root.to_path_buf(),
        database_path,
        objects_path: archive_root.join("objects"),
        exports_path: root.join("exports"),
        backups_path,
        logs_path: root.join("logs"),
        archive,
        archive_lease: None,
    };
    let app = mock_builder()
        .manage(AppState {
            paths,
            approvals: approval::ApprovalBroker::default(),
            setup_secrets: setup_secret::SecretBroker::default(),
            usage_monitor: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            usage_cache: Arc::new(usage_status::UsageCache::default()),
            chat_active: Default::default(),
            browser: Default::default(),
            selections: Default::default(),
        })
        .invoke_handler(tauri::generate_handler![
            source_delete::source_asset_delete_preview,
            source_delete::source_asset_deleted_list,
            source_delete::source_asset_trash,
            source_delete::source_asset_purge,
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let preview = invoke(
        &window,
        "source_asset_delete_preview",
        json!({"assetId":"asset-1"}),
    );
    let trash = invoke(
        &window,
        "source_asset_trash",
        json!({"assetId":"asset-1","planDigest":preview["planDigest"]}),
    );
    assert_eq!(trash["state"], "trashed");
    let listed = invoke(
        &window,
        "source_asset_deleted_list",
        json!({"before":null,"limit":10}),
    );
    assert_eq!(listed["items"][0]["originalFileName"], "lesson.wav");
    assert_eq!(listed["items"][0]["canRestore"], true);
    let preview = invoke(
        &window,
        "source_asset_delete_preview",
        json!({"assetId":"asset-1"}),
    );
    let purged = invoke(
        &window,
        "source_asset_purge",
        json!({"assetId":"asset-1","planDigest":preview["planDigest"],"confirmed":true}),
    );
    assert_eq!(
        purged["state"],
        if oauth_r2 {
            "remote_retry_pending"
        } else {
            "partial"
        }
    );
    assert!(purged["remoteRemaining"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v
            == if oauth_r2 {
                "remote_access:recovery_key_missing"
            } else {
                "remote_access:not_connected"
            }));
    assert!(!object.exists());
    let listed = invoke(
        &window,
        "source_asset_deleted_list",
        json!({"before":null,"limit":10}),
    );
    assert_eq!(listed["items"], json!([]));
}

#[test]
fn fresh_folder_list_before_archive_initialization_is_empty() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let database_path = root.join("momso.db");
    let backups_path = root.join("backups");
    std::fs::create_dir_all(&backups_path).unwrap();
    storage::initialize(&database_path, &backups_path, "test").unwrap();
    let archive =
        archive_location::ArchiveLocation::load(database_path.clone(), root, vec![]).unwrap();
    let paths = RuntimePaths {
        data_root: root.to_path_buf(),
        database_path,
        objects_path: root.join("momso/objects"),
        exports_path: root.join("exports"),
        backups_path,
        logs_path: root.join("logs"),
        archive,
        archive_lease: None,
    };
    let app = mock_builder()
        .manage(AppState {
            paths,
            approvals: Default::default(),
            setup_secrets: Default::default(),
            usage_monitor: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            usage_cache: Default::default(),
            chat_active: Default::default(),
            browser: Default::default(),
            selections: Default::default(),
        })
        .invoke_handler(tauri::generate_handler![zet_folders])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    assert_eq!(invoke(&window, "zet_folders", json!({})), json!([]));
}

#[test]
fn fresh_chat_commands_use_tauri_entry_points() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let database_path = root.join("momso.db");
    let backups_path = root.join("backups");
    std::fs::create_dir_all(&backups_path).unwrap();
    storage::initialize(&database_path, &backups_path, "test").unwrap();
    let archive =
        archive_location::ArchiveLocation::load(database_path.clone(), root, vec![]).unwrap();
    let archive_root = root.join("momso");
    archive::initialize(&archive_root, root).unwrap();
    let paths = RuntimePaths {
        data_root: root.to_path_buf(),
        database_path,
        objects_path: archive_root.join("objects"),
        exports_path: root.join("exports"),
        backups_path,
        logs_path: root.join("logs"),
        archive,
        archive_lease: None,
    };
    let app = mock_builder()
        .manage(AppState {
            paths,
            approvals: approval::ApprovalBroker::default(),
            setup_secrets: setup_secret::SecretBroker::default(),
            usage_monitor: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            usage_cache: Arc::new(usage_status::UsageCache::default()),
            chat_active: Default::default(),
            browser: Default::default(),
            selections: Default::default(),
        })
        .invoke_handler(tauri::generate_handler![
            chat_list,
            chat_create,
            chat_create_with_message,
            chat_read,
            chat_rename,
            chat_search,
            chat_archived_list,
            chat_group_list,
            chat_group_create,
            chat_group_rename,
            chat_group_reorder,
            chat_group_remove,
            chat_set_group,
            user_profile_get,
            zet_list,
            archive_reindex,
            permission_mode_get,
            chat_import_export,
            chat_import_export_preview,
            zet_create,
            zet_read,
            zet_update,
            chat_items,
            archive_remote::momso_username_get,
            archive_remote::momso_username_set,
            archive_location::archive_location_get,
            object_store::object_store_status,
            chat_archive_status::chat_archive_sync_status
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let remote_panel = tauri::WebviewWindowBuilder::new(&app, "panel-browser", Default::default())
        .build()
        .unwrap();
    for (cmd, body) in [
        ("chat_list", json!({})),
        ("chat_group_list", json!({})),
        ("momso_username_set", json!({"username": "fixture"})),
    ] {
        let response = get_ipc_response(
            &remote_panel,
            InvokeRequest {
                cmd: cmd.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "https://example.test/".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.into(),
            },
        );
        assert!(response.is_err(), "remote panel reached {cmd}");
    }
    assert_eq!(
        archive_remote::username_get_db(&app.state::<AppState>().paths.database_path).unwrap(),
        None
    );
    assert_eq!(invoke(&window, "chat_list", json!({})), json!([]));
    assert_eq!(invoke(&window, "chat_archived_list", json!({})), json!([]));
    assert_eq!(invoke(&window, "chat_group_list", json!({})), json!([]));
    assert_eq!(
        invoke(&window, "object_store_status", json!({}))["state"],
        "not_connected"
    );
    assert_eq!(
        invoke(&window, "chat_archive_sync_status", json!({}))["state"],
        "saved"
    );
    while !chat_redaction_recheck::step(&archive_root, &app.state::<AppState>().paths)
        .unwrap()
        .available
    {}
    assert_eq!(
        invoke(&window, "chat_archive_sync_status", json!({}))["state"],
        "saved"
    );
    assert!(invoke(&window, "archive_location_get", json!({}))["path"].is_string());
    invoke(&window, "momso_username_set", json!({"username":"example"}));
    assert_eq!(
        invoke(&window, "momso_username_get", json!({}))["username"],
        "example"
    );
    assert_eq!(invoke(&window, "zet_list", json!({})), json!([]));
    assert_eq!(invoke(&window, "user_profile_get", json!({}))["name"], "");
    assert!(invoke(&window, "permission_mode_get", json!({"provider":"codex"})).is_string());
    invoke(&window, "archive_reindex", json!({}));
    let empty = invoke(&window, "chat_create", json!({}));
    assert!(empty["id"].is_string());
    assert_eq!(invoke(&window, "chat_list", json!({})), json!([]));
    let created = invoke(&window, "chat_create_with_message", json!({"text":"hello"}));
    let id = created["chat"]["id"].as_str().unwrap();
    assert!(created["messageId"].is_string());
    let group = invoke(&window, "chat_group_create", json!({"name":"Yoga"}));
    let group_id = group["id"].as_str().unwrap();
    invoke(
        &window,
        "chat_group_rename",
        json!({"id":group_id,"name":"Practice"}),
    );
    invoke(&window, "chat_group_reorder", json!({"ids":[group_id]}));
    invoke(
        &window,
        "chat_set_group",
        json!({"chatId":id,"groupId":group_id}),
    );
    invoke(&window, "chat_group_remove", json!({"id":group_id}));
    assert_eq!(invoke(&window, "chat_read", json!({"id": id}))["id"], id);
    invoke(&window, "chat_rename", json!({"id": id, "title": "Yoga"}));
    assert_eq!(invoke(&window, "chat_list", json!({}))[0]["title"], "Yoga");
    assert_eq!(
        invoke(&window, "chat_search", json!({"query": "Yoga"}))[0]["id"],
        id
    );
    let export_path = root.join("conversations.json");
    std::fs::write(&export_path, json!([{"id":"source","current_node":"a","mapping":{"a":{"parent":null,"message":{"id":"m1","author":{"role":"user"},"content":{"parts":["hello"]}}}}}]).to_string()).unwrap();
    app.state::<AppState>().selections.remember(
        std::slice::from_ref(&export_path),
        selection_grants::Purpose::Conversation,
    );
    let preview = invoke(
        &window,
        "chat_import_export_preview",
        json!({"path":export_path.to_str().unwrap(),"provider":"chatgpt"}),
    );
    assert_eq!(preview["conversations"], 1);
    let result = invoke(
        &window,
        "chat_import_export",
        json!({"path":export_path.to_str().unwrap(),"provider":"chatgpt"}),
    );
    assert_eq!(result["imported"], 1);
    assert!(app
        .state::<AppState>()
        .selections
        .check(
            export_path.to_str().unwrap(),
            selection_grants::Purpose::Conversation,
            false
        )
        .is_err());
    let rooms = invoke(&window, "chat_list", json!({}));
    let rooms = rooms.as_array().unwrap();
    assert_eq!(rooms.len(), 2);
    let imported = rooms
        .iter()
        .find(|room| room["importedFrom"] == "chatgpt")
        .unwrap();
    let imported_id = imported["id"].as_str().unwrap();
    assert!(root
        .join("momso")
        .join("chats")
        .join(format!("{imported_id}.json"))
        .is_file());
    let document = invoke(
        &window,
        "zet_create",
        json!({"input":{"title":"Yoga","summary":"Practice","kind":"note","body":"First","assets":[],"edges":[]}}),
    );
    let document_id = document.as_str().unwrap();
    assert_eq!(
        invoke(&window, "zet_read", json!({"id":document_id}))["body"],
        "First"
    );
    invoke(
        &window,
        "zet_update",
        json!({"id":document_id,"changes":{"body":"Second"}}),
    );
    assert_eq!(
        invoke(&window, "zet_read", json!({"id":document_id}))["body"],
        "Second"
    );
    assert!(invoke(&window, "chat_items", json!({"chatId":id})).is_object());
}
