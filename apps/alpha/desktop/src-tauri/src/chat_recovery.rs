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

fn git_bytes(git: &Path, root: &Path, object: &str) -> Result<Vec<u8>, String> {
    let output = crate::process::hidden_command(git)
        .args(["show", object])
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "NUL")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|_| "recovery_unavailable")?;
    if !output.status.success() {
        return Err("not_found".into());
    }
    Ok(output.stdout)
}

fn reference_parts(id: &str) -> Result<(String, PathBuf), String> {
    if crate::archive_zet::valid_zet_id(id) {
        return Ok((id.to_owned(), PathBuf::from(format!("{id}.md"))));
    }
    let path = id.strip_prefix("document-path:").ok_or("invalid_id")?;
    if path.contains('\\')
        || path.contains(':')
        || path.chars().any(char::is_control)
        || !crate::object_store_transfer::safe_relative(Path::new(path))
        || !Path::new(path)
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("md"))
    {
        return Err("invalid_id".into());
    }
    Ok((
        format!("path-{}", digest(path.as_bytes())),
        PathBuf::from(path),
    ))
}

pub(crate) fn valid_document_reference(id: &str) -> bool {
    id.starts_with("document-path:") && reference_parts(id).is_ok()
}

fn document_path(root: &Path, id: &str, create_parents: bool) -> Result<PathBuf, String> {
    let (_, relative) = reference_parts(id)?;
    let mut parent = root.join("zettels");
    if create_parents {
        match fs::create_dir(&parent) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err("recovery_unavailable".into()),
        }
    }
    let meta = fs::symlink_metadata(&parent).map_err(|_| "recovery_unavailable")?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("recovery_unavailable".into());
    }
    for part in relative.parent().ok_or("invalid_id")?.components() {
        let std::path::Component::Normal(name) = part else {
            return Err("invalid_id".into());
        };
        parent.push(name);
        if create_parents {
            match fs::create_dir(&parent) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(_) => return Err("recovery_unavailable".into()),
            }
        }
        let meta = fs::symlink_metadata(&parent).map_err(|_| "recovery_unavailable")?;
        if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
            return Err("recovery_unavailable".into());
        }
    }
    Ok(parent.join(relative.file_name().ok_or("invalid_id")?))
}

pub(crate) fn document_exists(root: &Path, id: &str) -> bool {
    document_path(root, id, false)
        .ok()
        .and_then(|path| fs::symlink_metadata(path).ok())
        .is_some_and(|meta| meta.is_file() && !crate::archive_location::is_link_or_reparse(&meta))
}

fn store(root: &Path, id: &str, bytes: &[u8]) -> Result<(), String> {
    let (key, _) = reference_parts(id)?;
    let dir = directory(root)?;
    let target = dir.join(format!("{key}-{}.md", digest(bytes)));
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
        if !crate::archive_zet::valid_zet_id(id) && !id.starts_with("document-path:") {
            continue;
        }
        let path = document_path(root, id, false)?;
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
        let relative = format!(
            "zettels/{}",
            reference_parts(id)?
                .1
                .to_str()
                .ok_or("invalid_id")?
                .replace('\\', "/")
        );
        let object = format!("HEAD:{relative}");
        let saved = git_bytes(&git, root, &object)
            .ok()
            .is_some_and(|saved| saved == bytes);
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
    let (key, _) = reference_parts(id)?;
    let dir = directory(root)?;
    let prefix = format!("{key}-");
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

pub(crate) fn restore(
    root: &Path,
    database: Option<&Path>,
    id: &str,
    hash: &str,
) -> Result<(), String> {
    let (key, _) = reference_parts(id)?;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid_id".into());
    }
    let _guard = crate::chat_file_changes::lock_app_write();
    if let Some(database) = database {
        crate::archive_remote::ensure_document_write_allowed(root, database)?;
    }
    let source = directory(root)?.join(format!("{key}-{hash}.md"));
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
    database: Option<&Path>,
    id: &str,
    revision: &str,
) -> Result<(), String> {
    let _guard = crate::chat_file_changes::lock_app_write();
    if let Some(database) = database {
        crate::archive_remote::ensure_document_write_allowed(root, database)?;
    }
    let (_, relative) = reference_parts(id)?;
    if ![40, 64].contains(&revision.len()) || !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid_id".into());
    }
    let git = crate::archive::storage_tool(resources)
        .map_err(|_| "recovery_unavailable")?
        .0;
    let object = format!(
        "{revision}:zettels/{}",
        relative.to_str().ok_or("invalid_id")?.replace('\\', "/")
    );
    let bytes = git_bytes(&git, root, &object)?;
    restore_bytes(root, id, &bytes)
}

pub(crate) fn history(root: &Path, resources: &Path, id: &str) -> Result<Vec<String>, String> {
    let (_, relative) = reference_parts(id)?;
    let path = format!(
        "zettels/{}",
        relative.to_str().ok_or("invalid_id")?.replace('\\', "/")
    );
    let git = crate::archive::storage_tool(resources)
        .map_err(|_| "recovery_unavailable")?
        .0;
    let output = crate::archive::run_storage_tool(&git, root, &["log", "--format=%H", "--", &path])
        .map_err(|_| "recovery_unavailable")?;
    output
        .lines()
        .map(|line| {
            if [40, 64].contains(&line.len()) && line.bytes().all(|b| b.is_ascii_hexdigit()) {
                Ok(line.to_owned())
            } else {
                Err("recovery_unavailable".to_owned())
            }
        })
        .collect()
}

fn restore_bytes(root: &Path, id: &str, bytes: &[u8]) -> Result<(), String> {
    let _write_guard = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "recovery_unavailable".to_string())?;
    let path = document_path(root, id, true)?;
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
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().ok_or("recovery_unavailable")?)
        .map_err(|_| "recovery_unavailable")?;
    temp.write_all(bytes).map_err(|_| "recovery_unavailable")?;
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
    fn handwritten_nested_document_is_preserved_logged_and_restored() {
        let (dir, root, _) = fixture();
        fs::create_dir(root.join("receipts")).unwrap();
        fs::create_dir(root.join("zettels/classes")).unwrap();
        let path = root.join("zettels/classes/lesson.md");
        let id = "document-path:classes/lesson.md";
        fs::write(&path, b"uncommitted lesson").unwrap();
        let db = dir.path().join("state.db");
        crate::storage::initialize(&db, &dir.path().join("backups"), "test").unwrap();
        let chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let tracking = crate::chat_file_changes::Tracking::start(&root, &chat);
        let before = capture(&root).unwrap();
        assert_eq!(
            crate::chat_file_changes::reference(&root, path.to_str().unwrap()).as_deref(),
            Some(id)
        );
        fs::write(&path, b"AI rewrite").unwrap();
        restore(&root, None, id, &digest(b"uncommitted lesson")).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"uncommitted lesson");
        fs::remove_file(&path).unwrap();
        let after = crate::chat_file_changes::capture(&root).unwrap();
        let input = crate::archive_receipt::ReceiptCreate {
            tool: "codex".into(),
            model: "model".into(),
            chat: Some(chat.clone()),
            read: vec![],
            wrote: vec![],
            prompt: String::new(),
            changes: String::new(),
        };
        assert!(crate::chat_file_changes::record_tracked(
            &root,
            &db,
            input,
            &before,
            &after,
            &std::collections::BTreeSet::new(),
            tracking
        )
        .unwrap());
        let items =
            serde_json::to_value(crate::chat_items::list(&root, &db, &chat).unwrap()).unwrap();
        assert!(items["deleted"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == id && item["how"] == "deleted"));
        let receipt_id = fs::read_dir(root.join("receipts"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_name()
            .to_string_lossy()
            .trim_end_matches(".md")
            .to_owned();
        let receipt =
            serde_json::to_value(crate::archive_receipt::read(&root, &db, &receipt_id).unwrap())
                .unwrap();
        assert_eq!(receipt["head"]["wrote"][0]["type"], "file");
        assert_eq!(receipt["head"]["wrote"][0]["missing"], true);
        fs::remove_dir(root.join("zettels/classes")).unwrap();
        fs::remove_dir(root.join("zettels")).unwrap();
        restore(&root, None, id, &digest(b"uncommitted lesson")).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"uncommitted lesson");
        assert!(
            matches!(restore(&root, None, "document-path:../escape.md", &"a".repeat(64)), Err(e) if e == "invalid_id")
        );
    }

    #[test]
    fn handwritten_git_version_is_restored_without_a_duplicate_copy() {
        let (_dir, root, git) = fixture();
        let id = "document-path:note.md";
        let path = root.join("zettels/note.md");
        fs::write(&path, b"saved \xff text\n").unwrap();
        crate::archive::run_storage_tool(
            &git,
            &root,
            &["-c", "core.autocrlf=false", "add", "zettels/note.md"],
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
        let revision =
            crate::archive::run_storage_tool(&git, &root, &["rev-parse", "HEAD"]).unwrap();
        capture(&root).unwrap();
        assert!(list(&root, id).unwrap().is_empty());
        assert_eq!(history(&root, &root, id).unwrap(), vec![revision.trim()]);
        fs::remove_file(&path).unwrap();
        restore_git(&root, &root, None, id, revision.trim()).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"saved \xff text\n");
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
        restore(&root, None, id, &hash).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"unsaved lesson");
        assert!(list(&root, id)
            .unwrap()
            .iter()
            .any(|v| v.hash == digest(b"AI rewrite")));
        fs::remove_file(&path).unwrap();
        restore(&root, None, id, &hash).unwrap();
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
        restore_git(&root, &root, None, id, revision.trim()).unwrap();
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
        restore(&root, None, id, &hash).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"lesson\r\nnext\r\n");
    }
}
