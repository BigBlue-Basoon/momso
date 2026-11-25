//! Crash-safe movement of a clean, newly received Git checkout to a completed pair.
use crate::{archive_remote, runtime::RuntimePaths, storage};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::Path};

const JOURNAL: &str = "paired-restore-v1.json";

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Journal {
    version: u32,
    id: String,
    old_head: String,
    safety_revision: String,
    target: String,
}

fn valid_revision(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn safety_path(paths: &RuntimePaths, id: &str) -> Result<std::path::PathBuf, &'static str> {
    let parsed = uuid::Uuid::parse_str(id).map_err(|_| "restore_recovery_invalid")?;
    if parsed.to_string() != id {
        return Err("restore_recovery_invalid");
    }
    Ok(paths.backups_path.join(format!("paired-restore-{id}.db")))
}

fn tracked_paths_are_safe(tool: &Path, root: &Path, revision: &str) -> Result<(), &'static str> {
    archive_remote::safe_history(tool, root, revision)?;
    let paths = archive_remote::git(
        tool,
        root,
        &["ls-tree", "-r", "--name-only", "-z", revision],
    )
    .map_err(|_| "git_unavailable")?;
    for relative in paths.split('\0').filter(|part| !part.is_empty()) {
        let path = Path::new(relative);
        if !crate::safe_path::safe_relative(path) {
            return Err("unsafe_path");
        }
        let mut current = root.to_path_buf();
        for part in path.components() {
            current.push(part.as_os_str());
            match fs::symlink_metadata(&current) {
                Ok(meta) if crate::archive_location::is_link_or_reparse(&meta) => {
                    return Err("unsafe_path")
                }
                Ok(_) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(_) => return Err("local_read_failed"),
            }
        }
        if root.join(path).exists()
            && archive_remote::git(tool, root, &["ls-files", "--error-unmatch", "--", relative])
                .is_err()
        {
            return Err("local_data_conflict");
        }
    }
    Ok(())
}

pub(crate) fn begin(
    tool: &Path,
    root: &Path,
    paths: &RuntimePaths,
    old_head: &str,
    target: &str,
    preserve_dirty: bool,
) -> Result<Journal, &'static str> {
    if !valid_revision(old_head) || !valid_revision(target) {
        return Err("restore_recovery_invalid");
    }
    if paths.data_root.join(JOURNAL).exists() {
        return Err("restore_recovery_pending");
    }
    if archive_remote::git(tool, root, &["rev-parse", "--verify", "HEAD"]).as_deref()
        != Ok(old_head)
    {
        return Err("local_data_conflict");
    }
    let dirty = !archive_remote::git(
        tool,
        root,
        &["status", "--porcelain", "--untracked-files=all"],
    )
    .map_err(|_| "git_unavailable")?
    .is_empty();
    if dirty && !preserve_dirty {
        return Err("local_data_conflict");
    }
    if !preserve_dirty {
        archive_remote::git(
            tool,
            root,
            &["merge-base", "--is-ancestor", target, old_head],
        )
        .map_err(|_| "backup_pair_unrelated")?;
    }
    tracked_paths_are_safe(tool, root, target)?;
    fs::create_dir_all(&paths.backups_path).map_err(|_| "local_write_failed")?;
    let metadata = fs::symlink_metadata(&paths.backups_path).map_err(|_| "local_read_failed")?;
    if !metadata.is_dir() || crate::archive_location::is_link_or_reparse(&metadata) {
        return Err("unsafe_path");
    }
    let id = uuid::Uuid::new_v4().to_string();
    let safety_revision = if dirty {
        let saved = archive_remote::git(
            tool,
            root,
            &[
                "-c",
                "user.name=momso",
                "-c",
                "user.email=archive@momso.local",
                "stash",
                "create",
                "momso restore safety",
            ],
        )
        .map_err(|_| "restore_recovery_failed")?;
        if saved.is_empty() {
            old_head.to_owned()
        } else {
            saved
        }
    } else {
        old_head.to_owned()
    };
    if !valid_revision(&safety_revision)
        || archive_remote::git(
            tool,
            root,
            &["diff", "--quiet", &safety_revision, "--", "."],
        )
        .is_err()
    {
        return Err("local_data_conflict");
    }
    let safety_ref = format!("refs/momso/restore-safety/{id}");
    archive_remote::git(tool, root, &["update-ref", &safety_ref, &safety_revision])
        .map_err(|_| "restore_recovery_failed")?;
    let safety = safety_path(paths, &id)?;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&safety)
        .map_err(|_| "local_write_failed")?;
    storage::create_snapshot_file(&paths.database_path, &safety).map_err(|_| "restore_failed")?;
    let journal = Journal {
        version: 1,
        id,
        old_head: old_head.into(),
        safety_revision,
        target: target.into(),
    };
    let mut temp =
        tempfile::NamedTempFile::new_in(&paths.data_root).map_err(|_| "local_write_failed")?;
    serde_json::to_writer(&mut temp, &journal).map_err(|_| "local_write_failed")?;
    temp.flush().map_err(|_| "local_write_failed")?;
    temp.as_file()
        .sync_all()
        .map_err(|_| "local_write_failed")?;
    temp.persist_noclobber(paths.data_root.join(JOURNAL))
        .map_err(|_| "restore_recovery_pending")?;
    let source = format!("--source={target}");
    if archive_remote::git(
        tool,
        root,
        &["restore", &source, "--staged", "--worktree", "--", "."],
    )
    .is_err()
    {
        rollback(tool, root, paths, &journal)?;
        return Err("backup_git_restore_failed");
    }
    Ok(journal)
}

pub(crate) fn finish(paths: &RuntimePaths) -> Result<(), &'static str> {
    fs::remove_file(paths.data_root.join(JOURNAL)).map_err(|_| "restore_recovery_pending")
}

pub(crate) fn rollback(
    tool: &Path,
    root: &Path,
    paths: &RuntimePaths,
    journal: &Journal,
) -> Result<(), &'static str> {
    if journal.version != 1
        || !valid_revision(&journal.old_head)
        || !valid_revision(&journal.safety_revision)
        || !valid_revision(&journal.target)
        || archive_remote::git(tool, root, &["rev-parse", "--verify", "HEAD"]).as_deref()
            != Ok(journal.old_head.as_str())
    {
        return Err("restore_recovery_conflict");
    }
    let changed = archive_remote::git(
        tool,
        root,
        &["diff", "--name-only", "-z", &journal.old_head, "--", "."],
    )
    .map_err(|_| "restore_recovery_conflict")?;
    for path in changed.split('\0').filter(|part| !part.is_empty()) {
        if !crate::safe_path::safe_relative(Path::new(path)) {
            return Err("restore_recovery_conflict");
        }
        let old = archive_remote::git(
            tool,
            root,
            &["diff", "--quiet", &journal.safety_revision, "--", path],
        )
        .is_ok();
        let target = archive_remote::git(
            tool,
            root,
            &["diff", "--quiet", &journal.target, "--", path],
        )
        .is_ok();
        if !old && !target {
            return Err("restore_recovery_conflict");
        }
    }
    tracked_paths_are_safe(tool, root, &journal.safety_revision)?;
    let source = format!("--source={}", journal.safety_revision);
    archive_remote::git(
        tool,
        root,
        &["restore", &source, "--staged", "--worktree", "--", "."],
    )
    .map_err(|_| "restore_recovery_conflict")?;
    let safety = safety_path(paths, &journal.id)?;
    let meta = fs::symlink_metadata(&safety).map_err(|_| "restore_recovery_invalid")?;
    if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("restore_recovery_invalid");
    }
    storage::recover_exact_safety_snapshot(&safety, &paths.database_path)
        .map_err(|_| "restore_recovery_failed")?;
    finish(paths)
}

pub(crate) fn recover_if_needed(paths: &RuntimePaths) -> Result<(), &'static str> {
    let journal_path = paths.data_root.join(JOURNAL);
    let meta = match fs::symlink_metadata(&journal_path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("restore_recovery_invalid"),
    };
    if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) || meta.len() > 4096 {
        return Err("restore_recovery_invalid");
    }
    let journal: Journal =
        serde_json::from_slice(&fs::read(journal_path).map_err(|_| "restore_recovery_invalid")?)
            .map_err(|_| "restore_recovery_invalid")?;
    crate::git_tool::configure(paths.data_root.clone());
    let tool = crate::git_tool::resolve()
        .map(|(path, _)| path)
        .ok_or("git_missing")?;
    let root = paths
        .objects_path
        .parent()
        .ok_or("restore_recovery_invalid")?;
    rollback(&tool, root, paths, &journal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_previous_pair_restore_recovers_git_and_database() {
        let tool = crate::git_tool::resolve()
            .map(|(tool, _)| tool)
            .expect("local Git");
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("data");
        let root = temp.path().join("archive-root");
        fs::create_dir_all(&data_root).unwrap();
        fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| archive_remote::git(&tool, &root, args).unwrap();
        git(&["init"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        fs::write(root.join("profile.md"), "older").unwrap();
        git(&["add", "profile.md"]);
        git(&["commit", "-m", "older"]);
        let older = git(&["rev-parse", "HEAD"]);
        fs::write(root.join("profile.md"), "newer").unwrap();
        git(&["add", "profile.md"]);
        git(&["commit", "-m", "newer"]);
        let newer = git(&["rev-parse", "HEAD"]);
        let database_path = data_root.join("momso.db");
        let backups_path = data_root.join("backups");
        storage::initialize(&database_path, &backups_path, "test").unwrap();
        let archive = crate::archive_location::ArchiveLocation::load(
            database_path.clone(),
            temp.path(),
            vec![],
        )
        .unwrap();
        let paths = RuntimePaths {
            data_root: data_root.clone(),
            database_path: database_path.clone(),
            objects_path: root.join("objects"),
            exports_path: data_root.join("exports"),
            backups_path,
            logs_path: data_root.join("logs"),
            archive,
            archive_lease: None,
        };
        let chat = serde_json::to_value(crate::chat::create(&database_path).unwrap()).unwrap();
        let id = chat["id"].as_str().unwrap();
        crate::chat::append(
            &database_path,
            id,
            crate::chat::Role::User,
            "saved local chat",
            None,
            None,
        )
        .unwrap();
        let journal = begin(&tool, &root, &paths, &newer, &older, false).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("profile.md")).unwrap(),
            "older"
        );
        fs::write(root.join("profile.md"), "external change").unwrap();
        assert_eq!(
            rollback(&tool, &root, &paths, &journal),
            Err("restore_recovery_conflict")
        );
        assert_eq!(
            fs::read_to_string(root.join("profile.md")).unwrap(),
            "external change"
        );
        fs::write(root.join("profile.md"), "older").unwrap();
        let db = storage::open_writable_connection(&database_path).unwrap();
        db.execute(
            "UPDATE chat_messages SET text='changed during restore' WHERE chat_id=?1",
            [id],
        )
        .unwrap();
        drop(db);
        rollback(&tool, &root, &paths, &journal).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("profile.md")).unwrap(),
            "newer"
        );
        assert_eq!(
            serde_json::to_value(crate::chat::read(&database_path, id).unwrap()).unwrap()
                ["messages"][0]["text"],
            "saved local chat"
        );
        assert!(!data_root.join(JOURNAL).exists());
        fs::write(root.join("profile.md"), "local change").unwrap();
        assert!(matches!(
            begin(&tool, &root, &paths, &newer, &older, false),
            Err("local_data_conflict")
        ));
        let journal = begin(&tool, &root, &paths, &newer, &older, true).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("profile.md")).unwrap(),
            "older"
        );
        rollback(&tool, &root, &paths, &journal).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("profile.md")).unwrap(),
            "local change"
        );
        git(&["add", "profile.md"]);
        git(&["commit", "-m", "local change"]);
        let unrelated = git(&["commit-tree", "HEAD^{tree}", "-m", "unrelated fixture"]);
        git(&["reset", "--hard", &unrelated]);
        assert!(matches!(
            begin(&tool, &root, &paths, &unrelated, &older, false),
            Err("backup_pair_unrelated")
        ));
        let journal = begin(&tool, &root, &paths, &unrelated, &older, true).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("profile.md")).unwrap(),
            "older"
        );
        rollback(&tool, &root, &paths, &journal).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("profile.md")).unwrap(),
            "local change"
        );
    }
}
