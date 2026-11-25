use crate::{archive_remote, chat_archive_tracking, storage, AppState};
use serde::Serialize;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{Manager, State};

const QUIET: Duration = Duration::from_secs(120);
const INTERVAL: Duration = Duration::from_secs(30);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Default)]
struct Tracker {
    last: Option<u64>,
    changed_at: Option<Instant>,
    retry_at: Option<Instant>,
    pending: bool,
    running: bool,
    failures: u32,
    reason: Option<String>,
    loaded: bool,
}

#[derive(Default)]
pub(crate) struct AutoBackup(Mutex<Tracker>);

struct InFlight<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
    armed: bool,
}

impl<R: tauri::Runtime> Drop for InFlight<R> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(automatic) = self.app.try_state::<AutoBackup>() {
            if let Ok(mut tracker) = automatic.0.try_lock() {
                complete(&mut tracker, Err("interrupted"), Instant::now());
            }
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutoBackupStatus {
    enabled: bool,
    state: &'static str,
    consecutive_failures: u32,
    reason: Option<String>,
    retry_in_seconds: Option<u64>,
}

pub(crate) fn enabled(db: &Path) -> Result<bool, &'static str> {
    use rusqlite::OptionalExtension;
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "local_state_unavailable")?;
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM app_runtime_meta WHERE key='archive_auto_backup_enabled'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| "local_state_unavailable")?;
    match value.as_deref() {
        None | Some("1") => Ok(true),
        Some("0") => Ok(false),
        _ => Err("local_state_unavailable"),
    }
}

fn load_failures(db: &Path, tracker: &mut Tracker) -> Result<(), &'static str> {
    use rusqlite::OptionalExtension;
    if tracker.loaded {
        return Ok(());
    }
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "local_state_unavailable")?;
    let count: Option<String> = conn
        .query_row(
            "SELECT value FROM app_runtime_meta WHERE key='archive_auto_backup_failures'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| "local_state_unavailable")?;
    let reason: Option<String> = conn
        .query_row(
            "SELECT value FROM app_runtime_meta WHERE key='archive_auto_backup_reason'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| "local_state_unavailable")?;
    tracker.failures = count
        .map(|value| value.parse::<u32>().map_err(|_| "local_state_unavailable"))
        .transpose()?
        .unwrap_or(0);
    tracker.reason = reason.filter(|value| !value.is_empty());
    tracker.loaded = true;
    Ok(())
}

fn persist_failures(db: &Path, tracker: &Tracker) -> Result<(), &'static str> {
    let mut conn = storage::open_writable_connection(db).map_err(|_| "local_state_unavailable")?;
    let tx = conn.transaction().map_err(|_| "local_state_unavailable")?;
    for (key, value) in [
        ("archive_auto_backup_failures", tracker.failures.to_string()),
        (
            "archive_auto_backup_reason",
            tracker.reason.clone().unwrap_or_default(),
        ),
    ] {
        tx.execute("INSERT INTO app_runtime_meta(key,value,updated_at) VALUES(?1,?2,CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=CURRENT_TIMESTAMP", (key, value)).map_err(|_| "local_state_unavailable")?;
    }
    tx.commit().map_err(|_| "local_state_unavailable")
}

fn status(enabled: bool, tracker: &Tracker) -> AutoBackupStatus {
    AutoBackupStatus {
        enabled,
        state: if !enabled {
            "disabled"
        } else if tracker.running {
            "running"
        } else if tracker.failures > 0 {
            "retry_pending"
        } else if tracker.pending {
            "pending"
        } else {
            "idle"
        },
        consecutive_failures: tracker.failures,
        reason: tracker.reason.clone(),
        retry_in_seconds: tracker
            .retry_at
            .map(|at| at.saturating_duration_since(Instant::now()).as_secs()),
    }
}

#[tauri::command]
pub(crate) fn archive_auto_backup_get(
    state: State<'_, AppState>,
    automatic: State<'_, AutoBackup>,
) -> Result<AutoBackupStatus, &'static str> {
    let enabled = enabled(&state.paths.database_path)?;
    let mut tracker = automatic.0.lock().map_err(|_| "unavailable")?;
    load_failures(&state.paths.database_path, &mut tracker)?;
    Ok(status(enabled, &tracker))
}

#[tauri::command]
pub(crate) fn archive_auto_backup_set(
    state: State<'_, AppState>,
    automatic: State<'_, AutoBackup>,
    enabled: bool,
) -> Result<AutoBackupStatus, &'static str> {
    let _remote = archive_remote::try_lock_remote_for_auto()?;
    let conn = storage::open_writable_connection(&state.paths.database_path)
        .map_err(|_| "local_state_unavailable")?;
    conn.execute("INSERT INTO app_runtime_meta(key,value,updated_at) VALUES('archive_auto_backup_enabled',?1,CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=CURRENT_TIMESTAMP", [if enabled { "1" } else { "0" }]).map_err(|_| "local_state_unavailable")?;
    let mut tracker = automatic.0.lock().map_err(|_| "unavailable")?;
    load_failures(&state.paths.database_path, &mut tracker)?;
    if enabled {
        tracker.pending = true;
        tracker.changed_at = Some(Instant::now());
        tracker.retry_at = None;
    }
    Ok(status(enabled, &tracker))
}

fn fingerprint_path(
    base: &Path,
    relative: &Path,
    hasher: &mut DefaultHasher,
) -> Result<(), &'static str> {
    let path = base.join(relative);
    let mut ancestor = base.to_path_buf();
    for part in relative.components() {
        ancestor.push(part);
        match std::fs::symlink_metadata(&ancestor) {
            Ok(metadata) if crate::archive_location::is_link_or_reparse(&metadata) => {
                return Err("local_state_unavailable")
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err("local_state_unavailable"),
        }
    }
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("local_state_unavailable"),
    };
    if crate::archive_location::is_link_or_reparse(&metadata) {
        return Err("local_state_unavailable");
    }
    relative.hash(hasher);
    metadata.len().hash(hasher);
    metadata
        .modified()
        .map_err(|_| "local_state_unavailable")?
        .hash(hasher);
    if metadata.is_dir() {
        let mut children: Vec<PathBuf> = std::fs::read_dir(&path)
            .map_err(|_| "local_state_unavailable")?
            .map(|entry| {
                entry
                    .map(|e| relative.join(e.file_name()))
                    .map_err(|_| "local_state_unavailable")
            })
            .collect::<Result<_, _>>()?;
        children.sort();
        for child in children {
            fingerprint_path(base, &child, hasher)?;
        }
    }
    Ok(())
}

fn fingerprint(root: &Path, db: &Path) -> Result<u64, &'static str> {
    let mut hasher = DefaultHasher::new();
    for name in [
        "zettels",
        "dictionary",
        "skills",
        ".agents/skills",
        ".claude/skills",
        "receipts",
        "chats",
        ".git/HEAD",
        ".git/refs/heads",
        "AGENTS.md",
        "CLAUDE.md",
        "GEMINI.md",
        "profile.md",
        ".gitignore",
    ] {
        fingerprint_path(root, Path::new(name), &mut hasher)?;
    }
    for path in crate::archive::additional_files(root).map_err(|_| "local_state_unavailable")? {
        fingerprint_path(root, Path::new(&path), &mut hasher)?;
    }
    chat_archive_tracking::changes(db)?.hash(&mut hasher);
    Ok(hasher.finish())
}

fn observe(tracker: &mut Tracker, value: u64, now: Instant) {
    if tracker.last != Some(value) {
        tracker.last = Some(value);
        tracker.changed_at = Some(now);
        tracker.pending = true;
    }
}

fn ready(tracker: &Tracker, now: Instant, closing: bool) -> bool {
    tracker.pending
        && !tracker.running
        && (closing
            || tracker
                .changed_at
                .is_some_and(|at| now.saturating_duration_since(at) >= QUIET))
        && (closing || tracker.retry_at.is_none_or(|at| now >= at))
}

fn complete(tracker: &mut Tracker, result: Result<(), &'static str>, now: Instant) {
    tracker.running = false;
    match result {
        Ok(()) => {
            tracker.failures = 0;
            tracker.reason = None;
            tracker.retry_at = None;
        }
        Err(reason) => {
            tracker.pending = true;
            tracker.failures = tracker.failures.saturating_add(1);
            tracker.reason = Some(reason.to_owned());
            let delay = 30_u64.saturating_mul(1_u64 << tracker.failures.min(6));
            tracker.retry_at = Some(now + Duration::from_secs(delay.min(1800)));
        }
    }
}

pub(crate) async fn attempt<R: tauri::Runtime>(app: tauri::AppHandle<R>, closing: bool) {
    if attempt_once(app.clone(), closing).await {
        // A transfer can itself save chat snapshots and create Git commits.
        // Recheck them promptly rather than treating them as a fresh 2-minute
        // user edit. A real concurrent edit is still saved and uploaded.
        let _ = attempt_once(app, closing).await;
    }
}

async fn attempt_once<R: tauri::Runtime>(app: tauri::AppHandle<R>, closing: bool) -> bool {
    let state = app.state::<AppState>();
    if !enabled(&state.paths.database_path).unwrap_or(false) {
        return false;
    }
    if !closing && !state.chat_active.lock().await.is_empty() {
        return false;
    }
    let root = state.paths.archive.current.read().await.clone();
    let db = state.paths.database_path.clone();
    let auto = app.state::<AutoBackup>();
    let observed = tauri::async_runtime::spawn_blocking(move || fingerprint(&root, &db)).await;
    let value = match observed {
        Ok(Ok(value)) => value,
        _ => {
            if let Ok(mut tracker) = auto.0.lock() {
                let _ = load_failures(&state.paths.database_path, &mut tracker);
                if tracker.retry_at.is_none_or(|at| Instant::now() >= at) {
                    complete(&mut tracker, Err("local_state_unavailable"), Instant::now());
                    let _ = persist_failures(&state.paths.database_path, &tracker);
                }
            }
            return false;
        }
    };
    {
        let Ok(mut tracker) = auto.0.lock() else {
            return false;
        };
        if load_failures(&state.paths.database_path, &mut tracker).is_err() {
            return false;
        }
        observe(&mut tracker, value, Instant::now());
        if !ready(&tracker, Instant::now(), closing) {
            return false;
        }
        tracker.running = true;
        tracker.pending = false;
    }
    let mut guard = InFlight {
        app: app.clone(),
        armed: true,
    };
    let git_result = archive_remote::run_transfer(app.clone(), state.clone(), "auto_push")
        .await
        .automatic_result();
    let result = if git_result.is_ok() {
        crate::object_store_auto_upload::schedule_pair(&app);
        crate::paired_backup::capture_git_only(app.clone())
            .await
            .map(|_| ())
    } else {
        git_result
    };
    let after = if result.is_ok() {
        let root = state.paths.archive.current.read().await.clone();
        let db = state.paths.database_path.clone();
        tauri::async_runtime::spawn_blocking(move || fingerprint(&root, &db))
            .await
            .ok()
            .and_then(Result::ok)
    } else {
        None
    };
    let mut recheck = false;
    if let Ok(mut tracker) = auto.0.lock() {
        complete(&mut tracker, result, Instant::now());
        if let Some(after) = after.filter(|after| tracker.last != Some(*after)) {
            tracker.last = Some(after);
            tracker.changed_at = Instant::now().checked_sub(QUIET);
            tracker.pending = true;
            recheck = true;
        }
        if persist_failures(&app.state::<AppState>().paths.database_path, &tracker).is_err() {
            tracker.reason = Some("local_state_unavailable".to_owned());
            tracker.pending = true;
            recheck = false;
        }
    };
    guard.armed = false;
    recheck
}

pub(crate) fn start(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            attempt(app.clone(), false).await;
            tokio::time::sleep(INTERVAL).await;
        }
    });
}

pub(crate) async fn flush<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    loop {
        let running = app
            .state::<AutoBackup>()
            .0
            .lock()
            .map(|state| state.running)
            .unwrap_or(false);
        if !running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    attempt(app, true).await;
}

pub(crate) enum CloseResult {
    Finished(Option<&'static str>),
    TimedOut,
}

pub(crate) async fn close_backup<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    paths: crate::runtime::RuntimePaths,
    resources: PathBuf,
) -> CloseResult {
    close_backup_with_timeout(app, paths, resources, CLOSE_TIMEOUT).await
}

async fn close_backup_with_timeout<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    paths: crate::runtime::RuntimePaths,
    resources: PathBuf,
    timeout: Duration,
) -> CloseResult {
    let work = async move {
        let database = paths.database_path.clone();
        let root = paths.archive.current.clone().read_owned().await;
        let result = tauri::async_runtime::spawn_blocking(move || {
            crate::chat_archive_store::save_pending(&root, &resources, &paths)
        })
        .await
        .unwrap_or(Err("unavailable"));
        flush(app.clone()).await;
        let git_only_error = if enabled(&database).unwrap_or(false) {
            crate::paired_backup::capture_git_only(app).await.err()
        } else {
            None
        };
        result.err().or(git_only_error)
    };
    match tokio::time::timeout(timeout, work).await {
        Ok(error) => CloseResult::Finished(error),
        Err(_) => CloseResult::TimedOut,
    }
}

pub(crate) async fn close_all<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    paths: crate::runtime::RuntimePaths,
    resources: PathBuf,
) -> Result<(CloseResult, bool), tokio::time::error::Elapsed> {
    tokio::time::timeout(CLOSE_TIMEOUT, async {
        // Upload files in parallel; retry pair creation after Git has finished.
        let (git, first_object) = tokio::join!(
            close_backup(app.clone(), paths, resources),
            crate::object_store_auto_upload::close_upload(app.clone())
        );
        let object = if matches!(&git, CloseResult::Finished(None)) {
            crate::object_store_auto_upload::close_upload(app).await
        } else {
            first_object
        };
        (git, object)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    // These Git-specific fixtures do not exercise object uploads or the Git-only
    // recovery-key path. The local-only override keeps them independent of any
    // credentials installed on the developer's PC.
    fn register_object_fixture(root: &Path) {
        crate::object_store::register_test_config(
            root,
            crate::object_store::Config {
                provider: "s3_compatible".into(),
                endpoint: "http://127.0.0.1:1".into(),
                region: "auto".into(),
                bucket: "fixture".into(),
                credential_id: uuid::Uuid::new_v4().to_string(),
                last_backup_at: None,
                write_safety: Some(crate::object_store::WriteSafety::Conditional),
            },
        );
    }

    #[test]
    fn extra_files_survive_save_automatic_backup_and_first_receive() {
        use crate::{
            archive,
            archive_location::ArchiveLocation,
            archive_remote::{self, TestRemote},
            runtime::RuntimePaths,
        };
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        register_object_fixture(base);
        let root = base.join("old");
        let bare = base.join("remote.git");
        let db = base.join("old.db");
        let backups = base.join("backups");
        let git = crate::process::resolve_tool("git.exe").unwrap();
        archive_remote::git(&git, base, &["init", "--bare", bare.to_str().unwrap()]).unwrap();
        storage::initialize(&db, &backups, "fixture").unwrap();
        let location = ArchiveLocation::load(db.clone(), base, vec![]).unwrap();
        *location.current.try_write().unwrap() = root.clone();
        archive::initialize(&root, base).unwrap();
        let paths = RuntimePaths {
            data_root: base.to_path_buf(),
            database_path: db.clone(),
            objects_path: root.join("objects"),
            exports_path: base.join("exports"),
            backups_path: backups,
            logs_path: base.join("logs"),
            archive: location,
            archive_lease: None,
        };
        let app = tauri::test::mock_builder()
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
            .manage(AutoBackup::default())
            .manage(TestRemote {
                tool: git.clone(),
                target: bare.to_string_lossy().into_owned(),
            })
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let document = crate::archive_zet::create(
            &root,
            &db,
            crate::archive_zet::ZetCreate {
                title: "fixture".into(),
                summary: "fixture".into(),
                kind: "note".into(),
                body: "body".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        std::fs::create_dir_all(root.join("zettels/manual")).unwrap();
        std::fs::write(
            root.join("zettels/manual/수업.md"),
            b"unmanaged\r\nlesson\n",
        )
        .unwrap();
        std::fs::write(root.join("수업 사진.png"), [0, 1, 255, 10]).unwrap();
        std::fs::write(root.join("zettels/loose.md"), b"loose text").unwrap();
        std::fs::write(root.join("objects/private-original"), b"private original").unwrap();
        std::fs::write(root.join(".gitignore"), "objects/\n.momso/outputs/\nlocal-private.txt\nzettels/manual/secret*.md\n!zettels/manual/secret-allowed.md\n").unwrap();
        std::fs::write(root.join("local-private.txt"), b"ignored private data").unwrap();
        std::fs::write(
            root.join("zettels/manual/secret-hidden.md"),
            b"ignored nested data",
        )
        .unwrap();
        std::fs::write(
            root.join("zettels/manual/secret-allowed.md"),
            b"allowed by negation",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".momso/outputs/test/turn")).unwrap();
        std::fs::write(
            root.join(".momso/outputs/test/turn/output.png"),
            b"ignored output",
        )
        .unwrap();
        archive::initialize(&root, base).unwrap();
        assert!(std::fs::read_to_string(root.join(".gitignore"))
            .unwrap()
            .contains("local-private.txt"));
        archive_remote::git(&git, &root, &["add", "-f", "--", "local-private.txt"]).unwrap();
        assert_eq!(archive::additional_files(&root).unwrap().len(), 6);
        archive::save(&root, base, &db).unwrap();
        assert!(crate::archive_zet::folders(&root).unwrap().is_empty());
        assert_eq!(crate::archive_zet::list(&db, None, None).unwrap().len(), 1);
        while !crate::chat_redaction_recheck::step(&root, &app.state::<AppState>().paths)
            .unwrap()
            .available
        {}
        tauri::async_runtime::block_on(attempt(app.handle().clone(), false));
        app.state::<AutoBackup>().0.lock().unwrap().changed_at = Some(Instant::now() - QUIET);
        tauri::async_runtime::block_on(attempt(app.handle().clone(), false));
        let status = archive_auto_backup_get(app.state(), app.state()).unwrap();
        assert_eq!(status.state, "idle", "reason: {:?}", status.reason);
        std::fs::write(
            root.join("zettels/manual/수업.md"),
            b"new unmanaged bytes\r\n",
        )
        .unwrap();
        tauri::async_runtime::block_on(attempt(app.handle().clone(), false));
        app.state::<AutoBackup>().0.lock().unwrap().changed_at = Some(Instant::now() - QUIET);
        tauri::async_runtime::block_on(attempt(app.handle().clone(), false));
        assert_eq!(
            archive_auto_backup_get(app.state(), app.state())
                .unwrap()
                .state,
            "idle"
        );
        let fresh = base.join("fresh");
        let fresh_db = base.join("fresh.db");
        storage::initialize(&fresh_db, &base.join("fresh-backups"), "fixture").unwrap();
        archive::initialize(&fresh, base).unwrap();
        archive_remote::connect_local(&git, &fresh, bare.to_str().unwrap()).unwrap();
        archive_remote::receive_connected(&git, &fresh, &fresh_db, bare.to_str().unwrap(), "main")
            .unwrap();
        assert_eq!(
            std::fs::read(fresh.join("zettels/manual/수업.md")).unwrap(),
            b"new unmanaged bytes\r\n"
        );
        assert_eq!(
            std::fs::read(fresh.join("zettels/loose.md")).unwrap(),
            b"loose text"
        );
        assert_eq!(
            std::fs::read(fresh.join("수업 사진.png")).unwrap(),
            [0, 1, 255, 10]
        );
        assert_eq!(
            std::fs::read(fresh.join("zettels/manual/secret-allowed.md")).unwrap(),
            b"allowed by negation"
        );
        assert!(!fresh.join("local-private.txt").exists());
        assert!(!fresh.join("zettels/manual/secret-hidden.md").exists());
        assert!(!fresh.join(".momso/outputs/test/turn/output.png").exists());
        assert_eq!(
            std::fs::read(root.join("local-private.txt")).unwrap(),
            b"ignored private data"
        );
        assert!(fresh
            .join("zettels")
            .join(format!("{document}.md"))
            .is_file());
        assert_eq!(
            crate::archive_zet::list(&fresh_db, None, None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(archive::additional_files(&fresh).unwrap().len(), 4);
        assert!(!fresh.join("objects/private-original").exists());
    }

    #[test]
    fn batches_changes_retries_and_flushes_on_close() {
        let now = Instant::now();
        let mut tracker = Tracker::default();
        observe(&mut tracker, 1, now);
        observe(&mut tracker, 2, now + Duration::from_secs(100));
        assert!(!ready(&tracker, now + Duration::from_secs(120), false));
        assert!(ready(&tracker, now + Duration::from_secs(220), false));
        tracker.pending = false;
        tracker.running = true;
        complete(
            &mut tracker,
            Err("remote_unavailable"),
            now + Duration::from_secs(220),
        );
        assert!(!ready(&tracker, now + Duration::from_secs(221), false));
        assert!(ready(&tracker, now + Duration::from_secs(280), false));
        assert!(ready(&tracker, now + Duration::from_secs(221), true));
        tracker.pending = false;
        complete(&mut tracker, Ok(()), now + Duration::from_secs(280));
        assert_eq!(tracker.failures, 0);
        assert!(!ready(&tracker, now + Duration::from_secs(281), true));
    }

    #[test]
    #[ignore = "requires a local Git executable to create a temporary bare remote"]
    fn local_bare_chat_batch_retry_close_and_disabled() {
        use crate::{
            archive,
            archive_location::ArchiveLocation,
            archive_remote::{self, TestRemote},
            runtime::RuntimePaths,
        };
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        register_object_fixture(base);
        let root = base.join("archive");
        let bare = base.join("remote.git");
        let db = base.join("momso.db");
        let backups = base.join("backups");
        let git = crate::process::resolve_tool("git.exe").expect("Git required for this test");
        archive_remote::git(&git, base, &["init", "--bare", bare.to_str().unwrap()]).unwrap();
        storage::initialize(&db, &backups, "fixture").unwrap();
        let location = ArchiveLocation::load(db.clone(), base, vec![]).unwrap();
        *location.current.try_write().unwrap() = root.clone();
        archive::initialize(&root, base).unwrap();
        let paths = RuntimePaths {
            data_root: base.to_path_buf(),
            database_path: db.clone(),
            objects_path: root.join("objects"),
            exports_path: base.join("exports"),
            backups_path: backups,
            logs_path: base.join("logs"),
            archive: location,
            archive_lease: None,
        };
        let app = tauri::test::mock_builder()
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
            .manage(AutoBackup::default())
            .manage(TestRemote {
                tool: git.clone(),
                target: bare.to_string_lossy().into_owned(),
            })
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        for message in ["first fixture", "second fixture"] {
            let chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
            crate::chat::append(
                &db,
                chat["id"].as_str().unwrap(),
                crate::chat::Role::User,
                message,
                None,
                None,
            )
            .unwrap();
        }
        while !crate::chat_redaction_recheck::step(&root, &app.state::<AppState>().paths)
            .unwrap()
            .available
        {}
        if std::env::var_os("MOMSO_B84_BACKUP_MEASURE").is_some() {
            let id = crate::archive_zet::create(
                &root,
                &db,
                crate::archive_zet::ZetCreate {
                    title: "fixture".into(),
                    summary: "fixture".into(),
                    kind: "note".into(),
                    body: "synthetic fixture".into(),
                    assets: vec![],
                    edges: vec![],
                },
            )
            .unwrap();
            let template =
                std::fs::read_to_string(root.join("zettels").join(format!("{id}.md"))).unwrap();
            let base_time = chrono::Local::now().naive_local() - chrono::Duration::seconds(2000);
            let extra = if std::env::var("MOMSO_B84_BACKUP_MEASURE").as_deref() == Ok("small") {
                0
            } else {
                999
            };
            for n in 0..extra {
                let next = format!(
                    "zet_{}",
                    (base_time + chrono::Duration::seconds(n)).format("%Y%m%d_%H%M%S")
                );
                std::fs::write(
                    root.join("zettels").join(format!("{next}.md")),
                    template.replace(&id, &next),
                )
                .unwrap();
            }
            if extra != 0 {
                for n in 0..2000 {
                    let hash = format!("{n:064x}");
                    let directory = root.join("objects").join("sha256").join(&hash[..2]);
                    std::fs::create_dir_all(&directory).unwrap();
                    std::fs::write(directory.join(hash), [b'x'; 256]).unwrap();
                }
            }
        }
        tauri::async_runtime::block_on(attempt(app.handle().clone(), false));
        assert!(archive_remote::git(
            &git,
            base,
            &[
                "--git-dir",
                bare.to_str().unwrap(),
                "rev-parse",
                "refs/heads/main"
            ]
        )
        .is_err());
        app.state::<AutoBackup>().0.lock().unwrap().changed_at = Some(Instant::now() - QUIET);
        if std::env::var_os("MOMSO_B84_BACKUP_MEASURE").is_some() {
            crate::archive::BENCH_FOLLOW_CALLS.store(0, std::sync::atomic::Ordering::Relaxed);
            crate::archive::BENCH_FOLLOW_MICROS.store(0, std::sync::atomic::Ordering::Relaxed);
            println!("B84_BACKUP_START");
        }
        tauri::async_runtime::block_on(attempt(app.handle().clone(), false));
        if std::env::var_os("MOMSO_B84_BACKUP_MEASURE").is_some() {
            println!("B84_BACKUP_END");
            println!(
                "B85_FOLLOW count={} ms={}",
                crate::archive::BENCH_FOLLOW_CALLS.load(std::sync::atomic::Ordering::Relaxed),
                crate::archive::BENCH_FOLLOW_MICROS.load(std::sync::atomic::Ordering::Relaxed)
                    / 1000
            );
            println!(
                "B84_BACKUP_STATUS {}",
                serde_json::to_string(&archive_auto_backup_get(app.state(), app.state()).unwrap())
                    .unwrap()
            );
        }
        let remote_head = || {
            archive_remote::git(
                &git,
                base,
                &[
                    "--git-dir",
                    bare.to_str().unwrap(),
                    "rev-parse",
                    "refs/heads/main",
                ],
            )
            .unwrap()
        };
        let first = remote_head();
        let (_, _, paired_head) =
            archive_remote::paired_git_context(app.handle(), &root, &db, true)
                .expect("uploaded Git and local chat state can form a backup pair");
        assert_eq!(paired_head, first);
        assert_eq!(
            archive_auto_backup_get(app.state(), app.state())
                .unwrap()
                .state,
            "idle"
        );
        assert_eq!(
            archive_remote::git(&git, &root, &["rev-parse", "HEAD"]).unwrap(),
            first
        );
        assert_eq!(
            archive_remote::git(
                &git,
                &root,
                &["rev-list", "--count", "HEAD", &format!("^{first}")]
            )
            .unwrap(),
            "0"
        );
        tauri::async_runtime::block_on(attempt(app.handle().clone(), false));
        assert_eq!(
            archive_auto_backup_get(app.state(), app.state())
                .unwrap()
                .state,
            "idle"
        );
        let count = archive_remote::git(
            &git,
            base,
            &[
                "--git-dir",
                bare.to_str().unwrap(),
                "ls-tree",
                "-r",
                "--name-only",
                "refs/heads/main",
                "chats/",
            ],
        )
        .unwrap();
        assert_eq!(
            count.lines().filter(|line| line.ends_with(".json")).count(),
            2
        );
        let document = crate::archive_zet::create(
            &root,
            &db,
            crate::archive_zet::ZetCreate {
                title: "Fixture document".into(),
                summary: "fixture".into(),
                kind: "note".into(),
                body: "fixture body".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        tauri::async_runtime::block_on(attempt(app.handle().clone(), false));
        assert_eq!(remote_head(), first);
        app.state::<AutoBackup>().0.lock().unwrap().changed_at = Some(Instant::now() - QUIET);
        tauri::async_runtime::block_on(attempt(app.handle().clone(), false));
        let saved_document = archive_remote::git(
            &git,
            base,
            &[
                "--git-dir",
                bare.to_str().unwrap(),
                "show",
                &format!("refs/heads/main:zettels/{document}.md"),
            ],
        )
        .unwrap();
        assert!(saved_document.contains("fixture body"));
        let uploaded = remote_head();
        assert_eq!(
            archive_remote::git(
                &git,
                &root,
                &["rev-list", "--count", "HEAD", &format!("^{uploaded}")]
            )
            .unwrap(),
            "0"
        );
        assert_eq!(
            archive_auto_backup_get(app.state(), app.state())
                .unwrap()
                .state,
            "idle"
        );
        tauri::async_runtime::block_on(attempt(app.handle().clone(), false));
        assert_eq!(
            archive_auto_backup_get(app.state(), app.state())
                .unwrap()
                .state,
            "idle"
        );
        let first = remote_head();
        let skill = root.join("skills/naver-smartplace/SKILL.md");
        let mut original = std::fs::read_to_string(&skill).unwrap();
        original.push_str("\nfixture external skill edit\n");
        std::fs::write(&skill, &original).unwrap();
        tauri::async_runtime::block_on(attempt(app.handle().clone(), false));
        assert_eq!(remote_head(), first);
        app.state::<AutoBackup>().0.lock().unwrap().changed_at = Some(Instant::now() - QUIET);
        tauri::async_runtime::block_on(attempt(app.handle().clone(), false));
        let saved_skill = archive_remote::git(
            &git,
            base,
            &[
                "--git-dir",
                bare.to_str().unwrap(),
                "show",
                "refs/heads/main:skills/naver-smartplace/SKILL.md",
            ],
        )
        .unwrap();
        assert!(saved_skill.contains("fixture external skill edit"));
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), original);
        let custom =
            "---\nname: fixture-custom\ndescription: Fixture\ntitle: Fixture\n---\ncustom body\n";
        for folder in ["skills", ".agents/skills", ".claude/skills"] {
            let dir = root.join(folder).join("fixture-custom");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), custom).unwrap();
        }
        let custom_state =
            serde_json::json!({"hash": crate::archive_zet::content_hash(custom), "enabled": true});
        std::fs::write(
            base.join("skill-state/fixture-custom.json"),
            serde_json::to_vec(&custom_state).unwrap(),
        )
        .unwrap();
        tauri::async_runtime::block_on(attempt(app.handle().clone(), true));
        assert!(archive_remote::git(
            &git,
            base,
            &[
                "--git-dir",
                bare.to_str().unwrap(),
                "show",
                "refs/heads/main:skills/fixture-custom/SKILL.md"
            ]
        )
        .unwrap()
        .contains("custom body"));
        let provider_copy = root.join(".agents/skills/fixture-custom/SKILL.md");
        let conflicting_copy = format!("{custom}\nindependent edit\n");
        std::fs::write(&provider_copy, &conflicting_copy).unwrap();
        std::fs::remove_file(root.join("skills/fixture-custom/SKILL.md")).unwrap();
        let before_conflict = remote_head();
        tauri::async_runtime::block_on(attempt(app.handle().clone(), true));
        assert_eq!(remote_head(), before_conflict);
        assert_eq!(
            std::fs::read_to_string(&provider_copy).unwrap(),
            conflicting_copy
        );
        assert_eq!(
            app.state::<AutoBackup>()
                .0
                .lock()
                .unwrap()
                .reason
                .as_deref(),
            Some("unsaved_skill_changes")
        );
        std::fs::write(&provider_copy, custom).unwrap();
        tauri::async_runtime::block_on(attempt(app.handle().clone(), true));
        assert!(archive_remote::git(
            &git,
            base,
            &[
                "--git-dir",
                bare.to_str().unwrap(),
                "show",
                "refs/heads/main:skills/fixture-custom/SKILL.md"
            ]
        )
        .is_err());
        let first = remote_head();
        let conn = storage::open_writable_connection(&db).unwrap();
        conn.execute("INSERT INTO app_runtime_meta(key,value,updated_at) VALUES('archive_auto_backup_enabled','0',CURRENT_TIMESTAMP)", []).unwrap();
        let chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
        crate::chat::append(
            &db,
            chat["id"].as_str().unwrap(),
            crate::chat::Role::User,
            "offline fixture",
            None,
            None,
        )
        .unwrap();
        tauri::async_runtime::block_on(attempt(app.handle().clone(), true));
        assert_eq!(remote_head(), first);
        conn.execute(
            "UPDATE app_runtime_meta SET value='1' WHERE key='archive_auto_backup_enabled'",
            [],
        )
        .unwrap();
        let unavailable = base.join("temporarily-unavailable.git");
        std::fs::rename(&bare, &unavailable).unwrap();
        tauri::async_runtime::block_on(attempt(app.handle().clone(), true));
        assert!(app.state::<AutoBackup>().0.lock().unwrap().failures > 0);
        let mut reopened = Tracker::default();
        load_failures(&db, &mut reopened).unwrap();
        assert!(reopened.failures > 0);
        assert!(reopened.reason.is_some());
        std::fs::rename(&unavailable, &bare).unwrap();
        tauri::async_runtime::block_on(attempt(app.handle().clone(), true));
        assert_ne!(remote_head(), first);
        assert_eq!(app.state::<AutoBackup>().0.lock().unwrap().failures, 0);
        let before_lock = remote_head();
        let chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
        crate::chat::append(
            &db,
            chat["id"].as_str().unwrap(),
            crate::chat::Role::User,
            "locked fixture",
            None,
            None,
        )
        .unwrap();
        let receive_lock = archive_remote::lock_first_receive(&db).unwrap();
        tauri::async_runtime::block_on(attempt(app.handle().clone(), true));
        assert_eq!(remote_head(), before_lock);
        assert_eq!(
            app.state::<AutoBackup>()
                .0
                .lock()
                .unwrap()
                .reason
                .as_deref(),
            Some("busy")
        );
        drop(receive_lock);
        tauri::async_runtime::block_on(attempt(app.handle().clone(), true));
        assert_ne!(remote_head(), before_lock);
    }

    #[test]
    #[ignore = "requires a local Git executable and a temporary slow bare remote"]
    fn slow_remote_close_timeout_retries_on_restart() {
        use crate::{
            archive,
            archive_location::ArchiveLocation,
            archive_remote::{self, TestRemote},
            runtime::RuntimePaths,
        };
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        register_object_fixture(base);
        let root = base.join("archive");
        let bare = base.join("remote.git");
        let db = base.join("momso.db");
        let backups = base.join("backups");
        let git = crate::process::resolve_tool("git.exe").expect("Git required for this test");
        archive_remote::git(&git, base, &["init", "--bare", bare.to_str().unwrap()]).unwrap();
        storage::initialize(&db, &backups, "fixture").unwrap();
        let location = ArchiveLocation::load(db.clone(), base, vec![]).unwrap();
        *location.current.try_write().unwrap() = root.clone();
        archive::initialize(&root, base).unwrap();
        let paths = RuntimePaths {
            data_root: base.to_path_buf(),
            database_path: db.clone(),
            objects_path: root.join("objects"),
            exports_path: base.join("exports"),
            backups_path: backups,
            logs_path: base.join("logs"),
            archive: location,
            archive_lease: None,
        };
        let make_app = || {
            tauri::test::mock_builder()
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
                .manage(AutoBackup::default())
                .manage(TestRemote {
                    tool: git.clone(),
                    target: bare.to_string_lossy().into_owned(),
                })
                .build(tauri::test::mock_context(tauri::test::noop_assets()))
                .unwrap()
        };
        let app = make_app();
        let initial = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
        crate::chat::append(
            &db,
            initial["id"].as_str().unwrap(),
            crate::chat::Role::User,
            "initial fixture",
            None,
            None,
        )
        .unwrap();
        while !crate::chat_redaction_recheck::step(&root, &paths)
            .unwrap()
            .available
        {}
        tauri::async_runtime::block_on(attempt(app.handle().clone(), true));
        let remote_head = || {
            archive_remote::git(
                &git,
                base,
                &[
                    "--git-dir",
                    bare.to_str().unwrap(),
                    "rev-parse",
                    "refs/heads/main",
                ],
            )
            .unwrap()
        };
        let before = remote_head();
        let pending = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
        let pending_id = pending["id"].as_str().unwrap();
        crate::chat::append(
            &db,
            pending_id,
            crate::chat::Role::User,
            "pending fixture",
            None,
            None,
        )
        .unwrap();
        let hook = bare.join("hooks/pre-receive");
        std::fs::write(
            &hook,
            "#!/bin/sh\nprintf started > d22-slow-hook-started\nsleep 25\nexit 1\n",
        )
        .unwrap();
        let started = Instant::now();
        let finished = tauri::async_runtime::block_on(close_backup_with_timeout(
            app.handle().clone(),
            paths.clone(),
            base.to_path_buf(),
            Duration::from_secs(15),
        ));
        let elapsed = started.elapsed();
        assert!(
            matches!(finished, CloseResult::TimedOut),
            "slow remote unexpectedly completed before timeout"
        );
        assert!(
            elapsed >= Duration::from_secs(15) && elapsed < Duration::from_secs(20),
            "timeout took {elapsed:?}"
        );
        assert!(
            bare.join("d22-slow-hook-started").is_file(),
            "slow remote was not reached"
        );
        assert_eq!(remote_head(), before);
        {
            let state = app.state::<AutoBackup>();
            let tracker = state.0.lock().unwrap();
            assert!(tracker.pending && !tracker.running);
        }
        drop(app);
        let restarted = make_app();
        tauri::async_runtime::block_on(attempt(restarted.handle().clone(), false));
        assert_eq!(remote_head(), before);
        // The restarted scheduler waits for the old blocking transfer to
        // release the shared lock; the slow hook refuses that first push.
        drop(archive_remote::lock_remote_for_restore().unwrap());
        assert_eq!(remote_head(), before);
        std::fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();
        restarted.state::<AutoBackup>().0.lock().unwrap().changed_at = Some(Instant::now() - QUIET);
        tauri::async_runtime::block_on(attempt(restarted.handle().clone(), false));
        assert_ne!(remote_head(), before);
        let path = format!("refs/heads/main:chats/{pending_id}.json");
        assert!(archive_remote::git(
            &git,
            base,
            &["--git-dir", bare.to_str().unwrap(), "show", &path]
        )
        .is_ok());
    }
}
