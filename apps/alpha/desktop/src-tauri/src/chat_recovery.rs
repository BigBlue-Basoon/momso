use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

// Copies are intentionally kept until the user removes them. If the quota is
// reached, fail before starting the AI instead of silently deleting a preimage.
const MAX_BYTES: u64 = 256 * 1024 * 1024;

fn directory(root: &Path) -> Result<PathBuf, String> {
    let git = root.join(".git");
    let meta = fs::symlink_metadata(&git).map_err(|_| "recovery_unavailable")?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("recovery_unavailable".into());
    }
    let dir = git.join("momso-ai-recovery");
    match fs::create_dir(&dir) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err("recovery_unavailable".into()),
    }
    let meta = fs::symlink_metadata(&dir).map_err(|_| "recovery_unavailable")?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("recovery_unavailable".into());
    }
    Ok(dir)
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn store(root: &Path, id: &str, bytes: &[u8]) -> Result<(), String> {
    if !crate::archive_zet::valid_zet_id(id) {
        return Err("invalid_id".into());
    }
    let dir = directory(root)?;
    let target = dir.join(format!("{id}-{}.md", digest(bytes)));
    if target.exists() {
        let meta = fs::symlink_metadata(&target).map_err(|_| "recovery_unavailable")?;
        if !meta.is_file()
            || crate::archive_location::is_link_or_reparse(&meta)
            || meta.len() != bytes.len() as u64
            || fs::read(&target).map_err(|_| "recovery_unavailable")? != bytes
        {
            return Err("recovery_conflict".into());
        }
        return Ok(());
    }
    let mut used = 0u64;
    for entry in fs::read_dir(&dir).map_err(|_| "recovery_unavailable")? {
        let entry = entry.map_err(|_| "recovery_unavailable")?;
        let meta = fs::symlink_metadata(entry.path()).map_err(|_| "recovery_unavailable")?;
        if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
            return Err("recovery_unavailable".into());
        }
        used = used.checked_add(meta.len()).ok_or("recovery_full")?;
    }
    if used.saturating_add(bytes.len() as u64) > MAX_BYTES {
        return Err("recovery_full".into());
    }
    let mut temp = tempfile::NamedTempFile::new_in(&dir).map_err(|_| "recovery_unavailable")?;
    temp.write_all(bytes).map_err(|_| "recovery_unavailable")?;
    temp.as_file()
        .sync_all()
        .map_err(|_| "recovery_unavailable")?;
    temp.persist_noclobber(target)
        .map_err(|_| "recovery_unavailable")?;
    Ok(())
}

pub(crate) fn capture(root: &Path) -> Result<crate::chat_file_changes::Snapshot, String> {
    let _guard = crate::chat_file_changes::lock_app_write();
    let before = crate::chat_file_changes::capture_unlocked(root)?;
    let (git, _) = crate::archive::storage_tool(root).map_err(|_| "recovery_unavailable")?;
    for (id, hash) in &before {
        if !crate::archive_zet::valid_zet_id(id) {
            continue;
        }
        let path = root.join("zettels").join(format!("{id}.md"));
        let meta = fs::symlink_metadata(&path).map_err(|_| "recovery_unavailable")?;
        if !meta.is_file()
            || crate::archive_location::is_link_or_reparse(&meta)
            || meta.len() > MAX_BYTES
        {
            return Err("recovery_full".into());
        }
        let bytes = fs::read(path).map_err(|_| "recovery_unavailable")?;
        let actual: [u8; 32] = Sha256::digest(&bytes).into();
        if &actual != hash {
            return Err("recovery_changed".into());
        }
        let relative = format!("zettels/{id}.md");
        let object = format!("HEAD:{relative}");
        let saved = crate::archive::run_storage_tool(&git, root, &["show", &object])
            .ok()
            .is_some_and(|text| text.as_bytes() == bytes);
        let mut current = fs::File::open(root.join(&relative)).map_err(|_| "recovery_changed")?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            let size = current.read(&mut buffer).map_err(|_| "recovery_changed")?;
            if size == 0 {
                break;
            }
            hasher.update(&buffer[..size]);
        }
        let actual: [u8; 32] = hasher.finalize().into();
        if &actual != hash {
            return Err("recovery_changed".into());
        }
        if !saved {
            store(root, id, &bytes)?;
        }
    }
    Ok(before)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecoveryCopy {
    id: String,
    hash: String,
    captured_at: Option<String>,
}

pub(crate) fn list(root: &Path, id: &str) -> Result<Vec<RecoveryCopy>, String> {
    if !crate::archive_zet::valid_zet_id(id) {
        return Err("invalid_id".into());
    }
    let dir = directory(root)?;
    let prefix = format!("{id}-");
    let mut result = Vec::new();
    for entry in fs::read_dir(dir).map_err(|_| "recovery_unavailable")? {
        let entry = entry.map_err(|_| "recovery_unavailable")?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(hash) = name
            .strip_prefix(&prefix)
            .and_then(|s| s.strip_suffix(".md"))
        else {
            continue;
        };
        if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            let meta = fs::symlink_metadata(entry.path()).map_err(|_| "recovery_unavailable")?;
            if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
                return Err("recovery_unavailable".into());
            }
            let captured_at = meta
                .modified()
                .ok()
                .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
            result.push(RecoveryCopy {
                id: id.into(),
                hash: hash.into(),
                captured_at,
            });
        }
    }
    result.sort_by(|a, b| {
        b.captured_at
            .cmp(&a.captured_at)
            .then_with(|| a.hash.cmp(&b.hash))
    });
    Ok(result)
}

pub(crate) fn restore(root: &Path, id: &str, hash: &str) -> Result<(), String> {
    if !crate::archive_zet::valid_zet_id(id)
        || hash.len() != 64
        || !hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("invalid_id".into());
    }
    let _guard = crate::chat_file_changes::lock_app_write();
    let source = directory(root)?.join(format!("{id}-{hash}.md"));
    let meta = fs::symlink_metadata(&source).map_err(|_| "not_found")?;
    if !meta.is_file()
        || crate::archive_location::is_link_or_reparse(&meta)
        || meta.len() > MAX_BYTES
    {
        return Err("recovery_unavailable".into());
    }
    let bytes = fs::read(source).map_err(|_| "recovery_unavailable")?;
    if digest(&bytes) != hash {
        return Err("recovery_corrupt".into());
    }
    restore_bytes(root, id, &bytes)
}

pub(crate) fn restore_git(
    root: &Path,
    resources: &Path,
    id: &str,
    revision: &str,
) -> Result<(), String> {
    let _guard = crate::chat_file_changes::lock_app_write();
    let bytes =
        crate::archive::read_version(root, resources, id, revision).map_err(|_| "not_found")?;
    restore_bytes(root, id, bytes.as_bytes())
}

fn restore_bytes(root: &Path, id: &str, bytes: &[u8]) -> Result<(), String> {
    let parent = root.join("zettels");
    let parent_meta = fs::symlink_metadata(&parent).map_err(|_| "recovery_unavailable")?;
    if !parent_meta.is_dir() || crate::archive_location::is_link_or_reparse(&parent_meta) {
        return Err("recovery_unavailable".into());
    }
    let path = root.join("zettels").join(format!("{id}.md"));
    match fs::symlink_metadata(&path) {
        Ok(meta)
            if meta.is_file()
                && !crate::archive_location::is_link_or_reparse(&meta)
                && meta.len() <= MAX_BYTES =>
        {
            store(
                root,
                id,
                &fs::read(&path).map_err(|_| "recovery_unavailable")?,
            )?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        _ => return Err("recovery_conflict".into()),
    }
    let mut temp = tempfile::NamedTempFile::new_in(root.join("zettels"))
        .map_err(|_| "recovery_unavailable")?;
    temp.write_all(&bytes).map_err(|_| "recovery_unavailable")?;
    temp.as_file()
        .sync_all()
        .map_err(|_| "recovery_unavailable")?;
    temp.persist(path).map_err(|_| "recovery_unavailable")?;
    crate::chat_file_changes::app_write(root, Some(id));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        fs::create_dir_all(root.join("zettels")).unwrap();
        fs::create_dir_all(root.join("objects")).unwrap();
        let (git, _) = crate::archive::storage_tool(&root).unwrap();
        crate::archive::run_storage_tool(&git, &root, &["init"]).unwrap();
        (dir, root, git)
    }

    #[test]
    fn unsaved_edit_and_deletion_restore_original_without_losing_replacement() {
        let (_dir, root, _) = fixture();
        let id = "zet_20261115_120000";
        let path = root.join("zettels").join(format!("{id}.md"));
        fs::write(&path, b"unsaved lesson").unwrap();
        capture(&root).unwrap();
        let hash = digest(b"unsaved lesson");
        fs::write(&path, b"AI rewrite").unwrap();
        restore(&root, id, &hash).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"unsaved lesson");
        assert!(list(&root, id)
            .unwrap()
            .iter()
            .any(|v| v.hash == digest(b"AI rewrite")));
        fs::remove_file(&path).unwrap();
        restore(&root, id, &hash).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"unsaved lesson");
    }

    #[test]
    fn saved_version_is_not_duplicated_and_can_be_restored_after_deletion() {
        let (_dir, root, git) = fixture();
        let id = "zet_20261115_120000";
        let path = root.join("zettels").join(format!("{id}.md"));
        fs::write(&path, b"saved lesson\n").unwrap();
        crate::archive::run_storage_tool(&git, &root, &["add", "zettels"]).unwrap();
        crate::archive::run_storage_tool(
            &git,
            &root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "fixture",
            ],
        )
        .unwrap();
        let revision =
            crate::archive::run_storage_tool(&git, &root, &["rev-parse", "HEAD"]).unwrap();
        capture(&root).unwrap();
        assert!(list(&root, id).unwrap().is_empty());
        crate::archive::run_storage_tool(
            &git,
            &root,
            &[
                "update-index",
                "--skip-worktree",
                "zettels/zet_20261115_120000.md",
            ],
        )
        .unwrap();
        fs::write(&path, b"changed but ignored by git status").unwrap();
        capture(&root).unwrap();
        assert!(list(&root, id)
            .unwrap()
            .iter()
            .any(|v| v.hash == digest(b"changed but ignored by git status")));
        fs::remove_file(&path).unwrap();
        restore_git(&root, &root, id, revision.trim()).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"saved lesson\n");
    }

    #[test]
    fn preserves_worktree_line_endings_when_git_has_different_bytes() {
        let (_dir, root, git) = fixture();
        let id = "zet_20261115_120000";
        let path = root.join("zettels").join(format!("{id}.md"));
        fs::write(&path, b"lesson\nnext\n").unwrap();
        crate::archive::run_storage_tool(
            &git,
            &root,
            &["-c", "core.autocrlf=false", "add", "zettels"],
        )
        .unwrap();
        crate::archive::run_storage_tool(
            &git,
            &root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "fixture",
            ],
        )
        .unwrap();
        fs::write(&path, b"lesson\r\nnext\r\n").unwrap();
        capture(&root).unwrap();
        let hash = digest(b"lesson\r\nnext\r\n");
        assert!(list(&root, id).unwrap().iter().any(|v| v.hash == hash));
        fs::remove_file(&path).unwrap();
        restore(&root, id, &hash).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"lesson\r\nnext\r\n");
    }
}
