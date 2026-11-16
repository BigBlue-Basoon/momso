use crate::{
    backup_archive, backup_profile, backup_remote, credentials, dictionary, runtime::RuntimePaths,
    storage,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Error)]
pub(crate) enum BackupError {
    #[error(transparent)]
    Profile(#[from] backup_profile::BackupProfileError),
    #[error(transparent)]
    Remote(#[from] backup_remote::RemoteError),
    #[error(transparent)]
    Operation(#[from] crate::operations::OperationError),
    #[error(transparent)]
    Archive(#[from] backup_archive::ArchiveError),
    #[error(transparent)]
    Credential(#[from] credentials::CredentialError),
    #[error(transparent)]
    Dictionary(#[from] dictionary::DictionaryError),
    #[error(transparent)]
    Storage(#[from] storage::StorageError),
    #[error("백업 메타데이터를 만들지 못했습니다")]
    Json(#[from] serde_json::Error),
    #[error("백업 전에 GitHub, Cloudflare, Neon 공간을 모두 준비해 주세요")]
    NotConfigured,
    #[error("마지막 백업 뒤에 바뀐 기록이 없습니다")]
    NoChanges,
    #[error("원격 백업이 마지막 확인 뒤 변경되었습니다. 먼저 다시 비교해 주세요")]
    RemoteChanged,
    #[error("복원할 원격 백업이 없습니다")]
    NothingToRestore,
    #[error("선택한 기존 백업을 다시 찾을 수 없습니다")]
    BackupCandidateNotFound,
    #[error("복원 계획이 오래되었습니다. 원격과 로컬을 다시 비교해 주세요")]
    StaleRestorePlan,
    #[error("로컬 변경을 덮어쓰려면 확인이 필요합니다")]
    RestoreConfirmationRequired,
    #[error("복원된 기록의 내용 해시가 원격 연결 정보와 다릅니다")]
    RestoredContentMismatch,
    #[error("복구 키를 외부에 보관한 뒤 같은 키를 다시 입력해 확인해 주세요")]
    RecoveryKeyConfirmationRequired,
    #[error("입력한 복구 키가 이 작업공간의 키와 다릅니다")]
    RecoveryKeyMismatch,
    #[error("시스템 시간을 확인할 수 없습니다")]
    InvalidSystemTime,
    #[error("사용자가 백업 또는 복원을 중단했습니다")]
    Cancelled,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupResult {
    snapshot_id: String,
    encrypted_archive_sha256: String,
    recovery_key: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestorePlan {
    plan_digest: String,
    remote_snapshot: backup_remote::RemoteSnapshot,
    local_content_sha256: String,
    sync_state: BackupSyncState,
    requires_overwrite_confirmation: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestoreResult {
    restored_snapshot_id: String,
    safety_backup_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotManifest<'a> {
    format: &'static str,
    workspace_id: &'a str,
    snapshot_id: &'a str,
    parent_snapshot_id: Option<&'a str>,
    content_sha256: &'a str,
    encrypted_archive_sha256: &'a str,
    r2_object_key: &'a str,
    record_count: u64,
    object_count: u64,
    created_at_unix_ms: u128,
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

pub(crate) async fn prepare_resources(
    paths: &RuntimePaths,
    selected_neon_organization_id: Option<&str>,
) -> Result<BackupOverview, BackupError> {
    backup_remote::prepare_resources(&paths.database_path, selected_neon_organization_id).await?;
    overview(paths).await
}

pub(crate) async fn discover_existing() -> Result<Vec<backup_remote::BackupCandidate>, BackupError>
{
    backup_remote::discover_backups()
        .await
        .map_err(BackupError::from)
}

pub(crate) async fn adopt_existing(
    paths: &RuntimePaths,
    repository: &str,
) -> Result<BackupOverview, BackupError> {
    let candidate = backup_remote::find_backup(repository)
        .await?
        .ok_or(BackupError::BackupCandidateNotFound)?;
    let database_path = paths.database_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        backup_profile::adopt_remote(
            &database_path,
            backup_profile::AdoptedBackupProfile {
                workspace_id: candidate.workspace_id,
                github_repository: candidate.repository,
                r2_bucket: candidate.r2_bucket,
                neon_organization_id: candidate.neon_organization_id,
                neon_project_id: candidate.neon_project_id,
                neon_branch_id: candidate.neon_branch_id,
                neon_database_name: candidate.neon_database_name,
                neon_role_name: candidate.neon_role_name,
            },
        )
    })
    .await
    .map_err(|error| std::io::Error::other(error.to_string()))??;
    overview(paths).await
}

fn sha256_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn ensure_not_cancelled(cancellation: &CancellationToken) -> Result<(), BackupError> {
    if cancellation.is_cancelled() {
        Err(BackupError::Cancelled)
    } else {
        Ok(())
    }
}

pub(crate) async fn run_backup(paths: &RuntimePaths) -> Result<BackupResult, BackupError> {
    run_backup_cancellable(paths, CancellationToken::new(), |_, _, _| {}).await
}

pub(crate) async fn run_backup_cancellable<Progress>(
    paths: &RuntimePaths,
    cancellation: CancellationToken,
    progress: Progress,
) -> Result<BackupResult, BackupError>
where
    Progress: Fn(&str, u64, u64) + Send + Sync,
{
    progress("checking_local", 0, 5);
    ensure_not_cancelled(&cancellation)?;
    let (profile, local, recovery_key_stored) = load_local(paths).await?;
    if !profile.is_configured() {
        return Err(BackupError::NotConfigured);
    }
    if !recovery_key_stored || profile.recovery_key_confirmed_at.is_none() {
        return Err(BackupError::RecoveryKeyConfirmationRequired);
    }
    progress("comparing_remote", 1, 5);
    ensure_not_cancelled(&cancellation)?;
    let remote_head = backup_remote::read_remote_head(&profile).await?;
    if remote_head.as_ref().map(|head| head.snapshot_id.as_str())
        != profile.last_completed_snapshot_id.as_deref()
    {
        return Err(BackupError::RemoteChanged);
    }
    if profile.last_completed_content_sha256.as_deref() == Some(&local.sha256) {
        return Err(BackupError::NoChanges);
    }

    let snapshot_id = Uuid::new_v4().to_string();
    backup_profile::begin_snapshot(&paths.database_path, &profile, &snapshot_id, &local)?;
    let outcome = run_backup_steps(
        paths,
        &profile,
        &snapshot_id,
        cancellation.clone(),
        &progress,
    )
    .await;
    if outcome.is_err() {
        let failure_code = if cancellation.is_cancelled() {
            "cancelled"
        } else {
            "remote_backup_failed"
        };
        let _ = backup_profile::mark_failed(&paths.database_path, &snapshot_id, failure_code);
    }
    outcome
}

async fn run_backup_steps<Progress>(
    paths: &RuntimePaths,
    profile: &backup_profile::BackupProfile,
    snapshot_id: &str,
    cancellation: CancellationToken,
    progress: &Progress,
) -> Result<BackupResult, BackupError>
where
    Progress: Fn(&str, u64, u64) + Send + Sync,
{
    progress("creating_archive", 2, 5);
    ensure_not_cancelled(&cancellation)?;
    let database_path = paths.database_path.clone();
    let objects_path = paths.objects_path.clone();
    let lease = paths.archive_lease.clone();
    let backups_path = paths.backups_path.clone();
    let workspace_id = profile.workspace_id.clone();
    let snapshot_id_owned = snapshot_id.to_owned();
    let (archive, snapshot_content) = tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        let archive = backup_archive::prepare(
            &database_path,
            &objects_path,
            &backups_path,
            &workspace_id,
            &snapshot_id_owned,
        )?;
        let snapshot_content = backup_profile::local_content_state_from_inventory(
            &archive.snapshot_database_path,
            &archive.object_files,
        )?;
        Ok::<_, BackupError>((archive, snapshot_content))
    })
    .await
    .map_err(|error| std::io::Error::other(error.to_string()))??;
    ensure_not_cancelled(&cancellation)?;
    backup_profile::update_snapshot_content(&paths.database_path, snapshot_id, &snapshot_content)?;
    backup_profile::mark_uploading(&paths.database_path, snapshot_id)?;
    let r2_object_key = format!(
        "workspaces/{}/snapshots/{snapshot_id}.tar.age",
        profile.workspace_id
    );
    let created_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| BackupError::InvalidSystemTime)?
        .as_millis();
    let manifest = SnapshotManifest {
        format: "momso-user-owned-backup-v1",
        workspace_id: &profile.workspace_id,
        snapshot_id,
        parent_snapshot_id: profile.last_completed_snapshot_id.as_deref(),
        content_sha256: &snapshot_content.sha256,
        encrypted_archive_sha256: &archive.encrypted_sha256,
        r2_object_key: &r2_object_key,
        record_count: snapshot_content.record_count,
        object_count: snapshot_content.object_count,
        created_at_unix_ms,
    };
    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    let manifest_sha256 = sha256_text(&manifest_json);
    progress("uploading_archive", 3, 5);
    let upload = backup_remote::upload_r2(
        profile,
        &r2_object_key,
        &archive.encrypted_path,
        cancellation.clone(),
    )
    .await;
    if cancellation.is_cancelled() {
        return Err(BackupError::Cancelled);
    }
    upload?;

    progress("writing_metadata", 4, 5);
    ensure_not_cancelled(&cancellation)?;
    let database_path = archive.snapshot_database_path.clone();
    let dictionary_json = tauri::async_runtime::spawn_blocking(move || {
        let dictionary = dictionary::list(&database_path)?;
        serde_json::to_string_pretty(&dictionary).map_err(BackupError::from)
    })
    .await
    .map_err(|error| std::io::Error::other(error.to_string()))??;
    let github_commit_sha = backup_remote::push_metadata(
        profile,
        &paths.data_root,
        snapshot_id,
        &dictionary_json,
        &manifest_json,
    )
    .await?;
    backup_remote::advance_remote_head(
        profile,
        backup_remote::NewRemoteSnapshot {
            snapshot_id,
            parent_snapshot_id: profile.last_completed_snapshot_id.as_deref(),
            content_sha256: &snapshot_content.sha256,
            encrypted_archive_sha256: &archive.encrypted_sha256,
            manifest_sha256: &manifest_sha256,
            github_commit_sha: &github_commit_sha,
            r2_object_key: &r2_object_key,
            record_count: snapshot_content.record_count,
            object_count: snapshot_content.object_count,
        },
    )
    .await?;
    backup_profile::complete_snapshot(
        &paths.database_path,
        profile,
        snapshot_id,
        &snapshot_content.sha256,
        backup_profile::CompletedSnapshot {
            encrypted_archive_sha256: &archive.encrypted_sha256,
            manifest_sha256: &manifest_sha256,
            github_commit_sha: &github_commit_sha,
            r2_object_key: &r2_object_key,
        },
    )?;
    progress("complete", 5, 5);
    log::info!("User-owned backup completed");
    Ok(BackupResult {
        snapshot_id: snapshot_id.to_owned(),
        encrypted_archive_sha256: archive.encrypted_sha256,
        recovery_key: archive.new_recovery_key,
    })
}

fn restore_digest(
    profile: &backup_profile::BackupProfile,
    local: &backup_profile::LocalContentState,
    remote: &backup_remote::RemoteSnapshot,
) -> String {
    sha256_text(&format!(
        "momso-restore-plan-v1\n{}\n{}\n{}\n{}\n{}",
        profile.workspace_id,
        profile
            .last_completed_snapshot_id
            .as_deref()
            .unwrap_or("none"),
        local.sha256,
        remote.snapshot_id,
        remote.encrypted_archive_sha256,
    ))
}

pub(crate) async fn plan_restore(paths: &RuntimePaths) -> Result<RestorePlan, BackupError> {
    let (profile, local, _) = load_local(paths).await?;
    if !profile.is_configured() {
        return Err(BackupError::NotConfigured);
    }
    let remote = backup_remote::read_remote_head(&profile)
        .await?
        .ok_or(BackupError::NothingToRestore)?;
    let relation = backup_remote::compare_remote_head(
        &profile,
        profile.last_completed_snapshot_id.as_deref(),
        Some(&remote.snapshot_id),
    )
    .await?;
    let sync_state = classify(&profile, &local, relation);
    let local_changed = profile.last_completed_content_sha256.as_deref() != Some(&local.sha256);
    Ok(RestorePlan {
        plan_digest: restore_digest(&profile, &local, &remote),
        remote_snapshot: remote,
        local_content_sha256: local.sha256,
        sync_state,
        requires_overwrite_confirmation: local_changed
            || matches!(
                sync_state,
                BackupSyncState::RemoteOlder | BackupSyncState::Diverged
            ),
    })
}

pub(crate) async fn apply_restore(
    paths: &RuntimePaths,
    plan_digest: &str,
    confirm_overwrite: bool,
) -> Result<RestoreResult, BackupError> {
    apply_restore_cancellable(
        paths,
        plan_digest,
        confirm_overwrite,
        CancellationToken::new(),
        |_, _, _| {},
        None,
    )
    .await
}

pub(crate) async fn apply_restore_cancellable<Progress>(
    paths: &RuntimePaths,
    plan_digest: &str,
    confirm_overwrite: bool,
    cancellation: CancellationToken,
    progress: Progress,
    running_operation_id: Option<&str>,
) -> Result<RestoreResult, BackupError>
where
    Progress: Fn(&str, u64, u64) + Send + Sync,
{
    progress("revalidating_plan", 0, 5);
    ensure_not_cancelled(&cancellation)?;
    let plan = plan_restore(paths).await?;
    if plan.plan_digest != plan_digest {
        return Err(BackupError::StaleRestorePlan);
    }
    if plan.requires_overwrite_confirmation && !confirm_overwrite {
        return Err(BackupError::RestoreConfirmationRequired);
    }
    let (profile, _, _) = load_local(paths).await?;
    let download_dir = tempfile::Builder::new()
        .prefix("momso-download-")
        .tempdir_in(&paths.backups_path)?;
    let encrypted_path = download_dir.path().join("snapshot.tar.age");
    progress("downloading_archive", 1, 5);
    backup_remote::download_r2(
        &profile,
        &plan.remote_snapshot.r2_object_key,
        &encrypted_path,
        cancellation.clone(),
    )
    .await?;
    ensure_not_cancelled(&cancellation)?;
    progress("decrypting_archive", 2, 5);
    let backups_path = paths.backups_path.clone();
    let workspace_id = profile.workspace_id.clone();
    let expected_archive_sha = plan.remote_snapshot.encrypted_archive_sha256.clone();
    let extracted = tauri::async_runtime::spawn_blocking(move || {
        backup_archive::extract(
            &encrypted_path,
            &expected_archive_sha,
            &backups_path,
            &workspace_id,
        )
    })
    .await
    .map_err(|error| std::io::Error::other(error.to_string()))??;
    ensure_not_cancelled(&cancellation)?;
    progress("verifying_content", 3, 5);
    let extracted_state =
        backup_profile::local_content_state(&extracted.database_path, &extracted.objects_path)?;
    if extracted_state.sha256 != plan.remote_snapshot.content_sha256 {
        return Err(BackupError::RestoredContentMismatch);
    }
    ensure_not_cancelled(&cancellation)?;
    progress("replacing_local_copy", 4, 5);
    backup_archive::validate_object_merge(&extracted.objects_path, &paths.objects_path)?;
    if let Some(operation_id) = running_operation_id {
        crate::operations::copy_running_to_database(
            &paths.database_path,
            &extracted.database_path,
            operation_id,
        )?;
    }
    backup_archive::merge_objects(&extracted.objects_path, &paths.objects_path)?;
    let safety_backup = storage::restore_snapshot_file(
        &extracted.database_path,
        &paths.database_path,
        &paths.backups_path,
    )?;
    backup_profile::restore_profile_after_database_restore(
        &paths.database_path,
        &profile,
        backup_profile::RestoredSnapshot {
            snapshot_id: &plan.remote_snapshot.snapshot_id,
            parent_snapshot_id: plan.remote_snapshot.parent_snapshot_id.as_deref(),
            content_sha256: &plan.remote_snapshot.content_sha256,
            encrypted_archive_sha256: &plan.remote_snapshot.encrypted_archive_sha256,
            manifest_sha256: &plan.remote_snapshot.manifest_sha256,
            github_commit_sha: &plan.remote_snapshot.github_commit_sha,
            r2_object_key: &plan.remote_snapshot.r2_object_key,
            record_count: plan.remote_snapshot.record_count,
            object_count: plan.remote_snapshot.object_count,
            created_at: &plan.remote_snapshot.created_at,
        },
    )?;
    progress("complete", 5, 5);
    log::info!("User-owned backup restore completed");
    Ok(RestoreResult {
        restored_snapshot_id: plan.remote_snapshot.snapshot_id,
        safety_backup_path: safety_backup.to_string_lossy().into_owned(),
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
