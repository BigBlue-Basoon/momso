//! Independently scheduled GitHub and encrypted object backups.
use super::*;
use momso_server_providers::{vault, Provider};
use std::process::{Command as ProcessCommand, Stdio};
use tokio::sync::Notify;

fn last_success_path(app: &App, object: bool) -> PathBuf {
    app.data.join(if object {
        "object-backup-last-success"
    } else {
        "github-backup-last-success"
    })
}

fn load_last_success(app: &App, object: bool) -> Option<i64> {
    chat_backup::read_regular(&last_success_path(app, object))
        .ok()
        .flatten()
        .and_then(|bytes| std::str::from_utf8(&bytes).ok()?.parse().ok())
}

fn save_last_success(app: &App, object: bool, millis: i64) -> Result<(), &'static str> {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(&app.data).map_err(|_| "storage_unavailable")?;
    write!(file, "{millis}").map_err(|_| "storage_unavailable")?;
    file.as_file()
        .sync_all()
        .map_err(|_| "storage_unavailable")?;
    file.persist(last_success_path(app, object))
        .map_err(|_| "storage_unavailable")?;
    std::fs::File::open(&app.data)
        .and_then(|dir| dir.sync_all())
        .map_err(|_| "storage_unavailable")
}

#[derive(Clone, Copy)]
pub struct Status {
    pub active: bool,
    pub state: &'static str,
    pub reason: Option<&'static str>,
    pub synced_generation: Option<u64>,
    pub last_success_at: Option<i64>,
}

pub struct Control {
    status: Mutex<Status>,
    object_status: Mutex<Status>,
    wake: Notify,
    object_wake: Notify,
    pub(crate) selection: Mutex<()>,
}

impl Control {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            status: Mutex::new(Status {
                active: false,
                state: "not_configured",
                reason: None,
                synced_generation: None,
                last_success_at: None,
            }),
            object_status: Mutex::new(Status {
                active: false,
                state: "not_configured",
                reason: None,
                synced_generation: None,
                last_success_at: None,
            }),
            wake: Notify::new(),
            object_wake: Notify::new(),
            selection: Mutex::new(()),
        })
    }

    pub fn status(&self) -> Result<Status, ApiError> {
        self.status
            .lock()
            .map(|value| *value)
            .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))
    }

    pub fn wake(&self) {
        self.wake.notify_one();
        self.object_wake.notify_one();
    }
    pub fn object_status(&self) -> Result<Status, ApiError> {
        self.object_status
            .lock()
            .map(|value| *value)
            .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))
    }
}

fn checkpoint_idle(app: &App) -> Result<bool, &'static str> {
    let (_, admitted, _) = maintenance::observe(app).map_err(|_| "unavailable")?;
    match std::fs::symlink_metadata(app.data.join("chat-restore.pending")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        _ => return Ok(false),
    }
    if admitted != 0
        || app.writes.load(Ordering::SeqCst) != 0
        || app.github_logins.load(Ordering::SeqCst) != 0
        || !app.active.lock().map_err(|_| "unavailable")?.is_empty()
        || human_prompt::count(&app.human_inputs).map_err(|_| "unavailable")? != 0
        || app
            .login
            .lock()
            .map_err(|_| "unavailable")?
            .as_ref()
            .is_some_and(|login| matches!(login.state, "starting" | "waiting"))
    {
        return Ok(false);
    }
    let pending: bool = db(app)
        .map_err(|_| "storage_error")?
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM approvals WHERE state='pending')",
            [],
            |row| row.get(0),
        )
        .map_err(|_| "storage_error")?;
    Ok(!pending)
}

fn object_backup(app: &App) -> Result<bool, &'static str> {
    use std::io::BufRead;
    let Some(checkpoint) = maintenance::begin_checkpoint(&app.maintenance)? else {
        return Ok(false);
    };
    if !checkpoint_idle(app)? {
        return Ok(false);
    }
    let Some(mut command) = agent_command(app, "backup")? else {
        return Ok(false);
    };
    command.stdin(Stdio::null()).stdout(Stdio::piped());
    let mut child = command.spawn().map_err(|_| "object_backup_unavailable")?;
    let stdout = child.stdout.take().ok_or("object_backup_unavailable")?;
    let mut reader = std::io::BufReader::new(stdout);
    let mut line = String::new();
    let first = reader
        .read_line(&mut line)
        .map_err(|_| "object_backup_failed")?;
    if first != 0 && line == "momso-backup-v3:snapshot-ready\n" {
        drop(checkpoint);
        line.clear();
        reader
            .read_line(&mut line)
            .map_err(|_| "object_backup_failed")?;
    } else {
        drop(checkpoint);
    }
    let status = child.wait().map_err(|_| "object_backup_failed")?;
    if !status.success()
        || !matches!(
            line.as_str(),
            "momso-backup-v3:uploaded\n" | "momso-backup-v3:unchanged\n"
        )
    {
        return Err("object_backup_failed");
    }
    Ok(true)
}

fn agent_command(app: &App, action: &str) -> Result<Option<ProcessCommand>, &'static str> {
    let Some(agent) = std::env::var_os("MOMSO_BACKUP_AGENT") else {
        return Ok(None);
    };
    let agent = std::path::PathBuf::from(agent)
        .canonicalize()
        .map_err(|_| "object_backup_unavailable")?;
    use std::os::unix::fs::MetadataExt;
    let trusted = agent.ancestors().enumerate().all(|(index, path)| {
        std::fs::metadata(path).is_ok_and(|meta| {
            (if index == 0 {
                meta.is_file()
            } else {
                meta.is_dir()
            }) && meta.uid() == 0
                && meta.mode() & 0o022 == 0
        })
    });
    if !trusted {
        #[cfg(all(debug_assertions, feature = "test-fixtures"))]
        if std::env::var_os("MOMSO_TEST_ALLOW_BACKUP_AGENT").is_none() {
            return Err("object_backup_unavailable");
        }
        #[cfg(not(all(debug_assertions, feature = "test-fixtures")))]
        return Err("object_backup_unavailable");
    }
    let mut command = ProcessCommand::new("/usr/bin/python3");
    command
        .arg(agent)
        .arg(action)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("MOMSO_SERVER_DATA_DIR", &app.data)
        .env("MOMSO_BACKUP_QUIESCENT", "1")
        .stderr(Stdio::null());
    if action == "backup" {
        command.env("MOMSO_BACKUP_SNAPSHOT_SIGNAL", "1");
    }
    for name in [
        "MOMSO_BACKUP_BUCKET",
        "MOMSO_BACKUP_KMS_KEY",
        "MOMSO_BACKUP_RUNTIME_DIR",
        "CREDENTIALS_DIRECTORY",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    Ok(Some(command))
}

pub(crate) fn restore_assets(app: &App, assets: &[String]) -> Result<(), &'static str> {
    use std::io::Write;
    let Some(mut command) = agent_command(app, "restore-assets")? else {
        return Err("attachments_restore_unavailable");
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|_| "attachments_restore_unavailable")?;
    let input = serde_json::to_vec(assets).map_err(|_| "attachments_restore_unavailable")?;
    if child
        .stdin
        .take()
        .ok_or("attachments_restore_unavailable")?
        .write_all(&input)
        .is_err()
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err("attachments_restore_unavailable");
    }
    let output = child
        .wait_with_output()
        .map_err(|_| "attachments_restore_unavailable")?;
    if !output.status.success() || output.stdout != b"momso-backup-v3:assets-restored\n" {
        return Err("attachments_restore_unavailable");
    }
    Ok(())
}

pub(crate) fn record_original_purge(app: &App, digest: &str) -> Result<(), &'static str> {
    purge_agent(
        app,
        digest,
        "record-purge",
        b"momso-backup-v3:purge-recorded\n",
    )
}

pub(crate) fn check_original_purge(app: &App, digest: &str) -> Result<(), &'static str> {
    purge_agent(
        app,
        digest,
        "check-purge",
        b"momso-backup-v3:purge-checked\n",
    )
}

fn purge_agent(app: &App, digest: &str, action: &str, expected: &[u8]) -> Result<(), &'static str> {
    use std::io::Write;
    let mut command = agent_command(app, action)?.ok_or("object_backup_unavailable")?;
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|_| "object_backup_unavailable")?;
    let input = serde_json::to_vec(&json!({"digest": digest})).map_err(|_| "storage_error")?;
    if child
        .stdin
        .take()
        .ok_or("object_backup_unavailable")?
        .write_all(&input)
        .is_err()
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err("object_backup_unavailable");
    }
    let output = child
        .wait_with_output()
        .map_err(|_| "object_backup_unavailable")?;
    if !output.status.success() || output.stdout != expected {
        return Err("object_backup_failed");
    }
    Ok(())
}

fn sync(app: &App) -> Result<Option<bool>, &'static str> {
    let _selection = app
        .backup
        .selection
        .lock()
        .map_err(|_| "backup_unavailable")?;
    let Some(repository) = github_bridge::read_repository(&app.data)? else {
        return Ok(None);
    };
    let client = github_bridge::handle(app).map_err(|_| "github_unavailable")?;
    let _vault_guard = app
        .vault_lock
        .lock()
        .map_err(|_| "credential_unavailable")?;
    let mut keys = Vec::new();
    for provider in [Provider::Claude, Provider::Gemini] {
        let store = vault::open(&app.data.join("provider-credentials"), provider)
            .map_err(|_| "credential_unavailable")?;
        if let Some(key) = vault::load(&store).map_err(|_| "credential_unavailable")? {
            keys.push(key);
        }
    }
    let codex_auth =
        chat_backup::read_regular(&app.data.join("codex/auth.json"))?.map(zeroize::Zeroizing::new);
    let codex_value: Option<Value> = codex_auth
        .as_ref()
        .map(|bytes| serde_json::from_slice(bytes).map_err(|_| "credential_unavailable"))
        .transpose()?;
    let mut known: Vec<&str> = keys.iter().map(|key| key.expose_to_transport()).collect();
    known.push(app.token.as_ref());
    if let Some(value) = &codex_value {
        for path in [
            "/OPENAI_API_KEY",
            "/tokens/id_token",
            "/tokens/access_token",
            "/tokens/refresh_token",
        ] {
            if let Some(secret) = value
                .pointer(path)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                known.push(secret);
            }
        }
    }
    let mut connection = db(app).map_err(|_| "storage_error")?;
    chat_backup::stage_chat_copies(
        &mut connection,
        &app.data.join("archive"),
        &app.data,
        &known,
    )?;
    let adopt_path = app.data.join("github-adopt.pending");
    let pending = chat_backup::read_regular(&adopt_path)?
        .map(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(|_| "restore_changed"))
        .transpose()?;
    let expected = match pending.as_ref() {
        Some(value)
            if value.as_object().is_some_and(|v| v.len() == 2)
                && value.get("repository").and_then(Value::as_str)
                    == Some(repository.name_with_owner.as_str()) =>
        {
            Some(
                value
                    .get("head")
                    .and_then(Value::as_str)
                    .ok_or("restore_changed")?,
            )
        }
        Some(_) => return Err("restore_changed"),
        None => None,
    };
    let archive = app.data.join("archive");
    if let Some(head) = expected {
        client.verify_restored_remote(&archive, &repository, head)?;
    }
    match client.sync_once_with_known(&archive, &repository, &known) {
        Err("diverged") if expected.is_some() => {
            client.merge_restored_history(
                &archive,
                &repository,
                expected.ok_or("restore_changed")?,
                &known,
            )?;
            client.sync_once_with_known(&archive, &repository, &known)?;
        }
        Err(reason) => return Err(reason),
        Ok(_) => (),
    }
    if pending.is_some() {
        std::fs::remove_file(&adopt_path).map_err(|_| "storage_unavailable")?;
        std::fs::File::open(&app.data)
            .and_then(|dir| dir.sync_all())
            .map_err(|_| "storage_unavailable")?;
    }
    Ok(Some(true))
}

pub fn start(app: App) {
    let object_app = app.clone();
    tokio::spawn(async move {
        let control = object_app.backup.clone();
        if let Ok(mut status) = control.object_status.lock() {
            status.last_success_at = load_last_success(&object_app, true);
        }
        loop {
            let current = maintenance::generation(&object_app.maintenance).ok();
            let current_is_saved = control.object_status.lock().is_ok_and(|status| {
                status.state == "saved" && current.is_some() && status.synced_generation == current
            });
            if current_is_saved {
                tokio::select! {
                    _ = control.object_wake.notified() => (),
                    _ = tokio::time::sleep(Duration::from_secs(30)) => (),
                }
                continue;
            }
            if std::env::var_os("MOMSO_BACKUP_AGENT").is_none() {
                if let Ok(mut status) = control.object_status.lock() {
                    status.active = false;
                    status.state = "not_configured";
                    status.reason = None;
                    status.synced_generation = None;
                }
            } else {
                let started_generation = maintenance::generation(&object_app.maintenance).ok();
                if let Ok(mut status) = control.object_status.lock() {
                    status.active = true;
                    status.state = "syncing";
                    status.reason = None;
                }
                let work = object_app.clone();
                let result = tokio::task::spawn_blocking(move || object_backup(&work))
                    .await
                    .unwrap_or(Err("object_backup_unavailable"));
                let finished_generation = maintenance::generation(&object_app.maintenance).ok();
                if let Ok(mut status) = control.object_status.lock() {
                    status.active = false;
                    match result {
                        Ok(true) => {
                            let now = chrono::Utc::now().timestamp_millis();
                            if save_last_success(&object_app, true, now).is_ok() {
                                status.last_success_at = Some(now);
                            }
                            if started_generation.is_some()
                                && started_generation == finished_generation
                            {
                                status.state = "saved";
                                status.reason = None;
                                status.synced_generation = finished_generation;
                            } else {
                                status.state = "pending";
                                status.reason = Some("changes_during_backup");
                                status.synced_generation = None;
                            }
                        }
                        Ok(_) => {
                            status.state = "pending";
                            status.reason = Some("work_in_progress");
                            status.synced_generation = None;
                        }
                        Err(reason) => {
                            status.state = "failed";
                            status.reason = Some(reason);
                            status.synced_generation = None;
                        }
                    }
                }
            }
            tokio::select! {
                _ = control.object_wake.notified() => (),
                _ = tokio::time::sleep(Duration::from_secs(30)) => (),
            }
        }
    });
    tokio::spawn(async move {
        let control = app.backup.clone();
        if let Ok(mut status) = control.status.lock() {
            status.last_success_at = load_last_success(&app, false);
        }
        loop {
            if matches!(github_bridge::read_repository(&app.data), Ok(None)) {
                if let Ok(mut status) = control.status.lock() {
                    status.active = false;
                    status.state = "not_configured";
                    status.reason = None;
                    status.synced_generation = None;
                }
                tokio::select! {
                    _ = control.wake.notified() => (),
                    _ = tokio::time::sleep(Duration::from_secs(30)) => (),
                }
                continue;
            }
            let started_generation = maintenance::generation(&app.maintenance).ok();
            {
                if let Ok(mut status) = control.status.lock() {
                    status.active = true;
                    status.state = "syncing";
                    status.reason = None;
                }
            }
            let work = app.clone();
            let result = tokio::task::spawn_blocking(move || sync(&work))
                .await
                .unwrap_or(Err("backup_unavailable"));
            let finished_generation = maintenance::generation(&app.maintenance).ok();
            let mut retry = false;
            if let Ok(mut status) = control.status.lock() {
                status.active = false;
                match result {
                    Ok(Some(complete))
                        if started_generation.is_some()
                            && started_generation == finished_generation =>
                    {
                        status.state = if complete { "saved" } else { "saved_partial" };
                        status.reason = if complete {
                            None
                        } else {
                            Some("object_backup_not_integrated")
                        };
                        status.synced_generation = finished_generation;
                        let now = chrono::Utc::now().timestamp_millis();
                        if save_last_success(&app, false, now).is_ok() {
                            status.last_success_at = Some(now);
                        }
                    }
                    Ok(Some(_)) => {
                        let now = chrono::Utc::now().timestamp_millis();
                        if save_last_success(&app, false, now).is_ok() {
                            status.last_success_at = Some(now);
                        }
                        status.state = "pending";
                        status.reason = Some("changes_during_backup");
                        status.synced_generation = None;
                        retry = true;
                    }
                    Ok(None) => {
                        status.state = "not_configured";
                        status.reason = None;
                        status.synced_generation = None;
                    }
                    Err(reason) => {
                        status.state = "failed";
                        status.reason = Some(reason);
                        status.synced_generation = None;
                    }
                }
            }
            if retry {
                control.wake.notify_one();
            }
            tokio::select! {
                _ = control.wake.notified() => (),
                _ = tokio::time::sleep(Duration::from_secs(30)) => (),
            }
        }
    });
}
