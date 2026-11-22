//! Durable local removal after recording the deletion in encrypted backup metadata.
use super::*;

pub fn initialize(app: &App) -> Result<(), ApiError> {
    let conn = db(app)?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS pending_original_purges(asset_id TEXT PRIMARY KEY, digest TEXT NOT NULL)")
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    let pending: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pending_original_purges)",
            [],
            |r| r.get(0),
        )
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    maintenance::set_purge_pending(&app.maintenance, pending)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    Ok(())
}

pub fn record(app: &App, id: &str, digest: &str) -> Result<(), ApiError> {
    let conn = db(app)?;
    conn.execute(
        "INSERT INTO pending_original_purges(asset_id,digest) VALUES(?1,?2)",
        [id, digest],
    )
    .map_err(|_| error(StatusCode::CONFLICT, "purge_pending"))?;
    maintenance::set_purge_pending(&app.maintenance, true)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    Ok(())
}

pub fn pending(app: &App) -> Result<Vec<(String, String)>, ApiError> {
    let conn = db(app)?;
    let mut statement = conn
        .prepare("SELECT asset_id,digest FROM pending_original_purges ORDER BY asset_id")
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    let rows = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))
}

/// The caller holds the admission checkpoint and backup selection lock.
pub fn complete(app: &App, id: &str, digest: &str) -> Result<(), ApiError> {
    github_worker::record_original_purge(app, digest)
        .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "purge_backup_unavailable"))?;
    let store = attachment_bridge::store(app)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_unavailable"))?;
    store
        .mark_purged(id, digest)
        .and_then(|()| store.remove_purged_original(id))
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "purge_local_unavailable"))?;
    db(app)?
        .execute(
            "DELETE FROM pending_original_purges WHERE asset_id=?1 AND digest=?2",
            [id, digest],
        )
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    let remaining = !pending(app)?.is_empty();
    maintenance::set_purge_pending(&app.maintenance, remaining)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    maintenance::touch(app);
    app.backup.wake();
    Ok(())
}

pub fn start(app: App) {
    tokio::spawn(async move {
        loop {
            let work = app.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if !pending(&work).is_ok_and(|items| !items.is_empty()) {
                    return;
                }
                let Ok(Some(_checkpoint)) = maintenance::begin_purge(&work.maintenance, 0) else {
                    return;
                };
                if !work.active.lock().is_ok_and(|a| a.is_empty())
                    || work.writes.load(Ordering::SeqCst) != 0
                {
                    return;
                }
                let Ok(_backup) = work.backup.selection.lock() else {
                    return;
                };
                if let Ok(items) = pending(&work) {
                    for (id, digest) in items {
                        if complete(&work, &id, &digest).is_err() {
                            break;
                        }
                    }
                }
            })
            .await;
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    });
}
