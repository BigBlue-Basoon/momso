//! Download into an app-owned staging directory before a no-overwrite publish.
use std::{
    collections::HashMap,
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use tauri::{Emitter, Manager};

struct Pending {
    name: String,
    chat_id: Option<String>,
    staging: PathBuf,
    _marker: std::fs::File,
}
const MARKER: &str = ".momso-download";

fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

fn marker_options() -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    options
}

/// Remove only recorded files from app-owned, inactive staging directories.
pub(crate) fn cleanup_staging(root: &Path) -> io::Result<()> {
    let root = root.join("panel-download-staging");
    let metadata = match std::fs::symlink_metadata(&root) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.is_dir() || is_link_or_reparse(&metadata) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    for entry in std::fs::read_dir(root)?.flatten() {
        let dir = entry.path();
        if uuid::Uuid::parse_str(&entry.file_name().to_string_lossy()).is_err() {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&dir) else {
            continue;
        };
        if !meta.is_dir() || is_link_or_reparse(&meta) {
            continue;
        }
        let marker = dir.join(MARKER);
        let Ok(meta) = std::fs::symlink_metadata(&marker) else {
            continue;
        };
        if !meta.is_file() || is_link_or_reparse(&meta) {
            continue;
        }
        let Ok(mut file) = marker_options().open(&marker) else {
            continue;
        };
        let mut name = String::new();
        if (&mut file).take(1025).read_to_string(&mut name).is_err()
            || name.len() > 1024
            || !safe_name(&name)
            || name == MARKER
        {
            continue;
        }
        let partial = format!("{name}.crdownload");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let entries: Vec<_> = entries.flatten().collect();
        if entries.iter().any(|item| {
            let entry_name = item.file_name();
            let allowed = entry_name == MARKER
                || entry_name == name.as_str()
                || entry_name == partial.as_str();
            !allowed
                || std::fs::symlink_metadata(item.path())
                    .map(|m| !m.is_file() || is_link_or_reparse(&m))
                    .unwrap_or(true)
        }) {
            continue;
        }
        if entries
            .iter()
            .filter(|item| item.file_name() != MARKER)
            .any(|item| std::fs::remove_file(item.path()).is_err())
        {
            continue;
        }
        drop(file);
        let _ = std::fs::remove_file(marker);
        let _ = std::fs::remove_dir(dir);
    }
    Ok(())
}
static PENDING: OnceLock<Mutex<HashMap<String, Vec<Pending>>>> = OnceLock::new();
static COMPLETED: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
static REVEALABLE: OnceLock<Mutex<Vec<(PathBuf, PathBuf)>>> = OnceLock::new();

fn reveal_allowed(path: &Path, recorded: &[(PathBuf, PathBuf)]) -> bool {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return false;
    }
    let Some((_, original)) = recorded
        .iter()
        .find(|(saved, _)| saved.as_os_str() == path.as_os_str())
    else {
        return false;
    };
    path.is_file() && std::fs::canonicalize(path).is_ok_and(|current| &current == original)
}

#[tauri::command]
pub(crate) async fn panel_download_reveal(path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    let allowed = REVEALABLE
        .get_or_init(Default::default)
        .lock()
        .map(|records| reveal_allowed(&path, &records))
        .unwrap_or(false);
    if !allowed {
        return Err("selection_required".into());
    }
    #[cfg(windows)]
    {
        let windows = std::env::var_os("WINDIR").ok_or("unavailable")?;
        let mut argument = std::ffi::OsString::from("/select,");
        argument.push(path.as_os_str());
        crate::hidden_process::hidden_command(PathBuf::from(windows).join("explorer.exe"))
            .arg(argument)
            .spawn()
            .map_err(|_| "unavailable")?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err("unavailable".into())
    }
}

fn safe_name(name: &str) -> bool {
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return false;
    }
    let Some((_, ext)) = name.rsplit_once('.') else {
        return true;
    };
    !matches!(
        ext.to_ascii_lowercase().as_str(),
        "exe"
            | "msi"
            | "bat"
            | "cmd"
            | "ps1"
            | "scr"
            | "lnk"
            | "com"
            | "dll"
            | "vbs"
            | "vbe"
            | "js"
            | "jse"
            | "wsf"
            | "wsh"
            | "hta"
            | "msp"
            | "cpl"
            | "pif"
            | "reg"
            | "psm1"
            | "psd1"
            | "appx"
            | "msix"
            | "msixbundle"
            | "appxbundle"
            | "msu"
            | "diagcab"
            | "chm"
            | "appinstaller"
            | "appref-ms"
            | "jar"
            | "url"
            | "scf"
            | "sct"
            | "shb"
            | "shs"
            | "ws"
            | "wsc"
            | "msc"
    )
}

fn publish_no_overwrite(
    staging: &Path,
    downloads: &Path,
    name: &str,
) -> io::Result<(PathBuf, u64)> {
    if !safe_name(name) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut source = std::fs::File::open(staging)?;
    let mut magic = [0_u8; 2];
    if source.read(&mut magic)? == 2 && magic == *b"MZ" {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    source.rewind()?;
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|v| v.to_str()).unwrap_or(name);
    let ext = path.extension().and_then(|v| v.to_str());
    for number in 0..10_000 {
        let next = if number == 0 {
            name.to_owned()
        } else if let Some(ext) = ext {
            format!("{stem} ({number}).{ext}")
        } else {
            format!("{stem} ({number})")
        };
        let target = downloads.join(next);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(mut out) => {
                let result = io::copy(&mut source, &mut out).and_then(|bytes| {
                    out.sync_all()?;
                    Ok(bytes)
                });
                return match result {
                    Ok(bytes) => Ok((target, bytes)),
                    Err(error) => {
                        drop(out);
                        let _ = std::fs::remove_file(&target);
                        Err(error)
                    }
                };
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::ErrorKind::AlreadyExists.into())
}

fn event_payload(
    state: &str,
    name: Option<&str>,
    path: Option<&Path>,
    size: Option<u64>,
    chat_id: Option<String>,
) -> serde_json::Value {
    serde_json::json!({"chatId": crate::panel_chat::resolve(chat_id), "state": state, "name": name, "path": path.map(|v| v.to_string_lossy().into_owned()), "size": size})
}

fn emit(
    app: &tauri::AppHandle,
    state: &str,
    name: Option<&str>,
    path: Option<&Path>,
    size: Option<u64>,
    chat_id: Option<String>,
) {
    let _ = app.emit_to(
        "main",
        "panel-browser-download",
        event_payload(state, name, path, size, chat_id),
    );
}

fn downloads_dir(app: &tauri::AppHandle) -> io::Result<PathBuf> {
    #[cfg(debug_assertions)]
    if let (Some(path), Some(root)) = (
        std::env::var_os("T68_TEST_DOWNLOAD_DIR"),
        std::env::var_os("T66_TEMP_ROOT"),
    ) {
        let path = PathBuf::from(path).canonicalize()?;
        let root = PathBuf::from(root).canonicalize()?;
        if !path.starts_with(root) {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        return Ok(path);
    }
    app.path().download_dir().map_err(io::Error::other)
}

pub(crate) fn start(
    app: &tauri::AppHandle,
    url: &str,
    destination: &mut PathBuf,
    chat_id: Option<String>,
) -> bool {
    let Some(name) = destination
        .file_name()
        .and_then(|v| v.to_str())
        .map(str::to_owned)
    else {
        emit(app, "failed", None, None, None, chat_id.clone());
        return false;
    };
    if !safe_name(&name) || name == MARKER {
        emit(app, "blocked", Some(&name), None, None, chat_id.clone());
        return false;
    }
    let Ok(root) = app.path().app_data_dir() else {
        emit(app, "failed", Some(&name), None, None, chat_id.clone());
        return false;
    };
    let stage = root
        .join("panel-download-staging")
        .join(uuid::Uuid::new_v4().to_string());
    if std::fs::create_dir_all(&stage).is_err() {
        emit(app, "failed", Some(&name), None, None, chat_id.clone());
        return false;
    }
    let marker_path = stage.join(MARKER);
    let marker = marker_options()
        .create_new(true)
        .open(&marker_path)
        .and_then(|mut file| {
            file.write_all(name.as_bytes())?;
            file.sync_all()?;
            Ok(file)
        });
    let Ok(marker) = marker else {
        let _ = std::fs::remove_file(marker_path);
        let _ = std::fs::remove_dir(&stage);
        emit(app, "failed", Some(&name), None, None, chat_id.clone());
        return false;
    };
    let staging = stage.join(&name);
    let Ok(mut pending) = PENDING.get_or_init(Default::default).lock() else {
        drop(marker);
        let _ = std::fs::remove_file(stage.join(MARKER));
        let _ = std::fs::remove_dir(&stage);
        emit(app, "failed", Some(&name), None, None, chat_id.clone());
        return false;
    };
    pending.entry(url.to_owned()).or_default().push(Pending {
        name: name.clone(),
        chat_id: chat_id.clone(),
        staging: staging.clone(),
        _marker: marker,
    });
    *destination = staging;
    emit(app, "started", Some(&name), None, None, chat_id.clone());
    true
}

pub(crate) fn finish(
    app: tauri::AppHandle,
    url: String,
    path: Option<PathBuf>,
    success: bool,
    chat_id: Option<String>,
) {
    std::thread::spawn(move || {
        // WebView2 supplies no path on failure. Two downloads may share a URL,
        // so do not guess which staging file belongs to a failure.
        if !success && path.is_none() {
            let active = PENDING
                .get_or_init(Default::default)
                .lock()
                .ok()
                .and_then(|all| all.get(&url).map(|items| !items.is_empty()))
                .unwrap_or(false);
            if active {
                emit(&app, "failed", None, None, None, chat_id.clone());
            }
            return;
        }
        let item = PENDING
            .get_or_init(Default::default)
            .lock()
            .ok()
            .and_then(|mut all| {
                let items = all.get_mut(&url)?;
                let index = if let Some(path) = path.as_ref() {
                    items.iter().position(|item| &item.staging == path)?
                } else if items.len() == 1 {
                    0
                } else {
                    return None;
                };
                Some(items.remove(index))
            });
        let Some(item) = item else {
            if success {
                emit(&app, "failed", None, None, None, chat_id.clone());
            }
            return;
        };
        let result = if success {
            downloads_dir(&app)
                .and_then(|dir| publish_no_overwrite(&item.staging, &dir, &item.name))
        } else {
            Err(io::ErrorKind::Interrupted.into())
        };
        match result {
            Ok((path, size)) => {
                if let Ok(canonical) = std::fs::canonicalize(&path) {
                    if let Ok(mut records) = REVEALABLE.get_or_init(Default::default).lock() {
                        records.push((path.clone(), canonical));
                    }
                }
                if let Ok(mut done) = COMPLETED.get_or_init(Default::default).lock() {
                    done.push(path.clone());
                }
                let saved_name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&item.name);
                emit(
                    &app,
                    "finished",
                    Some(saved_name),
                    Some(&path),
                    Some(size),
                    item.chat_id.clone(),
                );
            }
            Err(_) => emit(
                &app,
                "failed",
                Some(&item.name),
                None,
                None,
                item.chat_id.clone(),
            ),
        }
        let removed = match std::fs::remove_file(&item.staging) {
            Ok(()) => true,
            Err(error) => error.kind() == io::ErrorKind::NotFound,
        };
        if let Some(parent) = item.staging.parent() {
            drop(item._marker);
            let marker_only = std::fs::read_dir(parent)
                .map(|entries| entries.flatten().all(|entry| entry.file_name() == MARKER))
                .unwrap_or(false);
            if removed && marker_only {
                let _ = std::fs::remove_file(parent.join(MARKER));
                let _ = std::fs::remove_dir(parent);
            }
        }
    });
}

pub(crate) fn take_completed(path: &Path) -> bool {
    let Ok(mut completed) = COMPLETED.get_or_init(Default::default).lock() else {
        return false;
    };
    let Some(index) = completed.iter().position(|candidate| candidate == path) else {
        return false;
    };
    completed.remove(index);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn download_event_includes_its_chat() {
        let event = event_payload(
            "finished",
            Some("example.txt"),
            None,
            Some(5),
            Some("chat-fixture".into()),
        );
        assert_eq!(event["chatId"], "chat-fixture");
        assert_eq!(event["state"], "finished");
    }
    #[test]
    fn reveal_accepts_only_unchanged_recorded_paths() {
        let temp = tempfile::tempdir().unwrap();
        let saved = temp.path().join("saved.txt");
        let other = temp.path().join("other.txt");
        std::fs::write(&saved, b"saved").unwrap();
        std::fs::write(&other, b"other").unwrap();
        let records = vec![(saved.clone(), saved.canonicalize().unwrap())];
        assert!(reveal_allowed(&saved, &records));
        assert!(!reveal_allowed(&other, &records));
        assert!(!reveal_allowed(
            &temp.path().join("sub").join("..").join("saved.txt"),
            &records
        ));
        std::fs::rename(&saved, temp.path().join("moved.txt")).unwrap();
        assert!(!reveal_allowed(&saved, &records));
        assert!(!reveal_allowed(&temp.path().join("moved.txt"), &records));
    }
    #[test]
    fn cleans_only_recorded_inactive_downloads() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("panel-download-staging");
        let stale = root.join(uuid::Uuid::new_v4().to_string());
        let unknown = root.join(uuid::Uuid::new_v4().to_string());
        let extra = root.join(uuid::Uuid::new_v4().to_string());
        for path in [&stale, &unknown, &extra] {
            std::fs::create_dir_all(path).unwrap();
        }
        std::fs::write(stale.join(MARKER), "file.txt").unwrap();
        std::fs::write(stale.join("file.txt.crdownload"), b"partial").unwrap();
        std::fs::write(unknown.join("other.crdownload"), b"keep").unwrap();
        std::fs::write(extra.join(MARKER), "file.txt").unwrap();
        std::fs::write(extra.join("other.txt"), b"keep").unwrap();
        cleanup_staging(temp.path()).unwrap();
        assert!(!stale.exists());
        assert!(unknown.join("other.crdownload").exists());
        assert!(extra.join("other.txt").exists());
    }
    #[test]
    fn rejects_executables_and_preserves_existing_download() {
        for name in [
            "installer.EXE",
            ".exe",
            "x.msi",
            "x.bat",
            "x.cmd",
            "x.ps1",
            "x.scr",
            "x.lnk",
            "x.exe.",
            "x.exe ",
            "x:secret.txt",
        ] {
            assert!(!safe_name(name), "{name}");
        }
        let temp = tempfile::tempdir().unwrap();
        let staging = temp.path().join("stage");
        let existing = temp.path().join("data.txt");
        std::fs::write(&staging, b"new").unwrap();
        std::fs::write(&existing, b"old").unwrap();
        let (created, size) = publish_no_overwrite(&staging, temp.path(), "data.txt").unwrap();
        assert_eq!(std::fs::read(existing).unwrap(), b"old");
        assert_eq!(std::fs::read(created).unwrap(), b"new");
        assert_eq!(size, 3);
        std::fs::write(&staging, b"MZ fixture").unwrap();
        assert!(publish_no_overwrite(&staging, temp.path(), "innocent.txt").is_err());
        assert!(!temp.path().join("innocent.txt").exists());
    }

    #[test]
    fn completed_download_selection_is_one_time_and_path_scoped() {
        let temp = tempfile::tempdir().unwrap();
        let saved = temp.path().join("saved.txt");
        let other = temp.path().join("other.txt");
        COMPLETED
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .push(saved.clone());
        assert!(!take_completed(&other));
        assert!(take_completed(&saved));
        assert!(!take_completed(&saved));
    }
}
