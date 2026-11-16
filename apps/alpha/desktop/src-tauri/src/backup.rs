use crate::{backup_profile, backup_remote, credentials, runtime::RuntimePaths};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum BackupError {
    #[error(transparent)]
    Profile(#[from] backup_profile::BackupProfileError),
    #[error(transparent)]
    Remote(#[from] backup_remote::RemoteError),
    #[error(transparent)]
    Credential(#[from] credentials::CredentialError),
    #[error("입력한 복구 키가 이 작업공간의 키와 다릅니다")]
    RecoveryKeyMismatch,
    #[error("복원 임시 폴더를 만들지 못했습니다")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BackupSyncState {
    NeedsSetup,
    ReadyFirstBackup,
    Current,
    LocalChanges,
    RemoteChanges,
    RemoteOlder,
    Diverged,
    Unavailable,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupOverview {
    profile: backup_profile::BackupProfile,
    sync_state: BackupSyncState,
    local_content_sha256: String,
    record_count: u64,
    object_count: u64,
    remote_snapshot: Option<backup_remote::RemoteSnapshot>,
    recovery_key_stored: bool,
    recovery_key_confirmed: bool,
    neon_organizations: Vec<backup_remote::NeonOrganization>,
    neon_organization_selection_required: bool,
}

fn classify(
    profile: &backup_profile::BackupProfile,
    local: &backup_profile::LocalContentState,
    relation: backup_remote::HeadRelation,
) -> BackupSyncState {
    let local_changed = profile.last_completed_content_sha256.as_deref() != Some(&local.sha256);
    match relation {
        backup_remote::HeadRelation::Same => {
            if profile.last_completed_snapshot_id.is_none() {
                BackupSyncState::ReadyFirstBackup
            } else if local_changed {
                BackupSyncState::LocalChanges
            } else {
                BackupSyncState::Current
            }
        }
        backup_remote::HeadRelation::RemoteNewer => {
            if local_changed && profile.last_completed_snapshot_id.is_some() {
                BackupSyncState::Diverged
            } else {
                BackupSyncState::RemoteChanges
            }
        }
        backup_remote::HeadRelation::RemoteOlder => BackupSyncState::RemoteOlder,
        backup_remote::HeadRelation::Unrelated => BackupSyncState::Diverged,
    }
}

async fn load_local(
    paths: &RuntimePaths,
) -> Result<
    (
        backup_profile::BackupProfile,
        backup_profile::LocalContentState,
        bool,
    ),
    BackupError,
> {
    let database_path = paths.database_path.clone();
    let objects_path = paths.objects_path.clone();
    let lease = paths.archive_lease.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        let profile = backup_profile::ensure(&database_path)?;
        let local = backup_profile::local_content_state(&database_path, &objects_path)?;
        let has_key = credentials::has_backup_identity(&profile.workspace_id)?;
        Ok::<_, BackupError>((profile, local, has_key))
    })
    .await
    .map_err(|error| std::io::Error::other(error.to_string()))?
}

pub(crate) async fn overview(paths: &RuntimePaths) -> Result<BackupOverview, BackupError> {
    let (profile, local, recovery_key_stored) = load_local(paths).await?;
    let recovery_key_confirmed = recovery_key_stored && profile.recovery_key_confirmed_at.is_some();
    let organizations = if profile.neon_project_id.is_none() {
        backup_remote::neon_organizations()
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let selection_required = organizations.len() > 1;
    if !profile.is_configured() {
        return Ok(BackupOverview {
            profile,
            sync_state: BackupSyncState::NeedsSetup,
            local_content_sha256: local.sha256,
            record_count: local.record_count,
            object_count: local.object_count,
            remote_snapshot: None,
            recovery_key_stored,
            recovery_key_confirmed,
            neon_organizations: organizations,
            neon_organization_selection_required: selection_required,
        });
    }

    let remote_snapshot = match backup_remote::read_remote_head(&profile).await {
        Ok(remote) => remote,
        Err(_) => {
            return Ok(BackupOverview {
                profile,
                sync_state: BackupSyncState::Unavailable,
                local_content_sha256: local.sha256,
                record_count: local.record_count,
                object_count: local.object_count,
                remote_snapshot: None,
                recovery_key_stored,
                recovery_key_confirmed,
                neon_organizations: organizations,
                neon_organization_selection_required: selection_required,
            });
        }
    };
    let relation = backup_remote::compare_remote_head(
        &profile,
        profile.last_completed_snapshot_id.as_deref(),
        remote_snapshot
            .as_ref()
            .map(|snapshot| snapshot.snapshot_id.as_str()),
    )
    .await?;
    let sync_state = classify(&profile, &local, relation);
    Ok(BackupOverview {
        profile,
        sync_state,
        local_content_sha256: local.sha256,
        record_count: local.record_count,
        object_count: local.object_count,
        remote_snapshot,
        recovery_key_stored,
        recovery_key_confirmed,
        neon_organizations: organizations,
        neon_organization_selection_required: selection_required,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_changes_and_head_relation_are_classified_separately() {
        let mut profile = backup_profile::BackupProfile {
            workspace_id: "workspace".into(),
            github_repository: Some("owner/repo".into()),
            r2_bucket: Some("bucket".into()),
            neon_organization_id: None,
            neon_project_id: Some("project".into()),
            neon_branch_id: Some("branch".into()),
            neon_database_name: Some("momso".into()),
            neon_role_name: Some("momso_owner".into()),
            last_completed_snapshot_id: Some("snapshot".into()),
            last_completed_content_sha256: Some("old".into()),
            recovery_key_confirmed_at: Some("now".into()),
        };
        let local = backup_profile::LocalContentState {
            sha256: "new".into(),
            record_count: 0,
            object_count: 0,
        };
        assert!(matches!(
            classify(&profile, &local, backup_remote::HeadRelation::Same),
            BackupSyncState::LocalChanges
        ));
        assert!(matches!(
            classify(&profile, &local, backup_remote::HeadRelation::RemoteNewer),
            BackupSyncState::Diverged
        ));
        profile.last_completed_content_sha256 = Some("new".into());
        assert!(matches!(
            classify(&profile, &local, backup_remote::HeadRelation::RemoteNewer),
            BackupSyncState::RemoteChanges
        ));
    }
}
