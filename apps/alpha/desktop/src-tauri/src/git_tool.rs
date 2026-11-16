use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        OnceLock,
    },
};
use tauri::{AppHandle, Emitter, Manager, State};

const URL: &str = "https://github.com/git-for-windows/git/releases/download/v2.56.1.windows.1/MinGit-2.56.1-64-bit.zip";
const SHA256: &str = "4a92850b6e917da4fd010669b614b55dccf5b0ec333a534cc4be4700dfefdafd";
const MAX_DOWNLOAD: usize = 100_000_000;
const MAX_EXTRACTED: u64 = 300_000_000;
static DATA_ROOT: OnceLock<PathBuf> = OnceLock::new();
static INSTALLING: AtomicBool = AtomicBool::new(false);

pub(crate) fn configure(root: PathBuf) {
    let _ = DATA_ROOT.set(root);
}

fn managed_path(root: &Path) -> PathBuf {
    root.join("git-tools").join("mingit-2.56.1")
}

fn usable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let mut command = Command::new(path);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child.wait_with_output().is_ok_and(|out| {
                    out.status.success()
                        && supports_git_version(&String::from_utf8_lossy(&out.stdout))
                })
            }
            Ok(None) if start.elapsed() < std::time::Duration::from_secs(3) => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

fn supports_git_version(output: &str) -> bool {
    let Some(version) = output.strip_prefix("git version ") else {
        return false;
    };
    let mut parts = version.split('.');
    let major = parts.next().and_then(|s| s.parse::<u32>().ok());
    let minor = parts.next().and_then(|s| s.parse::<u32>().ok());
    matches!((major, minor), (Some(2), Some(28..)) | (Some(3..), Some(_)))
}

fn choose(system: Option<PathBuf>, managed: Option<PathBuf>) -> Option<(PathBuf, bool)> {
    system
        .map(|path| (path, false))
        .or_else(|| managed.map(|path| (path, true)))
}

pub(crate) fn resolve() -> Option<(PathBuf, bool)> {
    let system = system_git();
    if system.is_some() {
        return choose(system, None);
    }
    let managed = DATA_ROOT.get().and_then(|root| managed_git(root));
    choose(system, managed)
}

fn managed_git(root: &Path) -> Option<PathBuf> {
    let dir = managed_path(root);
    let marker_path = dir.join(".momso-sha256");
    let metadata = fs::symlink_metadata(&marker_path).ok()?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() != SHA256.len() as u64
    {
        return None;
    }
    let marker = fs::read_to_string(marker_path).ok()?;
    let path = dir.join("cmd/git.exe");
    (marker == SHA256 && usable(&path)).then_some(path)
}

fn system_git() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH").and_then(|value| {
        std::env::split_paths(&value)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join("git.exe"))
            .find(|path| usable(path))
    });
    on_path.or_else(|| {
        ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .into_iter()
            .filter_map(|name| std::env::var_os(name).map(|dir| (name, PathBuf::from(dir))))
            .map(|(name, dir)| {
                if name == "LOCALAPPDATA" {
                    dir.join("Programs/Git/cmd/git.exe")
                } else {
                    dir.join("Git/cmd/git.exe")
                }
            })
            .find(|path| usable(path))
    })
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    provider: &'static str,
    state: &'static str,
    stage: &'static str,
    completed: u64,
    total: Option<u64>,
}

fn notify(
    app: &AppHandle,
    state: &'static str,
    stage: &'static str,
    completed: u64,
    total: Option<u64>,
) {
    let _ = app.emit(
        "git-install-changed",
        Progress {
            provider: "git",
            state,
            stage,
            completed,
            total,
        },
    );
}

#[tauri::command]
pub(crate) fn git_install_status() -> &'static str {
    if INSTALLING.load(Ordering::Acquire) {
        "inProgress"
    } else if resolve().is_some() {
        "installed"
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        "approvalRequired"
    } else {
        "unavailable"
    }
}

struct InstallGuard;
impl Drop for InstallGuard {
    fn drop(&mut self) {
        INSTALLING.store(false, Ordering::Release);
    }
}

fn installation_gate(existing: bool, approved: bool, supported: bool) -> Option<&'static str> {
    if !existing && !supported {
        Some("unavailable")
    } else if !existing && !approved {
        Some("approvalRequired")
    } else {
        None
    }
}

#[tauri::command]
pub(crate) fn git_install(
    app: AppHandle,
    state: State<'_, crate::AppState>,
    approved: bool,
) -> &'static str {
    let existing = resolve().is_some();
    if let Some(state) = installation_gate(
        existing,
        approved,
        cfg!(all(windows, target_arch = "x86_64")),
    ) {
        return state;
    }
    if INSTALLING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return "inProgress";
    }
    let paths = state.paths.clone();
    notify(
        &app,
        "inProgress",
        if existing {
            "initializing"
        } else {
            "downloading"
        },
        0,
        None,
    );
    tauri::async_runtime::spawn(async move {
        let _guard = InstallGuard;
        let outcome = if existing {
            Ok(())
        } else {
            download_and_install(&app, &paths.data_root).await
        };
        let outcome = match outcome {
            Ok(()) => {
                notify(&app, "inProgress", "initializing", 0, None);
                initialize_archive(&app, paths).await
            }
            Err(code) => Err(code),
        };
        match outcome {
            Ok(()) => notify(&app, "installed", "complete", 0, None),
            Err(code) => notify(&app, "installFailed", code, 0, None),
        }
    });
    "inProgress"
}

async fn download_and_install(app: &AppHandle, root: &Path) -> Result<(), &'static str> {
    let bytes =
        fetch(|completed, total| notify(app, "inProgress", "downloading", completed, total))
            .await?;
    notify(app, "inProgress", "verifying", bytes.len() as u64, None);
    let root = root.to_path_buf();
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if format!("{:x}", Sha256::digest(&bytes)) != SHA256 {
            return Err("checksumMismatch");
        }
        notify(&app, "inProgress", "extracting", 0, None);
        install_verified(&root, &bytes, SHA256)
    })
    .await
    .map_err(|_| "installFailed")?
}

async fn fetch(mut progress: impl FnMut(u64, Option<u64>)) -> Result<Vec<u8>, &'static str> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.url().scheme() != "https" || attempt.previous().len() >= 10 {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| "networkFailed")?;
    let mut response = client.get(URL).send().await.map_err(|_| "networkFailed")?;
    if !response.status().is_success() {
        return Err("networkFailed");
    }
    let total = response.content_length();
    if total.is_some_and(|n| n > MAX_DOWNLOAD as u64) {
        return Err("sizeLimit");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "networkFailed")? {
        if bytes.len().saturating_add(chunk.len()) > MAX_DOWNLOAD {
            return Err("sizeLimit");
        }
        bytes.extend_from_slice(&chunk);
        progress(bytes.len() as u64, total);
    }
    Ok(bytes)
}

fn install_verified(root: &Path, bytes: &[u8], expected: &str) -> Result<(), &'static str> {
    if format!("{:x}", Sha256::digest(bytes)) != expected {
        return Err("checksumMismatch");
    }
    let parent = root.join("git-tools");
    fs::create_dir_all(&parent).map_err(|_| "filesystemFailed")?;
    let parent_metadata = fs::symlink_metadata(&parent).map_err(|_| "filesystemFailed")?;
    if !parent_metadata.is_dir() || parent_metadata.file_type().is_symlink() {
        return Err("filesystemFailed");
    }
    let destination = managed_path(root);
    if destination.exists() {
        return Err("destinationExists");
    }
    let temporary = tempfile::Builder::new()
        .prefix(".mingit-stage-")
        .tempdir_in(&parent)
        .map_err(|_| "filesystemFailed")?;
    let tree = temporary.path().join("mingit");
    fs::create_dir(&tree).map_err(|_| "filesystemFailed")?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| "invalidArchive")?;
    if archive.len() > 10_000 {
        return Err("sizeLimit");
    }
    let mut total = 0u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|_| "invalidArchive")?;
        let name = entry.enclosed_name().ok_or("invalidArchive")?;
        if entry.name().contains('\\')
            || entry.name().contains(':')
            || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
        {
            return Err("invalidArchive");
        }
        total = total.checked_add(entry.size()).ok_or("sizeLimit")?;
        if total > MAX_EXTRACTED {
            return Err("sizeLimit");
        }
        let path = tree.join(name);
        if entry.is_dir() {
            fs::create_dir_all(path).map_err(|_| "filesystemFailed")?;
        } else {
            fs::create_dir_all(path.parent().ok_or("invalidArchive")?)
                .map_err(|_| "filesystemFailed")?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(|_| "filesystemFailed")?;
            let declared = entry.size();
            let written = io::copy(&mut entry.take(declared.saturating_add(1)), &mut output)
                .map_err(|_| "invalidArchive")?;
            if written != declared {
                return Err("invalidArchive");
            }
        }
    }
    if !tree.join("cmd/git.exe").is_file() {
        return Err("invalidArchive");
    }
    let mut marker = File::create(tree.join(".momso-sha256")).map_err(|_| "filesystemFailed")?;
    marker
        .write_all(expected.as_bytes())
        .map_err(|_| "filesystemFailed")?;
    drop(marker);
    fs::rename(&tree, destination).map_err(|_| "filesystemFailed")?;
    Ok(())
}

async fn initialize_archive(
    app: &AppHandle,
    paths: crate::runtime::RuntimePaths,
) -> Result<(), &'static str> {
    let resources = app.path().resource_dir().map_err(|_| "initializeFailed")?;
    let lease = paths.archive.current.clone().write_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        crate::archive::initialize(&lease, &resources).map_err(|_| "initializeFailed")?;
        if paths.archive.needs_initial_index {
            crate::archive::reindex(&lease, &paths.database_path)
                .map_err(|_| "initializeFailed")?;
            let connection = crate::storage::open_writable_connection(&paths.database_path)
                .map_err(|_| "initializeFailed")?;
            connection
                .execute(
                    "UPDATE archive_location_settings SET initialized=1 WHERE id=1",
                    [],
                )
                .map_err(|_| "initializeFailed")?;
        }
        Ok(())
    })
    .await
    .map_err(|_| "initializeFailed")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn fixture(name: &str) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(b"fixture").unwrap();
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn existing_system_git_has_priority() {
        let system = PathBuf::from("system");
        assert_eq!(
            choose(Some(system.clone()), Some(PathBuf::from("managed"))),
            Some((system, false))
        );
    }

    #[test]
    fn actual_system_git_wins_when_available() {
        if let Some(system) = system_git() {
            assert_eq!(resolve(), Some((system, false)));
        }
    }

    #[test]
    fn rejects_git_too_old_for_initial_branch() {
        assert!(!supports_git_version("git version 2.27.0.windows.1"));
        assert!(supports_git_version("git version 2.28.0.windows.1"));
        assert!(supports_git_version("git version 2.56.1.windows.1"));
        assert!(!supports_git_version("unexpected output"));
    }

    #[test]
    fn download_requires_explicit_approval() {
        assert_eq!(
            installation_gate(false, false, true),
            Some("approvalRequired")
        );
        assert_eq!(installation_gate(false, true, false), Some("unavailable"));
        assert_eq!(installation_gate(false, true, true), None);
        assert_eq!(installation_gate(true, false, true), None);
    }

    #[test]
    fn wrong_digest_is_rejected_without_writing() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            install_verified(root.path(), &fixture("cmd/git.exe"), "wrong"),
            Err("checksumMismatch")
        );
        assert!(!root.path().join("git-tools").exists());
    }

    #[test]
    fn verified_archive_is_staged_and_does_not_replace_existing() {
        let root = tempfile::tempdir().unwrap();
        let bytes = fixture("cmd/git.exe");
        let hash = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(install_verified(root.path(), &bytes, &hash), Ok(()));
        assert_eq!(
            fs::read(managed_path(root.path()).join("cmd/git.exe")).unwrap(),
            b"fixture"
        );
        assert_eq!(
            install_verified(root.path(), &bytes, &hash),
            Err("destinationExists")
        );
    }

    #[test]
    fn traversal_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let bytes = fixture("../escape");
        let hash = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(
            install_verified(root.path(), &bytes, &hash),
            Err("invalidArchive")
        );
        assert!(!root.path().join("escape").exists());
    }

    #[tokio::test]
    #[ignore = "downloads the official MinGit release without changing app data"]
    async fn official_download_matches_and_runs() {
        let bytes = fetch(|_, _| {}).await.unwrap();
        let root = tempfile::tempdir().unwrap();
        assert_eq!(install_verified(root.path(), &bytes, SHA256), Ok(()));
        let managed = managed_git(root.path()).expect("verified managed Git");
        assert_eq!(choose(None, Some(managed.clone())), Some((managed, true)));
        root.close().expect("remove isolated test installation");
    }
}
