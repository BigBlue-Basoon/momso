use crate::{backup_profile, process};
use native_tls::TlsConnector;
use postgres_native_tls::MakeTlsConnector;
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;
use thiserror::Error;
use tokio_postgres::Client;

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
    #[error("Neon 백업 프로젝트 구성을 확인하지 못했습니다")]
    InvalidNeonProject,
    #[error("Neon 연결을 안전하게 열지 못했습니다")]
    NeonTls,
    #[error("Neon 백업 데이터베이스 작업에 실패했습니다")]
    NeonDatabase(#[from] tokio_postgres::Error),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeadRelation {
    Same,
    RemoteNewer,
    RemoteOlder,
    Unrelated,
}

fn require_success(output: process::ProcessOutput) -> Result<String, RemoteError> {
    if output.success {
        Ok(output.stdout)
    } else {
        Err(RemoteError::ToolFailed)
    }
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
