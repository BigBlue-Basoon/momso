//! Windows updater supervisor. The supervisor runs from a verified snapshot.
use crate::{invalid, safe_root};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE, STILL_ACTIVE,
    WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteValueW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject, INFINITE,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};
use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

fn value_name(snapshot: &Path) -> io::Result<String> {
    let name = snapshot
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(invalid)?;
    if name.len() != 36
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Err(invalid());
    }
    Ok(format!("momso-update-recovery-{name}"))
}

fn registry_value(key: &str, snapshot: &Path, command: Option<&str>) -> io::Result<()> {
    let path = wide(OsStr::new(key));
    let name = wide(OsStr::new(&value_name(snapshot)?));
    let mut handle: HKEY = std::ptr::null_mut();
    let result = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut handle,
            std::ptr::null_mut(),
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let result = if let Some(command) = command {
        let value = wide(OsStr::new(command));
        unsafe {
            RegSetValueExW(
                handle,
                name.as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteValueW(handle, name.as_ptr()) }
    };
    unsafe {
        RegCloseKey(handle);
    }
    if result != 0 && !(command.is_none() && result == 2) {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    Ok(())
}

/// Register only this update's helper. Windows limits Run commands to 260 characters.
pub fn register_recovery(key: &str, snapshot: &Path, helper: &Path) -> io::Result<()> {
    let helper = helper.to_str().ok_or_else(invalid)?;
    let command = format!("\"{helper}\" --momso-update-recover");
    if command.encode_utf16().count() > 260 {
        return Err(invalid());
    }
    registry_value(key, snapshot, Some(&command))
}
pub fn unregister_recovery(key: &str, snapshot: &Path) -> io::Result<()> {
    registry_value(key, snapshot, None)?;
    if key.starts_with(r"Software\momso\s81-tests\") {
        let path = wide(OsStr::new(key));
        let result = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, path.as_ptr()) };
        if result != 0 && result != 2 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
    }
    Ok(())
}

pub fn supervisor_lock(snapshot: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(snapshot.join("supervisor.lock"))
}

fn process_running(executable: &Path) -> io::Result<bool> {
    let name = executable.file_name().ok_or_else(invalid)?;
    let expected = executable
        .parent()
        .ok_or_else(invalid)?
        .canonicalize()?
        .join(name);
    unsafe {
        let list = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if list == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = Process32FirstW(list, &mut entry);
        let mut result = Ok(false);
        if found == 0 && GetLastError() != ERROR_NO_MORE_FILES {
            result = Err(io::Error::last_os_error());
        }
        while found != 0 {
            let end = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            let candidate = std::ffi::OsString::from_wide(&entry.szExeFile[..end]);
            if candidate
                .to_string_lossy()
                .eq_ignore_ascii_case(&name.to_string_lossy())
            {
                let process =
                    OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID);
                if process.is_null() {
                    result = Err(io::Error::last_os_error());
                    break;
                }
                let mut exit_code = 0u32;
                if GetExitCodeProcess(process, &mut exit_code) == 0 {
                    CloseHandle(process);
                    result = Err(io::Error::last_os_error());
                    break;
                }
                if exit_code != STILL_ACTIVE as u32 {
                    CloseHandle(process);
                    found = Process32NextW(list, &mut entry);
                    if found == 0 && GetLastError() != ERROR_NO_MORE_FILES {
                        result = Err(io::Error::last_os_error());
                    }
                    continue;
                }
                let mut buffer = vec![0u16; 32768];
                let mut len = buffer.len() as u32;
                let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut len);
                CloseHandle(process);
                if ok == 0 {
                    result = Err(io::Error::last_os_error());
                    break;
                }
                let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(
                    &buffer[..len as usize],
                ));
                match path.canonicalize() {
                    Ok(path)
                        if path.components().count() == expected.components().count()
                            && crate::within(&path, &expected) =>
                    {
                        result = Ok(true);
                        break;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        result = Err(error);
                        break;
                    }
                }
            }
            found = Process32NextW(list, &mut entry);
            if found == 0 && GetLastError() != ERROR_NO_MORE_FILES {
                result = Err(io::Error::last_os_error());
            }
        }
        CloseHandle(list);
        result
    }
}

pub fn executable_running(executable: &Path) -> io::Result<bool> {
    process_running(executable)
}

/// Recovery invoked at a later login never races an active supervisor or installer.
pub fn resume_interrupted(
    snapshot: &Path,
    target: &Path,
    installer: &Path,
    executable: &Path,
) -> io::Result<fs::File> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match supervisor_lock(snapshot) {
            Ok(lock) if !process_running(installer)? && !process_running(executable)? => {
                restore_with_retry(snapshot, target)?;
                return Ok(lock);
            }
            Ok(_) => {}
            Err(error) if matches!(error.raw_os_error(), Some(32 | 33)) => {}
            Err(error) => return Err(error),
        }
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "update_process_running",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

fn restore_with_retry(snapshot: &Path, target: &Path) -> io::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match crate::restore(snapshot, target) {
            Ok(()) => return Ok(()),
            Err(error)
                if matches!(error.raw_os_error(), Some(32 | 33))
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            Err(error) => return Err(error),
        }
    }
}

/// Open the parent before signalling readiness; wait for it to exit before installation.
/// An interrupted installer is considered a failure and the verified snapshot is restored.
pub fn run(
    snapshot: &Path,
    target: &Path,
    parent_pid: u32,
    installer_name: &str,
    mode: &str,
) -> io::Result<bool> {
    let snapshot = safe_root(snapshot)?;
    let target = safe_root(target)?;
    let name = Path::new(installer_name);
    crate::checked_relative(name)?;
    if name.components().count() != 1 {
        return Err(invalid());
    }
    let installer = snapshot.join(name);
    let metadata = fs::symlink_metadata(&installer)?;
    if crate::is_link(&metadata) || !metadata.is_file() {
        return Err(invalid());
    }
    let expected = fs::read_to_string(snapshot.join("installer.sha256"))?;
    if crate::digest(&installer)? != expected {
        return Err(invalid());
    }
    let args = match mode {
        "quiet" => "/S /UPDATE",
        "passive" => "/P /UPDATE",
        "basicUi" => "/UPDATE",
        _ => return Err(invalid()),
    };
    // The process handle refers to this exact parent, avoiding PID reuse while waiting.
    let parent = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, parent_pid) };
    if parent.is_null() {
        return Err(io::Error::last_os_error());
    }
    if let Err(error) = fs::write(snapshot.join("watcher-ready"), b"ready") {
        unsafe {
            CloseHandle(parent);
        }
        return Err(error);
    }
    let waited = unsafe { WaitForSingleObject(parent, INFINITE) };
    unsafe {
        CloseHandle(parent);
    }
    if waited != WAIT_OBJECT_0 {
        return Err(io::Error::last_os_error());
    }

    enum InstallerResult {
        Finished(bool),
        Unknown(io::Error),
    }
    let file = wide(installer.as_os_str());
    let parameters = wide(OsStr::new(args));
    let verb = wide(OsStr::new("open"));
    let directory = wide(snapshot.as_os_str());
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.lpDirectory = directory.as_ptr();
    info.nShow = SW_SHOW;
    let exit = if unsafe { ShellExecuteExW(&mut info) } == 0 {
        InstallerResult::Finished(false)
    } else if info.hProcess.is_null() {
        // Shell accepted the request but did not give us a process to wait on.
        // It may still be writing, so do not touch the installation.
        InstallerResult::Unknown(invalid())
    } else {
        let waited = unsafe { WaitForSingleObject(info.hProcess, INFINITE) };
        let mut exit_code = 1;
        let got_exit = unsafe { GetExitCodeProcess(info.hProcess, &mut exit_code) };
        unsafe {
            CloseHandle(info.hProcess);
        }
        if waited != WAIT_OBJECT_0 {
            InstallerResult::Unknown(io::Error::last_os_error())
        } else {
            InstallerResult::Finished(got_exit != 0 && exit_code == 0)
        }
    };
    let outcome = match exit {
        InstallerResult::Finished(true) => Ok(true),
        InstallerResult::Finished(false) => restore_with_retry(&snapshot, &target).map(|()| false),
        InstallerResult::Unknown(error) => Err(error),
    };
    match &outcome {
        Ok(true) => {
            let _ = fs::write(snapshot.join("result"), b"installed");
        }
        Ok(false) => {
            let _ = fs::write(snapshot.join("result"), b"restored");
        }
        Err(_) => {
            let _ = fs::write(snapshot.join("result"), b"failed_or_unknown");
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    #[test]
    fn retry_restores_after_windows_file_lock_is_released() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        fs::create_dir(&app).unwrap();
        let executable = app.join("app.exe");
        fs::write(&executable, b"old").unwrap();
        let snapshot = root.path().join("snapshot");
        crate::snapshot(&app, &snapshot).unwrap();
        fs::write(&executable, b"partial").unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&executable)
            .unwrap();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(700));
            drop(lock);
        });
        restore_with_retry(&snapshot, &app).unwrap();
        releaser.join().unwrap();
        assert_eq!(fs::read(executable).unwrap(), b"old");
    }
}
