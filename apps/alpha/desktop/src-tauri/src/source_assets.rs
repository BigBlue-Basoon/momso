use crate::storage::{self, StorageError};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use thiserror::Error;
use uuid::Uuid;

const MAX_ACTOR_CHARS: usize = 120;
const MAX_FILE_NAME_CHARS: usize = 255;
const MAX_MEDIA_TYPE_CHARS: usize = 120;

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

#[derive(Debug)]
struct PreparedImport {
    canonical_source: PathBuf,
    media_type: Option<String>,
    actor: String,
    storage_relative_path: String,
    plan: SourceAssetImportPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoredPathInspection {
    Ready,
    Missing,
    Unsafe,
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
    materialize_source_file_with_hook(objects_path, prepared, |_| Ok(()))
}

fn materialize_source_file_with_hook(
    objects_path: &Path,
    prepared: &PreparedImport,
    before_publish: impl FnOnce(&Path) -> Result<(), SourceAssetError>,
) -> Result<(), SourceAssetError> {
    let destination = resolve_storage_path(objects_path, &prepared.storage_relative_path)?;
    match inspect_stored_file(objects_path, &destination)? {
        StoredPathInspection::Ready => {
            let (sha256, byte_length) = hash_file(&destination)?;
            if sha256 == prepared.plan.content_sha256 && byte_length == prepared.plan.byte_length {
                let mut permissions = fs::metadata(&destination)?.permissions();
                permissions.set_readonly(true);
                fs::set_permissions(&destination, permissions)?;
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
        before_publish(&destination)?;
        match tempfile::TempPath::try_from_path(&temporary)?.persist_noclobber(&destination) {
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
                StoredPathInspection::Missing => Err(SourceAssetError::Io(error.error)),
            },
        }
    })();
    if finalize_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    finalize_result?;
    let mut permissions = fs::metadata(&destination)?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&destination, permissions)?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records;
    use tempfile::TempDir;

    struct TestStore {
        _temp: TempDir,
        database_path: PathBuf,
        objects_path: PathBuf,
        source_path: PathBuf,
    }

    #[cfg(windows)]
    impl Drop for TestStore {
        fn drop(&mut self) {
            for entry in walkdir::WalkDir::new(&self.objects_path)
                .into_iter()
                .flatten()
            {
                if entry.file_type().is_file() {
                    if let Ok(metadata) = entry.metadata() {
                        let mut permissions = metadata.permissions();
                        permissions.set_readonly(false);
                        let _ = fs::set_permissions(entry.path(), permissions);
                    }
                }
            }
        }
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
            fs::read(&stored).expect("stored bytes"),
            "바순은 원본 표현입니다.\n".as_bytes()
        );
        assert!(fs::metadata(stored).unwrap().permissions().readonly());
        let connection = storage::open_writable_connection(&store.database_path).unwrap();
        let count: i64 = connection
            .query_row("SELECT count(*) FROM momso_source_assets", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn fake_cli_cannot_write_readonly_import_and_a_bypass_is_detected() {
        let store = TestStore::new("original source");
        let asset = store.import();
        let stored = resolve_storage_path(
            &store.objects_path,
            &relative_storage_path(&asset.content_sha256),
        )
        .unwrap();
        let root = store.objects_path.parent().unwrap();
        let before = crate::chat_file_changes::capture(root).unwrap();
        let status = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "try { [IO.File]::WriteAllText($env:MOMSO_FAKE_RAW, 'changed'); exit 0 } catch { exit 7 }"])
            .env("MOMSO_FAKE_RAW", &stored)
            .status().unwrap();
        assert!(!status.success());
        assert_eq!(fs::read(&stored).unwrap(), b"original source");
        let mut permissions = fs::metadata(&stored).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&stored, permissions).unwrap();
        fs::write(&stored, b"bypass").unwrap();
        let after = crate::chat_file_changes::capture(root).unwrap();
        assert_eq!(
            crate::chat_file_changes::source_changes(&before, &after)
                .get(&format!("sha256:{}", asset.content_sha256))
                .map(String::as_str),
            Some("edited")
        );
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
        let connection = storage::open_writable_connection(&store.database_path).unwrap();
        let count: i64 = connection
            .query_row("SELECT count(*) FROM momso_source_assets", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn duplicate_bytes_reuse_one_stored_asset() {
        let store = TestStore::new("same bytes");
        let first = store.import();
        let second = store.import();

        assert_eq!(first.id, second.id);
        let connection = storage::open_writable_connection(&store.database_path).unwrap();
        let count: i64 = connection
            .query_row("SELECT count(*) FROM momso_source_assets", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn same_filename_with_different_content_preserves_both_originals() {
        let store = TestStore::new("first contents");
        let first = store.import();
        fs::write(&store.source_path, "second contents").unwrap();
        let second = store.import();
        assert_ne!(first.id, second.id);
        let first_path = resolve_storage_path(
            &store.objects_path,
            &relative_storage_path(&first.content_sha256),
        )
        .unwrap();
        let second_path = resolve_storage_path(
            &store.objects_path,
            &relative_storage_path(&second.content_sha256),
        )
        .unwrap();
        assert_eq!(fs::read(first_path).unwrap(), b"first contents");
        assert_eq!(fs::read(second_path).unwrap(), b"second contents");
    }

    #[test]
    fn concurrent_destination_conflict_never_overwrites_existing_bytes() {
        let store = TestStore::new("new source");
        let prepared =
            prepare_import(&store.database_path, &store.objects_path, store.proposal()).unwrap();
        let mut collision = None;
        let result =
            materialize_source_file_with_hook(&store.objects_path, &prepared, |destination| {
                fs::write(destination, b"other existing bytes")?;
                collision = Some(destination.to_path_buf());
                Ok(())
            });
        assert!(matches!(result, Err(SourceAssetError::Integrity(_))));
        assert_eq!(
            fs::read(collision.unwrap()).unwrap(),
            b"other existing bytes"
        );
        assert_eq!(fs::read(&store.source_path).unwrap(), b"new source");
    }

    #[test]
    fn audit_reproduces_previous_rename_overwriting_a_file() {
        let temp = tempfile::tempdir().unwrap();
        let old_destination = temp.path().join("destination");
        let incoming = temp.path().join("incoming");
        fs::write(&old_destination, b"existing bytes").unwrap();
        fs::write(&incoming, b"new bytes").unwrap();
        fs::rename(&incoming, &old_destination).unwrap();
        assert_eq!(fs::read(old_destination).unwrap(), b"new bytes");
    }

    #[test]
    fn concurrent_identical_destination_is_reused() {
        let store = TestStore::new("same source");
        let prepared =
            prepare_import(&store.database_path, &store.objects_path, store.proposal()).unwrap();
        materialize_source_file_with_hook(&store.objects_path, &prepared, |destination| {
            fs::write(destination, b"same source")?;
            Ok(())
        })
        .unwrap();
        let staging = store.objects_path.parent().unwrap().join(".source-staging");
        assert_eq!(fs::read_dir(staging).unwrap().count(), 0);
    }

    #[test]
    fn source_file_can_be_bound_to_an_exact_record_version() {
        let store = TestStore::new("raw transcript");
        records::fixture_record(&store.database_path, "Morning class", "review copy", None)
            .unwrap();
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
