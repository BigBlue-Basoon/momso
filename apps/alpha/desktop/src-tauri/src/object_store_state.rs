use crate::{
    credentials, object_store, object_store_restore, object_store_transfer as transfer,
    runtime::RuntimePaths, storage, AppState,
};
use age::{secrecy::ExposeSecret, x25519};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppStateBackupResult {
    state: &'static str,
    created_at: String,
    size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppStateRestoreResult {
    state: &'static str,
    snapshot_at: String,
    safety_backup_created: bool,
}

async fn backup_snapshot(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    paths: &RuntimePaths,
) -> Result<transfer::Manifest, &'static str> {
    let temp = tempfile::Builder::new()
        .prefix("momso-state-")
        .tempdir_in(&paths.backups_path)
        .map_err(|_| "local_write_failed")?;
    let snapshot = temp.path().join("momso.db");
    storage::create_snapshot_file(&paths.database_path, &snapshot)
        .map_err(|_| "snapshot_failed")?;
    storage::sanitize_user_owned_backup_snapshot(&snapshot).map_err(|_| "snapshot_failed")?;
    let stage = paths
        .data_root
        .join("object-store-staging")
        .join(&config.credential_id);
    let manifest = transfer::upload_file(
        config,
        keys,
        identity,
        transfer::UploadSpec {
            source: &snapshot,
            relative: "momso.db",
            kind: "app_state",
            stage: &stage,
        },
        |_, _| {},
    )
    .await?;
    if std::fs::read_dir(&stage)
        .map_err(|_| "local_read_failed")?
        .next()
        .is_none()
    {
        std::fs::remove_dir(&stage).map_err(|_| "local_write_failed")?;
        if let Some(parent) = stage.parent() {
            if std::fs::read_dir(parent)
                .map_err(|_| "local_read_failed")?
                .next()
                .is_none()
            {
                std::fs::remove_dir(parent).map_err(|_| "local_write_failed")?;
            }
        }
    }
    Ok(manifest)
}

fn snapshot_workspace_id(path: &Path) -> Result<String, &'static str> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "invalid_snapshot")?;
    db.query_row(
        "SELECT workspace_id FROM momso_backup_profiles ORDER BY created_at ASC LIMIT 1",
        [],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|_| "invalid_snapshot")?
    .ok_or("invalid_snapshot")
}

fn preserve_recovery_identity(
    snapshot: &Path,
    identity: &x25519::Identity,
) -> Result<(), &'static str> {
    preserve_recovery_identity_with(
        snapshot,
        identity,
        |id| credentials::get_backup_identity(id).map_err(|_| "unavailable"),
        |id, key| credentials::import_backup_identity(id, key).map_err(|_| "unavailable"),
    )
}

fn preserve_recovery_identity_with(
    snapshot: &Path,
    identity: &x25519::Identity,
    get: impl FnOnce(&str) -> Result<Option<x25519::Identity>, &'static str>,
    put: impl FnOnce(&str, &str) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let workspace_id = snapshot_workspace_id(snapshot)?;
    match get(&workspace_id)? {
        Some(existing) if existing.to_public().to_string() != identity.to_public().to_string() => {
            Err("recovery_key_conflict")
        }
        Some(_) => Ok(()),
        None => put(&workspace_id, identity.to_string().expose_secret()),
    }
}

fn apply_snapshot(snapshot: &Path, paths: &RuntimePaths) -> Result<PathBuf, &'static str> {
    storage::validate_snapshot_file(snapshot).map_err(|_| "invalid_snapshot")?;
    let root = paths.objects_path.parent().ok_or("invalid_snapshot")?;
    let path = root.to_str().ok_or("invalid_snapshot")?;
    let db = Connection::open(snapshot).map_err(|_| "invalid_snapshot")?;
    db.execute("INSERT INTO archive_location_settings(id,path,initialized) VALUES(1,?1,1) ON CONFLICT(id) DO UPDATE SET path=excluded.path,initialized=1", [path]).map_err(|_| "invalid_snapshot")?;
    drop(db);
    crate::archive::reindex(root, snapshot).map_err(|_| "invalid_snapshot")?;
    storage::restore_snapshot_file(snapshot, &paths.database_path, &paths.backups_path).map_err(
        |error| match error {
            storage::StorageError::RestoreRollbackFailed => "restore_rollback_failed",
            _ => "restore_failed",
        },
    )
}

#[tauri::command]
pub(crate) async fn app_state_backup(
    state: State<'_, AppState>,
) -> Result<AppStateBackupResult, &'static str> {
    let paths = state.paths.scoped().await;
    let _guard = object_store::CONNECTION_LOCK.lock().await;
    let config = object_store::load_config(&paths.data_root)?.ok_or("not_connected")?;
    let keys = object_store::load_keys(&config)?;
    let identity = transfer::identity(&paths.database_path)?;
    let manifest = backup_snapshot(&config, &keys, &identity, &paths).await?;
    Ok(AppStateBackupResult {
        state: "complete",
        created_at: manifest.created_at,
        size: manifest.size,
    })
}

#[tauri::command]
pub(crate) async fn app_state_restore(
    state: State<'_, AppState>,
) -> Result<AppStateRestoreResult, &'static str> {
    let location = state.paths.archive.current.clone().write_owned().await;
    let mut paths = state.paths.clone();
    paths.objects_path = location.join("objects");
    let _guard = object_store::CONNECTION_LOCK.lock().await;
    let config = object_store::load_config(&paths.data_root)?.ok_or("not_connected")?;
    let keys = object_store::load_keys(&config)?;
    let identity = transfer::identity(&paths.database_path)?;
    let (snapshot, manifest) = download_latest_snapshot(&config, &keys, &identity, &paths).await?;
    preserve_recovery_identity(snapshot.path(), &identity)?;
    apply_snapshot(snapshot.path(), &paths)?;
    Ok(AppStateRestoreResult {
        state: "restored",
        snapshot_at: manifest.created_at,
        safety_backup_created: true,
    })
}

async fn download_latest_snapshot(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    paths: &RuntimePaths,
) -> Result<(tempfile::NamedTempFile, transfer::Manifest), &'static str> {
    let manifest = transfer::catalog(config, keys, identity)
        .await?
        .into_iter()
        .filter(|item| item.kind == "app_state" && item.relative_path == "momso.db")
        .max_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then(a.upload_id.cmp(&b.upload_id))
        })
        .ok_or("remote_missing")?;
    let snapshot = object_store_restore::download_verified(
        config,
        keys,
        identity,
        &manifest,
        &paths.backups_path,
    )
    .await?;
    storage::validate_snapshot_file(snapshot.path()).map_err(|_| "invalid_snapshot")?;
    Ok((snapshot, manifest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        fs,
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
    };

    async fn fake_skill_connection(chat_id: &str, cancel_first: bool) -> Option<String> {
        let action = serde_json::json!({"method":"object_store_r2_connect", "account":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"});
        let (service, account, region) =
            crate::setup_actions::storage_account(&action).expect("stage 5: skill action");
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let secrets = crate::setup_secret::SecretBroker::in_memory();
        let approvals = crate::approval::ApprovalBroker::default();
        let secret_events = events.clone();
        let approval_events = events.clone();
        let connection_events = events.clone();
        let secret_chat = chat_id.to_owned();
        let approval_chat = chat_id.to_owned();
        let connection_chat = chat_id.to_owned();
        let selected = Arc::new(Mutex::new(None::<String>));
        let connection_selected = selected.clone();
        let outcome = crate::setup_actions::run_storage_flow(
            chat_id,
            "teacher",
            service,
            account,
            region,
            "momso-teacher",
            tokio_util::sync::CancellationToken::new(),
            move |request| {
                let broker = secrets.clone();
                let responder = broker.clone();
                let events = secret_events.clone();
                let chat = secret_chat.clone();
                async move {
                    broker
                        .request_with(
                            request,
                            tokio_util::sync::CancellationToken::new(),
                            move |event| {
                                events.lock().unwrap().push("secret".into());
                                assert_eq!(event.resource_name, "momso-teacher");
                                let id = event.request_id.clone();
                                tokio::spawn(async move {
                                    if cancel_first {
                                        responder.cancel(&id, &chat).await.unwrap();
                                    } else {
                                        responder
                                            .submit(
                                                &id,
                                                &chat,
                                                crate::setup_secret::SecretInput {
                                                    access_key_id: "fixture-id".into(),
                                                    secret_access_key: "fixture-secret".into(),
                                                    gcp_oauth_token: None,
                                                },
                                            )
                                            .await
                                            .unwrap();
                                    }
                                });
                                true
                            },
                            |_| {},
                        )
                        .await
                }
            },
            move |plan| {
                let broker = approvals.clone();
                let responder = broker.clone();
                let events = approval_events.clone();
                let chat = approval_chat.clone();
                async move {
                    let expected_chat = chat.clone();
                    plan.authorize_with(
                        &broker,
                        &chat,
                        "codex",
                        tokio_util::sync::CancellationToken::new(),
                        move |event| {
                            let card = serde_json::to_value(event).unwrap();
                            assert_eq!(card["chatId"], expected_chat);
                            assert_eq!(card["setup"]["resourceName"], "momso-teacher");
                            events.lock().unwrap().push("approval".into());
                            let id = card["requestId"].as_str().unwrap().to_owned();
                            tokio::spawn(async move {
                                responder
                                    .respond(&id, crate::approval::ApprovalChoice::AllowOnce)
                                    .await
                                    .unwrap();
                            });
                            true
                        },
                        |_| {},
                    )
                    .await
                }
            },
            move |permit, secret| {
                let chat = connection_chat.clone();
                let events = connection_events.clone();
                let selected = connection_selected.clone();
                async move {
                    let details = permit.consume(&chat)?;
                    assert_eq!(details.resource_name, "momso-teacher");
                    assert_eq!(secret.take()?.access_key_id, "fixture-id");
                    *selected.lock().unwrap() = Some(details.resource_name);
                    events.lock().unwrap().push("connected".into());
                    Ok(())
                }
            },
        )
        .await;
        if cancel_first {
            assert_eq!(outcome["status"], "cancelled");
            assert_eq!(*events.lock().unwrap(), vec!["secret"]);
        } else {
            assert_eq!(outcome["status"], "connected");
            assert_eq!(
                *events.lock().unwrap(),
                vec!["secret", "approval", "connected"]
            );
        }
        let result = selected.lock().unwrap().clone();
        result
    }

    #[tokio::test]
    async fn sqlite_snapshot_round_trip_preserves_current_state_before_restore() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let database_path = root.join("momso.db");
        let backups_path = root.join("backups");
        fs::create_dir_all(&backups_path).unwrap();
        storage::initialize(&database_path, &backups_path, "test").unwrap();
        let archive =
            crate::archive_location::ArchiveLocation::load(database_path.clone(), root, vec![])
                .unwrap();
        let archive_root = root.join("momso");
        crate::archive::initialize(&archive_root, root).unwrap();
        crate::archive::profile_set(
            &archive_root,
            root,
            "Teacher".into(),
            "Yoga instructor".into(),
            "Use short answers".into(),
        )
        .expect("stage 1: profile and rule files");
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
        let username = serde_json::to_value(crate::archive_remote::username_set_db(
            &paths.database_path,
            "teacher".into(),
        ))
        .unwrap();
        assert_eq!(username["state"], "ok", "stage 1: username");
        let original_id = crate::archive_zet::create(
            &archive_root,
            &paths.database_path,
            crate::archive_zet::ZetCreate {
                title: "original".into(),
                summary: "original summary".into(),
                kind: "note".into(),
                body: "original body".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .expect("stage 1: seed document");
        let new_id = crate::archive_zet::create(
            &archive_root,
            &paths.database_path,
            crate::archive_zet::ZetCreate {
                title: "created".into(),
                summary: "created summary".into(),
                kind: "note".into(),
                body: "created body".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .expect("stage 1: create valid document template");
        let new_path = archive_root.join("zettels").join(format!("{new_id}.md"));
        let template_path = root.join("new-document-template.txt");
        fs::copy(&new_path, &template_path).unwrap();
        fs::remove_file(&new_path).unwrap();
        let room =
            serde_json::to_value(crate::chat::create(&paths.database_path).unwrap()).unwrap();
        let chat_id = room["id"].as_str().unwrap().to_owned();
        let sent = crate::chat::append(
            &paths.database_path,
            &chat_id,
            crate::chat::Role::User,
            "hello",
            None,
            None,
        )
        .expect("stage 2: chat message");
        let script = root.join("fake-codex.ps1");
        fs::write(&script, r#"
$null = [Console]::ReadLine()
'{"id":1,"result":{}}'
$null = [Console]::ReadLine()
$null = [Console]::ReadLine()
'{"id":2,"result":{"thread":{"id":"fixture-thread"},"approvalPolicy":"on-request","approvalsReviewer":"user","sandbox":{"type":"workspaceWrite"},"model":"fixture-model","reasoningEffort":"low"}}'
$null = [Console]::ReadLine()
$old = Join-Path 'zettels' ($env:FIXTURE_OLD + '.md')
$new = Join-Path 'zettels' ($env:FIXTURE_NEW + '.md')
$utf8 = [Text.UTF8Encoding]::new($false)
[IO.File]::WriteAllText($old, ([IO.File]::ReadAllText($old).Replace('original body', 'AI edited body')), $utf8)
[IO.File]::Copy($env:FIXTURE_TEMPLATE, $new)
$change = @{method='item/completed';params=@{threadId='fixture-thread';item=@{id='file-write';type='fileChange';status='completed';changes=@(@{path=$old;kind=@{type='update'}},@{path=$new;kind=@{type='add'}})}}} | ConvertTo-Json -Depth 12 -Compress
[Console]::WriteLine($change)
'{"method":"thread/tokenUsage/updated","params":{"threadId":"fixture-thread","tokenUsage":{"last":{"totalTokens":12},"modelContextWindow":1000}}}'
'{"method":"item/agentMessage/delta","params":{"delta":"hello teacher"}}'
'{"method":"turn/completed","params":{"turn":{"status":"completed"}}}'
"#).unwrap();
        let mut command = tokio::process::Command::new("powershell.exe");
        command
            .args(["-NoProfile", "-NonInteractive", "-File"])
            .arg(&script)
            .env("FIXTURE_OLD", &original_id)
            .env("FIXTURE_NEW", &new_id)
            .env("FIXTURE_TEMPLATE", &template_path)
            .current_dir(&archive_root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut events = Vec::new();
        crate::chat_codex::run_command(
            command,
            &paths.database_path,
            &archive_root,
            &chat_id,
            "hello",
            None,
            None,
            None,
            None,
            None,
            &crate::permission_modes::codex_settings("Ask for approval").unwrap(),
            None,
            tokio_util::sync::CancellationToken::new(),
            None,
            None,
            |name, value| events.push((name.to_owned(), value)),
        )
        .await
        .expect("stage 2: fake CLI reply");
        assert!(
            fs::read_to_string(
                archive_root
                    .join("zettels")
                    .join(format!("{original_id}.md"))
            )
            .unwrap()
            .contains("AI edited body"),
            "stage 4: fake AI edits document"
        );
        assert!(new_path.is_file(), "stage 4: fake AI creates document");
        let done = events
            .iter()
            .find(|(name, _)| name == "chat-done")
            .expect("stage 2: done event");
        assert_eq!(done.1["finishReason"], "completed");
        assert!(done.1.get("fastState").is_some());
        assert_eq!(
            serde_json::to_value(crate::chat_context::get(&paths.database_path, &chat_id).unwrap())
                .unwrap()["usedTokens"],
            12
        );
        assert!(
            !serde_json::to_value(crate::chat::read(&paths.database_path, &chat_id).unwrap())
                .unwrap()["title"]
                .as_str()
                .unwrap()
                .is_empty()
        );
        let text_file = root.join("lesson.txt");
        fs::write(&text_file, "original bytes").unwrap();
        let proposal_json = serde_json::json!({"sourcePath": text_file, "assetKind": "raw_transcript", "mediaType": "text/plain", "recordLink": null, "actor": "test"});
        let proposal = serde_json::from_value(proposal_json.clone()).unwrap();
        let plan = serde_json::to_value(
            crate::source_assets::plan_import(&paths.database_path, &paths.objects_path, proposal)
                .expect("stage 3: plan attachment"),
        )
        .unwrap();
        let input = serde_json::from_value(serde_json::json!({"proposal": proposal_json, "planDigest": plan["planDigest"], "importConfirmed": true})).unwrap();
        let asset = serde_json::to_value(
            crate::source_assets::apply_import(&paths.database_path, &paths.objects_path, input)
                .expect("stage 3: import attachment"),
        )
        .unwrap();
        let hash = asset["contentSha256"].as_str().unwrap();
        let reference = format!("sha256:{hash}");
        let object_relative = format!("sha256/{}/{hash}", &hash[..2]);
        crate::archive_receipt::attach_asset(
            &archive_root,
            &paths.database_path,
            &chat_id,
            asset["id"].as_str().unwrap(),
        )
        .expect("stage 3: attach");
        let page = serde_json::to_value(
            crate::chat_items::read_attachment_text(
                &archive_root,
                &paths.database_path,
                &reference,
                0,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            page["content"], "original bytes",
            "stage 3: attachment text"
        );
        let id = crate::archive_zet::create(
            &archive_root,
            &paths.database_path,
            crate::archive_zet::ZetCreate {
                title: "document".into(),
                summary: "summary".into(),
                kind: "note".into(),
                body: "body".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        let tool = crate::process::resolve_tool("git.exe").unwrap();
        let bare = root.join("remote.git");
        fs::write(
            archive_root.join("dictionary/people.json"),
            r#"{"people":[{"name":"fixture-person"}]}"#,
        )
        .unwrap();
        crate::archive_remote::git(&tool, root, &["init", "--bare", bare.to_str().unwrap()])
            .unwrap();
        crate::archive_remote::git(
            &tool,
            &archive_root,
            &[
                "add",
                "--",
                ".gitignore",
                "AGENTS.md",
                "CLAUDE.md",
                "GEMINI.md",
                "zettels/",
                "receipts/",
                "dictionary/",
                "skills/",
                ".agents/skills/",
                ".claude/skills/",
            ],
        )
        .unwrap();
        crate::archive_remote::git(
            &tool,
            &archive_root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "original",
            ],
        )
        .unwrap();
        crate::archive::save_one(
            &archive_root,
            root,
            &paths.database_path,
            &id,
            crate::archive_zet::ZetUpdate {
                title: None,
                summary: None,
                body: Some("edited body".into()),
                status: None,
                assets: None,
                edges: None,
            },
            None,
        )
        .expect("stage 4: save edited document");
        assert!(
            crate::archive::history(&archive_root, root, &id)
                .unwrap()
                .len()
                >= 2,
            "stage 4: history"
        );
        let revision =
            crate::archive_remote::git(&tool, &archive_root, &["rev-parse", "HEAD"]).unwrap();
        let document_path = archive_root.join("zettels").join(format!("{id}.md"));
        fs::remove_file(&document_path).unwrap();
        crate::chat_recovery::restore_git(&archive_root, root, &id, revision.trim())
            .expect("stage 4: recover deleted document");
        assert!(
            fs::read_to_string(&document_path)
                .unwrap()
                .contains("edited body"),
            "stage 4: restored content"
        );
        assert!(
            archive_root
                .join("skills/object-store-connect/SKILL.md")
                .is_file(),
            "stage 5: builtin skill"
        );
        assert!(fake_skill_connection(&chat_id, true).await.is_none());
        let selected_bucket = fake_skill_connection(&chat_id, false)
            .await
            .expect("stage 5: selected fake bucket");
        crate::archive_remote::transfer(
            &tool,
            &archive_root,
            bare.to_str().unwrap(),
            "main",
            "push",
        )
        .unwrap();
        let profile = crate::backup_profile::ensure(&paths.database_path).unwrap();
        let db = storage::open_writable_connection(&paths.database_path).unwrap();
        db.execute("INSERT INTO app_runtime_meta(key,value,updated_at) VALUES('test_state','before','now')", []).unwrap();
        db.execute(
            "UPDATE chats SET pinned_order=7,archived_at='archived' WHERE id=?1",
            [&chat_id],
        )
        .unwrap();
        db.execute(
            "UPDATE chat_messages SET fast=1,retryable_fast=1 WHERE id=?1",
            [&sent.id],
        )
        .unwrap();
        db.execute("INSERT INTO archive_location_settings(id,path,initialized) VALUES(1,'foreign-machine',1) ON CONFLICT(id) DO UPDATE SET path='foreign-machine'", []).unwrap();
        drop(db);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let objects = Arc::new(Mutex::new(HashMap::<String, Vec<u8>>::new()));
        let remote = Arc::clone(&objects);
        let server_bucket = selected_bucket.clone();
        let server = std::thread::spawn(move || {
            for _ in 0..16 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(15)))
                    .unwrap();
                let mut received = Vec::new();
                let mut buffer = [0u8; 16384];
                let end = loop {
                    let n = stream.read(&mut buffer).unwrap();
                    assert_ne!(n, 0);
                    received.extend_from_slice(&buffer[..n]);
                    if let Some(at) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                };
                let head = String::from_utf8(received[..end].to_vec()).unwrap();
                let mut line = head.lines().next().unwrap().split_whitespace();
                let method = line.next().unwrap();
                let path = line.next().unwrap();
                let prefix = format!("/{server_bucket}/");
                let key = path.trim_start_matches(prefix.as_str());
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while received.len() - end < length {
                    let n = stream.read(&mut buffer).unwrap();
                    assert_ne!(n, 0);
                    received.extend_from_slice(&buffer[..n]);
                }
                let (status, body) = if method == "GET"
                    && path.starts_with(&format!("/{server_bucket}?"))
                {
                    let entries = remote
                        .lock()
                        .unwrap()
                        .keys()
                        .filter(|key| key.contains("/manifests/"))
                        .map(|key| format!("<Contents><Key>{key}</Key></Contents>"))
                        .collect::<String>();
                    ("200 OK", format!("<ListBucketResult><IsTruncated>false</IsTruncated>{entries}</ListBucketResult>").into_bytes())
                } else if method == "PUT" {
                    let mut data = remote.lock().unwrap();
                    assert!(!data.contains_key(key));
                    data.insert(key.to_owned(), received[end..end + length].to_vec());
                    ("200 OK", Vec::new())
                } else {
                    match remote.lock().unwrap().get(key) {
                        Some(data) => ("200 OK", data.clone()),
                        None => ("404 Not Found", Vec::new()),
                    }
                };
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let config = object_store::Config {
            provider: "s3_compatible".into(),
            endpoint: format!("http://{address}"),
            region: "auto".into(),
            bucket: selected_bucket,
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(object_store::WriteSafety::Conditional),
        };
        let keys = object_store::Keys {
            access_key_id: "TEST".into(),
            secret_access_key: "test".into(),
        };
        let identity = x25519::Identity::generate();
        let _original = transfer::upload_file(
            &config,
            &keys,
            &identity,
            transfer::UploadSpec {
                source: &paths.objects_path.join(&object_relative),
                relative: &object_relative,
                kind: "object",
                stage: &root.join("stage").join("upload"),
            },
            |_, _| {},
        )
        .await
        .unwrap();
        let _manifest = backup_snapshot(&config, &keys, &identity, &paths)
            .await
            .unwrap();
        let new_pc = root.join("new-pc");
        fs::create_dir_all(&new_pc).unwrap();
        let database_path = new_pc.join("momso.db");
        let backups_path = new_pc.join("backups");
        fs::create_dir_all(&backups_path).unwrap();
        storage::initialize(&database_path, &backups_path, "test").unwrap();
        let archive =
            crate::archive_location::ArchiveLocation::load(database_path.clone(), &new_pc, vec![])
                .unwrap();
        let archive_root = new_pc.join("momso");
        crate::archive::initialize(&archive_root, &new_pc).unwrap();
        crate::archive_remote::connect_local(&tool, &archive_root, bare.to_str().unwrap()).unwrap();
        crate::archive_remote::receive_connected(
            &tool,
            &archive_root,
            &database_path,
            bare.to_str().unwrap(),
            "main",
        )
        .unwrap();
        assert!(
            archive_root
                .join("zettels")
                .join(format!("{id}.md"))
                .is_file(),
            "stage 6: document from Git"
        );
        assert!(
            archive_root
                .join("skills/github-connect/SKILL.md")
                .is_file(),
            "stage 6: builtin skill from Git"
        );
        assert!(
            archive_root.join("dictionary/yoga.json").is_file(),
            "stage 6: bundled dictionary"
        );
        assert!(
            fs::read_to_string(archive_root.join("dictionary/people.json"))
                .unwrap()
                .contains("fixture-person"),
            "stage 6: people file"
        );
        let github_profile =
            serde_json::to_value(crate::archive::profile_get(&archive_root).unwrap()).unwrap();
        assert_eq!(
            github_profile["name"], "Teacher",
            "stage 6: profile from Git"
        );
        assert!(
            crate::chat::read(&database_path, &chat_id).is_err(),
            "stage 6: Git alone does not restore chats"
        );
        assert!(
            !archive_root.join("objects").join(&object_relative).exists(),
            "stage 6: Git alone does not restore originals"
        );
        let paths = RuntimePaths {
            data_root: new_pc.clone(),
            database_path,
            objects_path: archive_root.join("objects"),
            exports_path: new_pc.join("exports"),
            backups_path,
            logs_path: new_pc.join("logs"),
            archive,
            archive_lease: None,
        };
        crate::backup_profile::ensure(&paths.database_path).unwrap();
        let db = storage::open_writable_connection(&paths.database_path).unwrap();
        db.execute(
            "INSERT INTO app_runtime_meta(key,value,updated_at) VALUES('test_state','after','now')",
            [],
        )
        .unwrap();
        drop(db);
        let (downloaded, _) = download_latest_snapshot(&config, &keys, &identity, &paths)
            .await
            .unwrap();
        assert_eq!(
            snapshot_workspace_id(downloaded.path()).unwrap(),
            profile.workspace_id
        );
        let new_pc_keys = std::cell::RefCell::new(HashMap::<String, String>::new());
        preserve_recovery_identity_with(
            downloaded.path(),
            &identity,
            |id| Ok(new_pc_keys.borrow().get(id).map(|key| key.parse().unwrap())),
            |id, key| {
                new_pc_keys
                    .borrow_mut()
                    .insert(id.to_owned(), key.to_owned());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            new_pc_keys.borrow().get(&profile.workspace_id).unwrap(),
            identity.to_string().expose_secret()
        );
        let failure = storage::restore_snapshot_file_with_hook(
            downloaded.path(),
            &paths.database_path,
            &paths.backups_path,
            || {
                Err(storage::StorageError::Integrity(
                    "injected after replacement".into(),
                ))
            },
        );
        assert!(matches!(failure, Err(storage::StorageError::Integrity(_))));
        let after_failure = storage::open_writable_connection(&paths.database_path).unwrap();
        let value: String = after_failure
            .query_row(
                "SELECT value FROM app_runtime_meta WHERE key='test_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "after");
        let indexed: i64 = after_failure
            .query_row(
                "SELECT count(*) FROM archive_zets WHERE id=?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(indexed, 1);
        drop(after_failure);
        let safety = apply_snapshot(downloaded.path(), &paths).unwrap();
        let restored = storage::open_writable_connection(&paths.database_path).unwrap();
        let restored_path: String = restored
            .query_row(
                "SELECT path FROM archive_location_settings WHERE id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(std::path::Path::new(&restored_path), archive_root);
        let indexed: i64 = restored
            .query_row(
                "SELECT count(*) FROM archive_zets WHERE id=?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(indexed, 1);
        let value: String = restored
            .query_row(
                "SELECT value FROM app_runtime_meta WHERE key='test_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "before");
        let visibility: (i64, String) = restored
            .query_row(
                "SELECT pinned_order, archived_at FROM chats WHERE id=?1",
                [&chat_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(visibility, (7, "archived".into()));
        let message: (String, i64, i64) = restored
            .query_row(
                "SELECT text,fast,retryable_fast FROM chat_messages WHERE id=?1",
                [&sent.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(message, ("hello".into(), 1, 1));
        drop(restored);
        object_store_restore::restore_selected(&config, &keys, &identity, &paths, None)
            .await
            .unwrap();
        assert_eq!(
            fs::read(paths.objects_path.join(&object_relative)).unwrap(),
            b"original bytes"
        );
        assert_eq!(
            crate::chat::set_archived(&paths.database_path, &chat_id, false).state,
            "ok",
            "stage 7: restore room"
        );
        assert_eq!(
            crate::chat::set_pinned(&paths.database_path, &chat_id, true).state,
            "ok",
            "stage 7: pin room"
        );
        assert_eq!(
            crate::chat::set_archived(&paths.database_path, &chat_id, true).state,
            "ok",
            "stage 7: archive room"
        );
        assert_eq!(
            crate::chat::set_archived(&paths.database_path, &chat_id, false).state,
            "ok",
            "stage 7: unarchive room"
        );
        let deletion = crate::chat::delete_with(
            &paths.database_path,
            &Default::default(),
            &chat_id,
            |_| async { Ok(()) },
        )
        .await;
        assert_eq!(
            serde_json::to_value(deletion).unwrap()["state"],
            "deleted",
            "stage 7: delete room"
        );
        assert!(
            archive_root
                .join("zettels")
                .join(format!("{id}.md"))
                .is_file(),
            "stage 7: document remains"
        );
        assert_eq!(
            fs::read(paths.objects_path.join(&object_relative)).unwrap(),
            b"original bytes",
            "stage 7: original remains"
        );
        let chatgpt_export = new_pc.join("chatgpt.json");
        fs::write(&chatgpt_export, serde_json::json!([{"id":"fixture-chatgpt","current_node":"a","mapping":{"a":{"parent":null,"message":{"author":{"role":"user"},"content":{"parts":["hello"]}}}}}]).to_string()).unwrap();
        let claude_export = new_pc.join("claude.json");
        fs::write(&claude_export, serde_json::json!([{"uuid":"fixture-claude","name":"lesson","chat_messages":[{"sender":"human","text":"hello"}]}]).to_string()).unwrap();
        for (provider, file) in [("chatgpt", &chatgpt_export), ("claude", &claude_export)] {
            let result = serde_json::to_value(
                crate::chat_export_import::import(&paths.database_path, file, provider)
                    .expect("stage 8: import"),
            )
            .unwrap();
            assert_eq!(result["imported"], 1, "stage 8: {provider}");
        }
        assert_eq!(
            crate::chat::list(&paths.database_path).unwrap().len(),
            2,
            "stage 8: imported rooms"
        );
        let safety_db = Connection::open(safety).unwrap();
        let value: String = safety_db
            .query_row(
                "SELECT value FROM app_runtime_meta WHERE key='test_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "after");
        let safety_count = fs::read_dir(&paths.backups_path)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("pre-restore-")
                    && entry.path().extension().is_some_and(|ext| ext == "db")
            })
            .count();
        assert_eq!(safety_count, 2);
        assert!(objects
            .lock()
            .unwrap()
            .values()
            .all(|bytes| !bytes.windows(b"before".len()).any(|part| part == b"before")));
        server.join().unwrap();
    }
}
