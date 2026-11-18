use crate::{backup_profile, credentials, runtime::RuntimePaths};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum BackupError {
    #[error(transparent)]
    Profile(#[from] backup_profile::BackupProfileError),
    #[error(transparent)]
    Credential(#[from] credentials::CredentialError),
    #[error("입력한 복구 키가 이 작업공간의 키와 다릅니다")]
    RecoveryKeyMismatch,
    #[error("복원 임시 폴더를 만들지 못했습니다")]
    Io(#[from] std::io::Error),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupOverview {
    profile: backup_profile::BackupProfile,
    sync_state: &'static str,
    local_content_sha256: String,
    record_count: u64,
    object_count: u64,
    remote_snapshot: Option<serde_json::Value>,
    recovery_key_stored: bool,
    recovery_key_confirmed: bool,
}

// This legacy overview only reports local information. GitHub and object-store
// backup status are exposed by their own commands, not a remote database.
pub(crate) async fn overview(paths: &RuntimePaths) -> Result<BackupOverview, BackupError> {
    let database_path = paths.database_path.clone();
    let objects_path = paths.objects_path.clone();
    let lease = paths.archive_lease.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        let profile = backup_profile::ensure(&database_path)?;
        let local = backup_profile::local_content_state(&database_path, &objects_path)?;
        let recovery_key_stored = credentials::has_backup_identity(&profile.workspace_id)?;
        let recovery_key_confirmed =
            recovery_key_stored && profile.recovery_key_confirmed_at.is_some();
        Ok::<_, BackupError>(BackupOverview {
            profile,
            sync_state: "unavailable",
            local_content_sha256: local.sha256,
            record_count: local.record_count,
            object_count: local.object_count,
            remote_snapshot: None,
            recovery_key_stored,
            recovery_key_confirmed,
        })
    })
    .await
    .map_err(|error| std::io::Error::other(error.to_string()))?
}

pub(crate) fn export_recovery_key(paths: &RuntimePaths) -> Result<String, BackupError> {
    let profile = backup_profile::ensure(&paths.database_path)?;
    if !credentials::has_backup_identity(&profile.workspace_id)? {
        backup_profile::clear_recovery_key_confirmation(
            &paths.database_path,
            &profile.workspace_id,
        )?;
    }
    credentials::export_backup_identity(&profile.workspace_id).map_err(BackupError::from)
}

pub(crate) fn import_recovery_key(
    paths: &RuntimePaths,
    recovery_key: &str,
) -> Result<(), BackupError> {
    let profile = backup_profile::ensure(&paths.database_path)?;
    credentials::import_backup_identity(&profile.workspace_id, recovery_key)?;
    backup_profile::confirm_recovery_key(&paths.database_path, &profile.workspace_id)?;
    Ok(())
}

pub(crate) fn confirm_recovery_key(
    paths: &RuntimePaths,
    recovery_key: &str,
) -> Result<(), BackupError> {
    let profile = backup_profile::ensure(&paths.database_path)?;
    let stored = credentials::export_backup_identity(&profile.workspace_id)?;
    if stored.trim() != recovery_key.trim() {
        return Err(BackupError::RecoveryKeyMismatch);
    }
    backup_profile::confirm_recovery_key(&paths.database_path, &profile.workspace_id)?;
    Ok(())
}
