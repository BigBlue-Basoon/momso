use std::{fs, path::PathBuf};
use tauri::Emitter;
use tauri::Manager;

static STARTUP_PROGRESS: std::sync::Mutex<crate::storage::StartupProgress> =
    std::sync::Mutex::new(crate::storage::StartupProgress {
        state: "checking",
        completed: 0,
        total: 4,
        available: false,
    });

pub(crate) fn publish_startup(app: &tauri::AppHandle, progress: crate::storage::StartupProgress) {
    if let Ok(mut current) = STARTUP_PROGRESS.lock() {
        *current = progress.clone();
    }
    let _ = app.emit("runtime-startup-progress", progress);
}

#[tauri::command]
pub(crate) fn runtime_startup_status() -> Result<crate::storage::StartupProgress, &'static str> {
    let _timer = crate::startup_timing::start("runtime_startup_status");
    STARTUP_PROGRESS
        .lock()
        .map(|current| current.clone())
        .map_err(|_| "unavailable")
}

const R2_TEST_IDENTIFIER: &str = "app.momso.desktop.r2-test";
#[cfg(debug_assertions)]
const R2_TEST_BUCKET: Option<&str> = option_env!("MOMSO_R2_TEST_ALLOWED_BUCKET");
#[cfg(not(debug_assertions))]
const R2_TEST_BUCKET: Option<&str> = None;
#[cfg(debug_assertions)]
const R2_TEST_FOLDER: Option<&str> = option_env!("MOMSO_R2_TEST_FOLDER");
#[cfg(not(debug_assertions))]
const R2_TEST_FOLDER: Option<&str> = None;

pub(crate) fn r2_test_enabled() -> bool {
    cfg!(debug_assertions) && std::env::var_os("MOMSO_R2_TEST_ROOT").is_some()
}

pub(crate) fn r2_test_target_matches(provider: &str, bucket: &str, allowed: Option<&str>) -> bool {
    provider == "cloudflare" && allowed.is_some_and(|name| !name.is_empty() && bucket == name)
}

pub(crate) fn r2_test_target(provider: &str, bucket: &str) -> bool {
    cfg!(debug_assertions) && r2_test_target_matches(provider, bucket, R2_TEST_BUCKET)
}

pub(crate) fn r2_test_connect_matches(
    provider: &str,
    automatic: bool,
    bucket: &str,
    allowed: Option<&str>,
) -> bool {
    automatic && r2_test_target_matches(provider, bucket, allowed)
}

pub(crate) fn r2_test_connect(provider: &str, automatic: bool, bucket: &str) -> bool {
    cfg!(debug_assertions) && r2_test_connect_matches(provider, automatic, bucket, R2_TEST_BUCKET)
}

fn r2_test_root(
    identifier: &str,
    requested: Option<PathBuf>,
    debug: bool,
    expected_folder: Option<&str>,
) -> std::io::Result<Option<PathBuf>> {
    if identifier == R2_TEST_IDENTIFIER || (debug && requested.is_some()) {
        if !debug || identifier != R2_TEST_IDENTIFIER {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid test app identifier",
            ));
        }
        let root = requested.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing test root")
        })?;
        if expected_folder
            .is_none_or(|name| !name.starts_with(".momso-r2-live-") || name.contains(['/', '\\']))
            || !root.is_absolute()
            || root
                .file_name()
                .is_none_or(|name| Some(name) != expected_folder.map(std::ffi::OsStr::new))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid test root",
            ));
        }
        return Ok(Some(root));
    }
    Ok(None)
}

fn prepare_test_root(root: &std::path::Path) -> std::io::Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(root) {
        if !metadata.is_dir() || is_reparse(&metadata) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid test root",
            ));
        }
    } else {
        fs::create_dir(root)?;
    }
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(std::io::Error::other)?;
        if is_reparse(&fs::symlink_metadata(entry.path())?) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid test path",
            ));
        }
    }
    Ok(())
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

#[derive(Clone)]
pub(crate) struct RuntimePaths {
    pub(crate) data_root: PathBuf,
    pub(crate) database_path: PathBuf,
    pub(crate) objects_path: PathBuf,
    pub(crate) exports_path: PathBuf,
    pub(crate) backups_path: PathBuf,
    pub(crate) logs_path: PathBuf,
    pub(crate) archive: crate::archive_location::ArchiveLocation,
    pub(crate) archive_lease: Option<std::sync::Arc<tokio::sync::OwnedRwLockReadGuard<PathBuf>>>,
}

impl RuntimePaths {
    pub(crate) fn prepare(
        app: &tauri::AppHandle,
    ) -> Result<(Self, crate::storage::StorageStatus), std::io::Error> {
        let _timer = crate::startup_timing::start("runtime_prepare");
        let test_root = r2_test_root(
            &app.config().identifier,
            std::env::var_os("MOMSO_R2_TEST_ROOT").map(PathBuf::from),
            cfg!(debug_assertions),
            R2_TEST_FOLDER,
        )?;
        if let Some(root) = &test_root {
            prepare_test_root(root)?;
        }
        let data_root = match &test_root {
            Some(root) => root.join("data"),
            None => app.path().app_config_dir().map_err(std::io::Error::other)?,
        };
        let logs_path = app.path().app_log_dir().map_err(std::io::Error::other)?;
        let backups_path = data_root.join("backups");
        for directory in [
            &data_root,
            &data_root.join("exports"),
            &backups_path,
            &logs_path,
        ] {
            fs::create_dir_all(directory)?;
        }
        let database_path = data_root.join("momso.db");
        let storage_timer = crate::startup_timing::start("storage_initialize");
        let storage = crate::storage::initialize_with_progress(
            &database_path,
            &backups_path,
            &app.package_info().version.to_string(),
            |progress| {
                publish_startup(app, progress);
            },
        )
        .map_err(std::io::Error::other)?;
        drop(storage_timer);
        let home = test_root
            .as_ref()
            .map(|root| root.join("home"))
            .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "user profile not found")
            })?;
        if test_root.is_some() {
            fs::create_dir_all(&home)?;
        }
        let cloud_roots = ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]
            .iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .collect();
        let location_timer = crate::startup_timing::start("archive_location_load");
        let archive = crate::archive_location::ArchiveLocation::load(
            database_path.clone(),
            &home,
            cloud_roots,
        )?;
        drop(location_timer);
        if test_root.is_some()
            && *archive.current.try_read().map_err(std::io::Error::other)? != home.join("momso")
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid test archive",
            ));
        }
        let objects_path = archive
            .current
            .try_read()
            .map_err(std::io::Error::other)?
            .join("objects");
        let paths = Self {
            objects_path,
            exports_path: data_root.join("exports"),
            backups_path,
            logs_path,
            database_path,
            data_root,
            archive,
            archive_lease: None,
        };
        Ok((paths, storage))
    }

    pub(crate) async fn scoped(&self) -> Self {
        let lease = std::sync::Arc::new(self.archive.current.clone().read_owned().await);
        let mut snapshot = self.clone();
        snapshot.objects_path = lease.join("objects");
        snapshot.archive_lease = Some(lease);
        snapshot
    }
}

#[cfg(test)]
mod isolation_tests {
    use super::*;

    #[test]
    fn startup_status_returns_latest_progress_shape() {
        let state = serde_json::to_value(runtime_startup_status().unwrap()).unwrap();
        assert!(state["state"].is_string());
        assert!(state["completed"].is_u64());
        assert_eq!(state["total"], 4);
        assert!(state["available"].is_boolean());
    }

    #[test]
    fn isolated_root_requires_debug_and_distinct_identifier() {
        let folder = ".momso-r2-live-fixture";
        let root = std::env::temp_dir().join(folder);
        assert_eq!(
            r2_test_root(R2_TEST_IDENTIFIER, Some(root.clone()), true, Some(folder)).unwrap(),
            Some(root.clone())
        );
        assert!(r2_test_root("app.momso.desktop", Some(root.clone()), true, Some(folder)).is_err());
        assert!(r2_test_root(R2_TEST_IDENTIFIER, None, true, Some(folder)).is_err());
        assert!(r2_test_root(R2_TEST_IDENTIFIER, Some(root.clone()), true, None).is_err());
        assert!(r2_test_root(
            R2_TEST_IDENTIFIER,
            Some(root.clone()),
            true,
            Some("../outside")
        )
        .is_err());
        assert!(r2_test_root(R2_TEST_IDENTIFIER, Some(root.clone()), false, Some(folder)).is_err());
        assert!(r2_test_root(
            R2_TEST_IDENTIFIER,
            Some(PathBuf::from("relative")),
            true,
            Some(folder)
        )
        .is_err());
        assert_eq!(
            r2_test_root("app.momso.desktop", Some(root), false, Some(folder)).unwrap(),
            None
        );
    }

    #[test]
    fn test_target_is_build_time_bound_and_fails_closed_without_it() {
        match R2_TEST_BUCKET {
            Some(allowed) => {
                assert!(r2_test_target("cloudflare", allowed));
                assert!(r2_test_connect("cloudflare", true, allowed));
                assert!(!r2_test_connect("cloudflare", false, allowed));
                assert!(!r2_test_target(
                    "cloudflare",
                    "someone-elses-private-bucket"
                ));
            }
            None => assert!(!r2_test_target("cloudflare", "momso-fixture")),
        }
    }

    #[test]
    fn preparing_test_root_keeps_other_data_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let preview = temp.path().join("preview");
        fs::create_dir(&preview).unwrap();
        fs::write(preview.join("document"), b"preview data").unwrap();
        let root = temp.path().join(".momso-r2-live-fixture");
        prepare_test_root(&root).unwrap();
        fs::create_dir(root.join("data")).unwrap();
        fs::write(root.join("data").join("fixture"), b"test data").unwrap();
        prepare_test_root(&root).unwrap();
        assert_eq!(fs::read(preview.join("document")).unwrap(), b"preview data");
        assert_eq!(
            fs::read(root.join("data").join("fixture")).unwrap(),
            b"test data"
        );
    }
}
