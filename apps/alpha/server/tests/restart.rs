#![cfg(feature = "test-fixtures")]

use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn launch(data: &Path, cli: &Path, address: &str) -> Server {
    launch_with(data, cli, address, None, false)
}

#[cfg(all(target_os = "linux", debug_assertions))]
#[test]
fn purge_old_backup_restore_and_restart_pending() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let credentials = temp.path().join("credentials");
    std::fs::create_dir_all(&credentials).unwrap();
    std::fs::write(credentials.join("momso-vault-identity"), "fixture").unwrap();
    let fake = temp.path().join("fake_backup.py");
    let module =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/momso_anywhere_backup.py");
    std::fs::write(
        &fake,
        format!(
            "MOMSO_FAKE_BACKUP_MODULE = {}\n{}",
            serde_json::to_string(&module.to_string_lossy()).unwrap(),
            include_str!("fixtures/fake_backup.py")
        ),
    )
    .unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let start = || {
        Server(
            Command::new(env!("CARGO_BIN_EXE_momso-server"))
                .env("MOMSO_SERVER_TOKEN", "test-only-token")
                .env("MOMSO_SERVER_DATA_DIR", &data)
                .env("MOMSO_SERVER_LISTEN", &address)
                .env("MOMSO_CODEX_PATH", env!("CARGO_BIN_EXE_fake-codex"))
                .env("MOMSO_TEST_DISABLE_AUTO_BACKUP", "1")
                .env("MOMSO_BACKUP_AGENT", &fake)
                .env("MOMSO_TEST_ALLOW_BACKUP_AGENT", "1")
                .env("CREDENTIALS_DIRECTORY", &credentials)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    };
    let server = start();
    ready(&address);
    let upload = |bytes: &[u8]| {
        let started = request(
            &address,
            "POST",
            "/uploads",
            &json!({"fileName":"fixture.txt","mediaType":"text/plain","byteLength":bytes.len()})
                .to_string(),
        )
        .unwrap();
        let id = started["uploadId"].as_str().unwrap();
        put_bytes(&address, &format!("/uploads/{id}/chunks?offset=0"), bytes).unwrap();
        request(&address, "POST", &format!("/uploads/{id}/finish"), "{}").unwrap()
    };
    let first = upload(b"old backup fixture");
    let id = first["id"].as_str().unwrap();
    let hash = first["sha256"].as_str().unwrap();
    let old = Command::new("python3")
        .arg(&fake)
        .arg("backup")
        .env("MOMSO_SERVER_DATA_DIR", &data)
        .env("MOMSO_BACKUP_QUIESCENT", "1")
        .env("CREDENTIALS_DIRECTORY", &credentials)
        .output()
        .unwrap();
    assert!(
        old.status.success(),
        "{}",
        String::from_utf8_lossy(&old.stderr)
    );
    request(&address, "POST", &format!("/assets/{id}/trash"), "{}").unwrap();
    let result = request(&address, "POST", &format!("/assets/{id}/purge"), "{}").unwrap();
    assert_eq!(result["backupCopiesRemain"], true);
    assert!(!data
        .join("archive/objects/sha256")
        .join(&hash[..2])
        .join(hash)
        .exists());
    let second = upload(b"restart fixture");
    let id2 = second["id"].as_str().unwrap();
    let hash2 = second["sha256"].as_str().unwrap();
    request(&address, "POST", &format!("/assets/{id2}/trash"), "{}").unwrap();
    let block = temp.path().join("fake-object-store/block-purge");
    std::fs::write(&block, "fixture").unwrap();
    assert!(
        raw_post(&address, &format!("/assets/{id2}/purge")).contains("purge_backup_unavailable")
    );
    let activity = request(&address, "GET", "/activity", "").unwrap();
    assert_eq!(activity["pendingPurges"], 1);
    assert_eq!(activity["busy"], true);
    assert_eq!(activity["shutdownReady"], false);
    assert!(data
        .join("archive/objects/sha256")
        .join(&hash2[..2])
        .join(hash2)
        .exists());
    drop(server);
    std::fs::remove_file(block).unwrap();
    let restarted = start();
    ready(&address);
    let object2 = data
        .join("archive/objects/sha256")
        .join(&hash2[..2])
        .join(hash2);
    let end = Instant::now() + Duration::from_secs(20);
    while object2.exists() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !object2.exists(),
        "pending original was not removed after restart"
    );
    drop(restarted);
    let restored = temp.path().join("restored");
    let output = Command::new("python3")
        .arg(&fake)
        .arg("restore")
        .env("MOMSO_SERVER_DATA_DIR", &restored)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!restored
        .join("archive/objects/sha256")
        .join(&hash[..2])
        .join(hash)
        .exists());
    let db = rusqlite::Connection::open(restored.join("attachments/attachments.sqlite3")).unwrap();
    assert!(db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM asset_purged WHERE asset_id=?1)",
            [id],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
}
fn launch_with(
    data: &Path,
    cli: &Path,
    address: &str,
    web: Option<&Path>,
    fail_once: bool,
) -> Server {
    let binary = std::env::var_os("MOMSO_TEST_SERVER_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_momso-server").into());
    let mut cmd = Command::new(binary);
    cmd.env("MOMSO_SERVER_DATA_DIR", data)
        .env("MOMSO_SERVER_TOKEN", "test-only-token")
        .env("MOMSO_SERVER_LISTEN", address)
        .env("MOMSO_CODEX_PATH", cli)
        .env_remove("CREDENTIALS_DIRECTORY")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(web) = web {
        cmd.env("MOMSO_SERVER_WEB_DIR", web);
    } else {
        cmd.env_remove("MOMSO_SERVER_WEB_DIR");
    }
    if fail_once {
        cmd.env("MOMSO_TEST_PROVIDER_FAIL_ONCE", "1");
    } else {
        cmd.env_remove("MOMSO_TEST_PROVIDER_FAIL_ONCE");
    }
    Server(cmd.spawn().unwrap())
}
fn request(address: &str, method: &str, path: &str, body: &str) -> Option<Value> {
    let mut stream = TcpStream::connect(address).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    write!(stream,"{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-only-token\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).ok()?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).ok()?;
    let (head, body) = raw.split_once("\r\n\r\n")?;
    if !head.contains(" 200 ") {
        return None;
    }
    serde_json::from_str(body).ok()
}
fn put_bytes(address: &str, path: &str, body: &[u8]) -> Option<Value> {
    let mut stream = TcpStream::connect(address).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(stream,"PUT {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-only-token\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).ok()?;
    stream.write_all(body).ok()?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).ok()?;
    let (head, body) = raw.split_once("\r\n\r\n")?;
    if !head.contains(" 200 ") {
        return None;
    }
    serde_json::from_str(body).ok()
}
fn ready(address: &str) {
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        if request(address, "GET", "/health", "").is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    panic!("server did not become ready");
}

#[cfg(unix)]
#[test]
fn customer_minimal_environment_starts_empty_and_s32_database() {
    let temp = tempfile::tempdir().unwrap();
    let web = temp.path().join("web");
    std::fs::create_dir(&web).unwrap();
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    for old in [false, true] {
        let data = temp.path().join(if old { "s32" } else { "empty" });
        std::fs::create_dir(&data).unwrap();
        if old {
            let db = rusqlite::Connection::open(data.join("server.sqlite3")).unwrap();
            db.execute_batch("CREATE TABLE chats(id TEXT PRIMARY KEY,title TEXT NOT NULL,thread_id TEXT); CREATE TABLE chat_messages(id TEXT PRIMARY KEY,chat_id TEXT NOT NULL,role TEXT NOT NULL,text TEXT NOT NULL,created_at TEXT NOT NULL,provider TEXT,model TEXT,effort TEXT,skill TEXT,actual_effort TEXT,finish_reason TEXT); CREATE TABLE turns(id TEXT PRIMARY KEY,chat_id TEXT NOT NULL,state TEXT NOT NULL,text TEXT NOT NULL DEFAULT '',error TEXT); CREATE TABLE approvals(id TEXT PRIMARY KEY,turn_id TEXT NOT NULL,chat_id TEXT NOT NULL,kind TEXT NOT NULL,description TEXT NOT NULL,options TEXT NOT NULL,state TEXT NOT NULL); CREATE TABLE browser_events(id INTEGER PRIMARY KEY,turn_id TEXT NOT NULL,chat_id TEXT NOT NULL,reason TEXT NOT NULL,origin TEXT NOT NULL); CREATE TABLE otp_redaction_meta(id INTEGER PRIMARY KEY,fingerprint BLOB NOT NULL); CREATE TABLE otp_redactions(digest BLOB NOT NULL,length INTEGER NOT NULL,UNIQUE(digest,length)); INSERT INTO chats VALUES('old-chat','이전 방',NULL); INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('old-msg','old-chat','user','이전 말','2026-01-01T00:00:00Z');").unwrap();
        }
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap().to_string();
        drop(socket);
        let child = Command::new(env!("CARGO_BIN_EXE_momso-server"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("MOMSO_SERVER_DATA_DIR", &data)
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_LISTEN", &address)
            .env("MOMSO_SERVER_WEB_DIR", &web)
            .env("MOMSO_CODEX_PATH", &cli)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let _server = Server(child);
        ready(&address);
        if old {
            let chat = request(&address, "GET", "/chats/old-chat", "").unwrap();
            assert_eq!(chat["messages"][0]["text"], "이전 말");
        }
    }
}

#[test]
fn startup_diagnostics_identify_file_without_leaking_token() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("not-a-directory");
    std::fs::write(&data, "fixture").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_momso-server"))
        .env("MOMSO_SERVER_TOKEN", "never-print-this-token")
        .env("MOMSO_SERVER_DATA_DIR", &data)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("stage=create_data file=."));
    assert!(stderr.contains("os_error="));
    assert!(!stderr.contains("never-print-this-token"));
}

#[cfg(unix)]
#[test]
fn missing_git_reports_archive_stage_and_os_error() {
    let temp = tempfile::tempdir().unwrap();
    let empty_path = temp.path().join("empty-path");
    std::fs::create_dir(&empty_path).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_momso-server"))
        .env_clear()
        .env("PATH", &empty_path)
        .env("MOMSO_SERVER_TOKEN", "never-print-this-token")
        .env("MOMSO_SERVER_DATA_DIR", temp.path().join("data"))
        .env("MOMSO_SERVER_LISTEN", "127.0.0.1:0")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("stage=execute file=git"));
    assert!(stderr.contains("os_error=Some(2)"));
    assert!(stderr.contains("stage=initialize_archive file=archive reason=git_unavailable"));
    assert!(!stderr.contains("never-print-this-token"));
}
fn read_events(address: &str, turn: &str) -> String {
    read_events_after(address, turn, None)
}
fn read_events_after(address: &str, turn: &str, after: Option<usize>) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let suffix = after.map(|n| format!("?after={n}")).unwrap_or_default();
    write!(stream, "GET /turns/{turn}/events{suffix} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-only-token\r\nConnection: close\r\n\r\n").unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    raw
}
fn raw_get(address: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    raw
}

#[cfg(target_os = "linux")]
#[test]
fn github_repository_list_requires_auth_and_vault() {
    let temp = tempfile::tempdir().unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let child = Command::new(env!("CARGO_BIN_EXE_momso-server"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("MOMSO_SERVER_TOKEN", "test-only-token")
        .env("MOMSO_SERVER_DATA_DIR", temp.path().join("data"))
        .env("MOMSO_SERVER_LISTEN", &address)
        .env("MOMSO_GITHUB_CLIENT_ID", "fixture-client-id")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _server = Server(child);
    ready(&address);
    assert!(raw_get(&address, "/github/repositories").contains(" 401 "));
    let mut stream = TcpStream::connect(&address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(stream, "GET /github/repositories HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-only-token\r\nConnection: close\r\n\r\n").unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    assert!(raw.contains(" 503 "));
    assert!(raw.contains("credential_unavailable"));
}

#[cfg(target_os = "linux")]
#[test]
fn maintenance_cannot_release_with_pending_chat_restore() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let _server = launch(&data, &cli, &address);
    ready(&address);
    request(&address, "POST", "/maintenance", "{}").unwrap();
    std::fs::write(data.join("chat-restore.pending"), "fixture").unwrap();
    assert!(raw_post(&address, "/maintenance/release").contains("restore_incomplete"));
    assert_eq!(
        request(&address, "GET", "/activity", "").unwrap()["maintenance"],
        true
    );
}

#[cfg(all(target_os = "linux", debug_assertions))]
#[test]
fn github_repository_list_uses_local_fake_and_hides_token() {
    use age::secrecy::ExposeSecret;
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let credentials = temp.path().join("credentials");
    std::fs::create_dir_all(&credentials).unwrap();
    let identity = age::x25519::Identity::generate().to_string();
    let key = credentials.join("momso-vault-identity");
    std::fs::write(&key, identity.expose_secret()).unwrap();
    let vault = momso_saved_login_core::linux::LinuxVault::from_credential_file(
        &data.join("saved-logins"),
        &key,
    )
    .unwrap();
    vault
        .store_service_secret("github-access-token", "fixture-secret-token")
        .unwrap();
    let fake = TcpListener::bind("127.0.0.1:0").unwrap();
    let fake_address = fake.local_addr().unwrap();
    let worker = std::thread::spawn(move || {
        for mut stream in fake.incoming().take(4).map(Result::unwrap) {
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut buf = [0; 8192];
            let count = stream.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..count]);
            assert!(req
                .to_ascii_lowercase()
                .contains("authorization: bearer fixture-secret-token"));
            let body = if req.starts_with("GET /user ") {
                r#"{"login":"fixture"}"#
            } else if req.starts_with("GET /user/repos?") {
                r#"[{"full_name":"fixture/private","private":true,"default_branch":"main"},{"full_name":"fixture/public","private":false,"default_branch":"main"}]"#
            } else if req.starts_with("GET /repos/fixture/private ") {
                r#"{"full_name":"fixture/private","private":true,"default_branch":"main"}"#
            } else {
                assert!(req.starts_with("GET /repos/fixture/public "));
                r#"{"full_name":"fixture/public","private":false,"default_branch":"main"}"#
            };
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        }
    });
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let server = Server(
        Command::new(env!("CARGO_BIN_EXE_momso-server"))
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_DATA_DIR", &data)
            .env("MOMSO_SERVER_LISTEN", &address)
            .env("MOMSO_GITHUB_CLIENT_ID", "fixture-client-id")
            .env("MOMSO_TEST_GITHUB_API", format!("http://{fake_address}/"))
            .env("MOMSO_TEST_DISABLE_AUTO_BACKUP", "1")
            .env("CREDENTIALS_DIRECTORY", &credentials)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address);
    let result = request(&address, "GET", "/github/repositories", "").unwrap();
    assert_eq!(result["account"], "fixture");
    assert_eq!(result["repositories"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["repositories"][0]["nameWithOwner"],
        "fixture/private"
    );
    assert!(!result.to_string().contains("fixture-secret-token"));
    let selected = request(
        &address,
        "POST",
        "/github/repository",
        r#"{"nameWithOwner":"fixture/private"}"#,
    )
    .unwrap();
    assert_eq!(selected["repository"]["nameWithOwner"], "fixture/private");
    assert_eq!(selected["backupStatus"], "queued");
    assert!(request(
        &address,
        "POST",
        "/github/repository",
        r#"{"nameWithOwner":"fixture/public"}"#,
    )
    .is_none());
    let loaded = request(&address, "GET", "/github/repository", "").unwrap();
    assert_eq!(loaded["repository"]["nameWithOwner"], "fixture/private");
    let saved = std::fs::read_to_string(data.join("github-repository.json")).unwrap();
    assert!(!saved.contains("fixture-secret-token"));
    assert_eq!(
        serde_json::from_str::<Value>(&saved).unwrap()["nameWithOwner"],
        "fixture/private"
    );
    worker.join().unwrap();
    drop(server);
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let next = socket.local_addr().unwrap().to_string();
    drop(socket);
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let _restarted = launch(&data, &cli, &next);
    ready(&next);
    let loaded = request(&next, "GET", "/github/repository", "").unwrap();
    assert_eq!(loaded["repository"]["nameWithOwner"], "fixture/private");
}

#[cfg(all(target_os = "linux", debug_assertions))]
#[test]
fn independent_backups_run_for_all_connection_and_failure_combinations() {
    use age::secrecy::ExposeSecret;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    for (github, object, fail_github, fail_object) in [
        (false, false, false, false),
        (false, true, false, false),
        (true, false, false, false),
        (true, true, false, false),
        (true, true, true, false),
        (true, true, false, true),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let credentials = temp.path().join("credentials");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&credentials).unwrap();
        let key = credentials.join("momso-vault-identity");
        std::fs::write(
            &key,
            age::x25519::Identity::generate()
                .to_string()
                .expose_secret(),
        )
        .unwrap();
        if github {
            let vault = momso_saved_login_core::linux::LinuxVault::from_credential_file(
                &data.join("saved-logins"),
                &key,
            )
            .unwrap();
            vault
                .store_service_secret("github-access-token", "fixture-token")
                .unwrap();
            std::fs::write(
                data.join("github-repository.json"),
                r#"{"nameWithOwner":"fixture/private","defaultBranch":"main"}"#,
            )
            .unwrap();
        }
        let remote = temp.path().join("remote.git");
        assert!(Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&remote)
            .output()
            .unwrap()
            .status
            .success());
        let fake = TcpListener::bind("127.0.0.1:0").unwrap();
        let fake_address = fake.local_addr().unwrap();
        fake.set_nonblocking(true).unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let object_marker = temp.path().join("object-finished");
        let wait_marker = object_marker.clone();
        let object_before_github = Arc::new(AtomicBool::new(false));
        let observed = object_before_github.clone();
        let worker = std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                if let Ok((mut stream, _)) = fake.accept() {
                    let mut buf = [0; 8192];
                    let _ = stream.read(&mut buf);
                    let (status, body) = if fail_github {
                        let deadline = Instant::now() + Duration::from_secs(5);
                        while !wait_marker.exists() && Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        observed.store(wait_marker.exists(), Ordering::SeqCst);
                        (503, "{}")
                    } else {
                        (
                            200,
                            r#"{"full_name":"fixture/private","private":true,"default_branch":"main"}"#,
                        )
                    };
                    let _ = write!(stream, "HTTP/1.1 {status} fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        });
        let agent = temp.path().join("object.py");
        let git_seen = temp.path().join("object-saw-git");
        std::fs::write(
            &agent,
            if fail_object {
                format!("from pathlib import Path\nimport time\nprint('momso-backup-v3:snapshot-ready', flush=True)\np=Path({})\nfor _ in range(500):\n if p.exists():\n  Path({}).touch()\n  break\n time.sleep(0.01)\nraise SystemExit(1)\n", serde_json::to_string(&remote.join("refs/heads/main").to_string_lossy()).unwrap(), serde_json::to_string(&git_seen.to_string_lossy()).unwrap())
            } else {
                format!("from pathlib import Path\nPath({}).touch()\nprint('momso-backup-v3:uploaded')\n", serde_json::to_string(&object_marker.to_string_lossy()).unwrap())
            },
        )
        .unwrap();
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap().to_string();
        drop(socket);
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_momso-server"));
        cmd.env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_DATA_DIR", &data)
            .env("MOMSO_SERVER_LISTEN", &address)
            .env("MOMSO_CODEX_PATH", env!("CARGO_BIN_EXE_fake-codex"))
            .env("MOMSO_GITHUB_CLIENT_ID", "fixture")
            .env("MOMSO_TEST_GITHUB_API", format!("http://{fake_address}/"))
            .env("MOMSO_TEST_GITHUB_GIT", &remote)
            .env("CREDENTIALS_DIRECTORY", &credentials)
            .env_remove("MOMSO_BACKUP_AGENT")
            .env("MOMSO_TEST_ALLOW_BACKUP_AGENT", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if object {
            cmd.env("MOMSO_BACKUP_AGENT", &agent);
        }
        let server = Server(cmd.spawn().unwrap());
        ready(&address);
        let expected_git = if !github {
            "not_configured"
        } else if fail_github {
            "failed"
        } else {
            "saved"
        };
        let expected_object = if !object {
            "not_configured"
        } else if fail_object {
            "failed"
        } else {
            "saved"
        };
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            let status = request(&address, "GET", "/activity", "").unwrap();
            if status["githubBackup"]["state"] == expected_git
                && status["objectBackup"]["state"] == expected_object
            {
                if expected_git == "saved" {
                    assert!(status["githubBackup"]["lastSuccessAt"].is_i64());
                    assert!(
                        std::fs::read_to_string(data.join("github-backup-last-success"))
                            .unwrap()
                            .parse::<i64>()
                            .is_ok()
                    );
                }
                if expected_object == "saved" {
                    assert!(status["objectBackup"]["lastSuccessAt"].is_i64());
                    assert!(
                        std::fs::read_to_string(data.join("object-backup-last-success"))
                            .unwrap()
                            .parse::<i64>()
                            .is_ok()
                    );
                }
                if fail_github {
                    assert!(status["githubBackup"]["reason"].is_string());
                    assert!(object_before_github.load(Ordering::SeqCst));
                }
                if fail_object {
                    assert!(status["objectBackup"]["reason"].is_string());
                    assert!(git_seen.exists());
                }
                break;
            }
            assert!(Instant::now() < deadline, "case github={github} object={object} fail_github={fail_github} fail_object={fail_object}: {status}");
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(server);
        stopped.store(true, Ordering::SeqCst);
        worker.join().unwrap();
    }
}

#[cfg(all(target_os = "linux", debug_assertions))]
#[test]
fn object_backup_releases_requests_after_snapshot_while_upload_continues() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let release = temp.path().join("release");
    let agent = temp.path().join("agent.py");
    std::fs::write(&agent, format!("from pathlib import Path\nimport time\nprint('momso-backup-v3:snapshot-ready', flush=True)\np=Path({})\nfor _ in range(1000):\n if p.exists(): break\n time.sleep(0.01)\nprint('momso-backup-v3:uploaded')\n", serde_json::to_string(&release.to_string_lossy()).unwrap())).unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let _server = Server(
        Command::new(env!("CARGO_BIN_EXE_momso-server"))
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_DATA_DIR", &data)
            .env("MOMSO_SERVER_LISTEN", &address)
            .env("MOMSO_CODEX_PATH", env!("CARGO_BIN_EXE_fake-codex"))
            .env("MOMSO_BACKUP_AGENT", &agent)
            .env("MOMSO_TEST_ALLOW_BACKUP_AGENT", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address);
    let deadline = Instant::now() + Duration::from_secs(8);
    while request(&address, "GET", "/activity", "").unwrap()["objectBackup"]["state"] != "syncing" {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let started = Instant::now();
    let document = request(
        &address,
        "POST",
        "/documents",
        r#"{"title":"fixture","body":"example","summary":"example"}"#,
    );
    assert!(
        document.is_some(),
        "document creation must not wait for the upload"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!release.exists());
    std::fs::write(&release, b"done").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = request(&address, "GET", "/activity", "").unwrap();
        if status["objectBackup"]["state"] == "pending"
            && status["objectBackup"]["lastSuccessAt"].is_i64()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "successful earlier snapshot was not recorded: {status}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(all(target_os = "linux", debug_assertions))]
#[test]
fn github_automatic_backup_saves_chat_to_local_bare_remote() {
    use age::secrecy::ExposeSecret;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let credentials = temp.path().join("credentials");
    std::fs::create_dir_all(&credentials).unwrap();
    let identity = age::x25519::Identity::generate().to_string();
    let key = credentials.join("momso-vault-identity");
    std::fs::write(&key, identity.expose_secret()).unwrap();
    let vault = momso_saved_login_core::linux::LinuxVault::from_credential_file(
        &data.join("saved-logins"),
        &key,
    )
    .unwrap();
    vault
        .store_service_secret("github-access-token", "fixture-secret-token")
        .unwrap();
    let remote = temp.path().join("remote.git");
    assert!(Command::new("git")
        .args(["init", "--bare", "--initial-branch=main"])
        .arg(&remote)
        .output()
        .unwrap()
        .status
        .success());
    let fake = TcpListener::bind("127.0.0.1:0").unwrap();
    let fake_address = fake.local_addr().unwrap();
    fake.set_nonblocking(true).unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let worker = std::thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            if let Ok((mut stream, _)) = fake.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut buf = [0; 8192];
                let count = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..count]);
                let body = if req.starts_with("GET /repos/fixture/private ") {
                    r#"{"full_name":"fixture/private","private":true,"default_branch":"main"}"#
                } else {
                    "{}"
                };
                let _ = write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    });
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let fake_backup = temp.path().join("fake_backup.py");
    let backup_module = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scripts/momso_anywhere_backup.py");
    std::fs::write(
        &fake_backup,
        format!(
            "MOMSO_FAKE_BACKUP_MODULE = {}\n{}",
            serde_json::to_string(&backup_module.to_string_lossy()).unwrap(),
            include_str!("fixtures/fake_backup.py")
        ),
    )
    .unwrap();
    let _server = Server(
        Command::new(env!("CARGO_BIN_EXE_momso-server"))
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_DATA_DIR", &data)
            .env("MOMSO_SERVER_LISTEN", &address)
            .env("MOMSO_CODEX_PATH", &cli)
            .env("MOMSO_GITHUB_CLIENT_ID", "fixture-client-id")
            .env("MOMSO_TEST_GITHUB_API", format!("http://{fake_address}/"))
            .env("MOMSO_TEST_GITHUB_GIT", &remote)
            .env("MOMSO_BACKUP_AGENT", &fake_backup)
            .env("MOMSO_TEST_ALLOW_BACKUP_AGENT", "1")
            .env("CREDENTIALS_DIRECTORY", &credentials)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address);
    let preferences = request(&address, "GET", "/preferences", "").unwrap();
    request(&address, "PUT", "/preferences", &json!({"revision":preferences["revision"],"preferences":{"assistantProvider":"claude","chatModel":null}}).to_string()).unwrap();
    let media_digest = "af6ad103ff906555df9d187134df31e0e58a8daf94e3792cb1404dcad5a817e6";
    let object = data.join("archive/objects/sha256/af").join(media_digest);
    std::fs::create_dir_all(object.parent().unwrap()).unwrap();
    std::fs::write(&object, b"fixture media").unwrap();
    let document = request(
        &address,
        "POST",
        "/documents",
        r#"{"title":"수업","summary":"요약","body":"복원할 글","folder":"수업"}"#,
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let upload = request(
        &address,
        "POST",
        "/uploads",
        r#"{"fileName":"수업.txt","mediaType":"text/plain","byteLength":11}"#,
    )
    .unwrap()["uploadId"]
        .as_str()
        .unwrap()
        .to_owned();
    request(
        &address,
        "PUT",
        &format!("/uploads/{upload}/chunks?offset=0"),
        "hello world",
    )
    .unwrap();
    let asset = request(&address, "POST", &format!("/uploads/{upload}/finish"), "{}").unwrap()
        ["id"]
        .as_str()
        .unwrap()
        .to_owned();
    request(
        &address,
        "POST",
        &format!("/chats/{chat}/attachments"),
        &json!({"assetId":asset}).to_string(),
    )
    .unwrap();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":"fixture conversation","attachments":[{"assetId":asset}]}).to_string(),
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_state(&address, &turn, "completed");
    let media_turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":"generated tool fixture"}).to_string(),
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        wait_state(&address, &media_turn, "completed")["text"],
        "generated registered"
    );
    let generated = request(&address, "GET", "/media", "").unwrap()["items"][0]["assetId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        request(&address, "POST", &format!("/assets/{asset}/trash"), "{}").unwrap()["deleted"],
        true
    );
    let group = request(&address, "POST", "/chat-groups", r#"{"name":"classes"}"#).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let empty_group = request(&address, "POST", "/chat-groups", r#"{"name":"empty"}"#).unwrap()
        ["id"]
        .as_str()
        .unwrap()
        .to_owned();
    request(
        &address,
        "PUT",
        &format!("/chats/{chat}/group"),
        &json!({"groupId":group}).to_string(),
    )
    .unwrap();
    request(
        &address,
        "POST",
        &format!("/chats/{chat}/pin"),
        r#"{"pinned":true}"#,
    )
    .unwrap();
    request(
        &address,
        "POST",
        &format!("/chats/{chat}/archive"),
        r#"{"archived":true}"#,
    )
    .unwrap();
    request(
        &address,
        "POST",
        "/github/repository",
        r#"{"nameWithOwner":"fixture/private"}"#,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let status = request(&address, "GET", "/activity", "").unwrap();
        if (status["backupStatus"] == "saved_partial" || status["backupStatus"] == "saved")
            && status["githubBackupReady"] == true
        {
            break;
        }
        assert_ne!(
            status["backupStatus"], "failed",
            "fake backup failed: {status}"
        );
        assert!(
            Instant::now() < deadline,
            "fake backup did not finish: {status}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let status = request(&address, "GET", "/activity", "").unwrap();
        if status["shutdownReady"] == true {
            assert_eq!(status["maintenance"], false);
            assert_eq!(status["maintenanceReady"], false);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "idle was not ready before maintenance: {status}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let before = request(&address, "GET", "/activity", "").unwrap();
    let gate = request(&address, "POST", "/maintenance", "{}").unwrap();
    let closed = gate["closedAt"].as_i64().expect("gate closure time");
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let status = request(&address, "GET", "/activity", "").unwrap();
        if status["githubBackupReady"] == true
            && status["maintenance"] == true
            && status["shutdownReady"] == true
            && status["objectBackup"]["lastSuccessAt"]
                .as_i64()
                .is_some_and(|at| at > closed)
            && status["githubBackup"]["lastSuccessAt"]
                .as_i64()
                .is_some_and(|at| at > closed)
        {
            assert_eq!(status["maintenanceReady"], true);
            assert!(
                status["objectBackup"]["lastSuccessAt"].as_i64()
                    > before["objectBackup"]["lastSuccessAt"].as_i64()
            );
            break;
        }
        assert_ne!(
            status["backupStatus"], "failed",
            "final fake backup failed: {status}"
        );
        assert!(
            Instant::now() < deadline,
            "final fake backup did not finish: {status}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    request(&address, "POST", "/maintenance/release", "{}").unwrap();
    // The old VM is no longer active while the replacement restores and writes.
    // Otherwise its periodic worker races the replacement's fake Git remote.
    drop(_server);
    let recovered = temp.path().join("object-restored-data");
    assert!(Command::new("python3")
        .arg(&fake_backup)
        .arg("restore")
        .env("MOMSO_SERVER_DATA_DIR", &recovered)
        .output()
        .unwrap()
        .status
        .success());
    assert_eq!(
        std::fs::read(
            recovered
                .join("archive/objects/sha256/af")
                .join(media_digest)
        )
        .unwrap(),
        b"fixture media"
    );
    let recovered_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let recovered_address = recovered_listener.local_addr().unwrap().to_string();
    drop(recovered_listener);
    let _recovered_server = Server(
        Command::new(env!("CARGO_BIN_EXE_momso-server"))
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_DATA_DIR", &recovered)
            .env("MOMSO_SERVER_LISTEN", &recovered_address)
            .env("MOMSO_CODEX_PATH", &cli)
            .env("MOMSO_TEST_DISABLE_AUTO_BACKUP", "1")
            .env("MOMSO_BACKUP_AGENT", &fake_backup)
            .env("MOMSO_TEST_ALLOW_BACKUP_AGENT", "1")
            .env("CREDENTIALS_DIRECTORY", &credentials)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&recovered_address);
    let restored_chat = request(&recovered_address, "GET", &format!("/chats/{chat}"), "").unwrap();
    assert!(restored_chat.to_string().contains("fixture conversation"));
    let checkout = temp.path().join("restored");
    assert!(Command::new("git")
        .arg("clone")
        .arg(&remote)
        .arg(&checkout)
        .output()
        .unwrap()
        .status
        .success());
    let snapshot =
        std::fs::read_to_string(checkout.join("chats").join(format!("chat_{chat}.json"))).unwrap();
    assert!(snapshot.contains("fixture conversation"));
    assert!(!snapshot.contains("fixture-secret-token"));
    let data2 = temp.path().join("new-data");
    let vault2 = momso_saved_login_core::linux::LinuxVault::from_credential_file(
        &data2.join("saved-logins"),
        &key,
    )
    .unwrap();
    vault2
        .store_service_secret("github-access-token", "fixture-secret-token")
        .unwrap();
    let socket2 = TcpListener::bind("127.0.0.1:0").unwrap();
    let address2 = socket2.local_addr().unwrap().to_string();
    drop(socket2);
    let _second = Server(
        Command::new(env!("CARGO_BIN_EXE_momso-server"))
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_DATA_DIR", &data2)
            .env("MOMSO_SERVER_LISTEN", &address2)
            .env("MOMSO_CODEX_PATH", &cli)
            .env("MOMSO_GITHUB_CLIENT_ID", "fixture-client-id")
            .env("MOMSO_TEST_GITHUB_API", format!("http://{fake_address}/"))
            .env("MOMSO_TEST_GITHUB_GIT", &remote)
            .env("MOMSO_TEST_DISABLE_AUTO_BACKUP", "1")
            .env("MOMSO_BACKUP_AGENT", &fake_backup)
            .env("MOMSO_TEST_ALLOW_BACKUP_AGENT", "1")
            .env("CREDENTIALS_DIRECTORY", &credentials)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address2);
    request(
        &address2,
        "POST",
        "/github/repository",
        r#"{"nameWithOwner":"fixture/private"}"#,
    )
    .unwrap();
    assert!(raw_post(&address2, "/github/restore").contains("maintenance_required"));
    request(&address2, "POST", "/maintenance", "{}").unwrap();
    for _ in 0..2 {
        let response = raw_post(&address2, "/github/restore");
        assert!(
            response.contains(" 200 "),
            "fake restore failed: {response}, diagnostic: {:?}",
            std::fs::read_to_string(temp.path().join("fake-object-store/diagnostic"))
        );
        let restored: Value =
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(restored["restoredChats"], 1);
        assert_eq!(restored["documentsRestored"], true);
        assert_eq!(restored["restoredDocuments"], 1);
        assert_eq!(restored["attachmentsRestored"], true);
    }
    request(&address2, "POST", "/maintenance/release", "{}").unwrap();
    assert_eq!(
        request(&address2, "GET", "/preferences", "").unwrap()["preferences"]["assistantProvider"],
        "claude"
    );
    let archived = request(&address2, "GET", "/chats/archived", "").unwrap();
    assert_eq!(archived["chats"][0]["id"], chat);
    assert_eq!(archived["chats"][0]["pinned"], true);
    let restored = request(&address2, "GET", &format!("/chats/{chat}"), "").unwrap();
    assert_eq!(restored["groupId"], group);
    let restored_media = request(&address2, "GET", "/media", "").unwrap();
    assert_eq!(restored_media["items"][0]["assetId"], generated);
    assert_eq!(restored_media["items"][0]["prompt"], "draw fixture");
    let groups = request(&address2, "GET", "/chat-groups", "").unwrap();
    assert!(groups["groups"]
        .as_array()
        .unwrap()
        .iter()
        .any(|g| g["id"] == empty_group));
    assert_eq!(restored["messages"][0]["text"], "fixture conversation");
    assert_eq!(restored["messages"][0]["attachments"][0]["assetId"], asset);
    assert_eq!(restored["messages"][0]["attachments"][0]["deleted"], true);
    let mut denied = TcpStream::connect(&address2).unwrap();
    write!(denied, "GET /assets/{asset}/download HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-only-token\r\nConnection: close\r\n\r\n").unwrap();
    let mut denied_response = String::new();
    denied.read_to_string(&mut denied_response).unwrap();
    assert!(denied_response.contains(" 409 "));
    assert!(denied_response.contains("asset_trashed"));
    assert_eq!(
        request(&address2, "POST", &format!("/assets/{asset}/restore"), "{}").unwrap()["deleted"],
        false
    );
    let mut download = TcpStream::connect(&address2).unwrap();
    download
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(download, "GET /chats/{chat}/attachments/{asset} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-only-token\r\nConnection: close\r\n\r\n").unwrap();
    let mut received = Vec::new();
    download.read_to_end(&mut received).unwrap();
    assert!(received.windows(5).any(|part| part == b" 200 "));
    assert!(received.ends_with(b"hello world"));
    let restored_document =
        request(&address2, "GET", &format!("/documents/{document}"), "").unwrap();
    assert_eq!(restored_document["body"], "복원할 글");
    assert_eq!(restored_document["title"], "수업");
    assert_eq!(restored_document["summary"], "요약");
    assert_eq!(restored_document["folder"], "수업");
    let revision = restored_document["revision"].as_str().unwrap();
    request(&address2, "PUT", &format!("/documents/{document}"), &json!({"title":"수업","summary":"요약","body":"이 기기에서 고친 글","folder":"수업","revision":revision}).to_string()).unwrap();
    request(&address2, "POST", "/maintenance", "{}").unwrap();
    assert!(raw_post(&address2, "/github/restore").contains("restore_conflict"));
    request(&address2, "POST", "/maintenance/release", "{}").unwrap();
    let preserved = request(&address2, "GET", &format!("/documents/{document}"), "").unwrap();
    assert_eq!(preserved["body"], "이 기기에서 고친 글");
    drop(_second);
    let _restarted = Server(
        Command::new(env!("CARGO_BIN_EXE_momso-server"))
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_DATA_DIR", &data2)
            .env("MOMSO_SERVER_LISTEN", &address2)
            .env("MOMSO_CODEX_PATH", &cli)
            .env("MOMSO_GITHUB_CLIENT_ID", "fixture-client-id")
            .env("MOMSO_TEST_GITHUB_API", format!("http://{fake_address}/"))
            .env("MOMSO_TEST_GITHUB_GIT", &remote)
            .env("CREDENTIALS_DIRECTORY", &credentials)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address2);
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let status = request(&address2, "GET", "/activity", "").unwrap();
        if status["githubBackupReady"] == true {
            break;
        }
        assert_ne!(
            status["backupStatus"], "failed",
            "restored archive cannot back up: {status}"
        );
        assert!(
            Instant::now() < deadline,
            "restored archive did not back up: {status}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // The process may stop after its push but before removing the restore marker.
    let prior = Command::new("git")
        .arg("--git-dir")
        .arg(&remote)
        .args(["rev-parse", "refs/heads/main^2"])
        .output()
        .unwrap();
    assert!(prior.status.success());
    let pending = data2.join("github-adopt.pending");
    std::fs::write(&pending, json!({"repository":"fixture/private","head":String::from_utf8(prior.stdout).unwrap().trim()}).to_string()).unwrap();
    request(&address2, "POST", "/github/sync", "{}").unwrap();
    let deadline = Instant::now() + Duration::from_secs(45);
    while pending.exists() {
        assert!(
            Instant::now() < deadline,
            "completed push marker was not cleared"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // Another device advances the remote after a restore was recorded.
    let old = Command::new("git")
        .arg("--git-dir")
        .arg(&remote)
        .args(["rev-parse", "refs/heads/main"])
        .output()
        .unwrap();
    assert!(old.status.success());
    let old_head = String::from_utf8(old.stdout).unwrap().trim().to_owned();
    let other = temp.path().join("other-device");
    assert!(Command::new("git")
        .arg("clone")
        .arg(&remote)
        .arg(&other)
        .output()
        .unwrap()
        .status
        .success());
    std::fs::write(other.join("other-device.txt"), "remote change").unwrap();
    assert!(Command::new("git")
        .current_dir(&other)
        .args(["add", "other-device.txt"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .current_dir(&other)
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@invalid",
            "commit",
            "-m",
            "fixture change"
        ])
        .output()
        .unwrap()
        .status
        .success());
    assert!(Command::new("git")
        .current_dir(&other)
        .args(["push", "origin", "HEAD:main"])
        .output()
        .unwrap()
        .status
        .success());
    std::fs::write(
        data2.join("github-adopt.pending"),
        json!({"repository":"fixture/private","head":old_head}).to_string(),
    )
    .unwrap();
    request(&address2, "POST", "/github/sync", "{}").unwrap();
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let status = request(&address2, "GET", "/activity", "").unwrap();
        if status["backupReason"] == "restore_changed" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "changed remote was not rejected: {status}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(other.join("other-device.txt").is_file());
    request(&address2, "POST", "/maintenance", "{}").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while request(&address2, "GET", "/activity", "").unwrap()["activeBackups"] != 0 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
    request(&address2, "POST", "/github/restore", "{}").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while request(&address2, "POST", "/maintenance/release", "{}").is_none() {
        assert!(
            Instant::now() < deadline,
            "backup did not release maintenance"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let status = request(&address2, "GET", "/activity", "").unwrap();
        if status["githubBackupReady"] == true {
            break;
        }
        assert!(
            status["backupStatus"] != "failed" || status["backupReason"] == "restore_changed",
            "retry after restore failed: {status}"
        );
        assert!(
            Instant::now() < deadline,
            "retry after restore did not finish: {status}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        std::fs::read_to_string(data2.join("archive/other-device.txt")).unwrap(),
        "remote change"
    );
    let deleted = request(&address2, "DELETE", &format!("/chats/{chat}"), "").unwrap();
    assert!(deleted["deletedAt"].as_str().is_some());
    assert!(request(&address2, "GET", &format!("/chats/{chat}"), "").is_none());
    request(&address2, "POST", "/github/sync", "{}").unwrap();
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let status = request(&address2, "GET", "/activity", "").unwrap();
        if status["githubBackupReady"] == true {
            break;
        }
        assert_ne!(
            status["backupStatus"], "failed",
            "deletion backup failed: {status}"
        );
        assert!(
            Instant::now() < deadline,
            "deletion backup timed out: {status}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let marker = Command::new("git")
        .arg("--git-dir")
        .arg(&remote)
        .args(["show", &format!("main:chats/chat_{chat}.json")])
        .output()
        .unwrap();
    assert!(marker.status.success());
    let marker: Value = serde_json::from_slice(&marker.stdout).unwrap();
    assert_eq!(marker["deleted"], true);
    assert_eq!(marker["room"]["id"], format!("chat_{chat}"));
    let data3 = temp.path().join("after-deletion");
    let vault3 = momso_saved_login_core::linux::LinuxVault::from_credential_file(
        &data3.join("saved-logins"),
        &key,
    )
    .unwrap();
    vault3
        .store_service_secret("github-access-token", "fixture-secret-token")
        .unwrap();
    let socket3 = TcpListener::bind("127.0.0.1:0").unwrap();
    let address3 = socket3.local_addr().unwrap().to_string();
    drop(socket3);
    let _third = Server(
        Command::new(env!("CARGO_BIN_EXE_momso-server"))
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_DATA_DIR", &data3)
            .env("MOMSO_SERVER_LISTEN", &address3)
            .env("MOMSO_CODEX_PATH", &cli)
            .env("MOMSO_GITHUB_CLIENT_ID", "fixture-client-id")
            .env("MOMSO_TEST_GITHUB_API", format!("http://{fake_address}/"))
            .env("MOMSO_TEST_GITHUB_GIT", &remote)
            .env("MOMSO_TEST_DISABLE_AUTO_BACKUP", "1")
            .env("MOMSO_BACKUP_AGENT", &fake_backup)
            .env("MOMSO_TEST_ALLOW_BACKUP_AGENT", "1")
            .env("CREDENTIALS_DIRECTORY", &credentials)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address3);
    request(
        &address3,
        "POST",
        "/github/repository",
        r#"{"nameWithOwner":"fixture/private"}"#,
    )
    .unwrap();
    request(&address3, "POST", "/maintenance", "{}").unwrap();
    let restored = request(&address3, "POST", "/github/restore", "{}").unwrap();
    assert_eq!(restored["restoredDeletions"], 1);
    assert_eq!(restored["restoredChats"], 0);
    assert!(request(&address3, "GET", &format!("/chats/{chat}"), "").is_none());
    let db = rusqlite::Connection::open(data3.join("server.sqlite3")).unwrap();
    let marked: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_deletions WHERE chat_id=?1)",
            [&chat],
            |r| r.get(0),
        )
        .unwrap();
    assert!(marked);
    drop(db);
    // Exercise an old GitHub chat copy against a newer encrypted deletion ledger.
    drop(_third);
    drop(_restarted);
    drop(_recovered_server);
    let old_remote = temp.path().join("old-remote.git");
    assert!(Command::new("git")
        .arg("clone")
        .arg("--bare")
        .arg(&checkout)
        .arg(&old_remote)
        .output()
        .unwrap()
        .status
        .success());
    use sha2::{Digest, Sha256};
    let deleted_hash = format!("{:x}", Sha256::digest(b"hello world"));
    assert!(Command::new("python3")
        .arg(&fake_backup)
        .arg("fixture-mark-purged")
        .arg(&deleted_hash)
        .env("MOMSO_SERVER_DATA_DIR", &data)
        .output()
        .unwrap()
        .status
        .success());
    let data4 = temp.path().join("old-chat-new-vm");
    let vault4 = momso_saved_login_core::linux::LinuxVault::from_credential_file(
        &data4.join("saved-logins"),
        &key,
    )
    .unwrap();
    vault4
        .store_service_secret("github-access-token", "fixture-secret-token")
        .unwrap();
    let socket4 = TcpListener::bind("127.0.0.1:0").unwrap();
    let address4 = socket4.local_addr().unwrap().to_string();
    drop(socket4);
    let _fourth = Server(
        Command::new(env!("CARGO_BIN_EXE_momso-server"))
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_DATA_DIR", &data4)
            .env("MOMSO_SERVER_LISTEN", &address4)
            .env("MOMSO_CODEX_PATH", &cli)
            .env("MOMSO_GITHUB_CLIENT_ID", "fixture-client-id")
            .env("MOMSO_TEST_GITHUB_API", format!("http://{fake_address}/"))
            .env("MOMSO_TEST_GITHUB_GIT", &old_remote)
            .env("MOMSO_TEST_DISABLE_AUTO_BACKUP", "1")
            .env("MOMSO_BACKUP_AGENT", &fake_backup)
            .env("MOMSO_TEST_ALLOW_BACKUP_AGENT", "1")
            .env("CREDENTIALS_DIRECTORY", &credentials)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address4);
    request(
        &address4,
        "POST",
        "/github/repository",
        r#"{"nameWithOwner":"fixture/private"}"#,
    )
    .unwrap();
    request(&address4, "POST", "/maintenance", "{}").unwrap();
    let old_restored = request(&address4, "POST", "/github/restore", "{}").unwrap();
    assert_eq!(old_restored["restoredChats"], 1);
    request(&address4, "POST", "/maintenance/release", "{}").unwrap();
    let old_chat = request(&address4, "GET", &format!("/chats/{chat}"), "").unwrap();
    assert_eq!(
        old_chat["messages"][0]["attachments"][0]["name"],
        "수업.txt"
    );
    assert_eq!(old_chat["messages"][0]["attachments"][0]["deleted"], true);
    assert!(raw_post(&address4, &format!("/assets/{asset}/restore")).contains("asset_purged"));
    assert!(!data4
        .join("archive/objects/sha256")
        .join(&deleted_hash[..2])
        .join(&deleted_hash)
        .exists());
    assert_eq!(
        request(&address4, "GET", &format!("/documents/{document}"), "").unwrap()["body"],
        "복원할 글"
    );
    stopped.store(true, Ordering::SeqCst);
    worker.join().unwrap();
}
fn raw_post(address: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(stream,"POST {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-only-token\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    raw
}
fn wait_state(address: &str, turn: &str, wanted: &str) -> Value {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let state = request(address, "GET", &format!("/turns/{turn}"), "").unwrap();
        if state["state"] == wanted {
            return state;
        }
        assert_eq!(state["state"], "running");
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(30));
    }
}
#[test]
fn fake_cli_http_chat_is_readable_after_server_process_restart() {
    let temp = tempfile::tempdir().unwrap();
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = temp.path().join("data");
    let server = launch(&data, &cli, &address);
    ready(&address);
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let route = format!("/chats/{chat}/turns");
    let turn = request(
        &address,
        "POST",
        &route,
        &json!({"text":"가짜 말"}).to_string(),
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    let stream_address = address.clone();
    let stream_turn = turn.clone();
    let events = std::thread::spawn(move || read_events(&stream_address, &stream_turn));
    let end = Instant::now() + Duration::from_secs(10);
    let mut saw_partial = false;
    loop {
        let state = request(&address, "GET", &format!("/turns/{turn}"), "").unwrap();
        if state["state"] == "completed" {
            assert!(saw_partial, "no running partial response was observed");
            assert_eq!(state["text"], "가짜 답");
            break;
        }
        assert_eq!(state["state"], "running");
        saw_partial |= state["text"] == "가짜 ";
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(30));
    }
    let events = events.join().unwrap();
    assert!(events.contains("event: chat-delta"));
    assert!(events.contains("event: chat-done"));
    drop(server);
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let restart_address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let _restarted = launch(&data, &cli, &restart_address);
    ready(&restart_address);
    let chat = request(&restart_address, "GET", &format!("/chats/{chat}"), "").unwrap();
    assert_eq!(chat["messages"][0]["text"], "가짜 말");
    assert_eq!(chat["messages"][1]["text"], "가짜 답");
}

#[test]
fn screen_assets_have_expected_content_types_without_token() {
    let temp = tempfile::tempdir().unwrap();
    let web = temp.path().join("web");
    std::fs::create_dir_all(web.join("app-icons")).unwrap();
    let files = [
        ("index.html", "text/html; charset=utf-8"),
        ("manifest.webmanifest", "application/manifest+json"),
        ("app-icons/icon-192.png", "image/png"),
        ("app-icons/icon-512.png", "image/png"),
        ("image.svg", "image/svg+xml"),
        ("favicon.ico", "image/x-icon"),
        ("font.woff2", "font/woff2"),
        ("data.json", "application/json"),
        ("style.css", "text/css; charset=utf-8"),
        ("app.js", "text/javascript; charset=utf-8"),
    ];
    for (path, _) in files {
        std::fs::write(web.join(path), "fixture").unwrap();
    }
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let _server = launch_with(&temp.path().join("data"), &cli, &address, Some(&web), false);
    ready(&address);
    for (path, kind) in files {
        let response = raw_get(&address, &format!("/{path}"));
        let headers = response
            .split_once("\r\n\r\n")
            .unwrap()
            .0
            .to_ascii_lowercase();
        assert!(headers.contains(" 200 "), "{path}: {headers}");
        assert!(
            headers
                .lines()
                .any(|line| line == format!("content-type: {kind}")),
            "{path}: {headers}"
        );
        assert!(response.ends_with("fixture"), "{path}: missing body");
    }
    assert!(raw_get(&address, "/health").contains(" 401 "));
}

#[test]
fn parity_dictionary_skill_folder_and_chat_routes_require_auth_and_persist() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let server = launch(&data, &cli, &address);
    ready(&address);
    for path in [
        "/dictionary",
        "/skills",
        "/document-folders",
        "/chats/search?q=test",
    ] {
        assert!(raw_get(&address, path).contains(" 401 "), "{path}");
    }
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    request(
        &address,
        "PATCH",
        &format!("/chats/{chat}"),
        r#"{"title":"test-title"}"#,
    )
    .unwrap();
    assert_eq!(
        request(&address, "GET", "/chats/search?q=test-title", "").unwrap()["chats"][0]["id"],
        chat
    );
    request(
        &address,
        "POST",
        &format!("/chats/{chat}/pin"),
        r#"{"pinned":true}"#,
    )
    .unwrap();
    assert_eq!(
        request(&address, "GET", "/chats", "").unwrap()["chats"][0]["pinned"],
        true
    );
    request(
        &address,
        "POST",
        &format!("/chats/{chat}/archive"),
        r#"{"archived":true}"#,
    )
    .unwrap();
    assert!(request(&address, "GET", "/chats", "").unwrap()["chats"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        request(&address, "GET", "/chats/archived", "").unwrap()["chats"][0]["id"],
        chat
    );
    request(
        &address,
        "POST",
        "/document-folders",
        r#"{"folder":"lesson"}"#,
    )
    .unwrap();
    request(
        &address,
        "POST",
        "/skills",
        r#"{"name":"test-skill","title":"Test","description":"Test skill","body":"example"}"#,
    )
    .unwrap();
    let dict = request(&address, "GET", "/dictionary", "").unwrap();
    let body =
        json!({"id":null,"addVariants":["test term"],"expectedRevision":dict["userRevision"]})
            .to_string();
    let term = request(&address, "POST", "/dictionary/terms", &body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    drop(server);
    let _server = launch(&data, &cli, &address);
    ready(&address);
    assert_eq!(
        request(&address, "GET", "/chats/archived", "").unwrap()["chats"][0]["id"],
        chat
    );
    request(
        &address,
        "POST",
        &format!("/chats/{chat}/archive"),
        r#"{"archived":false}"#,
    )
    .unwrap();
    assert_eq!(
        request(&address, "GET", "/chats", "").unwrap()["chats"][0]["pinned"],
        true
    );
    assert_eq!(
        request(&address, "GET", &format!("/chats/{chat}"), "").unwrap()["title"],
        "test-title"
    );
    assert!(
        request(&address, "GET", "/document-folders", "").unwrap()["folders"]
            .as_array()
            .unwrap()
            .contains(&json!("lesson"))
    );
    assert_eq!(
        request(&address, "GET", "/skills/test-skill", "").unwrap()["body"],
        "example"
    );
    assert!(
        request(&address, "GET", "/dictionary", "").unwrap()["terms"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == term)
    );
}

#[test]
fn chat_groups_are_transactional_and_unassign_on_delete() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let _server = launch(&data, &cli, &address);
    ready(&address);
    assert!(raw_get(&address, "/chat-groups").contains(" 401 "));
    let a = request(&address, "POST", "/chat-groups", r#"{"name":"one"}"#).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let b = request(&address, "POST", "/chat-groups", r#"{"name":"two"}"#).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(request(
        &address,
        "POST",
        "/chat-groups/reorder",
        &json!({"ids":[a]}).to_string()
    )
    .is_none());
    request(
        &address,
        "POST",
        "/chat-groups/reorder",
        &json!({"ids":[b,a]}).to_string(),
    )
    .unwrap();
    let groups = request(&address, "GET", "/chat-groups", "").unwrap();
    assert_eq!(groups["groups"][0]["id"], b);
    request(
        &address,
        "PATCH",
        &format!("/chat-groups/{a}"),
        r#"{"name":"renamed"}"#,
    )
    .unwrap();
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    request(
        &address,
        "PUT",
        &format!("/chats/{chat}/group"),
        &json!({"groupId":a}).to_string(),
    )
    .unwrap();
    assert_eq!(
        request(&address, "GET", &format!("/chats/{chat}"), "").unwrap()["groupId"],
        a
    );
    request(&address, "DELETE", &format!("/chat-groups/{a}"), "").unwrap();
    assert_eq!(
        request(&address, "GET", &format!("/chats/{chat}"), "").unwrap()["groupId"],
        Value::Null
    );
    assert!(request(
        &address,
        "PUT",
        &format!("/chats/{chat}/group"),
        &json!({"groupId":a}).to_string()
    )
    .is_none());
}

#[test]
fn selected_skill_reaches_fake_codex_and_survives_chat_read() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let _server = launch(&data, &cli, &address);
    ready(&address);
    request(&address,"POST","/skills",r#"{"name":"test-skill","title":"Test","description":"Test","body":"s75-skill-fixture-marker"}"#).unwrap();
    let room = request(&address, "POST", "/chats", "{}").unwrap();
    let chat = room["chatId"].as_str().unwrap();
    let sent = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        r#"{"text":"hello","skill":"test-skill"}"#,
    )
    .unwrap();
    assert_eq!(
        wait_state(&address, sent["turnId"].as_str().unwrap(), "completed")["text"],
        "skill fixture accepted"
    );
    let read = request(&address, "GET", &format!("/chats/{chat}"), "").unwrap();
    assert_eq!(read["messages"][0]["text"], "hello");
    assert_eq!(read["messages"][0]["skill"], "test-skill");
    request(
        &address,
        "POST",
        "/skills/test-skill/enabled",
        r#"{"enabled":false}"#,
    )
    .unwrap();
    assert!(request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        r#"{"text":"next","skill":"test-skill"}"#
    )
    .is_none());
}

#[test]
fn folder_rename_updates_documents_and_preserves_unrelated_folders() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let _server = launch(&data, &cli, &address);
    ready(&address);
    request(
        &address,
        "POST",
        "/document-folders",
        r#"{"folder":"other"}"#,
    )
    .unwrap();
    let document = request(
        &address,
        "POST",
        "/documents",
        r#"{"title":"class","summary":"summary","body":"text","folder":"classes/a"}"#,
    )
    .unwrap();
    let id = document["id"].as_str().unwrap();
    assert!(request(
        &address,
        "POST",
        "/document-folders/rename",
        r#"{"from":"classes","to":"classes/nested"}"#
    )
    .is_none());
    request(
        &address,
        "POST",
        "/document-folders/rename",
        r#"{"from":"classes","to":"lessons"}"#,
    )
    .unwrap();
    let doc = request(&address, "GET", &format!("/documents/{id}"), "").unwrap();
    assert_eq!(doc["folder"], "lessons/a");
    assert_eq!(doc["body"], "text");
    let folders = request(&address, "GET", "/document-folders", "").unwrap();
    assert!(folders["folders"]
        .as_array()
        .unwrap()
        .contains(&json!("other")));
}

#[test]
fn document_history_restores_without_overwriting_concurrent_edits() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let _server = launch(&data, &cli, &address);
    ready(&address);
    let created = request(
        &address,
        "POST",
        "/documents",
        r#"{"title":"class","summary":"summary","body":"first"}"#,
    )
    .unwrap();
    let id = created["id"].as_str().unwrap();
    let archive = data.join("archive");
    let path = format!("zettels/{id}.md");
    let commit = |message: &str| {
        assert!(Command::new("git")
            .current_dir(&archive)
            .args(["add", "--", &path])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .current_dir(&archive)
            .args([
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--quiet",
                "--only",
                "-m",
                message,
                "--",
                &path
            ])
            .status()
            .unwrap()
            .success());
    };
    commit("first");
    let update =
        json!({"title":"class","summary":"summary","body":"second","revision":created["revision"]})
            .to_string();
    let changed = request(&address, "PUT", &format!("/documents/{id}"), &update).unwrap();
    commit("second");
    assert!(raw_get(&address, &format!("/documents/{id}/history")).contains(" 401 "));
    let versions = request(&address, "GET", &format!("/documents/{id}/history"), "").unwrap();
    assert_eq!(versions["versions"].as_array().unwrap().len(), 2);
    let old = versions["versions"][1]["revision"].as_str().unwrap();
    assert_eq!(
        request(
            &address,
            "GET",
            &format!("/documents/{id}/versions/{old}"),
            ""
        )
        .unwrap()["body"],
        "first"
    );
    let stale = json!({"revision":old,"expectedRevision":"wrong"}).to_string();
    assert!(request(
        &address,
        "POST",
        &format!("/documents/{id}/restore"),
        &stale
    )
    .is_none());
    assert_eq!(
        request(&address, "GET", &format!("/documents/{id}"), "").unwrap()["body"],
        "second"
    );
    let unsaved =
        json!({"title":"class","summary":"summary","body":"third","revision":changed["revision"]})
            .to_string();
    let unsaved = request(&address, "PUT", &format!("/documents/{id}"), &unsaved).unwrap();
    let restore = json!({"revision":old,"expectedRevision":unsaved["revision"]}).to_string();
    assert!(request(
        &address,
        "POST",
        &format!("/documents/{id}/restore"),
        &restore
    )
    .is_none());
    assert_eq!(
        request(&address, "GET", &format!("/documents/{id}"), "").unwrap()["body"],
        "third"
    );
    commit("third");
    request(
        &address,
        "POST",
        &format!("/documents/{id}/restore"),
        &restore,
    )
    .unwrap();
    assert_eq!(
        request(&address, "GET", &format!("/documents/{id}"), "").unwrap()["body"],
        "first"
    );
}

#[test]
fn serves_built_screen_and_lists_chats_with_timestamps() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let web = temp.path().join("web");
    std::fs::create_dir_all(web.join("assets")).unwrap();
    std::fs::write(web.join("index.html"), "<h1>fake screen</h1>").unwrap();
    std::fs::write(web.join("assets/app.js"), "console.log('test')").unwrap();
    std::fs::write(temp.path().join("outside.txt"), "private-test").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(temp.path().join("outside.txt"), web.join("escape.txt")).unwrap();
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let _server = launch_with(&data, &cli, &address, Some(&web), false);
    ready(&address);
    let home = raw_get(&address, "/");
    assert!(home.contains(" 200 ") && home.contains("fake screen"));
    assert!(!home
        .to_ascii_lowercase()
        .contains("access-control-allow-origin"));
    let js = raw_get(&address, "/assets/app.js");
    assert!(js.contains("text/javascript") && js.contains("console.log"));
    assert!(raw_get(&address, "/health").contains(" 401 "));
    let traversal = raw_get(&address, "/%2e%2e/outside.txt");
    assert!(!traversal.contains("private-test"));
    #[cfg(unix)]
    assert!(!raw_get(&address, "/escape.txt").contains("private-test"));
    let chat = request(&address, "POST", "/chats", r#"{"title":"첫 방"}"#).unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    std::thread::sleep(Duration::from_millis(20));
    let newer = request(&address, "POST", "/chats", r#"{"title":"새 방"}"#).unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let list = request(&address, "GET", "/chats", "").unwrap();
    assert_eq!(list["chats"][0]["id"], newer);
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        r#"{"text":"말"}"#,
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_state(&address, &turn, "completed");
    let list = request(&address, "GET", "/chats", "").unwrap();
    assert_eq!(list["chats"][0]["id"], chat);
    assert!(list["chats"][0]["updatedAt"].as_str().is_some());
    let read = request(&address, "GET", &format!("/chats/{chat}"), "").unwrap();
    assert_eq!(read["title"], "첫 방");
    assert!(read["messages"][0]["createdAt"].as_str().is_some());
    let after = read_events_after(&address, &turn, Some("가짜 ".len()));
    assert!(after.contains("답"));
    assert!(!after.contains("가짜"));
    assert!(read_events_after(&address, &turn, Some(1)).contains("invalid_offset"));
}

#[test]
fn cancel_then_retry_keeps_one_user_message_and_replays_by_offset() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let server = launch(&data, &cli, &address);
    ready(&address);
    assert_eq!(
        request(&address, "GET", "/activity", "").unwrap()["busy"],
        false
    );
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let sent = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        r#"{"text":"수업"}"#,
    )
    .unwrap();
    let turn = sent["turnId"].as_str().unwrap();
    let message = sent["userMessageId"].as_str().unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let state = request(&address, "GET", &format!("/turns/{turn}"), "").unwrap();
        if state["text"] == "가짜 " {
            break;
        }
        assert_eq!(state["state"], "running");
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(10));
    }
    let room = request(&address, "GET", &format!("/chats/{chat}"), "").unwrap();
    assert_eq!(room["activeTurn"]["turnId"], turn);
    assert!(room["activeTurn"]["offset"].as_i64().unwrap() > 0);
    let work = request(&address, "GET", "/activity", "").unwrap();
    assert_eq!(work["busy"], true);
    assert_eq!(work["activeTurns"], 1);
    let event_address = address.clone();
    let event_turn = turn.to_owned();
    let replay =
        std::thread::spawn(move || read_events_after(&event_address, &event_turn, Some(0)));
    std::thread::sleep(Duration::from_millis(150));
    request(&address, "POST", &format!("/turns/{turn}/cancel"), "").unwrap();
    let cancelled = wait_state(&address, turn, "failed");
    assert_eq!(cancelled["error"], "cancelled");
    assert!(replay.join().unwrap().contains("가짜 "));
    let deadline = Instant::now() + Duration::from_secs(3);
    while request(&address, "GET", "/activity", "").unwrap()["busy"] != false {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(server);
    let _server = launch(&data, &cli, &address);
    ready(&address);
    let restored = request(&address, "GET", &format!("/chats/{chat}"), "").unwrap();
    assert_eq!(restored["messages"][0]["turnId"], turn);
    assert_eq!(restored["messages"][0]["turnState"], "failed");
    assert_eq!(restored["messages"][0]["turnError"], "cancelled");
    assert_eq!(restored["messages"][1]["text"], "가짜 ");
    assert_eq!(restored["messages"][1]["finishReason"], "cancelled");
    let retry = request(&address, "POST", &format!("/turns/{turn}/retry"), "").unwrap();
    let again = request(&address, "POST", &format!("/turns/{turn}/retry"), "").unwrap();
    assert_eq!(retry, again);
    assert_eq!(retry["userMessageId"], message);
    wait_state(&address, retry["turnId"].as_str().unwrap(), "completed");
    let read = request(&address, "GET", &format!("/chats/{chat}"), "").unwrap();
    let messages = read["messages"].as_array().unwrap();
    assert_eq!(messages.iter().filter(|m| m["role"] == "user").count(), 1);
    assert_eq!(messages.last().unwrap()["text"], "가짜 답");
    assert_eq!(messages[0]["turnId"], retry["turnId"]);
    assert_eq!(messages[0]["turnState"], "completed");
    assert!(messages[0]["turnError"].is_null());
}

#[test]
fn failed_provider_retry_reuses_user_message() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let server = launch_with(&data, &cli, &address, None, true);
    ready(&address);
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let first = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        r#"{"text":"다시"}"#,
    )
    .unwrap();
    let turn = first["turnId"].as_str().unwrap();
    let failed = wait_state(&address, turn, "failed");
    assert_eq!(failed["error"], "provider_failed");
    drop(server);
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let next = socket.local_addr().unwrap().to_string();
    drop(socket);
    let _restarted = launch_with(&data, &cli, &next, None, true);
    ready(&next);
    let restored = request(&next, "GET", &format!("/chats/{chat}"), "").unwrap();
    assert_eq!(restored["messages"][0]["turnId"], turn);
    assert_eq!(restored["messages"][0]["turnState"], "failed");
    assert_eq!(restored["messages"][0]["turnError"], "provider_failed");
    let retry = request(&next, "POST", &format!("/turns/{turn}/retry"), "").unwrap();
    assert_eq!(retry["userMessageId"], first["userMessageId"]);
    wait_state(&next, retry["turnId"].as_str().unwrap(), "completed");
    let read = request(&next, "GET", &format!("/chats/{chat}"), "").unwrap();
    assert_eq!(read["messages"].as_array().unwrap().len(), 2);
}

#[test]
fn previous_database_migrates_without_losing_chat() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let db = rusqlite::Connection::open(data.join("server.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE chats(id TEXT PRIMARY KEY,title TEXT NOT NULL,thread_id TEXT); CREATE TABLE chat_messages(id TEXT PRIMARY KEY,chat_id TEXT NOT NULL,role TEXT NOT NULL,text TEXT NOT NULL,created_at TEXT NOT NULL,provider TEXT,model TEXT,effort TEXT,skill TEXT,actual_effort TEXT,finish_reason TEXT); CREATE TABLE turns(id TEXT PRIMARY KEY,chat_id TEXT NOT NULL,state TEXT NOT NULL,text TEXT NOT NULL DEFAULT '',error TEXT); INSERT INTO chats VALUES('old-chat','이전 방',NULL); INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('old-msg','old-chat','user','이전 말','2026-01-01T00:00:00Z'); INSERT INTO turns VALUES('old-turn','old-chat','failed','','provider_failed');").unwrap();
    drop(db);
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let server = launch(&data, &cli, &address);
    ready(&address);
    let chat = request(&address, "GET", "/chats/old-chat", "").unwrap();
    assert_eq!(chat["title"], "이전 방");
    assert_eq!(chat["messages"][0]["text"], "이전 말");
    assert_eq!(chat["createdAt"], "2026-01-01T00:00:00Z");
    let retry = raw_post(&address, "/turns/old-turn/retry");
    assert!(retry.contains(" 409 ") && retry.contains("retry_unavailable"));
    drop(server);
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let next = socket.local_addr().unwrap().to_string();
    drop(socket);
    let restarted = launch(&data, &cli, &next);
    ready(&next);
    assert_eq!(
        request(&next, "GET", "/chats/old-chat", "").unwrap()["messages"][0]["text"],
        "이전 말"
    );
    drop(restarted);
}

#[test]
fn approval_survives_reconnect_and_returns_to_fake_cli() {
    let temp = tempfile::tempdir().unwrap();
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let _server = launch(&temp.path().join("data"), &cli, &address);
    ready(&address);
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    for (choice, expected) in [("allow_once", "승인됨"), ("deny", "거절됨")] {
        let turn = request(
            &address,
            "POST",
            &format!("/chats/{chat}/turns"),
            &json!({"text":"approval fixture"}).to_string(),
        )
        .unwrap()["turnId"]
            .as_str()
            .unwrap()
            .to_owned();
        let event_address = address.clone();
        let event_turn = turn.clone();
        let events = std::thread::spawn(move || read_events(&event_address, &event_turn));
        let deadline = Instant::now() + Duration::from_secs(10);
        let approval = loop {
            let result = request(&address, "GET", &format!("/turns/{turn}/approval"), "").unwrap();
            if !result["approval"].is_null() {
                break result["approval"].clone();
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(30));
        };
        let id = approval["requestId"].as_str().unwrap();
        assert_eq!(approval["kind"], "command");
        assert_eq!(
            request(&address, "GET", &format!("/turns/{turn}/approval"), "").unwrap()["approval"]
                ["requestId"],
            id
        );
        std::thread::sleep(Duration::from_millis(250));
        request(
            &address,
            "POST",
            &format!("/approvals/{id}/respond"),
            &json!({"choice":choice}).to_string(),
        )
        .unwrap();
        assert_eq!(wait_state(&address, &turn, "completed")["text"], expected);
        let events = events.join().unwrap();
        assert!(events.contains("event: agent-approval-request"));
        assert!(events.contains("event: agent-approval-closed"));
        assert!(events.contains("responded"));
    }
}

fn login_request(address: &str, method: &str, path: &str, session: Option<&str>) -> Option<Value> {
    let mut stream = TcpStream::connect(address).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    write!(stream,"{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-only-token\r\nX-Momso-Login-Session: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",session.unwrap_or("")).ok()?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).ok()?;
    let (head, body) = raw.split_once("\r\n\r\n")?;
    if !head.contains(" 200 ")
        || !head
            .to_ascii_lowercase()
            .contains("cache-control: no-store")
    {
        return None;
    }
    serde_json::from_str(body).ok()
}
#[test]
fn device_login_then_chat_with_fake_cli() {
    let temp = tempfile::tempdir().unwrap();
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = temp.path().join("data");
    let _server = launch(&data, &cli, &address);
    ready(&address);
    let providers = login_request(&address, "GET", "/providers", None).unwrap();
    assert_eq!(providers["providers"][0]["state"], "not_connected");
    let login = login_request(&address, "POST", "/providers/codex/login", None).unwrap();
    assert_eq!(login.as_object().unwrap().len(), 1);
    let session = login["sessionId"].as_str().unwrap();
    let event_address = address.clone();
    let event_session = session.to_owned();
    let event_thread = std::thread::spawn(move || {
        let mut stream = TcpStream::connect(event_address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        write!(stream,"GET /providers/codex/login/events HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-only-token\r\nX-Momso-Login-Session: {event_session}\r\nConnection: close\r\n\r\n").unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        text
    });
    assert!(login_request(
        &address,
        "GET",
        "/providers/codex/login",
        Some("other-session")
    )
    .is_none());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let state =
            login_request(&address, "GET", "/providers/codex/login", Some(session)).unwrap();
        if state["state"] == "waiting" {
            assert_eq!(state["url"], "https://auth.openai.com/codex/device");
            assert_eq!(state["code"], "TEST-1234");
            assert!(state["expiresInSeconds"].as_i64().unwrap() > 0);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(30));
    }
    loop {
        let state =
            login_request(&address, "GET", "/providers/codex/login", Some(session)).unwrap();
        if state["state"] == "connected" {
            assert!(state["code"].is_null());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(30));
    }
    let events = event_thread.join().unwrap();
    assert!(events.contains("event: provider-login-device\n"));
    assert!(events.contains("event: provider-login-device-closed\n"));
    assert_eq!(
        login_request(&address, "GET", "/providers", None).unwrap()["providers"][0]["state"],
        "connected"
    );
    assert_eq!(
        login_request(&address, "GET", "/providers", None).unwrap()["providers"][0]["accountLabel"],
        "teacher@example.test"
    );
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":"test"}).to_string(),
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(wait_state(&address, &turn, "completed")["text"], "가짜 답");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(data.join("codex"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[test]
fn device_login_cancel_allows_new_start() {
    let temp = tempfile::tempdir().unwrap();
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let _server = launch(&temp.path().join("data"), &cli, &address);
    ready(&address);
    let first = login_request(&address, "POST", "/providers/codex/login", None).unwrap();
    let id = first["sessionId"].as_str().unwrap();
    assert!(login_request(
        &address,
        "POST",
        "/providers/codex/login/cancel",
        Some("other")
    )
    .is_none());
    login_request(&address, "POST", "/providers/codex/login/cancel", Some(id)).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let state = login_request(&address, "GET", "/providers/codex/login", Some(id)).unwrap();
        if state["state"] == "cancelled" {
            assert!(state["code"].is_null());
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(30));
    }
    let second = login_request(&address, "POST", "/providers/codex/login", None).unwrap();
    assert_ne!(second["sessionId"], id);
    assert!(login_request(&address, "GET", "/providers/codex/login", Some(id)).is_none());
}

#[test]
fn structured_unauthorized_is_auth_failed() {
    let temp = tempfile::tempdir().unwrap();
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let _server = launch(&temp.path().join("data"), &cli, &address);
    ready(&address);
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":"unauthorized fixture"}).to_string(),
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        wait_state(&address, &turn, "failed")["error"],
        "auth_failed"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn stored_login_errors_never_echo_secret() {
    let temp = tempfile::tempdir().unwrap();
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let _server = launch(&temp.path().join("data"), &cli, &address);
    ready(&address);
    let secret = "fixture-password-never-return";
    for body in [json!({"siteUrl":"https://example.test/login","label":"test","username":"teacher","password":secret}).to_string(), format!("{{\"password\":\"{secret}\"}}") ] {
        let mut stream=TcpStream::connect(&address).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        write!(stream,"POST /saved-logins HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-only-token\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        let mut raw=String::new();stream.read_to_string(&mut raw).unwrap();
        assert!(!raw.contains(secret));
        assert!(raw.contains("credential_unavailable") || raw.contains("invalid_input"));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn existing_work_records_are_available_without_following_paths() {
    let temp = tempfile::tempdir().unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = temp.path().join("data");
    let _server = launch(&data, Path::new(env!("CARGO_BIN_EXE_fake-codex")), &address);
    ready(&address);
    let id = "rec_20261122_120000";
    std::fs::write(data.join("archive/receipts").join(format!("{id}.md")),"---\nid: rec_20261122_120000\ncreated_at: '2026-11-22T12:00:00Z'\ntool: codex\nmodel: fixture\nread: []\nwrote: []\n---\n## 시킨 말\n가짜\n\n## 고친 내용\n없음\n").unwrap();
    assert!(raw_get(&address, "/records").contains(" 401 "));
    assert_eq!(
        request(&address, "GET", "/records", "").unwrap()["items"][0]["id"],
        id
    );
    assert!(
        request(&address, "GET", &format!("/records/{id}"), "").unwrap()["body"]
            .as_str()
            .unwrap()
            .contains("가짜")
    );
    assert!(request(&address, "GET", "/records/invalid", "").is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn generated_media_reports_scans_versions_and_recovers_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = temp.path().join("data");
    let cli = Path::new(env!("CARGO_BIN_EXE_fake-codex"));
    let server = launch(&data, cli, &address);
    ready(&address);
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":"generated tool fixture"}).to_string(),
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        wait_state(&address, &turn, "completed")["text"],
        "generated registered"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let first = loop {
        let list = request(&address, "GET", "/media", "").unwrap();
        if list["items"].as_array().unwrap().len() == 1 && list["pendingGenerations"] == 0 {
            break list["items"][0].clone();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(30));
    };
    assert_eq!(first["prompt"], "draw fixture");
    assert_eq!(first["promptSource"], "reported");
    assert_eq!(first["version"], 1);
    let parent = first["assetId"].as_str().unwrap();
    let regenerate = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":"regenerate fixture","regenerateOf":parent}).to_string(),
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        wait_state(&address, &regenerate, "completed")["text"],
        "generated registered"
    );
    let second = loop {
        let list = request(&address, "GET", "/media", "").unwrap();
        if list["items"].as_array().unwrap().len() == 1
            && list["items"][0]["version"] == 2
            && list["pendingGenerations"] == 0
        {
            break list["items"][0].clone();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(30));
    };
    assert_eq!(second["versionOf"], parent);
    assert_eq!(second["versionCount"], 2);
    assert_eq!(second["lineageId"], first["lineageId"]);
    let versions = request(
        &address,
        "GET",
        &format!(
            "/media/lineages/{}/versions",
            first["lineageId"].as_str().unwrap()
        ),
        "",
    )
    .unwrap();
    assert_eq!(versions["versions"].as_array().unwrap().len(), 2);
    let other = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let scan = request(
        &address,
        "POST",
        &format!("/chats/{other}/turns"),
        &json!({"text":"generated scan fixture"}).to_string(),
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_state(&address, &scan, "completed");
    loop {
        let list = request(&address, "GET", "/media", "").unwrap();
        if list["items"].as_array().unwrap().len() == 2 && list["pendingGenerations"] == 0 {
            assert!(list["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["promptSource"] == "userRequest"));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(30));
    }
    drop(server);
    let output = data.join("archive/.momso/outputs").join(&other).join(&scan);
    std::fs::write(
        output.join("after-interruption.png"),
        b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01recovery",
    )
    .unwrap();
    rusqlite::Connection::open(data.join("server.sqlite3"))
        .unwrap()
        .execute("UPDATE turns SET output_pending='1' WHERE id=?1", [&scan])
        .unwrap();
    let _server = launch(&data, cli, &address);
    ready(&address);
    let list = request(&address, "GET", "/media", "").unwrap();
    assert_eq!(list["pendingGenerations"], 0);
    assert_eq!(list["items"].as_array().unwrap().len(), 3);
    assert!(output.join("after-interruption.png").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn profile_is_saved_for_ai_and_survives_restart() {
    let temp = tempfile::tempdir().unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = temp.path().join("data");
    let cli = Path::new(env!("CARGO_BIN_EXE_fake-codex"));
    let server = launch(&data, cli, &address);
    ready(&address);
    assert!(raw_get(&address, "/profile").contains(" 401 "));
    assert_eq!(
        request(&address, "GET", "/profile", "").unwrap()["name"],
        ""
    );
    assert_eq!(
        request(
            &address,
            "PUT",
            "/profile",
            &json!({"name":"선생님","about":"수업","style":"공손하게"}).to_string()
        )
        .unwrap()["name"],
        "선생님"
    );
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":"profile fixture"}).to_string(),
    )
    .unwrap();
    assert_eq!(
        wait_state(&address, turn["turnId"].as_str().unwrap(), "completed")["text"],
        "profile received"
    );
    drop(server);
    let _server = launch(&data, cli, &address);
    ready(&address);
    assert_eq!(
        request(&address, "GET", "/profile", "").unwrap()["name"],
        "선생님"
    );
    assert!(std::fs::read_to_string(data.join("archive/AGENTS.md"))
        .unwrap()
        .contains("선생님"));
}

#[cfg(target_os = "linux")]
#[test]
fn profile_recovery_conflict_does_not_prevent_server_start() {
    let temp = tempfile::tempdir().unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = temp.path().join("data");
    let cli = Path::new(env!("CARGO_BIN_EXE_fake-codex"));
    let server = launch(&data, cli, &address);
    ready(&address);
    request(
        &address,
        "PUT",
        "/profile",
        &json!({"name":"선생님","about":"","style":""}).to_string(),
    )
    .unwrap();
    drop(server);
    let rules = data.join("archive/AGENTS.md");
    let before = std::fs::read(&rules).unwrap();
    std::fs::write(
        data.join("archive/.momso/profile-write-journal.json"),
        b"invalid fixture",
    )
    .unwrap();
    let _server = launch(&data, cli, &address);
    ready(&address);
    assert!(request(&address, "GET", "/profile", "").is_none());
    assert_eq!(std::fs::read(&rules).unwrap(), before);
}

#[cfg(target_os = "linux")]
#[test]
fn chatgpt_and_gemini_exports_import_without_model_access() {
    let temp = tempfile::tempdir().unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = temp.path().join("data");
    let _server = launch(&data, Path::new(env!("CARGO_BIN_EXE_fake-codex")), &address);
    ready(&address);
    for (provider, export) in [
        (
            "chatgpt",
            json!([{"id":"external-gpt","title":"제목","create_time":1700000000,"current_node":"b","mapping":{"a":{"parent":null,"message":{"author":{"role":"user"},"content":{"parts":["질문"]}}},"b":{"parent":"a","message":{"author":{"role":"assistant"},"content":{"parts":["답"]}}}}}]),
        ),
        (
            "gemini",
            json!([{"header":"Gemini Apps","title":"Prompted question","time":"2026-01-01T10:00:00Z","safeHtmlItem":[{"html":"<p>answer</p>"}]}]),
        ),
    ] {
        let bytes = export.to_string();
        let upload=request(&address,"POST","/uploads",&json!({"fileName":"export.json","mediaType":"application/json","byteLength":bytes.len()}).to_string()).unwrap();
        let id = upload["uploadId"].as_str().unwrap();
        put_bytes(
            &address,
            &format!("/uploads/{id}/chunks?offset=0"),
            bytes.as_bytes(),
        )
        .unwrap();
        let asset = request(&address, "POST", &format!("/uploads/{id}/finish"), "{}").unwrap()
            ["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let body =
            json!({"assetId":asset,"provider":provider,"requestId":format!("fixture-{provider}")})
                .to_string();
        let result = request(&address, "POST", "/chat-imports", &body).unwrap();
        assert_eq!(result["imported"], 1);
        assert_eq!(
            request(&address, "POST", "/chat-imports", &body).unwrap()["duplicate"],
            1
        );
        let progress = request(
            &address,
            "GET",
            &format!("/chat-imports/fixture-{provider}/progress"),
            "",
        )
        .unwrap();
        assert_eq!(progress["processed"], progress["total"]);
        if provider == "gemini" {
            assert!(result["groupingNotice"].as_str().is_some());
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn imported_conversations_skip_old_messages_and_keep_local_messages() {
    let temp = tempfile::tempdir().unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = temp.path().join("data");
    let _server = launch(&data, Path::new(env!("CARGO_BIN_EXE_fake-codex")), &address);
    ready(&address);
    let upload = |contents: &Value| {
        let bytes = contents.to_string();
        let started=request(&address,"POST","/uploads",&json!({"fileName":"export.json","mediaType":"application/json","byteLength":bytes.len()}).to_string()).unwrap();
        let id = started["uploadId"].as_str().unwrap();
        put_bytes(
            &address,
            &format!("/uploads/{id}/chunks?offset=0"),
            bytes.as_bytes(),
        )
        .unwrap();
        request(&address, "POST", &format!("/uploads/{id}/finish"), "{}").unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let first = json!([{"uuid":"example-id","name":"이전 대화","created_at":"2026-01-01T00:00:00Z","chat_messages":[{"uuid":"one","sender":"human","text":"첫 말","created_at":"2026-01-01T00:00:00Z"}]}]);
    let asset = upload(&first);
    let body =
        json!({"assetId":asset,"provider":"claude","requestId":"fixture-import"}).to_string();
    assert!(
        raw_get(&address, "/chat-imports").contains(" 401 ")
            || raw_get(&address, "/chat-imports").contains(" 405 ")
    );
    assert_eq!(
        request(&address, "POST", "/chat-imports/preview", &body).unwrap()["conversations"],
        1
    );
    assert_eq!(
        request(&address, "POST", "/chat-imports", &body).unwrap()["imported"],
        1
    );
    let progress = request(&address, "GET", "/chat-imports/fixture-import/progress", "").unwrap();
    assert_eq!(progress["state"], "completed");
    assert_eq!(progress["processed"], 1);
    assert_eq!(progress["total"], 1);
    assert_eq!(
        request(&address, "POST", "/chat-imports", &body).unwrap()["duplicate"],
        1
    );
    let chats = request(&address, "GET", "/chats", "").unwrap();
    assert_eq!(chats["chats"][0]["importedFrom"], "claude");
    let chat = chats["chats"][0]["id"].as_str().unwrap();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":"handoff fixture"}).to_string(),
    )
    .unwrap();
    assert_eq!(
        wait_state(&address, turn["turnId"].as_str().unwrap(), "completed")["text"],
        "handoff delivered"
    );
    let later = json!([{"uuid":"example-id","name":"이전 대화","created_at":"2026-01-01T00:00:00Z","chat_messages":[{"uuid":"one","sender":"human","text":"첫 말","created_at":"2026-01-01T00:00:00Z"},{"uuid":"two","sender":"assistant","text":"나중 답","created_at":"2026-01-02T00:00:00Z"}]}]);
    let newer = upload(&later);
    let updated = request(
        &address,
        "POST",
        "/chat-imports",
        &json!({"assetId":newer,"provider":"claude"}).to_string(),
    )
    .unwrap();
    assert_eq!(updated["addedMessages"], 1);
    let read = request(&address, "GET", &format!("/chats/{chat}"), "").unwrap();
    assert_eq!(read["importedFrom"], "claude");
    let all = read["messages"].as_array().unwrap();
    assert!(all.iter().any(|m| m["text"] == "handoff fixture"));
    assert!(all.iter().any(|m| m["text"] == "나중 답"));
}

#[cfg(target_os = "linux")]
#[test]
fn import_progress_is_readable_while_import_runs() {
    let temp = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    drop(listener);
    let _server = launch(
        &temp.path().join("data"),
        Path::new(env!("CARGO_BIN_EXE_fake-codex")),
        &address,
    );
    ready(&address);
    let exports: Vec<Value> = (0..5000).map(|i| json!({"uuid":format!("fixture-{i}"),"name":"fixture","created_at":"2026-01-01T00:00:00Z","chat_messages":[{"uuid":"one","sender":"human","text":"fixture","created_at":"2026-01-01T00:00:00Z"}]})).collect();
    let bytes = serde_json::to_vec(&exports).unwrap();
    let upload = request(
        &address,
        "POST",
        "/uploads",
        &json!({"fileName":"export.json","mediaType":"application/json","byteLength":bytes.len()})
            .to_string(),
    )
    .unwrap()["uploadId"]
        .as_str()
        .unwrap()
        .to_owned();
    put_bytes(
        &address,
        &format!("/uploads/{upload}/chunks?offset=0"),
        &bytes,
    )
    .unwrap();
    let asset = request(&address, "POST", &format!("/uploads/{upload}/finish"), "{}").unwrap()
        ["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let worker_address = address.clone();
    let worker = std::thread::spawn(move || {
        let body =
            json!({"assetId":asset,"provider":"claude","requestId":"large-fixture"}).to_string();
        let mut stream = TcpStream::connect(worker_address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .unwrap();
        write!(stream,"POST /chat-imports HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-only-token\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.contains(" 200 "), "{response}");
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(progress) = request(&address, "GET", "/chat-imports/large-fixture/progress", "")
        {
            if progress["state"] == "running" && progress["processed"].as_u64().unwrap_or(0) > 0 {
                assert_eq!(progress["total"], 5000);
                break;
            }
            assert_ne!(
                progress["state"], "completed",
                "completed before progress was observable"
            );
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    worker.join().unwrap();
    let final_progress =
        request(&address, "GET", "/chat-imports/large-fixture/progress", "").unwrap();
    assert_eq!(final_progress["state"], "completed");
    assert_eq!(final_progress["processed"], 5000);
}

#[cfg(target_os = "linux")]
#[test]
fn usage_reports_provider_limits_without_guessing() {
    let temp = tempfile::tempdir().unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = temp.path().join("data");
    let _server = launch(&data, Path::new(env!("CARGO_BIN_EXE_fake-codex")), &address);
    ready(&address);
    assert!(raw_get(&address, "/usage").contains(" 401 "));
    assert_eq!(
        request(&address, "GET", "/usage", "").unwrap()["providers"][0]["reason"],
        "not_connected"
    );
    std::fs::write(data.join("codex/auth.json"), "fixture").unwrap();
    let usage = request(&address, "GET", "/usage", "").unwrap();
    assert_eq!(usage["providers"][0]["state"], "ok");
    assert_eq!(usage["providers"][0]["windows"][0]["usedPercent"], 25.0);
    assert_eq!(
        usage["providers"][0]["windows"][0]["windowDurationMins"],
        300
    );
    assert_eq!(usage["providers"][0]["remaining"], Value::Null);
    assert_eq!(usage["providers"][1]["state"], "unavailable");
    assert_eq!(usage["providers"][2]["state"], "unavailable");
}

#[cfg(target_os = "linux")]
#[test]
fn codex_context_uses_only_reported_values_for_the_current_thread() {
    let temp = tempfile::tempdir().unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = temp.path().join("data");
    let cli = Path::new(env!("CARGO_BIN_EXE_fake-codex"));
    let server = launch(&data, cli, &address);
    ready(&address);
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = format!("/chats/{chat}/context");
    assert!(raw_get(&address, &path).contains(" 401 "));
    assert_eq!(
        request(&address, "GET", &path, "").unwrap()["state"],
        "unavailable"
    );
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":"context fixture"}).to_string(),
    )
    .unwrap();
    wait_state(&address, turn["turnId"].as_str().unwrap(), "completed");
    let value = request(&address, "GET", &path, "").unwrap();
    assert_eq!(value["provider"], "codex");
    assert_eq!(value["usedTokens"], 123);
    assert_eq!(value["limitTokens"], 1000);
    assert_eq!(value["usedPercent"], 12.3);
    assert_eq!(value["model"], Value::Null);
    drop(server);
    let _server = launch(&data, cli, &address);
    ready(&address);
    assert_eq!(
        request(&address, "GET", &path, "").unwrap()["usedTokens"],
        123
    );
}

#[cfg(target_os = "linux")]
#[test]
fn upload_resume_and_authenticated_chat_download() {
    let temp = tempfile::tempdir().unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = temp.path().join("data");
    let cli = Path::new(env!("CARGO_BIN_EXE_fake-codex"));
    let server = launch(&data, cli, &address);
    ready(&address);
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let started = request(
        &address,
        "POST",
        "/uploads",
        r#"{"fileName":"수업.txt","mediaType":"text/plain","byteLength":11}"#,
    )
    .unwrap();
    let id = started["uploadId"].as_str().unwrap().to_owned();
    assert_eq!(
        request(
            &address,
            "PUT",
            &format!("/uploads/{id}/chunks?offset=0"),
            "hello "
        )
        .unwrap()["received"],
        6
    );
    drop(server);
    let _server = launch(&data, cli, &address);
    ready(&address);
    assert_eq!(
        request(&address, "GET", &format!("/uploads/{id}"), "").unwrap()["received"],
        6
    );
    assert_eq!(
        request(
            &address,
            "PUT",
            &format!("/uploads/{id}/chunks?offset=6"),
            "world"
        )
        .unwrap()["received"],
        11
    );
    let asset = request(&address, "POST", &format!("/uploads/{id}/finish"), "{}").unwrap();
    assert_eq!(
        request(&address, "POST", &format!("/uploads/{id}/finish"), "{}").unwrap()["id"],
        asset["id"]
    );
    let asset_id = asset["id"].as_str().unwrap();
    request(
        &address,
        "POST",
        &format!("/chats/{chat}/attachments"),
        &json!({"assetId":asset_id}).to_string(),
    )
    .unwrap();
    assert!(raw_get(&address, &format!("/chats/{chat}/attachments/{asset_id}")).contains(" 401 "));
    let preview_path = format!("/chats/{chat}/attachments/{asset_id}/text");
    assert!(raw_get(&address, &preview_path).contains(" 401 "));
    let preview = request(&address, "GET", &preview_path, "").unwrap();
    assert_eq!(preview["state"], "ok");
    assert_eq!(preview["content"], "hello world");
    assert_eq!(preview["nextCursor"], Value::Null);
    assert_eq!(
        request(&address, "GET", &format!("{preview_path}?cursor=100"), "").unwrap()["state"],
        "invalid_cursor"
    );
    let other = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(request(
        &address,
        "GET",
        &format!("/chats/{other}/attachments/{asset_id}/text"),
        ""
    )
    .is_none());
    let mut korean = vec![b'a'; 65535];
    korean.extend_from_slice(&[0xb0, 0xa1]); // CP949 '가', split at the page boundary.
    let started = request(
        &address,
        "POST",
        "/uploads",
        &json!({"fileName":"korean.txt","mediaType":"text/plain","byteLength":korean.len()})
            .to_string(),
    )
    .unwrap();
    let upload = started["uploadId"].as_str().unwrap();
    put_bytes(
        &address,
        &format!("/uploads/{upload}/chunks?offset=0"),
        &korean,
    )
    .unwrap();
    let korean_asset =
        request(&address, "POST", &format!("/uploads/{upload}/finish"), "{}").unwrap();
    let korean_id = korean_asset["id"].as_str().unwrap();
    request(
        &address,
        "POST",
        &format!("/chats/{chat}/attachments"),
        &json!({"assetId":korean_id}).to_string(),
    )
    .unwrap();
    let path = format!("/chats/{chat}/attachments/{korean_id}/text");
    let first = request(&address, "GET", &path, "").unwrap();
    assert_eq!(first["nextCursor"], 65535);
    assert_eq!(first["content"].as_str().unwrap().len(), 65535);
    let second = request(&address, "GET", &format!("{path}?cursor=65535"), "").unwrap();
    assert_eq!(second["content"], "가");
    let image = b"\x89PNG\r\n\x1a\nfixture";
    let started = request(
        &address,
        "POST",
        "/uploads",
        &json!({"fileName":"photo.png","mediaType":"image/png","byteLength":image.len()})
            .to_string(),
    )
    .unwrap();
    let image_upload = started["uploadId"].as_str().unwrap();
    put_bytes(
        &address,
        &format!("/uploads/{image_upload}/chunks?offset=0"),
        image,
    )
    .unwrap();
    let image_asset = request(
        &address,
        "POST",
        &format!("/uploads/{image_upload}/finish"),
        "{}",
    )
    .unwrap();
    let image_id = image_asset["id"].as_str().unwrap();
    assert!(raw_get(&address, "/media").contains(" 401 "));
    assert!(raw_get(&address, "/assets/search?q=photo").contains(" 401 "));
    let found = request(&address, "GET", "/assets/search?q=photo", "").unwrap();
    assert_eq!(found["items"][0]["id"], image_id);
    assert!(request(&address, "GET", "/assets/search?q=", "").is_none());
    let items = request(&address, "GET", "/media?kind=image", "").unwrap();
    assert_eq!(items["items"][0]["assetId"], image_id);
    assert_eq!(items["items"][0]["prompt"], Value::Null);
    assert!(items["items"][0]["createdAt"].as_str().is_some());
    assert_eq!(items["items"][0]["createdAtSource"], "registered");
    assert_eq!(items["items"][0]["thumbnailState"], "not_applicable");
    assert!(
        request(&address, "GET", &format!("/media?chatId={chat}"), "").unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    request(
        &address,
        "POST",
        &format!("/chats/{chat}/attachments"),
        &json!({"assetId":image_id}).to_string(),
    )
    .unwrap();
    assert_eq!(
        request(&address, "GET", &format!("/media?chatId={chat}"), "").unwrap()["items"][0]
            ["assetId"],
        image_id
    );
    assert!(
        request(&address, "GET", &format!("/media?chatId={other}"), "").unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(raw_get(&address, &format!("/media/{image_id}/download")).contains(" 401 "));
    let mut media = TcpStream::connect(&address).unwrap();
    write!(media,"GET /media/{image_id}/download HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-only-token\r\nConnection: close\r\n\r\n").unwrap();
    let mut downloaded = Vec::new();
    media.read_to_end(&mut downloaded).unwrap();
    assert!(downloaded.ends_with(image));
    assert!(raw_get(&address, &format!("/assets/{asset_id}/download")).contains(" 401 "));
    let mut original = TcpStream::connect(&address).unwrap();
    write!(original,"GET /assets/{asset_id}/download HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-only-token\r\nConnection: close\r\n\r\n").unwrap();
    let mut downloaded_original = Vec::new();
    original.read_to_end(&mut downloaded_original).unwrap();
    assert!(downloaded_original.ends_with(b"hello world"));
    let mut stream = TcpStream::connect(&address).unwrap();
    write!(stream,"GET /chats/{chat}/attachments/{asset_id} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-only-token\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.contains(" 200 "));
    assert!(response
        .to_lowercase()
        .contains("x-content-type-options: nosniff"));
    assert!(response.ends_with("hello world"));
    let sent = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":"attachment fixture","attachments":[{"assetId":asset_id}]}).to_string(),
    )
    .unwrap();
    let turn = sent["turnId"].as_str().unwrap();
    assert_eq!(
        wait_state(&address, turn, "completed")["text"],
        "hello world"
    );
    let room = request(&address, "GET", &format!("/chats/{chat}"), "").unwrap();
    let messages = room["messages"].as_array().unwrap();
    assert_eq!(messages[0]["attachments"][0]["assetId"], asset_id);
    assert_eq!(messages[0]["attachments"][0]["name"], "수업.txt");
    assert_eq!(messages[0]["attachments"][0]["deleted"], false);
    assert!(messages[1]["attachments"].as_array().unwrap().is_empty());
    assert!(raw_get(&address, "/assets/trash").contains(" 401 "));
    assert_eq!(
        request(&address, "POST", &format!("/assets/{asset_id}/trash"), "{}").unwrap()["deleted"],
        true
    );
    let duplicate = request(&address, "POST", &format!("/assets/{asset_id}/trash"), "{}").unwrap();
    assert_eq!(duplicate["changed"], false);
    assert_eq!(
        request(&address, "GET", "/records", "").unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        request(&address, "GET", "/assets/trash", "").unwrap()["items"][0]["assetId"],
        asset_id
    );
    let purge = raw_post(&address, &format!("/assets/{asset_id}/purge"));
    assert!(purge.contains(" 409 ") && purge.contains("asset_in_use"));
    let purge_value: Value = serde_json::from_str(purge.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(purge_value["references"]["chats"], json!([chat]));
    assert_eq!(purge_value["references"]["documents"], json!([]));
    assert_eq!(
        request(&address, "GET", "/assets/trash", "").unwrap()["items"][0]["references"]["chats"],
        1
    );
    assert_eq!(
        request(&address, "GET", "/assets/trash", "").unwrap()["items"][0]["references"]
            ["documents"],
        0
    );
    let head = momso_core::document::ZetHead {
        id: "zet_20261122_120000".into(),
        title: "fixture".into(),
        summary: "fixture".into(),
        kind: "note".into(),
        status: "draft".into(),
        created_at: "2026-11-22T12:00:00Z".into(),
        updated_at: "2026-11-22T12:00:00Z".into(),
        assets: vec![format!("sha256:{}", asset["sha256"].as_str().unwrap())],
        edges: vec![],
        folder: String::new(),
        original_header: None,
    };
    let fixture = momso_core::document::render(&head, "fixture body").unwrap();
    std::fs::write(data.join("archive/zettels/zet_20261122_120000.md"), fixture).unwrap();
    assert_eq!(
        request(&address, "GET", "/assets/trash", "").unwrap()["items"][0]["references"]
            ["documents"],
        1
    );
    let purge = raw_post(&address, &format!("/assets/{asset_id}/purge"));
    let purge_value: Value = serde_json::from_str(purge.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        purge_value["references"]["documents"],
        json!(["zet_20261122_120000"])
    );
    std::fs::write(
        data.join("archive/zettels/zet_20261122_120001.md"),
        "invalid fixture",
    )
    .unwrap();
    assert_eq!(
        request(&address, "GET", "/assets/trash", "").unwrap()["items"][0]["references"]
            ["documents"],
        Value::Null
    );
    let unreadable_purge = raw_post(&address, &format!("/assets/{asset_id}/purge"));
    assert!(unreadable_purge.contains(" 500 ") && unreadable_purge.contains("storage_unavailable"));
    assert_eq!(
        request(&address, "GET", &format!("/chats/{chat}"), "").unwrap()["messages"][0]
            ["attachments"][0]["deleted"],
        true
    );
    assert!(request(&address, "GET", &preview_path, "").is_none());
    assert!(request(&address, "GET", &format!("/assets/{asset_id}/download"), "").is_none());
    let hash = asset["sha256"].as_str().unwrap();
    assert_eq!(
        std::fs::read(
            data.join("archive/objects/sha256")
                .join(&hash[..2])
                .join(hash)
        )
        .unwrap(),
        b"hello world"
    );
    assert!(request(
        &address,
        "POST",
        "/chats/not-a-chat/attachments",
        &json!({"assetId":asset_id}).to_string()
    )
    .is_none());
    drop(_server);
    let _restarted = launch(&data, cli, &address);
    ready(&address);
    let restored = request(&address, "GET", &format!("/chats/{chat}"), "").unwrap();
    assert_eq!(
        restored["messages"][0]["attachments"][0]["assetId"],
        asset_id
    );
    assert_eq!(restored["messages"][0]["attachments"][0]["deleted"], true);
    assert_eq!(
        request(
            &address,
            "POST",
            &format!("/assets/{asset_id}/restore"),
            "{}"
        )
        .unwrap()["deleted"],
        false
    );
    assert_eq!(
        request(&address, "GET", "/records", "").unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        request(&address, "GET", &preview_path, "").unwrap()["content"],
        "hello world"
    );
    // Simulate a restored backup whose original was permanently removed.
    drop(_restarted);
    let attachment_db = data.join("attachments/attachments.sqlite3");
    let database = rusqlite::Connection::open(attachment_db).unwrap();
    database
        .execute("INSERT INTO asset_purged(asset_id) VALUES(?1)", [asset_id])
        .unwrap();
    drop(database);
    std::fs::remove_file(
        data.join("archive/objects/sha256")
            .join(&hash[..2])
            .join(hash),
    )
    .unwrap();
    let _purged_restart = launch(&data, cli, &address);
    ready(&address);
    let restored = request(&address, "GET", &format!("/chats/{chat}"), "").unwrap();
    assert_eq!(
        restored["messages"][0]["attachments"][0]["name"],
        "수업.txt"
    );
    assert_eq!(restored["messages"][0]["attachments"][0]["deleted"], true);
    let purged_restore = raw_post(&address, &format!("/assets/{asset_id}/restore"));
    assert!(purged_restore.contains(" 409 ") && purged_restore.contains("asset_purged"));
    assert!(request(&address, "GET", &format!("/assets/{asset_id}/download"), "").is_none());
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires isolated ffmpeg installation"]
fn video_thumbnail_reads_only_verified_video_and_reports_invalid_format() {
    let temp = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    drop(listener);
    let _server = launch(
        &temp.path().join("data"),
        Path::new(env!("CARGO_BIN_EXE_fake-codex")),
        &address,
    );
    ready(&address);
    for (bytes, expected) in [
        (
            include_bytes!("fixtures/first-frame.mp4").as_slice(),
            " 200 ",
        ),
        (b"not a video".as_slice(), " 422 "),
    ] {
        let upload = request(
            &address,
            "POST",
            "/uploads",
            &json!({"fileName":"fixture.mp4","mediaType":"video/mp4","byteLength":bytes.len()})
                .to_string(),
        )
        .unwrap()["uploadId"]
            .as_str()
            .unwrap()
            .to_owned();
        put_bytes(
            &address,
            &format!("/uploads/{upload}/chunks?offset=0"),
            bytes,
        )
        .unwrap();
        let asset = request(&address, "POST", &format!("/uploads/{upload}/finish"), "{}").unwrap()
            ["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let gallery = request(&address, "GET", "/media?kind=video", "").unwrap();
        assert!(gallery["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["assetId"] == asset && item["thumbnailState"] == "on_demand"));
        assert!(raw_get(&address, &format!("/media/{asset}/thumbnail")).contains(" 401 "));
        let mut stream = TcpStream::connect(&address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(25)))
            .unwrap();
        write!(stream,"GET /media/{asset}/thumbnail HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-only-token\r\nConnection: close\r\n\r\n").unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        assert!(response
            .windows(expected.len())
            .any(|w| w == expected.as_bytes()));
        if expected == " 200 " {
            assert!(response.windows(3).any(|w| w == [0xff, 0xd8, 0xff]));
        } else {
            assert!(response
                .windows("thumbnail_unavailable".len())
                .any(|w| w == b"thumbnail_unavailable"));
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn fake_cli_creates_document_only_after_approval() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = tmp.path().join("data");
    let _server = launch(&data, Path::new(env!("CARGO_BIN_EXE_fake-codex")), &address);
    ready(&address);
    for (choice, expected) in [("deny", "denied"), ("allow_once", "ok")] {
        let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
            .as_str()
            .unwrap()
            .to_owned();
        let turn = request(
            &address,
            "POST",
            &format!("/chats/{chat}/turns"),
            r#"{"text":"document fixture"}"#,
        )
        .unwrap()["turnId"]
            .as_str()
            .unwrap()
            .to_owned();
        let deadline = Instant::now() + Duration::from_secs(10);
        let approval = loop {
            if let Some(a) = request(&address, "GET", &format!("/turns/{turn}/approval"), "")
                .and_then(|v| v.get("approval").cloned())
                .filter(|v| !v.is_null())
            {
                break a;
            }
            assert_eq!(
                request(&address, "GET", &format!("/turns/{turn}"), "").unwrap()["state"],
                "running"
            );
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(30));
        };
        assert_eq!(approval["kind"], "document_write");
        assert!(request(&address, "DELETE", &format!("/chats/{chat}"), "").is_none());
        assert!(request(&address, "GET", &format!("/chats/{chat}"), "").is_some());
        request(
            &address,
            "POST",
            &format!(
                "/approvals/{}/respond",
                approval["requestId"].as_str().unwrap()
            ),
            &json!({"choice":choice}).to_string(),
        )
        .unwrap();
        assert!(wait_state(&address, &turn, "completed")["text"]
            .as_str()
            .unwrap()
            .contains(expected));
        let documents = request(&address, "GET", "/documents", "").unwrap();
        assert_eq!(
            documents["documents"].as_array().unwrap().len(),
            if choice == "deny" { 0 } else { 1 }
        );
        let records = request(&address, "GET", "/records", "").unwrap();
        assert_eq!(
            records["items"].as_array().unwrap().len(),
            if choice == "deny" { 0 } else { 1 }
        );
        if choice == "allow_once" {
            assert_eq!(records["items"][0]["tool"], "codex");
            assert_eq!(records["items"][0]["chatId"], format!("chat_{chat}"));
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn provider_keys_and_chat_use_encrypted_vault_and_fake_api() {
    use age::secrecy::ExposeSecret;
    use base64::Engine;
    let tmp = tempfile::tempdir().unwrap();
    let credentials = tmp.path().join("credentials");
    std::fs::create_dir(&credentials).unwrap();
    let identity = age::x25519::Identity::generate().to_string();
    std::fs::write(
        credentials.join("momso-vault-identity"),
        identity.expose_secret(),
    )
    .unwrap();
    let claude_key = format!("fixture-claude-{}", uuid::Uuid::new_v4());
    let gemini_key = format!("fixture-gemini-{}", uuid::Uuid::new_v4());
    let expected_claude = claude_key.clone();
    let expected_gemini = gemini_key.clone();
    let api = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/fake", api.local_addr().unwrap());
    let fake = std::thread::spawn(move || {
        for _ in 0..5 {
            let (mut stream, _) = api.accept().unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0; 8192];
                let n = stream.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..n]);
                if let Some(i) = request.windows(4).position(|x| x == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&request[..i]);
                    let size = head.lines().find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|n| n.trim().parse::<usize>().ok())
                    });
                    if size.is_some_and(|n| request.len() >= i + 4 + n)
                        || (head.starts_with("GET ") && size.is_none())
                    {
                        break;
                    }
                }
            }
            let request = String::from_utf8_lossy(&request);
            let lower = request.to_lowercase();
            if request.starts_with("GET ") {
                let body = if lower.contains("x-api-key:") {
                    assert!(request.contains(&expected_claude));
                    r#"{"data":[{"id":"claude-opus-5-5","display_name":"Fixture Claude","capabilities":{"effort":{"high":{"supported":true}}}}],"has_more":false}"#
                } else {
                    assert!(request.contains(&expected_gemini));
                    r#"{"models":[{"name":"models/gemini-3-pro-preview","displayName":"Fixture Gemini","supportedGenerationMethods":["generateContent"]}]}"#
                };
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
                continue;
            }
            assert!(request.contains("가짜 파일 본문"));
            assert!(request.contains("s75-provider-skill-marker"));
            assert!(request.contains("image/png"));
            assert!(request.contains("application/pdf"));
            assert!(request.contains("iVBORw0KGgo"));
            let body = if lower.contains("x-api-key:") {
                assert!(request.contains(&expected_claude));
                assert!(request.contains("fixture_model=claude-opus-5-5"));
                assert!(!request.contains(&expected_gemini));
                assert!(request.contains("\"source\":{\"data\":"));
                assert!(request.contains("\"output_config\":{\"effort\":\"high\"}"));
                assert!(request.contains("\"speed\":\"fast\""));
                "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"가짜 답\"}}\n\ndata: {\"type\":\"message_stop\"}\n\n"
            } else {
                assert!(lower.contains("x-goog-api-key:"));
                assert!(request.contains(&expected_gemini));
                assert!(request.contains("fixture_model=gemini-3-pro-preview"));
                assert!(!request.contains(&expected_claude));
                assert!(request.contains("\"inline_data\""));
                assert!(request.contains("\"thinkingLevel\":\"high\""));
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"가짜 답\"}]}}]}\n\ndata: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n"
            };
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        }
    });
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = tmp.path().join("data");
    let binary = std::env::var_os("MOMSO_TEST_SERVER_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_momso-server").into());
    let _server = Server(
        Command::new(binary)
            .env("MOMSO_SERVER_DATA_DIR", &data)
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_LISTEN", &address)
            .env("MOMSO_CODEX_PATH", env!("CARGO_BIN_EXE_fake-codex"))
            .env("CREDENTIALS_DIRECTORY", credentials)
            .env("MOMSO_TEST_MODEL_ENDPOINT", &endpoint)
            .env("MOMSO_TEST_PROVIDER_ENDPOINT", endpoint)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address);
    request(&address,"POST","/skills",r#"{"name":"provider-skill","title":"Test","description":"Test","body":"s75-provider-skill-marker"}"#).unwrap();
    for (provider, key) in [("claude", &claude_key), ("gemini", &gemini_key)] {
        let pending = request(
            &address,
            "POST",
            &format!("/providers/{provider}/key/request"),
            "{}",
        )
        .unwrap();
        assert_eq!(pending["kind"], "provider_api_key");
        let answer = request(
            &address,
            "POST",
            &format!(
                "/input-requests/{}/respond",
                pending["requestId"].as_str().unwrap()
            ),
            &json!({"apiKey": key}).to_string(),
        )
        .unwrap();
        assert!(!answer.to_string().contains(key.as_str()));
        let states = request(&address, "GET", "/providers", "").unwrap();
        assert!(states["providers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["provider"] == provider && p["state"] == "credential_saved"));
        assert!(!states.to_string().contains(key.as_str()));
        let catalog = request(&address, "GET", "/providers/models", "").unwrap();
        assert!(!catalog.to_string().contains(key.as_str()));
        let selected = catalog["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["provider"] == provider)
            .unwrap()["models"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            selected,
            if provider == "claude" {
                "claude-opus-5-5"
            } else {
                "gemini-3-pro-preview"
            }
        );
        let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
            .as_str()
            .unwrap()
            .to_owned();
        let png=base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC").unwrap();
        let mut assets = Vec::new();
        for (name, mime, data) in [
            ("a.txt", "text/plain", "가짜 파일 본문".as_bytes()),
            ("a.png", "image/png", png.as_slice()),
            ("a.pdf", "application/pdf", b"%PDF-1.4\n%%EOF\n".as_slice()),
        ] {
            let upload = request(
                &address,
                "POST",
                "/uploads",
                &json!({"fileName":name,"mediaType":mime,"byteLength":data.len()}).to_string(),
            )
            .unwrap();
            let id = upload["uploadId"].as_str().unwrap();
            put_bytes(&address, &format!("/uploads/{id}/chunks?offset=0"), data).unwrap();
            let asset = request(&address, "POST", &format!("/uploads/{id}/finish"), "{}").unwrap();
            assets.push(json!({"assetId":asset["id"]}));
        }
        let mut turn_body = json!({"provider":provider,"model":selected,"text":"안녕","skill":"provider-skill","attachments":assets});
        turn_body["effort"] = json!("high");
        if provider == "claude" {
            turn_body["fast"] = json!(true);
        }
        let turn = request(
            &address,
            "POST",
            &format!("/chats/{chat}/turns"),
            &turn_body.to_string(),
        )
        .unwrap()["turnId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(wait_state(&address, &turn, "completed")["text"], "가짜 답");
        let db = rusqlite::Connection::open(data.join("server.sqlite3")).unwrap();
        let (saved_effort, saved_fast): (Option<String>, Option<String>) = db
            .query_row("SELECT effort,fast FROM turns WHERE id=?1", [&turn], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(saved_effort.as_deref(), Some("high"));
        assert_eq!(
            saved_fast.as_deref(),
            Some(if provider == "claude" { "1" } else { "0" })
        );
        let vault =
            std::fs::read(data.join(format!("provider-credentials/{provider}/vault.age"))).unwrap();
        assert!(!vault.windows(key.len()).any(|w| w == key.as_bytes()));
    }
    for suffix in ["server.sqlite3", "server.sqlite3-wal"] {
        if let Ok(bytes) = std::fs::read(data.join(suffix)) {
            for key in [&claude_key, &gemini_key] {
                assert!(!bytes.windows(key.len()).any(|w| w == key.as_bytes()));
            }
        }
    }
    fake.join().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn fake_cli_crm_write_requires_approval() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = tmp.path().join("data");
    let _server = launch(&data, Path::new(env!("CARGO_BIN_EXE_fake-codex")), &address);
    ready(&address);
    for (choice, expected) in [("deny", "denied"), ("allow_once", "ok")] {
        let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
            .as_str()
            .unwrap()
            .to_owned();
        let turn = request(
            &address,
            "POST",
            &format!("/chats/{chat}/turns"),
            r#"{"text":"crm direct fixture"}"#,
        )
        .unwrap()["turnId"]
            .as_str()
            .unwrap()
            .to_owned();
        let deadline = Instant::now() + Duration::from_secs(10);
        let approval = loop {
            if let Some(a) = request(&address, "GET", &format!("/turns/{turn}/approval"), "")
                .and_then(|v| v.get("approval").cloned())
                .filter(|v| !v.is_null())
            {
                break a;
            }
            let state = request(&address, "GET", &format!("/turns/{turn}"), "").unwrap();
            assert_eq!(state["state"], "running", "{state}");
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(30));
        };
        assert_eq!(approval["kind"], "crm_write");
        if choice == "deny" {
            assert!(!data.join("class-student-private.sqlite3").exists());
        }
        request(
            &address,
            "POST",
            &format!(
                "/approvals/{}/respond",
                approval["requestId"].as_str().unwrap()
            ),
            &json!({"choice":choice}).to_string(),
        )
        .unwrap();
        assert!(wait_state(&address, &turn, "completed")["text"]
            .as_str()
            .unwrap()
            .contains(expected));
        if choice == "deny" {
            assert!(!data.join("class-student-private.sqlite3").exists());
        }
        let records = request(&address, "GET", "/records", "").unwrap();
        assert_eq!(
            records["items"].as_array().unwrap().len(),
            if choice == "deny" { 0 } else { 1 }
        );
        if choice == "allow_once" {
            assert_eq!(records["items"][0]["tool"], "codex");
            assert_eq!(records["items"][0]["chatId"], format!("chat_{chat}"));
        }
    }
    assert!(data.join("class-student-private.sqlite3").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn preferences_survive_restart_and_reject_stale_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    drop(listener);
    let data = tmp.path().join("data");
    let cli = Path::new(env!("CARGO_BIN_EXE_fake-codex"));
    let server = launch(&data, cli, &address);
    ready(&address);
    assert!(raw_get(&address, "/preferences").contains(" 401 "));
    let before = request(&address, "GET", "/preferences", "").unwrap();
    let body = json!({"revision":before["revision"], "preferences":{"assistantProvider":"gemini", "chatModel":{"provider":"gemini","model":"gemini-test","effort":"low","fast":false}}}).to_string();
    let saved = request(&address, "PUT", "/preferences", &body).unwrap();
    assert_eq!(saved["preferences"]["assistantProvider"], "gemini");
    assert!(request(&address, "PUT", "/preferences", &body).is_none());
    drop(server);
    let _server = launch(&data, cli, &address);
    ready(&address);
    let after = request(&address, "GET", "/preferences", "").unwrap();
    assert_eq!(after, saved);
    let invalid = json!({"revision":after["revision"], "preferences":{"assistantProvider":"gemini", "chatModel":null,"secret":"fixture"}}).to_string();
    assert!(request(&address, "PUT", "/preferences", &invalid).is_none());
    assert_eq!(request(&address, "GET", "/preferences", "").unwrap(), saved);
}

#[cfg(target_os = "linux")]
#[test]
fn disconnect_removes_only_selected_provider_credentials() {
    use age::secrecy::ExposeSecret;
    use momso_server_providers::{vault, ApiKey};
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let credentials = tmp.path().join("credentials");
    std::fs::create_dir_all(&credentials).unwrap();
    let keyfile = credentials.join("momso-vault-identity");
    std::fs::write(
        &keyfile,
        age::x25519::Identity::generate()
            .to_string()
            .expose_secret(),
    )
    .unwrap();
    let claude = momso_saved_login_core::linux::LinuxVault::from_credential_file(
        &data.join("provider-credentials/claude"),
        &keyfile,
    )
    .unwrap();
    let gemini = momso_saved_login_core::linux::LinuxVault::from_credential_file(
        &data.join("provider-credentials/gemini"),
        &keyfile,
    )
    .unwrap();
    vault::save(&claude, &ApiKey::new("fake-claude-key".into()).unwrap()).unwrap();
    vault::save(&gemini, &ApiKey::new("fake-gemini-key".into()).unwrap()).unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let _server = Server(
        Command::new(env!("CARGO_BIN_EXE_momso-server"))
            .env("MOMSO_SERVER_DATA_DIR", &data)
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_LISTEN", &address)
            .env("MOMSO_CODEX_PATH", env!("CARGO_BIN_EXE_fake-codex"))
            .env("CREDENTIALS_DIRECTORY", &credentials)
            .env("MOMSO_TEST_DISABLE_AUTO_BACKUP", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address);
    std::fs::write(data.join("codex/auth.json"), "fixture").unwrap();
    assert!(raw_get(&address, "/providers").contains(" 401 "));
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        r#"{"text":"fixture"}"#,
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(raw_post(&address, "/providers/codex/disconnect").contains("turn_in_progress"));
    wait_state(&address, &turn, "completed");
    assert_eq!(
        request(&address, "POST", "/providers/claude/disconnect", "{}").unwrap()["state"],
        "not_connected"
    );
    assert!(vault::load(&claude).unwrap().is_none());
    assert!(vault::load(&gemini).unwrap().is_some());
    assert!(data.join("codex/auth.json").exists());
    assert_eq!(
        request(&address, "POST", "/providers/codex/disconnect", "{}").unwrap()["state"],
        "not_connected"
    );
    assert!(!data.join("codex/auth.json").exists());
    assert_eq!(
        request(&address, "POST", "/providers/gemini/disconnect", "{}").unwrap()["state"],
        "not_connected"
    );
    assert!(vault::load(&gemini).unwrap().is_none());
    let login = request(&address, "POST", "/providers/codex/login", "{}").unwrap();
    assert!(raw_post(&address, "/providers/codex/disconnect").contains("login_in_progress"));
    vault::save(&claude, &ApiKey::new("fake-claude-key".into()).unwrap()).unwrap();
    assert_eq!(
        request(&address, "POST", "/providers/claude/disconnect", "{}").unwrap()["state"],
        "not_connected"
    );
    assert!(vault::load(&claude).unwrap().is_none());
    login_request(
        &address,
        "POST",
        "/providers/codex/login/cancel",
        login["sessionId"].as_str(),
    )
    .unwrap();
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires isolated headless Chromium"]
fn fake_cli_uses_skill_collects_site_and_reads_local_crm() {
    use age::secrecy::ExposeSecret;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    let tmp = tempfile::tempdir().unwrap();
    let credential_dir = tmp.path().join("credentials");
    std::fs::create_dir_all(&credential_dir).unwrap();
    let identity = age::x25519::Identity::generate().to_string();
    std::fs::write(
        credential_dir.join("momso-vault-identity"),
        identity.expose_secret(),
    )
    .unwrap();
    let site = TcpListener::bind("127.0.0.1:0").unwrap();
    let site_address = site.local_addr().unwrap();
    site.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let hits = Arc::new(AtomicUsize::new(0));
    let (site_stop, site_hits) = (stop.clone(), hits.clone());
    let worker = std::thread::spawn(move || {
        while !site_stop.load(Ordering::SeqCst) {
            if let Ok((mut stream, _)) = site.accept() {
                let mut request = [0; 4096];
                let _ = stream.read(&mut request);
                site_hits.fetch_add(1, Ordering::SeqCst);
                let body = "<html><title>시간표</title><body>가짜 수업</body></html>";
                let _ = write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    });
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = tmp.path().join("data");
    let binary = std::env::var_os("MOMSO_TEST_SERVER_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_momso-server").into());
    let _server = Server(
        Command::new(binary)
            .env("MOMSO_SERVER_DATA_DIR", &data)
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_LISTEN", &address)
            .env("MOMSO_CODEX_PATH", env!("CARGO_BIN_EXE_fake-codex"))
            .env("CREDENTIALS_DIRECTORY", credential_dir)
            .env(
                "MOMSO_CHROMIUM_PATH",
                std::env::var_os("MOMSO_TEST_CHROMIUM").expect("test Chromium path"),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address);
    assert!(data.join("archive/dictionary/yoga.json").is_file());
    assert!(data
        .join("archive/skills/bodycodi-operations/SKILL.md")
        .is_file());
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":format!("crm fixture http://{site_address}/")}).to_string(),
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    // Reading a site needs no write approval; storing the observation does.
    {
        let expected = "crm_write";
        let deadline = Instant::now() + Duration::from_secs(40);
        let approval = loop {
            let state = request(&address, "GET", &format!("/turns/{turn}"), "").unwrap();
            assert_eq!(
                state["state"], "running",
                "turn ended before {expected}: {state}"
            );
            if let Some(a) = request(&address, "GET", &format!("/turns/{turn}/approval"), "")
                .and_then(|v| v.get("approval").cloned())
                .filter(|v| !v.is_null() && v["kind"] == expected)
            {
                break a;
            }
            assert!(Instant::now() < deadline, "missing approval {expected}");
            std::thread::sleep(Duration::from_millis(50));
        };
        request(
            &address,
            "POST",
            &format!(
                "/approvals/{}/respond",
                approval["requestId"].as_str().unwrap()
            ),
            r#"{"choice":"allow_once"}"#,
        )
        .unwrap();
    }
    assert!(wait_state(&address, &turn, "completed")["text"]
        .as_str()
        .unwrap()
        .contains("ok"));
    std::thread::sleep(Duration::from_millis(500));
    let before = hits.load(Ordering::SeqCst);
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        r#"{"text":"crm lookup fixture"}"#,
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(wait_state(&address, &turn, "completed")["text"]
        .as_str()
        .unwrap()
        .contains("가짜 수업"));
    assert_eq!(hits.load(Ordering::SeqCst), before);
    stop.store(true, Ordering::SeqCst);
    worker.join().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires isolated Chromium, Playwright and a network screen build"]
fn mobile_s47_real_screen_probe() {
    use age::secrecy::ExposeSecret;
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let credentials = temp.path().join("credentials");
    std::fs::create_dir_all(&credentials).unwrap();
    std::fs::create_dir_all(data.join("codex")).unwrap();
    std::fs::write(data.join("codex/auth.json"), "fixture").unwrap();
    let identity = age::x25519::Identity::generate().to_string();
    std::fs::write(
        credentials.join("momso-vault-identity"),
        identity.expose_secret(),
    )
    .unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let binary = std::env::var_os("MOMSO_TEST_SERVER_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_momso-server").into());
    let _server = Server(
        Command::new(binary)
            .env("MOMSO_SERVER_DATA_DIR", &data)
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_LISTEN", &address)
            .env("MOMSO_CODEX_PATH", env!("CARGO_BIN_EXE_fake-codex"))
            .env(
                "MOMSO_SERVER_WEB_DIR",
                std::env::var_os("MOMSO_S47_SCREEN_DIR").expect("network screen"),
            )
            .env("CREDENTIALS_DIRECTORY", credentials)
            .env(
                "MOMSO_CHROMIUM_PATH",
                std::env::var_os("MOMSO_TEST_CHROMIUM").expect("test Chromium"),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address);
    let output = Command::new("node")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/mobile_s47.cjs"
        ))
        .env("MOMSO_S47_SERVER", address)
        .env("MOMSO_S47_SCREENSHOT", temp.path().join("screen.png"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(temp.path().join("screen.png").is_file());
    println!("{}", String::from_utf8_lossy(&output.stdout));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires isolated headless Chromium"]
fn human_login_notice_reaches_phone_event_stream() {
    use age::secrecy::ExposeSecret;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let temp = tempfile::tempdir().unwrap();
    let credentials = temp.path().join("credentials");
    std::fs::create_dir_all(&credentials).unwrap();
    let identity = age::x25519::Identity::generate().to_string();
    std::fs::write(
        credentials.join("momso-vault-identity"),
        identity.expose_secret(),
    )
    .unwrap();
    let site = TcpListener::bind("127.0.0.1:0").unwrap();
    let site_address = site.local_addr().unwrap();
    site.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let site_stop = stop.clone();
    let worker = std::thread::spawn(move || {
        while !site_stop.load(Ordering::SeqCst) {
            if let Ok((mut stream, _)) = site.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut buf = [0; 4096];
                let _ = stream.read(&mut buf);
                let body = "<html><body><div class='g-recaptcha'>Verification required</div></body></html>";
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    });
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let binary = std::env::var_os("MOMSO_TEST_SERVER_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_momso-server").into());
    let _server = Server(
        Command::new(binary)
            .env("MOMSO_SERVER_DATA_DIR", temp.path().join("data"))
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_LISTEN", &address)
            .env("MOMSO_CODEX_PATH", env!("CARGO_BIN_EXE_fake-codex"))
            .env("CREDENTIALS_DIRECTORY", credentials)
            .env(
                "MOMSO_CHROMIUM_PATH",
                std::env::var_os("MOMSO_TEST_CHROMIUM").expect("test Chromium path"),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address);
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":format!("otp fixture http://{site_address}/")}).to_string(),
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = wait_state(&address, &turn, "completed");
    let events = read_events(&address, &turn);
    assert!(events.contains("browser-human-required"), "{events}");
    assert!(events.contains("verification"), "{events}");
    stop.store(true, Ordering::SeqCst);
    worker.join().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires headless Chromium and a live public website"]
fn fake_codex_public_site_open_read_click_read() {
    use age::secrecy::ExposeSecret;
    let temp = tempfile::tempdir().unwrap();
    let credentials = temp.path().join("credentials");
    std::fs::create_dir_all(&credentials).unwrap();
    let identity = age::x25519::Identity::generate().to_string();
    std::fs::write(
        credentials.join("momso-vault-identity"),
        identity.expose_secret(),
    )
    .unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let binary = std::env::var_os("MOMSO_TEST_SERVER_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_momso-server").into());
    let _server = Server(
        Command::new(binary)
            .env("MOMSO_SERVER_DATA_DIR", temp.path().join("data"))
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_LISTEN", &address)
            .env("MOMSO_CODEX_PATH", env!("CARGO_BIN_EXE_fake-codex"))
            .env("CREDENTIALS_DIRECTORY", credentials)
            .env(
                "MOMSO_CHROMIUM_PATH",
                std::env::var_os("MOMSO_TEST_CHROMIUM").expect("test Chromium path"),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address);
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":"public browser fixture"}).to_string(),
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    let deadline = Instant::now() + Duration::from_secs(60);
    let approval = loop {
        let state = request(&address, "GET", &format!("/turns/{turn}"), "").unwrap();
        assert_eq!(
            state["state"], "running",
            "turn ended before site approval: {state}"
        );
        if let Some(approval) = request(&address, "GET", &format!("/turns/{turn}/approval"), "")
            .and_then(|v| v.get("approval").cloned())
            .filter(|v| !v.is_null())
        {
            break approval;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(approval["kind"], "browser_site");
    request(
        &address,
        "POST",
        &format!(
            "/approvals/{}/respond",
            approval["requestId"].as_str().unwrap()
        ),
        &json!({"choice":"allow_once"}).to_string(),
    )
    .unwrap();
    let finished = wait_state(&address, &turn, "completed");
    assert!(
        finished["text"]
            .as_str()
            .unwrap_or("")
            .starts_with("public browser finished:"),
        "{finished}"
    );
    assert!(
        finished["text"].as_str().unwrap_or("").len() > 40,
        "{finished}"
    );
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires isolated headless Chromium"]
fn fake_cli_browser_site_approval_round_trip() {
    use age::secrecy::ExposeSecret;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    let temp = tempfile::tempdir().unwrap();
    let credential_dir = temp.path().join("credentials");
    std::fs::create_dir_all(&credential_dir).unwrap();
    let identity = age::x25519::Identity::generate().to_string();
    std::fs::write(
        credential_dir.join("momso-vault-identity"),
        identity.expose_secret(),
    )
    .unwrap();
    let site = TcpListener::bind("127.0.0.1:0").unwrap();
    let site_address = site.local_addr().unwrap();
    site.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let writes = Arc::new(AtomicUsize::new(0));
    let site_stop = stop.clone();
    let site_writes = writes.clone();
    let site_thread = std::thread::spawn(move || {
        while !site_stop.load(Ordering::SeqCst) {
            if let Ok((mut stream, _)) = site.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut buffer = [0u8; 8192];
                let n = stream.read(&mut buffer).unwrap_or(0);
                if String::from_utf8_lossy(&buffer[..n]).starts_with("POST /publish") {
                    site_writes.fetch_add(1, Ordering::SeqCst);
                }
                let body="<html><title>가짜 수업</title><body><form action='/publish' method='post'><button>등록</button></form></body></html>";
                let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    });
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let binary = std::env::var_os("MOMSO_TEST_SERVER_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_momso-server").into());
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let chrome = std::env::var_os("MOMSO_TEST_CHROMIUM").expect("test Chromium path");
    let _server = Server(
        Command::new(binary)
            .env("MOMSO_SERVER_DATA_DIR", temp.path().join("data"))
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_LISTEN", &address)
            .env("MOMSO_CODEX_PATH", cli)
            .env("CREDENTIALS_DIRECTORY", credential_dir)
            .env("MOMSO_CHROMIUM_PATH", chrome)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address);
    for (choice, expected) in [("allow_once", "게시됨"), ("deny", "거절됨")] {
        let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
            .as_str()
            .unwrap()
            .to_owned();
        let turn = request(
            &address,
            "POST",
            &format!("/chats/{chat}/turns"),
            &json!({"text":format!("browser fixture http://{site_address}/")}).to_string(),
        )
        .unwrap()["turnId"]
            .as_str()
            .unwrap()
            .to_owned();
        let deadline = Instant::now() + Duration::from_secs(40);
        let approval = loop {
            let turn_state = request(&address, "GET", &format!("/turns/{turn}"), "").unwrap();
            assert_eq!(
                turn_state["state"], "running",
                "browser failed before approval: {turn_state}"
            );
            if let Some(value) = request(&address, "GET", &format!("/turns/{turn}/approval"), "")
                .and_then(|v| v.get("approval").cloned())
                .filter(|v| !v.is_null())
            {
                break value;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(approval["kind"], "browser_site");
        let id = approval["requestId"].as_str().unwrap();
        request(
            &address,
            "POST",
            &format!("/approvals/{id}/respond"),
            &json!({"choice":choice}).to_string(),
        )
        .unwrap();
        assert_eq!(wait_state(&address, &turn, "completed")["text"], expected);
    }
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    stop.store(true, Ordering::SeqCst);
    site_thread.join().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn phone_saved_login_is_encrypted_and_never_echoed() {
    use age::secrecy::ExposeSecret;
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let credentials = temp.path().join("credentials");
    std::fs::create_dir_all(&credentials).unwrap();
    let identity = age::x25519::Identity::generate().to_string();
    std::fs::write(
        credentials.join("momso-vault-identity"),
        identity.expose_secret(),
    )
    .unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let binary = std::env::var_os("MOMSO_TEST_SERVER_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_momso-server").into());
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let _server = Server(
        Command::new(binary)
            .env("MOMSO_SERVER_DATA_DIR", &data)
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_LISTEN", &address)
            .env("MOMSO_CODEX_PATH", cli)
            .env("CREDENTIALS_DIRECTORY", &credentials)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address);
    let password = "fixture-never-echo-password";
    let body = json!({"siteUrl":"https://example.test/login","label":"test","username":"fixture-user","password":password});
    let saved = request(&address, "POST", "/saved-logins", &body.to_string()).unwrap();
    assert!(!saved.to_string().contains(password));
    assert!(saved["login"]["id"].as_str().is_some());
    let id = saved["login"]["id"].as_str().unwrap();
    let list = request(&address, "GET", "/saved-logins", "").unwrap();
    assert_eq!(list["logins"].as_array().unwrap().len(), 1);
    assert!(!list.to_string().contains(password));
    let updated = request(
        &address,
        "PUT",
        &format!("/saved-logins/{id}"),
        r#"{"label":"changed"}"#,
    )
    .unwrap();
    assert_eq!(updated["login"]["label"], "changed");
    assert!(!updated.to_string().contains(password));
    let vault = momso_saved_login_core::linux::LinuxVault::from_credential_file(
        &data.join("saved-logins"),
        &credentials.join("momso-vault-identity"),
    )
    .unwrap();
    assert_eq!(
        vault
            .browser_login(
                id,
                "https://example.test/login",
                "https://example.test/login",
                true
            )
            .unwrap()
            .password,
        password
    );
    let encrypted = std::fs::read(data.join("saved-logins/vault.age")).unwrap();
    assert!(!encrypted
        .windows(password.len())
        .any(|w| w == password.as_bytes()));
    assert!(!encrypted
        .windows("fixture-user".len())
        .any(|w| w == b"fixture-user"));
    assert!(request(&address, "DELETE", &format!("/saved-logins/{id}"), "").is_some());
    assert!(
        request(&address, "GET", "/saved-logins", "").unwrap()["logins"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires isolated headless Chromium"]
fn verification_request_survives_reconnect_and_redacts_after_restart() {
    use age::secrecy::ExposeSecret;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let credentials = temp.path().join("credentials");
    std::fs::create_dir_all(&credentials).unwrap();
    let identity = age::x25519::Identity::generate().to_string();
    std::fs::write(
        credentials.join("momso-vault-identity"),
        identity.expose_secret(),
    )
    .unwrap();
    let site = TcpListener::bind("127.0.0.1:0").unwrap();
    let site_address = site.local_addr().unwrap();
    site.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let received = Arc::new(AtomicUsize::new(0));
    let site_stop = stop.clone();
    let site_received = received.clone();
    let site_thread = std::thread::spawn(move || {
        while !site_stop.load(Ordering::SeqCst) {
            if let Ok((mut stream, _)) = site.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut data = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => data.extend_from_slice(&buf[..n]),
                    }
                    if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&data[..end]);
                        let len = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if data.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let request = String::from_utf8_lossy(&data);
                if request.starts_with("POST /verify") && request.contains("649217") {
                    site_received.fetch_add(1, Ordering::SeqCst);
                }
                let body = if request.starts_with("GET /challenge") {
                    "<html><title>인증</title><body><form action='/verify' method='post'><input name='code' autocomplete='one-time-code'></form></body></html>"
                } else {
                    "<html><title>결과</title><body>이전 인증번호 649217</body></html>"
                };
                let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    });
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let binary = std::env::var_os("MOMSO_TEST_SERVER_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_momso-server").into());
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let chrome = std::env::var_os("MOMSO_TEST_CHROMIUM").expect("test Chromium path");
    let spawn = || {
        Server(
            Command::new(&binary)
                .env("MOMSO_SERVER_DATA_DIR", &data)
                .env("MOMSO_SERVER_TOKEN", "test-only-token")
                .env("MOMSO_SERVER_LISTEN", &address)
                .env("MOMSO_CODEX_PATH", &cli)
                .env("CREDENTIALS_DIRECTORY", &credentials)
                .env("MOMSO_CHROMIUM_PATH", &chrome)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    };
    let server = spawn();
    ready(&address);
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":format!("otp fixture http://{site_address}/challenge")}).to_string(),
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    let event_address = address.clone();
    let event_turn = turn.clone();
    let (seen_tx, seen_rx) = std::sync::mpsc::sync_channel(1);
    let events = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut stream = TcpStream::connect(event_address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        write!(stream,"GET /turns/{event_turn}/events HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-only-token\r\nConnection: close\r\n\r\n").unwrap();
        let mut reader = std::io::BufReader::new(stream);
        let mut all = String::new();
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            if line.contains("agent-input-request") {
                let _ = seen_tx.try_send(());
            }
            all.push_str(&line);
            line.clear();
        }
        all
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let input = loop {
        let current = request(&address, "GET", &format!("/turns/{turn}/requests"), "").unwrap();
        if let Some(input) = current["inputs"].as_array().and_then(|v| v.first()) {
            break input.clone();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(40));
    };
    let work = request(&address, "GET", "/activity", "").unwrap();
    assert_eq!(work["busy"], true);
    assert_eq!(work["pendingInputs"], 1);
    assert_eq!(input["kind"], "one_time_code");
    let id = input["requestId"].as_str().unwrap();
    assert_eq!(
        request(&address, "GET", &format!("/turns/{turn}/requests"), "").unwrap()["inputs"][0]
            ["requestId"],
        id
    );
    seen_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("input event");
    assert!(request(
        &address,
        "POST",
        &format!("/input-requests/{id}/respond"),
        r#"{"code":"bad!"}"#
    )
    .is_none());
    assert_eq!(
        request(&address, "GET", &format!("/turns/{turn}/requests"), "").unwrap()["inputs"][0]
            ["requestId"],
        id
    );
    let answer = request(
        &address,
        "POST",
        &format!("/input-requests/{id}/respond"),
        r#"{"code":"649217"}"#,
    )
    .unwrap();
    assert!(!answer.to_string().contains("649217"));
    assert!(request(
        &address,
        "POST",
        &format!("/input-requests/{id}/respond"),
        r#"{"code":"649217"}"#
    )
    .is_none());
    let completed = wait_state(&address, &turn, "completed");
    assert!(!completed.to_string().contains("649217"));
    assert_eq!(received.load(Ordering::SeqCst), 1);
    let event_text = events.join().unwrap();
    assert!(event_text.contains("agent-input-request"));
    assert!(event_text.contains("agent-input-closed"));
    assert!(!event_text.contains("649217"));
    drop(server);
    for name in ["server.sqlite3", "server.sqlite3-wal"] {
        if let Ok(bytes) = std::fs::read(data.join(name)) {
            assert!(!bytes.windows(6).any(|w| w == b"649217"));
        }
    }
    let restarted = spawn();
    ready(&address);
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":format!("read fixture http://{site_address}/echo")}).to_string(),
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    let after = wait_state(&address, &turn, "completed");
    assert!(after["text"].as_str().unwrap().contains("[redacted]"));
    assert!(!after.to_string().contains("649217"));
    drop(restarted);
    let new_identity = age::x25519::Identity::generate().to_string();
    std::fs::write(
        credentials.join("momso-vault-identity"),
        new_identity.expose_secret(),
    )
    .unwrap();
    let _server = spawn();
    ready(&address);
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":format!("read fixture http://{site_address}/echo")}).to_string(),
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    let blocked = wait_state(&address, &turn, "completed");
    assert_eq!(blocked["text"], "no page");
    assert!(!blocked.to_string().contains("649217"));
    stop.store(true, Ordering::SeqCst);
    site_thread.join().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires isolated headless Chromium"]
fn requested_login_survives_reconnect_and_reaches_only_site() {
    use age::secrecy::ExposeSecret;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let credentials = temp.path().join("credentials");
    std::fs::create_dir_all(&credentials).unwrap();
    let identity = age::x25519::Identity::generate().to_string();
    std::fs::write(
        credentials.join("momso-vault-identity"),
        identity.expose_secret(),
    )
    .unwrap();
    let site = TcpListener::bind("127.0.0.1:0").unwrap();
    let site_address = site.local_addr().unwrap();
    site.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let received = Arc::new(AtomicUsize::new(0));
    let site_stop = stop.clone();
    let site_received = received.clone();
    let site_thread = std::thread::spawn(move || {
        while !site_stop.load(Ordering::SeqCst) {
            if let Ok((mut stream, _)) = site.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut data = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => data.extend_from_slice(&buf[..n]),
                    }
                    if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&data[..end]);
                        let len = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if data.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let request = String::from_utf8_lossy(&data);
                if request.starts_with("POST /login")
                    && ((request.contains("fixture-user") && request.contains("fixture-pass"))
                        || (request.contains("second-user") && request.contains("second-pass")))
                {
                    site_received.fetch_add(1, Ordering::SeqCst);
                }
                let body = if request.starts_with("GET /login") {
                    "<html><title>로그인</title><body><form action='/login' method='post'><input name='username' autocomplete='username'><input name='password' type='password' autocomplete='current-password'><button>로그인</button></form></body></html>"
                } else {
                    "<html><title>완료</title><body>완료 fixture-user fixture-pass</body></html>"
                };
                let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    });
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let binary = std::env::var_os("MOMSO_TEST_SERVER_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_momso-server").into());
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let chrome = std::env::var_os("MOMSO_TEST_CHROMIUM").expect("test Chromium path");
    let _server = Server(
        Command::new(&binary)
            .env("MOMSO_SERVER_DATA_DIR", &data)
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_LISTEN", &address)
            .env("MOMSO_CODEX_PATH", &cli)
            .env("CREDENTIALS_DIRECTORY", &credentials)
            .env("MOMSO_CHROMIUM_PATH", &chrome)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address);
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":format!("login fixture http://{site_address}/login")}).to_string(),
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    let deadline = Instant::now() + Duration::from_secs(30);
    let approval = loop {
        let current = request(&address, "GET", &format!("/turns/{turn}/requests"), "").unwrap();
        if !current["approval"].is_null() {
            break current["approval"].clone();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(40));
    };
    assert_eq!(approval["kind"], "browser_login");
    let id = approval["requestId"].as_str().unwrap();
    request(
        &address,
        "POST",
        &format!("/approvals/{id}/respond"),
        r#"{"choice":"allow_once"}"#,
    )
    .unwrap();
    let input = loop {
        let current = request(&address, "GET", &format!("/turns/{turn}/requests"), "").unwrap();
        if let Some(value) = current["inputs"].as_array().and_then(|v| v.first()) {
            break value.clone();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(40));
    };
    assert_eq!(input["kind"], "saved_login");
    let id = input["requestId"].as_str().unwrap();
    assert_eq!(
        request(&address, "GET", &format!("/turns/{turn}/requests"), "").unwrap()["inputs"][0]
            ["requestId"],
        id
    );
    let answer = request(
        &address,
        "POST",
        &format!("/input-requests/{id}/respond"),
        r#"{"label":"fixture","username":"fixture-user","password":"fixture-pass"}"#,
    )
    .unwrap();
    assert!(!answer.to_string().contains("fixture-pass"));
    let complete = wait_state(&address, &turn, "completed");
    assert!(!complete.to_string().contains("fixture-user"));
    assert!(!complete.to_string().contains("fixture-pass"));
    assert_eq!(received.load(Ordering::SeqCst), 1);
    let vault = std::fs::read(data.join("saved-logins/vault.age")).unwrap();
    assert!(!vault.windows(12).any(|w| w == b"fixture-pass"));
    let second=request(&address,"POST","/saved-logins",&json!({"siteUrl":format!("http://{site_address}/login"),"label":"second","username":"second-user","password":"second-pass"}).to_string()).unwrap();
    let second_id = second["login"]["id"].as_str().unwrap();
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        &json!({"text":format!("login fixture http://{site_address}/login")}).to_string(),
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    let deadline = Instant::now() + Duration::from_secs(30);
    let approval = loop {
        let v = request(&address, "GET", &format!("/turns/{turn}/requests"), "").unwrap();
        if !v["approval"].is_null() {
            break v["approval"].clone();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(40));
    };
    let approval_id = approval["requestId"].as_str().unwrap();
    request(
        &address,
        "POST",
        &format!("/approvals/{approval_id}/respond"),
        r#"{"choice":"allow_once"}"#,
    )
    .unwrap();
    let selection = loop {
        let v = request(&address, "GET", &format!("/turns/{turn}/requests"), "").unwrap();
        if let Some(item) = v["inputs"].as_array().and_then(|items| items.first()) {
            break item.clone();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(40));
    };
    assert_eq!(selection["kind"], "select_saved_login");
    assert_eq!(selection["choices"].as_array().unwrap().len(), 2);
    assert!(!selection.to_string().contains("second-pass"));
    let selection_id = selection["requestId"].as_str().unwrap();
    assert!(request(
        &address,
        "POST",
        &format!("/input-requests/{selection_id}/respond"),
        r#"{"selectionId":"not-offered"}"#
    )
    .is_none());
    assert_eq!(
        request(&address, "GET", &format!("/turns/{turn}/requests"), "").unwrap()["inputs"][0]
            ["requestId"],
        selection_id
    );
    request(
        &address,
        "POST",
        &format!("/input-requests/{selection_id}/respond"),
        &json!({"selectionId":second_id}).to_string(),
    )
    .unwrap();
    let complete = wait_state(&address, &turn, "completed");
    assert!(!complete.to_string().contains("second-pass"));
    assert_eq!(received.load(Ordering::SeqCst), 2);
    stop.store(true, Ordering::SeqCst);
    site_thread.join().unwrap();
}

#[test]
fn new_device_login_supersedes_old_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = temp.path().join("data");
    let _server = launch(&data, &cli, &address);
    ready(&address);
    let first = login_request(&address, "POST", "/providers/codex/login", None).unwrap()
        ["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let v = login_request(&address, "GET", "/providers/codex/login", Some(&first)).unwrap();
        if v["state"] == "waiting" {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let first_marker = std::fs::read_dir(data.join("codex"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .find(|name| name.starts_with("fixture-login-start-"))
        .unwrap();
    let first_pid = first_marker
        .strip_prefix("fixture-login-start-")
        .unwrap()
        .to_owned();
    let second = login_request(&address, "POST", "/providers/codex/login", None).unwrap()
        ["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(first, second);
    assert!(login_request(&address, "GET", "/providers/codex/login", Some(&first)).is_none());
    assert!(login_request(
        &address,
        "POST",
        "/providers/codex/login/cancel",
        Some(&first)
    )
    .is_none());
    let third = login_request(&address, "POST", "/providers/codex/login", None).unwrap()
        ["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(second, third);
    assert!(login_request(&address, "GET", "/providers/codex/login", Some(&second)).is_none());
    let mut saw_code = false;
    loop {
        let v = login_request(&address, "GET", "/providers/codex/login", Some(&third)).unwrap();
        if v["state"] == "waiting" {
            saw_code = true;
            assert_eq!(v["code"], "TEST-1234");
        }
        if v["state"] == "connected" {
            assert!(v["code"].is_null());
            break;
        }
        assert!(
            v["state"] == "starting" || v["state"] == "waiting",
            "unexpected {v}"
        );
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(saw_code);
    assert!(
        !data
            .join("codex")
            .join(format!("fixture-login-finish-{first_pid}"))
            .exists(),
        "superseded CLI finished instead of stopping"
    );
    let finishes = std::fs::read_dir(data.join("codex"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("fixture-login-finish-")
        })
        .count();
    assert_eq!(finishes, 1);
}

#[cfg(unix)]
#[test]
fn provider_status_uses_fixed_states_for_unavailable_cli() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let blocked = temp.path().join("not-executable");
    std::fs::write(&blocked, "fixture").unwrap();
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o600)).unwrap();
    for (index, cli, want) in [
        (1, blocked, "cli_unavailable"),
        (2, temp.path().join("missing-cli"), "cli_unavailable"),
    ] {
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap().to_string();
        drop(socket);
        let _server = launch(&temp.path().join(format!("data-{index}")), &cli, &address);
        ready(&address);
        let providers = login_request(&address, "GET", "/providers", None).unwrap();
        assert_eq!(providers["providers"][0]["state"], want);
        let models = login_request(&address, "GET", "/providers/models", None).unwrap();
        assert_eq!(models["providers"][0]["state"], want);
        assert_eq!(models["providers"][0]["models"], json!([]));
        let absent_vault = if cfg!(target_os = "linux") {
            "storage_unavailable"
        } else {
            "not_supported"
        };
        assert_eq!(providers["providers"][1]["state"], absent_vault);
        assert_eq!(providers["providers"][2]["state"], absent_vault);
    }
}

#[test]
fn maintenance_drains_inflight_turn_and_survives_restart() {
    let temp = tempfile::tempdir().unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let data = temp.path().join("data");
    let cli = Path::new(env!("CARGO_BIN_EXE_fake-codex"));
    let server = launch(&data, cli, &address);
    ready(&address);
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        r#"{"text":"approval fixture"}"#,
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    let deadline = Instant::now() + Duration::from_secs(10);
    let approval = loop {
        if let Some(a) = request(&address, "GET", &format!("/turns/{turn}/approval"), "")
            .and_then(|v| v.get("approval").cloned())
            .filter(|v| !v.is_null())
        {
            break a;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(30));
    };
    assert_eq!(
        request(&address, "POST", "/maintenance", "{}").unwrap()["maintenance"],
        true
    );
    assert!(raw_post(&address, "/chats").contains(" 503 "));
    let state = request(&address, "GET", "/activity", "").unwrap();
    assert_eq!(state["maintenance"], true);
    assert_eq!(state["quiescent"], false);
    assert_eq!(state["shutdownReady"], false);
    request(
        &address,
        "POST",
        &format!(
            "/approvals/{}/respond",
            approval["requestId"].as_str().unwrap()
        ),
        r#"{"choice":"allow_once"}"#,
    )
    .unwrap();
    wait_state(&address, &turn, "completed");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if request(&address, "GET", "/activity", "").unwrap()["quiescent"] == true {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(30));
    }
    drop(server);
    let _server = launch(&data, cli, &address);
    ready(&address);
    assert!(raw_post(&address, "/chats").contains(" 503 "));
    assert_eq!(
        request(&address, "POST", "/maintenance/release", "{}").unwrap()["maintenance"],
        false
    );
    assert!(request(&address, "POST", "/chats", "{}").is_some());
}

#[test]
fn approval_timeout_emits_closed_while_turn_continues() {
    let temp = tempfile::tempdir().unwrap();
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let binary = std::env::var_os("MOMSO_TEST_SERVER_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_momso-server").into());
    let _server = Server(
        Command::new(binary)
            .env("MOMSO_SERVER_DATA_DIR", temp.path().join("data"))
            .env("MOMSO_SERVER_TOKEN", "test-only-token")
            .env("MOMSO_SERVER_LISTEN", &address)
            .env("MOMSO_CODEX_PATH", cli)
            .env("MOMSO_TEST_APPROVAL_TIMEOUT_MS", "1200")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&address);
    let chat = request(&address, "POST", "/chats", "{}").unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = request(
        &address,
        "POST",
        &format!("/chats/{chat}/turns"),
        r#"{"text":"approval fixture"}"#,
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(raw_get(&address, "/activity").contains(" 401 "));
    let event_address = address.clone();
    let event_turn = turn.clone();
    let event_thread = std::thread::spawn(move || read_events(&event_address, &event_turn));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let work = request(&address, "GET", "/activity", "").unwrap();
        if work["pendingApprovals"] == 1 {
            assert_eq!(work["busy"], true);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let events = event_thread.join().unwrap();
    assert!(events.contains("event: agent-approval-request"));
    assert!(events.contains("event: agent-approval-closed"));
    assert!(events.contains("\"reason\":\"timeout\""));
    assert_eq!(wait_state(&address, &turn, "completed")["text"], "거절됨");
    assert!(
        request(&address, "GET", &format!("/turns/{turn}/approval"), "").unwrap()["approval"]
            .is_null()
    );
}

#[cfg(unix)]
#[test]
fn codex_model_catalog_and_selected_model_reach_cli() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    drop(listener);
    let _server = launch(&data, &cli, &address);
    ready(&address);
    std::fs::create_dir_all(data.join("codex")).unwrap();
    std::fs::write(data.join("codex/auth.json"), "fixture").unwrap();
    let mut anonymous = TcpStream::connect(&address).unwrap();
    write!(
        anonymous,
        "GET /providers/models HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut rejected = String::new();
    anonymous.read_to_string(&mut rejected).unwrap();
    assert!(rejected.starts_with("HTTP/1.1 401"));
    let models = request(&address, "GET", "/providers/models", "").unwrap();
    assert_eq!(models["providers"][0]["models"][0]["id"], "gpt-6.1-sol");
    assert_eq!(
        models["providers"][0]["models"][0]["fast"]["state"],
        "unknown"
    );
    assert_eq!(
        models["providers"][0]["models"][0]["efforts"][0]["id"],
        "high"
    );
    assert_eq!(
        models["providers"][0]["models"][0]["efforts"][1]["id"],
        "ultra"
    );
    let chat = request(&address, "POST", "/chats", r#"{"title":"fixture"}"#).unwrap()["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = format!("/chats/{chat}/turns");
    let turn = request(
        &address,
        "POST",
        &path,
        r#"{"text":"model fixture","provider":"codex","model":"gpt-6.1-sol","effort":"ultra","fast":true}"#,
    )
    .unwrap()["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let result = request(&address, "GET", &format!("/turns/{turn}"), "").unwrap();
        if result["state"] != "running" {
            assert_eq!(result["state"], "completed");
            assert_eq!(result["fastState"], "requested_unverified");
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(30));
    }
    let failed = request(&address, "POST", &path, r#"{"text":"fast unavailable fixture","provider":"codex","model":"gpt-6.1-sol","fast":true}"#).unwrap()["turnId"].as_str().unwrap().to_owned();
    let failure = wait_state(&address, &failed, "failed");
    assert_eq!(failure["error"], "provider_failed");
    assert_eq!(failure["fastState"], "requested_unverified");
}
