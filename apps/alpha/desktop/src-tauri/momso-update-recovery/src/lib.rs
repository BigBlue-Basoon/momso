//! Verified snapshots for a separately supervised updater. No app-data files are included.
#[cfg(windows)]
pub mod windows;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;

#[derive(Serialize, Deserialize)]
struct Manifest {
    origin: String,
    files: Vec<Entry>,
    dirs: Vec<String>,
    #[serde(default)]
    protected: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct Entry {
    path: String,
    digest: String,
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid_update_snapshot")
}
/// Return the original installation root recorded in a complete snapshot.
pub fn snapshot_origin(snapshot_dir: &Path) -> io::Result<PathBuf> {
    let snapshot_dir = safe_root(snapshot_dir)?;
    let manifest_path = snapshot_dir.join("manifest.json");
    if is_link(&fs::symlink_metadata(&manifest_path)?) {
        return Err(invalid());
    }
    let manifest: Manifest =
        serde_json::from_reader(File::open(manifest_path)?).map_err(io::Error::other)?;
    let origin = safe_root(Path::new(&manifest.origin))?;
    if origin != Path::new(&manifest.origin) {
        return Err(invalid());
    }
    Ok(origin)
}
fn digest(path: &Path) -> io::Result<String> {
    let mut input = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut data = [0; 65536];
    loop {
        let count = input.read(&mut data)?;
        if count == 0 {
            break;
        }
        hasher.update(&data[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}
fn checked_relative(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty() || !path.components().all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(invalid());
    }
    Ok(())
}
#[cfg(windows)]
fn same_component(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Globalization::{CompareStringOrdinal, CSTR_EQUAL};
    let left: Vec<u16> = left.encode_wide().collect();
    let right: Vec<u16> = right.encode_wide().collect();
    let result = unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            left.len() as i32,
            right.as_ptr(),
            right.len() as i32,
            1,
        )
    };
    result == CSTR_EQUAL || result == 0
}
#[cfg(not(windows))]
fn same_component(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    left == right
}
fn within(path: &Path, prefix: &Path) -> bool {
    let mut parts = path.components();
    prefix.components().all(|part| {
        parts
            .next()
            .is_some_and(|candidate| same_component(candidate.as_os_str(), part.as_os_str()))
    })
}
#[cfg(windows)]
fn known_path(paths: &std::collections::HashSet<PathBuf>, path: &Path) -> bool {
    paths.contains(path)
        || paths.iter().any(|candidate| {
            candidate.components().count() == path.components().count() && within(path, candidate)
        })
}
#[cfg(not(windows))]
fn known_path(paths: &std::collections::HashSet<PathBuf>, path: &Path) -> bool {
    paths.contains(path)
}
fn safe_root(path: &Path) -> io::Result<PathBuf> {
    let meta = fs::symlink_metadata(path)?;
    if is_link(&meta) || !meta.is_dir() {
        return Err(invalid());
    }
    fs::canonicalize(path)
}
fn protected_root(path: &Path) -> io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    let mut missing = Vec::new();
    loop {
        match fs::symlink_metadata(&current) {
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(current.file_name().ok_or_else(invalid)?.to_os_string());
                current = current.parent().ok_or_else(invalid)?.to_path_buf();
            }
            Err(error) => return Err(error),
        }
    }
    let mut resolved = safe_root(&current)?;
    for part in missing.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}
fn preflight_target(root: &Path, relative: &Path, directory: bool) -> io::Result<()> {
    checked_relative(relative)?;
    let mut ancestor = root.to_path_buf();
    let mut parts = relative.components().peekable();
    while let Some(part) = parts.next() {
        ancestor.push(part.as_os_str());
        match fs::symlink_metadata(&ancestor) {
            Ok(meta) => {
                if is_link(&meta) {
                    return Err(invalid());
                }
                if parts.peek().is_some() || directory {
                    if !meta.is_dir() {
                        return Err(invalid());
                    }
                } else if !meta.is_file() {
                    return Err(invalid());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
/// Create a new snapshot. The final manifest is written only after every file is verified.
/// The caller must ensure no installer is concurrently changing the installation.
pub fn snapshot(source: &Path, snapshot_dir: &Path) -> io::Result<()> {
    snapshot_protected(source, snapshot_dir, &[])
}

/// Protected directories are user data and are never copied, restored, or removed.
pub fn snapshot_protected(
    source: &Path,
    snapshot_dir: &Path,
    protected: &[PathBuf],
) -> io::Result<()> {
    let source = safe_root(source)?;
    let parent = safe_root(snapshot_dir.parent().ok_or_else(invalid)?)?;
    let name = snapshot_dir.file_name().ok_or_else(invalid)?;
    let target = parent.join(name);
    if target.starts_with(&source) || source.starts_with(&target) {
        return Err(invalid());
    }
    let mut protected_relative = Vec::new();
    for path in protected {
        let path = protected_root(path)?;
        if path == source || source.starts_with(&path) {
            return Err(invalid());
        }
        if let Ok(relative) = path.strip_prefix(&source) {
            checked_relative(relative)?;
            protected_relative.push(relative.to_path_buf());
        }
    }
    fs::create_dir(&target)?;
    let files_dir = target.join("files");
    fs::create_dir(&files_dir)?;
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for item in WalkDir::new(&source)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry
                .path()
                .strip_prefix(&source)
                .map(|relative| !protected_relative.iter().any(|path| within(relative, path)))
                .unwrap_or(false)
        })
    {
        let item = item.map_err(io::Error::other)?;
        let relative = item.path().strip_prefix(&source).map_err(|_| invalid())?;
        checked_relative(relative)?;
        let meta = fs::symlink_metadata(item.path())?;
        if is_link(&meta) {
            return Err(invalid());
        }
        let destination = files_dir.join(relative);
        if meta.is_dir() {
            fs::create_dir(&destination)?;
            dirs.push(relative.to_str().ok_or_else(invalid)?.to_owned());
        } else if meta.is_file() {
            fs::copy(item.path(), &destination)?;
            let copied = digest(&destination)?;
            if digest(item.path())? != copied {
                return Err(invalid());
            }
            OpenOptions::new()
                .write(true)
                .open(&destination)?
                .sync_all()?;
            files.push(Entry {
                path: relative.to_str().ok_or_else(invalid)?.to_owned(),
                digest: copied,
            });
        } else {
            return Err(invalid());
        }
    }
    let manifest = Manifest {
        origin: source.to_str().ok_or_else(invalid)?.to_owned(),
        files,
        dirs,
        protected: protected_relative
            .iter()
            .map(|path| path.to_str().map(str::to_owned).ok_or_else(invalid))
            .collect::<io::Result<_>>()?,
    };
    let bytes = serde_json::to_vec(&manifest).map_err(io::Error::other)?;
    let mut output = File::create(target.join("manifest.json"))?;
    output.write_all(&bytes)?;
    output.sync_all()?;
    Ok(())
}
/// Validate a snapshot before replacing files and remove new installation files.
/// The caller must run outside the installation and stop all processes using its files.
pub fn restore(snapshot_dir: &Path, target: &Path) -> io::Result<()> {
    let snapshot_dir = safe_root(snapshot_dir)?;
    let target = safe_root(target)?;
    let manifest_path = snapshot_dir.join("manifest.json");
    if is_link(&fs::symlink_metadata(&manifest_path)?) {
        return Err(invalid());
    }
    let manifest: Manifest =
        serde_json::from_reader(File::open(manifest_path)?).map_err(io::Error::other)?;
    if Path::new(&manifest.origin) != target {
        return Err(invalid());
    }
    let files_dir = safe_root(&snapshot_dir.join("files"))?;
    let mut protected = Vec::new();
    for entry in &manifest.protected {
        let path = PathBuf::from(entry);
        checked_relative(&path)?;
        protected.push(path);
    }
    for entry in &manifest.files {
        let relative = Path::new(&entry.path);
        checked_relative(relative)?;
        let path = files_dir.join(relative);
        let mut ancestor = files_dir.clone();
        for part in relative.components() {
            ancestor.push(part.as_os_str());
            if is_link(&fs::symlink_metadata(&ancestor)?) {
                return Err(invalid());
            }
        }
        if digest(&path)? != entry.digest {
            return Err(invalid());
        }
    }
    // Do not follow links or touch protected user data, even while enumerating.
    let old_files: std::collections::HashSet<PathBuf> = manifest
        .files
        .iter()
        .map(|entry| PathBuf::from(&entry.path))
        .collect();
    let old_dirs: std::collections::HashSet<PathBuf> =
        manifest.dirs.iter().map(PathBuf::from).collect();
    let mut extra_files = Vec::new();
    let mut extra_dirs = Vec::new();
    for item in WalkDir::new(&target)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry
                .path()
                .strip_prefix(&target)
                .map(|relative| !protected.iter().any(|path| within(relative, path)))
                .unwrap_or(false)
        })
    {
        let item = item.map_err(io::Error::other)?;
        let relative = item.path().strip_prefix(&target).map_err(|_| invalid())?;
        checked_relative(relative)?;
        let metadata = fs::symlink_metadata(item.path())?;
        if is_link(&metadata) {
            return Err(invalid());
        }
        if metadata.is_file() && !known_path(&old_files, relative) {
            extra_files.push(item.path().to_path_buf());
        } else if metadata.is_dir() && !known_path(&old_dirs, relative) {
            extra_dirs.push(item.path().to_path_buf());
        } else if !metadata.is_file() && !metadata.is_dir() {
            return Err(invalid());
        }
    }
    // Validate every target path before modifying any installation file.
    for directory in &manifest.dirs {
        preflight_target(&target, Path::new(directory), true)?;
    }
    for entry in &manifest.files {
        preflight_target(&target, Path::new(&entry.path), false)?;
    }
    for directory in &manifest.dirs {
        let mut ancestor = target.clone();
        for part in Path::new(directory).components() {
            ancestor.push(part.as_os_str());
            match fs::symlink_metadata(&ancestor) {
                Ok(meta) if meta.is_dir() && !is_link(&meta) => {}
                Ok(_) => return Err(invalid()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&ancestor)?,
                Err(error) => return Err(error),
            }
        }
    }
    for entry in &manifest.files {
        let relative = Path::new(&entry.path);
        let mut ancestor = target.clone();
        let parent = relative.parent().ok_or_else(invalid)?;
        for part in parent.components() {
            ancestor.push(part.as_os_str());
            if ancestor.exists() {
                let meta = fs::symlink_metadata(&ancestor)?;
                if is_link(&meta) || !meta.is_dir() {
                    return Err(invalid());
                }
            } else {
                fs::create_dir(&ancestor)?;
            }
        }
        let destination = target.join(relative);
        if let Ok(meta) = fs::symlink_metadata(&destination) {
            if is_link(&meta) || !meta.is_file() {
                return Err(invalid());
            }
        }
        fs::copy(files_dir.join(relative), &destination)?;
        OpenOptions::new()
            .write(true)
            .open(&destination)?
            .sync_all()?;
        if digest(&destination)? != entry.digest {
            return Err(invalid());
        }
    }
    for file in extra_files {
        fs::remove_file(file)?;
    }
    extra_dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in extra_dirs {
        match fs::remove_dir(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Run a supervised installation and restore verified prior files on failure.
/// The installer callback must wait until the installer process has exited.
/// A supervisor outside the installation must invoke this function.
pub fn supervise<F>(snapshot_dir: &Path, target: &Path, install: F) -> io::Result<bool>
where
    F: FnOnce() -> io::Result<bool>,
{
    match install() {
        Ok(true) => Ok(true),
        Ok(false) => {
            restore(snapshot_dir, target)?;
            Ok(false)
        }
        Err(error) => {
            restore(snapshot_dir, target)?;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removes_new_install_files_but_preserves_user_data() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        let user = app.join("user-data");
        fs::create_dir_all(&user).unwrap();
        fs::write(app.join("app.exe"), b"old").unwrap();
        fs::write(user.join("note"), b"before").unwrap();
        let snap = root.path().join("snapshot");
        snapshot_protected(&app, &snap, std::slice::from_ref(&user)).unwrap();
        fs::write(user.join("note"), b"user edit").unwrap();
        fs::write(user.join("new"), b"user file").unwrap();
        fs::create_dir(app.join("new-component")).unwrap();
        fs::write(app.join("new-component/new.dll"), b"new").unwrap();
        fs::write(app.join("app.exe"), b"partial").unwrap();
        restore(&snap, &app).unwrap();
        assert_eq!(fs::read(app.join("app.exe")).unwrap(), b"old");
        assert!(!app.join("new-component").exists());
        assert_eq!(fs::read(user.join("note")).unwrap(), b"user edit");
        assert_eq!(fs::read(user.join("new")).unwrap(), b"user file");
    }
    #[test]
    fn protects_user_directory_created_after_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        fs::create_dir(&app).unwrap();
        fs::write(app.join("app.exe"), b"old").unwrap();
        let user = app.join("later/user-data");
        let snap = root.path().join("snapshot");
        snapshot_protected(&app, &snap, std::slice::from_ref(&user)).unwrap();
        fs::create_dir_all(&user).unwrap();
        fs::write(user.join("note"), b"user").unwrap();
        restore(&snap, &app).unwrap();
        assert_eq!(fs::read(user.join("note")).unwrap(), b"user");
    }
    #[cfg(windows)]
    #[test]
    fn windows_case_changes_do_not_delete_existing_or_protected_paths() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        fs::create_dir(&app).unwrap();
        fs::write(app.join("app.exe"), b"old").unwrap();
        let protected = app.join("UserData");
        let snap = root.path().join("snapshot");
        snapshot_protected(&app, &snap, std::slice::from_ref(&protected)).unwrap();
        fs::create_dir(app.join("USERDATA")).unwrap();
        fs::write(app.join("USERDATA/note"), b"keep").unwrap();
        fs::rename(app.join("app.exe"), app.join("temporary-name")).unwrap();
        fs::rename(app.join("temporary-name"), app.join("APP.EXE")).unwrap();
        restore(&snap, &app).unwrap();
        assert_eq!(fs::read(app.join("app.exe")).unwrap(), b"old");
        assert_eq!(fs::read(app.join("USERDATA/note")).unwrap(), b"keep");
    }
    #[test]
    fn restores_changed_and_missing_files() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        fs::create_dir(&app).unwrap();
        fs::create_dir(app.join("assets")).unwrap();
        fs::write(app.join("app.exe"), b"old").unwrap();
        fs::write(app.join("assets/data"), b"data").unwrap();
        let snap = root.path().join("snapshot");
        snapshot(&app, &snap).unwrap();
        fs::write(app.join("app.exe"), b"partial").unwrap();
        fs::remove_file(app.join("assets/data")).unwrap();
        restore(&snap, &app).unwrap();
        assert_eq!(fs::read(app.join("app.exe")).unwrap(), b"old");
        assert_eq!(fs::read(app.join("assets/data")).unwrap(), b"data");
    }
    #[cfg(unix)]
    #[test]
    fn killed_installer_restores_previous_executable() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        fs::create_dir(&app).unwrap();
        let executable = app.join("app.exe");
        fs::write(&executable, b"old").unwrap();
        let snap = root.path().join("snapshot");
        snapshot(&app, &snap).unwrap();
        let ready = root.path().join("installer-ready");
        let recovered = supervise(&snap, &app, || {
            let mut child = Command::new("sh")
                .arg("-c")
                .arg("printf partial > \"$1\"; printf ready > \"$2\"; exec sleep 60")
                .arg("fixture")
                .arg(&executable)
                .arg(&ready)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            let deadline = Instant::now() + Duration::from_secs(5);
            while !ready.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if !ready.exists() {
                child.kill()?;
                child.wait()?;
                return Err(io::Error::new(io::ErrorKind::TimedOut, "fixture_not_ready"));
            }
            child.kill()?;
            Ok(child.wait()?.success())
        })
        .unwrap();
        assert!(!recovered);
        assert_eq!(fs::read(executable).unwrap(), b"old");
    }
    #[test]
    fn rejects_corruption_before_touching_installation() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        fs::create_dir(&app).unwrap();
        fs::write(app.join("app.exe"), b"old").unwrap();
        let snap = root.path().join("snapshot");
        snapshot(&app, &snap).unwrap();
        fs::write(snap.join("files/app.exe"), b"corrupt").unwrap();
        fs::write(app.join("app.exe"), b"current").unwrap();
        assert!(restore(&snap, &app).is_err());
        assert_eq!(fs::read(app.join("app.exe")).unwrap(), b"current");
    }
    #[test]
    fn rejects_nested_snapshot_and_wrong_target() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        fs::create_dir(&app).unwrap();
        fs::write(app.join("app.exe"), b"old").unwrap();
        assert!(snapshot(&app, &app.join("nested")).is_err());
        let snap = root.path().join("snapshot");
        snapshot(&app, &snap).unwrap();
        let other = root.path().join("other");
        fs::create_dir(&other).unwrap();
        assert!(restore(&snap, &other).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_target_symlink_before_restoring_other_files() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        fs::create_dir(&app).unwrap();
        fs::write(app.join("a"), b"old").unwrap();
        fs::write(app.join("z"), b"old").unwrap();
        let snap = root.path().join("snapshot");
        snapshot(&app, &snap).unwrap();
        fs::write(app.join("a"), b"current").unwrap();
        fs::remove_file(app.join("z")).unwrap();
        std::os::unix::fs::symlink(root.path().join("outside"), app.join("z")).unwrap();
        assert!(restore(&snap, &app).is_err());
        assert_eq!(fs::read(app.join("a")).unwrap(), b"current");
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_installation_content() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        fs::create_dir(&app).unwrap();
        std::os::unix::fs::symlink(root.path(), app.join("outside")).unwrap();
        assert!(snapshot(&app, &root.path().join("snapshot")).is_err());
    }
}
