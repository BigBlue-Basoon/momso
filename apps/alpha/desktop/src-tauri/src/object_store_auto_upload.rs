use crate::{object_store, object_store_transfer, storage, AppState};
use serde::Serialize;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{Manager, State};

const QUIET: Duration = Duration::from_secs(120);
const INTERVAL: Duration = Duration::from_secs(30);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Default)]
struct Tracker {
    fingerprint: Option<u64>,
    changed_at: Option<Instant>,
    retry_at: Option<Instant>,
    pending: bool,
    running: bool,
    failures: u32,
    reason: Option<String>,
    loaded: bool,
}

#[derive(Default)]
pub(crate) struct AutoUpload(Mutex<Tracker>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutoUploadStatus {
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
            "SELECT value FROM app_runtime_meta WHERE key='object_store_auto_upload_enabled'",
            [],
            |row| row.get(0),
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
            "SELECT value FROM app_runtime_meta WHERE key='object_store_auto_upload_failures'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| "local_state_unavailable")?;
    let reason: Option<String> = conn
        .query_row(
            "SELECT value FROM app_runtime_meta WHERE key='object_store_auto_upload_reason'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| "local_state_unavailable")?;
    tracker.failures = count
        .map(|v| v.parse::<u32>().map_err(|_| "local_state_unavailable"))
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
        (
            "object_store_auto_upload_failures",
            tracker.failures.to_string(),
        ),
        (
            "object_store_auto_upload_reason",
            tracker.reason.clone().unwrap_or_default(),
        ),
    ] {
        tx.execute("INSERT INTO app_runtime_meta(key,value,updated_at) VALUES(?1,?2,CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=CURRENT_TIMESTAMP", (key, value)).map_err(|_| "local_state_unavailable")?;
    }
    tx.commit().map_err(|_| "local_state_unavailable")
}

fn status(enabled: bool, connected: bool, tracker: &Tracker) -> AutoUploadStatus {
    AutoUploadStatus {
        enabled,
        state: if !enabled {
            "disabled"
        } else if tracker.running {
            "running"
        } else if !connected {
            "not_connected"
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
pub(crate) fn object_store_auto_upload_get(
    state: State<'_, AppState>,
    automatic: State<'_, AutoUpload>,
) -> Result<AutoUploadStatus, &'static str> {
    let enabled = enabled(&state.paths.database_path)?;
    let connected = object_store::load_config(&state.paths.data_root)?.is_some();
    let mut tracker = automatic.0.lock().map_err(|_| "local_state_unavailable")?;
    load_failures(&state.paths.database_path, &mut tracker)?;
    Ok(status(enabled, connected, &tracker))
}

#[tauri::command]
pub(crate) async fn object_store_auto_upload_set(
    state: State<'_, AppState>,
    automatic: State<'_, AutoUpload>,
    enabled: bool,
) -> Result<AutoUploadStatus, &'static str> {
    // Once disabling returns, no queued automatic backup can pass its own
    // enabled check while holding this same lock.
    let _connection = object_store::CONNECTION_LOCK.lock().await;
    let conn = storage::open_writable_connection(&state.paths.database_path)
        .map_err(|_| "local_state_unavailable")?;
    conn.execute("INSERT INTO app_runtime_meta(key,value,updated_at) VALUES('object_store_auto_upload_enabled',?1,CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=CURRENT_TIMESTAMP", [if enabled { "1" } else { "0" }]).map_err(|_| "local_state_unavailable")?;
    let connected = object_store::load_config(&state.paths.data_root)?.is_some();
    let mut tracker = automatic.0.lock().map_err(|_| "local_state_unavailable")?;
    load_failures(&state.paths.database_path, &mut tracker)?;
    if enabled {
        tracker.pending = true;
        tracker.changed_at = Some(Instant::now());
        tracker.retry_at = None;
    }
    Ok(status(enabled, connected, &tracker))
}

fn fingerprint(paths: &crate::runtime::RuntimePaths) -> Result<(bool, u64), &'static str> {
    let mut hash = DefaultHasher::new();
    let config = object_store::load_config(&paths.data_root)?;
    if let Some(config) = &config {
        config.credential_id.hash(&mut hash);
        config.bucket.hash(&mut hash);
        config.endpoint.hash(&mut hash);
    }
    paths.objects_path.hash(&mut hash);
    for (relative, path, size) in object_store_transfer::object_files(&paths.objects_path)? {
        relative.hash(&mut hash);
        size.hash(&mut hash);
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .map_err(|_| "local_read_failed")?
            .hash(&mut hash);
    }
    let conn = rusqlite::Connection::open_with_flags(
        &paths.database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| "local_state_unavailable")?;
    let mut statement = conn
        .prepare("SELECT asset_id,operation_id,state FROM source_asset_deletions ORDER BY asset_id")
        .map_err(|_| "local_state_unavailable")?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|_| "local_state_unavailable")?;
    for row in rows {
        row.map_err(|_| "local_state_unavailable")?.hash(&mut hash);
    }
    Ok((config.is_some(), hash.finish()))
}

fn observe(tracker: &mut Tracker, value: u64, now: Instant) {
    if tracker.fingerprint != Some(value) {
        tracker.fingerprint = Some(value);
        tracker.pending = true;
        tracker.changed_at = Some(now);
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
        Err("disabled") => {
            tracker.pending = true;
        }
        Err(reason) => {
            tracker.pending = true;
            tracker.failures = tracker.failures.saturating_add(1);
            tracker.reason = Some(reason.to_owned());
            let seconds = 30_u64
                .saturating_mul(1_u64 << tracker.failures.min(6))
                .min(1800);
            tracker.retry_at = Some(now + Duration::from_secs(seconds));
        }
    }
}

struct InFlight<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
    armed: bool,
}
impl<R: tauri::Runtime> Drop for InFlight<R> {
    fn drop(&mut self) {
        if self.armed {
            if let Some(auto) = self.app.try_state::<AutoUpload>() {
                if let Ok(mut tracker) = auto.0.try_lock() {
                    complete(&mut tracker, Err("interrupted"), Instant::now());
                }
            }
        }
    }
}

pub(crate) async fn attempt<R: tauri::Runtime>(app: tauri::AppHandle<R>, closing: bool) {
    let state = app.state::<AppState>();
    if !enabled(&state.paths.database_path).unwrap_or(false) {
        return;
    }
    let paths = state.paths.scoped().await;
    let auto = app.state::<AutoUpload>();
    let observed = tauri::async_runtime::spawn_blocking(move || fingerprint(&paths)).await;
    let (connected, value) = match observed {
        Ok(Ok(value)) => value,
        _ => {
            if let Ok(mut tracker) = auto.0.lock() {
                let _ = load_failures(&state.paths.database_path, &mut tracker);
                if tracker.retry_at.is_none_or(|at| Instant::now() >= at) {
                    complete(&mut tracker, Err("local_state_unavailable"), Instant::now());
                    let _ = persist_failures(&state.paths.database_path, &tracker);
                }
            }
            return;
        }
    };
    {
        let Ok(mut tracker) = auto.0.lock() else {
            return;
        };
        if load_failures(&state.paths.database_path, &mut tracker).is_err() {
            return;
        }
        observe(&mut tracker, value, Instant::now());
        if !connected || !ready(&tracker, Instant::now(), closing) {
            return;
        }
        tracker.running = true;
        tracker.pending = false;
    }
    let mut guard = InFlight {
        app: app.clone(),
        armed: true,
    };
    let result = match object_store_transfer::backup_with_mode(app.clone(), app.state(), true).await
    {
        Ok(_) => crate::paired_backup::capture(app.clone()).await.map(|_| ()),
        Err(error) => Err(error),
    };
    let paths = state.paths.scoped().await;
    let after = tauri::async_runtime::spawn_blocking(move || fingerprint(&paths))
        .await
        .ok()
        .and_then(Result::ok);
    if let Ok(mut tracker) = auto.0.lock() {
        complete(&mut tracker, result, Instant::now());
        if let Some((_, after)) = after.filter(|(_, after)| tracker.fingerprint != Some(*after)) {
            observe(&mut tracker, after, Instant::now());
        }
        if persist_failures(&state.paths.database_path, &tracker).is_err() {
            tracker.pending = true;
            tracker.reason = Some("local_state_unavailable".to_owned());
        }
    }
    guard.armed = false;
}

pub(crate) fn start(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            attempt(app.clone(), false).await;
            tokio::time::sleep(INTERVAL).await;
        }
    });
}

/// A successful Git backup also needs an encrypted database backup.
pub(crate) fn schedule_pair<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(auto) = app.try_state::<AutoUpload>() {
        if let Ok(mut tracker) = auto.0.lock() {
            tracker.pending = true;
            tracker.changed_at = Instant::now().checked_sub(QUIET);
            tracker.retry_at = None;
        }
    }
}

pub(crate) async fn close_upload<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> bool {
    tokio::time::timeout(CLOSE_TIMEOUT, async move {
        let state = app.state::<AppState>();
        match enabled(&state.paths.database_path) {
            Ok(false) => return true,
            Err(_) => return false,
            Ok(true) => (),
        }
        match object_store::load_config(&state.paths.data_root) {
            Ok(None) => return true,
            Err(_) => return false,
            Ok(Some(_)) => (),
        }
        loop {
            let running = app
                .state::<AutoUpload>()
                .0
                .lock()
                .map(|tracker| tracker.running)
                .unwrap_or(false);
            if !running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        schedule_pair(&app);
        attempt(app.clone(), true).await;
        app.state::<AutoUpload>()
            .0
            .lock()
            .map(|tracker| !tracker.running && !tracker.pending && tracker.failures == 0)
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

#[cfg(test)]
pub(crate) fn expedite(auto: &AutoUpload) {
    let mut tracker = auto.0.lock().unwrap();
    tracker.changed_at = Some(Instant::now() - QUIET);
    tracker.retry_at = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_recovery_key_is_visible_as_retry_reason() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let error = match crate::object_store_transfer::identity(&db) {
            Err(error) => error,
            Ok(_) => panic!("new profile must not already have a recovery key"),
        };
        let mut tracker = Tracker::default();
        complete(&mut tracker, Err(error), Instant::now());
        let value = serde_json::to_value(status(true, true, &tracker)).unwrap();
        assert_eq!(value["state"], "retry_pending");
        assert_eq!(value["reason"], "recovery_key_missing");
    }

    #[test]
    fn batches_and_retries_with_exponential_backoff() {
        let now = Instant::now();
        let mut tracker = Tracker::default();
        observe(&mut tracker, 1, now);
        observe(&mut tracker, 2, now + Duration::from_secs(90));
        assert!(!ready(&tracker, now + Duration::from_secs(120), false));
        assert!(ready(&tracker, now + Duration::from_secs(210), false));
        tracker.running = true;
        tracker.pending = false;
        complete(
            &mut tracker,
            Err("remote_unavailable"),
            now + Duration::from_secs(210),
        );
        assert!(!ready(&tracker, now + Duration::from_secs(269), false));
        assert!(ready(&tracker, now + Duration::from_secs(270), false));
        complete(
            &mut tracker,
            Err("remote_unavailable"),
            now + Duration::from_secs(270),
        );
        assert!(!ready(&tracker, now + Duration::from_secs(389), false));
        assert!(ready(&tracker, now + Duration::from_secs(390), false));
        assert!(ready(&tracker, now + Duration::from_secs(271), true));
        tracker.pending = false;
        complete(&mut tracker, Ok(()), now + Duration::from_secs(390));
        assert_eq!(tracker.failures, 0);
        assert!(!tracker.pending);
    }
}
