//! First receive: copy remote tracked text without overwriting any local file.
use std::{
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

type RestoredFiles = Vec<(String, Vec<u8>)>;

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, &'static str> {
    let output = Command::new("git")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "git_unavailable")?;
    if !output.status.success() {
        return Err("restore_invalid_history");
    }
    Ok(output.stdout)
}

fn safe_parent(root: &Path, relative: &Path, create: bool) -> Result<PathBuf, &'static str> {
    let mut current = root.to_owned();
    for item in relative.components() {
        let Component::Normal(part) = item else {
            return Err("restore_unsafe_path");
        };
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_dir() => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                std::fs::create_dir(&current).map_err(|_| "storage_unavailable")?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            _ => return Err("restore_unsafe_path"),
        }
    }
    Ok(current)
}

pub fn prepare(source: &Path, target: &Path) -> Result<(String, RestoredFiles), &'static str> {
    let head = String::from_utf8(git(source, &["rev-parse", "HEAD"])?)
        .map_err(|_| "restore_invalid_history")?;
    let head = head.trim();
    if head.len() != 40 || !head.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("restore_invalid_history");
    }
    let names = git(source, &["ls-files", "-z"])?;
    let mut files = Vec::new();
    for raw in names.split(|c| *c == 0).filter(|x| !x.is_empty()) {
        let name = std::str::from_utf8(raw).map_err(|_| "restore_unsafe_path")?;
        if !momso_core::git::managed_path(name) {
            return Err("restore_unsupported_file");
        }
        if name.starts_with("chats/") {
            continue;
        }
        let relative = Path::new(name);
        let parent = relative.parent().ok_or("restore_unsafe_path")?;
        let source_parent = safe_parent(source, parent, false)?;
        let bytes = super::chat_backup::read_regular(
            &source_parent.join(relative.file_name().ok_or("restore_unsafe_path")?),
        )?
        .ok_or("restore_unsafe_path")?;
        if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
            return Err("restore_unsupported_file");
        }
        let target_parent = safe_parent(target, parent, false)?;
        match super::chat_backup::read_regular(
            &target_parent.join(relative.file_name().ok_or("restore_unsafe_path")?),
        )? {
            Some(existing) if existing != bytes => return Err("restore_conflict"),
            _ => (),
        }
        files.push((name.to_owned(), bytes));
    }
    Ok((head.to_owned(), files))
}

pub fn copy(target: &Path, files: &[(String, Vec<u8>)]) -> Result<(), &'static str> {
    let _guard = momso_core::lock::exclusive(target)?;
    for (name, bytes) in files {
        let relative = Path::new(name);
        let parent = safe_parent(
            target,
            relative.parent().ok_or("restore_unsafe_path")?,
            true,
        )?;
        let path = parent.join(relative.file_name().ok_or("restore_unsafe_path")?);
        if let Some(existing) = super::chat_backup::read_regular(&path)? {
            if existing != *bytes {
                return Err("restore_conflict");
            }
            continue;
        }
        let mut tmp =
            tempfile::NamedTempFile::new_in(&parent).map_err(|_| "storage_unavailable")?;
        tmp.write_all(bytes)
            .and_then(|()| tmp.as_file().sync_all())
            .map_err(|_| "storage_unavailable")?;
        tmp.persist_noclobber(path)
            .map_err(|_| "restore_conflict")?;
        std::fs::File::open(parent)
            .and_then(|dir| dir.sync_all())
            .map_err(|_| "storage_unavailable")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracked_source(root: &Path) {
        std::fs::create_dir_all(root.join("zettels")).unwrap();
        std::fs::write(root.join("zettels/example.md"), "remote").unwrap();
        for args in [
            vec!["init", "--quiet"],
            vec!["add", "zettels/example.md"],
            vec![
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@invalid",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ],
        ] {
            assert!(Command::new("git")
                .current_dir(root)
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args(args)
                .status()
                .unwrap()
                .success());
        }
    }

    #[test]
    fn restore_never_overwrites_different_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        tracked_source(&source);
        std::fs::create_dir_all(target.join("zettels")).unwrap();
        let file = target.join("zettels/example.md");
        std::fs::write(&file, "local").unwrap();
        assert_eq!(prepare(&source, &target), Err("restore_conflict"));
        assert_eq!(std::fs::read_to_string(file).unwrap(), "local");
    }

    #[test]
    fn file_created_after_preflight_is_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        tracked_source(&source);
        momso_core::filesystem::initialize(&target).unwrap();
        let (_, files) = prepare(&source, &target).unwrap();
        let file = target.join("zettels/example.md");
        std::fs::write(&file, "created after preflight").unwrap();
        assert_eq!(copy(&target, &files), Err("restore_conflict"));
        assert_eq!(
            std::fs::read_to_string(file).unwrap(),
            "created after preflight"
        );
    }

    #[test]
    fn remote_symlink_is_not_read() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        tracked_source(&source);
        std::fs::create_dir_all(&target).unwrap();
        std::fs::remove_file(source.join("zettels/example.md")).unwrap();
        symlink("/etc/passwd", source.join("zettels/example.md")).unwrap();
        assert!(prepare(&source, &target).is_err());
        assert!(!target.join("zettels/example.md").exists());
    }
}
