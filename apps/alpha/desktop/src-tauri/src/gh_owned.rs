use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU8, Ordering},
        OnceLock,
    },
};

static ROOT: OnceLock<PathBuf> = OnceLock::new();
static SELECTED: AtomicU8 = AtomicU8::new(0);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RecoveryError {
    Unknown,
    CleanupFailed,
}

pub(crate) fn configure(data_root: &Path) {
    let _ = ROOT.set(data_root.join("gh-owned"));
}

pub(crate) fn directory() -> Option<&'static Path> {
    ROOT.get().map(PathBuf::as_path)
}

fn marker(directory: &Path) -> PathBuf {
    directory.join(".momso-login-pid")
}

pub(crate) fn begin_login(directory: &Path) -> Result<(), ()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker(directory))
        .map_err(|_| ())?;
    write!(file, "starting:{}", std::process::id()).map_err(|_| ())?;
    file.sync_all().map_err(|_| ())
}

pub(crate) fn record_login_pid(directory: &Path, pid: u32) -> Result<(), ()> {
    let temp = directory.join(format!(".momso-login-marker-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| ())?;
        write!(file, "{pid}").map_err(|_| ())?;
        file.sync_all().map_err(|_| ())?;
        fs::rename(&temp, marker(directory)).map_err(|_| ())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

pub(crate) fn finish_login(directory: &Path) -> Result<(), ()> {
    fs::remove_file(marker(directory)).map_err(|_| ())
}

#[cfg(windows)]
fn process_exists(pid: Option<u32>) -> Result<bool, ()> {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(());
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut success = unsafe { Process32FirstW(snapshot, &mut entry) };
    if success == 0 {
        unsafe {
            CloseHandle(snapshot);
        }
        return Err(());
    }
    let mut found = false;
    while success != 0 {
        let name_end = entry
            .szExeFile
            .iter()
            .position(|v| *v == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..name_end]);
        if pid.map_or_else(
            || name.eq_ignore_ascii_case("gh.exe"),
            |id| entry.th32ProcessID == id,
        ) {
            found = true;
            break;
        }
        success = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    let completed = found || unsafe { GetLastError() } == ERROR_NO_MORE_FILES;
    unsafe {
        CloseHandle(snapshot);
    }
    if !completed {
        return Err(());
    }
    Ok(found)
}

#[cfg(not(windows))]
fn process_exists(_pid: Option<u32>) -> Result<bool, ()> {
    Err(())
}

pub(crate) fn select(source: u8) {
    SELECTED.store(source, Ordering::SeqCst);
}
pub(crate) fn source() -> Option<&'static str> {
    match SELECTED.load(Ordering::SeqCst) {
        1 => Some("default"),
        2 => Some("owned"),
        _ => None,
    }
}
pub(crate) fn apply(command: &mut std::process::Command) -> std::io::Result<()> {
    if ROOT.get().is_none() {
        return Ok(());
    }
    command
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_ENTERPRISE_TOKEN")
        .env_remove("GITHUB_ENTERPRISE_TOKEN");
    match source() {
        Some("default") => Ok(()),
        Some("owned") => {
            command.env(
                "GH_CONFIG_DIR",
                directory().ok_or(std::io::ErrorKind::NotFound)?,
            );
            Ok(())
        }
        _ => Err(std::io::Error::other(
            "GitHub credential storage is unverified",
        )),
    }
}

fn strip_tokens(value: &mut serde_yaml::Value) -> bool {
    match value {
        serde_yaml::Value::Mapping(map) => {
            let mut removed = map
                .remove(serde_yaml::Value::String("oauth_token".into()))
                .is_some();
            for value in map.values_mut() {
                removed |= strip_tokens(value);
            }
            removed
        }
        serde_yaml::Value::Sequence(items) => {
            let mut found = false;
            for item in items {
                found |= strip_tokens(item);
            }
            found
        }
        _ => false,
    }
}

// The caller supplies only the application-owned directory, never the default gh directory.
pub(crate) fn recover(directory: &Path) -> Result<bool, RecoveryError> {
    let meta = match fs::symlink_metadata(directory) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(RecoveryError::Unknown),
    };
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err(RecoveryError::Unknown);
    }
    let marker = marker(directory);
    match fs::symlink_metadata(&marker) {
        Ok(meta) => {
            if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
                return Err(RecoveryError::Unknown);
            }
            let text = fs::read_to_string(&marker).map_err(|_| RecoveryError::Unknown)?;
            let text = text.trim();
            let starting = text.strip_prefix("starting:");
            let pid: u32 = starting
                .unwrap_or(text)
                .parse()
                .map_err(|_| RecoveryError::Unknown)?;
            if process_exists(Some(pid)).map_err(|_| RecoveryError::Unknown)?
                || (starting.is_some()
                    && process_exists(None).map_err(|_| RecoveryError::Unknown)?)
            {
                return Err(RecoveryError::Unknown);
            }
            fs::remove_file(&marker).map_err(|_| RecoveryError::CleanupFailed)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(RecoveryError::Unknown),
    }
    let file = directory.join("hosts.yml");
    let meta = match fs::symlink_metadata(&file) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(RecoveryError::Unknown),
    };
    if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err(RecoveryError::Unknown);
    }
    let bytes = fs::read(&file).map_err(|_| RecoveryError::Unknown)?;
    let mut document: serde_yaml::Value =
        serde_yaml::from_slice(&bytes).map_err(|_| RecoveryError::Unknown)?;
    if !matches!(document, serde_yaml::Value::Mapping(_)) {
        return Err(RecoveryError::Unknown);
    }
    if !strip_tokens(&mut document) {
        return Ok(false);
    }
    let output = serde_yaml::to_string(&document).map_err(|_| RecoveryError::CleanupFailed)?;
    let temp = directory.join(format!(".hosts-clean-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| RecoveryError::CleanupFailed)?;
        out.write_all(output.as_bytes())
            .map_err(|_| RecoveryError::CleanupFailed)?;
        out.sync_all().map_err(|_| RecoveryError::CleanupFailed)?;
        if fs::read(&file).map_err(|_| RecoveryError::CleanupFailed)? != bytes {
            return Err(RecoveryError::CleanupFailed);
        }
        fs::rename(&temp, &file).map_err(|_| RecoveryError::CleanupFailed)?;
        Ok(true)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_recovery_removes_only_tokens_and_retries_after_invalid_yaml() {
        let temp = tempfile::tempdir().unwrap();
        let own = temp.path().join("own");
        let original = temp.path().join("default");
        fs::create_dir_all(&own).unwrap();
        fs::create_dir_all(&original).unwrap();
        let untouched = b"github.com:\n  oauth_token: original-test-value\n";
        fs::write(original.join("hosts.yml"), untouched).unwrap();
        fs::write(own.join("hosts.yml"), b"not: [valid").unwrap();
        assert_eq!(recover(&own), Err(RecoveryError::Unknown));
        fs::write(own.join("hosts.yml"), b"github.com:\n  user: example\n  oauth_token: test-value\n  users:\n    example:\n      oauth_token: test-value\n").unwrap();
        assert_eq!(recover(&own), Ok(true));
        let result = fs::read_to_string(own.join("hosts.yml")).unwrap();
        assert!(!result.contains("oauth_token"));
        assert!(result.contains("example"));
        assert_eq!(fs::read(original.join("hosts.yml")).unwrap(), untouched);
        assert_eq!(recover(&own), Ok(false));
    }

    #[cfg(windows)]
    #[test]
    fn fake_login_restart_recovery_never_calls_gh_logout() {
        let temp = tempfile::tempdir().unwrap();
        let own = temp.path().join("own");
        let default = temp.path().join("default");
        fs::create_dir_all(&own).unwrap();
        fs::create_dir_all(&default).unwrap();
        let untouched = b"github.com:\n  oauth_token: original-fixture\n";
        fs::write(default.join("hosts.yml"), untouched).unwrap();
        let script = temp.path().join("fake-gh.ps1");
        let calls = temp.path().join("calls.txt");
        fs::write(&script, "param($folder, $calls)\nAdd-Content -LiteralPath $calls 'login'\nSet-Content -LiteralPath (Join-Path $folder 'hosts.yml') 'github.com: {oauth_token: fixture}'\nStart-Sleep -Seconds 30\n").unwrap();
        let mut child = crate::process::hidden_command("powershell")
            .args(["-NoProfile", "-File"])
            .arg(&script)
            .arg(&own)
            .arg(&calls)
            .spawn()
            .unwrap();
        begin_login(&own).unwrap();
        let while_starting = recover(&own);
        record_login_pid(&own, child.id()).unwrap();
        for _ in 0..100 {
            if own.join("hosts.yml").is_file() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let created = own.join("hosts.yml").is_file();
        let while_running = recover(&own);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(while_starting, Err(RecoveryError::Unknown));
        assert_eq!(while_running, Err(RecoveryError::Unknown));
        assert!(created);
        assert_eq!(recover(&own), Ok(true));
        assert!(!fs::read_to_string(own.join("hosts.yml"))
            .unwrap()
            .contains("oauth_token"));
        assert_eq!(fs::read(default.join("hosts.yml")).unwrap(), untouched);
        assert_eq!(fs::read_to_string(calls).unwrap().trim(), "login");
    }
}
