//! Admission and status boundary for the external S38 updater.
use super::*;
use axum::{extract::Request, middleware::Next, response::Response};

pub struct Gate {
    maintenance: bool,
    checkpoint: bool,
    purge_pending: bool,
    admitted: usize,
    generation: u64,
    closed_at_ms: Option<i64>,
    last_activity: chrono::DateTime<chrono::Utc>,
}
pub type Shared = Arc<Mutex<Gate>>;

pub fn new() -> Shared {
    Arc::new(Mutex::new(Gate {
        maintenance: false,
        checkpoint: false,
        purge_pending: false,
        admitted: 0,
        generation: 0,
        closed_at_ms: None,
        last_activity: chrono::Utc::now(),
    }))
}
fn marker(app: &App) -> PathBuf {
    app.data.join("maintenance.pending")
}

pub fn initialize(app: &App) -> Result<(), ApiError> {
    let enabled = match std::fs::symlink_metadata(marker(app)) {
        Ok(meta) if meta.file_type().is_file() => true,
        Ok(_) => {
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "maintenance_state_invalid",
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => {
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "maintenance_state_unavailable",
            ))
        }
    };
    let mut gate = app
        .maintenance
        .lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?;
    gate.maintenance = enabled;
    Ok(())
}

struct Admitted(Shared, bool);
impl Drop for Admitted {
    fn drop(&mut self) {
        if let Ok(mut gate) = self.0.lock() {
            gate.admitted = gate.admitted.saturating_sub(1);
            if self.1 {
                gate.generation = gate.generation.saturating_add(1);
            }
            gate.last_activity = chrono::Utc::now();
        }
    }
}
fn completion(path: &str) -> bool {
    (path.starts_with("/approvals/") && path.ends_with("/respond"))
        || (path.starts_with("/input-requests/")
            && (path.ends_with("/respond") || path.ends_with("/cancel")))
        || (path.starts_with("/turns/") && path.ends_with("/cancel"))
        || path == "/github/login/cancel"
        || path == "/github/restore"
        || path == "/providers/codex/login/cancel"
}
pub async fn admit(State(app): State<App>, request: Request, next: Next) -> Response {
    let method = request.method();
    let path = request.uri().path();
    let maintenance_endpoint = path == "/maintenance" || path == "/maintenance/release";
    let changes = matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );
    let meaningful_read = *method == Method::GET
        && (path == "/providers"
            || path == "/chats"
            || path.starts_with("/chats/")
            || path == "/documents"
            || path.starts_with("/documents/")
            || path == "/github/repositories"
            || path.starts_with("/uploads/"));
    let should_count = (changes && !maintenance_endpoint) || meaningful_read;
    let guard = if should_count && auth(&app, request.headers()).is_ok() {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let waiting = {
                let mut gate = match app.maintenance.lock() {
                    Ok(gate) => gate,
                    Err(_) => {
                        return error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable")
                            .into_response()
                    }
                };
                if (gate.maintenance && !completion(path))
                    || (gate.purge_pending
                        && changes
                        && !(path.starts_with("/assets/") && path.ends_with("/purge")))
                {
                    return error(StatusCode::SERVICE_UNAVAILABLE, "maintenance_in_progress")
                        .into_response();
                }
                if gate.checkpoint {
                    true
                } else {
                    gate.admitted += 1;
                    if changes {
                        gate.generation = gate.generation.saturating_add(1);
                    }
                    gate.last_activity = chrono::Utc::now();
                    false
                }
            };
            if !waiting {
                break Some(Admitted(app.maintenance.clone(), changes));
            }
            if tokio::time::Instant::now() >= deadline {
                return error(StatusCode::SERVICE_UNAVAILABLE, "maintenance_in_progress")
                    .into_response();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    } else {
        None
    };
    let result = next.run(request).await;
    drop(guard);
    result
}

pub struct Checkpoint(Shared);
impl Drop for Checkpoint {
    fn drop(&mut self) {
        if let Ok(mut gate) = self.0.lock() {
            gate.checkpoint = false;
        }
    }
}

/// Called after HTTP admission (or before startup, with no admitted request).
pub fn begin_purge(shared: &Shared, admitted: usize) -> Result<Option<Checkpoint>, &'static str> {
    let mut gate = shared.lock().map_err(|_| "unavailable")?;
    if gate.maintenance || gate.checkpoint || gate.admitted != admitted {
        return Ok(None);
    }
    gate.checkpoint = true;
    Ok(Some(Checkpoint(shared.clone())))
}

pub fn set_purge_pending(shared: &Shared, pending: bool) -> Result<(), &'static str> {
    shared.lock().map_err(|_| "unavailable")?.purge_pending = pending;
    Ok(())
}

pub fn begin_checkpoint(shared: &Shared) -> Result<Option<Checkpoint>, &'static str> {
    let mut gate = shared.lock().map_err(|_| "unavailable")?;
    if gate.checkpoint || gate.admitted != 0 {
        return Ok(None);
    }
    gate.checkpoint = true;
    Ok(Some(Checkpoint(shared.clone())))
}

pub fn observe(app: &App) -> Result<(bool, usize, String), ApiError> {
    let gate = app
        .maintenance
        .lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?;
    Ok((
        gate.maintenance,
        gate.admitted,
        gate.last_activity.to_rfc3339(),
    ))
}
pub fn touch_shared(shared: &Shared) {
    if let Ok(mut gate) = shared.lock() {
        gate.generation = gate.generation.saturating_add(1);
        gate.last_activity = chrono::Utc::now();
    }
}

pub fn generation(shared: &Shared) -> Result<u64, &'static str> {
    shared
        .lock()
        .map(|gate| gate.generation)
        .map_err(|_| "unavailable")
}
pub fn touch(app: &App) {
    touch_shared(&app.maintenance);
}

pub async fn acquire(State(app): State<App>, h: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    auth(&app, &h)?;
    let mut gate = app
        .maintenance
        .lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?;
    if !gate.maintenance {
        // Fail closed even when creating or syncing the marker fails.
        gate.maintenance = true;
        let path = marker(&app);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => file.sync_all().map_err(|_| {
                error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "maintenance_state_unavailable",
                )
            })?,
            Err(e)
                if e.kind() == std::io::ErrorKind::AlreadyExists
                    && std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) => {}
            Err(_) => {
                return Err(error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "maintenance_state_unavailable",
                ))
            }
        }
        #[cfg(unix)]
        std::fs::File::open(&app.data)
            .and_then(|f| f.sync_all())
            .map_err(|_| {
                error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "maintenance_state_unavailable",
                )
            })?;
        // A maintenance request needs a new backup even when the last saved
        // generation was current before admission closed.
        gate.generation = gate.generation.saturating_add(1);
        gate.closed_at_ms = Some(chrono::Utc::now().timestamp_millis());
    }
    #[cfg(target_os = "linux")]
    app.backup.wake();
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(
            json!({"maintenance":true,"admittedRequests":gate.admitted,"closedAt":gate.closed_at_ms}),
        ),
    ))
}
pub async fn release(State(app): State<App>, h: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    auth(&app, &h)?;
    let mut gate = app
        .maintenance
        .lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?;
    if gate.admitted != 0 {
        return Err(error(StatusCode::CONFLICT, "work_in_progress"));
    }
    #[cfg(target_os = "linux")]
    if app.backup.status()?.active || app.backup.object_status()?.active {
        return Err(error(StatusCode::CONFLICT, "backup_in_progress"));
    }
    match std::fs::symlink_metadata(app.data.join("chat-restore.pending")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        _ => return Err(error(StatusCode::CONFLICT, "restore_incomplete")),
    }
    let path = marker(&app);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_file() => std::fs::remove_file(&path).map_err(|_| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "maintenance_state_unavailable",
            )
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        _ => {
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "maintenance_state_invalid",
            ))
        }
    }
    #[cfg(unix)]
    std::fs::File::open(&app.data)
        .and_then(|f| f.sync_all())
        .map_err(|_| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "maintenance_state_unavailable",
            )
        })?;
    gate.maintenance = false;
    gate.last_activity = chrono::Utc::now();
    #[cfg(target_os = "linux")]
    app.backup.wake();
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"maintenance":false})),
    ))
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;

    #[test]
    fn checkpoint_excludes_admitted_requests() {
        let gate = new();
        let checkpoint = begin_checkpoint(&gate).unwrap().unwrap();
        assert!(begin_checkpoint(&gate).unwrap().is_none());
        drop(checkpoint);
        {
            let mut state = gate.lock().unwrap();
            state.maintenance = true;
            state.admitted = 1;
        }
        assert!(begin_checkpoint(&gate).unwrap().is_none());
        gate.lock().unwrap().admitted = 0;
        let checkpoint = begin_checkpoint(&gate).unwrap().unwrap();
        assert!(begin_checkpoint(&gate).unwrap().is_none());
        drop(checkpoint);
        assert!(begin_checkpoint(&gate).unwrap().is_some());
    }

    #[test]
    fn purge_checkpoint_requires_other_requests_to_finish() {
        let gate = new();
        gate.lock().unwrap().admitted = 2;
        assert!(begin_purge(&gate, 1).unwrap().is_none());
        gate.lock().unwrap().admitted = 1;
        let checkpoint = begin_purge(&gate, 1).unwrap().unwrap();
        assert!(begin_purge(&gate, 1).unwrap().is_none());
        drop(checkpoint);
        assert!(begin_purge(&gate, 1).unwrap().is_some());
    }
}
