//! Commit records for matching Git and encrypted database backups.
//!
//! A record may be published only after both components have been uploaded.
//! Its presence alone is not enough: restore also checks both components.
use crate::{object_store, object_store_transfer as transfer};
use serde::{Deserialize, Serialize};
use tauri::Manager;

const MAX_PAIR: usize = 16 * 1024;
static PAIR_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// When no object store is configured, keep the encrypted DB in Git blobs.
pub(crate) async fn capture_git_only<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<bool, &'static str> {
    let state = app.state::<crate::AppState>();
    if object_store::load_config(&state.paths.data_root)?.is_some()
        && crate::object_store_auto_upload::enabled(&state.paths.database_path)?
    {
        return Ok(false);
    }
    let pair_guard = PAIR_LOCK.lock().await;
    let root = state.paths.archive.current.clone().write_owned().await;
    let active = state.chat_active.clone().lock_owned().await;
    if !active.is_empty() {
        return Err("backup_busy");
    }
    let mut paths = state.paths.clone();
    paths.objects_path = root.join("objects");
    if object_store::load_config(&paths.data_root)?.is_some()
        && crate::object_store_auto_upload::enabled(&paths.database_path)?
    {
        return Ok(false);
    }
    if crate::archive_remote::first_receive_pending(&root, &paths.database_path) {
        return Err("first_receive_in_progress");
    }
    let (tool, target, revision) =
        match crate::archive_remote::paired_git_context(&app, &root, &paths.database_path, true) {
            Ok(value) => value,
            Err("not_connected" | "git_missing") => return Ok(false),
            Err(error) => return Err(error),
        };
    let identity = transfer::identity(&paths.database_path)?;
    let root_path = root.to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let _guards = (pair_guard, root, active);
        crate::paired_git_store::capture(&tool, &root_path, &target, &revision, &paths, &identity)
    })
    .await
    .map_err(|_| "snapshot_failed")??;
    Ok(true)
}

/// Capture only after the current Git copy has been uploaded. The archive lease
/// prevents app writes between validating the Git copy and taking the DB copy.
pub(crate) async fn capture<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Option<CompletedPair>, &'static str> {
    let _pair = PAIR_LOCK.lock().await;
    let state = app.state::<crate::AppState>();
    let root = state.paths.archive.current.clone().write_owned().await;
    let active = state.chat_active.lock().await;
    if !active.is_empty() {
        return Err("backup_busy");
    }
    let mut paths = state.paths.clone();
    paths.objects_path = root.join("objects");
    if crate::archive_remote::first_receive_pending(&root, &paths.database_path) {
        return Err("first_receive_in_progress");
    }
    let git = if crate::archive_auto_backup::enabled(&paths.database_path)? {
        match crate::archive_remote::paired_git_context(&app, &root, &paths.database_path, true) {
            Ok(value) => Some(value),
            Err("not_connected" | "git_missing") => None,
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    // Complete original files before advertising the associated DB backup.
    transfer::backup_with_paths(app.clone(), paths.clone(), true).await?;
    let _connection = object_store::CONNECTION_LOCK.lock().await;
    let config = object_store::load_config(&paths.data_root)?.ok_or("not_connected")?;
    let keys = object_store::load_keys(&config)?;
    let identity = transfer::identity(&paths.database_path)?;
    let manifest = crate::object_store_state::backup_paired_snapshot(
        &config,
        &keys,
        &identity,
        &paths,
        git.is_some(),
    )
    .await?;
    let Some((tool, target, revision)) = git else {
        return Ok(None);
    };
    let (_, _, verified_revision) =
        crate::archive_remote::paired_git_context(&app, &root, &paths.database_path, true)?;
    if verified_revision != revision {
        return Err("backup_pending_git");
    }
    let pair = CompletedPair {
        version: 1,
        pair_id: uuid::Uuid::new_v4().to_string(),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        git_revision: revision,
        snapshot_upload_id: manifest.upload_id,
    };
    publish_git_ref(&tool, &root, &target, &pair)?;
    publish(&config, &keys, &identity, &pair).await?;
    Ok(Some(pair))
}

pub(crate) async fn verify_referenced_originals(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &age::x25519::Identity,
    database: &std::path::Path,
) -> Result<(), &'static str> {
    let assets = {
        let db = rusqlite::Connection::open_with_flags(
            database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|_| "local_state_unavailable")?;
        let mut statement = db.prepare("SELECT a.storage_relative_path,a.content_sha256,a.byte_length FROM momso_source_assets a LEFT JOIN source_asset_deletions d ON d.asset_id=a.id WHERE d.state IS NULL OR d.state IN ('pending','trashed')")
            .map_err(|_| "local_state_unavailable")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|_| "local_state_unavailable")?;
        rows.map(|row| {
            let (path, hash, size) = row.map_err(|_| "local_state_unavailable")?;
            Ok((
                path,
                hash,
                u64::try_from(size).map_err(|_| "local_state_unavailable")?,
            ))
        })
        .collect::<Result<Vec<_>, &'static str>>()?
    };
    if assets.is_empty() {
        return Ok(());
    }
    let manifests = transfer::catalog(config, keys, identity).await?;
    let available: std::collections::HashSet<_> =
        transfer::list_keys(config, keys, &format!("{}/data/", object_store::PREFIX))
            .await?
            .into_iter()
            .collect();
    if referenced_originals_complete(&assets, &manifests, &available) {
        Ok(())
    } else {
        Err("backup_original_missing")
    }
}

fn referenced_originals_complete(
    assets: &[(String, String, u64)],
    manifests: &[transfer::Manifest],
    available: &std::collections::HashSet<String>,
) -> bool {
    assets.iter().all(|(relative, hash, size)| {
        let relative = relative.replace('\\', "/");
        manifests.iter().any(|item| {
            item.kind == "object"
                && !item.chunks.is_empty()
                && item.relative_path == relative
                && item.sha256 == *hash
                && item.size == *size
                && item
                    .chunks
                    .iter()
                    .enumerate()
                    .all(|(index, _)| available.contains(&transfer::chunk_key(item, index)))
        })
    })
}

fn pair_prefix() -> String {
    format!("{}/completed-pairs/", object_store::PREFIX)
}

fn pair_ref(id: &str) -> Result<String, &'static str> {
    let parsed = uuid::Uuid::parse_str(id).map_err(|_| "invalid_backup_pair")?;
    if parsed.to_string() != id {
        return Err("invalid_backup_pair");
    }
    Ok(format!("refs/tags/momso-backup/{id}"))
}

/// The Git side is an immutable annotated tag on the already-pushed commit.
pub(crate) fn publish_git_ref(
    tool: &std::path::Path,
    root: &std::path::Path,
    target: &str,
    pair: &CompletedPair,
) -> Result<(), &'static str> {
    if !pair.valid() {
        return Err("invalid_backup_pair");
    }
    let reference = pair_ref(&pair.pair_id)?;
    let name = reference
        .strip_prefix("refs/tags/")
        .ok_or("invalid_backup_pair")?;
    let message =
        serde_json::json!({"pairId": &pair.pair_id, "createdAt": &pair.created_at}).to_string();
    crate::archive_remote::git(
        tool,
        root,
        &[
            "-c",
            "user.name=momso",
            "-c",
            "user.email=archive@momso.local",
            "-c",
            "tag.gpgSign=false",
            "tag",
            "-a",
            name,
            "-m",
            &message,
            &pair.git_revision,
        ],
    )
    .map_err(|_| "backup_git_ref_failed")?;
    let refspec = format!("{reference}:{reference}");
    crate::archive_remote::git(tool, root, &["push", "--", target, &refspec])
        .map_err(|_| "backup_git_ref_failed")?;
    Ok(())
}

pub(crate) fn remote_git_refs(
    tool: &std::path::Path,
    root: &std::path::Path,
    target: &str,
) -> Result<std::collections::HashMap<String, String>, &'static str> {
    let output = crate::archive_remote::git(
        tool,
        root,
        &[
            "ls-remote",
            "--tags",
            "--",
            target,
            "refs/tags/momso-backup/*",
        ],
    )
    .map_err(|_| "backup_git_ref_failed")?;
    let mut refs = std::collections::HashMap::new();
    for line in output.lines() {
        let Some((revision, name)) = line.split_once('\t') else {
            continue;
        };
        let (name, peeled) = name
            .strip_suffix("^{}")
            .map_or((name, false), |name| (name, true));
        let Some(id) = name.strip_prefix("refs/tags/momso-backup/") else {
            continue;
        };
        if pair_ref(id).is_err()
            || !matches!(revision.len(), 40 | 64)
            || !revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            continue;
        }
        if peeled {
            refs.insert(id.to_owned(), revision.to_owned());
        } else {
            refs.entry(id.to_owned())
                .or_insert_with(|| revision.to_owned());
        }
    }
    Ok(refs)
}

/// Check the Git-side timestamp and ID, not just the tag's target commit.
pub(crate) fn git_ref_matches(
    tool: &std::path::Path,
    root: &std::path::Path,
    target: &str,
    pair: &CompletedPair,
) -> Result<bool, &'static str> {
    let reference = pair_ref(&pair.pair_id)?;
    let output = crate::archive_remote::git(
        tool,
        root,
        &["ls-remote", "--tags", "--", target, &reference],
    )
    .map_err(|_| "backup_git_ref_failed")?;
    let Some(object) = output
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .find_map(|(object, name)| (name == reference.as_str()).then_some(object))
    else {
        return Ok(false);
    };
    if !matches!(object.len(), 40 | 64)
        || !object
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Ok(false);
    }
    crate::archive_remote::git(
        tool,
        root,
        &["fetch", "--no-tags", "--", target, &reference],
    )
    .map_err(|_| "backup_git_ref_failed")?;
    let kind = crate::archive_remote::git(tool, root, &["cat-file", "-t", object])
        .map_err(|_| "backup_git_ref_failed")?;
    if kind != "tag" {
        return Ok(false);
    }
    let size = crate::archive_remote::git(tool, root, &["cat-file", "-s", object])
        .map_err(|_| "backup_git_ref_failed")?
        .parse::<usize>()
        .map_err(|_| "invalid_backup_pair")?;
    if size > MAX_PAIR {
        return Ok(false);
    }
    let raw = crate::archive_remote::git_bytes(tool, root, &["cat-file", "tag", object])
        .map_err(|_| "backup_git_ref_failed")?;
    let text = std::str::from_utf8(&raw).map_err(|_| "invalid_backup_pair")?;
    let Some((headers, body)) = text.split_once("\n\n") else {
        return Ok(false);
    };
    let object_header = format!("object {}", pair.git_revision);
    if !headers.lines().any(|line| line == object_header.as_str())
        || !headers.lines().any(|line| line == "type commit")
    {
        return Ok(false);
    }
    let Ok(metadata) = serde_json::from_str::<serde_json::Value>(body.trim()) else {
        return Ok(false);
    };
    Ok(
        metadata.get("pairId").and_then(|value| value.as_str()) == Some(pair.pair_id.as_str())
            && metadata.get("createdAt").and_then(|value| value.as_str())
                == Some(pair.created_at.as_str()),
    )
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CompletedPair {
    pub(crate) version: u32,
    pub(crate) pair_id: String,
    pub(crate) created_at: String,
    pub(crate) git_revision: String,
    pub(crate) snapshot_upload_id: String,
}

impl CompletedPair {
    pub(crate) fn valid(&self) -> bool {
        self.version == 1
            && uuid::Uuid::parse_str(&self.pair_id).is_ok()
            && uuid::Uuid::parse_str(&self.snapshot_upload_id).is_ok()
            && chrono::DateTime::parse_from_rfc3339(&self.created_at).is_ok()
            && matches!(self.git_revision.len(), 40 | 64)
            && self
                .git_revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

/// Order only candidates with valid metadata; callers must verify both
/// referenced components and may then fall back to the next candidate.
pub(crate) fn candidates(mut pairs: Vec<CompletedPair>) -> Vec<CompletedPair> {
    pairs.retain(CompletedPair::valid);
    pairs.sort_by(|a, b| {
        let a_time = chrono::DateTime::parse_from_rfc3339(&a.created_at).expect("validated");
        let b_time = chrono::DateTime::parse_from_rfc3339(&b.created_at).expect("validated");
        b_time.cmp(&a_time).then(b.pair_id.cmp(&a.pair_id))
    });
    pairs
}

/// References are supplied after fetching Git refs and the encrypted catalog.
pub(crate) fn complete_candidates(
    pairs: Vec<CompletedPair>,
    git_refs: &std::collections::HashMap<String, String>,
    snapshot_ids: &std::collections::HashSet<String>,
) -> Vec<CompletedPair> {
    candidates(pairs)
        .into_iter()
        .filter(|pair| {
            git_refs.get(&pair.pair_id) == Some(&pair.git_revision)
                && snapshot_ids.contains(&pair.snapshot_upload_id)
        })
        .collect()
}

pub(crate) async fn publish(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &age::x25519::Identity,
    pair: &CompletedPair,
) -> Result<(), &'static str> {
    if !pair.valid() {
        return Err("invalid_backup_pair");
    }
    let plain = serde_json::to_vec(pair).map_err(|_| "invalid_backup_pair")?;
    let encrypted = transfer::encrypt_bytes(&plain, identity)?;
    let key = format!("{}{}.age", pair_prefix(), pair.pair_id);
    transfer::put_once(config, keys, &key, encrypted).await
}

pub(crate) async fn list(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &age::x25519::Identity,
) -> Result<Vec<CompletedPair>, &'static str> {
    let prefix = pair_prefix();
    let mut pairs = Vec::new();
    for key in transfer::list_keys(config, keys, &prefix).await? {
        let Some(id) = key
            .strip_prefix(&prefix)
            .and_then(|name| name.strip_suffix(".age"))
        else {
            continue;
        };
        if uuid::Uuid::parse_str(id).is_err() {
            continue;
        }
        let bytes = match transfer::get_bytes(config, keys, &key, MAX_PAIR).await {
            Ok(Some(bytes)) => bytes,
            Ok(None) | Err("invalid_remote_object") => continue,
            Err(error) => return Err(error),
        };
        let Ok(plain) = transfer::decrypt_bytes(&bytes, identity, MAX_PAIR) else {
            continue;
        };
        let Ok(pair) = serde_json::from_slice::<CompletedPair>(&plain) else {
            continue;
        };
        if pair.pair_id == id && pair.valid() {
            pairs.push(pair);
        }
    }
    Ok(candidates(pairs))
}

pub(crate) async fn download_for_restore<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    paths: &crate::runtime::RuntimePaths,
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &age::x25519::Identity,
    use_backup: bool,
) -> Result<
    (
        tempfile::NamedTempFile,
        transfer::Manifest,
        Option<CompletedPair>,
    ),
    &'static str,
> {
    let root = paths
        .objects_path
        .parent()
        .ok_or("local_state_unavailable")?;
    // A damaged or unreadable completion record is still evidence that this
    // store uses paired backups. Never treat it as a legacy unpaired snapshot.
    let has_pair_markers = !transfer::list_keys(config, keys, &pair_prefix())
        .await?
        .is_empty();
    let pairs = list(config, keys, identity).await?;
    let (tool, target, _head) = match crate::archive_remote::paired_git_context_for_restore(
        app,
        root,
        &paths.database_path,
        !use_backup,
    ) {
        Ok(value) => value,
        Err("not_connected" | "git_missing") if !has_pair_markers => {
            let (snapshot, manifest) =
                crate::object_store_state::download_latest_snapshot(config, keys, identity, paths)
                    .await?;
            return Ok((snapshot, manifest, None));
        }
        Err("backup_pending_git") => return Err("local_data_conflict"),
        Err(error) => return Err(error),
    };
    let refs = remote_git_refs(&tool, root, &target)?;
    if !has_pair_markers && refs.is_empty() {
        let (snapshot, manifest) =
            crate::object_store_state::download_latest_snapshot(config, keys, identity, paths)
                .await?;
        return Ok((snapshot, manifest, None));
    }
    let manifests = transfer::catalog(config, keys, identity).await?;
    let ids = manifests
        .iter()
        .filter(|manifest| {
            manifest.kind == "app_state" && manifest.relative_path == "paired-momso.db"
        })
        .map(|manifest| manifest.upload_id.clone())
        .collect();
    for pair in complete_candidates(pairs, &refs, &ids) {
        if !git_ref_matches(&tool, root, &target, &pair)? {
            continue;
        }
        let Some(manifest) = manifests.iter().find(|manifest| {
            manifest.upload_id == pair.snapshot_upload_id
                && manifest.kind == "app_state"
                && manifest.relative_path == "paired-momso.db"
        }) else {
            continue;
        };
        let snapshot = match crate::object_store_restore::download_verified(
            config,
            keys,
            identity,
            manifest,
            &paths.backups_path,
        )
        .await
        {
            Ok(snapshot) => snapshot,
            Err("hash_mismatch" | "remote_missing" | "decryption_failed" | "invalid_manifest") => {
                continue
            }
            Err(error) => return Err(error),
        };
        if crate::storage::validate_snapshot_file(snapshot.path()).is_err() {
            continue;
        }
        match verify_referenced_originals(config, keys, identity, snapshot.path()).await {
            Ok(()) => (),
            Err("backup_original_missing") => continue,
            Err(error) => return Err(error),
        }
        return Ok((snapshot, manifest.clone(), Some(pair)));
    }
    Err("backup_pair_incomplete")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(time: &str) -> CompletedPair {
        CompletedPair {
            version: 1,
            pair_id: uuid::Uuid::new_v4().to_string(),
            created_at: time.to_owned(),
            git_revision: "a".repeat(40),
            snapshot_upload_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    #[test]
    fn invalid_pairs_are_never_restore_candidates() {
        let older = pair("2026-01-01T00:00:00Z");
        let newer = pair("2026-01-02T00:00:00Z");
        let mut invalid = pair("2026-01-03T00:00:00Z");
        invalid.git_revision = "../invalid".into();
        assert_eq!(
            candidates(vec![older.clone(), invalid, newer.clone()]),
            vec![newer, older]
        );
    }

    #[test]
    fn incomplete_newer_pairs_fall_back_to_last_complete_pair() {
        let older = pair("2026-01-01T00:00:00Z");
        let missing_git = pair("2026-01-02T00:00:00Z");
        let missing_db = pair("2026-01-03T00:00:00Z");
        let git_refs = [&older, &missing_db]
            .into_iter()
            .map(|pair| (pair.pair_id.clone(), pair.git_revision.clone()))
            .collect();
        let snapshots = [
            older.snapshot_upload_id.clone(),
            missing_git.snapshot_upload_id.clone(),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            complete_candidates(
                vec![missing_db, missing_git, older.clone()],
                &git_refs,
                &snapshots
            ),
            vec![older]
        );
    }

    #[test]
    fn missing_original_chunk_prevents_complete_backup() {
        let manifest = transfer::Manifest {
            version: 2,
            kind: "object".into(),
            relative_path: "sha256/aa/fixture".into(),
            sha256: "a".repeat(64),
            size: 12,
            upload_id: uuid::Uuid::new_v4().to_string(),
            chunks: vec!["b".repeat(64), "c".repeat(64)],
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let assets = vec![(
            manifest.relative_path.clone(),
            manifest.sha256.clone(),
            manifest.size,
        )];
        let mut keys = std::collections::HashSet::new();
        keys.insert(transfer::chunk_key(&manifest, 0));
        assert!(!referenced_originals_complete(
            &assets,
            std::slice::from_ref(&manifest),
            &keys
        ));
        keys.insert(transfer::chunk_key(&manifest, 1));
        assert!(referenced_originals_complete(&assets, &[manifest], &keys));
    }

    #[test]
    fn local_git_pair_ref_is_immutable_and_discoverable() {
        let Some(tool) = crate::git_tool::resolve().map(|(tool, _)| tool) else {
            panic!("git is needed for the local backup fixture");
        };
        let temp = tempfile::tempdir().unwrap();
        let bare = temp.path().join("remote.git");
        let work = temp.path().join("work");
        std::fs::create_dir(&work).unwrap();
        let git = |root: &std::path::Path, args: &[&str]| {
            crate::archive_remote::git(&tool, root, args).unwrap()
        };
        git(temp.path(), &["init", "--bare", bare.to_str().unwrap()]);
        git(&work, &["init"]);
        git(&work, &["config", "user.email", "fixture@example.invalid"]);
        git(&work, &["config", "user.name", "Fixture"]);
        std::fs::write(work.join("fixture.txt"), "fixture").unwrap();
        git(&work, &["add", "fixture.txt"]);
        git(&work, &["commit", "-m", "fixture"]);
        let mut value = pair("2026-01-01T00:00:00Z");
        value.git_revision = git(&work, &["rev-parse", "HEAD"]);
        publish_git_ref(&tool, &work, bare.to_str().unwrap(), &value).unwrap();
        assert_eq!(
            remote_git_refs(&tool, &work, bare.to_str().unwrap())
                .unwrap()
                .get(&value.pair_id),
            Some(&value.git_revision)
        );
        assert!(git_ref_matches(&tool, &work, bare.to_str().unwrap(), &value).unwrap());
        value.created_at = "2026-01-02T00:00:00Z".into();
        assert!(!git_ref_matches(&tool, &work, bare.to_str().unwrap(), &value).unwrap());
    }

    #[tokio::test]
    async fn encrypted_pair_records_skip_corrupt_objects() {
        exercise_fake_store(false).await;
    }

    #[tokio::test]
    #[ignore = "requires fake credentials and local Git for the full backup fixture"]
    async fn automatic_pair_backup_and_new_profile_restore() {
        assert!(std::env::var_os("MOMSO_TEST_FAKE_CREDENTIAL_STORE").is_some());
        exercise_fake_store(true).await;
    }

    async fn exercise_fake_store(include_auto: bool) {
        use std::io::{Read, Write};
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let object_folder = tempfile::tempdir().unwrap();
        let server_folder = object_folder.path().to_path_buf();
        let store = Arc::new(Mutex::new(
            std::collections::HashMap::<String, Vec<u8>>::new(),
        ));
        let server_store = store.clone();
        let stopping = Arc::new(AtomicBool::new(false));
        let server_stopping = stopping.clone();
        let server = std::thread::spawn(move || {
            while !server_stopping.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(15)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0_u8; 8192];
                let end = loop {
                    let n = stream.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        break index + 4;
                    }
                };
                let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
                let mut line = head.lines().next().unwrap().split_whitespace();
                let method = line.next().unwrap();
                let path = line.next().unwrap();
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|s| s.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while bytes.len() - end < length {
                    let n = stream.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                }
                let key = path
                    .trim_start_matches("/fixture/")
                    .split('?')
                    .next()
                    .unwrap();
                let (status, body) = if method == "PUT" {
                    let destination = server_folder.join(key);
                    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
                    std::fs::write(destination, &bytes[end..end + length]).unwrap();
                    server_store
                        .lock()
                        .unwrap()
                        .insert(key.to_owned(), bytes[end..end + length].to_vec());
                    ("200 OK", Vec::new())
                } else if path.starts_with("/fixture?") {
                    let prefix = if path.contains("completed-pairs") {
                        "/completed-pairs/"
                    } else if path.contains("data%2F") || path.contains("data%2f") {
                        "/data/"
                    } else {
                        "/manifests/"
                    };
                    let entries = server_store
                        .lock()
                        .unwrap()
                        .keys()
                        .filter(|key| key.contains(prefix))
                        .map(|key| format!("<Contents><Key>{key}</Key></Contents>"))
                        .collect::<String>();
                    ("200 OK", format!("<ListBucketResult><IsTruncated>false</IsTruncated>{entries}</ListBucketResult>").into_bytes())
                } else if method == "DELETE" {
                    server_store.lock().unwrap().remove(key);
                    let _ = std::fs::remove_file(server_folder.join(key));
                    ("204 No Content", Vec::new())
                } else {
                    match std::fs::read(server_folder.join(key)) {
                        Ok(data) => ("200 OK", data),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            ("404 Not Found", Vec::new())
                        }
                        Err(error) => panic!("fixture read failed: {error}"),
                    }
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let config = object_store::Config {
            provider: "s3_compatible".into(),
            endpoint: format!("http://{address}"),
            region: "auto".into(),
            bucket: "fixture".into(),
            credential_id: "fixture".into(),
            last_backup_at: None,
            write_safety: Some(object_store::WriteSafety::Conditional),
        };
        let keys = object_store::Keys {
            access_key_id: "fixture".into(),
            secret_access_key: "fixture".into(),
        };
        let identity = age::x25519::Identity::generate();
        let older = pair("2026-01-01T00:00:00Z");
        let newer = pair("2026-01-02T00:00:00Z");
        publish(&config, &keys, &identity, &older).await.unwrap();
        publish(&config, &keys, &identity, &newer).await.unwrap();
        assert_eq!(
            list(&config, &keys, &identity).await.unwrap(),
            vec![newer.clone(), older.clone()]
        );
        let newer_key = format!("{}{}.age", pair_prefix(), newer.pair_id);
        store
            .lock()
            .unwrap()
            .insert(newer_key.clone(), b"corrupted".to_vec());
        std::fs::write(object_folder.path().join(newer_key), b"corrupted").unwrap();
        assert_eq!(list(&config, &keys, &identity).await.unwrap(), vec![older]);
        if include_auto {
            store.lock().unwrap().clear();
            exercise_automatic_pair(&config, &keys, &identity).await;
        }
        stopping.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }

    async fn exercise_automatic_pair(
        config: &object_store::Config,
        keys: &object_store::Keys,
        identity: &age::x25519::Identity,
    ) {
        use crate::{
            archive,
            archive_location::ArchiveLocation,
            archive_remote::{self, TestRemote},
            runtime::RuntimePaths,
            storage, AppState,
        };
        use age::secrecy::ExposeSecret;
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("old");
        let root = data.join("momso");
        let bare = temp.path().join("remote.git");
        std::fs::create_dir_all(&data).unwrap();
        let db = data.join("momso.db");
        let backups = data.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        storage::initialize(&db, &backups, "test").unwrap();
        let tool = crate::git_tool::resolve()
            .map(|(tool, _)| tool)
            .expect("local Git");
        archive_remote::git(
            &tool,
            temp.path(),
            &["init", "--bare", bare.to_str().unwrap()],
        )
        .unwrap();
        let archive = ArchiveLocation::load(db.clone(), &data, vec![]).unwrap();
        *archive.current.try_write().unwrap() = root.clone();
        archive::initialize(&root, &data).unwrap();
        let paths = RuntimePaths {
            data_root: data.clone(),
            database_path: db.clone(),
            objects_path: root.join("objects"),
            exports_path: data.join("exports"),
            backups_path: backups,
            logs_path: data.join("logs"),
            archive,
            archive_lease: None,
        };
        let profile = crate::backup_profile::ensure(&db).unwrap();
        crate::credentials::import_backup_identity(
            &profile.workspace_id,
            identity.to_string().expose_secret(),
        )
        .unwrap();
        crate::backup_profile::confirm_recovery_key(&db, &profile.workspace_id).unwrap();
        let mut config = config.clone();
        config.credential_id = uuid::Uuid::new_v4().to_string();
        object_store::register_test_config(&data, config.clone());
        crate::credentials::put_secret_unlocked(
            &format!("object-store:{}", config.credential_id),
            &serde_json::to_string(keys).unwrap(),
        )
        .unwrap();
        let chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
        let chat_id = chat["id"].as_str().unwrap();
        crate::chat::append(
            &db,
            chat_id,
            crate::chat::Role::User,
            "fixture conversation",
            None,
            None,
        )
        .unwrap();
        let original = b"fixture attachment";
        let hash = object_store::digest(original);
        let relative = format!("sha256/{}/{hash}", &hash[..2]);
        let original_path = paths.objects_path.join(&relative);
        std::fs::create_dir_all(original_path.parent().unwrap()).unwrap();
        std::fs::write(&original_path, original).unwrap();
        let asset_id = uuid::Uuid::new_v4().to_string();
        let connection = storage::open_writable_connection(&db).unwrap();
        connection.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES(?1,'other',?2,?3,?4,'fixture.txt','fixture','now')", rusqlite::params![asset_id, hash, original.len() as i64, relative]).unwrap();
        drop(connection);
        crate::archive_receipt::attach_asset(&root, &db, chat_id, &asset_id).unwrap();
        while !crate::chat_redaction_recheck::step(&root, &paths)
            .unwrap()
            .available
        {}
        crate::chat_archive_store::write(&root, &paths, chat_id).unwrap();
        let app = tauri::test::mock_builder()
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
            .manage(crate::archive_auto_backup::AutoBackup::default())
            .manage(crate::object_store_auto_upload::AutoUpload::default())
            .manage(TestRemote {
                tool: tool.clone(),
                target: bare.to_string_lossy().into_owned(),
            })
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        crate::archive_auto_backup::attempt(app.handle().clone(), true).await;
        crate::object_store_auto_upload::attempt(app.handle().clone(), true).await;
        let pairs = list(&config, keys, identity).await.unwrap();
        assert_eq!(
            pairs.len(),
            1,
            "automatic backup publishes one completed pair"
        );
        let resources = app.path().resource_dir().unwrap();
        let (git_close, object_close) =
            crate::archive_auto_backup::close_all(app.handle().clone(), paths.clone(), resources)
                .await
                .unwrap();
        assert!(matches!(
            git_close,
            crate::archive_auto_backup::CloseResult::Finished(None)
        ));
        assert!(object_close);
        let pairs = list(&config, keys, identity).await.unwrap();
        assert!(pairs.len() >= 2, "closing creates another completed pair");
        crate::chat::append(
            &db,
            chat_id,
            crate::chat::Role::User,
            "unpaired later change",
            None,
            None,
        )
        .unwrap();
        crate::chat_archive_store::write(&root, &paths, chat_id).unwrap();
        while !crate::chat_redaction_recheck::step(&root, &paths)
            .unwrap()
            .available
        {}
        crate::archive_auto_backup::attempt(app.handle().clone(), true).await;
        let newer_head = archive_remote::git(&tool, &root, &["rev-parse", "HEAD"]).unwrap();
        assert_ne!(
            newer_head, pairs[0].git_revision,
            "the newer Git upload has no matching DB pair"
        );
        let new_data = temp.path().join("new");
        std::fs::create_dir_all(&new_data).unwrap();
        let new_db = new_data.join("momso.db");
        let new_backups = new_data.join("backups");
        std::fs::create_dir_all(&new_backups).unwrap();
        storage::initialize(&new_db, &new_backups, "test").unwrap();
        let new_root = new_data.join("momso");
        let new_archive = ArchiveLocation::load(new_db.clone(), &new_data, vec![]).unwrap();
        *new_archive.current.try_write().unwrap() = new_root.clone();
        archive::initialize(&new_root, &new_data).unwrap();
        archive_remote::connect_local(&tool, &new_root, bare.to_str().unwrap()).unwrap();
        archive_remote::receive_connected(
            &tool,
            &new_root,
            &new_db,
            bare.to_str().unwrap(),
            "main",
        )
        .unwrap();
        let new_paths = RuntimePaths {
            data_root: new_data.clone(),
            database_path: new_db.clone(),
            objects_path: new_root.join("objects"),
            exports_path: new_data.join("exports"),
            backups_path: new_backups,
            logs_path: new_data.join("logs"),
            archive: new_archive,
            archive_lease: None,
        };
        crate::backup::import_recovery_key(&new_paths, identity.to_string().expose_secret())
            .unwrap();
        object_store::register_test_config(&new_data, config.clone());
        let new_app = tauri::test::mock_builder()
            .manage(AppState {
                paths: new_paths.clone(),
                approvals: Default::default(),
                setup_secrets: Default::default(),
                usage_monitor: Default::default(),
                usage_cache: Default::default(),
                chat_active: Default::default(),
                browser: Default::default(),
                selections: Default::default(),
            })
            .manage(TestRemote {
                tool: tool.clone(),
                target: bare.to_string_lossy().into_owned(),
            })
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let (_, _, received_head) = crate::archive_remote::paired_git_context_for_restore(
            &new_app.handle().clone(),
            &new_root,
            &new_db,
            true,
        )
        .unwrap_or_else(|error| {
            panic!(
                "fresh receive: {error}; status={:?}; head={:?}; remote={:?}; archive={:?}",
                archive_remote::git(
                    &tool,
                    &new_root,
                    &["status", "--porcelain", "--untracked-files=all"]
                ),
                archive_remote::git(&tool, &new_root, &["rev-parse", "HEAD"]),
                archive_remote::git(
                    &tool,
                    &new_root,
                    &[
                        "ls-remote",
                        "--heads",
                        bare.to_str().unwrap(),
                        "refs/heads/main"
                    ]
                ),
                crate::chat_archive_store::validate(&new_root, &new_db),
            )
        });
        let marker: Option<String> = rusqlite::Connection::open(&new_db)
            .unwrap()
            .query_row(
                "SELECT value FROM app_runtime_meta WHERE key='paired_first_receive_head'",
                [],
                |row| row.get(0),
            )
            .ok();
        assert_eq!(
            marker.as_deref(),
            Some(received_head.as_str()),
            "first receive marker"
        );
        let (candidate, _, _) = download_for_restore(
            &new_app.handle().clone(),
            &new_paths,
            &config,
            keys,
            identity,
            false,
        )
        .await
        .expect("complete pair must be available");
        crate::object_store_state::ensure_nonchat_content_preserved(&new_db, candidate.path())
            .expect("fresh profile must have no conflicting non-chat data");
        let restored = crate::object_store_state::restore_with_choice(
            new_app.handle().clone(),
            new_app.state(),
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::to_value(restored).unwrap()["bundleId"],
            pairs[0].pair_id
        );
        crate::object_store_restore::restore_selected(&config, keys, identity, &new_paths, None)
            .await
            .unwrap();
        assert_eq!(
            transfer::hash_file(&new_paths.objects_path.join(&relative))
                .unwrap()
                .0,
            hash
        );
        assert_eq!(
            serde_json::to_value(crate::chat::read(&new_db, chat_id).unwrap()).unwrap()["messages"]
                [0]["text"],
            "fixture conversation"
        );
        let safety = std::fs::read_dir(&new_paths.backups_path)
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("paired-restore-")
            })
            .expect("pre-restore database is retained")
            .path();
        assert!(
            serde_json::to_value(crate::chat::read(&safety, chat_id).unwrap())
                .unwrap()
                .to_string()
                .contains("unpaired later change")
        );
        let local_chat = serde_json::to_value(crate::chat::create(&new_db).unwrap()).unwrap();
        let local_id = local_chat["id"].as_str().unwrap();
        crate::chat::append(
            &new_db,
            local_id,
            crate::chat::Role::User,
            "new local data",
            None,
            None,
        )
        .unwrap();
        assert!(matches!(
            crate::object_store_state::restore_with_choice(
                new_app.handle().clone(),
                new_app.state(),
                false
            )
            .await,
            Err("local_data_conflict")
        ));
        assert_eq!(
            serde_json::to_value(crate::object_store_state::app_state_restore_keep_current())
                .unwrap()["state"],
            "kept"
        );
        assert!(crate::chat::read(&new_db, local_id).is_ok());
        crate::object_store_state::restore_with_choice(
            new_app.handle().clone(),
            new_app.state(),
            true,
        )
        .await
        .expect("explicit backup choice must keep a safety copy");
        assert!(crate::chat::read(&new_db, local_id).is_err());
        assert!(
            std::fs::read_dir(&new_paths.backups_path)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| { crate::chat::read(&entry.path(), local_id).is_ok() }),
            "the original local chat must remain in a safety database"
        );
        #[cfg(windows)]
        crate::source_assets::clear_readonly_attribute(&new_paths.objects_path.join(&relative))
            .unwrap();
        object_store::unregister_test_config(&data);
        object_store::unregister_test_config(&new_data);
    }
}
