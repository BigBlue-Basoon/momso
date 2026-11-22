//! Host-only GitHub device authentication. No OAuth token reaches the client.
use super::*;
use momso_github_backup::github::{Github, Repository};
use momso_saved_login_core::linux::LinuxVault;

pub(crate) fn handle(app: &App) -> Result<Arc<Github>, ApiError> {
    let mut slot = app
        .github
        .lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?;
    if let Some(client) = slot.as_ref() {
        return Ok(client.clone());
    }
    let client_id = std::env::var("MOMSO_GITHUB_CLIENT_ID")
        .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "github_client_id_missing"))?;
    let vault = LinuxVault::from_systemd(&app.data.join("saved-logins"))
        .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "credential_unavailable"))?;
    let vault = Arc::new(vault);
    #[cfg(all(debug_assertions, feature = "test-fixtures"))]
    let client = if let Ok(base) = std::env::var("MOMSO_TEST_GITHUB_API") {
        Github::fixture(client_id, vault, &base)
    } else {
        Github::new(client_id, vault)
    };
    #[cfg(not(all(debug_assertions, feature = "test-fixtures")))]
    let client = Github::new(client_id, vault);
    let client = Arc::new(client.map_err(|reason| error(StatusCode::SERVICE_UNAVAILABLE, reason))?);
    #[cfg(all(debug_assertions, feature = "test-fixtures"))]
    if std::env::var_os("MOMSO_TEST_GITHUB_API").is_some() {
        if let Some(path) = std::env::var_os("MOMSO_TEST_GITHUB_GIT") {
            client
                .set_fixture_git_remote(std::path::Path::new(&path))
                .map_err(|reason| error(StatusCode::SERVICE_UNAVAILABLE, reason))?;
        }
    }
    *slot = Some(client.clone());
    Ok(client)
}
fn no_store(value: Value) -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-store")], Json(value))
}
async fn client(app: &App) -> Result<Arc<Github>, ApiError> {
    let app = app.clone();
    let guard = WorkGuard::new(&app);
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        handle(&app)
    })
    .await
    .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
}
pub async fn start(State(app): State<App>, h: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    auth(&app, &h)?;
    let client = client(&app).await?;
    let worker = client.clone();
    let guard = WorkGuard::new(&app);
    let vault_lock = app.vault_lock.clone();
    let started = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let _vault_guard = vault_lock.lock().map_err(|_| "credential_unavailable")?;
        worker.start_device_login()
    })
    .await
    .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
    .map_err(|reason| error(StatusCode::SERVICE_UNAVAILABLE, reason))?;
    let session = started.session_id.clone();
    app.github_logins.fetch_add(1, Ordering::SeqCst);
    let owner = app.clone();
    tokio::spawn(async move {
        struct LoginGuard(App);
        impl Drop for LoginGuard {
            fn drop(&mut self) {
                self.0.github_logins.fetch_sub(1, Ordering::SeqCst);
                maintenance::touch(&self.0);
            }
        }
        let login_guard = LoginGuard(owner);
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let worker = client.clone();
            let session = session.clone();
            let guard = WorkGuard::new(&login_guard.0);
            let vault_lock = login_guard.0.vault_lock.clone();
            match tokio::task::spawn_blocking(move || {
                let _guard = guard;
                let _vault_guard = vault_lock.lock().map_err(|_| "credential_unavailable")?;
                worker.poll_login(&session)
            })
            .await
            {
                Ok(Ok(view)) if view.state == "pending" => (),
                Ok(Err("provider_unavailable")) => (),
                _ => break,
            }
        }
    });
    Ok(no_store(json!({"sessionId":started.session_id})))
}
fn session(h: &HeaderMap) -> Result<String, ApiError> {
    h.get("x-momso-login-session")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "login_not_found"))
}
pub async fn status(State(app): State<App>, h: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    auth(&app, &h)?;
    let id = session(&h)?;
    let client = client(&app).await?;
    let view = tokio::task::spawn_blocking(move || client.login_status(&id))
        .await
        .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
        .map_err(|reason| error(StatusCode::NOT_FOUND, reason))?;
    Ok(no_store(serde_json::to_value(view).map_err(|_| {
        error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable")
    })?))
}
pub async fn cancel(State(app): State<App>, h: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    auth(&app, &h)?;
    let id = session(&h)?;
    let client = client(&app).await?;
    tokio::task::spawn_blocking(move || client.cancel_login(&id))
        .await
        .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
        .map_err(|reason| error(StatusCode::NOT_FOUND, reason))?;
    Ok(no_store(json!({"state":"cancelled"})))
}

pub async fn repositories(
    State(app): State<App>,
    h: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    auth(&app, &h)?;
    let client = client(&app).await?;
    let guard = WorkGuard::new(&app);
    let (account, repositories) = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let account = client.account_name()?;
        let repositories = client.list_private_repositories()?;
        Ok::<_, &'static str>((account, repositories))
    })
    .await
    .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
    .map_err(|reason| error(StatusCode::SERVICE_UNAVAILABLE, reason))?;
    Ok(no_store(
        json!({"account":account,"repositories":repositories}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectRepository {
    name_with_owner: String,
}

pub(crate) fn read_repository(data: &std::path::Path) -> Result<Option<Repository>, &'static str> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(data.join("github-repository.json"))
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("storage_unavailable"),
    };
    if !file
        .metadata()
        .map_err(|_| "storage_unavailable")?
        .is_file()
    {
        return Err("storage_unavailable");
    }
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| "storage_unavailable")?;
    if bytes.len() > 4096 {
        return Err("storage_unavailable");
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "storage_unavailable")
}

pub async fn selected_repository(
    State(app): State<App>,
    h: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    auth(&app, &h)?;
    let data = app.data.clone();
    let guard = ReadGuard::new(&app);
    let repository = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        read_repository(&data)
    })
    .await
    .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
    .map_err(|reason| error(StatusCode::SERVICE_UNAVAILABLE, reason))?;
    let status = app.backup.status()?;
    let generation = maintenance::generation(&app.maintenance)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?;
    let object = app.backup.object_status()?;
    let (state, reason) = if matches!(status.state, "saved" | "saved_partial")
        && status.synced_generation != Some(generation)
    {
        app.backup.wake();
        ("pending", Some("changes_after_backup"))
    } else if status.state == "saved"
        && (object.state != "saved" || object.synced_generation != Some(generation))
    {
        ("saved_partial", Some("object_backup_not_integrated"))
    } else {
        (status.state, status.reason)
    };
    Ok(no_store(
        json!({"repository":repository,"backupStatus":state,"backupReason":reason,
        "githubBackup":{"state":status.state,"reason":status.reason,"lastSuccessAt":status.last_success_at},
        "objectBackup":{"state":object.state,"reason":object.reason,"lastSuccessAt":object.last_success_at}}),
    ))
}

pub async fn select_repository(
    State(app): State<App>,
    h: HeaderMap,
    Json(body): Json<SelectRepository>,
) -> Result<impl IntoResponse, ApiError> {
    auth(&app, &h)?;
    let client = client(&app).await?;
    let data = app.data.clone();
    let backup = app.backup.clone();
    let guard = WorkGuard::new(&app);
    let repository = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let _selection = backup.selection.lock().map_err(|_| "backup_unavailable")?;
        let repository = client
            .inspect_repository(&body.name_with_owner)?
            .ok_or("repository_not_found")?;
        let bytes = serde_json::to_vec(&repository).map_err(|_| "storage_unavailable")?;
        let mut temp = tempfile::NamedTempFile::new_in(&data).map_err(|_| "storage_unavailable")?;
        use std::io::Write;
        temp.write_all(&bytes).map_err(|_| "storage_unavailable")?;
        temp.as_file()
            .sync_all()
            .map_err(|_| "storage_unavailable")?;
        temp.persist(data.join("github-repository.json"))
            .map_err(|_| "storage_unavailable")?;
        std::fs::File::open(&data)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "storage_unavailable")?;
        Ok::<_, &'static str>(repository)
    })
    .await
    .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
    .map_err(|reason| error(StatusCode::SERVICE_UNAVAILABLE, reason))?;
    app.backup.wake();
    Ok(no_store(
        json!({"repository":repository,"backupStatus":"queued"}),
    ))
}

pub async fn request_sync(
    State(app): State<App>,
    h: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    auth(&app, &h)?;
    if read_repository(&app.data)
        .map_err(|reason| error(StatusCode::SERVICE_UNAVAILABLE, reason))?
        .is_none()
    {
        return Err(error(StatusCode::CONFLICT, "repository_not_selected"));
    }
    app.backup.wake();
    Ok(no_store(json!({"state":"queued"})))
}

/// Import portable chat copies without replacing the current archive or files.
/// The caller keeps maintenance enabled until the operation has completed.
pub async fn restore_chats(
    State(app): State<App>,
    h: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    auth(&app, &h)?;
    let (maintenance, admitted, _) = maintenance::observe(&app)?;
    if !maintenance {
        return Err(error(StatusCode::CONFLICT, "maintenance_required"));
    }
    if admitted > 1
        || app.writes.load(Ordering::SeqCst) != 0
        || !app
            .active
            .lock()
            .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?
            .is_empty()
        || human_prompt::count(&app.human_inputs)? != 0
        || app.github_logins.load(Ordering::SeqCst) != 0
        || app
            .login
            .lock()
            .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?
            .as_ref()
            .is_some_and(|login| matches!(login.state, "starting" | "waiting"))
        || db(&app)?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM approvals WHERE state='pending')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?
    {
        return Err(error(StatusCode::CONFLICT, "work_in_progress"));
    }
    let client = client(&app).await?;
    let work = app.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _selection = work
            .backup
            .selection
            .lock()
            .map_err(|_| "backup_unavailable")?;
        let repository = read_repository(&work.data)?.ok_or("repository_not_selected")?;
        let temp = tempfile::Builder::new()
            .prefix(".chat-receive-")
            .tempdir_in(&work.data)
            .map_err(|_| "storage_unavailable")?;
        let destination = temp.path().join("archive");
        client.restore(&destination, &repository)?;
        let (remote_head, remote_files) =
            super::github_restore_files::prepare(&destination, &work.data.join("archive"))?;
        let groups = remote_files
            .iter()
            .find(|(name, _)| name == "chat-groups.json")
            .map(|(_, bytes)| {
                if bytes.len() > 4 * 1024 * 1024 {
                    return Err("invalid_snapshot");
                }
                serde_json::from_slice::<Value>(bytes).map_err(|_| "invalid_snapshot")
            })
            .transpose()?;
        let chats = destination.join("chats");
        let documents_dir = destination.join("zettels");
        let mut documents = Vec::new();
        match std::fs::symlink_metadata(&documents_dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Ok(meta) if meta.file_type().is_dir() => {
                for entry in std::fs::read_dir(&documents_dir).map_err(|_| "storage_unavailable")? {
                    let entry = entry.map_err(|_| "storage_unavailable")?;
                    let name = entry
                        .file_name()
                        .into_string()
                        .map_err(|_| "invalid_document")?;
                    let Some(id) = name.strip_suffix(".md") else {
                        continue;
                    };
                    if !momso_core::document::valid_zet_id(id) {
                        continue;
                    }
                    let bytes =
                        chat_backup::read_regular(&entry.path())?.ok_or("invalid_document")?;
                    let text = String::from_utf8(bytes).map_err(|_| "invalid_document")?;
                    let (head, _) =
                        momso_core::document::parse(&text).map_err(|_| "invalid_document")?;
                    if head.id != id {
                        return Err("invalid_document");
                    }
                    let archive = work.data.join("archive");
                    match momso_core::filesystem::read_document(&archive, id) {
                        Ok(existing) if existing == text => (),
                        Err("not_found") => (),
                        Ok(_) => return Err("restore_conflict"),
                        Err(_) => return Err("invalid_document"),
                    }
                    documents.push((id.to_owned(), text));
                }
            }
            _ => return Err("invalid_document"),
        }
        documents.sort_by(|a, b| a.0.cmp(&b.0));
        let mut snapshots = Vec::new();
        let mut asset_links = Vec::new();
        let mut missing_assets = Vec::new();
        let mut generated_assets = Vec::new();
        match std::fs::symlink_metadata(&chats) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Ok(meta) if meta.file_type().is_dir() => {
                for entry in std::fs::read_dir(&chats).map_err(|_| "storage_unavailable")? {
                    let entry = entry.map_err(|_| "storage_unavailable")?;
                    let name = entry
                        .file_name()
                        .into_string()
                        .map_err(|_| "invalid_snapshot")?;
                    let expected = name.strip_suffix(".json").ok_or("invalid_snapshot")?;
                    let bytes =
                        chat_backup::read_regular(&entry.path())?.ok_or("invalid_snapshot")?;
                    let snapshot: Value =
                        serde_json::from_slice(&bytes).map_err(|_| "invalid_snapshot")?;
                    if snapshot.pointer("/room/id").and_then(Value::as_str) != Some(expected) {
                        return Err("invalid_snapshot");
                    }
                    let chat = expected.strip_prefix("chat_").ok_or("invalid_snapshot")?;
                    let conn = db(&work).map_err(|_| "storage_error")?;
                    if snapshot.get("deleted").is_some() {
                        let live: bool = conn
                            .query_row(
                                "SELECT EXISTS(SELECT 1 FROM chats WHERE id=?1)",
                                [chat],
                                |r| r.get(0),
                            )
                            .map_err(|_| "storage_error")?;
                        if live {
                            return Err("restore_conflict");
                        }
                        snapshots.push(snapshot);
                        continue;
                    }
                    let deleted: bool = conn
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM chat_deletions WHERE chat_id=?1)",
                            [chat],
                            |r| r.get(0),
                        )
                        .map_err(|_| "storage_error")?;
                    if deleted {
                        return Err("restore_conflict");
                    }
                    let assets = snapshot
                        .pointer("/server/attachments")
                        .and_then(Value::as_array)
                        .ok_or("unsupported_snapshot")?;
                    for item in assets {
                        let id = item
                            .get("assetId")
                            .and_then(Value::as_str)
                            .ok_or("invalid_snapshot")?;
                        let size = item
                            .get("byteLength")
                            .and_then(Value::as_u64)
                            .ok_or("invalid_snapshot")?;
                        match attachment_bridge::verify_restored_asset(&work, id, size) {
                            Ok(()) => (),
                            Err("not_found" | "storage_unavailable") => {
                                missing_assets.push((id.to_owned(), size))
                            }
                            Err(_) => return Err("attachments_restore_unavailable"),
                        }
                        asset_links.push((chat.to_owned(), id.to_owned()));
                    }
                    if let Some(generated) = snapshot.pointer("/server/generated") {
                        for item in generated.as_array().ok_or("invalid_snapshot")? {
                            let id = item
                                .get("assetId")
                                .and_then(Value::as_str)
                                .ok_or("invalid_snapshot")?;
                            let size = item
                                .get("byteLength")
                                .and_then(Value::as_u64)
                                .ok_or("invalid_snapshot")?;
                            let turn = item
                                .get("turnId")
                                .and_then(Value::as_str)
                                .ok_or("invalid_snapshot")?;
                            let message = snapshot
                                .pointer("/server/turns")
                                .and_then(Value::as_array)
                                .and_then(|values| {
                                    values
                                        .iter()
                                        .find(|value| value["id"].as_str() == Some(turn))
                                })
                                .and_then(|value| value["userMessageId"].as_str())
                                .ok_or("invalid_snapshot")?;
                            generated_assets.push((
                                id.to_owned(),
                                chat.to_owned(),
                                message.to_owned(),
                            ));
                            match attachment_bridge::verify_restored_asset(&work, id, size) {
                                Ok(()) => (),
                                Err("not_found" | "storage_unavailable") => {
                                    missing_assets.push((id.to_owned(), size))
                                }
                                Err(_) => return Err("attachments_restore_unavailable"),
                            }
                        }
                    }
                    snapshots.push(snapshot);
                }
            }
            _ => return Err("invalid_snapshot"),
        }
        if !missing_assets.is_empty() {
            let mut ids: Vec<String> = missing_assets.iter().map(|(id, _)| id.clone()).collect();
            ids.sort();
            ids.dedup();
            super::github_worker::restore_assets(&work, &ids)?;
            for (id, size) in &missing_assets {
                attachment_bridge::verify_restored_asset(&work, id, *size)
                    .map_err(|_| "attachments_restore_unavailable")?;
            }
        }
        for (asset, chat, message) in &generated_assets {
            attachment_bridge::verify_restored_generation(&work, asset, chat, message)
                .map_err(|_| "generation_restore_conflict")?;
        }
        snapshots.sort_by(|a, b| {
            a.pointer("/room/id")
                .and_then(Value::as_str)
                .cmp(&b.pointer("/room/id").and_then(Value::as_str))
        });
        let serialized = serde_json::to_vec(&(&snapshots, &documents, &remote_head))
            .map_err(|_| "invalid_snapshot")?;
        use sha2::{Digest, Sha256};
        let digest = format!("{:x}", Sha256::digest(&serialized));
        let previous_digest = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(&snapshots, &documents)).map_err(|_| "invalid_snapshot")?
            )
        );
        let legacy_digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&snapshots).map_err(|_| "invalid_snapshot")?)
        );
        let marker = work.data.join("chat-restore.pending");
        let created_marker = match chat_backup::read_regular(&marker)? {
            Some(previous)
                if previous == digest.as_bytes()
                    || (groups.is_none() && previous == previous_digest.as_bytes())
                    || (groups.is_none()
                        && documents.is_empty()
                        && previous == legacy_digest.as_bytes()) =>
            {
                false
            }
            Some(_) => return Err("restore_changed"),
            None => {
                use std::io::Write;
                let mut temp = tempfile::NamedTempFile::new_in(&work.data)
                    .map_err(|_| "storage_unavailable")?;
                temp.write_all(digest.as_bytes())
                    .map_err(|_| "storage_unavailable")?;
                temp.as_file()
                    .sync_all()
                    .map_err(|_| "storage_unavailable")?;
                temp.persist_noclobber(&marker)
                    .map_err(|_| "storage_unavailable")?;
                std::fs::File::open(&work.data)
                    .and_then(|dir| dir.sync_all())
                    .map_err(|_| "storage_unavailable")?;
                true
            }
        };
        super::github_restore_files::copy(&work.data.join("archive"), &remote_files)?;
        for (id, text) in &documents {
            let archive = work.data.join("archive");
            match momso_core::filesystem::read_document(&archive, id) {
                Ok(existing) if existing == *text => continue,
                Err("not_found") => (),
                _ => return Err("restore_conflict"),
            }
            let (head, body) = momso_core::document::parse(text).map_err(|_| "invalid_document")?;
            momso_core::filesystem::create_document(&archive, &head, body)
                .map_err(|_| "restore_conflict")?;
        }
        let mut connection = db(&work).map_err(|_| "storage_error")?;
        if let Err(reason) =
            chat_backup::restore_bundle(&mut connection, &snapshots, groups.as_ref())
        {
            if created_marker && documents.is_empty() && remote_files.is_empty() {
                std::fs::remove_file(&marker).map_err(|_| "storage_unavailable")?;
                std::fs::File::open(&work.data)
                    .and_then(|dir| dir.sync_all())
                    .map_err(|_| "storage_unavailable")?;
            }
            return Err(reason);
        }
        for (chat, asset) in asset_links {
            attachment_bridge::restore_link(&work, &chat, &asset)
                .map_err(|_| "attachments_restore_unavailable")?;
        }
        let adopt = work.data.join("github-adopt.pending");
        let record = serde_json::to_vec(
            &json!({"repository": repository.name_with_owner, "head":remote_head}),
        )
        .map_err(|_| "storage_unavailable")?;
        let replace = match chat_backup::read_regular(&adopt)? {
            Some(existing) if existing == record => None,
            Some(existing) => {
                let old: Value =
                    serde_json::from_slice(&existing).map_err(|_| "restore_changed")?;
                let new: Value = serde_json::from_slice(&record).map_err(|_| "restore_changed")?;
                if old.get("repository") != new.get("repository")
                    || old.get("repository").and_then(Value::as_str).is_none()
                {
                    return Err("restore_changed");
                }
                Some(true)
            }
            None => Some(false),
        };
        if let Some(replace) = replace {
            use std::io::Write;
            let mut tmp =
                tempfile::NamedTempFile::new_in(&work.data).map_err(|_| "storage_unavailable")?;
            tmp.write_all(&record)
                .and_then(|()| tmp.as_file().sync_all())
                .map_err(|_| "storage_unavailable")?;
            if replace {
                tmp.persist(&adopt).map_err(|_| "storage_unavailable")?;
            } else {
                tmp.persist_noclobber(&adopt)
                    .map_err(|_| "storage_unavailable")?;
            }
            std::fs::File::open(&work.data)
                .and_then(|dir| dir.sync_all())
                .map_err(|_| "storage_unavailable")?;
        }
        std::fs::remove_file(&marker).map_err(|_| "storage_unavailable")?;
        std::fs::File::open(&work.data)
            .and_then(|dir| dir.sync_all())
            .map_err(|_| "storage_unavailable")?;
        let deleted_count = snapshots
            .iter()
            .filter(|s| s.get("deleted").is_some())
            .count();
        Ok::<_, &'static str>((
            snapshots.len() - deleted_count,
            documents.len(),
            deleted_count,
        ))
    })
    .await
    .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
    .map_err(|reason| error(StatusCode::CONFLICT, reason))?;
    app.backup.wake();
    Ok(no_store(
        json!({"restoredChats":result.0,"documentsRestored":true,"restoredDocuments":result.1,"attachmentsRestored":true,"restoredDeletions":result.2}),
    ))
}
