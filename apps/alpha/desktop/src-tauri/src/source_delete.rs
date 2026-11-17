use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceReferences {
    asset_id: String,
    sha256: String,
    original_file_name: String,
    state: String,
    documents: Vec<String>,
    receipts: Vec<String>,
    chats: Vec<String>,
    record_versions: Vec<RecordReference>,
    local_remaining: Option<Vec<String>>,
    remote_remaining: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_digest: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordReference {
    record_id: String,
    record_version: i64,
}

fn contains_reference(path: &Path, first: &[u8], second: &[u8]) -> Result<bool, String> {
    let mut input = fs::File::open(path).map_err(|_| "references_unavailable")?;
    let mut carry = Vec::new();
    let mut chunk = [0u8; 65536];
    let overlap = first.len().max(second.len()).saturating_sub(1);
    loop {
        let count = input
            .read(&mut chunk)
            .map_err(|_| "references_unavailable")?;
        if count == 0 {
            return Ok(false);
        }
        carry.extend_from_slice(&chunk[..count]);
        if carry.windows(first.len()).any(|part| part == first)
            || carry.windows(second.len()).any(|part| part == second)
        {
            return Ok(true);
        }
        if carry.len() > overlap {
            carry.drain(..carry.len() - overlap);
        }
    }
}

pub(crate) fn preview(
    root: &Path,
    database: &Path,
    asset_id: &str,
) -> Result<SourceReferences, String> {
    let conn = crate::storage::open_writable_connection(database).map_err(|_| "unavailable")?;
    let lookup = crate::source_assets::reference_lookup(asset_id).ok_or("not_found")?;
    type AssetRow = (
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
    );
    let row: Option<AssetRow> = conn.query_row(
        "SELECT a.id,a.content_sha256,a.original_file_name,COALESCE(d.state,a.integrity_state),d.local_remaining_json,d.remote_remaining_json FROM momso_source_assets a LEFT JOIN source_asset_deletions d ON d.asset_id=a.id WHERE a.id=?1 OR a.content_sha256=?2",
        rusqlite::params![asset_id, lookup], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
    ).optional().map_err(|_| "unavailable")?;
    let (asset_id, sha256, original_file_name, state, local_json, remote_json) =
        row.ok_or("not_found")?;
    let local_remaining = local_json
        .map(|value| serde_json::from_str(&value).map_err(|_| "unavailable"))
        .transpose()?;
    let remote_remaining = remote_json
        .map(|value| serde_json::from_str(&value).map_err(|_| "unavailable"))
        .transpose()?;
    let reference = format!("sha256:{sha256}");
    if asset_id.is_empty()
        || asset_id.len() > 128
        || !crate::archive_zet::valid_asset_id(&reference)
    {
        return Err("invalid_asset".into());
    }
    let mut documents = BTreeSet::new();
    for entry in WalkDir::new(root.join("zettels")).follow_links(false) {
        let entry = entry.map_err(|_| "references_unavailable")?;
        let meta = fs::symlink_metadata(entry.path()).map_err(|_| "references_unavailable")?;
        if crate::archive_location::is_link_or_reparse(&meta) {
            return Err("references_unavailable".into());
        }
        if !meta.is_file()
            || !entry
                .path()
                .extension()
                .and_then(|v| v.to_str())
                .is_some_and(|v| v.eq_ignore_ascii_case("md"))
        {
            continue;
        }
        if contains_reference(entry.path(), reference.as_bytes(), asset_id.as_bytes())? {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|_| "references_unavailable")?
                .to_str()
                .ok_or("references_unavailable")?
                .replace('\\', "/");
            documents.insert(relative);
        }
    }
    let mut receipts = BTreeSet::new();
    let mut chats = BTreeSet::new();
    for entry in fs::read_dir(root.join("receipts")).map_err(|_| "references_unavailable")? {
        let entry = entry.map_err(|_| "references_unavailable")?;
        let meta = fs::symlink_metadata(entry.path()).map_err(|_| "references_unavailable")?;
        if crate::archive_location::is_link_or_reparse(&meta) {
            return Err("references_unavailable".into());
        }
        if !meta.is_file() || entry.path().extension().is_none_or(|v| v != "md") {
            continue;
        }
        if meta.len() > 16 * 1024 * 1024 {
            return Err("references_unavailable".into());
        }
        let mut bytes = Vec::new();
        fs::File::open(entry.path())
            .map_err(|_| "references_unavailable")?
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "references_unavailable")?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err("references_unavailable".into());
        }
        let text = String::from_utf8(bytes).map_err(|_| "references_unavailable")?;
        let (head, _) =
            crate::archive_receipt::parse(&text).map_err(|_| "references_unavailable")?;
        if entry.file_name() != format!("{}.md", head.id).as_str() {
            return Err("references_unavailable".into());
        }
        if head
            .read
            .iter()
            .chain(&head.wrote)
            .any(|id| id == &reference)
        {
            receipts.insert(head.id);
            if let Some(chat) = head.chat {
                chats.insert(chat);
            }
        }
    }
    let mut stmt = conn
        .prepare("SELECT DISTINCT chat_id FROM chat_attachments WHERE asset_id=?1 ORDER BY chat_id")
        .map_err(|_| "unavailable")?;
    for value in stmt
        .query_map([&asset_id], |r| r.get::<_, String>(0))
        .map_err(|_| "unavailable")?
    {
        chats.insert(value.map_err(|_| "unavailable")?);
    }
    let mut live_chats = BTreeSet::new();
    let mut chat_stmt = conn
        .prepare("SELECT EXISTS(SELECT 1 FROM chats WHERE id=?1) AND NOT EXISTS(SELECT 1 FROM chat_deletions WHERE chat_id=?1)")
        .map_err(|_| "unavailable")?;
    for chat in chats {
        let live: bool = chat_stmt
            .query_row([&chat], |row| row.get(0))
            .map_err(|_| "unavailable")?;
        if live {
            live_chats.insert(chat);
        }
    }
    let mut stmt = conn.prepare("SELECT DISTINCT record_id,record_version FROM momso_record_source_assets WHERE asset_id=?1 ORDER BY record_id,record_version").map_err(|_| "unavailable")?;
    let record_versions = stmt
        .query_map([&asset_id], |r| {
            Ok(RecordReference {
                record_id: r.get(0)?,
                record_version: r.get(1)?,
            })
        })
        .map_err(|_| "unavailable")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "unavailable")?;
    let mut result = SourceReferences {
        asset_id,
        sha256,
        original_file_name,
        state,
        documents: documents.into_iter().collect(),
        receipts: receipts.into_iter().collect(),
        chats: live_chats.into_iter().collect(),
        record_versions,
        local_remaining,
        remote_remaining,
        plan_digest: None,
    };
    let encoded = serde_json::to_vec(&result).map_err(|_| "unavailable")?;
    result.plan_digest = Some(crate::object_store::digest(&encoded));
    Ok(result)
}

#[tauri::command]
pub(crate) async fn source_asset_delete_preview(
    state: tauri::State<'_, crate::AppState>,
    asset_id: String,
) -> Result<SourceReferences, String> {
    let paths = state.paths.scoped().await;
    let _guard = crate::chat_file_changes::lock_app_write();
    preview(
        paths.objects_path.parent().ok_or("unavailable")?,
        &paths.database_path,
        &asset_id,
    )
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeletedSourceCursor {
    deleted_at: String,
    asset_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReferenceCounts {
    documents: usize,
    receipts: usize,
    chats: usize,
    record_versions: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeletedSourceItem {
    #[serde(flatten)]
    asset: crate::source_assets::SourceAsset,
    deleted_at: String,
    deletion_state: String,
    can_restore: bool,
    can_purge: bool,
    reference_count: usize,
    reference_counts: ReferenceCounts,
    local_remaining: Option<Vec<String>>,
    remote_remaining: Option<Vec<String>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeletedSourcePage {
    items: Vec<DeletedSourceItem>,
    has_more: bool,
    next_cursor: Option<DeletedSourceCursor>,
}

fn local_retry_needed(remaining: &[String]) -> bool {
    remaining.iter().any(|item| {
        item != "external_copies_unverified" && item != "database_backups_may_contain_metadata"
    })
}

fn list_deleted(
    root: &Path,
    database: &Path,
    before: Option<DeletedSourceCursor>,
    limit: usize,
) -> Result<DeletedSourcePage, String> {
    let data_root = database.parent().ok_or("unavailable")?;
    let target = crate::object_store::load_config(data_root)?
        .filter(remote_deletion_supported)
        .and_then(|config| {
            crate::object_store::load_keys(&config).ok()?;
            let identity = crate::object_store_transfer::identity(database).ok()?;
            Some(remote_target(&config, &identity))
        });
    list_deleted_with_target(root, database, before, limit, target.as_deref())
}

fn remote_deletion_supported(config: &crate::object_store::Config) -> bool {
    crate::object_store_lock::supported(config) || crate::r2_oauth::uses_oauth(config)
}

fn list_deleted_with_target(
    root: &Path,
    database: &Path,
    before: Option<DeletedSourceCursor>,
    limit: usize,
    target: Option<&str>,
) -> Result<DeletedSourcePage, String> {
    if before
        .as_ref()
        .is_some_and(|cursor| cursor.deleted_at.is_empty() || cursor.asset_id.is_empty())
    {
        return Err("invalid_cursor".into());
    }
    let limit = limit.clamp(1, 100);
    let conn = crate::storage::open_writable_connection(database).map_err(|_| "unavailable")?;
    let mut statement = conn.prepare("SELECT asset_id,requested_at,state,local_remaining_json,remote_plan_json FROM source_asset_deletions WHERE state IN ('pending','trashed','purge_pending') AND (?1 IS NULL OR requested_at < ?1 OR (requested_at = ?1 AND asset_id < ?2)) ORDER BY requested_at DESC,asset_id DESC").map_err(|_| "unavailable")?;
    let rows = statement
        .query_map(
            rusqlite::params![
                before.as_ref().map(|v| &v.deleted_at),
                before.as_ref().map(|v| &v.asset_id),
            ],
            |row| {
                Ok((
                    DeletedSourceCursor {
                        asset_id: row.get(0)?,
                        deleted_at: row.get(1)?,
                    },
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .map_err(|_| "unavailable")?;
    let mut items = Vec::with_capacity(limit);
    let mut has_more = false;
    let mut last_cursor = None;
    for row in rows {
        let (cursor, row_state, local_json, remote_plan) = row.map_err(|_| "unavailable")?;
        let local_remaining: Option<Vec<String>> = local_json
            .map(|value| serde_json::from_str(&value).map_err(|_| "unavailable"))
            .transpose()?;
        let local_retry = row_state == "purge_pending"
            && local_remaining.as_deref().is_none_or(local_retry_needed);
        let remote_retry = target
            .zip(remote_plan.as_deref())
            .is_some_and(|(target, plan)| parse_bound_plan(plan, target).is_ok());
        if row_state == "purge_pending" && !local_retry && !remote_retry {
            continue;
        }
        if items.len() == limit {
            has_more = true;
            break;
        }
        let references = preview(root, database, &cursor.asset_id)?;
        last_cursor = Some(cursor.clone());
        let counts = ReferenceCounts {
            documents: references.documents.len(),
            receipts: references.receipts.len(),
            chats: references.chats.len(),
            record_versions: references.record_versions.len(),
        };
        let total = counts.documents + counts.receipts + counts.chats + counts.record_versions;
        let state = match references.state.as_str() {
            "purge_pending" if local_retry => "local_retry_pending".to_owned(),
            "purge_pending" => "remote_retry_pending".to_owned(),
            _ => references.state,
        };
        items.push(DeletedSourceItem {
            asset: crate::source_assets::get(database, &cursor.asset_id)
                .map_err(|_| "unavailable")?,
            deleted_at: cursor.deleted_at,
            can_restore: matches!(state.as_str(), "pending" | "trashed"),
            can_purge: matches!(
                state.as_str(),
                "trashed" | "local_retry_pending" | "remote_retry_pending"
            ),
            deletion_state: state,
            reference_count: total,
            reference_counts: counts,
            local_remaining: references.local_remaining,
            remote_remaining: references.remote_remaining,
        });
    }
    Ok(DeletedSourcePage {
        items,
        has_more,
        next_cursor: if has_more { last_cursor } else { None },
    })
}

#[tauri::command]
pub(crate) async fn source_asset_deleted_list(
    state: tauri::State<'_, crate::AppState>,
    before: Option<DeletedSourceCursor>,
    limit: Option<usize>,
) -> Result<DeletedSourcePage, String> {
    let paths = state.paths.scoped().await;
    let _guard = crate::chat_file_changes::lock_app_write();
    list_deleted(
        paths.objects_path.parent().ok_or("unavailable")?,
        &paths.database_path,
        before,
        limit.unwrap_or(40),
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeleteResult {
    state: &'static str,
    asset_id: String,
    retain_until: Option<String>,
}

#[derive(Default, Deserialize, Serialize)]
struct RemotePlan {
    target: String,
    data_keys: BTreeMap<String, String>,
    manifest_keys: BTreeMap<String, String>,
    shared_keys: Vec<String>,
}

fn valid_remote_key(key: &str, kind: &str) -> bool {
    let prefix = format!("{}/{kind}/", crate::object_store::PREFIX);
    let Some(rest) = key.strip_prefix(&prefix) else {
        return false;
    };
    if rest.is_empty()
        || rest.contains("..")
        || rest
            .bytes()
            .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'/')))
    {
        return false;
    }
    if kind == "manifests" {
        return !rest.contains('/') && rest.ends_with(".age");
    }
    let mut parts = rest.split('/');
    let (Some(upload), Some(file), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if uuid::Uuid::parse_str(upload).is_err() {
        return false;
    }
    let Some(index) = file.get(..8) else {
        return false;
    };
    if !index.bytes().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let suffix = &file[8..];
    suffix.is_empty()
        || suffix
            .strip_prefix('-')
            .is_some_and(|hash| crate::archive_zet::valid_asset_id(&format!("sha256:{hash}")))
}

fn valid_remote_plan(plan: &RemotePlan) -> bool {
    plan.data_keys.iter().all(|(key, hash)| {
        valid_remote_key(key, "data")
            && crate::archive_zet::valid_asset_id(&format!("sha256:{hash}"))
    }) && plan.manifest_keys.iter().all(|(key, hash)| {
        valid_remote_key(key, "manifests")
            && crate::archive_zet::valid_asset_id(&format!("sha256:{hash}"))
    }) && plan
        .shared_keys
        .iter()
        .all(|key| valid_remote_key(key, "data"))
}

fn parse_bound_plan(json: &str, target: &str) -> Result<RemotePlan, &'static str> {
    let plan: RemotePlan = serde_json::from_str(json).map_err(|_| "remote_plan_invalid")?;
    if plan.target != target {
        return Err("remote_target_changed");
    }
    if !valid_remote_plan(&plan) {
        return Err("remote_plan_invalid");
    }
    Ok(plan)
}

fn remote_target(config: &crate::object_store::Config, identity: &age::x25519::Identity) -> String {
    crate::object_store::digest(
        format!(
            "{}\0{}\0{}\0{}\0{}",
            config.provider,
            config.endpoint,
            config.bucket,
            config.credential_id,
            identity.to_public()
        )
        .as_bytes(),
    )
}

fn persist_staged_plan(
    target: (&Path, &str, &str),
    config: &crate::object_store::Config,
    identity: &age::x25519::Identity,
    stage_id: &str,
    manifest: &crate::object_store_transfer::Manifest,
    encrypted_manifest: Option<&[u8]>,
) -> Result<(), &'static str> {
    let (database, asset_id, hash) = target;
    if stage_id != config.credential_id
        || manifest.kind != "object"
        || manifest.sha256 != hash
        || !matches!(manifest.version, 1 | 2)
        || uuid::Uuid::parse_str(&manifest.upload_id).is_err()
    {
        return Err("remote_target_changed");
    }
    let target = remote_target(config, identity);
    let conn = crate::storage::open_writable_connection(database).map_err(|_| "unavailable")?;
    let saved: Option<String> = conn.query_row(
        "SELECT remote_plan_json FROM source_asset_deletions WHERE asset_id=?1 AND state='purge_pending'",
        [asset_id], |row| row.get(0),
    ).map_err(|_| "unavailable")?;
    let mut plan = if let Some(json) = &saved {
        parse_bound_plan(json, &target)?
    } else {
        RemotePlan {
            target,
            ..RemotePlan::default()
        }
    };
    for (index, hash) in manifest.chunks.iter().enumerate() {
        let key = crate::object_store_transfer::chunk_key(manifest, index);
        if plan.shared_keys.contains(&key) {
            continue;
        }
        if plan
            .data_keys
            .insert(key, hash.clone())
            .is_some_and(|old| old != *hash)
        {
            return Err("remote_plan_invalid");
        }
    }
    if let Some(encrypted) = encrypted_manifest {
        let key = crate::object_store_transfer::manifest_key(
            config,
            &manifest.created_at,
            &manifest.upload_id,
            encrypted,
        );
        let digest = crate::object_store::digest(encrypted);
        if plan
            .manifest_keys
            .insert(key, digest.clone())
            .is_some_and(|old| old != digest)
        {
            return Err("remote_plan_invalid");
        }
    }
    if !valid_remote_plan(&plan) {
        return Err("remote_plan_invalid");
    }
    let json = serde_json::to_string(&plan).map_err(|_| "remote_plan_invalid")?;
    if conn.execute("UPDATE source_asset_deletions SET remote_plan_json=?1 WHERE asset_id=?2 AND state='purge_pending' AND remote_plan_json IS ?3", rusqlite::params![json,asset_id,saved]).map_err(|_| "unavailable")? != 1 {
        return Err("remote_plan_changed");
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PurgeResult {
    state: &'static str,
    asset_id: String,
    local_remaining: Vec<String>,
    remote_remaining: Vec<String>,
    removed_remote_keys: Vec<String>,
}

fn remote_plan(
    entries: &[(String, crate::object_store_transfer::Manifest, String)],
    hash: &str,
    target: String,
) -> Result<RemotePlan, String> {
    let mut targets = BTreeMap::new();
    let mut others = BTreeSet::new();
    let mut manifests = BTreeMap::new();
    for (key, manifest, encrypted_hash) in entries {
        let target = manifest.kind == "object" && manifest.sha256 == hash;
        if target {
            manifests.insert(key.clone(), encrypted_hash.clone());
        }
        for index in 0..manifest.chunks.len() {
            let chunk = crate::object_store_transfer::chunk_key(manifest, index);
            if target {
                let expected = manifest.chunks[index].clone();
                if targets
                    .insert(chunk, expected.clone())
                    .is_some_and(|old| old != expected)
                {
                    return Err("invalid_manifest".into());
                }
            } else {
                others.insert(chunk);
            }
        }
    }
    let plan = RemotePlan {
        target,
        data_keys: targets
            .iter()
            .filter(|(key, _)| !others.contains(*key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        manifest_keys: manifests,
        shared_keys: targets
            .keys()
            .filter(|key| others.contains(*key))
            .cloned()
            .collect(),
    };
    if !valid_remote_plan(&plan) {
        return Err("invalid_manifest".into());
    }
    Ok(plan)
}

async fn delete_remote_keys(
    config: &crate::object_store::Config,
    keys: &crate::object_store::Keys,
    plan: &RemotePlan,
) -> (Vec<String>, Vec<String>) {
    if !valid_remote_plan(plan) {
        return (Vec::new(), vec!["remote_plan_invalid".into()]);
    }
    let mut removed = Vec::new();
    let mut remaining = Vec::new();
    for (key, hash) in &plan.data_keys {
        if delete_remote_key(config, keys, key, hash).await {
            removed.push(key.clone());
        } else {
            remaining.push(key.clone());
        }
    }
    // Keep all manifests discoverable if even one exclusive chunk could not be deleted.
    if remaining.is_empty() {
        for (key, hash) in &plan.manifest_keys {
            if delete_remote_key(config, keys, key, hash).await {
                removed.push(key.clone());
            } else {
                remaining.push(key.clone());
            }
        }
    } else {
        remaining.extend(plan.manifest_keys.keys().cloned());
    }
    remaining.extend(plan.shared_keys.iter().map(|key| format!("shared:{key}")));
    (removed, remaining)
}

pub(crate) async fn delete_remote_key(
    config: &crate::object_store::Config,
    keys: &crate::object_store::Keys,
    key: &str,
    expected_hash: &str,
) -> bool {
    let header = match config.provider.as_str() {
        "aws" => Some(reqwest::header::ETAG.as_str()),
        "gcp" => Some("x-goog-generation"),
        "cloudflare" if crate::r2_oauth::uses_oauth(config) => None,
        // These APIs do not document an atomic conditional delete. Do not
        // guess: keep the exact key in remoteRemaining for the caller.
        _ => return false,
    };
    let Ok(mut response) =
        crate::object_store::request(config, keys, reqwest::Method::GET, key, &[], vec![]).await
    else {
        return false;
    };
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return true;
    }
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|n| n > 128 * 1024 * 1024)
    {
        return false;
    }
    let condition = match header {
        Some(header) => match response.headers().get(header).and_then(|v| v.to_str().ok()) {
            Some(value) => Some(value.to_owned()),
            None => return false,
        },
        None => None,
    };
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk))
                if chunk.len() <= (128 * 1024 * 1024usize).saturating_sub(bytes.len()) =>
            {
                bytes.extend_from_slice(&chunk)
            }
            Ok(None) => break,
            _ => return false,
        }
    }
    if crate::object_store::digest(&bytes) != expected_hash {
        return false;
    }
    let result = if let Some(condition) = condition {
        crate::object_store::conditional_delete(config, keys, key, &condition).await
    } else {
        crate::object_store::request(config, keys, reqwest::Method::DELETE, key, &[], vec![]).await
    };
    if !result
        .is_ok_and(|r| r.status().is_success() || r.status() == reqwest::StatusCode::NOT_FOUND)
    {
        return false;
    }
    crate::object_store::request(config, keys, reqwest::Method::HEAD, key, &[], vec![])
        .await
        .is_ok_and(|r| r.status() == reqwest::StatusCode::NOT_FOUND)
}

fn safe_directory(path: &Path, create: bool) -> Result<(), String> {
    if create {
        match fs::create_dir(path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err("local_write_failed".into()),
        }
    }
    let meta = fs::symlink_metadata(path).map_err(|_| "local_read_failed")?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("unsafe_path".into());
    }
    Ok(())
}

fn locations(
    root: &Path,
    hash: &str,
    operation: &str,
    create_trash: bool,
) -> Result<(PathBuf, PathBuf), String> {
    if !crate::archive_zet::valid_asset_id(&format!("sha256:{hash}"))
        || uuid::Uuid::parse_str(operation).is_err()
    {
        return Err("invalid_asset".into());
    }
    let git = root.join(".git");
    safe_directory(&git, false)?;
    let trash = git.join("momso-source-trash");
    safe_directory(&trash, create_trash)?;
    let slot = trash.join(operation);
    safe_directory(&slot, create_trash)?;
    let objects = root.join("objects");
    safe_directory(&objects, false)?;
    let sha = objects.join("sha256");
    safe_directory(&sha, true)?;
    let prefix = sha.join(&hash[..2]);
    safe_directory(&prefix, true)?;
    Ok((prefix.join(hash), slot.join("original")))
}

fn exists_valid(path: &Path, expected_hash: &str) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !crate::archive_location::is_link_or_reparse(&meta) => {
            let (hash, _) = crate::object_store_transfer::hash_file(path).map_err(str::to_owned)?;
            if hash != expected_hash {
                return Err("local_conflict".into());
            }
            Ok(true)
        }
        Ok(_) => Err("unsafe_path".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("local_read_failed".into()),
    }
}

#[cfg(test)]
pub(crate) fn trash_for_test(
    root: &Path,
    database: &Path,
    asset_id: &str,
    expected_digest: &str,
) -> Result<(), String> {
    trash(root, database, asset_id, expected_digest).map(|_| ())
}

fn trash(
    root: &Path,
    database: &Path,
    asset_id: &str,
    expected_digest: &str,
) -> Result<DeleteResult, String> {
    let references = preview(root, database, asset_id)?;
    if references.plan_digest.as_deref() != Some(expected_digest) {
        return Err("references_changed".into());
    }
    let conn = crate::storage::open_writable_connection(database).map_err(|_| "unavailable")?;
    let existing: Option<(String, String, String)> = conn
        .query_row(
            "SELECT operation_id,state,retain_until FROM source_asset_deletions WHERE asset_id=?1",
            [&references.asset_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|_| "unavailable")?;
    let (operation, state, _retain_until) = if let Some(existing) = existing {
        existing
    } else {
        if references.state != "active" {
            return Err("invalid_state".into());
        }
        let operation = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        // Legacy schema requires a timestamp. The app never automatically
        // deletes these files; this sentinel is not an expiry promise.
        let retain_until = "9999-12-31T23:59:59Z".to_owned();
        conn.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until) VALUES(?1,?2,'pending',?3,?4)",
            rusqlite::params![references.asset_id,operation,now.to_rfc3339(),retain_until]).map_err(|_| "unavailable")?;
        (operation, "pending".to_owned(), retain_until)
    };
    if state == "trashed" {
        return Ok(DeleteResult {
            state: "trashed",
            asset_id: references.asset_id,
            retain_until: None,
        });
    }
    if state != "pending" {
        return Err("invalid_state".into());
    }
    let (source, saved) = locations(root, &references.sha256, &operation, true)?;
    match (
        exists_valid(&source, &references.sha256)?,
        exists_valid(&saved, &references.sha256)?,
    ) {
        (true, false) => {
            move_without_replacement(&source, &saved).map_err(|_| "local_write_failed")?
        }
        (false, true) => (),
        (true, true) => return Err("local_conflict".into()),
        (false, false) => return Err("local_missing".into()),
    }
    let mut conn = conn;
    let tx = conn.transaction().map_err(|_| "unavailable")?;
    if tx.execute("UPDATE momso_source_assets SET integrity_state='quarantined',integrity_issue='user_deleted',quarantined_at=?1,quarantined_by='user' WHERE id=?2 AND integrity_state='active'", rusqlite::params![chrono::Utc::now().to_rfc3339(),references.asset_id]).map_err(|_| "unavailable")? != 1 { return Err("invalid_state".into()); }
    if tx.execute("UPDATE source_asset_deletions SET state='trashed' WHERE asset_id=?1 AND state='pending'", [&references.asset_id]).map_err(|_| "unavailable")? != 1 { return Err("invalid_state".into()); }
    tx.commit().map_err(|_| "unavailable")?;
    Ok(DeleteResult {
        state: "trashed",
        asset_id: references.asset_id,
        retain_until: None,
    })
}

// A concurrent writer must never be able to make a move replace its file.
#[cfg(windows)]
pub(crate) fn move_without_replacement(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileW(existing: *const u16, new: *const u16) -> i32;
    }
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileW(from.as_ptr(), to.as_ptr()) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub(crate) fn move_without_replacement(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::hard_link(from, to)?;
    // On failure preserve both links: recovery treats this as a conflict.
    fs::remove_file(from)
}

fn restore_trash(root: &Path, database: &Path, asset_id: &str) -> Result<DeleteResult, String> {
    let mut conn = crate::storage::open_writable_connection(database).map_err(|_| "unavailable")?;
    let lookup = crate::source_assets::reference_lookup(asset_id).ok_or("not_found")?;
    let row: Option<(String,String,String,String)> = conn.query_row("SELECT d.operation_id,d.state,a.content_sha256,a.id FROM source_asset_deletions d JOIN momso_source_assets a ON a.id=d.asset_id WHERE d.asset_id=?1 OR a.content_sha256=?2", rusqlite::params![asset_id, lookup], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|_| "unavailable")?;
    let (operation, state, hash, canonical_id) = row.ok_or("not_found")?;
    let asset_id = canonical_id.as_str();
    if !["pending", "trashed"].contains(&state.as_str()) {
        return Err("invalid_state".into());
    }
    let (source, saved) = locations(root, &hash, &operation, true)?;
    match (exists_valid(&source, &hash)?, exists_valid(&saved, &hash)?) {
        (false, true) => {
            move_without_replacement(&saved, &source).map_err(|_| "local_write_failed")?
        }
        (true, false) => (),
        (true, true) => return Err("local_conflict".into()),
        (false, false) => return Err("local_missing".into()),
    }
    let mut permissions = fs::metadata(&source)
        .map_err(|_| "local_read_failed")?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&source, permissions).map_err(|_| "local_write_failed")?;
    let tx = conn.transaction().map_err(|_| "unavailable")?;
    if tx.execute("UPDATE momso_source_assets SET integrity_state='active',integrity_issue=NULL,quarantined_at=NULL,quarantined_by=NULL WHERE id=?1 AND (integrity_state='active' OR integrity_issue='user_deleted')", [asset_id]).map_err(|_| "unavailable")? != 1 { return Err("invalid_state".into()); }
    if tx
        .execute(
            "DELETE FROM source_asset_deletions WHERE asset_id=?1",
            [asset_id],
        )
        .map_err(|_| "unavailable")?
        != 1
    {
        return Err("invalid_state".into());
    }
    tx.commit().map_err(|_| "unavailable")?;
    if let Some(slot) = saved.parent() {
        let _ = fs::remove_dir(slot);
    }
    Ok(DeleteResult {
        state: "restored",
        asset_id: asset_id.into(),
        retain_until: None,
    })
}

fn remove_owned_file(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|_| "local_read_failed")?;
    if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("unsafe_path".into());
    }
    #[cfg(windows)]
    if meta.permissions().readonly() {
        crate::source_assets::clear_readonly_attribute(path).map_err(|_| "local_write_failed")?;
    }
    fs::remove_file(path).map_err(|_| "local_write_failed".into())
}

fn purge_local(
    root: &Path,
    backups: &Path,
    data_root: &Path,
    hash: &str,
    operation: &str,
    identity: Option<&age::x25519::Identity>,
    preserve_remote: impl FnMut(
        &str,
        &crate::object_store_transfer::Manifest,
        Option<&[u8]>,
    ) -> Result<(), &'static str>,
) -> Vec<String> {
    let mut remaining = Vec::new();
    match locations(root, hash, operation, true) {
        Ok((source, saved)) => {
            if source.exists() {
                remaining.push("original_path_conflict".into());
            }
            match exists_valid(&saved, hash) {
                Ok(true) => {
                    if remove_owned_file(&saved).is_err() {
                        remaining.push(saved.to_string_lossy().into_owned());
                    }
                }
                Ok(false) => (),
                Err(_) => remaining.push(saved.to_string_lossy().into_owned()),
            }
        }
        Err(_) => remaining.push("trash_unavailable".into()),
    }
    match fs::read_dir(backups) {
        Ok(entries) => {
            for entry in entries {
                let Ok(entry) = entry else {
                    remaining.push("recovery_copies_unverified".into());
                    continue;
                };
                if !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("momso-source-recovery-")
                {
                    continue;
                }
                let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                    remaining.push("recovery_copies_unverified".into());
                    continue;
                };
                if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
                    remaining.push("recovery_copies_unverified".into());
                    continue;
                }
                let Ok(files) = fs::read_dir(entry.path()) else {
                    remaining.push("recovery_copies_unverified".into());
                    continue;
                };
                let mut removed_matching = false;
                for file in files {
                    let Ok(file) = file else {
                        remaining.push("recovery_copies_unverified".into());
                        continue;
                    };
                    let path = file.path();
                    let Ok(meta) = fs::symlink_metadata(&path) else {
                        remaining.push("recovery_copies_unverified".into());
                        continue;
                    };
                    if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
                        remaining.push("recovery_copies_unverified".into());
                        continue;
                    }
                    match crate::object_store_transfer::hash_file(&path) {
                        Ok((actual, _)) if actual == hash => {
                            if remove_owned_file(&path).is_err() {
                                remaining.push(path.to_string_lossy().into_owned());
                            } else {
                                removed_matching = true;
                            }
                        }
                        Ok(_) => (),
                        Err(_) => remaining.push(path.to_string_lossy().into_owned()),
                    }
                }
                if removed_matching {
                    let _ = fs::remove_dir(entry.path());
                }
            }
        }
        Err(_) => remaining.push("recovery_copies_unverified".into()),
    }
    remaining.extend(crate::object_store_transfer::purge_staged_source(
        data_root,
        hash,
        identity,
        preserve_remote,
    ));
    remaining.push("external_copies_unverified".into());
    remaining.sort();
    remaining.dedup();
    remaining
}

#[tauri::command]
pub(crate) async fn source_asset_trash(
    state: tauri::State<'_, crate::AppState>,
    asset_id: String,
    plan_digest: String,
) -> Result<DeleteResult, String> {
    let paths = state.paths.scoped().await;
    let active = state.chat_active.lock().await;
    if !active.is_empty() {
        return Err("chat_active".into());
    }
    let _connection = crate::object_store::CONNECTION_LOCK.lock().await;
    let _guard = crate::chat_file_changes::lock_app_write();
    trash(
        paths.objects_path.parent().ok_or("unavailable")?,
        &paths.database_path,
        &asset_id,
        &plan_digest,
    )
}

#[tauri::command]
pub(crate) async fn source_asset_restore_trashed(
    state: tauri::State<'_, crate::AppState>,
    asset_id: String,
) -> Result<DeleteResult, String> {
    let paths = state.paths.scoped().await;
    let active = state.chat_active.lock().await;
    if !active.is_empty() {
        return Err("chat_active".into());
    }
    let _connection = crate::object_store::CONNECTION_LOCK.lock().await;
    let root = paths.objects_path.parent().ok_or("unavailable")?;
    let local = {
        let _guard = crate::chat_file_changes::lock_app_write();
        restore_trash(root, &paths.database_path, &asset_id)
    };
    match local {
        Ok(result) => Ok(result),
        Err(error) if error == "local_missing" => {
            let config =
                crate::object_store::load_config(&paths.data_root)?.ok_or("not_connected")?;
            let keys = crate::object_store::load_keys(&config)?;
            let identity = crate::object_store_transfer::identity(&paths.database_path)?;
            restore_trashed_from_remote(
                root,
                &paths.database_path,
                &asset_id,
                &config,
                &keys,
                &identity,
            )
            .await
        }
        Err(error) => Err(error),
    }
}

async fn restore_trashed_from_remote(
    root: &Path,
    database: &Path,
    asset_id: &str,
    config: &crate::object_store::Config,
    keys: &crate::object_store::Keys,
    identity: &age::x25519::Identity,
) -> Result<DeleteResult, String> {
    let (canonical_id, hash) = {
        let conn = crate::storage::open_writable_connection(database).map_err(|_| "unavailable")?;
        let lookup = crate::source_assets::reference_lookup(asset_id).ok_or("not_found")?;
        conn.query_row("SELECT a.id,a.content_sha256 FROM source_asset_deletions d JOIN momso_source_assets a ON a.id=d.asset_id WHERE (d.asset_id=?1 OR a.content_sha256=?2) AND d.state IN ('pending','trashed')", rusqlite::params![asset_id, lookup], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).map_err(|_| "not_found")?
    };
    if !crate::archive_zet::valid_asset_id(&format!("sha256:{hash}")) {
        return Err("invalid_asset".into());
    }
    let mut manifest = crate::object_store_transfer::catalog(config, keys, identity)
        .await?
        .into_iter()
        .filter(|item| item.kind == "object" && item.sha256 == hash)
        .max_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then(a.upload_id.cmp(&b.upload_id))
        })
        .ok_or("remote_missing")?;
    manifest.relative_path = format!("sha256/{}/{hash}", &hash[..2]);
    crate::object_store_restore::restore_one(
        config,
        keys,
        identity,
        &manifest,
        &root.join("objects"),
    )
    .await?;
    let _guard = crate::chat_file_changes::lock_app_write();
    restore_trash(root, database, &canonical_id)
}

async fn purge_remote(
    database: &Path,
    asset_id: &str,
    hash: &str,
    mut saved_plan: Option<String>,
    config: &crate::object_store::Config,
    keys: &crate::object_store::Keys,
    identity: &age::x25519::Identity,
) -> Result<(Vec<String>, Vec<String>), &'static str> {
    let target = remote_target(config, identity);
    let mut plan = if let Some(json) = &saved_plan {
        parse_bound_plan(json, &target)?
    } else {
        RemotePlan {
            target: target.clone(),
            ..RemotePlan::default()
        }
    };
    if saved_plan.is_none() {
        let initial = serde_json::to_string(&plan).map_err(|_| "remote_plan_invalid")?;
        let conn = crate::storage::open_writable_connection(database).map_err(|_| "unavailable")?;
        if conn.execute("UPDATE source_asset_deletions SET remote_plan_json=?1 WHERE asset_id=?2 AND state='purge_pending' AND remote_plan_json IS NULL", rusqlite::params![initial,asset_id]).map_err(|_| "unavailable")? != 1 {
            return Err("remote_plan_changed");
        }
        saved_plan = Some(initial);
    }
    let catalog = crate::object_store_transfer::catalog_with_keys(config, keys, identity).await?;
    let fresh = remote_plan(&catalog, hash, target).map_err(|_| "invalid_manifest")?;
    for (key, value) in fresh.data_keys {
        if plan
            .data_keys
            .insert(key, value.clone())
            .is_some_and(|old| old != value)
        {
            return Err("remote_plan_invalid");
        }
    }
    for (key, value) in fresh.manifest_keys {
        if plan
            .manifest_keys
            .insert(key, value.clone())
            .is_some_and(|old| old != value)
        {
            return Err("remote_plan_invalid");
        }
    }
    let mut shared: BTreeSet<String> = plan
        .shared_keys
        .into_iter()
        .chain(fresh.shared_keys)
        .collect();
    for (_, manifest, _) in &catalog {
        if manifest.kind != "object" || manifest.sha256 != hash {
            for index in 0..manifest.chunks.len() {
                let key = crate::object_store_transfer::chunk_key(manifest, index);
                if plan.data_keys.contains_key(&key) {
                    shared.insert(key);
                }
            }
        }
    }
    for key in &shared {
        plan.data_keys.remove(key);
    }
    plan.shared_keys = shared.into_iter().collect();
    let json = serde_json::to_string(&plan).map_err(|_| "remote_plan_invalid")?;
    {
        let conn = crate::storage::open_writable_connection(database).map_err(|_| "unavailable")?;
        if conn.execute("UPDATE source_asset_deletions SET remote_plan_json=?1 WHERE asset_id=?2 AND state='purge_pending' AND remote_plan_json IS ?3", rusqlite::params![json,asset_id,saved_plan]).map_err(|_| "unavailable")? != 1 { return Err("remote_plan_changed"); }
    }
    if !remote_deletion_supported(config) {
        let remaining = plan
            .data_keys
            .keys()
            .chain(plan.manifest_keys.keys())
            .cloned()
            .chain(plan.shared_keys.iter().map(|key| format!("shared:{key}")))
            .collect();
        return Ok((Vec::new(), remaining));
    }
    Ok(delete_remote_keys(config, keys, &plan).await)
}

fn finish_purge(
    database: &Path,
    asset_id: &str,
    mut local_remaining: Vec<String>,
    remote: Result<(Vec<String>, Vec<String>), &'static str>,
    remote_deletable: bool,
) -> Result<PurgeResult, String> {
    local_remaining.push("database_backups_may_contain_metadata".into());
    local_remaining.sort();
    local_remaining.dedup();
    let mut remote_remaining = vec![
        "object_versions_unverified".to_owned(),
        "provider_soft_delete_or_retention_unverified".to_owned(),
        "unfinished_uploads_unverified".to_owned(),
        "other_stores_unverified".to_owned(),
        "app_state_backups_may_contain_metadata".to_owned(),
    ];
    let (removed_remote_keys, retry_remote, remote_complete) = match remote {
        Ok((removed, remaining)) => {
            let retry =
                remote_deletable && remaining.iter().any(|item| !item.starts_with("shared:"));
            let complete = remaining.is_empty();
            remote_remaining.extend(remaining);
            (removed, retry, complete)
        }
        Err(reason) => {
            remote_remaining.push(format!("remote_access:{reason}"));
            (
                Vec::new(),
                remote_deletable
                    && !matches!(reason, "remote_target_changed" | "remote_plan_invalid"),
                false,
            )
        }
    };
    let retry_local = local_retry_needed(&local_remaining);
    let state = if retry_local || retry_remote {
        "purge_pending"
    } else {
        "purged"
    };
    let result_state = if retry_local {
        "local_retry_pending"
    } else if retry_remote {
        "remote_retry_pending"
    } else {
        "partial"
    };
    let mut conn = crate::storage::open_writable_connection(database).map_err(|_| "unavailable")?;
    let remote_json = serde_json::to_string(&remote_remaining).map_err(|_| "unavailable")?;
    let local_json = serde_json::to_string(&local_remaining).map_err(|_| "unavailable")?;
    let tx = conn.transaction().map_err(|_| "unavailable")?;
    if tx.execute("UPDATE source_asset_deletions SET state=?1,remote_remaining_json=?2,local_remaining_json=?3 WHERE asset_id=?4 AND state='purge_pending'", rusqlite::params![state,remote_json,local_json,asset_id]).map_err(|_| "unavailable")? != 1 {
        return Err("invalid_state".into());
    }
    if remote_complete && !retry_local {
        tx.execute("UPDATE momso_source_assets SET original_file_name='deleted',media_type=NULL,captured_by='redacted' WHERE id=?1", [asset_id]).map_err(|_| "unavailable")?;
    }
    tx.commit().map_err(|_| "unavailable")?;
    Ok(PurgeResult {
        state: result_state,
        asset_id: asset_id.to_owned(),
        local_remaining,
        remote_remaining,
        removed_remote_keys,
    })
}

// Called with the connection and remote source-mutation locks already held.
pub(crate) async fn retry_pending_for_backup(
    paths: &crate::runtime::RuntimePaths,
    config: &crate::object_store::Config,
    keys: &crate::object_store::Keys,
    identity: &age::x25519::Identity,
) -> Result<(), &'static str> {
    if !remote_deletion_supported(config) {
        return Ok(());
    }
    let rows = {
        let conn = crate::storage::open_writable_connection(&paths.database_path)
            .map_err(|_| "unavailable")?;
        let mut stmt = conn.prepare("SELECT d.asset_id,a.content_sha256,d.operation_id,d.state,d.remote_remaining_json FROM source_asset_deletions d JOIN momso_source_assets a ON a.id=d.asset_id WHERE d.state IN ('purge_pending','purged') ORDER BY d.requested_at,d.asset_id").map_err(|_| "unavailable")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(|_| "unavailable")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "unavailable")?;
        rows
    };
    let root = paths.objects_path.parent().ok_or("unavailable")?;
    for (asset_id, hash, operation, state, remote_json) in rows {
        if state == "purged" {
            let Some(json) = remote_json else { continue };
            let remaining: Vec<String> = serde_json::from_str(&json).map_err(|_| "unavailable")?;
            if !remaining.iter().any(|value| {
                matches!(
                    value.as_str(),
                    "remote_access:not_connected" | "remote_access:remote_target_changed"
                )
            }) {
                continue;
            }
            let conn = crate::storage::open_writable_connection(&paths.database_path)
                .map_err(|_| "unavailable")?;
            let saved: Option<String> = conn.query_row("SELECT remote_plan_json FROM source_asset_deletions WHERE asset_id=?1 AND state='purged'", [&asset_id], |row| row.get(0)).map_err(|_| "unavailable")?;
            let Some(plan) = saved.as_deref() else {
                continue;
            };
            if parse_bound_plan(plan, &remote_target(config, identity)).is_err() {
                continue;
            }
            if conn.execute("UPDATE source_asset_deletions SET state='purge_pending' WHERE asset_id=?1 AND state='purged' AND remote_remaining_json=?2", rusqlite::params![asset_id,json]).map_err(|_| "unavailable")? != 1 {
                return Err("invalid_state");
            }
        }
        let local = {
            let _guard = crate::chat_file_changes::lock_app_write();
            purge_local(
                root,
                &paths.backups_path,
                &paths.data_root,
                &hash,
                &operation,
                Some(identity),
                |stage, manifest, encrypted| {
                    persist_staged_plan(
                        (&paths.database_path, &asset_id, &hash),
                        config,
                        identity,
                        stage,
                        manifest,
                        encrypted,
                    )
                },
            )
        };
        let saved: Option<String> = crate::storage::open_writable_connection(&paths.database_path).map_err(|_| "unavailable")?.query_row("SELECT remote_plan_json FROM source_asset_deletions WHERE asset_id=?1 AND state='purge_pending'", [&asset_id], |row| row.get(0)).map_err(|_| "unavailable")?;
        let remote = if saved.is_some() {
            purge_remote(
                &paths.database_path,
                &asset_id,
                &hash,
                saved,
                config,
                keys,
                identity,
            )
            .await
        } else {
            Err("remote_target_changed")
        };
        finish_purge(&paths.database_path, &asset_id, local, remote, true)
            .map_err(|_| "unavailable")?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn source_asset_purge(
    state: tauri::State<'_, crate::AppState>,
    asset_id: String,
    plan_digest: String,
    confirmed: bool,
) -> Result<PurgeResult, String> {
    if !confirmed {
        return Err("confirmation_required".into());
    }
    let paths = state.paths.scoped().await;
    let active = state.chat_active.lock().await;
    if !active.is_empty() {
        return Err("chat_active".into());
    }
    let _connection = crate::object_store::CONNECTION_LOCK.lock().await;
    let root = paths.objects_path.parent().ok_or("unavailable")?;
    let references = {
        let _guard = crate::chat_file_changes::lock_app_write();
        let refs = preview(root, &paths.database_path, &asset_id)?;
        if refs.plan_digest.as_deref() != Some(&plan_digest) {
            return Err("references_changed".into());
        }
        if refs.state != "purge_pending" {
            trash(root, &paths.database_path, &asset_id, &plan_digest)?;
        }
        refs
    };
    let operation = {
        let conn = crate::storage::open_writable_connection(&paths.database_path)
            .map_err(|_| "unavailable")?;
        if conn.execute("UPDATE source_asset_deletions SET state='purge_pending' WHERE asset_id=?1 AND state IN ('trashed','purge_pending')", [&references.asset_id]).map_err(|_| "unavailable")? != 1 { return Err("invalid_state".into()); }
        conn.query_row(
            "SELECT operation_id FROM source_asset_deletions WHERE asset_id=?1",
            [&references.asset_id],
            |r| r.get::<_, String>(0),
        )
        .map_err(|_| "unavailable")?
    };
    let local_remaining = {
        let _guard = crate::chat_file_changes::lock_app_write();
        let identity = crate::object_store_transfer::identity(&paths.database_path).ok();
        let config = crate::object_store::load_config(&paths.data_root)
            .ok()
            .flatten();
        purge_local(
            root,
            &paths.backups_path,
            &paths.data_root,
            &references.sha256,
            &operation,
            identity.as_ref(),
            |stage_id, manifest, encrypted_manifest| {
                persist_staged_plan(
                    (
                        &paths.database_path,
                        &references.asset_id,
                        &references.sha256,
                    ),
                    config.as_ref().ok_or("not_connected")?,
                    identity.as_ref().ok_or("unavailable")?,
                    stage_id,
                    manifest,
                    encrypted_manifest,
                )
            },
        )
    };
    let connection = crate::object_store::load_config(&paths.data_root);
    let remote_deletable = match &connection {
        Ok(Some(config)) => remote_deletion_supported(config),
        Ok(None) => false,
        Err(_) => true,
    };
    let remote = async {
        let config = connection?.ok_or("not_connected")?;
        let keys = crate::object_store::load_keys(&config)?;
        let remote_lock = if crate::object_store_lock::supported(&config) {
            Some(crate::object_store_lock::SourceMutationLock::acquire(&paths.data_root, &config, &keys).await?)
        } else {
            None
        };
        let work = async {
        let identity = crate::object_store_transfer::identity(&paths.database_path)?;
        let saved_plan: Option<String> = crate::storage::open_writable_connection(&paths.database_path)
            .map_err(|_| "unavailable")?.query_row(
                "SELECT remote_plan_json FROM source_asset_deletions WHERE asset_id=?1 AND state='purge_pending'",
                [&references.asset_id], |row| row.get(0),
            ).map_err(|_| "unavailable")?;
        purge_remote(
            &paths.database_path,
            &references.asset_id,
            &references.sha256,
            saved_plan,
            &config,
            &keys,
            &identity,
        )
        .await
        }.await;
        if let Some(lock) = remote_lock {
            lock.release(&config, &keys).await?;
        }
        work
    }
    .await;
    finish_purge(
        &paths.database_path,
        &references.asset_id,
        local_remaining,
        remote,
        remote_deletable,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn oauth_r2_upload_purge_failure_and_retry_preserves_other_assets() {
        use std::{
            collections::BTreeMap,
            io::{Read, Write},
            net::TcpListener,
            sync::{
                atomic::{AtomicBool, Ordering},
                Arc, Mutex,
            },
        };
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        for folder in ["objects", "zettels", "receipts", ".git"] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        let database = temp.path().join("state.db");
        crate::storage::initialize(&database, &temp.path().join("backups"), "test").unwrap();
        let hash = "a".repeat(64);
        let conn = crate::storage::open_writable_connection(&database).unwrap();
        conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset','other',?1,1,?2,'lesson.wav','test','now')", rusqlite::params![hash,format!("sha256/aa/{hash}")]).unwrap();
        conn.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until) VALUES('asset',?1,'purge_pending','now','later')", [uuid::Uuid::new_v4().to_string()]).unwrap();
        drop(conn);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let objects = Arc::new(Mutex::new(BTreeMap::<String, Vec<u8>>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let (server_objects, server_stop, server_failed) =
            (objects.clone(), stop.clone(), failed.clone());
        let server = std::thread::spawn(move || {
            let base = format!(
                "/client/v4/accounts/{}/r2/buckets/momso-fixture/objects",
                "a".repeat(32)
            );
            while !server_stop.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 8192];
                let head_end = loop {
                    let n = stream.read(&mut buffer).unwrap();
                    assert_ne!(n, 0);
                    request.extend_from_slice(&buffer[..n]);
                    if let Some(i) = request.windows(4).position(|v| v == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let head = String::from_utf8(request[..head_end].to_vec()).unwrap();
                assert!(head
                    .lines()
                    .any(|line| line.eq_ignore_ascii_case("authorization: Bearer test-token")));
                let mut first = head.lines().next().unwrap().split_whitespace();
                let method = first.next().unwrap();
                let path = first.next().unwrap();
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|n| n.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while request.len() - head_end < length {
                    let n = stream.read(&mut buffer).unwrap();
                    assert_ne!(n, 0);
                    request.extend_from_slice(&buffer[..n]);
                }
                let mut map = server_objects.lock().unwrap();
                let (status, body) = if method == "GET" && path.starts_with(&format!("{base}?")) {
                    let entries: Vec<_> = map
                        .keys()
                        .filter(|key| key.contains("/manifests/"))
                        .map(|key| serde_json::json!({"key":key}))
                        .collect();
                    ("200 OK", serde_json::to_vec(&serde_json::json!({"success":true,"result":entries,"result_info":{"is_truncated":false}})).unwrap())
                } else {
                    let key = path
                        .strip_prefix(&format!("{base}/"))
                        .expect("R2 object path");
                    match method {
                        "PUT" => {
                            map.insert(key.into(), request[head_end..head_end + length].to_vec());
                            ("200 OK", b"{}".to_vec())
                        }
                        "GET" => map
                            .get(key)
                            .map(|value| ("200 OK", value.clone()))
                            .unwrap_or(("404 Not Found", vec![])),
                        "DELETE" if !server_failed.swap(true, Ordering::SeqCst) => {
                            ("403 Forbidden", vec![])
                        }
                        "DELETE" => {
                            map.remove(key);
                            ("200 OK", b"{}".to_vec())
                        }
                        _ => panic!("unexpected method"),
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
        let config = crate::object_store::Config {
            provider: "cloudflare".into(),
            endpoint: format!("http://{address}"),
            region: "wrangleroauth".into(),
            bucket: "momso-fixture".into(),
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(crate::object_store::WriteSafety::CheckThenWrite),
        };
        let keys = crate::object_store::Keys {
            access_key_id: "fixture".into(),
            secret_access_key: "test-token".into(),
        };
        let identity = age::x25519::Identity::generate();
        let mut target_keys = Vec::new();
        let mut other_keys = Vec::new();
        for (name, source_hash, bytes) in [
            ("target", hash.clone(), b"target cipher".to_vec()),
            ("other", "b".repeat(64), b"other cipher".to_vec()),
        ] {
            let manifest = crate::object_store_transfer::Manifest {
                version: 1,
                kind: "object".into(),
                relative_path: name.into(),
                sha256: source_hash,
                size: 1,
                upload_id: uuid::Uuid::new_v4().to_string(),
                chunks: vec![crate::object_store::digest(&bytes)],
                created_at: "2026-01-01T00:00:00Z".into(),
            };
            let data_key = crate::object_store_transfer::chunk_key(&manifest, 0);
            let manifest_key = format!("{}/manifests/{name}.age", crate::object_store::PREFIX);
            let encrypted = crate::object_store_transfer::encrypt_bytes(
                &serde_json::to_vec(&manifest).unwrap(),
                &identity,
            )
            .unwrap();
            for (key, value) in [(&data_key, bytes), (&manifest_key, encrypted)] {
                assert!(crate::object_store::request(
                    &config,
                    &keys,
                    reqwest::Method::PUT,
                    key,
                    &[],
                    value
                )
                .await
                .unwrap()
                .status()
                .is_success());
            }
            if name == "target" {
                target_keys.extend([data_key, manifest_key]);
            } else {
                other_keys.extend([data_key, manifest_key]);
            }
        }
        let (removed, remaining) =
            purge_remote(&database, "asset", &hash, None, &config, &keys, &identity)
                .await
                .unwrap();
        assert!(!remaining.is_empty());
        assert_eq!(
            finish_purge(&database, "asset", vec![], Ok((removed, remaining)), true)
                .unwrap()
                .state,
            "remote_retry_pending"
        );
        let target = remote_target(&config, &identity);
        let page = list_deleted_with_target(&root, &database, None, 40, Some(&target)).unwrap();
        assert_eq!(page.items.len(), 1);
        let saved: String = crate::storage::open_writable_connection(&database)
            .unwrap()
            .query_row(
                "SELECT remote_plan_json FROM source_asset_deletions WHERE asset_id='asset'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let (removed, remaining) = purge_remote(
            &database,
            "asset",
            &hash,
            Some(saved),
            &config,
            &keys,
            &identity,
        )
        .await
        .unwrap();
        assert!(remaining.is_empty());
        finish_purge(&database, "asset", vec![], Ok((removed, remaining)), true).unwrap();
        assert!(
            list_deleted_with_target(&root, &database, None, 40, Some(&target))
                .unwrap()
                .items
                .is_empty()
        );
        let map = objects.lock().unwrap();
        assert!(target_keys.iter().all(|key| !map.contains_key(key)));
        assert!(other_keys.iter().all(|key| map.contains_key(key)));
        drop(map);
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn failed_remote_lookup_saves_bound_retry_target() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("state.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        let hash = "a".repeat(64);
        conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset','other',?1,1,?2,'lesson.wav','test','now')", rusqlite::params![hash,format!("sha256/aa/{hash}")]).unwrap();
        conn.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until) VALUES('asset',?1,'purge_pending','now','later')", [uuid::Uuid::new_v4().to_string()]).unwrap();
        drop(conn);
        let config = crate::object_store::Config {
            provider: "aws".into(),
            endpoint: "http://127.0.0.1:1".into(),
            region: "auto".into(),
            bucket: "fixture".into(),
            credential_id: "fixture".into(),
            last_backup_at: None,
            write_safety: Some(crate::object_store::WriteSafety::Conditional),
        };
        let keys = crate::object_store::Keys {
            access_key_id: "fixture".into(),
            secret_access_key: "fixture".into(),
        };
        let identity = age::x25519::Identity::generate();
        assert!(
            purge_remote(&db, "asset", &hash, None, &config, &keys, &identity)
                .await
                .is_err()
        );
        let plan: String = crate::storage::open_writable_connection(&db)
            .unwrap()
            .query_row(
                "SELECT remote_plan_json FROM source_asset_deletions WHERE asset_id='asset'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(parse_bound_plan(&plan, &remote_target(&config, &identity)).is_ok());
        let mut other = config.clone();
        other.bucket = "different".into();
        assert!(parse_bound_plan(&plan, &remote_target(&other, &identity)).is_err());
    }

    #[tokio::test]
    async fn preview_counts_archived_chats_but_not_permanently_deleted_chats() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        for name in ["zettels", "receipts", "objects", "chats", ".git"] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        let db = temp.path().join("state.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let first = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let second = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let hash = crate::object_store::digest(b"source bytes");
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset-1','other',?1,12,?2,'photo.jpg','test','now')", rusqlite::params![hash, format!("sha256/{}/{hash}", &hash[..2])]).unwrap();
        drop(conn);
        let object = root.join("objects/sha256").join(&hash[..2]).join(&hash);
        fs::create_dir_all(object.parent().unwrap()).unwrap();
        fs::write(object, b"source bytes").unwrap();
        crate::archive_receipt::attach_asset(&root, &db, &first, "asset-1").unwrap();
        crate::archive_receipt::attach_asset(&root, &db, &second, "asset-1").unwrap();
        let active: crate::chat_codex::Active = Default::default();
        assert_eq!(
            crate::chat::archive_if_idle(&db, &active, &second)
                .await
                .state,
            "ok"
        );
        assert_eq!(preview(&root, &db, "asset-1").unwrap().chats.len(), 2);
        let deleted = crate::chat::delete_with(&db, &active, &second, |_| async { Ok(()) }).await;
        assert_eq!(serde_json::to_value(deleted).unwrap()["state"], "deleted");
        crate::chat_archive_store::remove(&root, &db, &second).unwrap();
        let references = preview(&root, &db, "asset-1").unwrap();
        assert_eq!(references.chats, vec![first]);
        assert_eq!(references.receipts.len(), 2);
        assert!(root.join("chats").join(format!("{second}.json")).exists());
    }

    #[test]
    fn preview_excludes_deleted_document_with_stale_index() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        for name in ["zettels", "receipts", "objects", ".git"] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        let db = temp.path().join("state.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let hash = crate::object_store::digest(b"source bytes");
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset-1','other',?1,12,?2,'photo.jpg','test','now')", rusqlite::params![hash, format!("sha256/{}/{hash}", &hash[..2])]).unwrap();
        drop(conn);
        let id = crate::archive_zet::create(
            &root,
            &db,
            crate::archive_zet::ZetCreate {
                title: "lesson".into(),
                summary: "summary".into(),
                kind: "note".into(),
                body: "text".into(),
                assets: vec![format!("sha256:{hash}")],
                edges: vec![],
            },
        )
        .unwrap();
        let file = root.join("zettels").join(format!("{id}.md"));
        assert_eq!(preview(&root, &db, "asset-1").unwrap().documents.len(), 1);
        fs::remove_file(file).unwrap();
        assert!(preview(&root, &db, "asset-1").unwrap().documents.is_empty());
    }

    #[test]
    fn move_never_replaces_a_destination() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::write(&source, b"original").unwrap();
        fs::write(&destination, b"other data").unwrap();
        assert!(move_without_replacement(&source, &destination).is_err());
        assert_eq!(fs::read(&source).unwrap(), b"original");
        assert_eq!(fs::read(&destination).unwrap(), b"other data");
    }
    #[test]
    fn preview_finds_documents_receipts_and_chats_without_touching_source() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        for name in ["zettels", "receipts", "objects"] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        fs::create_dir(root.join(".git")).unwrap();
        let db = temp.path().join("state.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let hash = crate::object_store::digest(b"source bytes");
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset-1','other',?1,12,?2,'photo.jpg','test','now')", rusqlite::params![hash,format!("sha256/{}/{hash}",&hash[..2])]).unwrap();
        drop(conn);
        let object = root.join("objects/sha256").join(&hash[..2]).join(&hash);
        fs::create_dir_all(object.parent().unwrap()).unwrap();
        fs::write(&object, b"source bytes").unwrap();
        let mut original_permissions = fs::metadata(&object).unwrap().permissions();
        original_permissions.set_readonly(true);
        fs::set_permissions(&object, original_permissions).unwrap();
        fs::write(
            root.join("zettels/lesson.md"),
            format!("reference sha256:{hash}"),
        )
        .unwrap();
        let zet = crate::archive_zet::create(
            &root,
            &db,
            crate::archive_zet::ZetCreate {
                title: "lesson".into(),
                summary: "summary".into(),
                kind: "note".into(),
                body: "text".into(),
                assets: vec![format!("sha256:{hash}")],
                edges: vec![],
            },
        )
        .unwrap();
        let receipt = crate::archive_receipt::create(
            &root,
            &db,
            crate::archive_receipt::ReceiptCreate {
                tool: "codex".into(),
                model: String::new(),
                chat: Some(chat.clone()),
                read: vec![format!("sha256:{hash}")],
                wrote: vec![],
                prompt: String::new(),
                changes: String::new(),
            },
        )
        .unwrap();
        let second_chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        conn.execute("INSERT INTO chat_attachments(receipt_id,chat_id,asset_id) VALUES('direct',?1,'asset-1')", [&second_chat]).unwrap();
        conn.execute("INSERT INTO momso_records(id,record_kind,title,status,current_version,body_sha256,body_bytes,created_by,updated_by,created_at,updated_at) VALUES('record-1','note','title','draft',1,'hash',0,'test','test','now','now')", []).unwrap();
        conn.execute("INSERT INTO momso_record_versions(record_id,version,record_kind,title,status,body_sha256,body_bytes,change_kind,actor,created_at) VALUES('record-1',1,'note','title','draft','hash',0,'create','test','now')", []).unwrap();
        conn.execute("INSERT INTO momso_record_source_assets(record_id,record_version,asset_id,relation_role,linked_by,created_at) VALUES('record-1',1,'asset-1','source','test','now')", []).unwrap();
        drop(conn);
        let result = preview(&root, &db, "asset-1").unwrap();
        let reference = format!("sha256:{hash}");
        let from_reference = preview(&root, &db, &reference).unwrap();
        let from_raw_hash = preview(&root, &db, &hash).unwrap();
        assert_eq!(from_reference.asset_id, "asset-1");
        assert_eq!(from_reference.plan_digest, result.plan_digest);
        assert_eq!(from_raw_hash.plan_digest, result.plan_digest);
        assert!(matches!(preview(&root, &db, "sha256:not-a-hash"), Err(e) if e == "not_found"));
        assert!(result.documents.contains(&"zettels/lesson.md".to_owned()));
        assert!(result.documents.contains(&format!("zettels/{zet}.md")));
        assert_eq!(result.receipts, [receipt]);
        assert!(result.chats.contains(&chat) && result.chats.contains(&second_chat));
        assert_eq!(result.record_versions.len(), 1);
        assert_eq!(result.record_versions[0].record_id, "record-1");
        assert_eq!(fs::read(&object).unwrap(), b"source bytes");
        let stale = result.plan_digest.unwrap();
        fs::write(root.join("zettels/another.md"), format!("sha256:{hash}")).unwrap();
        assert!(
            matches!(trash(&root, &db, "asset-1", &stale), Err(e) if e == "references_changed")
        );
        let current = preview(&root, &db, "asset-1").unwrap();
        let deleted = trash(
            &root,
            &db,
            &reference,
            current.plan_digest.as_deref().unwrap(),
        )
        .unwrap();
        assert_eq!(deleted.state, "trashed");
        assert!(deleted.retain_until.is_none());
        assert!(!object.exists());
        assert_eq!(preview(&root, &db, "asset-1").unwrap().state, "trashed");
        let page = serde_json::to_value(list_deleted(&root, &db, None, 40).unwrap()).unwrap();
        assert_eq!(page["items"][0]["id"], "asset-1");
        assert_eq!(page["items"][0]["originalFileName"], "photo.jpg");
        assert_eq!(page["items"][0]["deletionState"], "trashed");
        assert_eq!(page["items"][0]["canRestore"], true);
        assert_eq!(page["items"][0]["canPurge"], true);
        assert_eq!(page["items"][0]["referenceCounts"]["receipts"], 1);
        assert_eq!(page["items"][0]["referenceCounts"]["chats"], 2);
        assert_eq!(page["items"][0]["referenceCounts"]["recordVersions"], 1);
        assert_eq!(page["hasMore"], false);
        let linked =
            serde_json::to_value(crate::archive_zet::linked(&root, &db, &zet).unwrap()).unwrap();
        assert_eq!(linked["assets"][0]["id"], reference);
        assert_eq!(linked["assets"][0]["deleted"], true);
        let items =
            serde_json::to_value(crate::chat_items::list(&root, &db, &chat).unwrap()).unwrap();
        assert!(items["deleted"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == format!("sha256:{hash}") && item["how"] == "deleted"));
        let receipt_view = serde_json::to_value(
            crate::archive_receipt::read(&root, &db, &result.receipts[0]).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt_view["head"]["read"][0]["deleted"], true);
        assert_eq!(
            restore_trash(&root, &db, &reference).unwrap().state,
            "restored"
        );
        assert_eq!(fs::read(&object).unwrap(), b"source bytes");
        assert!(list_deleted(&root, &db, None, 40).unwrap().items.is_empty());
        assert!(fs::metadata(&object).unwrap().permissions().readonly());
        let operation = uuid::Uuid::new_v4().to_string();
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        conn.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until) VALUES('asset-1',?1,'pending','now','later')", [&operation]).unwrap();
        drop(conn);
        let (_, saved) = locations(&root, &hash, &operation, true).unwrap();
        fs::rename(&object, &saved).unwrap();
        let pending = preview(&root, &db, "asset-1").unwrap();
        assert_eq!(
            trash(
                &root,
                &db,
                "asset-1",
                pending.plan_digest.as_deref().unwrap()
            )
            .unwrap()
            .state,
            "trashed"
        );
        assert_eq!(
            restore_trash(&root, &db, "asset-1").unwrap().state,
            "restored"
        );
        assert_eq!(fs::read(&object).unwrap(), b"source bytes");
        let current = preview(&root, &db, "asset-1").unwrap();
        trash(
            &root,
            &db,
            "asset-1",
            current.plan_digest.as_deref().unwrap(),
        )
        .unwrap();
        fs::write(&object, b"newer local file").unwrap();
        assert!(matches!(restore_trash(&root,&db,"asset-1"), Err(e) if e == "local_conflict"));
        assert_eq!(fs::read(&object).unwrap(), b"newer local file");
        fs::remove_file(&object).unwrap();
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        let operation: String = conn
            .query_row(
                "SELECT operation_id FROM source_asset_deletions WHERE asset_id='asset-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        drop(conn);
        let backup = temp.path().join("backups");
        let own_copy = backup.join("momso-source-recovery-our");
        let other_copy = backup.join("momso-source-recovery-other");
        fs::create_dir_all(&own_copy).unwrap();
        fs::create_dir_all(&other_copy).unwrap();
        fs::write(own_copy.join("original"), b"source bytes").unwrap();
        fs::write(other_copy.join("original"), b"other bytes").unwrap();
        let remaining = purge_local(
            &root,
            &backup,
            temp.path(),
            &hash,
            &operation,
            None,
            |_, _, _| Err("unavailable"),
        );
        assert!(remaining
            .iter()
            .any(|value| value == "external_copies_unverified"));
        assert!(!own_copy.join("original").exists());
        assert_eq!(
            fs::read(other_copy.join("original")).unwrap(),
            b"other bytes"
        );
        let (_, saved) = locations(&root, &hash, &operation, true).unwrap();
        assert!(!saved.exists());
        // Recreate only this synthetic fixture so the temporary directory can be checked normally.
        fs::write(&object, b"source bytes").unwrap();
        #[cfg(windows)]
        {
            crate::source_assets::clear_readonly_attribute(&object).unwrap();
        }
        fs::write(root.join("receipts/broken.md"), "broken").unwrap();
        assert!(matches!(preview(&root, &db, "asset-1"), Err(e) if e == "references_unavailable"));
    }

    #[test]
    fn deleted_list_uses_stable_cursor_and_preserves_partial_purge_results() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        for folder in ["objects", "zettels", "receipts"] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        let db = temp.path().join("state.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        for (id, hash, state) in [
            ("a", "a".repeat(64), "trashed"),
            ("b", "b".repeat(64), "purge_pending"),
            ("c", "c".repeat(64), "pending"),
            ("d", "d".repeat(64), "purge_pending"),
        ] {
            conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES(?1,'other',?2,1,?3,'source.txt','test','2026-01-01T00:00:00Z')", rusqlite::params![id,hash,format!("sha256/{}/{hash}",&hash[..2])]).unwrap();
            let local = if id == "d" {
                r#"["external_copies_unverified"]"#
            } else {
                r#"["local-copy"]"#
            };
            conn.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until,local_remaining_json,remote_remaining_json) VALUES(?1,?2,?3,'2026-01-02T00:00:00Z','old',?4,'[\"remote-copy\"]')", rusqlite::params![id,format!("operation-{id}"),state,local]).unwrap();
        }
        drop(conn);
        let first = list_deleted(&root, &db, None, 1).unwrap();
        let first_json = serde_json::to_value(&first).unwrap();
        assert_eq!(first_json["items"][0]["id"], "c");
        assert_eq!(first_json["hasMore"], true);
        let second = list_deleted(&root, &db, first.next_cursor, 1).unwrap();
        let second_json = serde_json::to_value(&second).unwrap();
        assert_eq!(second_json["items"][0]["id"], "b");
        assert_eq!(second_json["items"][0]["canRestore"], false);
        assert_eq!(second_json["items"][0]["canPurge"], true);
        assert_eq!(second_json["items"][0]["remoteRemaining"][0], "remote-copy");
        let third =
            serde_json::to_value(list_deleted(&root, &db, second.next_cursor, 1).unwrap()).unwrap();
        assert_eq!(third["items"][0]["id"], "a");
        assert_eq!(third["hasMore"], false);
        assert!(third["nextCursor"].is_null());
    }

    #[test]
    fn purge_without_connected_store_leaves_no_actionable_list_item() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        for folder in ["objects", "zettels", "receipts", ".git"] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        let backups = temp.path().join("backups");
        fs::create_dir_all(&backups).unwrap();
        let db = temp.path().join("state.db");
        crate::storage::initialize(&db, &backups, "test").unwrap();
        let bytes = b"private source";
        let hash = crate::object_store::digest(bytes);
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset','other',?1,?2,?3,'lesson.wav','test','now')", rusqlite::params![hash,bytes.len() as i64,format!("sha256/{}/{hash}",&hash[..2])]).unwrap();
        drop(conn);
        let object = root.join("objects/sha256").join(&hash[..2]).join(&hash);
        fs::create_dir_all(object.parent().unwrap()).unwrap();
        fs::write(&object, bytes).unwrap();
        let plan = preview(&root, &db, "asset").unwrap().plan_digest.unwrap();
        trash(&root, &db, "asset", &plan).unwrap();
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        conn.execute(
            "UPDATE source_asset_deletions SET state='purge_pending' WHERE asset_id='asset'",
            [],
        )
        .unwrap();
        let operation: String = conn
            .query_row(
                "SELECT operation_id FROM source_asset_deletions WHERE asset_id='asset'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        drop(conn);
        let local = purge_local(
            &root,
            &backups,
            temp.path(),
            &hash,
            &operation,
            None,
            |_, _, _| Err("not_connected"),
        );
        let result = finish_purge(&db, "asset", local, Err("not_connected"), false).unwrap();
        assert_eq!(result.state, "partial");
        assert!(result
            .remote_remaining
            .iter()
            .any(|v| v == "remote_access:not_connected"));
        assert!(!object.exists());
        assert!(list_deleted(&root, &db, None, 40).unwrap().items.is_empty());
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        let (state, name): (String, String) = conn.query_row("SELECT d.state,a.original_file_name FROM source_asset_deletions d JOIN momso_source_assets a ON a.id=d.asset_id WHERE d.asset_id='asset'", [], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
        assert_eq!(state, "purged");
        assert_eq!(name, "lesson.wav");
    }

    #[tokio::test]
    async fn backup_retries_confirmed_purge_after_reconnection_but_not_trash() {
        use std::io::{Read, Write};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        for folder in ["objects", "zettels", "receipts", ".git"] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        let backups = temp.path().join("backups");
        fs::create_dir_all(&backups).unwrap();
        let database = temp.path().join("state.db");
        crate::storage::initialize(&database, &backups, "test").unwrap();
        let conn = crate::storage::open_writable_connection(&database).unwrap();
        for (id, hash, state, remaining) in [
            (
                "confirmed",
                "a".repeat(64),
                "purged",
                Some(r#"["remote_access:not_connected"]"#),
            ),
            ("trash", "b".repeat(64), "trashed", None),
            (
                "other",
                "c".repeat(64),
                "purged",
                Some(r#"["remote_access:remote_target_changed"]"#),
            ),
            (
                "unbound",
                "d".repeat(64),
                "purged",
                Some(r#"["remote_access:not_connected"]"#),
            ),
        ] {
            conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES(?1,'other',?2,1,?3,'lesson.wav','test','now')", rusqlite::params![id,hash,format!("sha256/{}/{hash}",&hash[..2])]).unwrap();
            conn.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until,remote_remaining_json) VALUES(?1,?2,?3,'now','later',?4)", rusqlite::params![id,uuid::Uuid::new_v4().to_string(),state,remaining]).unwrap();
        }
        let other_plan = RemotePlan {
            target: "different-target".into(),
            ..RemotePlan::default()
        };
        conn.execute(
            "UPDATE source_asset_deletions SET remote_plan_json=?1 WHERE asset_id='other'",
            [serde_json::to_string(&other_plan).unwrap()],
        )
        .unwrap();
        drop(conn);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut request = [0u8; 8192];
            let count = stream.read(&mut request).unwrap();
            assert!(String::from_utf8_lossy(&request[..count]).starts_with("GET /bucket?"));
            let body = "<ListBucketResult><IsTruncated>false</IsTruncated></ListBucketResult>";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let config = crate::object_store::Config {
            provider: "aws".into(),
            endpoint: format!("http://{address}"),
            region: "auto".into(),
            bucket: "bucket".into(),
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(crate::object_store::WriteSafety::Conditional),
        };
        let keys = crate::object_store::Keys {
            access_key_id: "TEST".into(),
            secret_access_key: "test".into(),
        };
        let identity = age::x25519::Identity::generate();
        let plan = RemotePlan {
            target: remote_target(&config, &identity),
            ..RemotePlan::default()
        };
        crate::storage::open_writable_connection(&database)
            .unwrap()
            .execute(
                "UPDATE source_asset_deletions SET remote_plan_json=?1 WHERE asset_id='confirmed'",
                [serde_json::to_string(&plan).unwrap()],
            )
            .unwrap();
        let archive =
            crate::archive_location::ArchiveLocation::load(database.clone(), temp.path(), vec![])
                .unwrap();
        let paths = crate::runtime::RuntimePaths {
            data_root: temp.path().to_path_buf(),
            database_path: database.clone(),
            objects_path: root.join("objects"),
            exports_path: temp.path().join("exports"),
            backups_path: backups,
            logs_path: temp.path().join("logs"),
            archive,
            archive_lease: None,
        };
        retry_pending_for_backup(&paths, &config, &keys, &identity)
            .await
            .unwrap();
        server.join().unwrap();
        let conn = crate::storage::open_writable_connection(&database).unwrap();
        let confirmed: (String, String) = conn.query_row("SELECT d.state,a.original_file_name FROM source_asset_deletions d JOIN momso_source_assets a ON a.id=d.asset_id WHERE d.asset_id='confirmed'", [], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
        let trash: (String, String) = conn.query_row("SELECT d.state,a.original_file_name FROM source_asset_deletions d JOIN momso_source_assets a ON a.id=d.asset_id WHERE d.asset_id='trash'", [], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
        let other: (String, String) = conn.query_row("SELECT d.state,a.original_file_name FROM source_asset_deletions d JOIN momso_source_assets a ON a.id=d.asset_id WHERE d.asset_id='other'", [], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
        let unbound: (String, String) = conn.query_row("SELECT d.state,a.original_file_name FROM source_asset_deletions d JOIN momso_source_assets a ON a.id=d.asset_id WHERE d.asset_id='unbound'", [], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
        assert_eq!(confirmed, ("purged".into(), "deleted".into()));
        assert_eq!(trash, ("trashed".into(), "lesson.wav".into()));
        assert_eq!(other, ("purged".into(), "lesson.wav".into()));
        assert_eq!(unbound, ("purged".into(), "lesson.wav".into()));
    }

    #[test]
    fn purge_staged_remote_plan_is_saved_and_bound_to_its_connection() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("state.db");
        crate::storage::initialize(&database, &temp.path().join("backups"), "test").unwrap();
        let hash = "a".repeat(64);
        let conn = crate::storage::open_writable_connection(&database).unwrap();
        conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset','other',?1,1,?2,'photo','test','now')", rusqlite::params![hash,format!("sha256/aa/{hash}")]).unwrap();
        conn.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until) VALUES('asset',?1,'purge_pending','now','later')", [uuid::Uuid::new_v4().to_string()]).unwrap();
        drop(conn);
        let credential = uuid::Uuid::new_v4().to_string();
        let config = crate::object_store::Config {
            provider: "s3_compatible".into(),
            endpoint: "http://127.0.0.1:1".into(),
            region: "auto".into(),
            bucket: "bucket".into(),
            credential_id: credential.clone(),
            last_backup_at: None,
            write_safety: Some(crate::object_store::WriteSafety::Conditional),
        };
        let identity = age::x25519::Identity::generate();
        let manifest = crate::object_store_transfer::Manifest {
            version: 2,
            kind: "object".into(),
            relative_path: "sha256/item".into(),
            sha256: hash.clone(),
            size: 1,
            upload_id: uuid::Uuid::new_v4().to_string(),
            chunks: vec!["b".repeat(64)],
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        assert_eq!(
            persist_staged_plan(
                (&database, "asset", &hash),
                &config,
                &identity,
                "wrong",
                &manifest,
                None
            ),
            Err("remote_target_changed")
        );
        let encrypted_manifest = b"synthetic encrypted manifest";
        persist_staged_plan(
            (&database, "asset", &hash),
            &config,
            &identity,
            &credential,
            &manifest,
            Some(encrypted_manifest),
        )
        .unwrap();
        let conn = crate::storage::open_writable_connection(&database).unwrap();
        let saved: String = conn
            .query_row(
                "SELECT remote_plan_json FROM source_asset_deletions WHERE asset_id='asset'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let plan = parse_bound_plan(&saved, &remote_target(&config, &identity)).unwrap();
        assert_eq!(
            plan.data_keys
                .get(&crate::object_store_transfer::chunk_key(&manifest, 0)),
            Some(&manifest.chunks[0])
        );
        let key = crate::object_store_transfer::manifest_key(
            &config,
            &manifest.created_at,
            &manifest.upload_id,
            encrypted_manifest,
        );
        assert_eq!(
            plan.manifest_keys.get(&key),
            Some(&crate::object_store::digest(encrypted_manifest))
        );
        let mut changed = config.clone();
        changed.bucket = "another-bucket".into();
        assert!(matches!(
            parse_bound_plan(&saved, &remote_target(&changed, &identity)),
            Err("remote_target_changed")
        ));
    }

    #[test]
    fn remote_plan_keeps_chunks_used_by_other_manifests() {
        let hash = "a".repeat(64);
        let target_manifest = format!("{}/manifests/target.age", crate::object_store::PREFIX);
        let other_manifest = format!("{}/manifests/other.age", crate::object_store::PREFIX);
        let mut target = crate::object_store_transfer::Manifest {
            version: 1,
            kind: "object".into(),
            relative_path: "one".into(),
            sha256: hash.clone(),
            size: 1,
            upload_id: uuid::Uuid::new_v4().to_string(),
            chunks: vec!["b".repeat(64)],
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let mut other = target.clone();
        other.sha256 = "c".repeat(64);
        other.relative_path = "other".into();
        let shared = crate::object_store_transfer::chunk_key(&target, 0);
        let plan = remote_plan(
            &[
                (target_manifest.clone(), target.clone(), "d".repeat(64)),
                (other_manifest.clone(), other, "e".repeat(64)),
            ],
            &hash,
            "target".into(),
        )
        .unwrap();
        assert!(plan.data_keys.is_empty());
        assert_eq!(plan.shared_keys, [shared]);
        assert!(plan.manifest_keys.contains_key(&target_manifest));
        assert!(!plan.manifest_keys.contains_key(&other_manifest));
        let saved = serde_json::to_string(&plan).unwrap();
        assert!(parse_bound_plan(&saved, "target").is_ok());
        assert!(matches!(
            parse_bound_plan(&saved, "different-bucket"),
            Err("remote_target_changed")
        ));
        let mut tampered = serde_json::to_value(&plan).unwrap();
        tampered["data_keys"] = serde_json::json!({"unrelated/private": "a".repeat(64)});
        assert!(matches!(
            parse_bound_plan(&tampered.to_string(), "target"),
            Err("remote_plan_invalid")
        ));
        target.chunks[0] = "f".repeat(64);
        assert!(remote_plan(
            &[
                (target_manifest, target.clone(), "d".repeat(64)),
                (
                    other_manifest,
                    {
                        let mut m = target.clone();
                        m.chunks[0] = "b".repeat(64);
                        m
                    },
                    "e".repeat(64)
                )
            ],
            &hash,
            "target".into()
        )
        .is_err());
    }

    #[tokio::test]
    async fn conditional_delete_removes_only_the_verified_generation() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            sync::{Arc, Mutex},
        };
        for (provider, concurrent) in [("aws", true), ("gcp", true), ("aws", false), ("gcp", false)]
        {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let current = Arc::new(Mutex::new(b"original".to_vec()));
            let remote = current.clone();
            let expected_header = if provider == "aws" {
                "if-match: \"old\""
            } else {
                "x-goog-if-generation-match: 17"
            };
            let expected_signed = if provider == "aws" {
                "signedheaders=host;if-match;"
            } else {
                ";x-goog-if-generation-match"
            };
            let server = std::thread::spawn(move || {
                for index in 0..if concurrent { 2 } else { 3 } {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                        .unwrap();
                    let mut received = Vec::new();
                    let mut buffer = [0u8; 2048];
                    while !received.windows(4).any(|part| part == b"\r\n\r\n") {
                        let n = stream.read(&mut buffer).unwrap();
                        assert_ne!(n, 0);
                        received.extend_from_slice(&buffer[..n]);
                    }
                    let headers = String::from_utf8(received).unwrap().to_ascii_lowercase();
                    if index == 0 {
                        assert!(headers.starts_with("get "));
                        let body = remote.lock().unwrap().clone();
                        write!(stream, "HTTP/1.1 200 OK\r\nETag: \"old\"\r\nx-goog-generation: 17\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                        stream.write_all(&body).unwrap();
                    } else if index == 1 {
                        assert!(headers.starts_with("delete "));
                        assert!(headers.contains(expected_header));
                        assert!(headers.contains(expected_signed));
                        if concurrent {
                            *remote.lock().unwrap() = b"replacement".to_vec();
                            stream.write_all(b"HTTP/1.1 412 Precondition Failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                        } else {
                            remote.lock().unwrap().clear();
                            stream.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                        }
                    } else {
                        assert!(headers.starts_with("head "));
                        stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                    }
                }
            });
            let config = crate::object_store::Config {
                provider: provider.into(),
                endpoint: format!("http://{address}"),
                region: "auto".into(),
                bucket: "bucket".into(),
                credential_id: "test".into(),
                last_backup_at: None,
                write_safety: Some(crate::object_store::WriteSafety::Conditional),
            };
            let keys = crate::object_store::Keys {
                access_key_id: "TEST".into(),
                secret_access_key: "test".into(),
            };
            let removed = delete_remote_key(
                &config,
                &keys,
                "momso/object-store/v1/data/test",
                &crate::object_store::digest(b"original"),
            )
            .await;
            assert_eq!(removed, !concurrent);
            server.join().unwrap();
            assert_eq!(
                *current.lock().unwrap(),
                if concurrent {
                    b"replacement".to_vec()
                } else {
                    Vec::new()
                }
            );
        }
    }

    #[tokio::test]
    async fn malformed_delete_conditions_never_reach_the_network() {
        let keys = crate::object_store::Keys {
            access_key_id: "TEST".into(),
            secret_access_key: "test".into(),
        };
        for (provider, conditions) in [
            ("aws", vec!["*", "W/\"old\"", "\"old\",\"*\"", "\"a\nb\""]),
            ("gcp", vec!["0", "00", "-1", "1,2", "18446744073709551616"]),
        ] {
            let config = crate::object_store::Config {
                provider: provider.into(),
                endpoint: "http://127.0.0.1:1".into(),
                region: "auto".into(),
                bucket: "bucket".into(),
                credential_id: "test".into(),
                last_backup_at: None,
                write_safety: Some(crate::object_store::WriteSafety::Conditional),
            };
            for condition in conditions {
                assert!(matches!(
                    crate::object_store::conditional_delete(
                        &config,
                        &keys,
                        "momso/object-store/v1/data/test",
                        condition
                    )
                    .await,
                    Err("delete_condition_unverified")
                ));
            }
        }
    }

    #[tokio::test]
    async fn undocumented_conditional_delete_preserves_remote_key() {
        for provider in ["cloudflare", "ncp", "s3_compatible"] {
            let config = crate::object_store::Config {
                provider: provider.into(),
                endpoint: "http://127.0.0.1:1".into(),
                region: "auto".into(),
                bucket: "bucket".into(),
                credential_id: "test".into(),
                last_backup_at: None,
                write_safety: Some(crate::object_store::WriteSafety::Conditional),
            };
            let keys = crate::object_store::Keys {
                access_key_id: "TEST".into(),
                secret_access_key: "test".into(),
            };
            let key = "momso/object-store/v1/data/00000000-0000-4000-8000-000000000001/00000000";
            let plan = RemotePlan {
                target: "test".into(),
                data_keys: BTreeMap::from([(key.into(), "a".repeat(64))]),
                ..RemotePlan::default()
            };
            let (removed, remaining) = delete_remote_keys(&config, &keys, &plan).await;
            assert!(removed.is_empty());
            assert_eq!(remaining, [key]);
        }
    }

    #[tokio::test]
    async fn remote_purge_reports_denied_keys_and_leaves_other_objects_untouched() {
        use std::{
            collections::BTreeMap,
            io::{Read, Write},
            net::TcpListener,
            sync::{Arc, Mutex},
        };
        let data_key = "momso/object-store/v1/data/00000000-0000-4000-8000-000000000001/00000000";
        let manifest_key = "momso/object-store/v1/manifests/target.age";
        let other_key = "momso/object-store/v1/data/00000000-0000-4000-8000-000000000002/00000000";
        let objects = Arc::new(Mutex::new(BTreeMap::from([
            (data_key.to_owned(), b"cipher".to_vec()),
            (manifest_key.to_owned(), b"manifest".to_vec()),
            (other_key.to_owned(), b"other".to_vec()),
        ])));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let remote = objects.clone();
        let requests = seen.clone();
        let server = std::thread::spawn(move || {
            let mut denied_once = false;
            for _ in 0..9 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                    .unwrap();
                let mut received = Vec::new();
                let mut buffer = [0u8; 2048];
                while !received.windows(4).any(|w| w == b"\r\n\r\n") {
                    let size = stream.read(&mut buffer).unwrap();
                    assert_ne!(size, 0);
                    received.extend_from_slice(&buffer[..size]);
                }
                let head = String::from_utf8(received).unwrap();
                let mut first = head.lines().next().unwrap().split_whitespace();
                let method = first.next().unwrap();
                let path = first.next().unwrap();
                assert!(path.starts_with("/bucket/momso/"));
                let key = path.strip_prefix("/bucket/").unwrap().to_owned();
                requests
                    .lock()
                    .unwrap()
                    .push((method.to_owned(), key.clone()));
                let mut all = remote.lock().unwrap();
                let (status, body) = match method {
                    "GET" => match all.get(&key) {
                        Some(bytes) => ("200 OK", bytes.clone()),
                        None => ("404 Not Found", vec![]),
                    },
                    "DELETE" if key == manifest_key && !denied_once => {
                        denied_once = true;
                        ("403 Forbidden", vec![])
                    }
                    "DELETE" => {
                        all.remove(&key);
                        ("204 No Content", vec![])
                    }
                    "HEAD" => (
                        if all.contains_key(&key) {
                            "200 OK"
                        } else {
                            "404 Not Found"
                        },
                        vec![],
                    ),
                    _ => panic!("unexpected method"),
                };
                if method == "DELETE" {
                    assert!(head.to_ascii_lowercase().contains("if-match: \"fixture\""));
                    assert!(head
                        .to_ascii_lowercase()
                        .contains("signedheaders=host;if-match;"));
                }
                let header = format!(
                    "HTTP/1.1 {status}\r\nETag: \"fixture\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let config = crate::object_store::Config {
            provider: "aws".into(),
            endpoint: format!("http://{address}"),
            region: "auto".into(),
            bucket: "bucket".into(),
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(crate::object_store::WriteSafety::Conditional),
        };
        let keys = crate::object_store::Keys {
            access_key_id: "TEST".into(),
            secret_access_key: "test".into(),
        };
        let plan = RemotePlan {
            target: "test".into(),
            data_keys: BTreeMap::from([(data_key.into(), crate::object_store::digest(b"cipher"))]),
            manifest_keys: BTreeMap::from([(
                manifest_key.into(),
                crate::object_store::digest(b"manifest"),
            )]),
            shared_keys: vec![],
        };
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        for folder in ["objects", "zettels", "receipts"] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        let database = temp.path().join("state.db");
        crate::storage::initialize(&database, &temp.path().join("backups"), "test").unwrap();
        let conn = crate::storage::open_writable_connection(&database).unwrap();
        let hash = "a".repeat(64);
        conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset','other',?1,1,?2,'lesson.wav','test','now')", rusqlite::params![hash,format!("sha256/aa/{hash}")]).unwrap();
        conn.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until) VALUES('asset',?1,'purge_pending','now','later')", [uuid::Uuid::new_v4().to_string()]).unwrap();
        drop(conn);
        let mut stored = config.clone();
        stored.endpoint = "https://s3.us-east-1.amazonaws.com".into();
        stored.region = "us-east-1".into();
        stored.credential_id = uuid::Uuid::new_v4().to_string();
        crate::object_store::save_config(temp.path(), &stored).unwrap();
        let (removed, remaining) = delete_remote_keys(&config, &keys, &plan).await;
        assert_eq!(removed, [data_key]);
        assert_eq!(remaining, [manifest_key]);
        let outcome = finish_purge(
            &database,
            "asset",
            vec!["external_copies_unverified".into()],
            Ok((removed, remaining)),
            true,
        )
        .unwrap();
        assert_eq!(outcome.state, "remote_retry_pending");
        crate::storage::open_writable_connection(&database)
            .unwrap()
            .execute(
                "UPDATE source_asset_deletions SET remote_plan_json=?1 WHERE asset_id='asset'",
                [serde_json::to_string(&plan).unwrap()],
            )
            .unwrap();
        assert!(
            list_deleted_with_target(&root, &database, None, 40, Some("other"))
                .unwrap()
                .items
                .is_empty()
        );
        let page = serde_json::to_value(
            list_deleted_with_target(&root, &database, None, 40, Some("test")).unwrap(),
        )
        .unwrap();
        assert_eq!(page["items"][0]["originalFileName"], "lesson.wav");
        assert_eq!(page["items"][0]["deletionState"], "remote_retry_pending");
        assert_eq!(page["items"][0]["canPurge"], true);
        let (removed, remaining) = delete_remote_keys(&config, &keys, &plan).await;
        assert!(remaining.is_empty());
        finish_purge(
            &database,
            "asset",
            vec!["external_copies_unverified".into()],
            Ok((removed, remaining)),
            true,
        )
        .unwrap();
        assert!(list_deleted(&root, &database, None, 40)
            .unwrap()
            .items
            .is_empty());
        let conn = crate::storage::open_writable_connection(&database).unwrap();
        let name: String = conn
            .query_row(
                "SELECT original_file_name FROM momso_source_assets WHERE id='asset'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "deleted");
        server.join().unwrap();
        assert_eq!(objects.lock().unwrap().get(other_key).unwrap(), b"other");
        assert!(!seen.lock().unwrap().iter().any(|(_, key)| key == other_key));
    }

    #[tokio::test]
    async fn purge_catalog_plan_is_persisted_before_deletion_and_bound_to_bucket() {
        use std::{
            collections::BTreeMap,
            io::{Read, Write},
            net::TcpListener,
            sync::{Arc, Mutex},
        };
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("state.db");
        crate::storage::initialize(&database, &temp.path().join("backups"), "test").unwrap();
        let hash = "a".repeat(64);
        let conn = crate::storage::open_writable_connection(&database).unwrap();
        conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset','other',?1,1,?2,'photo','test','now')", rusqlite::params![hash,format!("sha256/aa/{hash}")]).unwrap();
        conn.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until) VALUES('asset',?1,'purge_pending','now','later')", [uuid::Uuid::new_v4().to_string()]).unwrap();
        drop(conn);
        let identity = age::x25519::Identity::generate();
        let target_data = b"target cipher".to_vec();
        let other_data = b"other cipher".to_vec();
        let target = crate::object_store_transfer::Manifest {
            version: 1,
            kind: "object".into(),
            relative_path: "target".into(),
            sha256: hash.clone(),
            size: 1,
            upload_id: uuid::Uuid::new_v4().to_string(),
            chunks: vec![crate::object_store::digest(&target_data)],
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let other = crate::object_store_transfer::Manifest {
            version: 1,
            kind: "object".into(),
            relative_path: "other".into(),
            sha256: "b".repeat(64),
            size: 1,
            upload_id: uuid::Uuid::new_v4().to_string(),
            chunks: vec![crate::object_store::digest(&other_data)],
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let target_key = format!("{}/manifests/target.age", crate::object_store::PREFIX);
        let other_key = format!("{}/manifests/other.age", crate::object_store::PREFIX);
        let target_chunk = crate::object_store_transfer::chunk_key(&target, 0);
        let other_chunk = crate::object_store_transfer::chunk_key(&other, 0);
        let objects = Arc::new(Mutex::new(BTreeMap::from([
            (
                target_key.clone(),
                crate::object_store_transfer::encrypt_bytes(
                    &serde_json::to_vec(&target).unwrap(),
                    &identity,
                )
                .unwrap(),
            ),
            (
                other_key.clone(),
                crate::object_store_transfer::encrypt_bytes(
                    &serde_json::to_vec(&other).unwrap(),
                    &identity,
                )
                .unwrap(),
            ),
            (target_chunk.clone(), target_data),
            (other_chunk.clone(), other_data),
        ])));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let remote = objects.clone();
        let server = std::thread::spawn(move || {
            for _ in 0..9 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                    .unwrap();
                let mut received = Vec::new();
                let mut buffer = [0u8; 2048];
                while !received.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buffer).unwrap();
                    assert_ne!(n, 0);
                    received.extend_from_slice(&buffer[..n]);
                }
                let head = String::from_utf8(received).unwrap();
                let mut first = head.lines().next().unwrap().split_whitespace();
                let method = first.next().unwrap();
                let path = first.next().unwrap();
                let mut all = remote.lock().unwrap();
                let (status, body) = if path.starts_with("/bucket?") {
                    let entries = all
                        .keys()
                        .filter(|key| key.contains("/manifests/"))
                        .map(|key| format!("<Contents><Key>{key}</Key></Contents>"))
                        .collect::<String>();
                    ("200 OK",format!("<ListBucketResult><IsTruncated>false</IsTruncated>{entries}</ListBucketResult>").into_bytes())
                } else {
                    let key = path.strip_prefix("/bucket/").unwrap();
                    match method {
                        "GET" => match all.get(key) {
                            Some(bytes) => ("200 OK", bytes.clone()),
                            None => ("404 Not Found", vec![]),
                        },
                        "DELETE" => {
                            all.remove(key);
                            ("204 No Content", vec![])
                        }
                        "HEAD" => (
                            if all.contains_key(key) {
                                "200 OK"
                            } else {
                                "404 Not Found"
                            },
                            vec![],
                        ),
                        _ => panic!("unexpected method"),
                    }
                };
                if method == "DELETE" {
                    assert!(head.to_ascii_lowercase().contains("if-match: \"fixture\""));
                    assert!(head
                        .to_ascii_lowercase()
                        .contains("signedheaders=host;if-match;"));
                }
                let header = format!(
                    "HTTP/1.1 {status}\r\nETag: \"fixture\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let config = crate::object_store::Config {
            provider: "aws".into(),
            endpoint: format!("http://{address}"),
            region: "auto".into(),
            bucket: "bucket".into(),
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(crate::object_store::WriteSafety::Conditional),
        };
        let keys = crate::object_store::Keys {
            access_key_id: "TEST".into(),
            secret_access_key: "test".into(),
        };
        let (removed, remaining) =
            purge_remote(&database, "asset", &hash, None, &config, &keys, &identity)
                .await
                .unwrap();
        server.join().unwrap();
        assert!(remaining.is_empty());
        assert!(removed.contains(&target_key) && removed.contains(&target_chunk));
        assert!(objects.lock().unwrap().contains_key(&other_key));
        assert!(objects.lock().unwrap().contains_key(&other_chunk));
        let conn = crate::storage::open_writable_connection(&database).unwrap();
        let saved: String = conn
            .query_row(
                "SELECT remote_plan_json FROM source_asset_deletions WHERE asset_id='asset'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        drop(conn);
        let mut different = config.clone();
        different.bucket = "different".into();
        assert!(matches!(
            purge_remote(
                &database,
                "asset",
                &hash,
                Some(saved),
                &different,
                &keys,
                &identity
            )
            .await,
            Err("remote_target_changed")
        ));
    }

    #[tokio::test]
    async fn trashed_source_can_be_restored_on_new_pc_from_fake_backup() {
        use std::{
            collections::BTreeMap,
            io::{Read, Write},
            net::TcpListener,
        };
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        for name in ["objects", ".git"] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        let database = temp.path().join("state.db");
        crate::storage::initialize(&database, &temp.path().join("backups"), "test").unwrap();
        let content = b"fictional source";
        let hash = crate::object_store::digest(content);
        let conn = crate::storage::open_writable_connection(&database).unwrap();
        conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at,integrity_state,integrity_issue,quarantined_at,quarantined_by) VALUES('asset-1','other',?1,?2,?3,'photo','test','now','quarantined','user_deleted','now','user')", rusqlite::params![hash,content.len() as i64, format!("sha256/{}/{hash}",&hash[..2])]).unwrap();
        conn.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until) VALUES('asset-1',?1,'trashed','now','later')", [uuid::Uuid::new_v4().to_string()]).unwrap();
        drop(conn);
        assert!(matches!(restore_trash(&root,&database,"asset-1"), Err(e) if e == "local_missing"));
        let identity = age::x25519::Identity::generate();
        let encrypted = crate::object_store_transfer::encrypt_bytes(content, &identity).unwrap();
        let manifest = crate::object_store_transfer::Manifest {
            version: 1,
            kind: "object".into(),
            relative_path: format!("sha256/{}/{hash}", &hash[..2]),
            sha256: hash.clone(),
            size: content.len() as u64,
            upload_id: uuid::Uuid::new_v4().to_string(),
            chunks: vec![crate::object_store::digest(&encrypted)],
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let manifest_key = format!("{}/manifests/test.age", crate::object_store::PREFIX);
        let manifest_bytes = crate::object_store_transfer::encrypt_bytes(
            &serde_json::to_vec(&manifest).unwrap(),
            &identity,
        )
        .unwrap();
        let objects = BTreeMap::from([
            (manifest_key.clone(), manifest_bytes),
            (
                crate::object_store_transfer::chunk_key(&manifest, 0),
                encrypted,
            ),
        ]);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                    .unwrap();
                let mut received = Vec::new();
                let mut buffer = [0u8; 2048];
                while !received.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buffer).unwrap();
                    assert_ne!(n, 0);
                    received.extend_from_slice(&buffer[..n]);
                }
                let head = String::from_utf8(received).unwrap();
                let path = head
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                let body = if path.starts_with("/bucket?") {
                    format!("<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>{manifest_key}</Key></Contents></ListBucketResult>").into_bytes()
                } else {
                    objects
                        .get(path.strip_prefix("/bucket/").unwrap())
                        .unwrap()
                        .clone()
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let config = crate::object_store::Config {
            provider: "s3_compatible".into(),
            endpoint: format!("http://{address}"),
            region: "auto".into(),
            bucket: "bucket".into(),
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(crate::object_store::WriteSafety::Conditional),
        };
        let keys = crate::object_store::Keys {
            access_key_id: "TEST".into(),
            secret_access_key: "test".into(),
        };
        let archive =
            crate::archive_location::ArchiveLocation::load(database.clone(), temp.path(), vec![])
                .unwrap();
        let paths = crate::runtime::RuntimePaths {
            data_root: temp.path().to_path_buf(),
            database_path: database.clone(),
            objects_path: root.join("objects"),
            exports_path: temp.path().join("exports"),
            backups_path: temp.path().join("backups"),
            logs_path: temp.path().join("logs"),
            archive,
            archive_lease: None,
        };
        assert!(matches!(
            crate::object_store_restore::restore_selected(
                &config,
                &keys,
                &identity,
                &paths,
                Some(std::collections::HashSet::from([hash.clone()]))
            )
            .await,
            Err("source_deleted")
        ));
        let external = temp.path().join("upload-source");
        fs::write(&external, content).unwrap();
        let proposal: crate::source_assets::SourceAssetImportProposal = serde_json::from_value(serde_json::json!({"sourcePath":external.to_str().unwrap(),"assetKind":"other","mediaType":null,"recordLink":null,"actor":"test"})).unwrap();
        assert!(
            crate::source_assets::plan_import(&database, &root.join("objects"), proposal).is_err()
        );
        assert_eq!(
            restore_trashed_from_remote(
                &root,
                &database,
                &format!("sha256:{hash}"),
                &config,
                &keys,
                &identity
            )
            .await
            .unwrap()
            .state,
            "restored"
        );
        server.join().unwrap();
        let source = root.join("objects/sha256").join(&hash[..2]).join(hash);
        assert_eq!(fs::read(&source).unwrap(), content);
        assert!(fs::metadata(&source).unwrap().permissions().readonly());
        #[cfg(windows)]
        {
            crate::source_assets::clear_readonly_attribute(&source).unwrap();
        }
    }
}
