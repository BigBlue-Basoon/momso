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
    #[serde(rename = "IsTruncated")]
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
    let Some(raw) = path.to_str() else {
        return false;
    };
    if raw.is_empty() || raw.encode_utf16().count() > 32_000 {
        return false;
    }
    path.components().all(|component| {
        let Component::Normal(part) = component else {
            return false;
        };
        let Some(name) = part.to_str() else {
            return false;
        };
        if name.is_empty()
            || name.encode_utf16().count() > 255
            || name.ends_with(['.', ' '])
            || name
                .chars()
                .any(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        {
            return false;
        }
        let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) {
            return false;
        }
        let device_suffix = stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"));
        !device_suffix.is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    })
}

// Legacy objects can have arbitrary names. Only the reserved, canonical
// content-addressed layout promises that the file name is its SHA-256.
pub(crate) fn validate_content_address(relative: &str, hash: &str) -> Result<(), &'static str> {
    let mut parts = Path::new(relative).components();
    let mismatch = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (
            Some(Component::Normal(folder)),
            Some(Component::Normal(prefix)),
            Some(Component::Normal(name)),
            None,
        ) => {
            folder == "sha256"
                && name.to_str().is_some_and(|name| {
                    crate::archive_zet::valid_asset_id(&format!("sha256:{name}"))
                        && (prefix != &name[..2] || name != hash)
                })
        }
        _ => false,
    };
    if mismatch {
        return Err("source_integrity_mismatch");
    }
    Ok(())
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
        let page: ListResponse = if crate::r2_oauth::uses_oauth(config) {
            r2_list_response(&body, prefix)?
        } else {
            s3_list_response(&body)?
        };
        if page
            .contents
            .iter()
            .any(|entry| !entry.key.starts_with(prefix))
        {
            return Err("remote_list_failed");
        }
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

fn s3_list_response(body: &str) -> Result<ListResponse, &'static str> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(body);
    loop {
        match reader.read_event().map_err(|_| "remote_list_failed")? {
            Event::Start(element) if element.local_name().as_ref() == b"ListBucketResult" => break,
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => (),
            Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => (),
            _ => return Err("remote_list_failed"),
        }
    }
    quick_xml::de::from_str(body).map_err(|_| "remote_list_failed")
}

fn r2_list_response(body: &str, prefix: &str) -> Result<ListResponse, &'static str> {
    let data: serde_json::Value = serde_json::from_str(body).map_err(|_| "remote_list_failed")?;
    if data.get("success").and_then(|v| v.as_bool()) != Some(true) {
        return Err("remote_list_failed");
    }
    let values = data
        .get("result")
        .and_then(|v| v.as_array())
        .ok_or("remote_list_failed")?;
    let mut contents = Vec::with_capacity(values.len());
    for value in values {
        let key = value
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or("remote_list_failed")?;
        if !key.starts_with(prefix)
            || !key.starts_with("momso/object-store/v1/")
            || key
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err("remote_list_failed");
        }
        contents.push(ListEntry {
            key: key.to_owned(),
        });
    }
    let info = data
        .get("result_info")
        .and_then(|v| v.as_object())
        .ok_or("remote_list_failed")?;
    let next = match info.get("cursor") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(value.as_str().ok_or("remote_list_failed")?.to_owned()),
    };
    let truncated = match info.get("is_truncated") {
        Some(value) => value.as_bool().ok_or("remote_list_failed")?,
        None if next.as_deref().is_some_and(|value| !value.is_empty()) => true,
        None => return Err("remote_list_failed"),
    };
    Ok(ListResponse {
        contents,
        truncated,
        next,
    })
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
        if crate::r2_oauth::uses_oauth(config) {
            read_bounded(response, 64 * 1024)
                .await
                .map_err(|_| "remote_read_failed")?;
        }
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

pub(crate) async fn read_bounded(
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

pub(crate) async fn put_once(
    config: &object_store::Config,
    keys: &object_store::Keys,
    key: &str,
    bytes: Vec<u8>,
) -> Result<(), &'static str> {
    if config.write_safety == Some(object_store::WriteSafety::CheckThenWrite) {
        if crate::r2_oauth::uses_oauth(config) {
            let existing = get_bytes(config, keys, key, bytes.len())
                .await
                .map_err(|error| {
                    if error == "invalid_remote_object" {
                        "remote_conflict"
                    } else {
                        error
                    }
                })?;
            if let Some(existing) = existing {
                return if existing == bytes {
                    Ok(())
                } else {
                    Err("remote_conflict")
                };
            }
            let response =
                object_store::request(config, keys, reqwest::Method::PUT, key, &[], bytes.clone())
                    .await?;
            if !response.status().is_success() {
                return Err("remote_write_failed");
            }
            read_bounded(response, 64 * 1024)
                .await
                .map_err(|_| "remote_write_failed")?;
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

pub(crate) fn manifest_key(
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

pub(crate) async fn catalog(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
) -> Result<Vec<Manifest>, &'static str> {
    Ok(catalog_with_keys(config, keys, identity)
        .await?
        .into_iter()
        .map(|(_, manifest, _)| manifest)
        .collect())
}

pub(crate) async fn catalog_with_keys(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
) -> Result<Vec<(String, Manifest, String)>, &'static str> {
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
        let encrypted_hash = object_store::digest(&encrypted);
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
        result.push((key, manifest, encrypted_hash));
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
    if kind == "object" {
        validate_content_address(relative, &sha256)?;
    }
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

// Interrupted uploads contain encrypted recoverable originals. Only remove a
// staged file after authenticating and hashing its decrypted content.
pub(crate) fn purge_staged_source(
    data_root: &Path,
    target_hash: &str,
    identity: Option<&x25519::Identity>,
    mut preserve_remote: impl FnMut(&str, &Manifest, Option<&[u8]>) -> Result<(), &'static str>,
) -> Vec<String> {
    let root = data_root.join("object-store-staging");
    let metadata = match fs::symlink_metadata(&root) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(_) => return vec!["upload_staging_unverified".into()],
    };
    if !metadata.is_dir() || crate::archive_location::is_link_or_reparse(&metadata) {
        return vec!["upload_staging_unverified".into()];
    }
    let Ok(parent) = fs::symlink_metadata(data_root) else {
        return vec!["upload_staging_unverified".into()];
    };
    if !parent.is_dir() || crate::archive_location::is_link_or_reparse(&parent) {
        return vec!["upload_staging_unverified".into()];
    }
    let Some(identity) = identity else {
        return vec!["upload_staging_unverified".into()];
    };
    let mut remaining = Vec::new();
    let Ok(directories) = fs::read_dir(&root) else {
        return vec!["upload_staging_unverified".into()];
    };
    for directory in directories {
        let Ok(directory) = directory else {
            remaining.push("upload_staging_unverified".into());
            continue;
        };
        let dir = directory.path();
        let Ok(meta) = fs::symlink_metadata(&dir) else {
            remaining.push("upload_staging_unverified".into());
            continue;
        };
        if !meta.is_dir()
            || crate::archive_location::is_link_or_reparse(&meta)
            || Uuid::parse_str(&directory.file_name().to_string_lossy()).is_err()
        {
            remaining.push("upload_staging_unverified".into());
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            remaining.push("upload_staging_unverified".into());
            continue;
        };
        let mut journals = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) if entry.path().extension().is_some_and(|ext| ext == "json") => {
                    journals.push(entry.path())
                }
                Ok(_) => (),
                Err(_) => remaining.push("upload_staging_unverified".into()),
            }
        }
        for journal_path in journals {
            let result = (|| -> Result<(), &'static str> {
                let meta = fs::symlink_metadata(&journal_path).map_err(|_| "unverified")?;
                if !meta.is_file()
                    || crate::archive_location::is_link_or_reparse(&meta)
                    || meta.len() > MAX_MANIFEST as u64
                {
                    return Err("unverified");
                }
                let data = fs::read(&journal_path).map_err(|_| "unverified")?;
                let journal: Journal = serde_json::from_slice(&data).map_err(|_| "unverified")?;
                if journal.manifest.kind != "object" || journal.manifest.sha256 != target_hash {
                    return Ok(());
                }
                if !safe_relative(Path::new(&journal.manifest.relative_path)) {
                    return Err("unverified");
                }
                let id = object_store::digest(
                    format!(
                        "object\0{}\0{}",
                        journal.manifest.relative_path, target_hash
                    )
                    .as_bytes(),
                );
                if journal_path.file_name().and_then(|v| v.to_str()) != Some(&format!("{id}.json"))
                {
                    return Err("unverified");
                }
                let cipher = dir.join(format!("{id}.age"));
                let meta = fs::symlink_metadata(&cipher).map_err(|_| "unverified")?;
                if !meta.is_file()
                    || crate::archive_location::is_link_or_reparse(&meta)
                    || hash_file(&cipher).map_err(|_| "unverified")?.0 != journal.encrypted_sha256
                {
                    return Err("unverified");
                }
                if !matches!(journal.manifest.version, 1 | 2)
                    || Uuid::parse_str(&journal.manifest.upload_id).is_err()
                {
                    return Err("unverified");
                }
                let mut encrypted_chunks = File::open(&cipher).map_err(|_| "unverified")?;
                let mut chunk_buffer = vec![0u8; CHUNK_SIZE];
                let mut index = 0usize;
                loop {
                    let mut used = 0;
                    while used < chunk_buffer.len() {
                        let n = encrypted_chunks
                            .read(&mut chunk_buffer[used..])
                            .map_err(|_| "unverified")?;
                        if n == 0 {
                            break;
                        }
                        used += n;
                    }
                    if used == 0 {
                        break;
                    }
                    if journal.manifest.chunks.get(index)
                        != Some(&object_store::digest(&chunk_buffer[..used]))
                    {
                        return Err("unverified");
                    }
                    index += 1;
                }
                if index != journal.manifest.chunks.len() {
                    return Err("unverified");
                }
                let encrypted = File::open(&cipher).map_err(|_| "unverified")?;
                let decryptor = age::Decryptor::new(encrypted).map_err(|_| "unverified")?;
                let mut reader = decryptor
                    .decrypt(std::iter::once(identity as &dyn age::Identity))
                    .map_err(|_| "unverified")?;
                let mut digest = Sha256::new();
                let mut length = 0u64;
                let mut buffer = [0u8; 65536];
                loop {
                    let count = reader.read(&mut buffer).map_err(|_| "unverified")?;
                    if count == 0 {
                        break;
                    }
                    length = length.checked_add(count as u64).ok_or("unverified")?;
                    if length > journal.manifest.size {
                        return Err("unverified");
                    }
                    digest.update(&buffer[..count]);
                }
                if length != journal.manifest.size
                    || format!("{:x}", digest.finalize()) != target_hash
                {
                    return Err("unverified");
                }
                let manifest = dir.join(format!("{id}.manifest.age"));
                let encrypted_manifest = match fs::symlink_metadata(&manifest) {
                    Ok(meta) => {
                        if !meta.is_file()
                            || crate::archive_location::is_link_or_reparse(&meta)
                            || meta.len() > MAX_MANIFEST as u64
                        {
                            return Err("unverified");
                        }
                        let encrypted = fs::read(&manifest).map_err(|_| "unverified")?;
                        if decrypt_bytes(&encrypted, identity, MAX_MANIFEST)
                            .map_err(|_| "unverified")?
                            != serde_json::to_vec(&journal.manifest).map_err(|_| "unverified")?
                        {
                            return Err("unverified");
                        }
                        Some(encrypted)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(_) => return Err("unverified"),
                };
                let stage_id = directory.file_name();
                let stage_id = stage_id.to_str().ok_or("unverified")?;
                preserve_remote(stage_id, &journal.manifest, encrypted_manifest.as_deref())?;
                if encrypted_manifest.is_some() {
                    fs::remove_file(&manifest).map_err(|_| "unverified")?;
                }
                fs::remove_file(&cipher).map_err(|_| "unverified")?;
                fs::remove_file(&journal_path).map_err(|_| "unverified")?;
                Ok(())
            })();
            if result.is_err() {
                remaining.push(journal_path.to_string_lossy().into_owned());
            }
        }
        // Orphaned encrypted files without a complete journal cannot safely be classified.
        if fs::read_dir(&dir)
            .map(|mut it| it.next().is_some())
            .unwrap_or(true)
        {
            remaining.push("upload_staging_unverified".into());
        }
    }
    remaining.sort();
    remaining.dedup();
    remaining
}

pub(crate) async fn pending_files<F: FnMut(Option<&str>)>(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    database: &Path,
    objects: &Path,
    mut on_file: F,
) -> Result<Vec<(String, PathBuf, u64)>, &'static str> {
    let (deleted_hashes, deleted_paths) = {
        let db = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "unavailable")?;
        let mut statement = db.prepare("SELECT a.content_sha256,a.storage_relative_path FROM momso_source_assets a JOIN source_asset_deletions d ON d.asset_id=a.id").map_err(|_| "unavailable")?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|_| "unavailable")?;
        let mut hashes = std::collections::HashSet::new();
        let mut paths = std::collections::HashSet::new();
        for row in rows {
            let (hash, path) = row.map_err(|_| "unavailable")?;
            hashes.insert(hash);
            paths.insert(path.replace('\\', "/"));
        }
        (hashes, paths)
    };
    let catalog = catalog(config, keys, identity).await?;
    let available = list_keys(config, keys, &format!("{}/data/", object_store::PREFIX)).await?;
    let available: std::collections::HashSet<_> = available.into_iter().collect();
    let mut pending = Vec::new();
    for (relative, path, size) in object_files(objects)? {
        if deleted_paths.contains(&relative) {
            continue;
        }
        on_file(Some(&relative));
        let (hash, observed) = hash_file(&path)?;
        if observed != size {
            return Err("source_changed");
        }
        validate_content_address(&relative, &hash)?;
        if deleted_hashes.contains(&hash) {
            continue;
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
pub(crate) async fn object_store_backup<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
) -> Result<BackupProgress, &'static str> {
    let paths = state.paths.scoped().await;
    let _guard = object_store::CONNECTION_LOCK.lock().await;
    let mut active_file = None;
    let result: Result<BackupProgress, &'static str> = async {
        let mut config = object_store::load_config(&paths.data_root)?.ok_or("not_connected")?;
        let keys = object_store::load_keys(&config)?;
        let identity = identity(&paths.database_path)?;
        let remote_lock = if crate::object_store_lock::supported(&config) {
            Some(
                crate::object_store_lock::SourceMutationLock::acquire(
                    &paths.data_root,
                    &config,
                    &keys,
                )
                .await?,
            )
        } else {
            None
        };
        let work: Result<BackupProgress, &'static str> = async {
            crate::source_delete::retry_pending_for_backup(&paths, &config, &keys, &identity)
                .await?;
            let stage = paths
                .data_root
                .join("object-store-staging")
                .join(&config.credential_id);
            cleanup_completed(&config, &keys, &identity, &stage).await?;
            let pending = pending_files(
                &config,
                &keys,
                &identity,
                &paths.database_path,
                &paths.objects_path,
                |file| active_file = file.map(str::to_owned),
            )
            .await?;
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
                let (_, observed) = hash_file(&path)?;
                if observed != size {
                    return Err("source_changed");
                }
                // Each new manifest owns its own upload ID and chunks. Sharing
                // another manifest's chunks races with a delete on a different PC.
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
                .await?;
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
        if let Some(lock) = remote_lock {
            lock.release(&config, &keys).await?;
        }
        work
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
    fn object_relative_paths_reject_windows_devices_and_unsafe_names() {
        for value in [
            "../outside",
            "/outside",
            "C:/outside",
            "file:stream",
            "a/NUL.txt",
            "a/con",
            "a/COM1.log",
            "a/lpt²",
            "a/trailing.",
            "a/trailing ",
            "a/question?",
            "a/pipe|name",
        ] {
            assert!(!safe_relative(Path::new(value)), "{value}");
        }
        assert!(!safe_relative(Path::new(&format!("a/{}", "x".repeat(256)))));
        assert!(safe_relative(Path::new("sha256/ab/abcdef")));
        assert!(safe_relative(Path::new("legacy/lesson.txt")));
        #[cfg(windows)]
        assert!(!safe_relative(Path::new(r"\\server\share\outside")));
    }

    #[test]
    fn corrupted_content_address_is_not_staged_for_backup() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::write(&source, b"changed original").unwrap();
        let expected = object_store::digest(b"original");
        let relative = format!("sha256/{}/{}", &expected[..2], expected);
        let stage = temp.path().join("staging").join("upload");
        let identity = x25519::Identity::generate();
        assert!(matches!(
            prepare_journal(&source, &relative, "object", &stage, &identity, true),
            Err("source_integrity_mismatch")
        ));
        assert!(!stage.exists());
        assert_eq!(fs::read(&source).unwrap(), b"changed original");
        assert!(validate_content_address("legacy/source.txt", &expected).is_ok());
        assert!(validate_content_address(&relative, &expected).is_ok());
        let native_relative = Path::new("sha256").join(&expected[..2]).join(&expected);
        assert_eq!(
            validate_content_address(
                native_relative.to_str().unwrap(),
                &object_store::digest(b"different")
            ),
            Err("source_integrity_mismatch")
        );
        let wrong_prefix = if &expected[..2] == "00" { "01" } else { "00" };
        let wrong_directory = Path::new("sha256").join(wrong_prefix).join(&expected);
        assert_eq!(
            validate_content_address(wrong_directory.to_str().unwrap(), &expected),
            Err("source_integrity_mismatch")
        );
    }

    #[test]
    fn purge_staging_removes_only_verified_matching_originals() {
        let temp = tempfile::tempdir().unwrap();
        let stage = temp
            .path()
            .join("object-store-staging")
            .join(Uuid::new_v4().to_string());
        let source = temp.path().join("source");
        let other = temp.path().join("other");
        fs::write(&source, b"synthetic private original").unwrap();
        fs::write(&other, b"different source").unwrap();
        let identity = x25519::Identity::generate();
        let (journal, journal_path, cipher_path) =
            prepare_journal(&source, "sha256/first", "object", &stage, &identity, true).unwrap();
        let (_, second_journal, second_cipher) =
            prepare_journal(&source, "sha256/second", "object", &stage, &identity, true).unwrap();
        let manifest_path = journal_path.with_extension("manifest.age");
        fs::write(
            &manifest_path,
            encrypt_bytes(&serde_json::to_vec(&journal.manifest).unwrap(), &identity).unwrap(),
        )
        .unwrap();
        let (_, other_journal, other_cipher) =
            prepare_journal(&other, "sha256/other", "object", &stage, &identity, true).unwrap();
        let other_before = fs::read(&other_cipher).unwrap();
        let hash = object_store::digest(b"synthetic private original");
        assert_eq!(
            purge_staged_source(temp.path(), &hash, None, |_, _, _| Ok(())),
            ["upload_staging_unverified"]
        );
        assert!(journal_path.exists() && cipher_path.exists() && manifest_path.exists());
        let remaining = purge_staged_source(temp.path(), &hash, Some(&identity), |_, _, _| Ok(()));
        assert!(!journal_path.exists() && !cipher_path.exists() && !manifest_path.exists());
        assert!(!second_journal.exists() && !second_cipher.exists());
        assert!(other_journal.exists());
        assert_eq!(fs::read(&other_cipher).unwrap(), other_before);
        assert!(remaining.contains(&"upload_staging_unverified".to_owned()));
    }

    #[test]
    fn purge_staging_refuses_forged_journal_and_keeps_ciphertext() {
        let temp = tempfile::tempdir().unwrap();
        let stage = temp
            .path()
            .join("object-store-staging")
            .join(Uuid::new_v4().to_string());
        let source = temp.path().join("other");
        fs::write(&source, b"different source").unwrap();
        let identity = x25519::Identity::generate();
        let (mut journal, old_journal, old_cipher) =
            prepare_journal(&source, "sha256/forged", "object", &stage, &identity, true).unwrap();
        let target_hash = object_store::digest(b"synthetic private original");
        journal.manifest.sha256 = target_hash.clone();
        let new_id = object_store::digest(
            format!(
                "object\0{}\0{}",
                journal.manifest.relative_path, target_hash
            )
            .as_bytes(),
        );
        let forged_journal = stage.join(format!("{new_id}.json"));
        let forged_cipher = stage.join(format!("{new_id}.age"));
        fs::rename(old_cipher, &forged_cipher).unwrap();
        fs::remove_file(old_journal).unwrap();
        fs::write(&forged_journal, serde_json::to_vec(&journal).unwrap()).unwrap();
        let before = fs::read(&forged_cipher).unwrap();
        let remaining =
            purge_staged_source(temp.path(), &target_hash, Some(&identity), |_, _, _| Ok(()));
        assert_eq!(fs::read(&forged_cipher).unwrap(), before);
        assert!(forged_journal.exists());
        assert!(remaining
            .iter()
            .any(|item| item.ends_with(&format!("{new_id}.json"))));
    }

    #[test]
    fn purge_staging_preserves_files_when_remote_plan_cannot_be_saved_or_chunks_are_forged() {
        let temp = tempfile::tempdir().unwrap();
        let stage = temp
            .path()
            .join("object-store-staging")
            .join(Uuid::new_v4().to_string());
        let source = temp.path().join("source");
        fs::write(&source, b"original").unwrap();
        let identity = x25519::Identity::generate();
        let (mut journal, journal_path, cipher) =
            prepare_journal(&source, "sha256/item", "object", &stage, &identity, true).unwrap();
        let encrypted =
            encrypt_bytes(&serde_json::to_vec(&journal.manifest).unwrap(), &identity).unwrap();
        let manifest_path = journal_path.with_extension("manifest.age");
        fs::write(&manifest_path, encrypted).unwrap();
        let hash = object_store::digest(b"original");
        let mut called = false;
        let remaining = purge_staged_source(temp.path(), &hash, Some(&identity), |_, _, _| {
            called = true;
            Err("remote_plan_changed")
        });
        assert!(called && !remaining.is_empty());
        assert!(journal_path.exists() && cipher.exists() && manifest_path.exists());
        journal.manifest.chunks[0] = "a".repeat(64);
        fs::write(&journal_path, serde_json::to_vec(&journal).unwrap()).unwrap();
        let mut called = false;
        let remaining = purge_staged_source(temp.path(), &hash, Some(&identity), |_, _, _| {
            called = true;
            Ok(())
        });
        assert!(!called && !remaining.is_empty());
        assert!(journal_path.exists() && cipher.exists() && manifest_path.exists());
    }

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

    #[tokio::test]
    async fn oauth_r2_encrypted_backup_resume_and_restore() {
        exercise_server("r2_oauth", false).await;
    }

    #[test]
    fn oauth_listing_rejects_error_and_outside_prefix() {
        let prefix = "momso/object-store/v1/manifests/";
        assert!(r2_list_response(
            r#"{"success":false,"result":[],"result_info":{"is_truncated":false}}"#,
            prefix
        )
        .is_err());
        assert!(r2_list_response(
            r#"{"success":true,"result":[{"key":"outside"}],"result_info":{"is_truncated":false}}"#,
            prefix
        )
        .is_err());
        assert!(r2_list_response(r#"{"success":true,"result":[]}"#, prefix).is_err());
        let page = r2_list_response(r#"{"success":true,"result":[{"key":"momso/object-store/v1/manifests/a.age"}],"result_info":{"is_truncated":true,"cursor":"next"}}"#, prefix).unwrap();
        assert_eq!(page.contents.len(), 1);
        assert!(page.truncated);
        assert_eq!(page.next.as_deref(), Some("next"));
    }

    #[tokio::test]
    async fn oauth_listing_follows_cursor_without_truncation_flag_but_rejects_unknown_end() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let pages = [
                r#"{"success":true,"result":[{"key":"momso/object-store/v1/manifests/a.age"}],"result_info":{"cursor":"next"}}"#,
                r#"{"success":true,"result":[{"key":"momso/object-store/v1/manifests/b.age"}],"result_info":{"is_truncated":false}}"#,
                r#"{"success":true,"result":[],"result_info":{}}"#,
            ];
            for (index, body) in pages.iter().enumerate() {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut request = [0u8; 8192];
                let read = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..read]);
                assert!(request.contains("/r2/buckets/momso-fixture/objects?"));
                if index == 1 {
                    assert!(request.contains("cursor=next"));
                }
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        });
        let config = object_store::Config {
            provider: "cloudflare".into(),
            endpoint: format!("http://{address}"),
            region: "wrangleroauth".into(),
            bucket: "momso-fixture".into(),
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(object_store::WriteSafety::CheckThenWrite),
        };
        let keys = object_store::Keys {
            access_key_id: "test".into(),
            secret_access_key: "test-token".into(),
        };
        let prefix = "momso/object-store/v1/manifests/";
        assert_eq!(
            list_keys(&config, &keys, prefix).await.unwrap(),
            vec![format!("{prefix}a.age"), format!("{prefix}b.age")]
        );
        assert!(matches!(
            list_keys(&config, &keys, prefix).await,
            Err("remote_list_failed")
        ));
        server.join().unwrap();
        for body in [
            r#"{"success":true,"result":[],"result_info":{"cursor":""}}"#,
            r#"{"success":true,"result":[],"result_info":{"cursor":123}}"#,
            r#"{"success":true,"result":[],"result_info":{"cursor":"next","is_truncated":null}}"#,
        ] {
            assert!(r2_list_response(body, prefix).is_err());
        }
    }

    #[test]
    fn s3_listing_requires_explicit_pagination_status() {
        let incomplete = "<ListBucketResult><Contents><Key>momso/object-store/v1/manifests/a.age</Key></Contents></ListBucketResult>";
        assert!(s3_list_response(incomplete).is_err());
        let complete = "<ListBucketResult><IsTruncated>false</IsTruncated></ListBucketResult>";
        assert!(!s3_list_response(complete).unwrap().truncated);
        let error_document = "<Error><IsTruncated>false</IsTruncated></Error>";
        assert!(s3_list_response(error_document).is_err());
        let namespaced = "<?xml version=\"1.0\"?><ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><IsTruncated>false</IsTruncated></ListBucketResult>";
        assert!(!s3_list_response(namespaced).unwrap().truncated);
    }

    #[tokio::test]
    async fn s3_listing_refuses_incomplete_or_outside_pages() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let pages = [
                "<ListBucketResult><Contents><Key>momso/object-store/v1/manifests/a.age</Key></Contents></ListBucketResult>",
                "<Error><IsTruncated>false</IsTruncated></Error>",
                "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>outside</Key></Contents></ListBucketResult>",
                "<ListBucketResult><IsTruncated>true</IsTruncated><NextContinuationToken>next</NextContinuationToken><Contents><Key>momso/object-store/v1/manifests/a.age</Key></Contents></ListBucketResult>",
                "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>momso/object-store/v1/manifests/b.age</Key></Contents></ListBucketResult>",
            ];
            for (index, body) in pages.iter().enumerate() {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut request = [0u8; 8192];
                let read = stream.read(&mut request).unwrap();
                if index == 4 {
                    assert!(String::from_utf8_lossy(&request[..read])
                        .contains("continuation-token=next"));
                }
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
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
        let prefix = "momso/object-store/v1/manifests/";
        assert!(matches!(
            list_keys(&config, &keys, prefix).await,
            Err("remote_list_failed")
        ));
        assert!(matches!(
            list_keys(&config, &keys, prefix).await,
            Err("remote_list_failed")
        ));
        assert!(matches!(
            list_keys(&config, &keys, prefix).await,
            Err("remote_list_failed")
        ));
        let complete = list_keys(&config, &keys, prefix).await.unwrap();
        assert_eq!(
            complete,
            vec![format!("{prefix}a.age"), format!("{prefix}b.age")]
        );
        server.join().unwrap();
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
        let google = provider == "gcp";
        let oauth = provider == "r2_oauth";
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
                if oauth {
                    assert!(head
                        .lines()
                        .any(|line| line.eq_ignore_ascii_case("authorization: Bearer test-token")));
                }
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
                let oauth_path = format!(
                    "/client/v4/accounts/{}/r2/buckets/momso-fixture/objects",
                    "a".repeat(32)
                );
                let oauth_bucket = oauth_path.strip_suffix("/objects").unwrap();
                let oauth_collection = oauth_bucket.strip_suffix("/momso-fixture").unwrap();
                if oauth {
                    assert!(
                        path == oauth_path
                            || path.starts_with(&format!("{oauth_path}/"))
                            || path.starts_with(&format!("{oauth_path}?"))
                            || (method == "GET" && path == oauth_bucket)
                            || (method == "POST" && path == oauth_collection)
                    );
                }
                let oauth_object_prefix = format!("{oauth_path}/");
                let key = path
                    .trim_start_matches(if oauth {
                        &oauth_object_prefix
                    } else {
                        "/bucket/"
                    })
                    .split('?')
                    .next()
                    .unwrap()
                    .to_owned();
                let (status, body) = if oauth && method == "GET" && path == oauth_bucket {
                    ("404 Not Found", Vec::new())
                } else if oauth && method == "POST" && path == oauth_collection {
                    assert_eq!(
                        serde_json::from_slice::<serde_json::Value>(
                            &bytes[header_end..header_end + length]
                        )
                        .unwrap(),
                        serde_json::json!({"name":"momso-fixture"})
                    );
                    (
                        "200 OK",
                        serde_json::to_vec(
                            &serde_json::json!({"success":true,"result":{"name":"momso-fixture"}}),
                        )
                        .unwrap(),
                    )
                } else if method == "GET"
                    && ((oauth && path.starts_with(&format!("{oauth_path}?")))
                        || (!oauth && path.starts_with("/bucket?")))
                {
                    let objects = server_data.lock().unwrap();
                    let data_list = path.contains("data%2F");
                    if oauth {
                        let entries: Vec<_> = objects
                            .keys()
                            .filter(|key| {
                                key.contains(if data_list { "/data/" } else { "/manifests/" })
                            })
                            .map(|key| serde_json::json!({"key":key}))
                            .collect();
                        ("200 OK", serde_json::to_vec(&serde_json::json!({"success":true,"result":entries,"result_info":{"is_truncated":false}})).unwrap())
                    } else {
                        let entries = objects
                            .keys()
                            .filter(|key| {
                                key.contains(if data_list { "/data/" } else { "/manifests/" })
                            })
                            .map(|key| format!("<Contents><Key>{key}</Key></Contents>"))
                            .collect::<String>();
                        ("200 OK", format!("<ListBucketResult><IsTruncated>false</IsTruncated>{entries}</ListBucketResult>").into_bytes())
                    }
                } else if method == "GET" || method == "HEAD" {
                    match server_data.lock().unwrap().get(&key) {
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
                    let create_only = head.lines().any(|line| {
                        if google {
                            line.eq_ignore_ascii_case("x-goog-if-generation-match: 0")
                        } else {
                            line.eq_ignore_ascii_case("if-none-match: *")
                        }
                    });
                    if conditional && create_only && objects.contains_key(&key) {
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
            provider: if oauth { "cloudflare" } else { provider }.into(),
            endpoint: format!("http://{address}"),
            region: if oauth { "wrangleroauth" } else { "auto" }.into(),
            bucket: if oauth { "momso-fixture" } else { "bucket" }.into(),
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(if conditional {
                object_store::WriteSafety::Conditional
            } else {
                object_store::WriteSafety::CheckThenWrite
            }),
        };
        #[cfg(windows)]
        let fake_secret = if oauth {
            crate::r2_oauth::fake_wrangler_and_create(&format!("http://{address}/"))
                .await
                .unwrap()
        } else {
            "test".to_owned()
        };
        #[cfg(not(windows))]
        let fake_secret = if oauth { "test-token" } else { "test" }.to_owned();
        let keys = object_store::Keys {
            access_key_id: "TEST".into(),
            secret_access_key: fake_secret,
        };
        if !oauth {
            assert_eq!(
                object_store::probe_connection(&config, &keys)
                    .await
                    .unwrap(),
                config.write_safety.unwrap()
            );
        }
        let identity = x25519::Identity::generate();
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("state.db");
        crate::storage::initialize(&database, &temp.path().join("backups"), "test").unwrap();
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
        // A second PC backs up equal bytes under another path while the
        // first backup is being deleted. Its new manifest must not share the
        // first upload's chunks; removing those chunks must not break it.
        let independent = upload_file(
            &config,
            &keys,
            &identity,
            UploadSpec {
                source: &second,
                relative: "sha256/cd/second",
                kind: "object",
                stage: &stage,
            },
            |_, _| {},
        )
        .await
        .unwrap();
        assert_ne!(independent.upload_id, manifest.upload_id);
        for index in 0..manifest.chunks.len() {
            assert_ne!(chunk_key(&independent, index), chunk_key(&manifest, index));
        }
        let mut removed = Vec::new();
        {
            let mut remote = data.lock().unwrap();
            for index in 0..manifest.chunks.len() {
                let key = chunk_key(&manifest, index);
                removed.push((key.clone(), remote.remove(&key).unwrap()));
            }
        }
        let recovered = crate::object_store_restore::download_verified(
            &config,
            &keys,
            &identity,
            &independent,
            temp.path(),
        )
        .await
        .unwrap();
        assert_eq!(fs::read(recovered.path()).unwrap(), original);
        {
            let mut remote = data.lock().unwrap();
            remote.extend(removed);
        }
        assert!(
            pending_files(&config, &keys, &identity, &database, &objects, |_| {})
                .await
                .unwrap()
                .is_empty()
        );
        // A crash after the deletion marker can leave bytes at the old path.
        // Neither that path nor the same content under another path may be re-uploaded.
        let deleted_bytes = b"deleted synthetic source";
        let deleted_hash = object_store::digest(deleted_bytes);
        let relative = format!("sha256/{}/{}", &deleted_hash[..2], deleted_hash);
        let deleted_file = objects.join(&relative);
        fs::create_dir_all(deleted_file.parent().unwrap()).unwrap();
        fs::write(&deleted_file, deleted_bytes).unwrap();
        fs::write(objects.join("same-deleted-content"), deleted_bytes).unwrap();
        let db = crate::storage::open_writable_connection(&database).unwrap();
        db.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES('deleted-asset','other',?1,?2,?3,'deleted','test','now')", rusqlite::params![deleted_hash,deleted_bytes.len() as i64,relative]).unwrap();
        db.execute("INSERT INTO source_asset_deletions(asset_id,operation_id,state,requested_at,retain_until) VALUES('deleted-asset',?1,'pending','now','later')", [Uuid::new_v4().to_string()]).unwrap();
        drop(db);
        assert!(
            pending_files(&config, &keys, &identity, &database, &objects, |_| {})
                .await
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            pending_files(
                &config,
                &keys,
                &identity,
                &temp.path().join("missing.db"),
                &objects,
                |_| {}
            )
            .await,
            Err("unavailable")
        ));
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
