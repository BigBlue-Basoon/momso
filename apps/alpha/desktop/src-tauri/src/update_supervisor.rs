//! Start an isolated supervisor from the verified previous installation snapshot.
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant};

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid_update_recovery")
}

fn registry_key(snapshot: &Path) -> io::Result<String> {
    #[cfg(all(debug_assertions, feature = "g61-test-probe"))]
    if let Some(root) = std::env::var_os("MOMSO_S69_TEST_ROOT") {
        let root = std::path::PathBuf::from(root).canonicalize()?;
        if !snapshot.canonicalize()?.starts_with(root) {
            return Err(invalid());
        }
        return Ok(format!(
            r"Software\momso\s81-tests\{}",
            snapshot.file_name().ok_or_else(invalid)?.to_string_lossy()
        ));
    }
    let _ = snapshot;
    Ok(momso_update_recovery::windows::RUN_KEY.to_owned())
}

pub(crate) fn launch(
    backups: &Path,
    protected: &[std::path::PathBuf],
    bytes: &[u8],
    mode: &str,
) -> io::Result<()> {
    // The official Tauri updater accepts a raw Windows executable using the same MZ check.
    if !bytes.starts_with(b"MZ") || !matches!(mode, "quiet" | "passive" | "basicUi") {
        return Err(invalid());
    }
    let executable = std::env::current_exe()?;
    let target = executable.parent().ok_or_else(invalid)?;
    let name = executable.file_name().ok_or_else(invalid)?;
    let stem = executable
        .file_stem()
        .and_then(|v| v.to_str())
        .ok_or_else(invalid)?;
    let parent = backups.join("update-recovery");
    fs::create_dir_all(&parent)?;
    let snapshot = parent.join(uuid::Uuid::new_v4().to_string());
    momso_update_recovery::snapshot_protected(target, &snapshot, protected)?;
    let installer_name = format!("{stem}-installer.exe");
    let mut installer = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(snapshot.join(&installer_name))?;
    installer.write_all(bytes)?;
    installer.sync_all()?;
    let mut checksum = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(snapshot.join("installer.sha256"))?;
    checksum.write_all(format!("{:x}", Sha256::digest(bytes)).as_bytes())?;
    checksum.sync_all()?;
    let mut journal = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(snapshot.join("s81"))?;
    journal.write_all(b"pending")?;
    journal.sync_all()?;
    let helper = snapshot.join("files").join(name);
    let registry = registry_key(&snapshot)?;
    if let Err(error) =
        momso_update_recovery::windows::register_recovery(&registry, &snapshot, &helper)
    {
        let _ = fs::remove_file(snapshot.join("s81"));
        return Err(error);
    }
    let spawn = crate::hidden_process::hidden_command(helper)
        .arg("--momso-update-supervisor")
        .arg(&snapshot)
        .arg(target)
        .arg(std::process::id().to_string())
        .arg(&installer_name)
        .arg(mode)
        .arg(name)
        .args(std::env::args_os().skip(1))
        .spawn();
    let mut child = match spawn {
        Ok(child) => child,
        Err(error) => {
            let _ = momso_update_recovery::windows::unregister_recovery(&registry, &snapshot);
            let _ = fs::remove_file(snapshot.join("s81"));
            return Err(error);
        }
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if snapshot.join("watcher-ready").is_file() {
            return Ok(());
        }
        if child.try_wait()?.is_some() {
            let _ = momso_update_recovery::windows::unregister_recovery(&registry, &snapshot);
            let _ = fs::remove_file(snapshot.join("s81"));
            return Err(invalid());
        }
        if Instant::now() >= deadline {
            // This is the helper just launched above, never an unrelated process.
            let _ = child.kill();
            let _ = child.wait();
            let _ = momso_update_recovery::windows::unregister_recovery(&registry, &snapshot);
            let _ = fs::remove_file(snapshot.join("s81"));
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "update_supervisor_timeout",
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn entry(args: Vec<OsString>) -> io::Result<()> {
    if args.len() < 6 {
        return Err(invalid());
    }
    let snapshot = Path::new(&args[0]);
    let target = Path::new(&args[1]);
    let parent: u32 = args[2]
        .to_str()
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    let installer_name = args[3].to_str().ok_or_else(invalid)?;
    let mode = args[4].to_str().ok_or_else(invalid)?;
    let executable_name = Path::new(&args[5]);
    if executable_name.components().count() != 1
        || executable_name.file_name() != Some(executable_name.as_os_str())
    {
        return Err(invalid());
    }
    let expected_helper = snapshot
        .join("files")
        .join(executable_name)
        .canonicalize()?;
    if std::env::current_exe()?.canonicalize()? != expected_helper {
        return Err(invalid());
    }
    let _lock = momso_update_recovery::windows::supervisor_lock(snapshot)?;
    let installed =
        momso_update_recovery::windows::run(snapshot, target, parent, installer_name, mode)?;
    // On failure the previous files are restored before this new process starts.
    let mut command = crate::hidden_process::hidden_command(target.join(executable_name));
    command.args(&args[6..]);
    command.env("MOMSO_UPDATE_SNAPSHOT", snapshot);
    if !installed {
        command.env("MOMSO_UPDATE_RESTORED", "1");
    } else {
        command.env_remove("MOMSO_UPDATE_RESTORED");
    }
    command.spawn()?;
    Ok(())
}

/// The copy outside the installation starts after a reboot or interrupted supervisor.
pub fn resume() -> io::Result<()> {
    let helper = std::env::current_exe()?.canonicalize()?;
    let name = helper.file_name().ok_or_else(invalid)?;
    let files = helper.parent().ok_or_else(invalid)?;
    if files.file_name() != Some(std::ffi::OsStr::new("files")) {
        return Err(invalid());
    }
    let snapshot = files.parent().ok_or_else(invalid)?;
    let target = momso_update_recovery::snapshot_origin(snapshot)?;
    let executable = target.join(name);
    let stem = Path::new(name)
        .file_stem()
        .and_then(|v| v.to_str())
        .ok_or_else(invalid)?;
    let installer = snapshot.join(format!("{stem}-installer.exe"));
    let healthy = snapshot.join("healthy").is_file();
    let _lock = if !healthy {
        let lock = momso_update_recovery::windows::resume_interrupted(
            snapshot,
            &target,
            &installer,
            &executable,
        )?;
        let mut result = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(snapshot.join("result"))?;
        result.write_all(b"restored")?;
        result.sync_all()?;
        lock
    } else {
        momso_update_recovery::windows::supervisor_lock(snapshot)?
    };
    if healthy && momso_update_recovery::windows::executable_running(&executable)? {
        return Ok(());
    }
    let mut command = crate::hidden_process::hidden_command(executable);
    command.env("MOMSO_UPDATE_SNAPSHOT", snapshot);
    if !healthy {
        command.env("MOMSO_UPDATE_RESTORED", "1");
    }
    command.spawn()?;
    Ok(())
}

/// Do not start a partially replaced application before its recovery helper runs.
pub(crate) fn recover_before_start(data_root: &Path) -> io::Result<bool> {
    let root = data_root.join("backups/update-recovery");
    if !root.exists() {
        return Ok(false);
    }
    let current = std::env::current_exe()?.canonicalize()?;
    let active = std::env::var_os("MOMSO_UPDATE_SNAPSHOT")
        .and_then(|path| std::path::PathBuf::from(path).canonicalize().ok());
    for entry in fs::read_dir(root)? {
        let snapshot = entry?.path();
        if uuid::Uuid::parse_str(snapshot.file_name().and_then(|v| v.to_str()).unwrap_or(""))
            .is_err()
            || !snapshot.join("s81").is_file()
            || snapshot.join("healthy").is_file()
        {
            continue;
        }
        let snapshot = snapshot.canonicalize()?;
        if active.as_ref() == Some(&snapshot) {
            continue;
        }
        let origin = momso_update_recovery::snapshot_origin(&snapshot)?;
        if current.parent() != Some(origin.as_path()) {
            continue;
        }
        let helper = snapshot
            .join("files")
            .join(current.file_name().ok_or_else(invalid)?);
        crate::hidden_process::hidden_command(helper)
            .arg("--momso-update-recover")
            .spawn()?;
        return Ok(true);
    }
    Ok(false)
}

/// Called only after app setup has succeeded. Never remove anything outside our own snapshots.
pub(crate) fn confirm_healthy(backups: &Path) -> io::Result<()> {
    let Some(snapshot) = std::env::var_os("MOMSO_UPDATE_SNAPSHOT") else {
        return cleanup_healthy(backups);
    };
    let snapshot = std::path::PathBuf::from(snapshot).canonicalize()?;
    let root = backups.join("update-recovery").canonicalize()?;
    if snapshot.parent() != Some(root.as_path())
        || uuid::Uuid::parse_str(
            snapshot
                .file_name()
                .and_then(|v| v.to_str())
                .ok_or_else(invalid)?,
        )
        .is_err()
    {
        return Err(invalid());
    }
    let current = std::env::current_exe()?.canonicalize()?;
    if current.parent() != Some(momso_update_recovery::snapshot_origin(&snapshot)?.as_path()) {
        return Err(invalid());
    }
    let result = fs::read(snapshot.join("result"))?;
    if result != b"installed" && result != b"restored" {
        return Err(invalid());
    }
    let mut marker = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(snapshot.join("healthy"))?;
    marker.write_all(b"healthy")?;
    marker.sync_all()?;
    momso_update_recovery::windows::unregister_recovery(&registry_key(&snapshot)?, &snapshot)?;
    cleanup_healthy(backups)
}

fn cleanup_healthy(backups: &Path) -> io::Result<()> {
    let root = backups.join("update-recovery");
    if !root.exists() {
        return Ok(());
    }
    let root = root.canonicalize()?;
    let current = std::env::current_exe()?.canonicalize()?;
    for entry in fs::read_dir(&root)? {
        let snapshot = entry?.path();
        let Ok(canonical) = snapshot.canonicalize() else {
            continue;
        };
        if canonical.parent() != Some(root.as_path())
            || uuid::Uuid::parse_str(canonical.file_name().and_then(|v| v.to_str()).unwrap_or(""))
                .is_err()
            || !canonical.join("healthy").is_file()
        {
            continue;
        }
        let Ok(origin) = momso_update_recovery::snapshot_origin(&canonical) else {
            continue;
        };
        if current.parent() != Some(origin.as_path()) {
            continue;
        }
        momso_update_recovery::windows::unregister_recovery(
            &registry_key(&canonical)?,
            &canonical,
        )?;
        let helper = canonical
            .join("files")
            .join(current.file_name().ok_or_else(invalid)?);
        std::thread::spawn(move || {
            for _ in 0..120 {
                use std::os::windows::fs::MetadataExt;
                use std::os::windows::fs::OpenOptionsExt;
                match OpenOptions::new().read(true).share_mode(0).open(&helper) {
                    Ok(lock) => drop(lock),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                }
                let clean = walkdir::WalkDir::new(&canonical)
                    .follow_links(false)
                    .into_iter()
                    .all(|entry| {
                        entry
                            .ok()
                            .and_then(|entry| entry.metadata().ok())
                            .is_some_and(|metadata| metadata.file_attributes() & 0x400 == 0)
                    });
                if !clean || fs::remove_dir_all(&canonical).is_ok() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        });
    }
    Ok(())
}
