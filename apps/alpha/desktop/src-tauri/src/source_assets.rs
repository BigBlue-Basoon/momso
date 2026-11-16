use crate::{
    storage::{self, StorageError},
    text_paging,
};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use thiserror::Error;
use uuid::Uuid;
use walkdir::WalkDir;

const MAX_ACTOR_CHARS: usize = 120;
const MAX_FILE_NAME_CHARS: usize = 255;
const MAX_MEDIA_TYPE_CHARS: usize = 120;
const MAX_TEXT_ASSET_BYTES: u64 = 64 * 1024 * 1024;
const MAX_AUDIT_ISSUE_SAMPLES: usize = 20;

#[derive(Debug, Error)]
pub(crate) enum SourceAssetError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Local source file operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid source file input: {0}")]
    Validation(String),
    #[error("Source file {0} was not found")]
    NotFound(String),
    #[error("The source file changed while its import preview was open; create a new preview")]
    PlanMismatch,
    #[error("The source file import requires explicit confirmation")]
    ConfirmationRequired,
    #[error("Source file integrity check failed: {0}")]
    Integrity(String),
    #[error("A continuation read requires the complete content hash returned by the first page")]
    ContinuationHashRequired,
    #[error("The source text changed after the first page was read")]
    ContentChanged,
    #[error("The selected source file is not supported as bounded UTF-8 text: {0}")]
    UnsupportedText(String),
    #[error("The source file integrity audit was cancelled")]
    Cancelled,
    #[error("The source file is healthy and does not need quarantine")]
    Healthy,
    #[error("Source file quarantine requires explicit confirmation")]
    QuarantineConfirmationRequired,
    #[error(transparent)]
    Paging(#[from] text_paging::TextPagingError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceAssetKind {
    AudioRecording,
    RawTranscript,
    ReferenceDocument,
    GeneratedReport,
    Other,
}

impl SourceAssetKind {
    fn as_db(self) -> &'static str {
        match self {
            Self::AudioRecording => "audio_recording",
            Self::RawTranscript => "raw_transcript",
            Self::ReferenceDocument => "reference_document",
            Self::GeneratedReport => "generated_report",
            Self::Other => "other",
        }
    }

    fn from_db(value: &str) -> Result<Self, SourceAssetError> {
        match value {
            "audio_recording" => Ok(Self::AudioRecording),
            "raw_transcript" => Ok(Self::RawTranscript),
            "reference_document" => Ok(Self::ReferenceDocument),
            "generated_report" => Ok(Self::GeneratedReport),
            "other" => Ok(Self::Other),
            _ => Err(SourceAssetError::Integrity(format!(
                "unknown source file kind {value:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceAssetRelationRole {
    Source,
    Supporting,
    Derived,
}

impl SourceAssetRelationRole {
    fn as_db(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Supporting => "supporting",
            Self::Derived => "derived",
        }
    }

    fn from_db(value: &str) -> Result<Self, SourceAssetError> {
        match value {
            "source" => Ok(Self::Source),
            "supporting" => Ok(Self::Supporting),
            "derived" => Ok(Self::Derived),
            _ => Err(SourceAssetError::Integrity(format!(
                "unknown source file relation {value:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceAssetRecordLinkInput {
    record_id: String,
    record_version: i64,
    relation_role: SourceAssetRelationRole,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceAssetImportProposal {
    source_path: String,
    asset_kind: SourceAssetKind,
    media_type: Option<String>,
    record_link: Option<SourceAssetRecordLinkInput>,
    actor: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceAssetImportPlan {
    plan_digest: String,
    original_file_name: String,
    asset_kind: SourceAssetKind,
    content_sha256: String,
    byte_length: u64,
    already_stored: bool,
    record_link: Option<SourceAssetRecordLink>,
    requires_confirmation: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplySourceAssetImportInput {
    proposal: SourceAssetImportProposal,
    plan_digest: String,
    import_confirmed: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceAssetQuarantineProposal {
    asset_id: String,
    actor: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceAssetQuarantinePlan {
    plan_digest: String,
    asset_id: String,
    issue: String,
    expected_content_sha256: String,
    expected_byte_length: u64,
    requires_confirmation: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplySourceAssetQuarantineInput {
    proposal: SourceAssetQuarantineProposal,
    plan_digest: String,
    quarantine_confirmed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceAssetRecordLink {
    record_id: String,
    record_version: i64,
    relation_role: SourceAssetRelationRole,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceAsset {
    id: String,
    asset_kind: SourceAssetKind,
    content_sha256: String,
    byte_length: u64,
    original_file_name: String,
    media_type: Option<String>,
    captured_by: String,
    created_at: String,
    integrity_state: SourceAssetIntegrityState,
    integrity_issue: Option<String>,
    quarantined_at: Option<String>,
    quarantined_by: Option<String>,
    links: Vec<SourceAssetRecordLink>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceAssetIntegrityState {
    Active,
    Quarantined,
}

impl SourceAssetIntegrityState {
    fn from_db(value: &str) -> Result<Self, SourceAssetError> {
        match value {
            "active" => Ok(Self::Active),
            "quarantined" => Ok(Self::Quarantined),
            _ => Err(SourceAssetError::Integrity(format!(
                "unknown source file integrity state {value:?}"
            ))),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceTextPage {
    asset_id: String,
    content_sha256: String,
    byte_length: u64,
    cursor: u64,
    next_cursor: Option<u64>,
    content: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceAssetAuditIssueKind {
    Missing,
    Changed,
    UnsafePath,
    Unreadable,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceAssetAuditIssue {
    asset_id: String,
    issue: SourceAssetAuditIssueKind,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceAssetAuditResult {
    pub(crate) total_assets: u64,
    pub(crate) total_object_files: u64,
    healthy_assets: u64,
    missing_assets: u64,
    changed_assets: u64,
    unsafe_path_assets: u64,
    unreadable_assets: u64,
    unregistered_files: u64,
    issue_samples: Vec<SourceAssetAuditIssue>,
    samples_truncated: bool,
}

#[derive(Debug)]
struct PreparedImport {
    canonical_source: PathBuf,
    media_type: Option<String>,
    actor: String,
    storage_relative_path: String,
    plan: SourceAssetImportPlan,
}

#[derive(Debug)]
struct PreparedQuarantine {
    stored: StoredAssetRecord,
    actor: String,
    issue: String,
    plan: SourceAssetQuarantinePlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoredPathInspection {
    Ready,
    Missing,
    Unsafe,
}

#[derive(Debug)]
pub(crate) struct StoredAssetRecord {
    pub(crate) id: String,
    pub(crate) content_sha256: String,
    pub(crate) byte_length: u64,
    pub(crate) storage_relative_path: String,
    pub(crate) integrity_state: SourceAssetIntegrityState,
}

pub(crate) fn plan_import(
    database_path: &Path,
    objects_path: &Path,
    proposal: SourceAssetImportProposal,
) -> Result<SourceAssetImportPlan, SourceAssetError> {
    Ok(prepare_import(database_path, objects_path, proposal)?.plan)
}

pub(crate) fn cleanup_staging(objects_path: &Path) -> Result<usize, SourceAssetError> {
    let data_root = objects_path.parent().ok_or_else(|| {
        SourceAssetError::Validation("objects directory has no data root".to_owned())
    })?;
    let staging = data_root.join(".source-staging");
    if !existing_internal_directory(&staging)? {
        return Ok(0);
    }

    let mut removed = 0;
    for entry in fs::read_dir(&staging)? {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if !file_name.starts_with("import-") || !file_name.ends_with(".tmp") {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_file() && !metadata.file_type().is_symlink() {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    if fs::read_dir(&staging)?.next().is_none() {
        fs::remove_dir(staging)?;
    }
    Ok(removed)
}

pub(crate) fn apply_import(
    database_path: &Path,
    objects_path: &Path,
    input: ApplySourceAssetImportInput,
) -> Result<SourceAsset, SourceAssetError> {
    if !input.import_confirmed {
        return Err(SourceAssetError::ConfirmationRequired);
    }
    let prepared = prepare_import(database_path, objects_path, input.proposal)?;
    if prepared.plan.plan_digest != input.plan_digest {
        return Err(SourceAssetError::PlanMismatch);
    }

    if let Some(root) = objects_path.parent() {
        crate::chat_file_changes::app_write(
            root,
            Some(&format!("sha256:{}", prepared.plan.content_sha256)),
        );
    }
    materialize_source_file(objects_path, &prepared)?;

    let mut connection = storage::open_writable_connection(database_path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let timestamp: String =
        transaction.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')", [], |row| {
            row.get(0)
        })?;
    let existing_id = transaction
        .query_row(
            "SELECT id FROM momso_source_assets WHERE content_sha256 = ?1",
            [&prepared.plan.content_sha256],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let asset_id = existing_id.unwrap_or_else(|| Uuid::new_v4().to_string());

    transaction.execute(
        "INSERT INTO momso_source_assets (
           id, asset_kind, content_sha256, byte_length, storage_relative_path,
           original_file_name, media_type, captured_by, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(content_sha256) DO NOTHING",
        params![
            asset_id,
            prepared.plan.asset_kind.as_db(),
            prepared.plan.content_sha256,
            u64_to_i64(prepared.plan.byte_length, "source file size")?,
            prepared.storage_relative_path,
            prepared.plan.original_file_name,
            prepared.media_type,
            prepared.actor,
            timestamp,
        ],
    )?;
    let stored_id: String = transaction.query_row(
        "SELECT id FROM momso_source_assets WHERE content_sha256 = ?1",
        [&prepared.plan.content_sha256],
        |row| row.get(0),
    )?;
    transaction.execute(
        "UPDATE momso_source_assets
         SET integrity_state = 'active', integrity_issue = NULL,
             quarantined_at = NULL, quarantined_by = NULL
         WHERE id = ?1",
        [&stored_id],
    )?;

    if let Some(link) = &prepared.plan.record_link {
        transaction.execute(
            "INSERT OR IGNORE INTO momso_record_source_assets (
               record_id, record_version, asset_id, relation_role, linked_by, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                link.record_id,
                link.record_version,
                stored_id,
                link.relation_role.as_db(),
                prepared.actor,
                timestamp,
            ],
        )?;
    }
    transaction.commit()?;
    get(database_path, &stored_id)
}

pub(crate) fn plan_quarantine(
    database_path: &Path,
    objects_path: &Path,
    proposal: SourceAssetQuarantineProposal,
) -> Result<SourceAssetQuarantinePlan, SourceAssetError> {
    Ok(prepare_quarantine(database_path, objects_path, proposal)?.plan)
}

pub(crate) fn apply_quarantine(
    database_path: &Path,
    objects_path: &Path,
    input: ApplySourceAssetQuarantineInput,
) -> Result<SourceAsset, SourceAssetError> {
    if !input.quarantine_confirmed {
        return Err(SourceAssetError::QuarantineConfirmationRequired);
    }
    let prepared = prepare_quarantine(database_path, objects_path, input.proposal)?;
    if prepared.plan.plan_digest != input.plan_digest {
        return Err(SourceAssetError::PlanMismatch);
    }

    move_quarantined_bytes(objects_path, &prepared.stored, &prepared.issue)?;
    let connection = storage::open_writable_connection(database_path)?;
    let changed = connection.execute(
        "UPDATE momso_source_assets
         SET integrity_state = 'quarantined', integrity_issue = ?2,
             quarantined_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
             quarantined_by = ?3
         WHERE id = ?1 AND integrity_state = 'active'",
        params![prepared.stored.id, prepared.issue, prepared.actor],
    )?;
    if changed != 1 {
        return Err(SourceAssetError::PlanMismatch);
    }
    get(database_path, &prepared.plan.asset_id)
}

pub(crate) fn list(
    database_path: &Path,
    record_id: Option<&str>,
) -> Result<Vec<SourceAsset>, SourceAssetError> {
    let connection = storage::open_writable_connection(database_path)?;
    let mut assets = Vec::new();
    if let Some(record_id) = record_id {
        let mut statement = connection.prepare(
            "SELECT DISTINCT a.id, a.asset_kind, a.content_sha256, a.byte_length,
                    a.original_file_name, a.media_type, a.captured_by, a.created_at,
                    a.integrity_state, a.integrity_issue, a.quarantined_at, a.quarantined_by
             FROM momso_source_assets a
             JOIN momso_record_source_assets l ON l.asset_id = a.id
             WHERE l.record_id = ?1
             ORDER BY a.created_at, a.id",
        )?;
        let rows = statement.query_map([record_id], read_asset_row)?;
        for row in rows {
            assets.push(row?);
        }
    } else {
        let mut statement = connection.prepare(
            "SELECT id, asset_kind, content_sha256, byte_length,
                    original_file_name, media_type, captured_by, created_at,
                    integrity_state, integrity_issue, quarantined_at, quarantined_by
             FROM momso_source_assets
             ORDER BY created_at, id",
        )?;
        let rows = statement.query_map([], read_asset_row)?;
        for row in rows {
            assets.push(row?);
        }
    }

    for asset in &mut assets {
        asset.links = load_links(&connection, &asset.id)?;
    }
    Ok(assets)
}

pub(crate) fn read_text_page(
    database_path: &Path,
    objects_path: &Path,
    asset_id: &str,
    cursor: u64,
    expected_content_sha256: Option<&str>,
    max_bytes: u64,
) -> Result<SourceTextPage, SourceAssetError> {
    let stored = stored_asset(database_path, asset_id)?;
    if stored.integrity_state == SourceAssetIntegrityState::Quarantined {
        return Err(SourceAssetError::Integrity(format!(
            "source file {asset_id} is quarantined"
        )));
    }
    if stored.byte_length > MAX_TEXT_ASSET_BYTES {
        return Err(SourceAssetError::UnsupportedText(format!(
            "file is larger than {MAX_TEXT_ASSET_BYTES} bytes"
        )));
    }
    if cursor > 0 && expected_content_sha256.is_none() {
        return Err(SourceAssetError::ContinuationHashRequired);
    }

    let path = resolve_storage_path(objects_path, &stored.storage_relative_path)?;
    match inspect_stored_file(objects_path, &path)? {
        StoredPathInspection::Ready => {}
        StoredPathInspection::Missing => {
            return Err(SourceAssetError::NotFound(asset_id.to_owned()));
        }
        StoredPathInspection::Unsafe => {
            return Err(SourceAssetError::Integrity(format!(
                "stored path is unsafe for source file {asset_id}"
            )));
        }
    }
    let bytes = fs::read(path)?;
    let actual_sha256 = hash_bytes(&bytes);
    if actual_sha256 != stored.content_sha256 || bytes.len() as u64 != stored.byte_length {
        return Err(SourceAssetError::Integrity(format!(
            "stored bytes differ for source file {asset_id}"
        )));
    }
    if expected_content_sha256.is_some_and(|expected| expected != actual_sha256) {
        return Err(SourceAssetError::ContentChanged);
    }

    let text = String::from_utf8(bytes)
        .map_err(|_| SourceAssetError::UnsupportedText("file is not valid UTF-8".to_owned()))?;
    let page = text_paging::page(&text, cursor, max_bytes)?;
    Ok(SourceTextPage {
        asset_id: asset_id.to_owned(),
        content_sha256: actual_sha256,
        byte_length: stored.byte_length,
        cursor,
        next_cursor: page.next_cursor,
        content: page.content,
    })
}

pub(crate) fn stored_assets(
    database_path: &Path,
) -> Result<Vec<StoredAssetRecord>, SourceAssetError> {
    let connection = storage::open_writable_connection(database_path)?;
    let mut statement = connection.prepare(
        "SELECT id, content_sha256, byte_length, storage_relative_path, integrity_state
         FROM momso_source_assets
         WHERE integrity_state = 'active'
         ORDER BY created_at, id",
    )?;
    let rows = statement.query_map([], |row| {
        let byte_length: i64 = row.get(2)?;
        Ok(StoredAssetRecord {
            id: row.get(0)?,
            content_sha256: row.get(1)?,
            byte_length: u64::try_from(byte_length).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Integer,
                    Box::new(error),
                )
            })?,
            storage_relative_path: row.get(3)?,
            integrity_state: SourceAssetIntegrityState::from_db(&row.get::<_, String>(4)?)
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(SourceAssetError::from)
}

pub(crate) fn audit_integrity<Progress, Cancelled>(
    database_path: &Path,
    objects_path: &Path,
    mut progress: Progress,
    cancelled: Cancelled,
) -> Result<SourceAssetAuditResult, SourceAssetError>
where
    Progress: FnMut(u64, u64) -> Result<(), SourceAssetError>,
    Cancelled: Fn() -> bool,
{
    let assets = stored_assets(database_path)?;
    let total_assets = assets.len() as u64;
    let mut object_files = Vec::new();
    let mut scan_unreadable = 0;
    let mut scan_unsafe = 0;
    let mut scanned_entries = 0_u64;
    if objects_path.exists() {
        for entry in WalkDir::new(objects_path).follow_links(false) {
            if cancelled() {
                return Err(SourceAssetError::Cancelled);
            }
            scanned_entries += 1;
            if scanned_entries.is_multiple_of(250) {
                progress(0, 0)?;
            }
            match entry {
                Ok(entry) if entry.file_type().is_symlink() => scan_unsafe += 1,
                Ok(entry) if entry.file_type().is_file() => {
                    object_files.push(entry.path().to_path_buf());
                }
                Ok(_) => {}
                Err(_) => scan_unreadable += 1,
            }
        }
    }
    object_files.sort();
    let total_object_files = object_files.len() as u64;
    let total_work = total_assets
        .checked_add(total_object_files)
        .ok_or_else(|| SourceAssetError::Validation("source audit is too large".to_owned()))?;
    progress(0, total_work)?;
    let mut result = SourceAssetAuditResult {
        total_assets,
        total_object_files,
        healthy_assets: 0,
        missing_assets: 0,
        changed_assets: 0,
        unsafe_path_assets: scan_unsafe,
        unreadable_assets: scan_unreadable,
        unregistered_files: 0,
        issue_samples: Vec::new(),
        samples_truncated: false,
    };

    for (index, asset) in assets.iter().enumerate() {
        if cancelled() {
            return Err(SourceAssetError::Cancelled);
        }
        let issue = match resolve_storage_path(objects_path, &asset.storage_relative_path) {
            Err(_) => Some(SourceAssetAuditIssueKind::UnsafePath),
            Ok(path) => match inspect_stored_file(objects_path, &path) {
                Ok(StoredPathInspection::Missing) => Some(SourceAssetAuditIssueKind::Missing),
                Ok(StoredPathInspection::Unsafe) => Some(SourceAssetAuditIssueKind::UnsafePath),
                Ok(StoredPathInspection::Ready) => match hash_file(&path) {
                    Ok((sha256, byte_length))
                        if sha256 == asset.content_sha256 && byte_length == asset.byte_length =>
                    {
                        None
                    }
                    Ok(_) => Some(SourceAssetAuditIssueKind::Changed),
                    Err(_) => Some(SourceAssetAuditIssueKind::Unreadable),
                },
                Err(_) => Some(SourceAssetAuditIssueKind::Unreadable),
            },
        };

        match issue {
            None => result.healthy_assets += 1,
            Some(issue) => {
                match issue {
                    SourceAssetAuditIssueKind::Missing => result.missing_assets += 1,
                    SourceAssetAuditIssueKind::Changed => result.changed_assets += 1,
                    SourceAssetAuditIssueKind::UnsafePath => result.unsafe_path_assets += 1,
                    SourceAssetAuditIssueKind::Unreadable => result.unreadable_assets += 1,
                }
                if result.issue_samples.len() < MAX_AUDIT_ISSUE_SAMPLES {
                    result.issue_samples.push(SourceAssetAuditIssue {
                        asset_id: asset.id.clone(),
                        issue,
                    });
                } else {
                    result.samples_truncated = true;
                }
            }
        }

        let completed = index as u64 + 1;
        if completed == total_assets || completed.is_multiple_of(25) {
            progress(completed, total_work)?;
        }
    }

    let registered_paths = assets
        .iter()
        .map(|asset| asset.storage_relative_path.replace('\\', "/"))
        .collect::<HashSet<_>>();
    for (index, path) in object_files.iter().enumerate() {
        if cancelled() {
            return Err(SourceAssetError::Cancelled);
        }
        match path.strip_prefix(objects_path) {
            Ok(relative) => {
                let relative = relative.to_string_lossy().replace('\\', "/");
                if !registered_paths.contains(&relative) {
                    result.unregistered_files += 1;
                }
            }
            Err(_) => {
                result.unsafe_path_assets += 1;
            }
        }
        let completed = total_assets + index as u64 + 1;
        if completed == total_work || completed.is_multiple_of(100) {
            progress(completed, total_work)?;
        }
    }
    Ok(result)
}

fn prepare_quarantine(
    database_path: &Path,
    objects_path: &Path,
    proposal: SourceAssetQuarantineProposal,
) -> Result<PreparedQuarantine, SourceAssetError> {
    let actor = validate_required("actor", &proposal.actor, MAX_ACTOR_CHARS)?;
    let stored = stored_asset(database_path, proposal.asset_id.trim())?;
    if stored.integrity_state == SourceAssetIntegrityState::Quarantined {
        return Err(SourceAssetError::Validation(
            "source file is already quarantined".to_owned(),
        ));
    }
    let issue = inspect_quarantine_issue(objects_path, &stored)?;
    let plan_digest = quarantine_plan_digest(&stored, &actor, &issue);
    let plan = SourceAssetQuarantinePlan {
        plan_digest,
        asset_id: stored.id.clone(),
        issue: issue.clone(),
        expected_content_sha256: stored.content_sha256.clone(),
        expected_byte_length: stored.byte_length,
        requires_confirmation: true,
    };
    Ok(PreparedQuarantine {
        stored,
        actor,
        issue,
        plan,
    })
}

fn inspect_quarantine_issue(
    objects_path: &Path,
    stored: &StoredAssetRecord,
) -> Result<String, SourceAssetError> {
    let path = match resolve_storage_path(objects_path, &stored.storage_relative_path) {
        Ok(path) => path,
        Err(_) => return Ok("unsafe_path".to_owned()),
    };
    let inspection = match inspect_stored_file(objects_path, &path) {
        Ok(inspection) => inspection,
        Err(_) => return Ok("unreadable".to_owned()),
    };
    match inspection {
        StoredPathInspection::Missing => Ok("missing".to_owned()),
        StoredPathInspection::Unsafe => Ok("unsafe_path".to_owned()),
        StoredPathInspection::Ready => match hash_file(&path) {
            Ok((sha256, bytes))
                if sha256 == stored.content_sha256 && bytes == stored.byte_length =>
            {
                Err(SourceAssetError::Healthy)
            }
            Ok(_) => Ok("changed".to_owned()),
            Err(_) => Ok("unreadable".to_owned()),
        },
    }
}

fn move_quarantined_bytes(
    objects_path: &Path,
    stored: &StoredAssetRecord,
    issue: &str,
) -> Result<(), SourceAssetError> {
    if !matches!(issue, "changed" | "unreadable") {
        return Ok(());
    }
    let source = resolve_storage_path(objects_path, &stored.storage_relative_path)?;
    if inspect_stored_file(objects_path, &source)? != StoredPathInspection::Ready {
        return Ok(());
    }
    let data_root = objects_path.parent().ok_or_else(|| {
        SourceAssetError::Validation("objects directory has no data root".to_owned())
    })?;
    let quarantine = data_root.join(".source-quarantine");
    fs::create_dir_all(&quarantine)?;
    let destination = quarantine.join(format!("{}-{}", stored.id, stored.content_sha256));
    if destination.exists() {
        return Err(SourceAssetError::Integrity(
            "quarantine destination already exists".to_owned(),
        ));
    }
    fs::rename(source, destination)?;
    Ok(())
}

fn prepare_import(
    database_path: &Path,
    objects_path: &Path,
    proposal: SourceAssetImportProposal,
) -> Result<PreparedImport, SourceAssetError> {
    let source = validate_source_path(&proposal.source_path)?;
    let original_file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| SourceAssetError::Validation("file name is not valid UTF-8".to_owned()))?
        .trim()
        .to_owned();
    validate_length("file name", &original_file_name, 1, MAX_FILE_NAME_CHARS)?;
    let actor = validate_required("actor", &proposal.actor, MAX_ACTOR_CHARS)?;
    let media_type = normalize_optional("media type", proposal.media_type, MAX_MEDIA_TYPE_CHARS)?;
    let (content_sha256, byte_length) = hash_file(&source)?;
    let storage_relative_path = relative_storage_path(&content_sha256);
    let destination = resolve_storage_path(objects_path, &storage_relative_path)?;

    let connection = storage::open_writable_connection(database_path)?;
    let record_link = proposal
        .record_link
        .map(|link| validate_record_link(&connection, link))
        .transpose()?;
    let db_asset_state: Option<String> = connection
        .query_row(
            "SELECT integrity_state FROM momso_source_assets WHERE content_sha256 = ?1",
            [&content_sha256],
            |row| row.get(0),
        )
        .optional()?;
    let db_has_asset = db_asset_state.is_some();
    let mut destination_ready = false;
    match inspect_stored_file(objects_path, &destination)? {
        StoredPathInspection::Ready => {
            let (stored_sha256, stored_bytes) = hash_file(&destination)?;
            if stored_sha256 != content_sha256 || stored_bytes != byte_length {
                return Err(SourceAssetError::Integrity(
                    "the content-addressed destination contains different bytes".to_owned(),
                ));
            }
            destination_ready = true;
        }
        StoredPathInspection::Missing if db_asset_state.as_deref() == Some("active") => {
            return Err(SourceAssetError::Integrity(
                "the source file ledger points to missing stored bytes".to_owned(),
            ));
        }
        StoredPathInspection::Unsafe => {
            return Err(SourceAssetError::Integrity(
                "the content-addressed destination path is unsafe".to_owned(),
            ));
        }
        StoredPathInspection::Missing => {}
    }

    let plan_digest = import_plan_digest(
        &source,
        proposal.asset_kind,
        media_type.as_deref(),
        record_link.as_ref(),
        &actor,
        &content_sha256,
        byte_length,
    );
    Ok(PreparedImport {
        canonical_source: source,
        media_type,
        actor,
        storage_relative_path,
        plan: SourceAssetImportPlan {
            plan_digest,
            original_file_name,
            asset_kind: proposal.asset_kind,
            content_sha256,
            byte_length,
            already_stored: db_has_asset && destination_ready,
            record_link,
            requires_confirmation: true,
        },
    })
}

fn validate_source_path(value: &str) -> Result<PathBuf, SourceAssetError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SourceAssetError::Validation(
            "source path is required".to_owned(),
        ));
    }
    let input = Path::new(trimmed);
    let metadata = fs::symlink_metadata(input).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            SourceAssetError::NotFound("selected path".to_owned())
        } else {
            SourceAssetError::Io(error)
        }
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(SourceAssetError::Validation(
            "source path must be a regular file, not a directory or link".to_owned(),
        ));
    }
    reject_windows_reparse_point(&metadata)?;
    Ok(fs::canonicalize(input)?)
}

#[cfg(windows)]
fn reject_windows_reparse_point(metadata: &fs::Metadata) -> Result<(), SourceAssetError> {
    if is_windows_reparse_point(metadata) {
        return Err(SourceAssetError::Validation(
            "source path must not be a Windows reparse point".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn reject_windows_reparse_point(_metadata: &fs::Metadata) -> Result<(), SourceAssetError> {
    Ok(())
}

#[cfg(windows)]
fn is_windows_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_windows_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

fn existing_internal_directory(path: &Path) -> Result<bool, SourceAssetError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(SourceAssetError::Io(error)),
    };
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || is_windows_reparse_point(&metadata)
    {
        return Err(SourceAssetError::Integrity(
            "internal source staging path is unsafe".to_owned(),
        ));
    }
    Ok(true)
}

fn validate_record_link(
    connection: &rusqlite::Connection,
    input: SourceAssetRecordLinkInput,
) -> Result<SourceAssetRecordLink, SourceAssetError> {
    if input.record_id.trim().is_empty() || input.record_version < 1 {
        return Err(SourceAssetError::Validation(
            "record link requires an id and positive version".to_owned(),
        ));
    }
    let exists: bool = connection.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM momso_record_versions WHERE record_id = ?1 AND version = ?2
         )",
        params![input.record_id, input.record_version],
        |row| row.get(0),
    )?;
    if !exists {
        return Err(SourceAssetError::Validation(
            "the selected record version does not exist".to_owned(),
        ));
    }
    Ok(SourceAssetRecordLink {
        record_id: input.record_id,
        record_version: input.record_version,
        relation_role: input.relation_role,
    })
}

fn materialize_source_file(
    objects_path: &Path,
    prepared: &PreparedImport,
) -> Result<(), SourceAssetError> {
    let destination = resolve_storage_path(objects_path, &prepared.storage_relative_path)?;
    match inspect_stored_file(objects_path, &destination)? {
        StoredPathInspection::Ready => {
            let (sha256, byte_length) = hash_file(&destination)?;
            if sha256 == prepared.plan.content_sha256 && byte_length == prepared.plan.byte_length {
                return Ok(());
            }
            return Err(SourceAssetError::Integrity(
                "the content-addressed destination contains different bytes".to_owned(),
            ));
        }
        StoredPathInspection::Unsafe => {
            return Err(SourceAssetError::Integrity(
                "the content-addressed destination path is unsafe".to_owned(),
            ));
        }
        StoredPathInspection::Missing => {}
    }

    let data_root = objects_path.parent().ok_or_else(|| {
        SourceAssetError::Validation("objects directory has no data root".to_owned())
    })?;
    let staging = data_root.join(".source-staging");
    if !existing_internal_directory(&staging)? {
        fs::create_dir(&staging)?;
    }
    if !existing_internal_directory(&staging)? {
        return Err(SourceAssetError::Integrity(
            "source staging directory was not created".to_owned(),
        ));
    }
    let temporary = staging.join(format!("import-{}.tmp", Uuid::new_v4()));
    let copy_result = copy_and_hash(&prepared.canonical_source, &temporary);
    let (copied_sha256, copied_bytes) = match copy_result {
        Ok(result) => result,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    };
    if copied_sha256 != prepared.plan.content_sha256 || copied_bytes != prepared.plan.byte_length {
        let _ = fs::remove_file(&temporary);
        return Err(SourceAssetError::PlanMismatch);
    }

    let finalize_result = (|| {
        if let Some(parent) = destination.parent() {
            ensure_existing_storage_parents_safe(objects_path, &destination)?;
            fs::create_dir_all(parent)?;
            ensure_existing_storage_parents_safe(objects_path, &destination)?;
        }
        if matches!(
            inspect_stored_file(objects_path, &destination)?,
            StoredPathInspection::Unsafe
        ) {
            return Err(SourceAssetError::Integrity(
                "the content-addressed destination path became unsafe".to_owned(),
            ));
        }
        match fs::rename(&temporary, &destination) {
            Ok(()) => Ok(()),
            Err(error) => match inspect_stored_file(objects_path, &destination)? {
                StoredPathInspection::Ready => {
                    let (sha256, byte_length) = hash_file(&destination)?;
                    if sha256 == prepared.plan.content_sha256
                        && byte_length == prepared.plan.byte_length
                    {
                        Ok(())
                    } else {
                        Err(SourceAssetError::Integrity(
                            "a concurrent source import created different destination bytes"
                                .to_owned(),
                        ))
                    }
                }
                StoredPathInspection::Unsafe => Err(SourceAssetError::Integrity(
                    "the content-addressed destination became unsafe".to_owned(),
                )),
                StoredPathInspection::Missing => Err(SourceAssetError::Io(error)),
            },
        }
    })();
    if finalize_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    finalize_result
}

fn copy_and_hash(source: &Path, destination: &Path) -> Result<(String, u64), SourceAssetError> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut hasher = Sha256::new();
    let mut byte_length = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
        byte_length = byte_length
            .checked_add(read as u64)
            .ok_or_else(|| SourceAssetError::Validation("source file is too large".to_owned()))?;
    }
    output.sync_all()?;
    Ok((format!("{:x}", hasher.finalize()), byte_length))
}

fn hash_file(path: &Path) -> Result<(String, u64), SourceAssetError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut byte_length = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        byte_length = byte_length
            .checked_add(read as u64)
            .ok_or_else(|| SourceAssetError::Validation("source file is too large".to_owned()))?;
    }
    Ok((format!("{:x}", hasher.finalize()), byte_length))
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn relative_storage_path(content_sha256: &str) -> String {
    format!("sha256/{}/{}", &content_sha256[..2], content_sha256)
}

fn resolve_storage_path(
    objects_path: &Path,
    relative_path: &str,
) -> Result<PathBuf, SourceAssetError> {
    let components = Path::new(relative_path).components().collect::<Vec<_>>();
    let valid = matches!(
        components.as_slice(),
        [Component::Normal(root), Component::Normal(prefix), Component::Normal(hash)]
            if root.to_string_lossy() == "sha256"
                && prefix.to_string_lossy().len() == 2
                && hash.to_string_lossy().len() == 64
                && hash.to_string_lossy().starts_with(prefix.to_string_lossy().as_ref())
                && hash.to_string_lossy().chars().all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
    );
    if !valid {
        return Err(SourceAssetError::Integrity(
            "stored source file path is outside the content-addressed layout".to_owned(),
        ));
    }
    Ok(objects_path.join(relative_path))
}

fn inspect_stored_file(
    objects_path: &Path,
    path: &Path,
) -> Result<StoredPathInspection, std::io::Error> {
    let relative = match path.strip_prefix(objects_path) {
        Ok(relative) => relative,
        Err(_) => return Ok(StoredPathInspection::Unsafe),
    };
    let mut current = objects_path.to_path_buf();
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in std::iter::once(None)
        .chain(components.iter().map(Some))
        .enumerate()
    {
        if let Some(component) = component {
            current.push(component.as_os_str());
        }
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(StoredPathInspection::Missing);
            }
            Err(error) => return Err(error),
        };
        if metadata.file_type().is_symlink() || is_windows_reparse_point(&metadata) {
            return Ok(StoredPathInspection::Unsafe);
        }
        let is_leaf = index == components.len();
        if (is_leaf && !metadata.is_file()) || (!is_leaf && !metadata.is_dir()) {
            return Ok(StoredPathInspection::Unsafe);
        }
    }
    Ok(StoredPathInspection::Ready)
}

fn ensure_existing_storage_parents_safe(
    objects_path: &Path,
    destination: &Path,
) -> Result<(), SourceAssetError> {
    let relative_parent = destination
        .parent()
        .and_then(|parent| parent.strip_prefix(objects_path).ok())
        .ok_or_else(|| {
            SourceAssetError::Integrity(
                "stored source file parent is outside the objects directory".to_owned(),
            )
        })?;
    let mut current = objects_path.to_path_buf();
    for component in std::iter::once(None).chain(relative_parent.components().map(Some)) {
        if let Some(component) = component {
            current.push(component.as_os_str());
        }
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(SourceAssetError::Io(error)),
        };
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || is_windows_reparse_point(&metadata)
        {
            return Err(SourceAssetError::Integrity(
                "stored source file parent path is unsafe".to_owned(),
            ));
        }
    }
    Ok(())
}

fn import_plan_digest(
    source: &Path,
    kind: SourceAssetKind,
    media_type: Option<&str>,
    link: Option<&SourceAssetRecordLink>,
    actor: &str,
    content_sha256: &str,
    byte_length: u64,
) -> String {
    let mut hasher = Sha256::new();
    push_digest_field(&mut hasher, b"momso.source-asset-import.v1");
    push_digest_field(&mut hasher, source.to_string_lossy().as_bytes());
    push_digest_field(&mut hasher, kind.as_db().as_bytes());
    push_digest_field(&mut hasher, media_type.unwrap_or("").as_bytes());
    push_digest_field(&mut hasher, actor.as_bytes());
    push_digest_field(&mut hasher, content_sha256.as_bytes());
    push_digest_field(&mut hasher, byte_length.to_string().as_bytes());
    push_digest_field(
        &mut hasher,
        link.map(|value| value.record_id.as_str())
            .unwrap_or("")
            .as_bytes(),
    );
    push_digest_field(
        &mut hasher,
        link.map(|value| value.record_version.to_string())
            .unwrap_or_default()
            .as_bytes(),
    );
    push_digest_field(
        &mut hasher,
        link.map(|value| value.relation_role.as_db())
            .unwrap_or("")
            .as_bytes(),
    );
    format!("{:x}", hasher.finalize())
}

fn quarantine_plan_digest(stored: &StoredAssetRecord, actor: &str, issue: &str) -> String {
    let mut hasher = Sha256::new();
    push_digest_field(&mut hasher, b"momso.source-asset-quarantine.v1");
    push_digest_field(&mut hasher, stored.id.as_bytes());
    push_digest_field(&mut hasher, stored.content_sha256.as_bytes());
    push_digest_field(&mut hasher, stored.byte_length.to_string().as_bytes());
    push_digest_field(&mut hasher, stored.storage_relative_path.as_bytes());
    push_digest_field(&mut hasher, actor.as_bytes());
    push_digest_field(&mut hasher, issue.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn push_digest_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn validate_required(
    field: &str,
    value: &str,
    max_chars: usize,
) -> Result<String, SourceAssetError> {
    let normalized = value.trim();
    validate_length(field, normalized, 1, max_chars)?;
    Ok(normalized.to_owned())
}

fn normalize_optional(
    field: &str,
    value: Option<String>,
    max_chars: usize,
) -> Result<Option<String>, SourceAssetError> {
    value
        .map(|value| {
            let normalized = value.trim();
            if normalized.is_empty() {
                Ok(None)
            } else {
                validate_length(field, normalized, 1, max_chars)?;
                Ok(Some(normalized.to_owned()))
            }
        })
        .transpose()
        .map(Option::flatten)
}

fn validate_length(
    field: &str,
    value: &str,
    min_chars: usize,
    max_chars: usize,
) -> Result<(), SourceAssetError> {
    let count = value.chars().count();
    if count < min_chars || count > max_chars {
        return Err(SourceAssetError::Validation(format!(
            "{field} must contain between {min_chars} and {max_chars} characters"
        )));
    }
    Ok(())
}

fn u64_to_i64(value: u64, field: &str) -> Result<i64, SourceAssetError> {
    i64::try_from(value).map_err(|_| SourceAssetError::Validation(format!("{field} is too large")))
}

fn read_asset_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceAsset> {
    let kind_value: String = row.get(1)?;
    let byte_length: i64 = row.get(3)?;
    Ok(SourceAsset {
        id: row.get(0)?,
        asset_kind: SourceAssetKind::from_db(&kind_value).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        content_sha256: row.get(2)?,
        byte_length: u64::try_from(byte_length).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        original_file_name: row.get(4)?,
        media_type: row.get(5)?,
        captured_by: row.get(6)?,
        created_at: row.get(7)?,
        integrity_state: SourceAssetIntegrityState::from_db(&row.get::<_, String>(8)?).map_err(
            |error| {
                rusqlite::Error::FromSqlConversionFailure(
                    8,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            },
        )?,
        integrity_issue: row.get(9)?,
        quarantined_at: row.get(10)?,
        quarantined_by: row.get(11)?,
        links: Vec::new(),
    })
}

fn load_links(
    connection: &rusqlite::Connection,
    asset_id: &str,
) -> Result<Vec<SourceAssetRecordLink>, SourceAssetError> {
    let mut statement = connection.prepare(
        "SELECT record_id, record_version, relation_role
         FROM momso_record_source_assets
         WHERE asset_id = ?1
         ORDER BY record_id, record_version, relation_role",
    )?;
    let rows = statement.query_map([asset_id], |row| {
        let role: String = row.get(2)?;
        Ok(SourceAssetRecordLink {
            record_id: row.get(0)?,
            record_version: row.get(1)?,
            relation_role: SourceAssetRelationRole::from_db(&role).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(SourceAssetError::from)
}

fn get(database_path: &Path, asset_id: &str) -> Result<SourceAsset, SourceAssetError> {
    let connection = storage::open_writable_connection(database_path)?;
    let mut asset = connection
        .query_row(
            "SELECT id, asset_kind, content_sha256, byte_length,
                    original_file_name, media_type, captured_by, created_at,
                    integrity_state, integrity_issue, quarantined_at, quarantined_by
             FROM momso_source_assets WHERE id = ?1",
            [asset_id],
            read_asset_row,
        )
        .optional()?
        .ok_or_else(|| SourceAssetError::NotFound(asset_id.to_owned()))?;
    asset.links = load_links(&connection, asset_id)?;
    Ok(asset)
}

fn stored_asset(
    database_path: &Path,
    asset_id: &str,
) -> Result<StoredAssetRecord, SourceAssetError> {
    let connection = storage::open_writable_connection(database_path)?;
    connection
        .query_row(
            "SELECT id, content_sha256, byte_length, storage_relative_path, integrity_state
             FROM momso_source_assets WHERE id = ?1",
            [asset_id],
            |row| {
                let byte_length: i64 = row.get(2)?;
                Ok(StoredAssetRecord {
                    id: row.get(0)?,
                    content_sha256: row.get(1)?,
                    byte_length: u64::try_from(byte_length).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    storage_relative_path: row.get(3)?,
                    integrity_state: SourceAssetIntegrityState::from_db(&row.get::<_, String>(4)?)
                        .map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                4,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| SourceAssetError::NotFound(asset_id.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{self, CreateRecordDraftInput};
    use tempfile::TempDir;

    struct TestStore {
        _temp: TempDir,
        database_path: PathBuf,
        objects_path: PathBuf,
        source_path: PathBuf,
    }

    impl TestStore {
        fn new(source: &str) -> Self {
            let temp = tempfile::tempdir().expect("temp dir");
            let database_path = temp.path().join("momso.db");
            let objects_path = temp.path().join("objects");
            let source_path = temp.path().join("recording.txt");
            fs::create_dir_all(&objects_path).expect("objects path");
            fs::write(&source_path, source).expect("source writes");
            storage::initialize(&database_path, &temp.path().join("backups"), "test")
                .expect("storage initializes");
            Self {
                _temp: temp,
                database_path,
                objects_path,
                source_path,
            }
        }

        fn proposal(&self) -> SourceAssetImportProposal {
            SourceAssetImportProposal {
                source_path: self.source_path.to_string_lossy().into_owned(),
                asset_kind: SourceAssetKind::RawTranscript,
                media_type: Some("text/plain".to_owned()),
                record_link: None,
                actor: "tester".to_owned(),
            }
        }

        fn import(&self) -> SourceAsset {
            let proposal = self.proposal();
            let plan = plan_import(&self.database_path, &self.objects_path, proposal.clone())
                .expect("plan succeeds");
            apply_import(
                &self.database_path,
                &self.objects_path,
                ApplySourceAssetImportInput {
                    proposal,
                    plan_digest: plan.plan_digest,
                    import_confirmed: true,
                },
            )
            .expect("import succeeds")
        }
    }

    #[test]
    fn import_preserves_exact_bytes_at_a_content_address() {
        let store = TestStore::new("바순은 원본 표현입니다.\n");
        let asset = store.import();
        let relative = relative_storage_path(&asset.content_sha256);
        let stored = resolve_storage_path(&store.objects_path, &relative).expect("safe path");

        assert_eq!(
            fs::read(stored).expect("stored bytes"),
            "바순은 원본 표현입니다.\n".as_bytes()
        );
        assert_eq!(list(&store.database_path, None).expect("assets").len(), 1);
    }

    #[test]
    fn changed_source_can_be_quarantined_without_erasing_its_ledger() {
        let store = TestStore::new("original bytes");
        let asset = store.import();
        let connection =
            storage::open_writable_connection(&store.database_path).expect("database opens");
        assert!(connection
            .execute(
                "UPDATE momso_source_assets SET integrity_state = 'quarantined' WHERE id = ?1",
                [&asset.id],
            )
            .is_err());
        drop(connection);
        let stored_path = resolve_storage_path(
            &store.objects_path,
            &relative_storage_path(&asset.content_sha256),
        )
        .expect("safe path");
        fs::write(&stored_path, b"changed bytes").expect("stored bytes change");
        let proposal = SourceAssetQuarantineProposal {
            asset_id: asset.id.clone(),
            actor: "human reviewer".to_owned(),
        };
        let plan = plan_quarantine(&store.database_path, &store.objects_path, proposal.clone())
            .expect("quarantine plans");
        assert_eq!(plan.issue, "changed");
        assert!(matches!(
            apply_quarantine(
                &store.database_path,
                &store.objects_path,
                ApplySourceAssetQuarantineInput {
                    proposal: proposal.clone(),
                    plan_digest: plan.plan_digest.clone(),
                    quarantine_confirmed: false,
                },
            ),
            Err(SourceAssetError::QuarantineConfirmationRequired)
        ));

        let quarantined = apply_quarantine(
            &store.database_path,
            &store.objects_path,
            ApplySourceAssetQuarantineInput {
                proposal,
                plan_digest: plan.plan_digest,
                quarantine_confirmed: true,
            },
        )
        .expect("quarantine applies");

        assert_eq!(
            quarantined.integrity_state,
            SourceAssetIntegrityState::Quarantined
        );
        assert_eq!(quarantined.integrity_issue.as_deref(), Some("changed"));
        assert_eq!(
            quarantined.quarantined_by.as_deref(),
            Some("human reviewer")
        );
        assert!(!stored_path.exists());
        assert!(stored_assets(&store.database_path)
            .expect("active ledger")
            .is_empty());
        assert_eq!(
            list(&store.database_path, None).expect("full ledger").len(),
            1
        );
        crate::backup_profile::local_content_state(&store.database_path, &store.objects_path)
            .expect("quarantined ledger no longer blocks backup");
    }

    #[test]
    fn stale_preview_is_rejected_without_replacing_the_planned_bytes() {
        let store = TestStore::new("first");
        let proposal = store.proposal();
        let plan = plan_import(&store.database_path, &store.objects_path, proposal.clone())
            .expect("plan succeeds");
        fs::write(&store.source_path, "second").expect("source changes");

        let error = apply_import(
            &store.database_path,
            &store.objects_path,
            ApplySourceAssetImportInput {
                proposal,
                plan_digest: plan.plan_digest,
                import_confirmed: true,
            },
        )
        .expect_err("stale plan fails");
        assert!(matches!(error, SourceAssetError::PlanMismatch));
        assert!(stored_assets(&store.database_path)
            .expect("ledger")
            .is_empty());
    }

    #[test]
    fn duplicate_bytes_reuse_one_stored_asset() {
        let store = TestStore::new("same bytes");
        let first = store.import();
        let second = store.import();

        assert_eq!(first.id, second.id);
        assert_eq!(
            stored_assets(&store.database_path).expect("ledger").len(),
            1
        );
    }

    #[test]
    fn source_file_can_be_bound_to_an_exact_record_version() {
        let store = TestStore::new("raw transcript");
        let input = serde_json::from_value::<CreateRecordDraftInput>(serde_json::json!({
            "recordKind": "class_record",
            "title": "Morning class",
            "body": "review copy",
            "summary": null,
            "actor": "tester"
        }))
        .expect("record input");
        records::create_draft(&store.database_path, input).expect("record creates");
        let connection =
            storage::open_writable_connection(&store.database_path).expect("database opens");
        let (record_id, record_version): (String, i64) = connection
            .query_row(
                "SELECT id, current_version FROM momso_records LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("record identity");
        let mut proposal = store.proposal();
        proposal.record_link = Some(SourceAssetRecordLinkInput {
            record_id: record_id.clone(),
            record_version,
            relation_role: SourceAssetRelationRole::Source,
        });
        let plan = plan_import(&store.database_path, &store.objects_path, proposal.clone())
            .expect("plan succeeds");
        let asset = apply_import(
            &store.database_path,
            &store.objects_path,
            ApplySourceAssetImportInput {
                proposal,
                plan_digest: plan.plan_digest,
                import_confirmed: true,
            },
        )
        .expect("import succeeds");

        assert_eq!(asset.links.len(), 1);
        assert_eq!(asset.links[0].record_id, record_id);
        assert_eq!(asset.links[0].record_version, 1);
    }

    #[test]
    fn continuation_pages_are_bound_to_the_first_complete_hash() {
        let store = TestStore::new("가나다라마바사아자차카타파하");
        let asset = store.import();
        let first = read_text_page(
            &store.database_path,
            &store.objects_path,
            &asset.id,
            0,
            None,
            8,
        )
        .expect("first page");
        let second = read_text_page(
            &store.database_path,
            &store.objects_path,
            &asset.id,
            first.next_cursor.expect("continuation"),
            Some(&first.content_sha256),
            8,
        )
        .expect("second page");

        assert_eq!(first.content, "가나");
        assert_eq!(second.content, "다라");
        assert!(matches!(
            read_text_page(
                &store.database_path,
                &store.objects_path,
                &asset.id,
                second.next_cursor.expect("next"),
                Some("changed"),
                8,
            ),
            Err(SourceAssetError::ContentChanged)
        ));
    }

    #[test]
    fn integrity_audit_reports_changed_bytes_without_exposing_them() {
        let store = TestStore::new("original");
        let asset = store.import();
        let stored = resolve_storage_path(
            &store.objects_path,
            &relative_storage_path(&asset.content_sha256),
        )
        .expect("safe path");
        fs::write(stored, "changed").expect("stored bytes change");
        let mut progress_updates = Vec::new();

        let report = audit_integrity(
            &store.database_path,
            &store.objects_path,
            |completed, total| {
                progress_updates.push((completed, total));
                Ok(())
            },
            || false,
        )
        .expect("audit completes");

        assert_eq!(report.total_assets, 1);
        assert_eq!(report.total_object_files, 1);
        assert_eq!(report.healthy_assets, 0);
        assert_eq!(report.changed_assets, 1);
        assert_eq!(report.issue_samples[0].asset_id, asset.id);
        assert_eq!(progress_updates, vec![(0, 2), (1, 2), (2, 2)]);
    }

    #[test]
    fn integrity_audit_rejects_a_non_file_at_a_registered_storage_path() {
        let store = TestStore::new("original");
        let asset = store.import();
        let stored = resolve_storage_path(
            &store.objects_path,
            &relative_storage_path(&asset.content_sha256),
        )
        .expect("safe path");
        fs::remove_file(&stored).expect("stored file removes");
        fs::create_dir(&stored).expect("directory replaces file");

        let report = audit_integrity(
            &store.database_path,
            &store.objects_path,
            |_completed, _total| Ok(()),
            || false,
        )
        .expect("audit completes");

        assert_eq!(report.healthy_assets, 0);
        assert_eq!(report.unsafe_path_assets, 1);
        assert!(matches!(
            read_text_page(
                &store.database_path,
                &store.objects_path,
                &asset.id,
                0,
                None,
                8,
            ),
            Err(SourceAssetError::Integrity(_))
        ));
    }

    #[test]
    fn integrity_audit_counts_files_that_have_no_ledger_entry() {
        let store = TestStore::new("source outside storage");
        fs::write(store.objects_path.join("unregistered.mp3"), b"bytes")
            .expect("loose file writes");

        let report = audit_integrity(
            &store.database_path,
            &store.objects_path,
            |_completed, _total| Ok(()),
            || false,
        )
        .expect("audit completes");

        assert_eq!(report.total_assets, 0);
        assert_eq!(report.total_object_files, 1);
        assert_eq!(report.unregistered_files, 1);
    }

    #[test]
    fn startup_cleanup_removes_only_reserved_import_temporary_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let objects_path = temp.path().join("objects");
        let staging = temp.path().join(".source-staging");
        fs::create_dir_all(&objects_path).expect("objects create");
        fs::create_dir_all(&staging).expect("staging create");
        fs::write(staging.join("import-abcd.tmp"), b"partial").expect("partial writes");
        fs::write(staging.join("keep.txt"), b"unrelated").expect("unrelated writes");

        assert_eq!(cleanup_staging(&objects_path).expect("cleanup"), 1);
        assert!(!staging.join("import-abcd.tmp").exists());
        assert!(staging.join("keep.txt").exists());
    }
}
