use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

pub(crate) fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

#[derive(Clone)]
pub(crate) struct ArchiveLocation {
    pub(crate) current: std::sync::Arc<tokio::sync::RwLock<PathBuf>>,
    database: PathBuf,
    default: PathBuf,
    cloud_roots: Vec<PathBuf>,
    pub(crate) needs_initial_index: bool,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocationStatus {
    pub(crate) path: String,
    pub(crate) is_default: bool,
    pub(crate) on_cloud_sync_folder: Option<bool>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocationSetResult {
    state: &'static str,
    path: Option<String>,
    previous_path: Option<String>,
    previous_kept: Option<bool>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LocationProgress {
    copied_bytes: u64,
    total_bytes: Option<u64>,
}

#[tauri::command]
pub(crate) async fn archive_location_get(
    state: tauri::State<'_, crate::AppState>,
) -> Result<LocationStatus, String> {
    Ok(state.paths.archive.status().await)
}

#[tauri::command]
pub(crate) async fn archive_location_set<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, crate::AppState>,
    path: String,
) -> Result<LocationSetResult, String> {
    use tauri::Emitter;
    let selection = state
        .selections
        .begin(&path, crate::selection_grants::Purpose::Archive)?;
    if crate::runtime::r2_test_enabled() {
        return Ok(LocationSetResult {
            state: "unavailable",
            path: None,
            previous_path: None,
            previous_kept: None,
        });
    }
    let _ = app.emit(
        "archive-location-progress",
        LocationProgress {
            copied_bytes: 0,
            total_bytes: None,
        },
    );
    let progress_app = app.clone();
    let result = state
        .paths
        .archive
        .set_with_progress(PathBuf::from(path), move |copied_bytes, total_bytes| {
            let _ = progress_app.emit(
                "archive-location-progress",
                LocationProgress {
                    copied_bytes,
                    total_bytes,
                },
            );
        })
        .await;
    let event = if matches!(result.state, "ok" | "unchanged") {
        selection.succeed();
        "archive-location-done"
    } else {
        "archive-location-failed"
    };
    let _ = app.emit(event, result.clone());
    Ok(result)
}

#[tauri::command]
pub(crate) async fn archive_location_open(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let root = state.paths.archive.current.clone().read_owned().await;
    app.opener()
        .open_path(display_path(&root), None::<&str>)
        .map_err(|_| "폴더를 열지 못했습니다".to_string())
}

impl ArchiveLocation {
    pub(crate) fn load(
        database: PathBuf,
        home: &Path,
        cloud_roots: Vec<PathBuf>,
    ) -> io::Result<Self> {
        use rusqlite::OptionalExtension;
        let conn = rusqlite::Connection::open(&database).map_err(io::Error::other)?;
        let configured: Option<(String, bool)> = conn
            .query_row(
                "SELECT path,initialized FROM archive_location_settings WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(io::Error::other)?;
        let default = home.join("momso");
        let mut needs_initial_index = configured
            .as_ref()
            .is_none_or(|(_, initialized)| !initialized);
        let current = if let Some((path, _)) = configured {
            PathBuf::from(path)
        } else if let Some(legacy) = database
            .parent()
            .map(|path| path.join("archive"))
            .filter(|path| path.exists())
        {
            let metadata = fs::symlink_metadata(&legacy)?;
            if !metadata.is_dir() || is_link_or_reparse(&metadata) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid archive location",
                ));
            }
            let path = legacy.to_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid path encoding")
            })?;
            conn.execute(
                "INSERT INTO archive_location_settings(id,path,initialized) VALUES(1,?1,1)",
                [path],
            )
            .map_err(io::Error::other)?;
            needs_initial_index = false;
            legacy
        } else {
            if default.exists() {
                let meta = fs::symlink_metadata(&default)?;
                if !meta.is_dir()
                    || is_link_or_reparse(&meta)
                    || fs::read_dir(&default)?.next().transpose()?.is_some()
                {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "default location is not empty",
                    ));
                }
            } else {
                // The first-run folder exists even before Git is installed.
                // Its Git repository and managed files are initialized later.
                fs::create_dir(&default)?;
            }
            let path = default.to_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid path encoding")
            })?;
            conn.execute(
                "INSERT INTO archive_location_settings(id,path) VALUES(1,?1)",
                [path],
            )
            .map_err(io::Error::other)?;
            default.clone()
        };
        if !current.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid archive location",
            ));
        }
        Ok(Self {
            current: std::sync::Arc::new(tokio::sync::RwLock::new(current)),
            database,
            default,
            cloud_roots,
            needs_initial_index,
        })
    }

    pub(crate) async fn status(&self) -> LocationStatus {
        let path = self.current.read().await;
        LocationStatus {
            path: display_path(&path),
            is_default: same_location(&path, &self.default),
            on_cloud_sync_folder: if in_onedrive(&path, &self.cloud_roots) {
                Some(true)
            } else {
                is_registered_sync_root(&path)
            },
        }
    }

    #[cfg(test)]
    pub(crate) async fn set(&self, destination: PathBuf) -> &'static str {
        self.set_with_progress(destination, |_, _| {}).await.state
    }

    pub(crate) async fn set_with_progress(
        &self,
        destination: PathBuf,
        progress: impl Fn(u64, Option<u64>) + Send + 'static,
    ) -> LocationSetResult {
        let mut guard = self.current.clone().write_owned().await;
        let database = self.database.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let previous = guard.clone();
            progress(0, None);
            let actual_destination = destination
                .parent()
                .and_then(|p| p.canonicalize().ok())
                .and_then(|p| destination.file_name().map(|name| p.join(name)));
            let state = match move_archive_with_progress(&guard, &destination, &database, progress)
            {
                Ok("ok") => {
                    *guard = actual_destination.unwrap_or(destination);
                    "ok"
                }
                Ok(state) => state,
                Err(_) => "failed",
            };
            LocationSetResult {
                state,
                path: matches!(state, "ok" | "unchanged").then(|| display_path(&guard)),
                previous_path: (state == "ok").then(|| display_path(&previous)),
                previous_kept: (state == "ok").then(|| previous.exists()),
            }
        })
        .await
        .unwrap_or(LocationSetResult {
            state: "failed",
            path: None,
            previous_path: None,
            previous_kept: None,
        })
    }
}

type Inventory = std::collections::BTreeMap<PathBuf, Option<(u64, [u8; 32])>>;
fn inventory(root: &Path) -> io::Result<Inventory> {
    let mut items = Inventory::new();
    for entry in WalkDir::new(root).follow_links(false).min_depth(1) {
        let entry = entry.map_err(io::Error::other)?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if is_link_or_reparse(&metadata) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "symbolic link"));
        }
        let value = if metadata.is_dir() {
            None
        } else if metadata.is_file() {
            Some(digest(entry.path())?)
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported entry",
            ));
        };
        items.insert(
            entry
                .path()
                .strip_prefix(root)
                .map_err(io::Error::other)?
                .to_path_buf(),
            value,
        );
    }
    Ok(items)
}

#[cfg(test)]
fn move_archive(source: &Path, destination: &Path, database: &Path) -> io::Result<&'static str> {
    move_archive_with_progress(source, destination, database, |_, _| {})
}

fn move_archive_with_progress(
    source: &Path,
    destination: &Path,
    database: &Path,
    progress: impl Fn(u64, Option<u64>),
) -> io::Result<&'static str> {
    if !destination.is_absolute() || destination.file_name().is_none() {
        return Ok("invalid");
    }
    let source_meta = fs::symlink_metadata(source)?;
    if !source_meta.is_dir() || is_link_or_reparse(&source_meta) {
        return Ok("invalid");
    }
    let source = source.canonicalize()?;
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing parent"))?
        .canonicalize()?;
    let destination = parent.join(destination.file_name().unwrap());
    if let Some(data_root) = database.parent().and_then(|p| p.canonicalize().ok()) {
        let legacy = data_root.join("archive");
        if destination == legacy
            || destination.starts_with(&legacy)
            || legacy.starts_with(&destination)
        {
            return Ok("invalid");
        }
    }
    if destination == source {
        return Ok("unchanged");
    }
    if destination.starts_with(&source) || source.starts_with(&destination) {
        return Ok("invalid");
    }
    let existed = destination.exists();
    if existed {
        let metadata = fs::symlink_metadata(&destination)?;
        if !metadata.is_dir() || is_link_or_reparse(&metadata) {
            return Ok("target_not_empty");
        }
        if fs::read_dir(&destination)?.next().transpose()?.is_some() {
            return Ok("target_not_empty");
        }
    }
    let before = inventory(&source)?;
    let total = before
        .values()
        .flatten()
        .try_fold(0_u64, |sum, (size, _)| sum.checked_add(*size));
    progress(0, total);
    let staging = stage_archive_copy(&source, &parent, |copied| progress(copied, total))?;
    if inventory(staging.path())? != before || inventory(&source)? != before {
        return Err(io::Error::other("archive changed during copy"));
    }
    if existed {
        fs::remove_dir(&destination)?;
    }
    if let Err(error) = fs::rename(staging.path(), &destination) {
        if existed {
            let _ = fs::create_dir(&destination);
        }
        return Err(error);
    }
    let configured = (|| -> io::Result<()> {
        let conn = rusqlite::Connection::open(database).map_err(io::Error::other)?;
        let path = destination
            .to_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid path encoding"))?;
        conn.execute("INSERT INTO archive_location_settings(id,path,initialized) VALUES(1,?1,1) ON CONFLICT(id) DO UPDATE SET path=excluded.path,initialized=1", [path]).map_err(io::Error::other)?;
        Ok(())
    })();
    if let Err(error) = configured {
        // Remove only the verified copy, and only while it still matches what we created.
        if inventory(&destination).ok().as_ref() == Some(&before) {
            fs::remove_dir_all(&destination)?;
            if existed {
                fs::create_dir(&destination)?;
            }
        }
        return Err(error);
    }
    Ok("ok")
}

fn digest(path: &Path) -> io::Result<(u64, [u8; 32])> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .ok_or_else(|| io::Error::other("size overflow"))?;
        hash.update(&buffer[..n]);
    }
    Ok((total, hash.finalize().into()))
}

// A staging copy is not a move: the caller must coordinate all writers before and after it.
fn stage_archive_copy(
    source: &Path,
    parent: &Path,
    progress: impl FnMut(u64),
) -> io::Result<tempfile::TempDir> {
    stage_archive_copy_with_hook(source, parent, progress, || Ok(()))
}

fn stage_archive_copy_with_hook(
    source: &Path,
    parent: &Path,
    mut progress: impl FnMut(u64),
    mut after_chunk: impl FnMut() -> io::Result<()>,
) -> io::Result<tempfile::TempDir> {
    let source_meta = fs::symlink_metadata(source)?;
    if !source_meta.is_dir() || is_link_or_reparse(&source_meta) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid source",
        ));
    }
    let source = source.canonicalize()?;
    let parent = parent.canonicalize()?;
    if parent.starts_with(&source) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination inside source",
        ));
    }
    let staging = tempfile::Builder::new()
        .prefix("momso-stage-")
        .tempdir_in(parent)?;
    let mut completed = 0u64;
    for entry in WalkDir::new(&source).follow_links(false).min_depth(1) {
        let entry = entry.map_err(io::Error::other)?;
        let relative = entry
            .path()
            .strip_prefix(&source)
            .map_err(io::Error::other)?;
        let destination = staging.path().join(relative);
        let metadata = fs::symlink_metadata(entry.path())?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "reparse point"));
            }
        }
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "symbolic link"));
        }
        if metadata.is_dir() {
            fs::create_dir(&destination)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported entry",
            ));
        }
        let original = digest(entry.path())?;
        let mut input = fs::File::open(entry.path())?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)?;
        let mut buffer = [0u8; 65536];
        loop {
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n])?;
            after_chunk()?;
            completed = completed
                .checked_add(n as u64)
                .ok_or_else(|| io::Error::other("size overflow"))?;
            progress(completed);
        }
        output.sync_all()?;
        if digest(&destination)? != original || digest(entry.path())? != original {
            return Err(io::Error::other("source changed or copy mismatch"));
        }
    }
    Ok(staging)
}

fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
    }
}
fn same_location(left: &Path, right: &Path) -> bool {
    display_path(left)
        .replace('/', "\\")
        .eq_ignore_ascii_case(&display_path(right).replace('/', "\\"))
}

#[cfg(windows)]
fn is_registered_sync_root(path: &Path) -> Option<bool> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{
        ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT, ERROR_NOT_A_CLOUD_FILE,
    };
    use windows_sys::Win32::Storage::CloudFilters::{
        CfGetSyncRootInfoByPath, CF_SYNC_ROOT_INFO_BASIC,
    };
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut buffer = [0u64; 1024];
    let mut returned = 0u32;
    // The buffer is aligned and remains alive for the duration of this read-only API call.
    let result = unsafe {
        CfGetSyncRootInfoByPath(
            wide.as_ptr(),
            CF_SYNC_ROOT_INFO_BASIC,
            buffer.as_mut_ptr().cast(),
            std::mem::size_of_val(&buffer) as u32,
            &mut returned,
        )
    };
    if result >= 0 {
        return Some(true);
    }
    let code = (result as u32) & 0xffff;
    if code == ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT || code == ERROR_NOT_A_CLOUD_FILE {
        return Some(false);
    }
    log::debug!("sync-root query failed: {result:#x}");
    None
}
#[cfg(not(windows))]
fn is_registered_sync_root(_path: &Path) -> Option<bool> {
    None
}
fn in_onedrive(path: &Path, roots: &[PathBuf]) -> bool {
    let path = display_path(path).replace('/', "\\").to_lowercase();
    roots.iter().any(|root| {
        let root = display_path(root)
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase();
        !root.is_empty()
            && (path == root
                || path
                    .strip_prefix(&root)
                    .is_some_and(|rest| rest.starts_with('\\')))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn legacy_location_is_preserved_without_reindexing() {
        let temp = tempfile::tempdir().unwrap();
        let app_data = temp.path().join("app-data");
        fs::create_dir_all(app_data.join("archive")).unwrap();
        fs::write(app_data.join("archive").join("keep"), "원래 자료").unwrap();
        let db = app_data.join("settings.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE archive_location_settings(id INTEGER PRIMARY KEY, path TEXT NOT NULL, initialized INTEGER NOT NULL DEFAULT 0)").unwrap();
        let service = ArchiveLocation::load(db.clone(), temp.path(), vec![]).unwrap();
        assert_eq!(*service.current.read().await, app_data.join("archive"));
        assert!(!service.needs_initial_index);
        assert!(!service.status().await.is_default);
        assert_eq!(
            fs::read_to_string(app_data.join("archive").join("keep")).unwrap(),
            "원래 자료"
        );
        let stored: (String, bool) = conn
            .query_row(
                "SELECT path, initialized FROM archive_location_settings WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(PathBuf::from(stored.0), app_data.join("archive"));
        assert!(stored.1);
    }
    #[tokio::test]
    async fn new_location_uses_default_and_requests_initial_index() {
        let temp = tempfile::tempdir().unwrap();
        let app_data = temp.path().join("app-data");
        fs::create_dir(&app_data).unwrap();
        let db = app_data.join("settings.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE archive_location_settings(id INTEGER PRIMARY KEY, path TEXT NOT NULL, initialized INTEGER NOT NULL DEFAULT 0)").unwrap();
        let service = ArchiveLocation::load(db, temp.path(), vec![]).unwrap();
        assert_eq!(*service.current.read().await, temp.path().join("momso"));
        assert!(temp.path().join("momso").is_dir());
        assert_eq!(fs::read_dir(temp.path().join("momso")).unwrap().count(), 0);
        assert!(service.needs_initial_index);
        assert!(service.status().await.is_default);
    }
    #[tokio::test]
    async fn existing_document_remains_in_the_index_when_location_is_adopted() {
        let temp = tempfile::tempdir().unwrap();
        let app_data = temp.path().join("app-data");
        let archive = app_data.join("archive");
        fs::create_dir_all(archive.join("zettels")).unwrap();
        let db = app_data.join("momso.db");
        crate::storage::initialize(&db, &app_data.join("backups"), "test").unwrap();
        let id = crate::archive_zet::create(
            &archive,
            &db,
            crate::archive_zet::ZetCreate {
                title: "요가".into(),
                summary: "수업".into(),
                kind: "note".into(),
                body: "내용".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        let service = ArchiveLocation::load(db.clone(), temp.path(), vec![]).unwrap();
        assert_eq!(*service.current.read().await, archive);
        assert!(!service.needs_initial_index);
        let list =
            serde_json::to_value(crate::archive_zet::list(&db, None, None).unwrap()).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["id"], id);
    }
    #[test]
    fn onedrive_check_respects_component_boundaries() {
        let roots = vec![PathBuf::from(r"C:\")
            .join("Users")
            .join("person")
            .join("OneDrive")];
        assert!(in_onedrive(
            &PathBuf::from(r"c:\")
                .join("users")
                .join("PERSON")
                .join("onedrive")
                .join("folder"),
            &roots
        ));
        assert!(!in_onedrive(
            &PathBuf::from(r"c:\")
                .join("users")
                .join("person")
                .join("OneDrive-other"),
            &roots
        ));
    }

    #[test]
    fn staging_copies_everything_without_moving_source() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        for dir in [".git", "zettels", "objects", "receipts"] {
            fs::create_dir_all(source.join(dir)).unwrap();
        }
        fs::write(source.join(".git/history"), b"history").unwrap();
        fs::write(source.join("objects/raw"), b"original").unwrap();
        let mut progress = Vec::new();
        let staging = stage_archive_copy(&source, root.path(), |n| progress.push(n)).unwrap();
        assert_eq!(
            fs::read(staging.path().join(".git/history")).unwrap(),
            b"history"
        );
        assert_eq!(
            fs::read(staging.path().join("objects/raw")).unwrap(),
            b"original"
        );
        assert_eq!(fs::read(source.join("objects/raw")).unwrap(), b"original");
        assert_eq!(progress.last().copied(), Some(15));
    }

    #[test]
    fn refuses_to_stage_within_source() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        fs::create_dir(&source).unwrap();
        assert!(stage_archive_copy(&source, &source, |_| {}).is_err());
        assert_eq!(fs::read_dir(source).unwrap().count(), 0);
    }

    #[test]
    fn interrupted_copy_keeps_source_and_removes_owned_staging() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("lesson"), vec![7u8; 100_000]).unwrap();
        let error = stage_archive_copy_with_hook(
            &source,
            root.path(),
            |_| {},
            || Err(io::Error::from_raw_os_error(112)),
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(112));
        assert_eq!(fs::read(source.join("lesson")).unwrap(), vec![7u8; 100_000]);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_setting_update_keeps_source_and_original_empty_destination() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(source.join("document"), "원본").unwrap();
        let bad_database = temp.path().join("missing").join("settings.db");
        assert!(move_archive(&source, &destination, &bad_database).is_err());
        assert_eq!(fs::read_to_string(source.join("document")).unwrap(), "원본");
        assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn move_copies_and_checks_files_and_preserves_source() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("settings.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE archive_location_settings(id INTEGER PRIMARY KEY, path TEXT NOT NULL, initialized INTEGER NOT NULL DEFAULT 0)",
        )
        .unwrap();
        let source = temp.path().join("momso");
        for dir in [".git", "zettels", "objects", "receipts"] {
            fs::create_dir_all(source.join(dir)).unwrap();
        }
        fs::write(source.join("zettels/example.md"), "요가 문서").unwrap();
        conn.execute(
            "INSERT INTO archive_location_settings(id,path) VALUES(1,?1)",
            [source.to_str().unwrap()],
        )
        .unwrap();
        let manager = ArchiveLocation::load(db.clone(), temp.path(), vec![]).unwrap();
        let dest = temp.path().join("new-place");
        assert_eq!(manager.set(dest.clone()).await, "ok");
        assert_eq!(
            fs::read_to_string(source.join("zettels/example.md")).unwrap(),
            "요가 문서"
        );
        assert_eq!(
            fs::read_to_string(dest.join("zettels/example.md")).unwrap(),
            "요가 문서"
        );
        assert_eq!(
            manager.status().await.path,
            display_path(&dest.canonicalize().unwrap())
        );
        let saved: String = conn
            .query_row(
                "SELECT path FROM archive_location_settings WHERE id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(PathBuf::from(saved), dest.canonicalize().unwrap());
    }

    #[tokio::test]
    async fn move_reports_bytes_and_preserved_previous_path() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("settings.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE archive_location_settings(id INTEGER PRIMARY KEY, path TEXT NOT NULL, initialized INTEGER NOT NULL DEFAULT 0)").unwrap();
        let source = temp.path().join("momso");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("large.bin"), vec![7u8; 200_000]).unwrap();
        conn.execute(
            "INSERT INTO archive_location_settings(id,path) VALUES(1,?1)",
            [source.to_str().unwrap()],
        )
        .unwrap();
        let manager = ArchiveLocation::load(db, temp.path(), vec![]).unwrap();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = events.clone();
        let result = manager
            .set_with_progress(temp.path().join("new"), move |done, total| {
                captured.lock().unwrap().push((done, total))
            })
            .await;
        assert_eq!(result.state, "ok");
        assert_eq!(
            result.previous_path.as_deref(),
            Some(display_path(&source).as_str())
        );
        assert_eq!(result.previous_kept, Some(true));
        let events = events.lock().unwrap();
        assert_eq!(events.first(), Some(&(0, None)));
        assert!(events
            .iter()
            .any(|(done, total)| *done > 0 && *done < 200_000 && *total == Some(200_000)));
        assert_eq!(events.last(), Some(&(200_000, Some(200_000))));
        assert_eq!(
            fs::metadata(source.join("large.bin")).unwrap().len(),
            200_000
        );
    }

    #[tokio::test]
    async fn move_waits_for_existing_lease_and_rejects_nonempty_destination() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("settings.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE archive_location_settings(id INTEGER PRIMARY KEY, path TEXT NOT NULL, initialized INTEGER NOT NULL DEFAULT 0)",
        )
        .unwrap();
        let source = temp.path().join("momso");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("document"), "원본").unwrap();
        conn.execute(
            "INSERT INTO archive_location_settings(id,path) VALUES(1,?1)",
            [source.to_str().unwrap()],
        )
        .unwrap();
        let manager = ArchiveLocation::load(db, temp.path(), vec![]).unwrap();
        let legacy = temp.path().join("archive");
        fs::create_dir(&legacy).unwrap();
        assert_eq!(manager.set(legacy.clone()).await, "invalid");
        assert_eq!(fs::read_dir(&legacy).unwrap().count(), 0);
        let busy = temp.path().join("occupied");
        fs::create_dir(&busy).unwrap();
        fs::write(busy.join("keep"), "보존").unwrap();
        assert_eq!(manager.set(busy.clone()).await, "target_not_empty");
        assert_eq!(fs::read_to_string(busy.join("keep")).unwrap(), "보존");
        let lease = manager.current.clone().read_owned().await;
        let task_manager = manager.clone();
        let dest = temp.path().join("destination");
        let child_dest = dest.clone();
        let task = tokio::spawn(async move { task_manager.set(child_dest).await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!task.is_finished());
        drop(lease);
        assert_eq!(task.await.unwrap(), "ok");
        assert_eq!(fs::read_to_string(dest.join("document")).unwrap(), "원본");
    }
}
