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
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
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
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => (),
            Ok(_) => return Err("unsafe_path"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::create_dir(&current) {
                    Ok(()) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
                    Err(_) => return Err("local_write_failed"),
                }
                let metadata = fs::symlink_metadata(&current).map_err(|_| "local_read_failed")?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err("unsafe_path");
                }
            }
            Err(_) => return Err("local_read_failed"),
        }
    }
    Ok(root.join(relative))
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

async fn restore_one(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    manifest: &transfer::Manifest,
    objects: &Path,
    staging: &Path,
) -> Result<bool, &'static str> {
    let relative = Path::new(&manifest.relative_path);
    if !transfer::safe_relative(relative) {
        return Err("unsafe_path");
    }
    let destination = objects.join(relative);
    match fs::symlink_metadata(&destination) {
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err("local_read_failed"),
    }
    let temp = download_verified(config, keys, identity, manifest, staging).await?;
    let _guard = crate::chat_file_changes::lock_app_write();
    let destination = safe_parent(objects, relative)?;
    match fs::hard_link(temp.path(), &destination) {
        Ok(()) => {
            if let Some(root) = objects.parent() {
                crate::chat_file_changes::app_write(
                    root,
                    Some(&format!("sha256:{}", manifest.sha256)),
                );
            }
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(_) => Err("local_write_failed"),
    }
}

fn selected_hashes(database: &Path, ids: Vec<String>) -> Result<HashSet<String>, &'static str> {
    let connection = storage::open_writable_connection(database).map_err(|_| "unavailable")?;
    let mut hashes = HashSet::new();
    for id in ids {
        let hash = connection.query_row("SELECT content_sha256 FROM momso_source_assets WHERE id = ?1 OR content_sha256 = ?1", [&id], |row| row.get::<_, String>(0)).optional().map_err(|_| "unavailable")?.ok_or("unknown_id")?;
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
    let mut latest: BTreeMap<String, transfer::Manifest> = BTreeMap::new();
    for manifest in transfer::catalog(&config, &keys, &identity).await? {
        if manifest.kind != "object"
            || selected
                .as_ref()
                .is_some_and(|v| !v.contains(&manifest.sha256))
        {
            continue;
        }
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
        if restore_one(
            &config,
            &keys,
            &identity,
            manifest,
            &paths.objects_path,
            &paths.backups_path,
        )
        .await?
        {
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

    #[tokio::test]
    async fn restore_checks_hash_and_never_overwrites_local_file() {
        let temp = tempfile::tempdir().unwrap();
        let objects = temp.path().join("objects");
        let staging = temp.path().join("staging");
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
            for index in 0..2 {
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
                if index == 1 {
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
        assert!(
            restore_one(&config, &keys, &identity, &manifest, &objects, &staging)
                .await
                .unwrap()
        );
        let destination = objects.join(&manifest.relative_path);
        assert_eq!(fs::read(&destination).unwrap(), original);
        fs::write(&destination, b"existing local data").unwrap();
        assert!(
            !restore_one(&config, &keys, &identity, &manifest, &objects, &staging)
                .await
                .unwrap()
        );
        assert_eq!(fs::read(&destination).unwrap(), b"existing local data");
        let other = temp.path().join("other");
        fs::create_dir(&other).unwrap();
        assert!(matches!(
            restore_one(&config, &keys, &identity, &manifest, &other, &staging).await,
            Err("hash_mismatch")
        ));
        assert!(!other.join(&manifest.relative_path).exists());
        assert_eq!(fs::read_dir(&staging).unwrap().count(), 0);
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
