use crate::{credentials, object_store, AppState};
use age::x25519;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use tauri::{Emitter, State};
use uuid::Uuid;
use walkdir::WalkDir;

pub(crate) const CHUNK_SIZE: usize = 5 * 1024 * 1024;
const MAX_MANIFEST: usize = 128 * 1024 * 1024;
const MAX_MANIFEST_PLAINTEXT: usize = MAX_MANIFEST - 64 * 1024;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Manifest {
    pub(crate) version: u32,
    pub(crate) kind: String,
    pub(crate) relative_path: String,
    pub(crate) sha256: String,
    pub(crate) size: u64,
    pub(crate) upload_id: String,
    pub(crate) chunks: Vec<String>,
    pub(crate) created_at: String,
}

pub(crate) fn chunk_key(manifest: &Manifest, index: usize) -> String {
    if manifest.version == 2 {
        format!(
            "{}/data/{}/{index:08}-{}",
            object_store::PREFIX,
            manifest.upload_id,
            manifest.chunks[index]
        )
    } else {
        format!(
            "{}/data/{}/{index:08}",
            object_store::PREFIX,
            manifest.upload_id
        )
    }
}

#[derive(Serialize, Deserialize)]
struct Journal {
    manifest: Manifest,
    encrypted_sha256: String,
}

#[derive(Deserialize)]
struct ListEntry {
    #[serde(rename = "Key")]
    key: String,
}
#[derive(Deserialize)]
struct ListResponse {
    #[serde(rename = "Contents", default)]
    contents: Vec<ListEntry>,
    #[serde(rename = "IsTruncated", default)]
    truncated: bool,
    #[serde(rename = "NextContinuationToken")]
    next: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupProgress {
    completed_count: u64,
    completed_bytes: u64,
    total_count: u64,
    total_bytes: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupFailure {
    file: Option<String>,
    state: &'static str,
}

pub(crate) fn identity(database: &Path) -> Result<x25519::Identity, &'static str> {
    let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "unavailable")?;
    let profile: Option<(String, Option<String>)> = connection.query_row("SELECT workspace_id, recovery_key_confirmed_at FROM momso_backup_profiles ORDER BY created_at ASC LIMIT 1", [], |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(|_| "unavailable")?;
    let (workspace_id, confirmed) = profile.ok_or("recovery_key_missing")?;
    if confirmed.is_none() {
        return Err("recovery_key_unconfirmed");
    }
    credentials::get_backup_identity(&workspace_id)
        .map_err(|_| "unavailable")?
        .ok_or("recovery_key_missing")
}

pub(crate) fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)))
}

pub(crate) fn hash_file(path: &Path) -> Result<(String, u64), &'static str> {
    let mut file = File::open(path).map_err(|_| "local_read_failed")?;
    let mut sha = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|_| "local_read_failed")?;
        if n == 0 {
            break;
        }
        sha.update(&buffer[..n]);
        total = total.checked_add(n as u64).ok_or("unavailable")?;
    }
    Ok((format!("{:x}", sha.finalize()), total))
}

pub(crate) fn encrypt_bytes(
    data: &[u8],
    identity: &x25519::Identity,
) -> Result<Vec<u8>, &'static str> {
    let recipient = identity.to_public();
    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
            .map_err(|_| "encryption_failed")?;
    let mut output = Vec::new();
    let mut writer = encryptor
        .wrap_output(&mut output)
        .map_err(|_| "encryption_failed")?;
    writer.write_all(data).map_err(|_| "encryption_failed")?;
    writer.finish().map_err(|_| "encryption_failed")?;
    Ok(output)
}

pub(crate) fn decrypt_bytes(
    data: &[u8],
    identity: &x25519::Identity,
    max: usize,
) -> Result<Vec<u8>, &'static str> {
    let decryptor = age::Decryptor::new(data).map_err(|_| "decryption_failed")?;
    let reader = decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|_| "decryption_failed")?;
    let mut output = Vec::new();
    reader
        .take(max as u64 + 1)
        .read_to_end(&mut output)
        .map_err(|_| "decryption_failed")?;
    if output.len() > max {
        return Err("invalid_manifest");
    }
    Ok(output)
}

pub(crate) async fn list_keys(
    config: &object_store::Config,
    keys: &object_store::Keys,
    prefix: &str,
) -> Result<Vec<String>, &'static str> {
    let mut result = Vec::new();
    let mut continuation: Option<String> = None;
    let mut seen_tokens = std::collections::HashSet::new();
    loop {
        let mut query = vec![("list-type", "2"), ("prefix", prefix)];
        if let Some(token) = &continuation {
            query.push(("continuation-token", token));
        }
        let response =
            object_store::request(config, keys, reqwest::Method::GET, "", &query, vec![]).await?;
        if !response.status().is_success() {
            return Err("remote_list_failed");
        }
        let body = read_bounded(response, 8 * 1024 * 1024)
            .await
            .map_err(|_| "remote_list_failed")?;
        let body = String::from_utf8(body).map_err(|_| "remote_list_failed")?;
        let page: ListResponse =
            quick_xml::de::from_str(&body).map_err(|_| "remote_list_failed")?;
        result.extend(page.contents.into_iter().map(|e| e.key));
        if !page.truncated {
            break;
        }
        let next = page
            .next
            .filter(|v| !v.is_empty())
            .ok_or("remote_list_failed")?;
        if !seen_tokens.insert(next.clone()) {
            return Err("remote_list_failed");
        }
        continuation = Some(next);
    }
    Ok(result)
}

pub(crate) async fn get_bytes(
    config: &object_store::Config,
    keys: &object_store::Keys,
    key: &str,
    max: usize,
) -> Result<Option<Vec<u8>>, &'static str> {
    let response =
        object_store::request(config, keys, reqwest::Method::GET, key, &[], vec![]).await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err("remote_read_failed");
    }
    if response.content_length().is_some_and(|n| n > max as u64) {
        return Err("invalid_remote_object");
    }
    Ok(Some(read_bounded(response, max).await?))
}

async fn read_bounded(
    mut response: reqwest::Response,
    max: usize,
) -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "remote_read_failed")? {
        if chunk.len() > max.saturating_sub(bytes.len()) {
            return Err("invalid_remote_object");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn put_once(
    config: &object_store::Config,
    keys: &object_store::Keys,
    key: &str,
    bytes: Vec<u8>,
) -> Result<(), &'static str> {
    if config.write_safety == Some(object_store::WriteSafety::CheckThenWrite) {
        let response =
            object_store::request(config, keys, reqwest::Method::HEAD, key, &[], vec![]).await?;
        if response.status().is_success() {
            if response
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|h| h.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .is_some_and(|n| n != bytes.len() as u64)
            {
                return Err("remote_conflict");
            }
            return match get_bytes(config, keys, key, bytes.len()).await? {
                Some(existing) if existing == bytes => Ok(()),
                _ => Err("remote_conflict"),
            };
        }
        if response.status() != reqwest::StatusCode::NOT_FOUND {
            return Err("remote_read_failed");
        }
        let response =
            object_store::request(config, keys, reqwest::Method::PUT, key, &[], bytes.clone())
                .await?;
        if !response.status().is_success() {
            return Err("remote_write_failed");
        }
        return match get_bytes(config, keys, key, bytes.len()).await? {
            Some(existing)
                if existing.len() == bytes.len()
                    && object_store::digest(&existing) == object_store::digest(&bytes) =>
            {
                Ok(())
            }
            _ => Err("remote_conflict"),
        };
    }
    if let Some(existing) = get_bytes(config, keys, key, bytes.len()).await? {
        return if existing == bytes {
            Ok(())
        } else {
            Err("remote_conflict")
        };
    }
    let response =
        object_store::request(config, keys, reqwest::Method::PUT, key, &[], bytes.clone()).await?;
    if response.status().is_success() {
        return Ok(());
    }
    if response.status() == reqwest::StatusCode::PRECONDITION_FAILED {
        return match get_bytes(config, keys, key, bytes.len()).await? {
            Some(existing) if existing == bytes => Ok(()),
            _ => Err("remote_conflict"),
        };
    }
    Err("remote_write_failed")
}

fn manifest_key(
    config: &object_store::Config,
    created_at: &str,
    unique: &str,
    encrypted: &[u8],
) -> String {
    if config.write_safety == Some(object_store::WriteSafety::CheckThenWrite) {
        let timestamp: String = created_at
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        format!(
            "{}/manifests/{}-{}-{}.age",
            object_store::PREFIX,
            timestamp,
            object_store::digest(encrypted),
            unique
        )
    } else {
        format!("{}/manifests/{}.age", object_store::PREFIX, unique)
    }
}

async fn publish_existing_content(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    existing: &Manifest,
    relative_path: &str,
) -> Result<Manifest, &'static str> {
    if !safe_relative(Path::new(relative_path)) {
        return Err("unsafe_path");
    }
    let mut manifest = existing.clone();
    manifest.relative_path = relative_path.to_owned();
    manifest.created_at = chrono::Utc::now().to_rfc3339();
    let plain = serde_json::to_vec(&manifest).map_err(|_| "invalid_manifest")?;
    if plain.len() > MAX_MANIFEST_PLAINTEXT {
        return Err("file_too_large");
    }
    let encrypted = encrypt_bytes(&plain, identity)?;
    if encrypted.len() > MAX_MANIFEST {
        return Err("file_too_large");
    }
    let key = manifest_key(
        config,
        &manifest.created_at,
        &Uuid::new_v4().to_string(),
        &encrypted,
    );
    put_once(config, keys, &key, encrypted).await?;
    Ok(manifest)
}

pub(crate) async fn catalog(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
) -> Result<Vec<Manifest>, &'static str> {
    let prefix = format!("{}/manifests/", object_store::PREFIX);
    let mut result = Vec::new();
    for key in list_keys(config, keys, &prefix).await? {
        if !key.starts_with(&prefix) || !key.ends_with(".age") {
            return Err("invalid_manifest");
        }
        let encrypted = get_bytes(config, keys, &key, MAX_MANIFEST)
            .await?
            .ok_or("remote_read_failed")?;
        let name = key.strip_prefix(&prefix).ok_or("invalid_manifest")?;
        if name.len() > 100 {
            let fields: Vec<_> = name.trim_end_matches(".age").split('-').collect();
            if fields.len() < 3 || fields[1] != object_store::digest(&encrypted) {
                return Err("hash_mismatch");
            }
        }
        let plain = decrypt_bytes(&encrypted, identity, MAX_MANIFEST)?;
        let manifest: Manifest = serde_json::from_slice(&plain).map_err(|_| "invalid_manifest")?;
        if name.len() > 100 {
            let timestamp: String = manifest
                .created_at
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            if !name.starts_with(&format!("{timestamp}-")) {
                return Err("invalid_manifest");
            }
        }
        if !matches!(manifest.version, 1 | 2)
            || !matches!(manifest.kind.as_str(), "object" | "app_state")
            || !safe_relative(Path::new(&manifest.relative_path))
            || Uuid::parse_str(&manifest.upload_id).is_err()
            || manifest.chunks.is_empty()
            || manifest.sha256.len() != 64
            || !manifest
                .sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || manifest.chunks.iter().any(|c| {
                c.len() != 64
                    || !c
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
        {
            return Err("invalid_manifest");
        }
        result.push(manifest);
    }
    Ok(result)
}

fn prepare_journal(
    source: &Path,
    relative: &str,
    kind: &str,
    stage: &Path,
    identity: &x25519::Identity,
    content_named: bool,
) -> Result<(Journal, PathBuf, PathBuf), &'static str> {
    if !safe_relative(Path::new(relative)) {
        return Err("unsafe_path");
    }
    let (sha256, size) = hash_file(source)?;
    let parent = stage.parent().ok_or("unsafe_path")?;
    for directory in [parent, stage] {
        match fs::symlink_metadata(directory) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => (),
            Ok(_) => return Err("unsafe_path"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(directory).map_err(|_| "local_write_failed")?;
                let metadata = fs::symlink_metadata(directory).map_err(|_| "local_read_failed")?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err("unsafe_path");
                }
            }
            Err(_) => return Err("local_read_failed"),
        }
    }
    let id = object_store::digest(format!("{kind}\0{relative}\0{sha256}").as_bytes());
    let journal_path = stage.join(format!("{id}.json"));
    let cipher_path = stage.join(format!("{id}.age"));
    if journal_path.exists() && cipher_path.exists() {
        let data = fs::read(&journal_path).map_err(|_| "local_read_failed")?;
        let journal: Journal = serde_json::from_slice(&data).map_err(|_| "local_state_invalid")?;
        if serde_json::to_vec(&journal.manifest)
            .map_err(|_| "local_state_invalid")?
            .len()
            > MAX_MANIFEST_PLAINTEXT
        {
            return Err("file_too_large");
        }
        if journal.manifest.sha256 != sha256
            || journal.manifest.size != size
            || journal.manifest.relative_path != relative
            || journal.manifest.kind != kind
            || Uuid::parse_str(&journal.manifest.upload_id).is_err()
            || journal.manifest.chunks.is_empty()
            || journal.manifest.chunks.iter().any(|hash| {
                hash.len() != 64
                    || !hash
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
            || hash_file(&cipher_path)?.0 != journal.encrypted_sha256
        {
            return Err("local_state_invalid");
        }
        let encrypted = File::open(&cipher_path).map_err(|_| "local_read_failed")?;
        let decryptor = age::Decryptor::new(encrypted).map_err(|_| "local_state_invalid")?;
        let mut reader = decryptor
            .decrypt(std::iter::once(identity as &dyn age::Identity))
            .map_err(|_| "recovery_key_mismatch")?;
        let mut hash = Sha256::new();
        let mut bytes = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            let n = reader
                .read(&mut buffer)
                .map_err(|_| "local_state_invalid")?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
            bytes += n as u64;
        }
        if format!("{:x}", hash.finalize()) != sha256 || bytes != size {
            return Err("local_state_invalid");
        }
        return Ok((journal, journal_path, cipher_path));
    }
    if !journal_path.exists() && cipher_path.exists() {
        // Uploads do not start until the journal exists. A crash between the two
        // durable renames can leave an encrypted staging file, but no remote chunks.
        let encrypted = File::open(&cipher_path).map_err(|_| "local_read_failed")?;
        let decryptor = age::Decryptor::new(encrypted).map_err(|_| "local_state_invalid")?;
        let mut reader = decryptor
            .decrypt(std::iter::once(identity as &dyn age::Identity))
            .map_err(|_| "recovery_key_mismatch")?;
        let mut hash = Sha256::new();
        let mut length = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            let n = reader
                .read(&mut buffer)
                .map_err(|_| "local_state_invalid")?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
            length += n as u64;
        }
        if format!("{:x}", hash.finalize()) != sha256 || length != size {
            return Err("local_state_invalid");
        }
        fs::remove_file(&cipher_path).map_err(|_| "local_write_failed")?;
    }
    if journal_path.exists() || cipher_path.exists() {
        return Err("local_state_invalid");
    }
    let mut temp = tempfile::NamedTempFile::new_in(stage).map_err(|_| "local_write_failed")?;
    let recipient = identity.to_public();
    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
            .map_err(|_| "encryption_failed")?;
    let mut writer = encryptor
        .wrap_output(temp.as_file_mut())
        .map_err(|_| "encryption_failed")?;
    let mut input = File::open(source).map_err(|_| "local_read_failed")?;
    let mut plaintext_hash = Sha256::new();
    let mut plaintext_size = 0u64;
    let mut plain_buffer = [0u8; 65536];
    loop {
        let count = input
            .read(&mut plain_buffer)
            .map_err(|_| "local_read_failed")?;
        if count == 0 {
            break;
        }
        plaintext_hash.update(&plain_buffer[..count]);
        plaintext_size += count as u64;
        writer
            .write_all(&plain_buffer[..count])
            .map_err(|_| "encryption_failed")?;
    }
    writer.finish().map_err(|_| "encryption_failed")?;
    temp.as_file()
        .sync_all()
        .map_err(|_| "local_write_failed")?;
    if format!("{:x}", plaintext_hash.finalize()) != sha256
        || plaintext_size != size
        || hash_file(source)? != (sha256.clone(), size)
    {
        return Err("source_changed");
    }
    let encrypted_sha256 = hash_file(temp.path())?.0;
    let mut encrypted = File::open(temp.path()).map_err(|_| "local_read_failed")?;
    let mut chunks = Vec::new();
    let mut buffer = vec![0u8; CHUNK_SIZE];
    loop {
        let mut used = 0;
        while used < buffer.len() {
            let n = encrypted
                .read(&mut buffer[used..])
                .map_err(|_| "local_read_failed")?;
            if n == 0 {
                break;
            }
            used += n;
        }
        if used == 0 {
            break;
        }
        chunks.push(object_store::digest(&buffer[..used]));
    }
    let journal = Journal {
        manifest: Manifest {
            version: if content_named { 2 } else { 1 },
            kind: kind.into(),
            relative_path: relative.into(),
            sha256,
            size,
            upload_id: Uuid::new_v4().to_string(),
            chunks,
            created_at: chrono::Utc::now().to_rfc3339(),
        },
        encrypted_sha256,
    };
    if serde_json::to_vec(&journal.manifest)
        .map_err(|_| "invalid_manifest")?
        .len()
        > MAX_MANIFEST_PLAINTEXT
    {
        return Err("file_too_large");
    }
    let mut journal_temp =
        tempfile::NamedTempFile::new_in(stage).map_err(|_| "local_write_failed")?;
    serde_json::to_writer(&mut journal_temp, &journal).map_err(|_| "local_write_failed")?;
    journal_temp
        .as_file()
        .sync_all()
        .map_err(|_| "local_write_failed")?;
    temp.persist_noclobber(&cipher_path)
        .map_err(|_| "local_state_invalid")?;
    journal_temp
        .persist_noclobber(&journal_path)
        .map_err(|_| "local_state_invalid")?;
    Ok((journal, journal_path, cipher_path))
}

pub(crate) struct UploadSpec<'a> {
    pub(crate) source: &'a Path,
    pub(crate) relative: &'a str,
    pub(crate) kind: &'a str,
    pub(crate) stage: &'a Path,
}

pub(crate) async fn upload_file<F>(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    spec: UploadSpec<'_>,
    mut on_progress: F,
) -> Result<Manifest, &'static str>
where
    F: FnMut(u64, u64),
{
    let (journal, journal_path, cipher_path) = prepare_journal(
        spec.source,
        spec.relative,
        spec.kind,
        spec.stage,
        identity,
        config.write_safety == Some(object_store::WriteSafety::CheckThenWrite),
    )?;
    let mut input = File::open(&cipher_path).map_err(|_| "local_read_failed")?;
    let total = input.metadata().map_err(|_| "local_read_failed")?.len();
    let mut uploaded = 0u64;
    let mut buffer = vec![0u8; CHUNK_SIZE];
    for (index, expected) in journal.manifest.chunks.iter().enumerate() {
        let mut used = 0;
        while used < buffer.len() {
            let n = input
                .read(&mut buffer[used..])
                .map_err(|_| "local_read_failed")?;
            if n == 0 {
                break;
            }
            used += n;
        }
        if object_store::digest(&buffer[..used]) != *expected {
            return Err("local_state_invalid");
        }
        let key = chunk_key(&journal.manifest, index);
        put_once(config, keys, &key, buffer[..used].to_vec()).await?;
        uploaded += used as u64;
        on_progress(uploaded, total);
    }
    let mut extra = [0u8; 1];
    if input.read(&mut extra).map_err(|_| "local_read_failed")? != 0 {
        return Err("local_state_invalid");
    }
    // Reuse the exact encrypted manifest after an uncertain write so resumption is idempotent.
    let manifest_path = journal_path.with_extension("manifest.age");
    let plain = serde_json::to_vec(&journal.manifest).map_err(|_| "invalid_manifest")?;
    let encrypted = if manifest_path.exists() {
        let bytes = fs::read(&manifest_path).map_err(|_| "local_read_failed")?;
        if decrypt_bytes(&bytes, identity, MAX_MANIFEST)? != plain {
            return Err("local_state_invalid");
        }
        bytes
    } else {
        let bytes = encrypt_bytes(&plain, identity)?;
        let mut file =
            tempfile::NamedTempFile::new_in(spec.stage).map_err(|_| "local_write_failed")?;
        file.write_all(&bytes).map_err(|_| "local_write_failed")?;
        file.as_file()
            .sync_all()
            .map_err(|_| "local_write_failed")?;
        file.persist_noclobber(&manifest_path)
            .map_err(|_| "local_state_invalid")?;
        bytes
    };
    if encrypted.len() > MAX_MANIFEST {
        return Err("file_too_large");
    }
    let key = manifest_key(
        config,
        &journal.manifest.created_at,
        &journal.manifest.upload_id,
        &encrypted,
    );
    put_once(config, keys, &key, encrypted).await?;
    fs::remove_file(&cipher_path).map_err(|_| "local_write_failed")?;
    fs::remove_file(&journal_path).map_err(|_| "local_write_failed")?;
    fs::remove_file(&manifest_path).map_err(|_| "local_write_failed")?;
    Ok(journal.manifest)
}

pub(crate) fn object_files(root: &Path) -> Result<Vec<(String, PathBuf, u64)>, &'static str> {
    let mut files = Vec::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|_| "local_read_failed")?;
        if entry.file_type().is_symlink() {
            return Err("unsafe_path");
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(root).map_err(|_| "unsafe_path")?;
        if !safe_relative(relative) {
            return Err("unsafe_path");
        }
        let name = relative.to_str().ok_or("unsafe_path")?.replace('\\', "/");
        files.push((
            name,
            entry.path().to_path_buf(),
            entry.metadata().map_err(|_| "local_read_failed")?.len(),
        ));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

pub(crate) async fn pending_files<F: FnMut(Option<&str>)>(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    objects: &Path,
    mut on_file: F,
) -> Result<Vec<(String, PathBuf, u64)>, &'static str> {
    let catalog = catalog(config, keys, identity).await?;
    let available = list_keys(config, keys, &format!("{}/data/", object_store::PREFIX)).await?;
    let available: std::collections::HashSet<_> = available.into_iter().collect();
    let mut pending = Vec::new();
    for (relative, path, size) in object_files(objects)? {
        on_file(Some(&relative));
        let (hash, observed) = hash_file(&path)?;
        if observed != size {
            return Err("source_changed");
        }
        let backed_up = catalog.iter().any(|m| {
            m.kind == "object"
                && m.relative_path == relative
                && m.sha256 == hash
                && m.size == size
                && m.chunks
                    .iter()
                    .enumerate()
                    .all(|(i, _)| available.contains(&chunk_key(m, i)))
        });
        if !backed_up {
            pending.push((relative, path, size));
        }
    }
    on_file(None);
    Ok(pending)
}

async fn cleanup_completed(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    stage: &Path,
) -> Result<(), &'static str> {
    if let Some(parent) = stage.parent() {
        match fs::symlink_metadata(parent) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => (),
            Ok(_) => return Err("unsafe_path"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err("local_read_failed"),
        }
    }
    match fs::symlink_metadata(stage) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => (),
        Ok(_) => return Err("unsafe_path"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("local_read_failed"),
    };
    let manifests = catalog(config, keys, identity).await?;
    let available: std::collections::HashSet<_> =
        list_keys(config, keys, &format!("{}/data/", object_store::PREFIX))
            .await?
            .into_iter()
            .collect();
    for entry in fs::read_dir(stage).map_err(|_| "local_read_failed")? {
        let entry = entry.map_err(|_| "local_read_failed")?;
        let meta = entry.file_type().map_err(|_| "local_read_failed")?;
        if !meta.is_file()
            || meta.is_symlink()
            || entry.path().extension().is_none_or(|ext| ext != "json")
        {
            continue;
        }
        let journal: Journal = match fs::read(entry.path())
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
        {
            Some(value) => value,
            None => continue,
        };
        let expected = object_store::digest(
            format!(
                "{}\0{}\0{}",
                journal.manifest.kind, journal.manifest.relative_path, journal.manifest.sha256
            )
            .as_bytes(),
        );
        if entry.file_name().to_string_lossy() != format!("{expected}.json") {
            continue;
        }
        if !manifests.iter().any(|m| m == &journal.manifest) {
            continue;
        }
        if !journal
            .manifest
            .chunks
            .iter()
            .enumerate()
            .all(|(i, _)| available.contains(&chunk_key(&journal.manifest, i)))
        {
            continue;
        }
        let cipher = stage.join(format!("{expected}.age"));
        if cipher.exists() {
            let metadata = fs::symlink_metadata(&cipher).map_err(|_| "local_read_failed")?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || hash_file(&cipher)?.0 != journal.encrypted_sha256
            {
                return Err("local_state_invalid");
            }
            fs::remove_file(cipher).map_err(|_| "local_write_failed")?;
        }
        let encrypted_manifest = stage.join(format!("{expected}.manifest.age"));
        if encrypted_manifest.exists() {
            let metadata =
                fs::symlink_metadata(&encrypted_manifest).map_err(|_| "local_read_failed")?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err("local_state_invalid");
            }
            let data = fs::read(&encrypted_manifest).map_err(|_| "local_read_failed")?;
            if decrypt_bytes(&data, identity, MAX_MANIFEST)?
                != serde_json::to_vec(&journal.manifest).map_err(|_| "local_state_invalid")?
            {
                return Err("local_state_invalid");
            }
            fs::remove_file(encrypted_manifest).map_err(|_| "local_write_failed")?;
        }
        fs::remove_file(entry.path()).map_err(|_| "local_write_failed")?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn object_store_backup(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<BackupProgress, &'static str> {
    let paths = state.paths.scoped().await;
    let _guard = object_store::CONNECTION_LOCK.lock().await;
    let mut active_file = None;
    let result: Result<BackupProgress, &'static str> = async {
        let mut config = object_store::load_config(&paths.data_root)?.ok_or("not_connected")?;
        let keys = object_store::load_keys(&config)?;
        let identity = identity(&paths.database_path)?;
        let stage = paths
            .data_root
            .join("object-store-staging")
            .join(&config.credential_id);
        cleanup_completed(&config, &keys, &identity, &stage).await?;
        let pending = pending_files(&config, &keys, &identity, &paths.objects_path, |file| {
            active_file = file.map(str::to_owned)
        })
        .await?;
        let mut reusable = catalog(&config, &keys, &identity).await?;
        let mut available: std::collections::HashSet<_> =
            list_keys(&config, &keys, &format!("{}/data/", object_store::PREFIX))
                .await?
                .into_iter()
                .collect();
        let total_bytes = pending
            .iter()
            .try_fold(0u64, |sum, item| sum.checked_add(item.2))
            .ok_or("unavailable")?;
        let mut progress = BackupProgress {
            completed_count: 0,
            completed_bytes: 0,
            total_count: pending.len() as u64,
            total_bytes,
        };
        let _ = app.emit("object-store-progress", progress.clone());
        for (relative, path, size) in pending {
            active_file = Some(relative.clone());
            let initial_progress = progress.clone();
            let (hash, observed) = hash_file(&path)?;
            if observed != size {
                return Err("source_changed");
            }
            let existing = reusable.iter().find(|m| {
                m.kind == "object"
                    && m.sha256 == hash
                    && m.size == size
                    && m.chunks
                        .iter()
                        .enumerate()
                        .all(|(i, _)| available.contains(&chunk_key(m, i)))
            });
            let manifest = if let Some(existing) = existing {
                publish_existing_content(&config, &keys, &identity, existing, &relative).await?
            } else {
                upload_file(
                    &config,
                    &keys,
                    &identity,
                    UploadSpec {
                        source: &path,
                        relative: &relative,
                        kind: "object",
                        stage: &stage,
                    },
                    |uploaded, total| {
                        let mut update = initial_progress.clone();
                        if total != 0 {
                            update.completed_bytes +=
                                ((size as u128 * uploaded as u128) / total as u128) as u64;
                        }
                        let _ = app.emit("object-store-progress", update);
                    },
                )
                .await?
            };
            for (i, _) in manifest.chunks.iter().enumerate() {
                available.insert(chunk_key(&manifest, i));
            }
            reusable.push(manifest);
            progress.completed_count += 1;
            progress.completed_bytes += size;
            let _ = app.emit("object-store-progress", progress.clone());
        }
        let safe_stage = fs::symlink_metadata(&stage)
            .ok()
            .is_some_and(|m| m.is_dir() && !m.file_type().is_symlink());
        let safe_parent = stage
            .parent()
            .and_then(|p| fs::symlink_metadata(p).ok())
            .is_some_and(|m| m.is_dir() && !m.file_type().is_symlink());
        if safe_stage
            && safe_parent
            && fs::read_dir(&stage)
                .map_err(|_| "local_read_failed")?
                .next()
                .is_none()
        {
            fs::remove_dir(&stage).map_err(|_| "local_write_failed")?;
            if let Some(parent) = stage.parent() {
                if fs::read_dir(parent)
                    .map_err(|_| "local_read_failed")?
                    .next()
                    .is_none()
                {
                    fs::remove_dir(parent).map_err(|_| "local_write_failed")?;
                }
            }
        }
        config.last_backup_at = Some(chrono::Utc::now().to_rfc3339());
        object_store::save_config(&paths.data_root, &config)?;
        Ok(progress)
    }
    .await;
    match result {
        Ok(progress) => {
            let _ = app.emit("object-store-done", progress.clone());
            Ok(progress)
        }
        Err(error) => {
            let _ = app.emit(
                "object-store-failed",
                BackupFailure {
                    file: active_file,
                    state: error,
                },
            );
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        net::TcpListener,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
        time::Duration,
    };

    #[test]
    fn missing_recovery_profile_does_not_create_a_database_record() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("test.db");
        let connection = Connection::open(&database).unwrap();
        connection.execute_batch("CREATE TABLE momso_backup_profiles (workspace_id TEXT, recovery_key_confirmed_at TEXT, created_at TEXT)").unwrap();
        drop(connection);
        assert!(matches!(identity(&database), Err("recovery_key_missing")));
        let connection = Connection::open(&database).unwrap();
        let count: i64 = connection
            .query_row("SELECT count(*) FROM momso_backup_profiles", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn download_rejects_an_oversized_chunked_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer).unwrap();
            stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n8\r\n12345678\r\n0\r\n\r\n").unwrap();
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
        assert!(matches!(
            get_bytes(&config, &keys, "key", 4).await,
            Err("invalid_remote_object")
        ));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn encrypted_chunks_resume_and_manifest_is_readable() {
        for provider in ["aws", "gcp", "cloudflare", "ncp", "s3_compatible"] {
            exercise_server(provider, true).await;
            exercise_server(provider, false).await;
        }
    }

    async fn exercise_server(provider: &str, conditional: bool) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let data = Arc::new(Mutex::new(HashMap::<String, Vec<u8>>::new()));
        let writes = Arc::new(Mutex::new(HashMap::<String, usize>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let server_data = Arc::clone(&data);
        let server_writes = Arc::clone(&writes);
        let server_stop = Arc::clone(&stop);
        let server = std::thread::spawn(move || {
            let mut failed_once = false;
            while !server_stop.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(15)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 16384];
                let header_end = loop {
                    let n = stream.read(&mut buffer).unwrap();
                    assert_ne!(n, 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(at) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                };
                let head = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
                let line = head.lines().next().unwrap();
                let mut fields = line.split_whitespace();
                let method = fields.next().unwrap();
                let path = fields.next().unwrap();
                let length = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while bytes.len() - header_end < length {
                    let n = stream.read(&mut buffer).unwrap();
                    assert_ne!(n, 0);
                    bytes.extend_from_slice(&buffer[..n]);
                }
                let key = path
                    .trim_start_matches("/bucket/")
                    .split('?')
                    .next()
                    .unwrap();
                let (status, body) = if method == "GET" && path.starts_with("/bucket?") {
                    let objects = server_data.lock().unwrap();
                    let data_list = path.contains("data%2F");
                    let entries = objects
                        .keys()
                        .filter(|key| {
                            key.contains(if data_list { "/data/" } else { "/manifests/" })
                        })
                        .map(|key| format!("<Contents><Key>{key}</Key></Contents>"))
                        .collect::<String>();
                    ("200 OK", format!("<ListBucketResult><IsTruncated>false</IsTruncated>{entries}</ListBucketResult>").into_bytes())
                } else if method == "GET" || method == "HEAD" {
                    match server_data.lock().unwrap().get(key) {
                        Some(value) => ("200 OK", value.clone()),
                        None => ("404 Not Found", Vec::new()),
                    }
                } else if method == "PUT"
                    && key.contains("/data/")
                    && (key.ends_with("00000001") || key.contains("/00000001-"))
                    && !failed_once
                {
                    failed_once = true;
                    ("503 Service Unavailable", Vec::new())
                } else if method == "PUT" {
                    let mut objects = server_data.lock().unwrap();
                    if conditional && objects.contains_key(key) {
                        ("412 Precondition Failed", Vec::new())
                    } else {
                        objects.insert(
                            key.to_owned(),
                            bytes[header_end..header_end + length].to_vec(),
                        );
                        *server_writes
                            .lock()
                            .unwrap()
                            .entry(key.to_owned())
                            .or_default() += 1;
                        ("200 OK", Vec::new())
                    }
                } else {
                    ("400 Bad Request", Vec::new())
                };
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(header.as_bytes()).unwrap();
                if method != "HEAD" {
                    stream.write_all(&body).unwrap();
                }
            }
        });
        let config = object_store::Config {
            provider: provider.into(),
            endpoint: format!("http://{address}"),
            region: "auto".into(),
            bucket: "bucket".into(),
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(if conditional {
                object_store::WriteSafety::Conditional
            } else {
                object_store::WriteSafety::CheckThenWrite
            }),
        };
        let keys = object_store::Keys {
            access_key_id: "TEST".into(),
            secret_access_key: "test".into(),
        };
        assert_eq!(
            object_store::probe_connection(&config, &keys)
                .await
                .unwrap(),
            config.write_safety.unwrap()
        );
        let identity = x25519::Identity::generate();
        let temp = tempfile::tempdir().unwrap();
        let objects = temp.path().join("objects");
        let source = objects.join("sha256/ab/file");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        let original = vec![b'Z'; CHUNK_SIZE + 1000];
        fs::write(&source, &original).unwrap();
        let stage = temp.path().join("stage");
        assert!(matches!(
            upload_file(
                &config,
                &keys,
                &identity,
                UploadSpec {
                    source: &source,
                    relative: "sha256/ab/file",
                    kind: "object",
                    stage: &stage
                },
                |_, _| {}
            )
            .await,
            Err("remote_write_failed")
        ));
        let manifest = upload_file(
            &config,
            &keys,
            &identity,
            UploadSpec {
                source: &source,
                relative: "sha256/ab/file",
                kind: "object",
                stage: &stage,
            },
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(manifest.chunks.len(), 2);
        let listed = catalog(&config, &keys, &identity).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].sha256, object_store::digest(&original));
        let available: std::collections::HashSet<_> =
            list_keys(&config, &keys, &format!("{}/data/", object_store::PREFIX))
                .await
                .unwrap()
                .into_iter()
                .collect();
        let local = std::collections::HashSet::from([(
            manifest.relative_path.clone(),
            manifest.sha256.clone(),
        )]);
        let verification =
            crate::object_store_restore::compare_lists(&listed, &available, &local).unwrap();
        let verification = serde_json::to_value(verification).unwrap();
        for field in ["missing", "extra", "missingChunks", "unreferencedObjects"] {
            assert_eq!(verification[field].as_array().unwrap().len(), 0);
        }
        let restored = crate::object_store_restore::download_verified(
            &config,
            &keys,
            &identity,
            &manifest,
            temp.path(),
        )
        .await
        .unwrap();
        assert_eq!(fs::read(restored.path()).unwrap(), original);
        let conflict_key = chunk_key(&manifest, 0);
        let before = data.lock().unwrap().get(&conflict_key).unwrap().clone();
        assert_eq!(
            put_once(&config, &keys, &conflict_key, vec![0; before.len()]).await,
            Err("remote_conflict")
        );
        assert_eq!(data.lock().unwrap().get(&conflict_key).unwrap(), &before);
        let second = objects.join("sha256/cd/second");
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&second, &original).unwrap();
        let linked =
            publish_existing_content(&config, &keys, &identity, &manifest, "sha256/cd/second")
                .await
                .unwrap();
        assert_eq!(linked.upload_id, manifest.upload_id);
        assert_eq!(
            data.lock()
                .unwrap()
                .keys()
                .filter(|key| key.contains("/data/"))
                .count(),
            2
        );
        assert!(pending_files(&config, &keys, &identity, &objects, |_| {})
            .await
            .unwrap()
            .is_empty());
        assert!(writes
            .lock()
            .unwrap()
            .iter()
            .filter(|(key, _)| !key.contains("/probes/"))
            .all(|(_, v)| *v == 1));
        assert!(data
            .lock()
            .unwrap()
            .values()
            .all(|value| !value.windows(64).any(|window| window == &original[..64])));
        assert_eq!(fs::read_dir(&stage).unwrap().count(), 0);
        let id = object_store::digest(
            format!(
                "{}\0{}\0{}",
                manifest.kind, manifest.relative_path, manifest.sha256
            )
            .as_bytes(),
        );
        let journal = Journal {
            manifest,
            encrypted_sha256: "unused".into(),
        };
        let journal_path = stage.join(format!("{id}.json"));
        fs::write(&journal_path, serde_json::to_vec(&journal).unwrap()).unwrap();
        fs::write(stage.join("keep.txt"), b"unrelated").unwrap();
        cleanup_completed(&config, &keys, &identity, &stage)
            .await
            .unwrap();
        assert!(!journal_path.exists());
        assert!(stage.join("keep.txt").exists());
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }
}
