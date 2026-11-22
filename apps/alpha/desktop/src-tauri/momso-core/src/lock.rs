//! Shared interprocess lock for cooperating archive writers.
use fs2::FileExt;
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

pub struct ArchiveGuard {
    file: File,
    root: std::path::PathBuf,
}

impl Drop for ArchiveGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn open(root: &Path) -> Result<File, &'static str> {
    let meta = std::fs::symlink_metadata(root).map_err(|_| "archive_unavailable")?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("unsafe_path");
    }
    let directory = root.join(".momso");
    match std::fs::create_dir(&directory) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err("lock_unavailable"),
    }
    let meta = std::fs::symlink_metadata(&directory).map_err(|_| "lock_unavailable")?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("unsafe_path");
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
        .open(directory.join("core-write.lock"))
        .map_err(|_| "lock_unavailable")
}

pub fn exclusive(root: &Path) -> Result<ArchiveGuard, &'static str> {
    let file = open(root)?;
    file.lock_exclusive().map_err(|_| "lock_unavailable")?;
    let root = root.canonicalize().map_err(|_| "archive_unavailable")?;
    Ok(ArchiveGuard { file, root })
}

pub(crate) fn covers(guard: &ArchiveGuard, root: &Path) -> bool {
    root.canonicalize().is_ok_and(|path| path == guard.root)
}

#[cfg(test)]
pub(crate) fn try_exclusive(root: &Path) -> Result<ArchiveGuard, &'static str> {
    let file = open(root)?;
    file.try_lock_exclusive().map_err(|_| "lock_busy")?;
    let root = root.canonicalize().map_err(|_| "archive_unavailable")?;
    Ok(ArchiveGuard { file, root })
}
