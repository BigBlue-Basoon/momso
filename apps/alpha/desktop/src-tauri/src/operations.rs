use crate::storage::{self, StorageError};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;
use thiserror::Error;
use uuid::Uuid;

const MAX_KIND_CHARS: usize = 80;
const MAX_STAGE_CHARS: usize = 80;
const MAX_ERROR_CODE_CHARS: usize = 80;

#[derive(Debug, Error)]
pub(crate) enum OperationError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Operation result serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Invalid operation state: {0}")]
    Validation(String),
    #[error("Operation {0} was not found")]
    NotFound(String),
    #[error("Operation {0} is no longer running")]
    NotRunning(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationState {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

impl OperationState {
    fn as_db(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    fn from_db(value: &str) -> Result<Self, OperationError> {
        match value {
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err(OperationError::Validation(format!(
                "unknown operation state {value:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OperationStatus {
    pub(crate) id: String,
    operation_kind: String,
    state: OperationState,
    stage: String,
    completed_units: u64,
    total_units: u64,
    result: Option<Value>,
    result_sha256: Option<String>,
    error_code: Option<String>,
    started_at: String,
    heartbeat_at: String,
    completed_at: Option<String>,
}

pub(crate) fn start(
    database_path: &Path,
    operation_kind: &str,
    stage: &str,
    total_units: u64,
) -> Result<OperationStatus, OperationError> {
    let operation_kind = validate_token("operation kind", operation_kind, MAX_KIND_CHARS)?;
    let stage = validate_token("operation stage", stage, MAX_STAGE_CHARS)?;
    let total_units = u64_to_i64(total_units, "total units")?;
    let operation_id = Uuid::new_v4().to_string();
    let mut connection = storage::open_writable_connection(database_path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let timestamp = current_timestamp(&transaction)?;
    transaction.execute(
        "INSERT INTO momso_operation_runs (
           id, operation_kind, state, stage, completed_units, total_units,
           started_at, heartbeat_at
         ) VALUES (?1, ?2, 'running', ?3, 0, ?4, ?5, ?5)",
        params![operation_id, operation_kind, stage, total_units, timestamp],
    )?;
    transaction.commit()?;
    get(database_path, &operation_id)
}

pub(crate) fn progress(
    database_path: &Path,
    operation_id: &str,
    stage: &str,
    completed_units: u64,
    total_units: u64,
) -> Result<(), OperationError> {
    if total_units > 0 && completed_units > total_units {
        return Err(OperationError::Validation(
            "completed units exceed total units".to_owned(),
        ));
    }
    let stage = validate_token("operation stage", stage, MAX_STAGE_CHARS)?;
    let completed_units = u64_to_i64(completed_units, "completed units")?;
    let total_units = u64_to_i64(total_units, "total units")?;
    let connection = storage::open_writable_connection(database_path)?;
    let changed = connection.execute(
        "UPDATE momso_operation_runs
         SET stage = ?2,
             completed_units = ?3,
             total_units = ?4,
             heartbeat_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?1 AND state = 'running'",
        params![operation_id, stage, completed_units, total_units],
    )?;
    ensure_running_update(database_path, operation_id, changed)
}

pub(crate) fn succeed<T: Serialize>(
    database_path: &Path,
    operation_id: &str,
    stage: &str,
    completed_units: u64,
    total_units: u64,
    result: &T,
) -> Result<OperationStatus, OperationError> {
    if total_units > 0 && completed_units > total_units {
        return Err(OperationError::Validation(
            "completed units exceed total units".to_owned(),
        ));
    }
    let stage = validate_token("operation stage", stage, MAX_STAGE_CHARS)?;
    let result_json = serde_json::to_string(result)?;
    let result_sha256 = hash_text(&result_json);
    let connection = storage::open_writable_connection(database_path)?;
    let changed = connection.execute(
        "UPDATE momso_operation_runs
         SET state = 'succeeded',
             stage = ?2,
             completed_units = ?3,
             total_units = ?4,
             result_json = ?5,
             result_sha256 = ?6,
             error_code = NULL,
             heartbeat_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
             completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?1 AND state = 'running'",
        params![
            operation_id,
            stage,
            u64_to_i64(completed_units, "completed units")?,
            u64_to_i64(total_units, "total units")?,
            result_json,
            result_sha256,
        ],
    )?;
    ensure_running_update(database_path, operation_id, changed)?;
    get(database_path, operation_id)
}

pub(crate) fn finish_without_result(
    database_path: &Path,
    operation_id: &str,
    state: OperationState,
    stage: &str,
    error_code: &str,
) -> Result<OperationStatus, OperationError> {
    if !matches!(
        state,
        OperationState::Failed | OperationState::Cancelled | OperationState::Interrupted
    ) {
        return Err(OperationError::Validation(
            "terminal operation state requires failure, cancellation, or interruption".to_owned(),
        ));
    }
    let stage = validate_token("operation stage", stage, MAX_STAGE_CHARS)?;
    let error_code = validate_token("error code", error_code, MAX_ERROR_CODE_CHARS)?;
    let connection = storage::open_writable_connection(database_path)?;
    let changed = connection.execute(
        "UPDATE momso_operation_runs
         SET state = ?2,
             stage = ?3,
             error_code = ?4,
             heartbeat_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
             completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?1 AND state = 'running'",
        params![operation_id, state.as_db(), stage, error_code],
    )?;
    ensure_running_update(database_path, operation_id, changed)?;
    get(database_path, operation_id)
}

pub(crate) fn recover_interrupted(database_path: &Path) -> Result<usize, OperationError> {
    let connection = storage::open_writable_connection(database_path)?;
    let changed = connection.execute(
        "UPDATE momso_operation_runs
         SET state = 'interrupted',
             stage = 'interrupted',
             error_code = 'app_restarted',
             heartbeat_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
             completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE state = 'running'",
        [],
    )?;
    Ok(changed)
}

pub(crate) fn get(
    database_path: &Path,
    operation_id: &str,
) -> Result<OperationStatus, OperationError> {
    let connection = storage::open_writable_connection(database_path)?;
    connection
        .query_row(
            "SELECT id, operation_kind, state, stage, completed_units, total_units,
                    result_json, result_sha256, error_code,
                    started_at, heartbeat_at, completed_at
             FROM momso_operation_runs WHERE id = ?1",
            [operation_id],
            |row| {
                let state_value: String = row.get(2)?;
                let completed_units: i64 = row.get(4)?;
                let total_units: i64 = row.get(5)?;
                let result_json: Option<String> = row.get(6)?;
                let result = result_json
                    .map(|value| serde_json::from_str(&value))
                    .transpose()
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            6,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                Ok(OperationStatus {
                    id: row.get(0)?,
                    operation_kind: row.get(1)?,
                    state: OperationState::from_db(&state_value).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                    stage: row.get(3)?,
                    completed_units: u64::try_from(completed_units).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    total_units: u64::try_from(total_units).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            5,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    result,
                    result_sha256: row.get(7)?,
                    error_code: row.get(8)?,
                    started_at: row.get(9)?,
                    heartbeat_at: row.get(10)?,
                    completed_at: row.get(11)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| OperationError::NotFound(operation_id.to_owned()))
}

pub(crate) fn copy_running_to_database(
    source_database_path: &Path,
    destination_database_path: &Path,
    operation_id: &str,
) -> Result<(), OperationError> {
    let operation = get(source_database_path, operation_id)?;
    if operation.state != OperationState::Running {
        return Err(OperationError::NotRunning(operation_id.to_owned()));
    }
    let connection = storage::open_writable_connection(destination_database_path)?;
    connection.execute(
        "INSERT INTO momso_operation_runs (
           id, operation_kind, state, stage, completed_units, total_units,
           started_at, heartbeat_at
         ) VALUES (?1, ?2, 'running', ?3, ?4, ?5, ?6, ?7)",
        params![
            operation.id,
            operation.operation_kind,
            operation.stage,
            u64_to_i64(operation.completed_units, "completed units")?,
            u64_to_i64(operation.total_units, "total units")?,
            operation.started_at,
            operation.heartbeat_at,
        ],
    )?;
    Ok(())
}

fn ensure_running_update(
    database_path: &Path,
    operation_id: &str,
    changed: usize,
) -> Result<(), OperationError> {
    if changed == 1 {
        return Ok(());
    }
    match get(database_path, operation_id) {
        Ok(_) => Err(OperationError::NotRunning(operation_id.to_owned())),
        Err(OperationError::NotFound(_)) => Err(OperationError::NotFound(operation_id.to_owned())),
        Err(error) => Err(error),
    }
}

fn current_timestamp(connection: &rusqlite::Connection) -> Result<String, rusqlite::Error> {
    connection.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')", [], |row| {
        row.get(0)
    })
}

fn validate_token(field: &str, value: &str, max_chars: usize) -> Result<String, OperationError> {
    let normalized = value.trim();
    let count = normalized.chars().count();
    if count == 0 || count > max_chars {
        return Err(OperationError::Validation(format!(
            "{field} must contain between 1 and {max_chars} characters"
        )));
    }
    if !normalized
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err(OperationError::Validation(format!(
            "{field} contains unsupported characters"
        )));
    }
    Ok(normalized.to_owned())
}

fn u64_to_i64(value: u64, field: &str) -> Result<i64, OperationError> {
    i64::try_from(value).map_err(|_| OperationError::Validation(format!("{field} is too large")))
}

fn hash_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn database() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempdir().expect("temp dir");
        let database_path = temp.path().join("momso.db");
        storage::initialize(&database_path, &temp.path().join("backups"), "test")
            .expect("storage initializes");
        (temp, database_path)
    }

    #[test]
    fn progress_and_result_survive_the_call_that_created_them() {
        let (_temp, database_path) = database();
        let started = start(&database_path, "source_asset_integrity", "preparing", 0)
            .expect("operation starts");
        progress(&database_path, &started.id, "checking", 2, 4).expect("progress persists");
        let completed = succeed(
            &database_path,
            &started.id,
            "complete",
            4,
            4,
            &serde_json::json!({"healthy": 4}),
        )
        .expect("operation completes");
        let reloaded = get(&database_path, &started.id).expect("operation reloads");

        assert_eq!(completed.state, OperationState::Succeeded);
        assert_eq!(reloaded.completed_units, 4);
        assert_eq!(reloaded.result, Some(serde_json::json!({"healthy": 4})));
        assert_eq!(reloaded.result_sha256.as_deref().map(str::len), Some(64));
    }

    #[test]
    fn startup_marks_abandoned_running_work_as_interrupted() {
        let (_temp, database_path) = database();
        let started = start(&database_path, "source_asset_integrity", "checking", 10)
            .expect("operation starts");

        assert_eq!(recover_interrupted(&database_path).expect("recovery"), 1);
        let recovered = get(&database_path, &started.id).expect("operation reloads");
        assert_eq!(recovered.state, OperationState::Interrupted);
        assert_eq!(recovered.error_code.as_deref(), Some("app_restarted"));
    }

    #[test]
    fn running_operation_can_follow_a_replaced_database() {
        let (source_temp, source_database_path) = database();
        let (_destination_temp, destination_database_path) = database();
        let started = start(
            &source_database_path,
            "user_owned_restore",
            "downloading",
            5,
        )
        .expect("operation starts");
        progress(
            &source_database_path,
            &started.id,
            "replacing_local_copy",
            4,
            5,
        )
        .expect("progress persists");

        copy_running_to_database(
            &source_database_path,
            &destination_database_path,
            &started.id,
        )
        .expect("operation copies");
        let copied = get(&destination_database_path, &started.id).expect("copied operation reads");

        assert_eq!(copied.state, OperationState::Running);
        assert_eq!(copied.stage, "replacing_local_copy");
        assert_eq!(copied.completed_units, 4);
        assert_eq!(copied.total_units, 5);

        storage::restore_snapshot_file(
            &destination_database_path,
            &source_database_path,
            &source_temp.path().join("backups"),
        )
        .expect("database replacement succeeds");
        let restored = get(&source_database_path, &started.id).expect("operation survives restore");
        assert_eq!(restored.state, OperationState::Running);
        assert_eq!(restored.stage, "replacing_local_copy");
    }
}
