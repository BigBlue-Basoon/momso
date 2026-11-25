//! Encrypted application snapshots for installations connected only to Git.
use crate::{
    archive_remote, object_store_transfer as transfer, paired_backup::CompletedPair,
    runtime::RuntimePaths, storage,
};
use age::x25519;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::Path,
};

const MAX_METADATA: usize = 128 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GitSnapshot {
    pair: CompletedPair,
    plaintext_sha256: String,
    plaintext_size: u64,
    chunks: Vec<String>,
}

fn chunk_ref(pair: &str, index: usize) -> String {
    format!("refs/tags/momso-state/{pair}/chunk-{index:08}")
}

fn metadata_ref(pair: &str) -> String {
    format!("refs/tags/momso-state/{pair}/manifest")
}

fn upload_blob(
    tool: &Path,
    root: &Path,
    target: &str,
    dir: &Path,
    reference: &str,
    bytes: &[u8],
) -> Result<String, &'static str> {
    let mut file = tempfile::NamedTempFile::new_in(dir).map_err(|_| "local_write_failed")?;
    file.write_all(bytes).map_err(|_| "local_write_failed")?;
    file.flush().map_err(|_| "local_write_failed")?;
    let path = file.path().to_str().ok_or("local_write_failed")?;
    let object = archive_remote::git(
        tool,
        root,
        &["hash-object", "-w", "--no-filters", "--", path],
    )
    .map_err(|_| "git_unavailable")?;
    let refspec = format!("{object}:{reference}");
    archive_remote::git(tool, root, &["push", "--", target, &refspec])
        .map_err(|_| "backup_git_ref_failed")?;
    Ok(object)
}

fn fetch_blob(
    tool: &Path,
    root: &Path,
    target: &str,
    reference: &str,
    expected: &str,
    maximum: usize,
) -> Result<Vec<u8>, &'static str> {
    archive_remote::git(tool, root, &["fetch", "--no-tags", "--", target, reference])
        .map_err(|_| "remote_read_failed")?;
    let size = archive_remote::git(tool, root, &["cat-file", "-s", expected])
        .map_err(|_| "remote_read_failed")?
        .parse::<usize>()
        .map_err(|_| "invalid_backup_pair")?;
    if size > maximum {
        return Err("invalid_backup_pair");
    }
    let bytes = archive_remote::git_bytes(tool, root, &["cat-file", "blob", expected])
        .map_err(|_| "remote_read_failed")?;
    if bytes.len() != size {
        return Err("invalid_backup_pair");
    }
    Ok(bytes)
}

/// Call only while archive changes are excluded and Git HEAD is uploaded.
pub(crate) fn capture(
    tool: &Path,
    root: &Path,
    target: &str,
    revision: &str,
    paths: &RuntimePaths,
    identity: &x25519::Identity,
) -> Result<CompletedPair, &'static str> {
    let temp = tempfile::Builder::new()
        .prefix("momso-git-state-")
        .tempdir_in(&paths.backups_path)
        .map_err(|_| "local_write_failed")?;
    let snapshot = temp.path().join("snapshot.db");
    storage::create_snapshot_file(&paths.database_path, &snapshot)
        .map_err(|_| "snapshot_failed")?;
    storage::sanitize_user_owned_backup_snapshot(&snapshot).map_err(|_| "snapshot_failed")?;
    let (plaintext_sha256, plaintext_size) = transfer::hash_file(&snapshot)?;
    let mut encrypted =
        tempfile::NamedTempFile::new_in(temp.path()).map_err(|_| "local_write_failed")?;
    let recipient = identity.to_public();
    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
            .map_err(|_| "encryption_failed")?;
    let mut writer = encryptor
        .wrap_output(encrypted.as_file_mut())
        .map_err(|_| "encryption_failed")?;
    let mut input = std::fs::File::open(&snapshot).map_err(|_| "local_read_failed")?;
    std::io::copy(&mut input, &mut writer).map_err(|_| "encryption_failed")?;
    writer.finish().map_err(|_| "encryption_failed")?;
    encrypted
        .as_file()
        .sync_all()
        .map_err(|_| "local_write_failed")?;
    let pair_id = uuid::Uuid::new_v4().to_string();
    let pair = CompletedPair {
        version: 1,
        pair_id: pair_id.clone(),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        git_revision: revision.into(),
        snapshot_upload_id: pair_id.clone(),
    };
    if !pair.valid() {
        return Err("invalid_backup_pair");
    }
    let mut cipher = std::fs::File::open(encrypted.path()).map_err(|_| "local_read_failed")?;
    let mut chunks = Vec::new();
    let mut buffer = vec![0u8; transfer::CHUNK_SIZE];
    loop {
        let mut used = 0;
        while used < buffer.len() {
            let n = cipher
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
        let reference = chunk_ref(&pair_id, chunks.len());
        chunks.push(upload_blob(
            tool,
            root,
            target,
            temp.path(),
            &reference,
            &buffer[..used],
        )?);
    }
    if chunks.is_empty() {
        return Err("invalid_backup_pair");
    }
    if archive_remote::git(tool, root, &["rev-parse", "--verify", "HEAD"]).as_deref()
        != Ok(revision)
        || !archive_remote::git(
            tool,
            root,
            &["status", "--porcelain", "--untracked-files=all"],
        )
        .map_err(|_| "git_unavailable")?
        .is_empty()
    {
        return Err("backup_pending_git");
    }
    crate::paired_backup::publish_git_ref(tool, root, target, &pair)?;
    let metadata = GitSnapshot {
        pair: pair.clone(),
        plaintext_sha256,
        plaintext_size,
        chunks,
    };
    let json = serde_json::to_vec(&metadata).map_err(|_| "invalid_backup_pair")?;
    if json.len() > MAX_METADATA {
        return Err("file_too_large");
    }
    let encrypted_metadata = transfer::encrypt_bytes(&json, identity)?;
    // Publishing the metadata ref last makes partial uploads invisible as a complete pair.
    upload_blob(
        tool,
        root,
        target,
        temp.path(),
        &metadata_ref(&pair_id),
        &encrypted_metadata,
    )?;
    Ok(pair)
}

pub(crate) fn download(
    tool: &Path,
    root: &Path,
    target: &str,
    paths: &RuntimePaths,
    identity: &x25519::Identity,
) -> Result<(tempfile::NamedTempFile, CompletedPair), &'static str> {
    let git_refs = crate::paired_backup::remote_git_refs(tool, root, target)?;
    let output = archive_remote::git(
        tool,
        root,
        &[
            "ls-remote",
            "--tags",
            "--",
            target,
            "refs/tags/momso-state/*",
        ],
    )
    .map_err(|_| "remote_read_failed")?;
    let state_refs: std::collections::HashMap<_, _> = output
        .lines()
        .filter_map(|line| line.split_once('\t').map(|(object, name)| (name, object)))
        .collect();
    let mut metadata = Vec::new();
    for line in output.lines() {
        let Some((object, reference)) = line.split_once('\t') else {
            continue;
        };
        let Some(id) = reference
            .strip_prefix("refs/tags/momso-state/")
            .and_then(|s| s.strip_suffix("/manifest"))
        else {
            continue;
        };
        if uuid::Uuid::parse_str(id)
            .ok()
            .is_none_or(|uuid| uuid.to_string() != id)
            || !matches!(object.len(), 40 | 64)
            || !object
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            continue;
        }
        let Ok(encrypted) = fetch_blob(tool, root, target, reference, object, MAX_METADATA) else {
            continue;
        };
        let Ok(plain) = transfer::decrypt_bytes(&encrypted, identity, MAX_METADATA) else {
            continue;
        };
        let Ok(item) = serde_json::from_slice::<GitSnapshot>(&plain) else {
            continue;
        };
        if item.pair.pair_id != id
            || !item.pair.valid()
            || git_refs.get(id) != Some(&item.pair.git_revision)
            || item.chunks.is_empty()
            || item.plaintext_sha256.len() != 64
            || !item
                .plaintext_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || item.chunks.iter().any(|chunk| {
                !matches!(chunk.len(), 40 | 64)
                    || !chunk
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        {
            continue;
        }
        if !crate::paired_backup::git_ref_matches(tool, root, target, &item.pair)? {
            continue;
        }
        metadata.push(item);
    }
    metadata.sort_by(|a, b| {
        let a_time = chrono::DateTime::parse_from_rfc3339(&a.pair.created_at).expect("validated");
        let b_time = chrono::DateTime::parse_from_rfc3339(&b.pair.created_at).expect("validated");
        b_time
            .cmp(&a_time)
            .then(b.pair.pair_id.cmp(&a.pair.pair_id))
    });
    for item in metadata {
        let outcome = (|| -> Result<tempfile::NamedTempFile, &'static str> {
            let mut encrypted = tempfile::NamedTempFile::new_in(&paths.backups_path)
                .map_err(|_| "local_write_failed")?;
            for (index, object) in item.chunks.iter().enumerate() {
                let reference = chunk_ref(&item.pair.pair_id, index);
                if state_refs.get(reference.as_str()).copied() != Some(object.as_str()) {
                    return Err("remote_missing");
                }
                let bytes =
                    fetch_blob(tool, root, target, &reference, object, transfer::CHUNK_SIZE)?;
                encrypted
                    .write_all(&bytes)
                    .map_err(|_| "local_write_failed")?;
            }
            encrypted.flush().map_err(|_| "local_write_failed")?;
            let input = std::fs::File::open(encrypted.path()).map_err(|_| "local_read_failed")?;
            let decryptor = age::Decryptor::new(input).map_err(|_| "decryption_failed")?;
            let mut reader = decryptor
                .decrypt(std::iter::once(identity as &dyn age::Identity))
                .map_err(|_| "decryption_failed")?;
            let mut snapshot = tempfile::NamedTempFile::new_in(&paths.backups_path)
                .map_err(|_| "local_write_failed")?;
            let mut hasher = Sha256::new();
            let mut size = 0_u64;
            let mut buffer = [0_u8; 65536];
            loop {
                let count = reader.read(&mut buffer).map_err(|_| "decryption_failed")?;
                if count == 0 {
                    break;
                }
                size = size
                    .checked_add(count as u64)
                    .ok_or("invalid_backup_pair")?;
                if size > item.plaintext_size {
                    return Err("hash_mismatch");
                }
                hasher.update(&buffer[..count]);
                snapshot
                    .write_all(&buffer[..count])
                    .map_err(|_| "local_write_failed")?;
            }
            if size != item.plaintext_size
                || format!("{:x}", hasher.finalize()) != item.plaintext_sha256
            {
                return Err("hash_mismatch");
            }
            snapshot
                .as_file()
                .sync_all()
                .map_err(|_| "local_write_failed")?;
            storage::validate_snapshot_file(snapshot.path()).map_err(|_| "invalid_snapshot")?;
            Ok(snapshot)
        })();
        match outcome {
            Ok(snapshot) => return Ok((snapshot, item.pair)),
            Err("local_write_failed") => return Err("local_write_failed"),
            Err("local_read_failed") => return Err("local_read_failed"),
            Err(_) => (),
        }
    }
    Err("backup_pair_incomplete")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_only_encrypted_backup_restores_and_ignores_partial_newer_pair() {
        let tool = crate::git_tool::resolve()
            .map(|(tool, _)| tool)
            .expect("local Git");
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive-root");
        let data = temp.path().join("data");
        let bare = temp.path().join("remote.git");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let git =
            |directory: &Path, args: &[&str]| archive_remote::git(&tool, directory, args).unwrap();
        git(temp.path(), &["init", "--bare", bare.to_str().unwrap()]);
        git(&root, &["init"]);
        git(&root, &["config", "user.name", "Fixture"]);
        git(&root, &["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(root.join("profile.md"), "fixture").unwrap();
        git(&root, &["add", "profile.md"]);
        git(&root, &["commit", "-m", "fixture"]);
        let revision = git(&root, &["rev-parse", "HEAD"]);
        git(
            &root,
            &["push", "--", bare.to_str().unwrap(), "HEAD:refs/heads/main"],
        );
        let database_path = data.join("momso.db");
        let backups_path = data.join("backups");
        std::fs::create_dir_all(&backups_path).unwrap();
        storage::initialize(&database_path, &backups_path, "test").unwrap();
        let archive = crate::archive_location::ArchiveLocation::load(
            database_path.clone(),
            temp.path(),
            vec![],
        )
        .unwrap();
        let paths = RuntimePaths {
            data_root: data.clone(),
            database_path: database_path.clone(),
            objects_path: root.join("objects"),
            exports_path: data.join("exports"),
            backups_path: backups_path.clone(),
            logs_path: data.join("logs"),
            archive,
            archive_lease: None,
        };
        let chat = serde_json::to_value(crate::chat::create(&database_path).unwrap()).unwrap();
        let id = chat["id"].as_str().unwrap();
        let private_fixture = format!("fixture-{}", uuid::Uuid::new_v4());
        crate::chat::append(
            &database_path,
            id,
            crate::chat::Role::User,
            &private_fixture,
            None,
            None,
        )
        .unwrap();
        let identity = x25519::Identity::generate();
        let complete = capture(
            &tool,
            &root,
            bare.to_str().unwrap(),
            &revision,
            &paths,
            &identity,
        )
        .unwrap();
        let refs = git(
            &root,
            &[
                "ls-remote",
                "--tags",
                "--",
                bare.to_str().unwrap(),
                "refs/tags/momso-state/*",
            ],
        );
        for line in refs.lines() {
            let (object, _) = line.split_once('\t').unwrap();
            let bytes =
                archive_remote::git_bytes(&tool, &root, &["cat-file", "blob", object]).unwrap();
            assert!(!bytes
                .windows(private_fixture.len())
                .any(|part| part == private_fixture.as_bytes()));
        }
        crate::chat::append(
            &database_path,
            id,
            crate::chat::Role::User,
            "later fixture",
            None,
            None,
        )
        .unwrap();
        std::fs::write(root.join("profile.md"), "later fixture").unwrap();
        git(&root, &["add", "profile.md"]);
        git(&root, &["commit", "-m", "later fixture"]);
        let later_revision = git(&root, &["rev-parse", "HEAD"]);
        git(
            &root,
            &["push", "--", bare.to_str().unwrap(), "HEAD:refs/heads/main"],
        );
        let incomplete = capture(
            &tool,
            &root,
            bare.to_str().unwrap(),
            &later_revision,
            &paths,
            &identity,
        )
        .unwrap();
        git(
            &bare,
            &["update-ref", "-d", &chunk_ref(&incomplete.pair_id, 0)],
        );
        let mut partial = complete.clone();
        partial.pair_id = uuid::Uuid::new_v4().to_string();
        partial.snapshot_upload_id = partial.pair_id.clone();
        partial.created_at = "2099-01-01T00:00:00Z".into();
        crate::paired_backup::publish_git_ref(&tool, &root, bare.to_str().unwrap(), &partial)
            .unwrap();
        let (snapshot, selected) =
            download(&tool, &root, bare.to_str().unwrap(), &paths, &identity).unwrap();
        assert_eq!(selected.pair_id, complete.pair_id);
        assert_eq!(
            serde_json::to_value(crate::chat::read(snapshot.path(), id).unwrap()).unwrap()
                ["messages"][0]["text"],
            private_fixture
        );
        assert!(download(
            &tool,
            &root,
            bare.to_str().unwrap(),
            &paths,
            &x25519::Identity::generate()
        )
        .is_err());
    }
}
