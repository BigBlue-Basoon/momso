use crate::{
    storage::{self, StorageError},
    text_paging,
};
use rusqlite::{params, types::Type, Connection, OptionalExtension, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io, path::Path};
use thiserror::Error;
use uuid::Uuid;

const MAX_TITLE_CHARS: usize = 200;
const MAX_SUMMARY_CHARS: usize = 4_000;
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_ACTOR_CHARS: usize = 120;
const MAX_RECORD_KIND_CHARS: usize = 64;

#[derive(Debug, Error)]
pub(crate) enum RecordError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Invalid record input: {0}")]
    Validation(String),
    #[error("Record {0} was not found")]
    NotFound(String),
    #[error("Record {record_id} changed while this plan was open (expected version {expected}, found {found})")]
    VersionConflict {
        record_id: String,
        expected: i64,
        found: i64,
    },
    #[error("The change plan no longer matches the proposed change; create a new preview")]
    PlanMismatch,
    #[error("A named reviewer must explicitly confirm this change before it is applied")]
    ReviewRequired,
    #[error("The proposed operation does not change the record")]
    NoChanges,
    #[error("Local record integrity check failed: {0}")]
    Integrity(String),
    #[error("A continuation read requires the complete body hash returned by the first page")]
    ContinuationHashRequired,
    #[error("The record body changed after the first page was read")]
    ContentChanged,
    #[error(transparent)]
    Paging(#[from] text_paging::TextPagingError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecordStatus {
    Draft,
    NeedsReview,
    Reviewed,
}

impl RecordStatus {
    fn as_db(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::NeedsReview => "needs_review",
            Self::Reviewed => "reviewed",
        }
    }

    fn from_db(value: &str) -> Result<Self, RecordError> {
        match value {
            "draft" => Ok(Self::Draft),
            "needs_review" => Ok(Self::NeedsReview),
            "reviewed" => Ok(Self::Reviewed),
            _ => Err(RecordError::Integrity(format!(
                "unknown record status {value:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SummaryState {
    MissingSummary,
    Unconfirmed,
    Fresh,
    Stale,
    BodyIntegrityError,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateRecordDraftInput {
    record_kind: String,
    title: String,
    body: String,
    summary: Option<String>,
    actor: String,
    #[serde(default)]
    initial_status: Option<RecordStatus>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordChangeProposal {
    record_id: String,
    expected_version: i64,
    record_kind: String,
    title: String,
    body: String,
    summary: Option<String>,
    target_status: RecordStatus,
    actor: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplyRecordChangeInput {
    proposal: RecordChangeProposal,
    plan_digest: String,
    reviewer: String,
    review_confirmed: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordRestoreProposal {
    record_id: String,
    expected_version: i64,
    restore_version: i64,
    actor: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplyRecordRestoreInput {
    proposal: RecordRestoreProposal,
    plan_digest: String,
    reviewer: String,
    review_confirmed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordSummary {
    id: String,
    record_kind: String,
    title: String,
    status: RecordStatus,
    current_version: i64,
    summary: Option<String>,
    summary_state: SummaryState,
    body_bytes: i64,
    updated_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordReadinessReport {
    records: Vec<RecordSummary>,
    metadata_read_count: usize,
    body_read_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordDetail {
    id: String,
    record_kind: String,
    title: String,
    status: RecordStatus,
    current_version: i64,
    summary: Option<String>,
    summary_state: SummaryState,
    body: String,
    body_sha256: String,
    body_bytes: i64,
    created_by: String,
    updated_by: String,
    reviewed_by: Option<String>,
    reviewed_at: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordBodyPage {
    record_id: String,
    version: i64,
    body_sha256: String,
    body_bytes: i64,
    cursor: u64,
    next_cursor: Option<u64>,
    content: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordChangePlan {
    record_id: String,
    current_version: i64,
    next_version: i64,
    changed_fields: Vec<String>,
    target_status: RecordStatus,
    summary_state_after: SummaryState,
    body_bytes_after: i64,
    requires_confirmation: bool,
    plan_digest: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordRestorePlan {
    record_id: String,
    current_version: i64,
    next_version: i64,
    restore_version: i64,
    changed_fields: Vec<String>,
    target_status: RecordStatus,
    summary_state_after: SummaryState,
    body_bytes_after: i64,
    requires_confirmation: bool,
    plan_digest: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordVersionEntry {
    version: i64,
    record_kind: String,
    title: String,
    status: RecordStatus,
    summary: Option<String>,
    summary_state: SummaryState,
    body_sha256: String,
    body_bytes: i64,
    change_kind: String,
    source_version: Option<i64>,
    actor: String,
    confirmed_by: Option<String>,
    reviewed_by: Option<String>,
    reviewed_at: Option<String>,
    plan_digest: Option<String>,
    created_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordHistory {
    record_id: String,
    versions: Vec<RecordVersionEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SummaryAuditEntry {
    record_id: String,
    current_version: i64,
    state: SummaryState,
    expected_body_sha256: String,
    actual_body_sha256: Option<String>,
    issue: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SummaryFreshnessReport {
    records: Vec<SummaryAuditEntry>,
    metadata_read_count: usize,
    body_read_count: usize,
    integrity_issue_count: usize,
}

#[derive(Debug, Clone)]
struct CurrentRecord {
    id: String,
    record_kind: String,
    title: String,
    status: RecordStatus,
    current_version: i64,
    summary: Option<String>,
    summary_sha256: Option<String>,
    summary_basis_body_sha256: Option<String>,
    body_sha256: String,
    body_bytes: i64,
    created_by: String,
    updated_by: String,
    reviewed_by: Option<String>,
    reviewed_at: Option<String>,
    created_at: String,
    updated_at: String,
}

impl CurrentRecord {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let status_value: String = row.get(3)?;
        let status = status_from_database(3, &status_value)?;
        Ok(Self {
            id: row.get(0)?,
            record_kind: row.get(1)?,
            title: row.get(2)?,
            status,
            current_version: row.get(4)?,
            summary: row.get(5)?,
            summary_sha256: row.get(6)?,
            summary_basis_body_sha256: row.get(7)?,
            body_sha256: row.get(8)?,
            body_bytes: row.get(9)?,
            created_by: row.get(10)?,
            updated_by: row.get(11)?,
            reviewed_by: row.get(12)?,
            reviewed_at: row.get(13)?,
            created_at: row.get(14)?,
            updated_at: row.get(15)?,
        })
    }
}

#[derive(Debug, Clone)]
struct ContentState {
    record_kind: String,
    title: String,
    status: RecordStatus,
    summary: Option<String>,
    summary_sha256: Option<String>,
    summary_basis_body_sha256: Option<String>,
    body: String,
    body_sha256: String,
    body_bytes: i64,
}

#[derive(Debug)]
struct PreparedChange {
    current: CurrentRecord,
    state: ContentState,
    actor: String,
    plan: RecordChangePlan,
}

#[derive(Debug)]
struct PreparedRestore {
    current: CurrentRecord,
    state: ContentState,
    actor: String,
    source_version: i64,
    plan: RecordRestorePlan,
}

pub(crate) fn create_draft(
    database_path: &Path,
    input: CreateRecordDraftInput,
) -> Result<RecordDetail, RecordError> {
    let record_kind = validate_record_kind(&input.record_kind)?;
    let title = validate_title(&input.title)?;
    validate_body(&input.body)?;
    let summary = normalize_summary(input.summary)?;
    let actor = validate_person("actor", &input.actor)?;
    let status = input.initial_status.unwrap_or(RecordStatus::Draft);
    if status == RecordStatus::Reviewed {
        return Err(RecordError::Validation(
            "a new record cannot begin in the reviewed state".to_owned(),
        ));
    }
    let body_sha256 = hash_text(&input.body);
    let body_bytes = usize_to_i64(input.body.len(), "body size")?;
    let summary_sha256 = summary.as_deref().map(hash_text);
    let record_id = Uuid::new_v4().to_string();
    let mut connection = storage::open_writable_connection(database_path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let timestamp = current_timestamp(&transaction)?;
    transaction.execute(
        "INSERT INTO momso_records (
           id, record_kind, title, status, current_version,
           summary, summary_sha256, summary_basis_body_sha256,
           body_sha256, body_bytes,
           created_by, updated_by, reviewed_by, reviewed_at,
           created_at, updated_at
         ) VALUES (
           ?1, ?2, ?3, ?4, 1,
           ?5, ?6, NULL,
           ?7, ?8,
           ?9, ?9, NULL, NULL,
           ?10, ?10
         )",
        params![
            record_id,
            record_kind,
            title,
            status.as_db(),
            summary,
            summary_sha256,
            body_sha256,
            body_bytes,
            actor,
            timestamp,
        ],
    )?;
    transaction.execute(
        "INSERT INTO momso_record_versions (
           record_id, version, record_kind, title, status,
           summary, summary_sha256, summary_basis_body_sha256,
           body_sha256, body_bytes, change_kind, source_version,
           actor, confirmed_by, reviewed_by, reviewed_at, plan_digest, created_at
         ) VALUES (
           ?1, 1, ?2, ?3, ?4,
           ?5, ?6, NULL,
           ?7, ?8, 'create', NULL,
           ?9, NULL, NULL, NULL, NULL, ?10
         )",
        params![
            record_id,
            record_kind,
            title,
            status.as_db(),
            summary,
            summary_sha256,
            body_sha256,
            body_bytes,
            actor,
            timestamp,
        ],
    )?;
    transaction.execute(
        "INSERT INTO momso_record_bodies (
           record_id, version, body, body_sha256, body_bytes
         ) VALUES (?1, 1, ?2, ?3, ?4)",
        params![record_id, input.body, body_sha256, body_bytes],
    )?;
    transaction.commit()?;

    get_record(database_path, &record_id)
}

pub(crate) fn list_summaries(database_path: &Path) -> Result<RecordReadinessReport, RecordError> {
    let connection = storage::open_writable_connection(database_path)?;
    let records = load_all_current_records(&connection)?;
    let summaries = records
        .into_iter()
        .map(|record| RecordSummary {
            id: record.id,
            record_kind: record.record_kind,
            title: record.title,
            status: record.status,
            current_version: record.current_version,
            summary_state: summary_state(
                record.summary.as_deref(),
                record.summary_basis_body_sha256.as_deref(),
                &record.body_sha256,
            ),
            summary: record.summary,
            body_bytes: record.body_bytes,
            updated_at: record.updated_at,
        })
        .collect::<Vec<_>>();

    Ok(RecordReadinessReport {
        metadata_read_count: summaries.len(),
        body_read_count: 0,
        records: summaries,
    })
}

pub(crate) fn read_body_page(
    database_path: &Path,
    record_id: &str,
    version: Option<i64>,
    cursor: u64,
    expected_body_sha256: Option<&str>,
    max_bytes: u64,
) -> Result<RecordBodyPage, RecordError> {
    if cursor > 0 && expected_body_sha256.is_none() {
        return Err(RecordError::ContinuationHashRequired);
    }
    let connection = storage::open_writable_connection(database_path)?;
    let selected_version = match version {
        Some(version) if version >= 1 => version,
        Some(_) => {
            return Err(RecordError::Validation(
                "record version must be positive".to_owned(),
            ))
        }
        None => connection
            .query_row(
                "SELECT current_version FROM momso_records WHERE id = ?1",
                [record_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| RecordError::NotFound(record_id.to_owned()))?,
    };
    let (body, body_sha256, body_bytes, version_sha256, version_bytes): (
        String,
        String,
        i64,
        String,
        i64,
    ) = connection
        .query_row(
            "SELECT b.body, b.body_sha256, b.body_bytes,
                    v.body_sha256, v.body_bytes
             FROM momso_record_bodies b
             JOIN momso_record_versions v
               ON v.record_id = b.record_id AND v.version = b.version
             WHERE b.record_id = ?1 AND b.version = ?2",
            params![record_id, selected_version],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| RecordError::NotFound(format!("{record_id} version {selected_version}")))?;
    let actual_sha256 = hash_text(&body);
    let actual_bytes = usize_to_i64(body.len(), "body size")?;
    if actual_sha256 != body_sha256
        || actual_bytes != body_bytes
        || actual_sha256 != version_sha256
        || actual_bytes != version_bytes
    {
        return Err(RecordError::Integrity(format!(
            "body and version ledger differ for {record_id} version {selected_version}"
        )));
    }
    if expected_body_sha256.is_some_and(|expected| expected != actual_sha256) {
        return Err(RecordError::ContentChanged);
    }
    let page = text_paging::page(&body, cursor, max_bytes)?;
    Ok(RecordBodyPage {
        record_id: record_id.to_owned(),
        version: selected_version,
        body_sha256: actual_sha256,
        body_bytes: actual_bytes,
        cursor,
        next_cursor: page.next_cursor,
        content: page.content,
    })
}

pub(crate) fn get_record(
    database_path: &Path,
    record_id: &str,
) -> Result<RecordDetail, RecordError> {
    let record_id = validate_record_id(record_id)?;
    let connection = storage::open_writable_connection(database_path)?;
    let record = load_current_record(&connection, &record_id)?;
    let body = load_verified_body(
        &connection,
        &record.id,
        record.current_version,
        &record.body_sha256,
        record.body_bytes,
    )?;

    Ok(detail_from_parts(record, body))
}

pub(crate) fn plan_change(
    database_path: &Path,
    proposal: RecordChangeProposal,
) -> Result<RecordChangePlan, RecordError> {
    let connection = storage::open_writable_connection(database_path)?;
    Ok(prepare_change(&connection, proposal)?.plan)
}

pub(crate) fn apply_change(
    database_path: &Path,
    input: ApplyRecordChangeInput,
) -> Result<RecordDetail, RecordError> {
    let reviewer = require_review(&input.reviewer, input.review_confirmed)?;
    let mut connection = storage::open_writable_connection(database_path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let prepared = prepare_change(&transaction, input.proposal)?;
    if prepared.plan.plan_digest != input.plan_digest {
        return Err(RecordError::PlanMismatch);
    }

    let timestamp = current_timestamp(&transaction)?;
    let next_version = prepared.plan.next_version;
    let (reviewed_by, reviewed_at) = if prepared.state.status == RecordStatus::Reviewed {
        (Some(reviewer.as_str()), Some(timestamp.as_str()))
    } else {
        (None, None)
    };

    insert_version(
        &transaction,
        &prepared.current.id,
        next_version,
        &prepared.state,
        "update",
        None,
        &prepared.actor,
        Some(&reviewer),
        reviewed_by,
        reviewed_at,
        Some(&prepared.plan.plan_digest),
        &timestamp,
    )?;
    update_current_record(
        &transaction,
        &prepared.current.id,
        prepared.current.current_version,
        next_version,
        &prepared.state,
        &prepared.actor,
        reviewed_by,
        reviewed_at,
        &timestamp,
    )?;
    transaction.commit()?;

    get_record(database_path, &prepared.current.id)
}

pub(crate) fn list_history(
    database_path: &Path,
    record_id: &str,
) -> Result<RecordHistory, RecordError> {
    let record_id = validate_record_id(record_id)?;
    let connection = storage::open_writable_connection(database_path)?;
    load_current_record(&connection, &record_id)?;
    let mut statement = connection.prepare(
        "SELECT version, record_kind, title, status,
                summary, summary_basis_body_sha256, body_sha256, body_bytes,
                change_kind, source_version, actor, confirmed_by,
                reviewed_by, reviewed_at, plan_digest, created_at
         FROM momso_record_versions
         WHERE record_id = ?1
         ORDER BY version DESC",
    )?;
    let versions = statement
        .query_map([&record_id], |row| {
            let status_value: String = row.get(3)?;
            let status = status_from_database(3, &status_value)?;
            let summary: Option<String> = row.get(4)?;
            let summary_basis: Option<String> = row.get(5)?;
            let body_sha256: String = row.get(6)?;
            Ok(RecordVersionEntry {
                version: row.get(0)?,
                record_kind: row.get(1)?,
                title: row.get(2)?,
                status,
                summary_state: summary_state(
                    summary.as_deref(),
                    summary_basis.as_deref(),
                    &body_sha256,
                ),
                summary,
                body_sha256,
                body_bytes: row.get(7)?,
                change_kind: row.get(8)?,
                source_version: row.get(9)?,
                actor: row.get(10)?,
                confirmed_by: row.get(11)?,
                reviewed_by: row.get(12)?,
                reviewed_at: row.get(13)?,
                plan_digest: row.get(14)?,
                created_at: row.get(15)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(RecordHistory {
        record_id,
        versions,
    })
}

pub(crate) fn plan_restore(
    database_path: &Path,
    proposal: RecordRestoreProposal,
) -> Result<RecordRestorePlan, RecordError> {
    let connection = storage::open_writable_connection(database_path)?;
    Ok(prepare_restore(&connection, proposal)?.plan)
}

pub(crate) fn apply_restore(
    database_path: &Path,
    input: ApplyRecordRestoreInput,
) -> Result<RecordDetail, RecordError> {
    let reviewer = require_review(&input.reviewer, input.review_confirmed)?;
    let mut connection = storage::open_writable_connection(database_path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let prepared = prepare_restore(&transaction, input.proposal)?;
    if prepared.plan.plan_digest != input.plan_digest {
        return Err(RecordError::PlanMismatch);
    }

    let timestamp = current_timestamp(&transaction)?;
    let next_version = prepared.plan.next_version;
    let (reviewed_by, reviewed_at) = if prepared.state.status == RecordStatus::Reviewed {
        (Some(reviewer.as_str()), Some(timestamp.as_str()))
    } else {
        (None, None)
    };

    insert_version(
        &transaction,
        &prepared.current.id,
        next_version,
        &prepared.state,
        "restore",
        Some(prepared.source_version),
        &prepared.actor,
        Some(&reviewer),
        reviewed_by,
        reviewed_at,
        Some(&prepared.plan.plan_digest),
        &timestamp,
    )?;
    update_current_record(
        &transaction,
        &prepared.current.id,
        prepared.current.current_version,
        next_version,
        &prepared.state,
        &prepared.actor,
        reviewed_by,
        reviewed_at,
        &timestamp,
    )?;
    transaction.commit()?;

    get_record(database_path, &prepared.current.id)
}

pub(crate) fn audit_summary_freshness(
    database_path: &Path,
) -> Result<SummaryFreshnessReport, RecordError> {
    let connection = storage::open_writable_connection(database_path)?;
    let records = load_all_current_records(&connection)?;
    let metadata_read_count = records.len();
    let mut body_read_count = 0;
    let mut integrity_issue_count = 0;
    let mut audits = Vec::with_capacity(records.len());

    for record in records {
        if record.summary.is_none() {
            audits.push(SummaryAuditEntry {
                record_id: record.id,
                current_version: record.current_version,
                state: SummaryState::MissingSummary,
                expected_body_sha256: record.body_sha256,
                actual_body_sha256: None,
                issue: None,
            });
            continue;
        }

        body_read_count += 1;
        let body_row = connection
            .query_row(
                "SELECT body, body_sha256, body_bytes
                 FROM momso_record_bodies
                 WHERE record_id = ?1 AND version = ?2",
                params![record.id, record.current_version],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;

        let (state, actual_body_sha256, issue) = match body_row {
            None => {
                integrity_issue_count += 1;
                (
                    SummaryState::BodyIntegrityError,
                    None,
                    Some("current body is missing".to_string()),
                )
            }
            Some((body, stored_body_sha256, stored_body_bytes)) => {
                let actual_hash = hash_text(&body);
                let actual_bytes = usize_to_i64(body.len(), "body size")?;
                if actual_hash != stored_body_sha256
                    || actual_hash != record.body_sha256
                    || actual_bytes != stored_body_bytes
                    || actual_bytes != record.body_bytes
                {
                    integrity_issue_count += 1;
                    (
                        SummaryState::BodyIntegrityError,
                        Some(actual_hash),
                        Some(
                            "stored body hash or byte count does not match the selected body"
                                .to_string(),
                        ),
                    )
                } else {
                    (
                        summary_state(
                            record.summary.as_deref(),
                            record.summary_basis_body_sha256.as_deref(),
                            &actual_hash,
                        ),
                        Some(actual_hash),
                        None,
                    )
                }
            }
        };

        audits.push(SummaryAuditEntry {
            record_id: record.id,
            current_version: record.current_version,
            state,
            expected_body_sha256: record.body_sha256,
            actual_body_sha256,
            issue,
        });
    }

    Ok(SummaryFreshnessReport {
        records: audits,
        metadata_read_count,
        body_read_count,
        integrity_issue_count,
    })
}

fn prepare_change(
    connection: &Connection,
    proposal: RecordChangeProposal,
) -> Result<PreparedChange, RecordError> {
    let record_id = validate_record_id(&proposal.record_id)?;
    validate_version("expectedVersion", proposal.expected_version)?;
    let current = load_current_record(connection, &record_id)?;
    ensure_expected_version(&current, proposal.expected_version)?;

    let record_kind = validate_record_kind(&proposal.record_kind)?;
    let title = validate_title(&proposal.title)?;
    validate_body(&proposal.body)?;
    let summary = normalize_summary(proposal.summary)?;
    let actor = validate_person("actor", &proposal.actor)?;
    if proposal.target_status == RecordStatus::Reviewed && summary.is_none() {
        return Err(RecordError::Validation(
            "a reviewed record must have a non-empty summary".to_string(),
        ));
    }

    let body_sha256 = hash_text(&proposal.body);
    let body_bytes = usize_to_i64(proposal.body.len(), "body size")?;
    let summary_sha256 = summary.as_deref().map(hash_text);
    let summary_basis_body_sha256 = if summary.is_none() {
        None
    } else if proposal.target_status == RecordStatus::Reviewed {
        Some(body_sha256.clone())
    } else if summary_sha256 == current.summary_sha256 {
        current.summary_basis_body_sha256.clone()
    } else {
        None
    };

    let state = ContentState {
        record_kind,
        title,
        status: proposal.target_status,
        summary,
        summary_sha256,
        summary_basis_body_sha256,
        body: proposal.body,
        body_sha256,
        body_bytes,
    };
    let changed_fields = changed_fields(&current, &state);
    if changed_fields.is_empty() {
        return Err(RecordError::NoChanges);
    }

    let next_version = current.current_version + 1;
    let plan_digest = change_plan_digest(&current.id, &state, &actor, current.current_version);
    let plan = RecordChangePlan {
        record_id: current.id.clone(),
        current_version: current.current_version,
        next_version,
        changed_fields,
        target_status: state.status,
        summary_state_after: summary_state(
            state.summary.as_deref(),
            state.summary_basis_body_sha256.as_deref(),
            &state.body_sha256,
        ),
        body_bytes_after: state.body_bytes,
        requires_confirmation: true,
        plan_digest,
    };

    Ok(PreparedChange {
        current,
        state,
        actor,
        plan,
    })
}

fn prepare_restore(
    connection: &Connection,
    proposal: RecordRestoreProposal,
) -> Result<PreparedRestore, RecordError> {
    let record_id = validate_record_id(&proposal.record_id)?;
    validate_version("expectedVersion", proposal.expected_version)?;
    validate_version("restoreVersion", proposal.restore_version)?;
    let actor = validate_person("actor", &proposal.actor)?;
    let current = load_current_record(connection, &record_id)?;
    ensure_expected_version(&current, proposal.expected_version)?;
    if proposal.restore_version == current.current_version {
        return Err(RecordError::NoChanges);
    }

    let state = load_historical_content(connection, &record_id, proposal.restore_version)?;
    let changed_fields = changed_fields(&current, &state);
    if changed_fields.is_empty() {
        return Err(RecordError::NoChanges);
    }

    let next_version = current.current_version + 1;
    let plan_digest = restore_plan_digest(
        &current.id,
        &state,
        &actor,
        current.current_version,
        proposal.restore_version,
    );
    let plan = RecordRestorePlan {
        record_id: current.id.clone(),
        current_version: current.current_version,
        next_version,
        restore_version: proposal.restore_version,
        changed_fields,
        target_status: state.status,
        summary_state_after: summary_state(
            state.summary.as_deref(),
            state.summary_basis_body_sha256.as_deref(),
            &state.body_sha256,
        ),
        body_bytes_after: state.body_bytes,
        requires_confirmation: true,
        plan_digest,
    };

    Ok(PreparedRestore {
        current,
        state,
        actor,
        source_version: proposal.restore_version,
        plan,
    })
}

fn load_current_record(
    connection: &Connection,
    record_id: &str,
) -> Result<CurrentRecord, RecordError> {
    connection
        .query_row(
            "SELECT id, record_kind, title, status, current_version,
                    summary, summary_sha256, summary_basis_body_sha256,
                    body_sha256, body_bytes,
                    created_by, updated_by, reviewed_by, reviewed_at,
                    created_at, updated_at
             FROM momso_records
             WHERE id = ?1",
            [record_id],
            CurrentRecord::from_row,
        )
        .optional()?
        .ok_or_else(|| RecordError::NotFound(record_id.to_string()))
}

fn load_all_current_records(connection: &Connection) -> Result<Vec<CurrentRecord>, RecordError> {
    let mut statement = connection.prepare(
        "SELECT id, record_kind, title, status, current_version,
                summary, summary_sha256, summary_basis_body_sha256,
                body_sha256, body_bytes,
                created_by, updated_by, reviewed_by, reviewed_at,
                created_at, updated_at
         FROM momso_records
         ORDER BY updated_at DESC, id ASC",
    )?;
    let records = statement
        .query_map([], CurrentRecord::from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(records)
}

fn load_verified_body(
    connection: &Connection,
    record_id: &str,
    version: i64,
    expected_hash: &str,
    expected_bytes: i64,
) -> Result<String, RecordError> {
    let body_row = connection
        .query_row(
            "SELECT body, body_sha256, body_bytes
             FROM momso_record_bodies
             WHERE record_id = ?1 AND version = ?2",
            params![record_id, version],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            RecordError::Integrity(format!("body is missing for {record_id} version {version}"))
        })?;
    let actual_hash = hash_text(&body_row.0);
    let actual_bytes = usize_to_i64(body_row.0.len(), "body size")?;
    if body_row.1 != expected_hash
        || actual_hash != expected_hash
        || body_row.2 != expected_bytes
        || actual_bytes != expected_bytes
    {
        return Err(RecordError::Integrity(format!(
            "body hash or byte count differs for {record_id} version {version}"
        )));
    }
    Ok(body_row.0)
}

fn load_historical_content(
    connection: &Connection,
    record_id: &str,
    version: i64,
) -> Result<ContentState, RecordError> {
    let row = connection
        .query_row(
            "SELECT v.record_kind, v.title, v.status,
                    v.summary, v.summary_sha256, v.summary_basis_body_sha256,
                    v.body_sha256, v.body_bytes,
                    b.body, b.body_sha256, b.body_bytes
             FROM momso_record_versions v
             JOIN momso_record_bodies b
               ON b.record_id = v.record_id AND b.version = v.version
             WHERE v.record_id = ?1 AND v.version = ?2",
            params![record_id, version],
            |row| {
                let status_value: String = row.get(2)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    status_from_database(2, &status_value)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, i64>(10)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| RecordError::NotFound(format!("{record_id} version {version}")))?;

    let actual_hash = hash_text(&row.8);
    let actual_bytes = usize_to_i64(row.8.len(), "body size")?;
    if actual_hash != row.6
        || actual_hash != row.9
        || actual_bytes != row.7
        || actual_bytes != row.10
    {
        return Err(RecordError::Integrity(format!(
            "historical body hash or byte count differs for {record_id} version {version}"
        )));
    }

    Ok(ContentState {
        record_kind: row.0,
        title: row.1,
        status: row.2,
        summary: row.3,
        summary_sha256: row.4,
        summary_basis_body_sha256: row.5,
        body: row.8,
        body_sha256: row.6,
        body_bytes: row.7,
    })
}

#[allow(clippy::too_many_arguments)]
fn insert_version(
    connection: &Connection,
    record_id: &str,
    version: i64,
    state: &ContentState,
    change_kind: &str,
    source_version: Option<i64>,
    actor: &str,
    confirmed_by: Option<&str>,
    reviewed_by: Option<&str>,
    reviewed_at: Option<&str>,
    plan_digest: Option<&str>,
    timestamp: &str,
) -> Result<(), RecordError> {
    connection.execute(
        "INSERT INTO momso_record_versions (
           record_id, version, record_kind, title, status,
           summary, summary_sha256, summary_basis_body_sha256,
           body_sha256, body_bytes, change_kind, source_version,
           actor, confirmed_by, reviewed_by, reviewed_at, plan_digest, created_at
         ) VALUES (
           ?1, ?2, ?3, ?4, ?5,
           ?6, ?7, ?8,
           ?9, ?10, ?11, ?12,
           ?13, ?14, ?15, ?16, ?17, ?18
         )",
        params![
            record_id,
            version,
            state.record_kind,
            state.title,
            state.status.as_db(),
            state.summary,
            state.summary_sha256,
            state.summary_basis_body_sha256,
            state.body_sha256,
            state.body_bytes,
            change_kind,
            source_version,
            actor,
            confirmed_by,
            reviewed_by,
            reviewed_at,
            plan_digest,
            timestamp,
        ],
    )?;
    connection.execute(
        "INSERT INTO momso_record_bodies (
           record_id, version, body, body_sha256, body_bytes
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            record_id,
            version,
            state.body,
            state.body_sha256,
            state.body_bytes,
        ],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn update_current_record(
    connection: &Connection,
    record_id: &str,
    expected_version: i64,
    next_version: i64,
    state: &ContentState,
    actor: &str,
    reviewed_by: Option<&str>,
    reviewed_at: Option<&str>,
    timestamp: &str,
) -> Result<(), RecordError> {
    let updated = connection.execute(
        "UPDATE momso_records
         SET record_kind = ?1,
             title = ?2,
             status = ?3,
             current_version = ?4,
             summary = ?5,
             summary_sha256 = ?6,
             summary_basis_body_sha256 = ?7,
             body_sha256 = ?8,
             body_bytes = ?9,
             updated_by = ?10,
             reviewed_by = ?11,
             reviewed_at = ?12,
             updated_at = ?13
         WHERE id = ?14 AND current_version = ?15",
        params![
            state.record_kind,
            state.title,
            state.status.as_db(),
            next_version,
            state.summary,
            state.summary_sha256,
            state.summary_basis_body_sha256,
            state.body_sha256,
            state.body_bytes,
            actor,
            reviewed_by,
            reviewed_at,
            timestamp,
            record_id,
            expected_version,
        ],
    )?;
    if updated != 1 {
        return Err(RecordError::VersionConflict {
            record_id: record_id.to_string(),
            expected: expected_version,
            found: expected_version + 1,
        });
    }
    Ok(())
}

fn detail_from_parts(record: CurrentRecord, body: String) -> RecordDetail {
    RecordDetail {
        id: record.id,
        record_kind: record.record_kind,
        title: record.title,
        status: record.status,
        current_version: record.current_version,
        summary_state: summary_state(
            record.summary.as_deref(),
            record.summary_basis_body_sha256.as_deref(),
            &record.body_sha256,
        ),
        summary: record.summary,
        body,
        body_sha256: record.body_sha256,
        body_bytes: record.body_bytes,
        created_by: record.created_by,
        updated_by: record.updated_by,
        reviewed_by: record.reviewed_by,
        reviewed_at: record.reviewed_at,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn changed_fields(current: &CurrentRecord, target: &ContentState) -> Vec<String> {
    let mut fields = Vec::new();
    if current.record_kind != target.record_kind {
        fields.push("record_kind".to_string());
    }
    if current.title != target.title {
        fields.push("title".to_string());
    }
    if current.summary_sha256 != target.summary_sha256 {
        fields.push("summary".to_string());
    }
    if current.body_sha256 != target.body_sha256 || current.body_bytes != target.body_bytes {
        fields.push("body".to_string());
    }
    if current.status != target.status
        || current.summary_basis_body_sha256 != target.summary_basis_body_sha256
    {
        fields.push("review_status".to_string());
    }
    fields
}

fn ensure_expected_version(
    current: &CurrentRecord,
    expected_version: i64,
) -> Result<(), RecordError> {
    if current.current_version != expected_version {
        return Err(RecordError::VersionConflict {
            record_id: current.id.clone(),
            expected: expected_version,
            found: current.current_version,
        });
    }
    Ok(())
}

fn summary_state(
    summary: Option<&str>,
    summary_basis_body_sha256: Option<&str>,
    body_sha256: &str,
) -> SummaryState {
    if summary.is_none() {
        SummaryState::MissingSummary
    } else if let Some(basis) = summary_basis_body_sha256 {
        if basis == body_sha256 {
            SummaryState::Fresh
        } else {
            SummaryState::Stale
        }
    } else {
        SummaryState::Unconfirmed
    }
}

fn change_plan_digest(
    record_id: &str,
    state: &ContentState,
    actor: &str,
    current_version: i64,
) -> String {
    let mut digest = DigestBuilder::new("momso.record-change.v1");
    digest.push(record_id);
    digest.push(&current_version.to_string());
    digest.push(&state.record_kind);
    digest.push(&state.title);
    digest.push(state.status.as_db());
    digest.push_optional(state.summary_sha256.as_deref());
    digest.push_optional(state.summary_basis_body_sha256.as_deref());
    digest.push(&state.body_sha256);
    digest.push(&state.body_bytes.to_string());
    digest.push(actor);
    digest.finish()
}

fn restore_plan_digest(
    record_id: &str,
    state: &ContentState,
    actor: &str,
    current_version: i64,
    restore_version: i64,
) -> String {
    let mut digest = DigestBuilder::new("momso.record-restore.v1");
    digest.push(record_id);
    digest.push(&current_version.to_string());
    digest.push(&restore_version.to_string());
    digest.push(&state.record_kind);
    digest.push(&state.title);
    digest.push(state.status.as_db());
    digest.push_optional(state.summary_sha256.as_deref());
    digest.push_optional(state.summary_basis_body_sha256.as_deref());
    digest.push(&state.body_sha256);
    digest.push(&state.body_bytes.to_string());
    digest.push(actor);
    digest.finish()
}

struct DigestBuilder(Sha256);

impl DigestBuilder {
    fn new(domain: &str) -> Self {
        let mut builder = Self(Sha256::new());
        builder.push(domain);
        builder
    }

    fn push(&mut self, value: &str) {
        self.0.update((value.len() as u64).to_be_bytes());
        self.0.update(value.as_bytes());
    }

    fn push_optional(&mut self, value: Option<&str>) {
        match value {
            Some(value) => {
                self.push("some");
                self.push(value);
            }
            None => self.push("none"),
        }
    }

    fn finish(self) -> String {
        format!("{:x}", self.0.finalize())
    }
}

fn require_review(reviewer: &str, confirmed: bool) -> Result<String, RecordError> {
    if !confirmed {
        return Err(RecordError::ReviewRequired);
    }
    validate_person("reviewer", reviewer)
}

fn validate_record_id(value: &str) -> Result<String, RecordError> {
    Uuid::parse_str(value.trim())
        .map(|id| id.to_string())
        .map_err(|_| RecordError::Validation("recordId must be a UUID".to_string()))
}

fn validate_record_kind(value: &str) -> Result<String, RecordError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_RECORD_KIND_CHARS {
        return Err(RecordError::Validation(format!(
            "recordKind must be 1 to {MAX_RECORD_KIND_CHARS} characters"
        )));
    }
    if !value.chars().all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '_'
            || character == '-'
    }) {
        return Err(RecordError::Validation(
            "recordKind may contain lowercase ASCII letters, numbers, underscores, and hyphens"
                .to_string(),
        ));
    }
    Ok(value.to_string())
}

fn validate_title(value: &str) -> Result<String, RecordError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_TITLE_CHARS {
        return Err(RecordError::Validation(format!(
            "title must be 1 to {MAX_TITLE_CHARS} characters"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(RecordError::Validation(
            "title may not contain control characters".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn normalize_summary(value: Option<String>) -> Result<Option<String>, RecordError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Ok(None);
    }
    if value.chars().count() > MAX_SUMMARY_CHARS {
        return Err(RecordError::Validation(format!(
            "summary may contain at most {MAX_SUMMARY_CHARS} characters"
        )));
    }
    Ok(Some(value))
}

fn validate_body(value: &str) -> Result<(), RecordError> {
    if value.len() > MAX_BODY_BYTES {
        return Err(RecordError::Validation(format!(
            "body may contain at most {MAX_BODY_BYTES} UTF-8 bytes"
        )));
    }
    Ok(())
}

fn validate_person(field: &str, value: &str) -> Result<String, RecordError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_ACTOR_CHARS {
        return Err(RecordError::Validation(format!(
            "{field} must be 1 to {MAX_ACTOR_CHARS} characters"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(RecordError::Validation(format!(
            "{field} may not contain control characters"
        )));
    }
    Ok(value.to_string())
}

fn validate_version(field: &str, value: i64) -> Result<(), RecordError> {
    if value < 1 {
        return Err(RecordError::Validation(format!(
            "{field} must be at least 1"
        )));
    }
    Ok(())
}

fn current_timestamp(connection: &Connection) -> Result<String, RecordError> {
    Ok(
        connection.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')", [], |row| {
            row.get(0)
        })?,
    )
}

fn hash_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn usize_to_i64(value: usize, label: &str) -> Result<i64, RecordError> {
    value
        .try_into()
        .map_err(|_| RecordError::Validation(format!("{label} is too large")))
}

fn status_from_database(column: usize, value: &str) -> rusqlite::Result<RecordStatus> {
    RecordStatus::from_db(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            Type::Text,
            Box::new(io::Error::new(
                io::ErrorKind::InvalidData,
                error.to_string(),
            )),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    struct TestStore {
        _temp: tempfile::TempDir,
        database_path: std::path::PathBuf,
    }

    impl TestStore {
        fn new() -> Self {
            let temp = tempdir().expect("tempdir");
            let database_path = temp.path().join("momso.db");
            let backups_path = temp.path().join("backups");
            storage::initialize(&database_path, &backups_path, "0.1.0-test")
                .expect("storage initializes");
            Self {
                _temp: temp,
                database_path,
            }
        }

        fn create(&self, title: &str, body: &str, summary: Option<&str>) -> RecordDetail {
            create_draft(
                &self.database_path,
                CreateRecordDraftInput {
                    record_kind: "session_note".to_string(),
                    title: title.to_string(),
                    body: body.to_string(),
                    summary: summary.map(str::to_string),
                    actor: "AI draft".to_string(),
                    initial_status: None,
                },
            )
            .expect("draft is created")
        }
    }

    fn change_proposal(
        record: &RecordDetail,
        title: &str,
        body: &str,
        summary: Option<&str>,
        target_status: RecordStatus,
    ) -> RecordChangeProposal {
        RecordChangeProposal {
            record_id: record.id.clone(),
            expected_version: record.current_version,
            record_kind: record.record_kind.clone(),
            title: title.to_string(),
            body: body.to_string(),
            summary: summary.map(str::to_string),
            target_status,
            actor: "AI draft".to_string(),
        }
    }

    #[test]
    fn imported_record_can_begin_needs_review_but_never_reviewed() {
        let store = TestStore::new();
        let needs_review = create_draft(
            &store.database_path,
            CreateRecordDraftInput {
                record_kind: "session_note".to_string(),
                title: "Imported note".to_string(),
                body: "Exact imported body".to_string(),
                summary: Some("Human must still review this".to_string()),
                actor: "momso-desktop".to_string(),
                initial_status: Some(RecordStatus::NeedsReview),
            },
        )
        .expect("an imported record can enter the review queue");
        assert_eq!(needs_review.status, RecordStatus::NeedsReview);

        let reviewed = create_draft(
            &store.database_path,
            CreateRecordDraftInput {
                record_kind: "session_note".to_string(),
                title: "Invalid imported note".to_string(),
                body: "Body".to_string(),
                summary: Some("Summary".to_string()),
                actor: "momso-desktop".to_string(),
                initial_status: Some(RecordStatus::Reviewed),
            },
        );
        assert!(matches!(reviewed, Err(RecordError::Validation(_))));
    }

    #[test]
    fn summary_listing_never_requires_body_rows() {
        let store = TestStore::new();
        let record = store.create("첫 기록", "긴 본문", Some("짧은 요약"));
        let connection =
            storage::open_writable_connection(&store.database_path).expect("database opens");
        connection
            .execute(
                "DELETE FROM momso_record_bodies WHERE record_id = ?1",
                [&record.id],
            )
            .expect("body row is removed for the boundary test");

        let report = list_summaries(&store.database_path).expect("metadata list succeeds");
        assert_eq!(report.metadata_read_count, 1);
        assert_eq!(report.body_read_count, 0);
        assert_eq!(report.records[0].summary.as_deref(), Some("짧은 요약"));
    }

    #[test]
    fn freshness_audit_reads_bodies_only_for_explicit_summaries() {
        let store = TestStore::new();
        store.create("요약 있음", "본문 A", Some("요약 A"));
        store.create("요약 없음 1", "본문 B", None);
        store.create("요약 없음 2", "본문 C", None);

        let report = audit_summary_freshness(&store.database_path).expect("audit succeeds");
        assert_eq!(report.metadata_read_count, 3);
        assert_eq!(report.body_read_count, 1);
        assert_eq!(report.integrity_issue_count, 0);
        assert_eq!(
            report
                .records
                .iter()
                .filter(|record| record.state == SummaryState::MissingSummary)
                .count(),
            2
        );
    }

    #[test]
    fn reviewed_change_requires_explicit_confirmation() {
        let store = TestStore::new();
        let record = store.create("수업 기록", "원문", None);
        let proposal = change_proposal(
            &record,
            "수업 기록",
            "검수한 원문",
            Some("검수한 요약"),
            RecordStatus::Reviewed,
        );
        let plan = plan_change(&store.database_path, proposal.clone()).expect("plan is created");
        assert_eq!(plan.summary_state_after, SummaryState::Fresh);

        let error = apply_change(
            &store.database_path,
            ApplyRecordChangeInput {
                proposal: proposal.clone(),
                plan_digest: plan.plan_digest.clone(),
                reviewer: "김성균".to_string(),
                review_confirmed: false,
            },
        )
        .expect_err("unconfirmed change is rejected");
        assert!(matches!(error, RecordError::ReviewRequired));

        let updated = apply_change(
            &store.database_path,
            ApplyRecordChangeInput {
                proposal,
                plan_digest: plan.plan_digest,
                reviewer: "김성균".to_string(),
                review_confirmed: true,
            },
        )
        .expect("confirmed change is applied");
        assert_eq!(updated.current_version, 2);
        assert_eq!(updated.status, RecordStatus::Reviewed);
        assert_eq!(updated.summary_state, SummaryState::Fresh);
        assert_eq!(updated.reviewed_by.as_deref(), Some("김성균"));
    }

    #[test]
    fn database_rejects_reviewed_state_without_a_confirmed_summary_basis() {
        let store = TestStore::new();
        let record = store.create("수업 기록", "원문", None);
        let connection =
            storage::open_writable_connection(&store.database_path).expect("database opens");
        let result = connection.execute(
            "UPDATE momso_records
             SET status = 'reviewed',
                 summary = '요약',
                 summary_sha256 = 'not-important-for-this-constraint',
                 reviewed_by = '김성균',
                 reviewed_at = '2026-07-15T00:00:00.000Z'
             WHERE id = ?1",
            [&record.id],
        );

        assert!(result.is_err());
    }

    #[test]
    fn stale_change_plan_cannot_overwrite_a_newer_version() {
        let store = TestStore::new();
        let record = store.create("기록", "v1", None);
        let stale_proposal = change_proposal(
            &record,
            "기록",
            "stale body",
            None,
            RecordStatus::NeedsReview,
        );
        let stale_plan =
            plan_change(&store.database_path, stale_proposal.clone()).expect("stale plan");

        let winning_proposal =
            change_proposal(&record, "기록", "v2", None, RecordStatus::NeedsReview);
        let winning_plan =
            plan_change(&store.database_path, winning_proposal.clone()).expect("winning plan");
        apply_change(
            &store.database_path,
            ApplyRecordChangeInput {
                proposal: winning_proposal,
                plan_digest: winning_plan.plan_digest,
                reviewer: "김성균".to_string(),
                review_confirmed: true,
            },
        )
        .expect("winning plan applies");

        let error = apply_change(
            &store.database_path,
            ApplyRecordChangeInput {
                proposal: stale_proposal,
                plan_digest: stale_plan.plan_digest,
                reviewer: "김성균".to_string(),
                review_confirmed: true,
            },
        )
        .expect_err("stale plan is rejected");
        assert!(matches!(error, RecordError::VersionConflict { .. }));
    }

    #[test]
    fn restore_appends_an_exact_new_version_without_erasing_history() {
        let store = TestStore::new();
        let v1 = store.create("기록 v1", "본문 v1", Some("요약 v1"));
        let update = change_proposal(
            &v1,
            "기록 v2",
            "본문 v2",
            Some("요약 v2"),
            RecordStatus::NeedsReview,
        );
        let update_plan = plan_change(&store.database_path, update.clone()).expect("update plan");
        let v2 = apply_change(
            &store.database_path,
            ApplyRecordChangeInput {
                proposal: update,
                plan_digest: update_plan.plan_digest,
                reviewer: "김성균".to_string(),
                review_confirmed: true,
            },
        )
        .expect("update applies");

        let restore_proposal = RecordRestoreProposal {
            record_id: v2.id.clone(),
            expected_version: v2.current_version,
            restore_version: 1,
            actor: "김성균".to_string(),
        };
        let restore_plan =
            plan_restore(&store.database_path, restore_proposal.clone()).expect("restore plan");
        let restored = apply_restore(
            &store.database_path,
            ApplyRecordRestoreInput {
                proposal: restore_proposal,
                plan_digest: restore_plan.plan_digest,
                reviewer: "김성균".to_string(),
                review_confirmed: true,
            },
        )
        .expect("restore applies");

        assert_eq!(restored.current_version, 3);
        assert_eq!(restored.title, "기록 v1");
        assert_eq!(restored.body, "본문 v1");
        assert_eq!(restored.summary.as_deref(), Some("요약 v1"));
        let history = list_history(&store.database_path, &restored.id).expect("history loads");
        assert_eq!(history.versions.len(), 3);
        assert_eq!(history.versions[0].change_kind, "restore");
        assert_eq!(history.versions[0].source_version, Some(1));
    }

    #[test]
    fn selected_body_is_rehashed_before_freshness_is_trusted() {
        let store = TestStore::new();
        let record = store.create("기록", "정상 본문", Some("요약"));
        let connection =
            storage::open_writable_connection(&store.database_path).expect("database opens");
        connection
            .execute(
                "UPDATE momso_record_bodies SET body = '변조된 본문'
                 WHERE record_id = ?1 AND version = 1",
                [&record.id],
            )
            .expect("body is tampered for the integrity test");

        let report = audit_summary_freshness(&store.database_path).expect("audit completes");
        assert_eq!(report.body_read_count, 1);
        assert_eq!(report.integrity_issue_count, 1);
        assert_eq!(report.records[0].state, SummaryState::BodyIntegrityError);
    }

    #[test]
    fn body_continuation_requires_the_hash_from_the_first_page() {
        let store = TestStore::new();
        let record = store.create("긴 기록", "가나다라마바사아자차카타파하", None);
        let first =
            read_body_page(&store.database_path, &record.id, None, 0, None, 8).expect("first page");
        assert_eq!(first.content, "가나");

        let cursor = first.next_cursor.expect("continuation cursor");
        assert!(matches!(
            read_body_page(&store.database_path, &record.id, None, cursor, None, 8),
            Err(RecordError::ContinuationHashRequired)
        ));
        let second = read_body_page(
            &store.database_path,
            &record.id,
            None,
            cursor,
            Some(&first.body_sha256),
            8,
        )
        .expect("second page");
        assert_eq!(second.content, "다라");
    }

    #[test]
    fn paged_read_rejects_a_version_ledger_hash_that_differs_from_the_body_row() {
        let store = TestStore::new();
        let record = store.create("기록", "원본 본문", None);
        let connection =
            storage::open_writable_connection(&store.database_path).expect("database opens");
        connection
            .execute(
                "UPDATE momso_record_versions
                 SET body_sha256 = ?2
                 WHERE record_id = ?1 AND version = 1",
                params![record.id, "0".repeat(64)],
            )
            .expect("version ledger is altered for the integrity test");
        drop(connection);

        assert!(matches!(
            read_body_page(&store.database_path, &record.id, Some(1), 0, None, 1024),
            Err(RecordError::Integrity(_))
        ));
    }
}
