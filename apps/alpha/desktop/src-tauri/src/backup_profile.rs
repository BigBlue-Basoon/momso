use crate::storage;
use rusqlite::OptionalExtension;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read},
    path::Path,
};
use thiserror::Error;
use uuid::Uuid;
use walkdir::WalkDir;

#[derive(Debug, Error)]
pub(crate) enum BackupProfileError {
    #[error(transparent)]
    Storage(#[from] storage::StorageError),
    #[error("백업 설정을 읽거나 저장하지 못했습니다")]
    Sqlite(#[from] rusqlite::Error),
    #[error("기록 파일을 읽지 못했습니다")]
    Io(#[from] io::Error),
    #[error("기록 파일 경로가 작업공간을 벗어났습니다")]
    UnsafeObjectPath,
    #[error("원본 파일 원장과 저장된 바이트가 일치하지 않습니다")]
    SourceAssetIntegrity,
    #[error("백업 스냅샷 상태가 예상과 달라 안전하게 계속할 수 없습니다")]
    SnapshotStateChanged,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupProfile {
    pub(crate) workspace_id: String,
    pub(crate) github_repository: Option<String>,
    pub(crate) r2_bucket: Option<String>,
    pub(crate) last_completed_snapshot_id: Option<String>,
    pub(crate) last_completed_content_sha256: Option<String>,
    pub(crate) recovery_key_confirmed_at: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct LocalContentState {
    pub(crate) sha256: String,
    pub(crate) record_count: u64,
    pub(crate) object_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ObjectFileState {
    pub(crate) relative_path: String,
    pub(crate) sha256: String,
    pub(crate) byte_length: u64,
}

fn read_profile(row: &rusqlite::Row<'_>) -> rusqlite::Result<BackupProfile> {
    Ok(BackupProfile {
        workspace_id: row.get(0)?,
        github_repository: row.get(1)?,
        r2_bucket: row.get(2)?,
        last_completed_snapshot_id: row.get(3)?,
        last_completed_content_sha256: row.get(4)?,
        recovery_key_confirmed_at: row.get(5)?,
    })
}

pub(crate) fn ensure(database_path: &Path) -> Result<BackupProfile, BackupProfileError> {
    let connection = storage::open_writable_connection(database_path)?;
    let existing = connection
        .query_row(
            "SELECT workspace_id, github_repository, r2_bucket,
                    last_completed_snapshot_id, last_completed_content_sha256,
                    recovery_key_confirmed_at
             FROM momso_backup_profiles
             ORDER BY created_at ASC LIMIT 1",
            [],
            read_profile,
        )
        .optional()?;
    if let Some(profile) = existing {
        return Ok(profile);
    }

    let workspace_id = Uuid::new_v4().to_string();
    connection.execute(
        "INSERT INTO momso_backup_profiles (
           workspace_id, created_at, updated_at
         ) VALUES (
           ?1,
           strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
           strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         )",
        [&workspace_id],
    )?;
    Ok(BackupProfile {
        workspace_id,
        github_repository: None,
        r2_bucket: None,
        last_completed_snapshot_id: None,
        last_completed_content_sha256: None,
        recovery_key_confirmed_at: None,
    })
}

pub(crate) fn confirm_recovery_key(
    database_path: &Path,
    workspace_id: &str,
) -> Result<(), BackupProfileError> {
    let connection = storage::open_writable_connection(database_path)?;
    let changed = connection.execute(
        "UPDATE momso_backup_profiles
         SET recovery_key_confirmed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE workspace_id = ?1",
        [workspace_id],
    )?;
    if changed != 1 {
        return Err(BackupProfileError::SnapshotStateChanged);
    }
    Ok(())
}

pub(crate) fn clear_recovery_key_confirmation(
    database_path: &Path,
    workspace_id: &str,
) -> Result<(), BackupProfileError> {
    let connection = storage::open_writable_connection(database_path)?;
    connection.execute(
        "UPDATE momso_backup_profiles
         SET recovery_key_confirmed_at = NULL,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE workspace_id = ?1",
        [workspace_id],
    )?;
    Ok(())
}

fn hash_bytes(hasher: &mut Sha256, label: &[u8], value: &[u8]) {
    hasher.update((label.len() as u64).to_le_bytes());
    hasher.update(label);
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

fn hash_optional(hasher: &mut Sha256, label: &[u8], value: Option<String>) {
    match value {
        Some(value) => hash_bytes(hasher, label, value.as_bytes()),
        None => hash_bytes(hasher, label, b"<null>"),
    }
}

pub(crate) fn local_content_state(
    database_path: &Path,
    objects_path: &Path,
) -> Result<LocalContentState, BackupProfileError> {
    let object_files = object_file_states(objects_path)?;
    local_content_state_from_inventory(database_path, &object_files)
}

pub(crate) fn local_content_state_from_inventory(
    database_path: &Path,
    object_files: &[ObjectFileState],
) -> Result<LocalContentState, BackupProfileError> {
    let connection = storage::open_writable_connection(database_path)?;
    let mut hasher = Sha256::new();
    hash_bytes(&mut hasher, b"format", b"momso-local-content-v2");

    let record_count = connection.query_row("SELECT count(*) FROM momso_records", [], |row| {
        row.get::<_, i64>(0)
    })? as u64;
    let mut records = connection.prepare(
        "SELECT v.record_id, v.version, v.record_kind, v.title, v.status,
                v.summary, v.summary_sha256, v.summary_basis_body_sha256,
                v.body_sha256, v.body_bytes, v.change_kind, v.source_version,
                v.actor, v.confirmed_by, v.reviewed_by, v.reviewed_at,
                v.plan_digest, v.created_at, b.body
         FROM momso_record_versions v
         JOIN momso_record_bodies b
           ON b.record_id = v.record_id AND b.version = v.version
         ORDER BY v.record_id ASC, v.version ASC",
    )?;
    let mut rows = records.query([])?;
    while let Some(row) = rows.next()? {
        for (index, label) in [
            b"record_id".as_slice(),
            b"version".as_slice(),
            b"record_kind".as_slice(),
            b"title".as_slice(),
            b"status".as_slice(),
        ]
        .iter()
        .enumerate()
        {
            let value = if index == 1 {
                row.get::<_, i64>(index)?.to_string()
            } else {
                row.get::<_, String>(index)?
            };
            hash_bytes(&mut hasher, label, value.as_bytes());
        }
        for (index, label) in [
            (5, b"summary".as_slice()),
            (6, b"summary_sha256".as_slice()),
            (7, b"summary_basis_body_sha256".as_slice()),
        ] {
            hash_optional(&mut hasher, label, row.get(index)?);
        }
        for (index, label) in [
            (8, b"body_sha256".as_slice()),
            (10, b"change_kind".as_slice()),
            (12, b"actor".as_slice()),
        ] {
            hash_bytes(&mut hasher, label, row.get::<_, String>(index)?.as_bytes());
        }
        hash_bytes(
            &mut hasher,
            b"body_bytes",
            row.get::<_, i64>(9)?.to_string().as_bytes(),
        );
        hash_optional(
            &mut hasher,
            b"source_version",
            row.get::<_, Option<i64>>(11)?
                .map(|value| value.to_string()),
        );
        for (index, label) in [
            (13, b"confirmed_by".as_slice()),
            (14, b"reviewed_by".as_slice()),
            (15, b"reviewed_at".as_slice()),
            (16, b"plan_digest".as_slice()),
        ] {
            hash_optional(&mut hasher, label, row.get(index)?);
        }
        hash_bytes(
            &mut hasher,
            b"created_at",
            row.get::<_, String>(17)?.as_bytes(),
        );
        hash_bytes(&mut hasher, b"body", row.get::<_, String>(18)?.as_bytes());
    }

    let mut terms = connection.prepare(
        "SELECT id, heard_as, corrected_to, note, enabled, created_at, updated_at
         FROM momso_transcription_terms ORDER BY id ASC",
    )?;
    let mut rows = terms.query([])?;
    while let Some(row) = rows.next()? {
        for (index, label) in [
            b"term_id".as_slice(),
            b"heard_as".as_slice(),
            b"corrected_to".as_slice(),
        ]
        .iter()
        .enumerate()
        {
            hash_bytes(&mut hasher, label, row.get::<_, String>(index)?.as_bytes());
        }
        hash_optional(&mut hasher, b"term_note", row.get(3)?);
        hash_bytes(
            &mut hasher,
            b"term_enabled",
            row.get::<_, i64>(4)?.to_string().as_bytes(),
        );
        hash_bytes(
            &mut hasher,
            b"term_created_at",
            row.get::<_, String>(5)?.as_bytes(),
        );
        hash_bytes(
            &mut hasher,
            b"term_updated_at",
            row.get::<_, String>(6)?.as_bytes(),
        );
    }

    let mut source_assets = connection.prepare(
        "SELECT id, asset_kind, content_sha256, byte_length, storage_relative_path,
                original_file_name, media_type, captured_by, created_at,
                integrity_state, integrity_issue, quarantined_at, quarantined_by
         FROM momso_source_assets ORDER BY id ASC",
    )?;
    let mut expected_source_files = HashMap::new();
    let mut rows = source_assets.query([])?;
    while let Some(row) = rows.next()? {
        let source_asset_sha256: String = row.get(2)?;
        let source_asset_bytes: i64 = row.get(3)?;
        let source_asset_path: String = row.get(4)?;
        for (index, label) in [
            b"source_asset_id".as_slice(),
            b"source_asset_kind".as_slice(),
        ]
        .iter()
        .enumerate()
        {
            hash_bytes(&mut hasher, label, row.get::<_, String>(index)?.as_bytes());
        }
        hash_bytes(
            &mut hasher,
            b"source_asset_sha256",
            source_asset_sha256.as_bytes(),
        );
        hash_bytes(
            &mut hasher,
            b"source_asset_bytes",
            source_asset_bytes.to_string().as_bytes(),
        );
        hash_bytes(
            &mut hasher,
            b"source_asset_path",
            source_asset_path.as_bytes(),
        );
        hash_bytes(
            &mut hasher,
            b"source_asset_original_name",
            row.get::<_, String>(5)?.as_bytes(),
        );
        hash_optional(&mut hasher, b"source_asset_media_type", row.get(6)?);
        for (index, label) in [
            (7, b"source_asset_captured_by".as_slice()),
            (8, b"source_asset_created_at".as_slice()),
        ] {
            hash_bytes(&mut hasher, label, row.get::<_, String>(index)?.as_bytes());
        }
        let integrity_state: String = row.get(9)?;
        hash_bytes(
            &mut hasher,
            b"source_asset_integrity_state",
            integrity_state.as_bytes(),
        );
        hash_optional(&mut hasher, b"source_asset_integrity_issue", row.get(10)?);
        hash_optional(&mut hasher, b"source_asset_quarantined_at", row.get(11)?);
        hash_optional(&mut hasher, b"source_asset_quarantined_by", row.get(12)?);
        let source_asset_bytes = u64::try_from(source_asset_bytes)
            .map_err(|_| BackupProfileError::SourceAssetIntegrity)?;
        match integrity_state.as_str() {
            "active" => {
                if expected_source_files
                    .insert(
                        source_asset_path.replace('\\', "/"),
                        (source_asset_sha256, source_asset_bytes),
                    )
                    .is_some()
                {
                    return Err(BackupProfileError::SourceAssetIntegrity);
                }
            }
            "quarantined" => {}
            _ => return Err(BackupProfileError::SourceAssetIntegrity),
        }
    }

    let mut source_links = connection.prepare(
        "SELECT record_id, record_version, asset_id, relation_role, linked_by, created_at
         FROM momso_record_source_assets
         ORDER BY record_id ASC, record_version ASC, asset_id ASC, relation_role ASC",
    )?;
    let mut rows = source_links.query([])?;
    while let Some(row) = rows.next()? {
        hash_bytes(
            &mut hasher,
            b"source_link_record_id",
            row.get::<_, String>(0)?.as_bytes(),
        );
        hash_bytes(
            &mut hasher,
            b"source_link_record_version",
            row.get::<_, i64>(1)?.to_string().as_bytes(),
        );
        for (index, label) in [
            (2, b"source_link_asset_id".as_slice()),
            (3, b"source_link_relation".as_slice()),
            (4, b"source_link_linked_by".as_slice()),
            (5, b"source_link_created_at".as_slice()),
        ] {
            hash_bytes(&mut hasher, label, row.get::<_, String>(index)?.as_bytes());
        }
    }

    let mut object_files = object_files.to_vec();
    object_files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let mut previous_path: Option<&str> = None;
    for entry in &object_files {
        validate_inventory_path(&entry.relative_path)?;
        if previous_path == Some(entry.relative_path.as_str()) {
            return Err(BackupProfileError::SourceAssetIntegrity);
        }
        previous_path = Some(&entry.relative_path);
        hash_bytes(&mut hasher, b"object_path", entry.relative_path.as_bytes());
        hash_bytes(
            &mut hasher,
            b"object_bytes",
            entry.byte_length.to_string().as_bytes(),
        );
        hash_bytes(&mut hasher, b"object_sha256", entry.sha256.as_bytes());
        if let Some((expected_sha256, expected_bytes)) =
            expected_source_files.remove(&entry.relative_path)
        {
            if entry.sha256 != expected_sha256 || entry.byte_length != expected_bytes {
                return Err(BackupProfileError::SourceAssetIntegrity);
            }
        }
    }
    if !expected_source_files.is_empty() {
        return Err(BackupProfileError::SourceAssetIntegrity);
    }

    Ok(LocalContentState {
        sha256: format!("{:x}", hasher.finalize()),
        record_count,
        object_count: object_files.len() as u64,
    })
}

fn object_file_states(objects_path: &Path) -> Result<Vec<ObjectFileState>, BackupProfileError> {
    let mut entries = WalkDir::new(objects_path)
        .follow_links(false)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    entries.retain(|entry| entry.file_type().is_file());
    entries.sort_by_key(|entry| entry.path().to_path_buf());
    entries
        .into_iter()
        .map(|entry| {
            let relative = entry
                .path()
                .strip_prefix(objects_path)
                .map_err(|_| BackupProfileError::UnsafeObjectPath)?;
            let relative_path = relative
                .to_str()
                .ok_or(BackupProfileError::UnsafeObjectPath)?
                .replace('\\', "/");
            validate_inventory_path(&relative_path)?;
            let mut file = File::open(entry.path())?;
            let mut hasher = Sha256::new();
            let mut byte_length = 0_u64;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
                byte_length = byte_length
                    .checked_add(read as u64)
                    .ok_or(BackupProfileError::SourceAssetIntegrity)?;
            }
            Ok(ObjectFileState {
                relative_path,
                sha256: format!("{:x}", hasher.finalize()),
                byte_length,
            })
        })
        .collect()
}

fn validate_inventory_path(path: &str) -> Result<(), BackupProfileError> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(BackupProfileError::UnsafeObjectPath);
    }
    Ok(())
}

impl From<walkdir::Error> for BackupProfileError {
    fn from(error: walkdir::Error) -> Self {
        match error.into_io_error() {
            Some(error) => Self::Io(error),
            None => Self::UnsafeObjectPath,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn content_hash_ignores_runtime_timestamps_but_tracks_dictionary_and_files() {
        let temp = tempdir().expect("tempdir");
        let database_path = temp.path().join("momso.db");
        let backups_path = temp.path().join("backups");
        let objects_path = temp.path().join("objects");
        fs::create_dir_all(&objects_path).expect("objects create");
        storage::initialize(&database_path, &backups_path, "test").expect("storage initializes");
        let first = local_content_state(&database_path, &objects_path).expect("hashes");

        storage::inspect_and_touch(&database_path, "test").expect("runtime timestamp updates");
        let after_runtime = local_content_state(&database_path, &objects_path).expect("hashes");
        assert_eq!(first.sha256, after_runtime.sha256);

        let connection = storage::open_writable_connection(&database_path).expect("database opens");
        connection
            .execute(
                "INSERT INTO momso_transcription_terms (
                   id, heard_as, corrected_to, enabled, created_at, updated_at
                 ) VALUES ('term', '요가아', '요가', 1, 'now', 'now')",
                [],
            )
            .expect("term inserts");
        drop(connection);
        let after_term = local_content_state(&database_path, &objects_path).expect("hashes");
        assert_ne!(first.sha256, after_term.sha256);

        fs::write(objects_path.join("recording.mp3"), b"audio bytes").expect("object writes");
        let after_file = local_content_state(&database_path, &objects_path).expect("hashes");
        assert_ne!(after_term.sha256, after_file.sha256);
        assert_eq!(after_file.object_count, 1);
    }

    #[test]
    fn content_hash_tracks_source_asset_ledger_and_record_links() {
        let temp = tempdir().expect("tempdir");
        let database_path = temp.path().join("momso.db");
        let objects_path = temp.path().join("objects");
        fs::create_dir_all(&objects_path).expect("objects create");
        storage::initialize(&database_path, &temp.path().join("backups"), "test")
            .expect("storage initializes");
        records::fixture_record(&database_path, "Morning class", "review copy", None).unwrap();
        let after_record = local_content_state(&database_path, &objects_path).expect("hashes");

        let connection = storage::open_writable_connection(&database_path).expect("database opens");
        let (record_id, record_version): (String, i64) = connection
            .query_row(
                "SELECT id, current_version FROM momso_records LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("record identity");
        let content_sha256 = format!("{:x}", Sha256::digest([]));
        let storage_path = format!("sha256/{}/{content_sha256}", &content_sha256[..2]);
        connection
            .execute(
                "INSERT INTO momso_source_assets (
                   id, asset_kind, content_sha256, byte_length, storage_relative_path,
                   original_file_name, captured_by, created_at
                 ) VALUES ('asset', 'raw_transcript', ?1, 0, ?2, 'raw.txt', 'tester', 'now')",
                rusqlite::params![content_sha256, storage_path],
            )
            .expect("asset inserts");
        drop(connection);
        let stored = objects_path.join(&storage_path);
        fs::create_dir_all(stored.parent().expect("storage parent")).expect("storage creates");
        fs::write(stored, []).expect("empty source writes");
        let after_asset = local_content_state(&database_path, &objects_path).expect("hashes");
        assert_ne!(after_record.sha256, after_asset.sha256);

        let connection = storage::open_writable_connection(&database_path).expect("database opens");
        connection
            .execute(
                "INSERT INTO momso_record_source_assets (
                   record_id, record_version, asset_id, relation_role, linked_by, created_at
                 ) VALUES (?1, ?2, 'asset', 'source', 'tester', 'now')",
                rusqlite::params![record_id, record_version],
            )
            .expect("link inserts");
        drop(connection);
        let after_link = local_content_state(&database_path, &objects_path).expect("hashes");
        assert_ne!(after_asset.sha256, after_link.sha256);
    }

    #[test]
    fn content_state_rejects_a_source_ledger_entry_without_matching_bytes() {
        let temp = tempdir().expect("tempdir");
        let database_path = temp.path().join("momso.db");
        let objects_path = temp.path().join("objects");
        fs::create_dir_all(&objects_path).expect("objects create");
        storage::initialize(&database_path, &temp.path().join("backups"), "test")
            .expect("storage initializes");
        let connection = storage::open_writable_connection(&database_path).expect("database opens");
        let content_sha256 = "a".repeat(64);
        let storage_path = format!("sha256/aa/{content_sha256}");
        connection
            .execute(
                "INSERT INTO momso_source_assets (
                   id, asset_kind, content_sha256, byte_length, storage_relative_path,
                   original_file_name, captured_by, created_at
                 ) VALUES ('missing', 'raw_transcript', ?1, 0, ?2, 'raw.txt', 'tester', 'now')",
                rusqlite::params![content_sha256, storage_path],
            )
            .expect("asset inserts");
        drop(connection);

        assert!(matches!(
            local_content_state(&database_path, &objects_path),
            Err(BackupProfileError::SourceAssetIntegrity)
        ));
    }
}
