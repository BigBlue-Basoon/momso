use crate::{object_store, object_store_transfer as transfer, storage, AppState};
use age::x25519;
use rusqlite::OptionalExtension;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path},
};
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestoreResult {
    restored_count: u64,
    skipped_count: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecoveredOriginal {
    path: String,
    relative_path: String,
    sha256: String,
    size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    id: String,
    relative_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VerifyResult {
    missing: Vec<Item>,
    extra: Vec<Item>,
    missing_chunks: Vec<String>,
    unreferenced_objects: Vec<String>,
}

fn safe_parent(root: &Path, relative: &Path) -> Result<std::path::PathBuf, &'static str> {
    if !transfer::safe_relative(relative) {
        return Err("unsafe_path");
    }
    let metadata = fs::symlink_metadata(root).map_err(|_| "local_read_failed")?;
    if !metadata.is_dir() || crate::archive_location::is_link_or_reparse(&metadata) {
        return Err("unsafe_path");
    }
    let parent = relative.parent().ok_or("unsafe_path")?;
    let mut current = root.to_path_buf();
    for component in parent.components() {
        let Component::Normal(part) = component else {
            return Err("unsafe_path");
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata)
                if metadata.is_dir() && !crate::archive_location::is_link_or_reparse(&metadata) => {
            }
            Ok(_) => return Err("unsafe_path"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::create_dir(&current) {
                    Ok(()) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
                    Err(_) => return Err("local_write_failed"),
                }
                let metadata = fs::symlink_metadata(&current).map_err(|_| "local_read_failed")?;
                if !metadata.is_dir() || crate::archive_location::is_link_or_reparse(&metadata) {
                    return Err("unsafe_path");
                }
            }
            Err(_) => return Err("local_read_failed"),
        }
    }
    Ok(root.join(relative))
}

fn existing_matches(path: &Path, manifest: &transfer::Manifest) -> Result<bool, &'static str> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.is_file() && !crate::archive_location::is_link_or_reparse(&metadata) =>
        {
            let (hash, size) = transfer::hash_file(path)?;
            if hash == manifest.sha256 && size == manifest.size {
                Ok(true)
            } else {
                Err("local_conflict")
            }
        }
        Ok(_) => Err("unsafe_path"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("local_read_failed"),
    }
}

fn make_readonly(path: &Path) -> Result<(), &'static str> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "local_read_failed")?;
    if !metadata.is_file() || crate::archive_location::is_link_or_reparse(&metadata) {
        return Err("unsafe_path");
    }
    let mut permissions = metadata.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).map_err(|_| "local_write_failed")
}

pub(crate) async fn download_verified(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    manifest: &transfer::Manifest,
    staging: &Path,
) -> Result<tempfile::NamedTempFile, &'static str> {
    fs::create_dir_all(staging).map_err(|_| "local_write_failed")?;
    let mut encrypted =
        tempfile::NamedTempFile::new_in(staging).map_err(|_| "local_write_failed")?;
    if manifest.chunks.is_empty() {
        return Err("invalid_manifest");
    }
    for (index, hash) in manifest.chunks.iter().enumerate() {
        let key = transfer::chunk_key(manifest, index);
        let bytes = transfer::get_bytes(config, keys, &key, transfer::CHUNK_SIZE)
            .await?
            .ok_or("remote_missing")?;
        if object_store::digest(&bytes) != *hash {
            return Err("hash_mismatch");
        }
        encrypted
            .write_all(&bytes)
            .map_err(|_| "local_write_failed")?;
    }
    encrypted
        .as_file()
        .sync_all()
        .map_err(|_| "local_write_failed")?;
    let input = File::open(encrypted.path()).map_err(|_| "local_read_failed")?;
    let decryptor = age::Decryptor::new(input).map_err(|_| "decryption_failed")?;
    let mut reader = decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|_| "decryption_failed")?;
    let mut result = tempfile::NamedTempFile::new_in(staging).map_err(|_| "local_write_failed")?;
    let mut sha = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let n = reader.read(&mut buffer).map_err(|_| "decryption_failed")?;
        if n == 0 {
            break;
        }
        size = size.checked_add(n as u64).ok_or("invalid_remote_object")?;
        if size > manifest.size {
            return Err("hash_mismatch");
        }
        sha.update(&buffer[..n]);
        result
            .write_all(&buffer[..n])
            .map_err(|_| "local_write_failed")?;
    }
    if size != manifest.size || format!("{:x}", sha.finalize()) != manifest.sha256 {
        return Err("hash_mismatch");
    }
    result
        .as_file()
        .sync_all()
        .map_err(|_| "local_write_failed")?;
    Ok(result)
}

// Search only recovery-named directories and never change an existing file.
// A matching name alone is not evidence: check its permissions, length and hash.
fn verified_recovery_copy(
    backups: &Path,
    manifest: &transfer::Manifest,
) -> Result<Option<RecoveredOriginal>, &'static str> {
    let filename = Path::new(&manifest.relative_path)
        .file_name()
        .ok_or("unsafe_path")?;
    for entry in fs::read_dir(backups).map_err(|_| "local_read_failed")? {
        let entry = entry.map_err(|_| "local_read_failed")?;
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("momso-source-recovery-")
        {
            continue;
        }
        let meta = fs::symlink_metadata(entry.path()).map_err(|_| "local_read_failed")?;
        if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
            continue;
        }
        let candidate = entry.path().join(filename);
        let meta = match fs::symlink_metadata(&candidate) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("local_read_failed"),
        };
        if !meta.is_file()
            || crate::archive_location::is_link_or_reparse(&meta)
            || !meta.permissions().readonly()
            || meta.len() != manifest.size
        {
            continue;
        }
        let (hash, size) = transfer::hash_file(&candidate)?;
        if hash == manifest.sha256 && size == manifest.size {
            let path = candidate.to_str().ok_or("unsafe_path")?.to_owned();
            return Ok(Some(RecoveredOriginal {
                path,
                relative_path: manifest.relative_path.clone(),
                sha256: hash,
                size,
            }));
        }
    }
    Ok(None)
}

// This command writes a separate recovery copy; it never replaces the local
// conflicting file. The caller must supply the pre-change hash explicitly.
async fn recover_original_copy(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    manifest: &transfer::Manifest,
    backups: &Path,
) -> Result<RecoveredOriginal, &'static str> {
    let relative = Path::new(&manifest.relative_path);
    if manifest.kind != "object" || !transfer::safe_relative(relative) {
        return Err("unsafe_path");
    }
    let metadata = fs::symlink_metadata(backups).map_err(|_| "local_read_failed")?;
    if !metadata.is_dir() || crate::archive_location::is_link_or_reparse(&metadata) {
        return Err("unsafe_path");
    }
    if let Some(existing) = verified_recovery_copy(backups, manifest)? {
        return Ok(existing);
    }
    let destination_dir = tempfile::Builder::new()
        .prefix("momso-source-recovery-")
        .tempdir_in(backups)
        .map_err(|_| "local_write_failed")?;
    let downloaded =
        download_verified(config, keys, identity, manifest, destination_dir.path()).await?;
    let filename = relative.file_name().ok_or("unsafe_path")?;
    let destination = destination_dir.path().join(filename);
    let path = destination.to_str().ok_or("unsafe_path")?.to_owned();
    downloaded
        .persist_noclobber(&destination)
        .map_err(|_| "local_write_failed")?;
    make_readonly(&destination)?;
    let _directory = destination_dir.keep();
    Ok(RecoveredOriginal {
        path,
        relative_path: manifest.relative_path.clone(),
        sha256: manifest.sha256.clone(),
        size: manifest.size,
    })
}

fn validate_recovery_request(
    relative_path: &str,
    expected_sha256: &str,
) -> Result<(), &'static str> {
    if !transfer::safe_relative(Path::new(&relative_path))
        || expected_sha256.len() != 64
        || !expected_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid_id");
    }
    Ok(())
}

fn select_recovery_manifest(
    catalog: Vec<transfer::Manifest>,
    relative_path: &str,
    expected_sha256: &str,
) -> Result<transfer::Manifest, &'static str> {
    validate_recovery_request(relative_path, expected_sha256)?;
    catalog
        .into_iter()
        .filter(|item| {
            item.kind == "object"
                && item.relative_path == relative_path
                && item.sha256 == expected_sha256
        })
        .max_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then(a.upload_id.cmp(&b.upload_id))
        })
        .ok_or("remote_missing")
}

#[tauri::command]
pub(crate) async fn object_store_recover_original_copy(
    state: State<'_, AppState>,
    relative_path: String,
    expected_sha256: String,
) -> Result<RecoveredOriginal, &'static str> {
    validate_recovery_request(&relative_path, &expected_sha256)?;
    let paths = state.paths.scoped().await;
    let _guard = object_store::CONNECTION_LOCK.lock().await;
    {
        let conn =
            storage::open_writable_connection(&paths.database_path).map_err(|_| "unavailable")?;
        let purging: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM source_asset_deletions d JOIN momso_source_assets a ON a.id=d.asset_id WHERE a.content_sha256=?1 AND d.state IN ('purge_pending','purged'))", [&expected_sha256], |r| r.get(0)).map_err(|_| "unavailable")?;
        if purging {
            return Err("source_deleted");
        }
    }
    let config = object_store::load_config(&paths.data_root)?.ok_or("not_connected")?;
    let keys = object_store::load_keys(&config)?;
    let identity = transfer::identity(&paths.database_path)?;
    let manifest = select_recovery_manifest(
        transfer::catalog(&config, &keys, &identity).await?,
        &relative_path,
        &expected_sha256,
    )?;
    recover_original_copy(&config, &keys, &identity, &manifest, &paths.backups_path).await
}

pub(crate) async fn restore_one(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    manifest: &transfer::Manifest,
    objects: &Path,
) -> Result<bool, &'static str> {
    let relative = Path::new(&manifest.relative_path);
    if !transfer::safe_relative(relative) {
        return Err("unsafe_path");
    }
    transfer::validate_content_address(&manifest.relative_path, &manifest.sha256)?;
    let destination = safe_parent(objects, relative)?;
    if existing_matches(&destination, manifest)? {
        make_readonly(&destination)?;
        return Ok(false);
    }
    // Keep the verified temporary file on the archive's volume so that the
    // no-overwrite hard link also works after the archive moves to another disk.
    let parent = objects.parent().ok_or("unsafe_path")?;
    let staging = tempfile::Builder::new()
        .prefix(".momso-restore-")
        .tempdir_in(parent)
        .map_err(|_| "local_write_failed")?;
    let temp = download_verified(config, keys, identity, manifest, staging.path()).await?;
    let _guard = crate::chat_file_changes::lock_app_write();
    let destination = safe_parent(objects, relative)?;
    match fs::hard_link(temp.path(), &destination) {
        Ok(()) => {
            // Remove the temporary link before setting the shared file attribute;
            // Windows cannot always remove a read-only temporary file.
            temp.close().map_err(|_| "local_write_failed")?;
            make_readonly(&destination)?;
            if let Some(root) = objects.parent() {
                crate::chat_file_changes::app_write(
                    root,
                    Some(&format!("sha256:{}", manifest.sha256)),
                );
            }
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if existing_matches(&destination, manifest)? {
                make_readonly(&destination)?;
                Ok(false)
            } else {
                Err("local_write_failed")
            }
        }
        Err(_) => Err("local_write_failed"),
    }
}

fn selected_hashes(database: &Path, ids: Vec<String>) -> Result<HashSet<String>, &'static str> {
    let connection = storage::open_writable_connection(database).map_err(|_| "unavailable")?;
    let mut hashes = HashSet::new();
    for id in ids {
        let lookup = crate::source_assets::reference_lookup(&id).ok_or("unknown_id")?;
        let hash = connection.query_row("SELECT content_sha256 FROM momso_source_assets WHERE id = ?1 OR content_sha256 = ?2", rusqlite::params![id, lookup], |row| row.get::<_, String>(0)).optional().map_err(|_| "unavailable")?.ok_or("unknown_id")?;
        hashes.insert(hash);
    }
    Ok(hashes)
}

#[tauri::command]
pub(crate) async fn object_store_restore(
    state: State<'_, AppState>,
    ids: Option<Vec<String>>,
) -> Result<RestoreResult, &'static str> {
    let paths = state.paths.scoped().await;
    let _guard = object_store::CONNECTION_LOCK.lock().await;
    let config = object_store::load_config(&paths.data_root)?.ok_or("not_connected")?;
    let keys = object_store::load_keys(&config)?;
    let identity = transfer::identity(&paths.database_path)?;
    let selected = ids
        .map(|ids| selected_hashes(&paths.database_path, ids))
        .transpose()?;
    if selected.is_none() {
        crate::class_student_backup::restore_if_missing(&config, &keys, &identity, &paths).await?;
    }
    restore_selected(&config, &keys, &identity, &paths, selected).await
}

pub(crate) async fn restore_selected(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    paths: &crate::runtime::RuntimePaths,
    selected: Option<HashSet<String>>,
) -> Result<RestoreResult, &'static str> {
    let deleted: HashSet<String> = {
        let database =
            storage::open_writable_connection(&paths.database_path).map_err(|_| "unavailable")?;
        let mut statement = database.prepare("SELECT a.content_sha256 FROM source_asset_deletions d JOIN momso_source_assets a ON a.id=d.asset_id").map_err(|_| "unavailable")?;
        let rows = statement
            .query_map([], |row| row.get(0))
            .map_err(|_| "unavailable")?
            .collect::<Result<_, _>>()
            .map_err(|_| "unavailable")?;
        rows
    };
    if selected
        .as_ref()
        .is_some_and(|hashes| hashes.iter().any(|hash| deleted.contains(hash)))
    {
        return Err("source_deleted");
    }
    let mut latest: BTreeMap<String, transfer::Manifest> = BTreeMap::new();
    for manifest in transfer::catalog(config, keys, identity).await? {
        if manifest.kind != "object"
            || deleted.contains(&manifest.sha256)
            || selected
                .as_ref()
                .is_some_and(|v| !v.contains(&manifest.sha256))
        {
            continue;
        }
        transfer::validate_content_address(&manifest.relative_path, &manifest.sha256)?;
        let replace = latest
            .get(&manifest.relative_path)
            .is_none_or(|existing| existing.created_at < manifest.created_at);
        if replace {
            latest.insert(manifest.relative_path.clone(), manifest);
        }
    }
    if selected.as_ref().is_some_and(|requested| {
        requested
            .iter()
            .any(|hash| !latest.values().any(|m| &m.sha256 == hash))
    }) {
        return Err("remote_missing");
    }
    let mut result = RestoreResult {
        restored_count: 0,
        skipped_count: 0,
    };
    for manifest in latest.values() {
        if restore_one(config, keys, identity, manifest, &paths.objects_path).await? {
            result.restored_count += 1;
        } else {
            result.skipped_count += 1;
        }
    }
    Ok(result)
}

#[tauri::command]
pub(crate) async fn object_store_verify(
    state: State<'_, AppState>,
) -> Result<VerifyResult, &'static str> {
    let paths = state.paths.scoped().await;
    let _guard = object_store::CONNECTION_LOCK.lock().await;
    let config = object_store::load_config(&paths.data_root)?.ok_or("not_connected")?;
    let keys = object_store::load_keys(&config)?;
    let identity = transfer::identity(&paths.database_path)?;
    let manifests = transfer::catalog(&config, &keys, &identity).await?;
    let available: HashSet<_> =
        transfer::list_keys(&config, &keys, &format!("{}/data/", object_store::PREFIX))
            .await?
            .into_iter()
            .collect();
    let mut local = HashSet::new();
    for (relative, path, _) in transfer::object_files(&paths.objects_path)? {
        let (hash, _) = transfer::hash_file(&path)?;
        local.insert((relative, hash));
    }
    compare_lists(&manifests, &available, &local)
}

pub(crate) fn compare_lists(
    manifests: &[transfer::Manifest],
    available: &HashSet<String>,
    local: &HashSet<(String, String)>,
) -> Result<VerifyResult, &'static str> {
    let mut remote = HashSet::new();
    let mut referenced = HashSet::new();
    let mut missing_chunks = Vec::new();
    for manifest in manifests.iter().filter(|m| m.kind == "object") {
        if manifest.chunks.is_empty() {
            return Err("invalid_manifest");
        }
        let mut complete = true;
        for (i, _) in manifest.chunks.iter().enumerate() {
            let key = transfer::chunk_key(manifest, i);
            referenced.insert(key.clone());
            if !available.contains(&key) {
                missing_chunks.push(key);
                complete = false;
            }
        }
        if complete {
            remote.insert((manifest.relative_path.clone(), manifest.sha256.clone()));
        }
    }
    // State snapshots use the same data prefix and must not be marked as orphans.
    for manifest in manifests.iter().filter(|m| m.kind == "app_state") {
        for (i, _) in manifest.chunks.iter().enumerate() {
            referenced.insert(transfer::chunk_key(manifest, i));
        }
    }
    let mut missing: Vec<_> = local
        .difference(&remote)
        .map(|(path, hash)| Item {
            id: hash.clone(),
            relative_path: path.clone(),
        })
        .collect();
    let mut extra: Vec<_> = remote
        .difference(local)
        .map(|(path, hash)| Item {
            id: hash.clone(),
            relative_path: path.clone(),
        })
        .collect();
    missing.sort_by(|a, b| a.relative_path.cmp(&b.relative_path).then(a.id.cmp(&b.id)));
    extra.sort_by(|a, b| a.relative_path.cmp(&b.relative_path).then(a.id.cmp(&b.id)));
    missing_chunks.sort();
    missing_chunks.dedup();
    let mut unreferenced_objects: Vec<_> = available.difference(&referenced).cloned().collect();
    unreferenced_objects.sort();
    Ok(VerifyResult {
        missing,
        extra,
        missing_chunks,
        unreferenced_objects,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    use uuid::Uuid;

    fn make_test_file_writable(path: &Path) {
        #[cfg(windows)]
        crate::source_assets::clear_readonly_attribute(path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(permissions.mode() | 0o200);
            fs::set_permissions(path, permissions).unwrap();
        }
    }

    #[test]
    fn selected_restore_accepts_all_asset_reference_forms() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("state.db");
        storage::initialize(&database, &temp.path().join("backups"), "test").unwrap();
        let hash = "a".repeat(64);
        let connection = storage::open_writable_connection(&database).unwrap();
        connection.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('asset-1','other',?1,1,'sha256/aa/file','note.txt','test','now')", [&hash]).unwrap();
        drop(connection);
        for id in ["asset-1".to_owned(), hash.clone(), format!("sha256:{hash}")] {
            assert_eq!(
                selected_hashes(&database, vec![id]).unwrap(),
                HashSet::from([hash.clone()])
            );
        }
        assert!(matches!(
            selected_hashes(&database, vec!["sha256:invalid".into()]),
            Err("unknown_id")
        ));
    }

    #[test]
    fn existing_recovery_copy_requires_matching_content_and_recovery_directory() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = transfer::Manifest {
            version: 1,
            kind: "object".into(),
            relative_path: "nested/original.txt".into(),
            sha256: object_store::digest(b"good"),
            size: 4,
            upload_id: Uuid::new_v4().to_string(),
            chunks: vec!["a".repeat(64)],
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let unowned = temp.path().join("other");
        fs::create_dir(&unowned).unwrap();
        fs::write(unowned.join("original.txt"), b"good").unwrap();
        let owned = temp.path().join("momso-source-recovery-fixture");
        fs::create_dir(&owned).unwrap();
        let path = owned.join("original.txt");
        fs::write(&path, b"evil").unwrap();
        assert!(verified_recovery_copy(temp.path(), &manifest)
            .unwrap()
            .is_none());
        fs::write(&path, b"good").unwrap();
        assert!(verified_recovery_copy(temp.path(), &manifest)
            .unwrap()
            .is_none());
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();
        let recovered = verified_recovery_copy(temp.path(), &manifest)
            .unwrap()
            .unwrap();
        assert_eq!(Path::new(&recovered.path), path);
        assert!(fs::metadata(&path).unwrap().permissions().readonly());
        make_test_file_writable(&path);
        assert_eq!(fs::read(unowned.join("original.txt")).unwrap(), b"good");
    }

    #[test]
    fn recovery_selects_only_matching_original_path_and_hash() {
        let manifest = transfer::Manifest {
            version: 1,
            kind: "object".into(),
            relative_path: "folder/original".into(),
            sha256: "a".repeat(64),
            size: 1,
            upload_id: Uuid::new_v4().to_string(),
            chunks: vec!["b".repeat(64)],
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        assert!(select_recovery_manifest(
            vec![manifest.clone()],
            "folder/original",
            &"a".repeat(64)
        )
        .is_ok());
        assert!(matches!(
            select_recovery_manifest(vec![manifest.clone()], "folder/other", &"a".repeat(64)),
            Err("remote_missing")
        ));
        assert!(matches!(
            select_recovery_manifest(vec![manifest.clone()], "folder/original", &"c".repeat(64)),
            Err("remote_missing")
        ));
        assert!(matches!(
            select_recovery_manifest(vec![manifest], "../original", &"a".repeat(64)),
            Err("invalid_id")
        ));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn shell_bypass_is_detected_and_verified_originals_are_recovered_safely() {
        let temp = tempfile::tempdir().unwrap();
        let objects = temp.path().join("objects");
        fs::create_dir(&objects).unwrap();
        let original = b"fictional original source";
        for name in ["overwritten.txt", "deleted.txt"] {
            let path = objects.join(name);
            fs::write(&path, original).unwrap();
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_readonly(true);
            fs::set_permissions(&path, permissions).unwrap();
        }
        let identity = x25519::Identity::generate();
        let encrypted = transfer::encrypt_bytes(original, &identity).unwrap();
        let manifest = transfer::Manifest {
            version: 1,
            kind: "object".into(),
            relative_path: "deleted.txt".into(),
            sha256: object_store::digest(original),
            size: original.len() as u64,
            upload_id: Uuid::new_v4().to_string(),
            chunks: vec![object_store::digest(&encrypted)],
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buffer).unwrap();
                    assert_ne!(n, 0);
                    request.extend_from_slice(&buffer[..n]);
                }
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    encrypted.len()
                );
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(&encrypted).unwrap();
            }
        });
        let config = object_store::Config {
            provider: "s3_compatible".into(),
            endpoint: format!("http://{address}"),
            region: "auto".into(),
            bucket: "bucket".into(),
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(object_store::WriteSafety::Conditional),
        };
        let keys = object_store::Keys {
            access_key_id: "TEST".into(),
            secret_access_key: "test".into(),
        };
        let before = crate::chat_file_changes::capture(temp.path()).unwrap();
        let status = std::process::Command::new("cmd.exe").current_dir(&objects)
            .args(["/d", "/c", "attrib -r overwritten.txt && attrib -r deleted.txt && echo altered>overwritten.txt && del /f deleted.txt"])
            .status().unwrap();
        assert!(status.success());
        let after = crate::chat_file_changes::capture(temp.path()).unwrap();
        let changes = crate::chat_file_changes::source_changes(&before, &after);
        assert_eq!(
            changes
                .get("object-path:overwritten.txt")
                .map(String::as_str),
            Some("edited")
        );
        assert_eq!(
            changes.get("object-path:deleted.txt").map(String::as_str),
            Some("deleted")
        );
        let details = crate::chat_file_changes::source_change_details(&before, &after);
        let overwritten_details = details
            .iter()
            .find(|item| item["id"] == "object-path:overwritten.txt")
            .unwrap();
        assert_eq!(overwritten_details["relativePath"], "overwritten.txt");
        assert_eq!(
            overwritten_details["beforeSha256"],
            object_store::digest(original)
        );
        let database = temp.path().join("momso.db");
        crate::storage::initialize(&database, &temp.path().join("backups"), "test").unwrap();
        crate::source_integrity::record(&database, "chat-test", "codex", "changed", &details)
            .unwrap();
        let saved = serde_json::to_value(
            crate::source_integrity::list(&database, Some("chat-test"), None).unwrap(),
        )
        .unwrap();
        let saved_change = saved[0]["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == "object-path:overwritten.txt")
            .unwrap();
        let mut overwritten = manifest.clone();
        overwritten.relative_path = "overwritten.txt".into();
        assert!(matches!(
            restore_one(&config, &keys, &identity, &overwritten, &objects).await,
            Err("local_conflict")
        ));
        let changed = fs::read(objects.join("overwritten.txt")).unwrap();
        assert_ne!(changed, original);
        let selected = select_recovery_manifest(
            vec![overwritten],
            saved_change["relativePath"].as_str().unwrap(),
            saved_change["beforeSha256"].as_str().unwrap(),
        )
        .unwrap();
        let recovered = recover_original_copy(&config, &keys, &identity, &selected, temp.path())
            .await
            .unwrap();
        let recovered_again =
            recover_original_copy(&config, &keys, &identity, &selected, temp.path())
                .await
                .unwrap();
        assert_eq!(recovered_again.path, recovered.path);
        let recovered_path = Path::new(&recovered.path);
        assert_eq!(fs::read(recovered_path).unwrap(), original);
        assert!(fs::metadata(recovered_path)
            .unwrap()
            .permissions()
            .readonly());
        assert_eq!(fs::read(objects.join("overwritten.txt")).unwrap(), changed);
        assert!(restore_one(&config, &keys, &identity, &manifest, &objects)
            .await
            .unwrap());
        assert_eq!(fs::read(objects.join("deleted.txt")).unwrap(), original);
        assert!(fs::metadata(objects.join("deleted.txt"))
            .unwrap()
            .permissions()
            .readonly());
        make_test_file_writable(&objects.join("deleted.txt"));
        make_test_file_writable(recovered_path);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn restore_checks_hash_and_never_overwrites_local_file() {
        let temp = tempfile::tempdir().unwrap();
        let objects = temp.path().join("objects");
        fs::create_dir(&objects).unwrap();
        let identity = x25519::Identity::generate();
        let original = b"original private source bytes";
        let encrypted = transfer::encrypt_bytes(original, &identity).unwrap();
        let manifest = transfer::Manifest {
            version: 1,
            kind: "object".into(),
            relative_path: "sha256/aa/item".into(),
            sha256: object_store::digest(original),
            size: original.len() as u64,
            upload_id: Uuid::new_v4().to_string(),
            chunks: vec![object_store::digest(&encrypted)],
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for index in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buffer).unwrap();
                    assert_ne!(n, 0);
                    request.extend_from_slice(&buffer[..n]);
                }
                let mut bytes = encrypted.clone();
                if index > 0 {
                    bytes[0] ^= 1;
                }
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                );
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(&bytes).unwrap();
            }
        });
        let config = object_store::Config {
            provider: "s3_compatible".into(),
            endpoint: format!("http://{address}"),
            region: "auto".into(),
            bucket: "bucket".into(),
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(object_store::WriteSafety::Conditional),
        };
        let keys = object_store::Keys {
            access_key_id: "TEST".into(),
            secret_access_key: "test".into(),
        };
        let mut corrupted = manifest.clone();
        corrupted.relative_path = format!("sha256/aa/{}", "a".repeat(64));
        assert!(matches!(
            restore_one(&config, &keys, &identity, &corrupted, &objects).await,
            Err("source_integrity_mismatch")
        ));
        assert!(!objects.join(&corrupted.relative_path).exists());
        assert!(restore_one(&config, &keys, &identity, &manifest, &objects)
            .await
            .unwrap());
        let destination = objects.join(&manifest.relative_path);
        assert_eq!(fs::read(&destination).unwrap(), original);
        assert!(fs::metadata(&destination).unwrap().permissions().readonly());
        assert!(!restore_one(&config, &keys, &identity, &manifest, &objects)
            .await
            .unwrap());
        make_test_file_writable(&destination);
        fs::write(&destination, b"existing local data").unwrap();
        assert!(matches!(
            restore_one(&config, &keys, &identity, &manifest, &objects).await,
            Err("local_conflict")
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"existing local data");
        let other = temp.path().join("other");
        fs::create_dir(&other).unwrap();
        assert!(matches!(
            restore_one(&config, &keys, &identity, &manifest, &other).await,
            Err("hash_mismatch")
        ));
        assert!(!other.join(&manifest.relative_path).exists());
        assert!(matches!(
            recover_original_copy(&config, &keys, &identity, &manifest, temp.path()).await,
            Err("hash_mismatch")
        ));
        assert!(fs::read_dir(temp.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("momso-source-recovery-")));
        assert!(fs::read_dir(temp.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".momso-restore-")));
        server.join().unwrap();
    }

    #[test]
    fn verification_reports_missing_extra_and_orphaned_chunks() {
        let id = Uuid::new_v4().to_string();
        let one = transfer::Manifest {
            version: 1,
            kind: "object".into(),
            relative_path: "a".into(),
            sha256: "a".repeat(64),
            size: 3,
            upload_id: id.clone(),
            chunks: vec!["b".repeat(64)],
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let local = HashSet::from([("b".into(), "c".repeat(64))]);
        let key = format!("{}/data/{id}/00000000", object_store::PREFIX);
        let available = HashSet::from([key.clone(), "orphan".into()]);
        let result = compare_lists(std::slice::from_ref(&one), &available, &local).unwrap();
        assert_eq!(result.missing.len(), 1);
        assert_eq!(result.extra.len(), 1);
        assert_eq!(result.unreferenced_objects, vec!["orphan"]);
        let result = compare_lists(&[one], &HashSet::new(), &local).unwrap();
        assert_eq!(result.missing_chunks, vec![key]);
        assert!(result.extra.is_empty());
        assert!(!transfer::safe_relative(Path::new("../outside")));
    }
}
