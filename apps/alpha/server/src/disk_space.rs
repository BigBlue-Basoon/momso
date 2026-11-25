//! Reject growth before the filesystem reserve is exhausted.
use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::Path,
};

pub const RESERVE: u64 = 1536 * 1024 * 1024;
const STATE_LOCK: &str = ".momso-reservation-state.lock";
const OPERATION_LOCK: &str = ".momso-reservation-operation.lock";

pub fn initialize(root: &Path) -> Result<(), &'static str> {
    open_lock(root, STATE_LOCK)?;
    open_lock(root, OPERATION_LOCK)?;
    Ok(())
}

fn open_lock(root: &Path, name: &str) -> Result<File, &'static str> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root.join(name))
        .map_err(|_| "reservation_unavailable")?;
    let meta = file.metadata().map_err(|_| "reservation_unavailable")?;
    let owner = std::fs::metadata(root).map_err(|_| "reservation_unavailable")?;
    if !meta.is_file() || meta.nlink() != 1 || meta.uid() != owner.uid() || meta.mode() & 0o077 != 0
    {
        return Err("reservation_unavailable");
    }
    Ok(file)
}

fn lock(file: &File) -> Result<(), &'static str> {
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock {
                "reservation_busy"
            } else {
                "reservation_unavailable"
            },
        );
    }
    Ok(())
}

/// One large write at a time across all cooperating processes. Dropping the
/// guard releases the kernel lock even if the process crashes.
pub struct Reservation {
    _operation: File,
}

pub fn reserve(root: &Path, requested_bytes: u64) -> Result<Reservation, &'static str> {
    let state = open_lock(root, STATE_LOCK)?;
    lock(&state)?;
    let mut operation = open_lock(root, OPERATION_LOCK)?;
    lock(&operation)?;
    let (_, free) = usage(root).ok_or("reservation_unavailable")?;
    if !enough(free, requested_bytes) {
        return Err("disk_low");
    }
    operation
        .set_len(0)
        .map_err(|_| "reservation_unavailable")?;
    operation
        .seek(SeekFrom::Start(0))
        .map_err(|_| "reservation_unavailable")?;
    writeln!(operation, "{requested_bytes}").map_err(|_| "reservation_unavailable")?;
    operation
        .sync_data()
        .map_err(|_| "reservation_unavailable")?;
    Ok(Reservation {
        _operation: operation,
    })
}

pub fn active_reserved(root: &Path) -> Option<u64> {
    let state = open_lock(root, STATE_LOCK).ok()?;
    lock(&state).ok()?;
    let mut operation = open_lock(root, OPERATION_LOCK).ok()?;
    match lock(&operation) {
        Ok(()) => return Some(0),
        Err("reservation_busy") => (),
        Err(_) => return None,
    }
    let mut value = String::new();
    (&mut operation).take(32).read_to_string(&mut value).ok()?;
    value.trim().parse().ok()
}

pub fn usage(path: &Path) -> Option<(u64, u64)> {
    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return None;
    }
    let stat = unsafe { stat.assume_init() };
    Some((
        stat.f_blocks.saturating_mul(stat.f_frsize),
        stat.f_bavail.saturating_mul(stat.f_frsize),
    ))
}

fn enough(free: u64, additional: u64) -> bool {
    free >= RESERVE.saturating_add(additional)
}

pub fn can_grow(path: &Path, additional: u64) -> bool {
    usage(path)
        .zip(active_reserved(path))
        .is_some_and(|((_, free), active)| enough(free, additional.saturating_add(active)))
}

pub fn legacy_copies(root: &Path) -> Option<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(root).ok()? {
        let entry = entry.ok()?;
        let name = entry.file_name().into_string().ok()?;
        if (name.starts_with("momso-server.") || name.starts_with("web."))
            && (name.ends_with("-failed") || name.ends_with("-ok"))
        {
            names.push(name);
            if names.len() > 200 {
                return None;
            }
        }
    }
    names.sort();
    Some(names)
}

pub fn last_cleanup(root: &Path) -> Option<String> {
    let raw = std::fs::read(root.join("cleanup-status.json")).ok()?;
    if raw.len() > 512 {
        return None;
    }
    let status = serde_json::from_slice::<serde_json::Value>(&raw).ok()?;
    let time = status["lastCleanupAt"].as_str()?;
    if time.len() > 64 || chrono::DateTime::parse_from_rfc3339(time).is_err() {
        return None;
    }
    Some(time.to_owned())
}

pub fn directory_bytes(root: &Path) -> Option<u64> {
    let mut pending = vec![root.to_path_buf()];
    let mut total = 0_u64;
    let mut visited = 0_usize;
    while let Some(path) = pending.pop() {
        visited += 1;
        if visited > 10_000 {
            return None;
        }
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if meta.is_file() {
            total = total.saturating_add(meta.len());
        }
        if meta.is_dir() {
            for entry in std::fs::read_dir(path).ok()? {
                if visited.saturating_add(pending.len()) >= 10_000 {
                    return None;
                }
                pending.push(entry.ok()?.path());
            }
        }
    }
    Some(total)
}

/// Estimate Git's temporary data and chat exports, excluding separately backed-up originals.
pub fn github_backup_reservation(data: &Path) -> Option<u64> {
    let archive = data.join("archive");
    let mut pending = vec![archive.clone()];
    let mut total = 0_u64;
    let mut visited = 0_usize;
    while let Some(path) = pending.pop() {
        visited += 1;
        if visited > 100_000 {
            return None;
        }
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if meta.is_file() {
            total = total.checked_add(meta.len())?;
        } else if meta.is_dir() {
            for entry in std::fs::read_dir(&path).ok()? {
                let child = entry.ok()?.path();
                if child == archive.join("objects") {
                    continue;
                }
                pending.push(child);
            }
        }
    }
    for name in ["server.sqlite3", "server.sqlite3-wal"] {
        match std::fs::symlink_metadata(data.join(name)) {
            Ok(meta) if meta.is_file() => total = total.checked_add(meta.len())?,
            Ok(_) => return None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return None,
        }
    }
    total.checked_mul(3)?.checked_add(64 * 1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reserve_includes_requested_growth() {
        assert!(!enough(RESERVE - 1, 0));
        assert!(!enough(RESERVE, 1));
        assert!(enough(RESERVE + 1, 1));
        assert!(!enough(u64::MAX - 1, u64::MAX));
    }
    #[test]
    fn github_reservation_excludes_original_objects() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("archive/objects/sha256")).unwrap();
        std::fs::write(
            temp.path().join("archive/objects/sha256/original"),
            [0; 128],
        )
        .unwrap();
        std::fs::write(temp.path().join("archive/note.md"), b"abc").unwrap();
        std::fs::write(temp.path().join("server.sqlite3"), b"db").unwrap();
        assert_eq!(
            github_backup_reservation(temp.path()),
            Some(64 * 1024 * 1024 + 15)
        );
    }
    #[test]
    fn actual_filesystem_reports_consistent_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let (total, free) = usage(temp.path()).unwrap();
        assert!(total > 0);
        assert!(free <= total);
    }
    #[test]
    fn old_install_copies_are_listed_and_not_removed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("momso-server.fixture-failed");
        std::fs::write(&path, b"fixture").unwrap();
        std::fs::write(temp.path().join("unrelated"), b"fixture").unwrap();
        assert_eq!(
            legacy_copies(temp.path()).unwrap(),
            vec!["momso-server.fixture-failed"]
        );
        assert!(path.exists());
    }
    #[test]
    fn directory_size_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("file"), b"abc").unwrap();
        symlink("/", temp.path().join("external")).unwrap();
        assert_eq!(directory_bytes(temp.path()), Some(3));
    }
    #[test]
    fn reservation_rejects_concurrent_big_write_and_clears_after_drop() {
        let temp = tempfile::tempdir().unwrap();
        let first = reserve(temp.path(), 1024).unwrap();
        assert_eq!(active_reserved(temp.path()), Some(1024));
        assert!(matches!(
            reserve(temp.path(), 1024),
            Err("reservation_busy")
        ));
        drop(first);
        assert_eq!(active_reserved(temp.path()), Some(0));
        assert!(reserve(temp.path(), 1024).is_ok());
    }
    #[test]
    fn reservation_lock_is_visible_to_another_process() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        let temp = tempfile::tempdir().unwrap();
        initialize(temp.path()).unwrap();
        let code = "import fcntl,sys; f=open(sys.argv[1],'r+'); fcntl.flock(f,fcntl.LOCK_EX); f.seek(0); f.truncate(); f.write('2048\\n'); f.flush(); print('ready',flush=True); sys.stdin.readline()";
        let mut child = Command::new("python3")
            .arg("-c")
            .arg(code)
            .arg(temp.path().join(OPERATION_LOCK))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        assert_eq!(active_reserved(temp.path()), Some(2048));
        assert!(matches!(reserve(temp.path(), 1), Err("reservation_busy")));
        child.stdin.take().unwrap().write_all(b"done\n").unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(active_reserved(temp.path()), Some(0));
    }
}
