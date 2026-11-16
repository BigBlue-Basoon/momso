use crate::{backup_profile, process};
use native_tls::TlsConnector;
use postgres_native_tls::MakeTlsConnector;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use thiserror::Error;
use tokio_postgres::Client;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const MIN_R2_TRANSFER_SECONDS: u64 = 10 * 60;
const MAX_R2_TRANSFER_SECONDS: u64 = 6 * 60 * 60;
const R2_TRANSFER_FLOOR_BYTES_PER_SECOND: u64 = 256 * 1024;

fn r2_transfer_timeout(byte_length: Option<u64>) -> Duration {
    let estimated = byte_length
        .map(|bytes| {
            bytes
                .div_ceil(R2_TRANSFER_FLOOR_BYTES_PER_SECOND)
                .saturating_add(120)
        })
        .unwrap_or(MAX_R2_TRANSFER_SECONDS);
    Duration::from_secs(estimated.clamp(MIN_R2_TRANSFER_SECONDS, MAX_R2_TRANSFER_SECONDS))
}

const REMOTE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS momso_backup_snapshots (
  workspace_id text NOT NULL,
  snapshot_id text NOT NULL,
  parent_snapshot_id text,
  content_sha256 text NOT NULL,
  encrypted_archive_sha256 text NOT NULL,
  manifest_sha256 text NOT NULL,
  github_commit_sha text NOT NULL,
  r2_object_key text NOT NULL,
  record_count bigint NOT NULL CHECK (record_count >= 0),
  object_count bigint NOT NULL CHECK (object_count >= 0),
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (workspace_id, snapshot_id)
);
CREATE TABLE IF NOT EXISTS momso_backup_heads (
  workspace_id text PRIMARY KEY,
  snapshot_id text NOT NULL,
  updated_at timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (workspace_id, snapshot_id)
    REFERENCES momso_backup_snapshots(workspace_id, snapshot_id)
);
";

#[derive(Debug, Error)]
pub(crate) enum RemoteError {
    #[error(transparent)]
    Process(#[from] process::ProcessError),
    #[error(transparent)]
    Profile(#[from] backup_profile::BackupProfileError),
    #[error("연결 도구가 예상과 다른 응답을 보냈습니다")]
    InvalidResponse(#[from] serde_json::Error),
    #[error("원격 백업 공간을 만들거나 읽지 못했습니다")]
    ToolFailed,
    #[error("GitHub 백업 저장소는 private이어야 합니다")]
    PublicRepository,
    #[error("GitHub 계정 식별자를 확인하지 못했습니다")]
    InvalidGithubLogin,
    #[error("Neon 조직이 여러 개입니다. 사용할 조직을 선택해 주세요")]
    NeonOrganizationRequired,
    #[error("선택한 Neon 조직을 찾을 수 없습니다")]
    UnknownNeonOrganization,
    #[error("Neon 백업 프로젝트 구성을 확인하지 못했습니다")]
    InvalidNeonProject,
    #[error("Neon 연결을 안전하게 열지 못했습니다")]
    NeonTls,
    #[error("Neon 백업 데이터베이스 작업에 실패했습니다")]
    NeonDatabase(#[from] tokio_postgres::Error),
    #[error("다른 기기가 먼저 원격 백업을 갱신했습니다. 다시 비교해 주세요")]
    RemoteHeadChanged,
    #[error("로컬 메타데이터 저장소에 예상하지 못한 변경이 있습니다")]
    DirtyMetadataRepository,
    #[error("백업 메타데이터 파일을 만들지 못했습니다")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NeonOrganization {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupCandidate {
    pub(crate) repository: String,
    pub(crate) workspace_id: String,
    pub(crate) r2_bucket: String,
    pub(crate) neon_organization_id: Option<String>,
    pub(crate) neon_project_id: String,
    pub(crate) neon_branch_id: String,
    pub(crate) neon_database_name: String,
    pub(crate) neon_role_name: String,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteSnapshot {
    pub(crate) snapshot_id: String,
    pub(crate) parent_snapshot_id: Option<String>,
    pub(crate) content_sha256: String,
    pub(crate) encrypted_archive_sha256: String,
    pub(crate) manifest_sha256: String,
    pub(crate) github_commit_sha: String,
    pub(crate) r2_object_key: String,
    pub(crate) record_count: i64,
    pub(crate) object_count: i64,
    pub(crate) created_at: String,
}

#[derive(Clone)]
pub(crate) struct NewRemoteSnapshot<'a> {
    pub(crate) snapshot_id: &'a str,
    pub(crate) parent_snapshot_id: Option<&'a str>,
    pub(crate) content_sha256: &'a str,
    pub(crate) encrypted_archive_sha256: &'a str,
    pub(crate) manifest_sha256: &'a str,
    pub(crate) github_commit_sha: &'a str,
    pub(crate) r2_object_key: &'a str,
    pub(crate) record_count: u64,
    pub(crate) object_count: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeadRelation {
    Same,
    RemoteNewer,
    RemoteOlder,
    Unrelated,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceMetadata<'a> {
    format: &'static str,
    workspace_id: &'a str,
    github_repository: &'a str,
    r2_bucket: &'a str,
    neon_organization_id: Option<&'a str>,
    neon_project_id: &'a str,
    neon_branch_id: &'a str,
    neon_database_name: &'a str,
    neon_role_name: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredWorkspaceMetadata {
    format: String,
    workspace_id: String,
    github_repository: String,
    r2_bucket: String,
    neon_organization_id: Option<String>,
    neon_project_id: String,
    neon_branch_id: String,
    neon_database_name: String,
    neon_role_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubRepositoryListItem {
    name_with_owner: String,
    name: String,
    is_private: bool,
    updated_at: String,
}

#[derive(Deserialize)]
struct NeonProjectEnvelope {
    project: NeonProject,
}

#[derive(Deserialize)]
struct NeonProject {
    id: String,
}

#[derive(Deserialize)]
struct NeonBranch {
    id: String,
    #[serde(default)]
    default: bool,
}

async fn run_owned(
    tool: &str,
    args: &[String],
    cwd: Option<&Path>,
    timeout: Duration,
) -> Result<process::ProcessOutput, RemoteError> {
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    process::run_tool(
        tool,
        &args,
        cwd,
        None,
        &process::empty_environment(),
        timeout,
    )
    .await
    .map_err(RemoteError::from)
}

async fn run_owned_cancellable(
    tool: &str,
    args: &[String],
    cwd: Option<&Path>,
    timeout: Duration,
    cancellation: CancellationToken,
) -> Result<process::ProcessOutput, RemoteError> {
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    process::run_tool_cancellable(
        tool,
        &args,
        cwd,
        None,
        &process::empty_environment(),
        timeout,
        Some(cancellation),
    )
    .await
    .map_err(RemoteError::from)
}

fn require_success(output: process::ProcessOutput) -> Result<String, RemoteError> {
    if output.success {
        Ok(output.stdout)
    } else {
        Err(RemoteError::ToolFailed)
    }
}

fn valid_provider_identifier(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 255
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn backup_candidate(
    repository: &GithubRepositoryListItem,
    metadata: StoredWorkspaceMetadata,
) -> Option<BackupCandidate> {
    if !repository.is_private
        || !repository.name.starts_with("momso-backup-")
        || metadata.format != "momso-user-owned-backup-v1"
        || metadata.github_repository != repository.name_with_owner
        || Uuid::parse_str(&metadata.workspace_id).is_err()
        || !metadata.r2_bucket.starts_with("momso-backup-")
        || !valid_provider_identifier(&metadata.r2_bucket)
        || !valid_provider_identifier(&metadata.neon_project_id)
        || !valid_provider_identifier(&metadata.neon_branch_id)
        || !valid_provider_identifier(&metadata.neon_database_name)
        || !valid_provider_identifier(&metadata.neon_role_name)
        || metadata
            .neon_organization_id
            .as_deref()
            .is_some_and(|value| !valid_provider_identifier(value))
    {
        return None;
    }

    Some(BackupCandidate {
        repository: repository.name_with_owner.clone(),
        workspace_id: metadata.workspace_id,
        r2_bucket: metadata.r2_bucket,
        neon_organization_id: metadata.neon_organization_id,
        neon_project_id: metadata.neon_project_id,
        neon_branch_id: metadata.neon_branch_id,
        neon_database_name: metadata.neon_database_name,
        neon_role_name: metadata.neon_role_name,
        updated_at: repository.updated_at.clone(),
    })
}

pub(crate) async fn discover_backups() -> Result<Vec<BackupCandidate>, RemoteError> {
    let repositories: Vec<GithubRepositoryListItem> = serde_json::from_str(&require_success(
        process::run_tool(
            "gh",
            &[
                "repo",
                "list",
                "--visibility",
                "private",
                "--limit",
                "100",
                "--json",
                "nameWithOwner,name,isPrivate,updatedAt",
            ],
            None,
            None,
            &process::empty_environment(),
            Duration::from_secs(45),
        )
        .await?,
    )?)?;

    let mut candidates = Vec::new();
    for repository in repositories
        .into_iter()
        .filter(|repository| repository.is_private && repository.name.starts_with("momso-backup-"))
    {
        let endpoint = format!(
            "repos/{}/contents/workspace.json",
            repository.name_with_owner
        );
        let output = run_owned(
            "gh",
            &[
                "api".into(),
                "-H".into(),
                "Accept: application/vnd.github.raw+json".into(),
                endpoint,
            ],
            None,
            Duration::from_secs(30),
        )
        .await?;
        if !output.success {
            continue;
        }
        let Ok(metadata) = serde_json::from_str::<StoredWorkspaceMetadata>(&output.stdout) else {
            continue;
        };
        if let Some(candidate) = backup_candidate(&repository, metadata) {
            candidates.push(candidate);
        }
    }
    candidates.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    Ok(candidates)
}

pub(crate) async fn find_backup(repository: &str) -> Result<Option<BackupCandidate>, RemoteError> {
    Ok(discover_backups()
        .await?
        .into_iter()
        .find(|candidate| candidate.repository == repository))
}

pub(crate) async fn neon_organizations() -> Result<Vec<NeonOrganization>, RemoteError> {
    let stdout = require_success(
        process::run_tool(
            "neon",
            &["orgs", "list", "--output", "json", "--no-color"],
            None,
            None,
            &process::empty_environment(),
            Duration::from_secs(30),
        )
        .await?,
    )?;
    let value: Value = serde_json::from_str(&stdout)?;
    let organizations = value
        .as_array()
        .ok_or(RemoteError::InvalidNeonProject)?
        .iter()
        .filter_map(|organization| {
            Some(NeonOrganization {
                id: organization.get("id")?.as_str()?.to_owned(),
                name: organization
                    .get("name")
                    .or_else(|| organization.get("handle"))?
                    .as_str()?
                    .to_owned(),
            })
        })
        .collect::<Vec<_>>();
    Ok(organizations)
}

pub(crate) async fn prepare_resources(
    database_path: &Path,
    selected_neon_organization_id: Option<&str>,
) -> Result<backup_profile::BackupProfile, RemoteError> {
    let mut profile = backup_profile::ensure(database_path)?;
    if profile.github_repository.is_none() {
        let repository = ensure_github_repository(&profile.workspace_id).await?;
        backup_profile::set_github_repository(database_path, &profile.workspace_id, &repository)?;
        profile = backup_profile::ensure(database_path)?;
    }
    if profile.r2_bucket.is_none() {
        let bucket = ensure_r2_bucket(&profile.workspace_id).await?;
        backup_profile::set_r2_bucket(database_path, &profile.workspace_id, &bucket)?;
        profile = backup_profile::ensure(database_path)?;
    }
    if profile.neon_project_id.is_none() {
        let neon =
            ensure_neon_resource(&profile.workspace_id, selected_neon_organization_id).await?;
        backup_profile::set_neon_resource(database_path, &profile.workspace_id, &neon)?;
        profile = backup_profile::ensure(database_path)?;
    }
    let client = neon_client(&profile).await?;
    ensure_remote_schema(&client).await?;
    Ok(profile)
}

async fn ensure_github_repository(workspace_id: &str) -> Result<String, RemoteError> {
    let login = require_success(
        process::run_tool(
            "gh",
            &["api", "user", "--jq", ".login"],
            None,
            None,
            &process::empty_environment(),
            Duration::from_secs(30),
        )
        .await?,
    )?
    .trim()
    .to_owned();
    if login.is_empty()
        || !login
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err(RemoteError::InvalidGithubLogin);
    }
    let short = workspace_id
        .replace('-', "")
        .chars()
        .take(12)
        .collect::<String>();
    let repository = format!("{login}/momso-backup-{short}");
    let view = process::run_tool(
        "gh",
        &[
            "repo",
            "view",
            &repository,
            "--json",
            "isPrivate",
            "--jq",
            ".isPrivate",
        ],
        None,
        None,
        &process::empty_environment(),
        Duration::from_secs(30),
    )
    .await?;
    if view.success {
        if view.stdout.trim() != "true" {
            return Err(RemoteError::PublicRepository);
        }
        return Ok(repository);
    }

    require_success(
        process::run_tool(
            "gh",
            &[
                "repo",
                "create",
                &repository,
                "--private",
                "--description",
                "Encrypted momso backup metadata",
                "--disable-issues",
                "--disable-wiki",
            ],
            None,
            None,
            &process::empty_environment(),
            Duration::from_secs(60),
        )
        .await?,
    )?;
    Ok(repository)
}

fn collect_bucket_names(value: &Value, names: &mut Vec<String>) {
    match value {
        Value::Array(values) => values
            .iter()
            .for_each(|value| collect_bucket_names(value, names)),
        Value::Object(map) => {
            if let Some(name) = map.get("name").and_then(Value::as_str) {
                names.push(name.to_owned());
            }
            map.values()
                .for_each(|value| collect_bucket_names(value, names));
        }
        _ => {}
    }
}

async fn ensure_r2_bucket(workspace_id: &str) -> Result<String, RemoteError> {
    let short = workspace_id
        .replace('-', "")
        .chars()
        .take(12)
        .collect::<String>();
    let bucket = format!("momso-backup-{short}");
    let list = require_success(
        process::run_tool(
            "wrangler",
            &["r2", "bucket", "list", "--json"],
            None,
            None,
            &process::empty_environment(),
            Duration::from_secs(60),
        )
        .await?,
    )?;
    let value: Value = serde_json::from_str(&list)?;
    let mut names = Vec::new();
    collect_bucket_names(&value, &mut names);
    if !names.iter().any(|name| name == &bucket) {
        require_success(
            process::run_tool(
                "wrangler",
                &["r2", "bucket", "create", &bucket],
                None,
                None,
                &process::empty_environment(),
                Duration::from_secs(60),
            )
            .await?,
        )?;
    }
    Ok(bucket)
}

fn project_from_list(value: &Value, project_name: &str) -> Option<String> {
    value.as_array()?.iter().find_map(|project| {
        (project.get("name")?.as_str()? == project_name)
            .then(|| project.get("id")?.as_str().map(str::to_owned))
            .flatten()
    })
}

async fn ensure_neon_resource(
    workspace_id: &str,
    selected_organization_id: Option<&str>,
) -> Result<backup_profile::NeonResource, RemoteError> {
    let organizations = neon_organizations().await?;
    let organization_id = match (organizations.as_slice(), selected_organization_id) {
        ([], _) => None,
        ([only], None) => Some(only.id.clone()),
        ([..], Some(selected)) => {
            if organizations
                .iter()
                .any(|organization| organization.id == selected)
            {
                Some(selected.to_owned())
            } else {
                return Err(RemoteError::UnknownNeonOrganization);
            }
        }
        ([_, _, ..], None) => return Err(RemoteError::NeonOrganizationRequired),
    };
    let short = workspace_id
        .replace('-', "")
        .chars()
        .take(12)
        .collect::<String>();
    let project_name = format!("momso-backup-{short}");
    let mut list_args = vec![
        "projects".into(),
        "list".into(),
        "--output".into(),
        "json".into(),
        "--no-color".into(),
    ];
    if let Some(organization_id) = &organization_id {
        list_args.extend(["--org-id".into(), organization_id.clone()]);
    }
    let projects: Value = serde_json::from_str(&require_success(
        run_owned("neon", &list_args, None, Duration::from_secs(45)).await?,
    )?)?;
    let project_id = if let Some(project_id) = project_from_list(&projects, &project_name) {
        project_id
    } else {
        let mut create_args = vec![
            "projects".into(),
            "create".into(),
            "--name".into(),
            project_name,
            "--region-id".into(),
            "aws-ap-southeast-1".into(),
            "--database".into(),
            "momso".into(),
            "--role".into(),
            "momso_owner".into(),
            "--output".into(),
            "json".into(),
            "--no-color".into(),
        ];
        if let Some(organization_id) = &organization_id {
            create_args.extend(["--org-id".into(), organization_id.clone()]);
        }
        let created: NeonProjectEnvelope = serde_json::from_str(&require_success(
            run_owned("neon", &create_args, None, Duration::from_secs(90)).await?,
        )?)?;
        created.project.id
    };

    let branches: Vec<NeonBranch> = serde_json::from_str(&require_success(
        run_owned(
            "neon",
            &[
                "branches".into(),
                "list".into(),
                "--project-id".into(),
                project_id.clone(),
                "--output".into(),
                "json".into(),
                "--no-color".into(),
            ],
            None,
            Duration::from_secs(45),
        )
        .await?,
    )?)?;
    let branch_id = branches
        .iter()
        .find(|branch| branch.default)
        .or_else(|| branches.first())
        .map(|branch| branch.id.clone())
        .ok_or(RemoteError::InvalidNeonProject)?;
    Ok(backup_profile::NeonResource {
        organization_id,
        project_id,
        branch_id,
        database_name: "momso".into(),
        role_name: "momso_owner".into(),
    })
}

async fn connection_string(profile: &backup_profile::BackupProfile) -> Result<String, RemoteError> {
    let project_id = profile
        .neon_project_id
        .as_deref()
        .ok_or(RemoteError::InvalidNeonProject)?;
    let branch_id = profile
        .neon_branch_id
        .as_deref()
        .ok_or(RemoteError::InvalidNeonProject)?;
    let role_name = profile
        .neon_role_name
        .as_deref()
        .ok_or(RemoteError::InvalidNeonProject)?;
    let database_name = profile
        .neon_database_name
        .as_deref()
        .ok_or(RemoteError::InvalidNeonProject)?;
    let output = process::run_tool(
        "neon",
        &[
            "connection-string",
            branch_id,
            "--project-id",
            project_id,
            "--role-name",
            role_name,
            "--database-name",
            database_name,
            "--ssl",
            "require",
        ],
        None,
        None,
        &process::empty_environment(),
        Duration::from_secs(45),
    )
    .await?;
    let connection_string = require_success(output)?.trim().to_owned();
    if !connection_string.starts_with("postgres") {
        return Err(RemoteError::InvalidNeonProject);
    }
    Ok(connection_string)
}

async fn neon_client(profile: &backup_profile::BackupProfile) -> Result<Client, RemoteError> {
    let tls = TlsConnector::builder()
        .build()
        .map_err(|_| RemoteError::NeonTls)?;
    let (client, connection) = tokio_postgres::connect(
        &connection_string(profile).await?,
        MakeTlsConnector::new(tls),
    )
    .await?;
    tauri::async_runtime::spawn(async move {
        let _ = connection.await;
    });
    Ok(client)
}

async fn ensure_remote_schema(client: &Client) -> Result<(), RemoteError> {
    client.batch_execute(REMOTE_SCHEMA).await?;
    Ok(())
}

pub(crate) async fn read_remote_head(
    profile: &backup_profile::BackupProfile,
) -> Result<Option<RemoteSnapshot>, RemoteError> {
    let client = neon_client(profile).await?;
    ensure_remote_schema(&client).await?;
    let row = client
        .query_opt(
            "SELECT s.snapshot_id, s.parent_snapshot_id, s.content_sha256,
                    s.encrypted_archive_sha256, s.manifest_sha256,
                    s.github_commit_sha, s.r2_object_key,
                    s.record_count, s.object_count, s.created_at::text
             FROM momso_backup_heads h
             JOIN momso_backup_snapshots s
               ON s.workspace_id = h.workspace_id AND s.snapshot_id = h.snapshot_id
             WHERE h.workspace_id = $1",
            &[&profile.workspace_id],
        )
        .await?;
    Ok(row.map(|row| RemoteSnapshot {
        snapshot_id: row.get(0),
        parent_snapshot_id: row.get(1),
        content_sha256: row.get(2),
        encrypted_archive_sha256: row.get(3),
        manifest_sha256: row.get(4),
        github_commit_sha: row.get(5),
        r2_object_key: row.get(6),
        record_count: row.get(7),
        object_count: row.get(8),
        created_at: row.get(9),
    }))
}

pub(crate) async fn compare_remote_head(
    profile: &backup_profile::BackupProfile,
    local_snapshot_id: Option<&str>,
    remote_snapshot_id: Option<&str>,
) -> Result<HeadRelation, RemoteError> {
    if local_snapshot_id == remote_snapshot_id {
        return Ok(HeadRelation::Same);
    }
    let (Some(local_snapshot_id), Some(remote_snapshot_id)) =
        (local_snapshot_id, remote_snapshot_id)
    else {
        return Ok(if local_snapshot_id.is_none() {
            HeadRelation::RemoteNewer
        } else {
            HeadRelation::RemoteOlder
        });
    };

    let client = neon_client(profile).await?;
    ensure_remote_schema(&client).await?;
    let is_ancestor = async |ancestor: &str, descendant: &str| {
        client
            .query_one(
                "WITH RECURSIVE lineage AS (
                   SELECT snapshot_id, parent_snapshot_id
                   FROM momso_backup_snapshots
                   WHERE workspace_id = $1 AND snapshot_id = $2
                   UNION ALL
                   SELECT parent.snapshot_id, parent.parent_snapshot_id
                   FROM momso_backup_snapshots parent
                   JOIN lineage child ON parent.snapshot_id = child.parent_snapshot_id
                   WHERE parent.workspace_id = $1
                 )
                 SELECT EXISTS(SELECT 1 FROM lineage WHERE snapshot_id = $3)",
                &[&profile.workspace_id, &descendant, &ancestor],
            )
            .await
            .map(|row| row.get::<_, bool>(0))
    };
    if is_ancestor(local_snapshot_id, remote_snapshot_id).await? {
        return Ok(HeadRelation::RemoteNewer);
    }
    if is_ancestor(remote_snapshot_id, local_snapshot_id).await? {
        return Ok(HeadRelation::RemoteOlder);
    }
    Ok(HeadRelation::Unrelated)
}

pub(crate) async fn advance_remote_head(
    profile: &backup_profile::BackupProfile,
    snapshot: NewRemoteSnapshot<'_>,
) -> Result<(), RemoteError> {
    let mut client = neon_client(profile).await?;
    ensure_remote_schema(&client).await?;
    let transaction = client.transaction().await?;
    transaction
        .query_one(
            "SELECT pg_advisory_xact_lock(hashtext($1))",
            &[&profile.workspace_id],
        )
        .await?;
    let current = transaction
        .query_opt(
            "SELECT snapshot_id FROM momso_backup_heads WHERE workspace_id = $1 FOR UPDATE",
            &[&profile.workspace_id],
        )
        .await?
        .map(|row| row.get::<_, String>(0));
    if current.as_deref() != snapshot.parent_snapshot_id {
        return Err(RemoteError::RemoteHeadChanged);
    }
    transaction
        .execute(
            "INSERT INTO momso_backup_snapshots (
               workspace_id, snapshot_id, parent_snapshot_id, content_sha256,
               encrypted_archive_sha256, manifest_sha256, github_commit_sha,
               r2_object_key, record_count, object_count
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
            &[
                &profile.workspace_id,
                &snapshot.snapshot_id,
                &snapshot.parent_snapshot_id,
                &snapshot.content_sha256,
                &snapshot.encrypted_archive_sha256,
                &snapshot.manifest_sha256,
                &snapshot.github_commit_sha,
                &snapshot.r2_object_key,
                &(snapshot.record_count as i64),
                &(snapshot.object_count as i64),
            ],
        )
        .await?;
    transaction
        .execute(
            "INSERT INTO momso_backup_heads (workspace_id, snapshot_id, updated_at)
             VALUES ($1, $2, now())
             ON CONFLICT(workspace_id) DO UPDATE SET
               snapshot_id = excluded.snapshot_id,
               updated_at = excluded.updated_at",
            &[&profile.workspace_id, &snapshot.snapshot_id],
        )
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub(crate) async fn upload_r2(
    profile: &backup_profile::BackupProfile,
    object_key: &str,
    source_path: &Path,
    cancellation: CancellationToken,
) -> Result<(), RemoteError> {
    let bucket = profile
        .r2_bucket
        .as_deref()
        .ok_or(RemoteError::ToolFailed)?;
    let target = format!("{bucket}/{object_key}");
    let byte_length = source_path.metadata()?.len();
    require_success(
        run_owned_cancellable(
            "wrangler",
            &[
                "r2".into(),
                "object".into(),
                "put".into(),
                target,
                "--file".into(),
                source_path.to_string_lossy().into_owned(),
                "--remote".into(),
            ],
            None,
            r2_transfer_timeout(Some(byte_length)),
            cancellation,
        )
        .await?,
    )?;
    Ok(())
}

pub(crate) async fn download_r2(
    profile: &backup_profile::BackupProfile,
    object_key: &str,
    destination_path: &Path,
    cancellation: CancellationToken,
) -> Result<(), RemoteError> {
    let bucket = profile
        .r2_bucket
        .as_deref()
        .ok_or(RemoteError::ToolFailed)?;
    let target = format!("{bucket}/{object_key}");
    require_success(
        run_owned_cancellable(
            "wrangler",
            &[
                "r2".into(),
                "object".into(),
                "get".into(),
                target,
                "--file".into(),
                destination_path.to_string_lossy().into_owned(),
                "--remote".into(),
            ],
            None,
            r2_transfer_timeout(None),
            cancellation,
        )
        .await?,
    )?;
    Ok(())
}

async fn git(
    args: &[String],
    repository_path: &Path,
    timeout: Duration,
) -> Result<process::ProcessOutput, RemoteError> {
    run_owned("git", args, Some(repository_path), timeout).await
}

pub(crate) async fn push_metadata(
    profile: &backup_profile::BackupProfile,
    data_root: &Path,
    snapshot_id: &str,
    transcription_dictionary_json: &str,
    snapshot_manifest_json: &str,
) -> Result<String, RemoteError> {
    let repository = profile
        .github_repository
        .as_deref()
        .ok_or(RemoteError::ToolFailed)?;
    let repository_path = data_root.join("sync").join("metadata");
    if !repository_path.join(".git").is_dir() {
        if repository_path.exists() {
            fs::remove_dir_all(&repository_path)?;
        }
        fs::create_dir_all(repository_path.parent().unwrap_or(data_root))?;
        require_success(
            run_owned(
                "gh",
                &[
                    "repo".into(),
                    "clone".into(),
                    repository.into(),
                    repository_path.to_string_lossy().into_owned(),
                ],
                None,
                Duration::from_secs(120),
            )
            .await?,
        )?;
    }

    let status = require_success(
        git(
            &["status".into(), "--porcelain".into()],
            &repository_path,
            Duration::from_secs(20),
        )
        .await?,
    )?;
    if !status.trim().is_empty() {
        return Err(RemoteError::DirtyMetadataRepository);
    }
    let has_head = git(
        &["rev-parse".into(), "--verify".into(), "HEAD".into()],
        &repository_path,
        Duration::from_secs(20),
    )
    .await?
    .success;
    if has_head {
        require_success(
            git(
                &[
                    "pull".into(),
                    "--ff-only".into(),
                    "origin".into(),
                    "main".into(),
                ],
                &repository_path,
                Duration::from_secs(90),
            )
            .await?,
        )?;
    }

    let workspace = WorkspaceMetadata {
        format: "momso-user-owned-backup-v1",
        workspace_id: &profile.workspace_id,
        github_repository: repository,
        r2_bucket: profile
            .r2_bucket
            .as_deref()
            .ok_or(RemoteError::ToolFailed)?,
        neon_organization_id: profile.neon_organization_id.as_deref(),
        neon_project_id: profile
            .neon_project_id
            .as_deref()
            .ok_or(RemoteError::ToolFailed)?,
        neon_branch_id: profile
            .neon_branch_id
            .as_deref()
            .ok_or(RemoteError::ToolFailed)?,
        neon_database_name: profile
            .neon_database_name
            .as_deref()
            .ok_or(RemoteError::ToolFailed)?,
        neon_role_name: profile
            .neon_role_name
            .as_deref()
            .ok_or(RemoteError::ToolFailed)?,
    };
    fs::create_dir_all(repository_path.join("snapshots"))?;
    fs::write(
        repository_path.join("README.md"),
        "# momso backup metadata\n\nThis private repository stores content-free backup metadata and transcription correction rules. Recordings, transcripts, reports, credentials, and recovery keys are not stored here.\n",
    )?;
    fs::write(
        repository_path.join("workspace.json"),
        serde_json::to_string_pretty(&workspace)?,
    )?;
    fs::write(
        repository_path.join("transcription-dictionary.json"),
        transcription_dictionary_json,
    )?;
    let snapshot_relative = PathBuf::from("snapshots").join(format!("{snapshot_id}.json"));
    fs::write(
        repository_path.join(&snapshot_relative),
        snapshot_manifest_json,
    )?;

    let snapshot_arg = snapshot_relative.to_string_lossy().replace('\\', "/");
    require_success(
        git(
            &[
                "add".into(),
                "--".into(),
                "README.md".into(),
                "workspace.json".into(),
                "transcription-dictionary.json".into(),
                snapshot_arg,
            ],
            &repository_path,
            Duration::from_secs(30),
        )
        .await?,
    )?;
    let message = format!(
        "backup: snapshot {}",
        &snapshot_id[..8.min(snapshot_id.len())]
    );
    require_success(
        git(
            &[
                "-c".into(),
                "user.name=momso".into(),
                "-c".into(),
                "user.email=backup@momso.local".into(),
                "commit".into(),
                "-m".into(),
                message,
            ],
            &repository_path,
            Duration::from_secs(45),
        )
        .await?,
    )?;
    require_success(
        git(
            &[
                "push".into(),
                "--set-upstream".into(),
                "origin".into(),
                "HEAD:main".into(),
            ],
            &repository_path,
            Duration::from_secs(120),
        )
        .await?,
    )?;
    let commit = require_success(
        git(
            &["rev-parse".into(), "HEAD".into()],
            &repository_path,
            Duration::from_secs(20),
        )
        .await?,
    )?;
    Ok(commit.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r2_transfer_timeout_scales_with_archive_size_and_has_a_six_hour_ceiling() {
        assert_eq!(
            r2_transfer_timeout(Some(1)).as_secs(),
            MIN_R2_TRANSFER_SECONDS
        );
        assert!(
            r2_transfer_timeout(Some(2 * 1024 * 1024 * 1024)).as_secs() > MIN_R2_TRANSFER_SECONDS
        );
        assert_eq!(r2_transfer_timeout(None).as_secs(), MAX_R2_TRANSFER_SECONDS);
        assert_eq!(
            r2_transfer_timeout(Some(u64::MAX)).as_secs(),
            MAX_R2_TRANSFER_SECONDS
        );
    }

    #[test]
    fn bucket_parser_accepts_array_and_wrapped_shapes() {
        let value: Value = serde_json::json!({"buckets": [{"name": "momso-backup-123"}]});
        let mut names = Vec::new();
        collect_bucket_names(&value, &mut names);
        assert_eq!(names, vec!["momso-backup-123"]);
    }

    #[test]
    fn project_lookup_requires_an_exact_name() {
        let value: Value = serde_json::json!([
            {"id": "one", "name": "momso-backup-abc"},
            {"id": "two", "name": "momso-backup-abcd"}
        ]);
        assert_eq!(
            project_from_list(&value, "momso-backup-abc"),
            Some("one".into())
        );
    }

    #[test]
    fn discovery_accepts_only_matching_private_momso_metadata() {
        let repository = GithubRepositoryListItem {
            name_with_owner: "owner/momso-backup-123".into(),
            name: "momso-backup-123".into(),
            is_private: true,
            updated_at: "2026-07-15T00:00:00Z".into(),
        };
        let metadata = StoredWorkspaceMetadata {
            format: "momso-user-owned-backup-v1".into(),
            workspace_id: "d90fb78d-af2d-4da3-86b8-ff1db07f8c46".into(),
            github_repository: repository.name_with_owner.clone(),
            r2_bucket: "momso-backup-123".into(),
            neon_organization_id: Some("org-123".into()),
            neon_project_id: "project-123".into(),
            neon_branch_id: "branch-123".into(),
            neon_database_name: "momso".into(),
            neon_role_name: "momso_owner".into(),
        };
        let candidate = backup_candidate(&repository, metadata).expect("valid candidate");
        assert_eq!(candidate.repository, "owner/momso-backup-123");
    }

    #[test]
    fn discovery_rejects_metadata_that_points_at_another_repository() {
        let repository = GithubRepositoryListItem {
            name_with_owner: "owner/momso-backup-123".into(),
            name: "momso-backup-123".into(),
            is_private: true,
            updated_at: "2026-07-15T00:00:00Z".into(),
        };
        let metadata = StoredWorkspaceMetadata {
            format: "momso-user-owned-backup-v1".into(),
            workspace_id: "d90fb78d-af2d-4da3-86b8-ff1db07f8c46".into(),
            github_repository: "owner/another-repository".into(),
            r2_bucket: "momso-backup-123".into(),
            neon_organization_id: None,
            neon_project_id: "project-123".into(),
            neon_branch_id: "branch-123".into(),
            neon_database_name: "momso".into(),
            neon_role_name: "momso_owner".into(),
        };
        assert!(backup_candidate(&repository, metadata).is_none());
    }
}
