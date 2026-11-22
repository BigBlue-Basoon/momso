//! Local Git history for explicitly selected momso files; no remote operations.
use std::{
    path::Path,
    process::{Command, Output, Stdio},
};

pub fn managed_path(path: &str) -> bool {
    if path.is_empty()
        || path.contains('\\')
        || path.split('/').any(|part| {
            part.is_empty() || part == "." || part == ".." || part.chars().any(char::is_control)
        })
    {
        return false;
    }
    let lower = path.to_ascii_lowercase();
    let parts: Vec<_> = lower.split('/').collect();
    if parts.contains(&".git")
        || matches!(
            parts[0],
            ".momso" | "objects" | "logs" | "credentials" | "vault"
        )
        || parts.iter().any(|part| {
            part.ends_with(".sqlite")
                || part.ends_with(".sqlite3")
                || part.ends_with(".db")
                || part.contains(".sqlite-")
                || part.contains(".sqlite3-")
                || part.contains(".db-")
                || part.ends_with(".log")
                || *part == ".env"
                || part.starts_with(".env.")
        })
    {
        return false;
    }
    if parts[0] == "chats" {
        let Some(name) = path
            .strip_prefix("chats/")
            .and_then(|value| value.strip_suffix(".json"))
        else {
            return false;
        };
        if let Some(suffix) = name.strip_prefix("chat_") {
            return uuid::Uuid::parse_str(suffix)
                .is_ok_and(|id| id.hyphenated().to_string() == suffix)
                || crate::document::valid_zet_id(&format!("zet_{suffix}"));
        }
        return false;
    }
    if matches!(parts[0], ".agents" | ".claude") {
        return [".agents/skills/", ".claude/skills/"].iter().any(|prefix| {
            path.strip_prefix(prefix)
                .and_then(|value| value.strip_suffix("/SKILL.md"))
                .is_some_and(crate::skill::valid_name)
        });
    }
    true
}

fn run(root: &Path, args: &[&str]) -> Result<Output, &'static str> {
    let mut command = Command::new("git");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
        .current_dir(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|cause| {
            eprintln!(
                "git: stage=execute file=git kind={:?} os_error={:?}",
                cause.kind(),
                cause.raw_os_error()
            );
            "git_unavailable"
        })
}

fn checked(root: &Path, args: &[&str]) -> Result<Output, &'static str> {
    let output = run(root, args)?;
    if !output.status.success() {
        eprintln!(
            "git: stage=command file=git exit_code={:?}",
            output.status.code()
        );
        return Err("git_failed");
    }
    Ok(output)
}

fn ensure_repo(root: &Path) -> Result<(), &'static str> {
    let git_dir = root.join(".git");
    match std::fs::symlink_metadata(&git_dir) {
        Ok(meta) if crate::archive_location::is_link_or_reparse(&meta) => Err("unsafe_path"),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            checked(root, &["init", "--quiet"])?;
            Ok(())
        }
        Err(_) => Err("io_error"),
    }
}

pub fn initialize(root: &Path) -> Result<(), &'static str> {
    let _file_guard = crate::lock::exclusive(root)?;
    let _guard = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "io_error")?;
    let meta = std::fs::symlink_metadata(root).map_err(|_| "io_error")?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("unsafe_path");
    }
    ensure_repo(root)
}

/// Saves only explicitly selected managed files. Other staged files stay staged.
/// The service must be the only writer to the archive during this operation.
pub fn save(root: &Path, paths: &[&str], message: &str) -> Result<bool, &'static str> {
    let _file_guard = crate::lock::exclusive(root)?;
    save_guarded(root, paths, message, &_file_guard)
}

/// The caller holds this root's process lock for the full read-and-save transaction.
pub fn save_guarded(
    root: &Path,
    paths: &[&str],
    message: &str,
    guard: &crate::lock::ArchiveGuard,
) -> Result<bool, &'static str> {
    if !crate::lock::covers(guard, root) {
        return Err("invalid_lock");
    }
    let _guard = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "io_error")?;
    if paths.is_empty() || paths.iter().any(|path| !managed_path(path)) {
        return Err("invalid_path");
    }
    if message.trim().is_empty() || message.chars().any(char::is_control) {
        return Err("invalid_message");
    }
    let meta = std::fs::symlink_metadata(root).map_err(|_| "io_error")?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("unsafe_path");
    }
    for path in paths {
        let mut current = root.to_path_buf();
        for component in path.split('/') {
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(meta) if crate::archive_location::is_link_or_reparse(&meta) => {
                    return Err("unsafe_path")
                }
                Ok(_) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(_) => return Err("io_error"),
            }
        }
    }
    ensure_repo(root)?;
    let literal: Vec<String> = paths
        .iter()
        .map(|path| format!(":(literal){path}"))
        .collect();
    let literal_refs: Vec<&str> = literal.iter().map(String::as_str).collect();
    let mut add = vec!["add", "-A", "--"];
    add.extend_from_slice(&literal_refs);
    checked(root, &add)?;
    let mut diff = vec!["diff", "--cached", "--quiet", "--"];
    diff.extend_from_slice(&literal_refs);
    let output = run(root, &diff)?;
    match output.status.code() {
        Some(0) => return Ok(false),
        Some(1) => (),
        _ => return Err("git_failed"),
    }
    let mut commit = vec![
        "-c",
        "user.name=momso",
        "-c",
        "user.email=archive@momso.local",
        "commit",
        "--only",
        "-m",
        message,
        "--",
    ];
    commit.extend_from_slice(&literal_refs);
    checked(root, &commit)?;
    Ok(true)
}

/// Returns changed managed text files without reading or including private data.
pub fn changed_paths(root: &Path) -> Result<Vec<String>, &'static str> {
    let _file_guard = crate::lock::exclusive(root)?;
    changed_paths_guarded(root, &_file_guard)
}

pub fn changed_paths_guarded(
    root: &Path,
    guard: &crate::lock::ArchiveGuard,
) -> Result<Vec<String>, &'static str> {
    if !crate::lock::covers(guard, root) {
        return Err("invalid_lock");
    }
    let _guard = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "io_error")?;
    ensure_repo(root)?;
    let output = checked(
        root,
        &[
            "ls-files",
            "-z",
            "--modified",
            "--deleted",
            "--others",
            "--exclude-standard",
            "--",
            ".",
        ],
    )?;
    let staged = checked(root, &["diff", "--cached", "--name-only", "-z", "--", "."])?;
    let mut paths = std::collections::BTreeSet::new();
    for raw in output
        .stdout
        .split(|byte| *byte == 0)
        .chain(staged.stdout.split(|byte| *byte == 0))
        .filter(|path| !path.is_empty())
    {
        let path = std::str::from_utf8(raw).map_err(|_| "invalid_path")?;
        if managed_path(path) {
            paths.insert(path.to_owned());
        }
    }
    Ok(paths.into_iter().collect())
}
