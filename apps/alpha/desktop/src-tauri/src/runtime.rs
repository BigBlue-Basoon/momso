use std::{fs, path::PathBuf};
use tauri::Manager;

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
        let data_root = app.path().app_config_dir().map_err(std::io::Error::other)?;
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
        let storage = crate::storage::initialize(
            &database_path,
            &backups_path,
            &app.package_info().version.to_string(),
        )
        .map_err(std::io::Error::other)?;
        let home = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "user profile not found")
            })?;
        let cloud_roots = ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]
            .iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .collect();
        let archive = crate::archive_location::ArchiveLocation::load(
            database_path.clone(),
            &home,
            cloud_roots,
        )?;
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
