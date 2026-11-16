use rusqlite::{backup::Backup, Connection, OptionalExtension, TransactionBehavior};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

const LATEST_SCHEMA_VERSION: u32 = 85;
const MAX_AUTOMATED_BACKUPS: usize = 5;

struct Migration {
    version: u32,
    description: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "create runtime metadata tables",
        sql: include_str!("../migrations/0001_runtime_metadata.sql"),
    },
    Migration {
        version: 2,
        description: "create record lifecycle tables",
        sql: include_str!("../migrations/0002_record_lifecycle.sql"),
    },
    Migration {
        version: 3,
        description: "create content-free feedback outbox",
        sql: include_str!("../migrations/0003_feedback_outbox.sql"),
    },
    Migration {
        version: 4,
        description: "create user-owned backup and transcription dictionary tables",
        sql: include_str!("../migrations/0004_user_owned_backup.sql"),
    },
    Migration {
        version: 5,
        description: "create source asset and durable operation tables",
        sql: include_str!("../migrations/0005_source_assets_and_operations.sql"),
    },
    Migration {
        version: 6,
        description: "add source quarantine and recovery key confirmation",
        sql: include_str!("../migrations/0006_integrity_recovery.sql"),
    },
    Migration {
        version: 7,
        description: "create local AI usage ledger",
        sql: include_str!("../migrations/0007_ai_usage.sql"),
    },
    Migration {
        version: 8,
        description: "create rebuildable archive indexes",
        sql: include_str!("../migrations/0008_archive_index.sql"),
    },
    Migration {
        version: 9,
        description: "create chat storage",
        sql: include_str!("../migrations/0009_chats.sql"),
    },
    Migration {
        version: 10,
        description: "add Codex chat thread",
        sql: include_str!("../migrations/0010_chat_codex.sql"),
    },
    Migration {
        version: 11,
        description: "add chat reasoning effort",
        sql: include_str!("../migrations/0011_chat_effort.sql"),
    },
    Migration {
        version: 12,
        description: "create chat groups",
        sql: include_str!("../migrations/0012_chat_groups.sql"),
    },
    Migration {
        version: 13,
        description: "store chat context usage",
        sql: include_str!("../migrations/0013_chat_context.sql"),
    },
    Migration {
        version: 15,
        description: "store archive location",
        sql: include_str!("../migrations/0015_archive_location.sql"),
    },
    Migration {
        version: 40,
        description: "add Claude chat sessions",
        sql: include_str!("../migrations/0040_chat_claude.sql"),
    },
    Migration {
        version: 70,
        description: "add selected chat skill",
        sql: include_str!("../migrations/0070_chat_skill.sql"),
    },
    Migration {
        version: 71,
        description: "track provider handoff progress",
        sql: include_str!("../migrations/0071_chat_handoff.sql"),
    },
    Migration {
        version: 72,
        description: "track chat delivery metadata and fast retry",
        sql: include_str!("../migrations/0072_chat_delivery.sql"),
    },
    Migration {
        version: 73,
        description: "store chat pin and archive state",
        sql: include_str!("../migrations/0073_chat_visibility.sql"),
    },
    Migration {
        version: 74,
        description: "link chat attachments to messages",
        sql: include_str!("../migrations/0074_chat_attachments.sql"),
    },
    Migration {
        version: 75,
        description: "track provider reported actual reasoning effort",
        sql: include_str!("../migrations/0075_chat_actual_effort.sql"),
    },
    Migration {
        version: 80,
        description: "store chat reply finish reason",
        sql: include_str!("../migrations/0080_chat_finish_reason.sql"),
    },
    Migration {
        version: 81,
        description: "track imported conversations",
        sql: include_str!("../migrations/0081_chat_import.sql"),
    },
    Migration {
        version: 82,
        description: "store provider model context limits",
        sql: include_str!("../migrations/0082_provider_context_limits.sql"),
    },
    Migration {
        version: 83,
        description: "reconcile imported conversations and provider model context limits",
        sql: include_str!("../migrations/0083_reconcile_context_import.sql"),
    },
    Migration {
        version: 84,
        description: "record chat authentication method",
        sql: include_str!("../migrations/0084_chat_auth_method.sql"),
    },
    Migration {
        version: 85,
        description: "store reported chat fast state",
        sql: include_str!("../migrations/0085_chat_fast_state.sql"),
    },
];

#[derive(Debug, Error)]
pub(crate) enum StorageError {
    #[error("Local file operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("The local database integrity check failed: {0}")]
    Integrity(String),
    #[error("The local database schema {found} is newer than this app supports ({supported})")]
    UnsupportedSchema { found: u32, supported: u32 },
    #[error("System clock is earlier than the Unix epoch")]
    InvalidSystemTime,
    #[error("Restore failed and the safety backup could not be reapplied")]
    RestoreRollbackFailed,
}

pub(crate) struct StorageStatus {
    pub(crate) schema_version: u32,
    pub(crate) health: &'static str,
}

pub(crate) fn initialize(
    database_path: &Path,
    backups_path: &Path,
    app_version: &str,
) -> Result<StorageStatus, StorageError> {
    let had_existing_data = database_path
        .metadata()
        .map(|metadata| metadata.len() > 0)
        .unwrap_or(false);
    let mut connection = open_connection(database_path)?;
    ensure_supported_schema(&connection)?;

    if had_existing_data {
        check_integrity(&connection)?;
    }
    configure_writable_storage(&connection)?;

    let pending = pending_migrations(&connection, MIGRATIONS)?;
    let has_pending_migrations = !pending.is_empty();
    if had_existing_data && has_pending_migrations {
        create_backup(&connection, backups_path, "pre-migration")?;
        prune_automated_backups(backups_path)?;
    }

    for migration in pending {
        apply_migration(&mut connection, migration)?;
    }

    record_runtime_state(&mut connection, app_version)?;
    check_integrity(&connection)?;

    Ok(StorageStatus {
        schema_version: schema_version(&connection)?,
        health: "ok",
    })
}

pub(crate) fn inspect_and_touch(
    database_path: &Path,
    app_version: &str,
) -> Result<StorageStatus, StorageError> {
    let mut connection = open_connection(database_path)?;
    let version = ensure_supported_schema(&connection)?;

    check_integrity(&connection)?;
    configure_writable_storage(&connection)?;
    record_runtime_state(&mut connection, app_version)?;
    Ok(StorageStatus {
        schema_version: version,
        health: "ok",
    })
}

pub(crate) fn create_manual_backup(
    database_path: &Path,
    backups_path: &Path,
) -> Result<PathBuf, StorageError> {
    let connection = open_connection(database_path)?;
    ensure_supported_schema(&connection)?;
    check_integrity(&connection)?;
    configure_writable_storage(&connection)?;
    create_backup(&connection, backups_path, "manual")
}

pub(crate) fn create_snapshot_file(
    database_path: &Path,
    destination_path: &Path,
) -> Result<(), StorageError> {
    let source = open_connection(database_path)?;
    ensure_supported_schema(&source)?;
    check_integrity(&source)?;
    configure_writable_storage(&source)?;
    if let Some(parent) = destination_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut destination = Connection::open(destination_path)?;
    destination.busy_timeout(Duration::from_secs(5))?;
    {
        let backup = Backup::new(&source, &mut destination)?;
        backup.run_to_completion(128, Duration::from_millis(10), None)?;
    }
    check_integrity(&destination)?;
    Ok(())
}

pub(crate) fn sanitize_user_owned_backup_snapshot(path: &Path) -> Result<(), StorageError> {
    let connection = Connection::open(path)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "DELETE")?;
    connection.execute("DELETE FROM feedback_outbox", [])?;
    connection.execute("DELETE FROM momso_operation_runs", [])?;
    connection.execute("DELETE FROM ai_usage_events", [])?;
    drop(connection);
    validate_snapshot_file(path)
}

pub(crate) fn validate_snapshot_file(snapshot_path: &Path) -> Result<(), StorageError> {
    let connection = open_connection(snapshot_path)?;
    let version = ensure_supported_schema(&connection)?;
    let pending = pending_migrations(&connection, MIGRATIONS)?;
    let compatible_previous = (71..LATEST_SCHEMA_VERSION).contains(&version)
        && pending.iter().all(|migration| migration.version > version);
    if version != LATEST_SCHEMA_VERSION && !compatible_previous {
        return Err(StorageError::UnsupportedSchema {
            found: version,
            supported: LATEST_SCHEMA_VERSION,
        });
    }
    if !pending.is_empty() && !compatible_previous {
        return Err(StorageError::Integrity(
            "누락된 데이터베이스 변경이 있습니다".into(),
        ));
    }
    check_integrity(&connection)
}

pub(crate) fn restore_snapshot_file(
    snapshot_path: &Path,
    database_path: &Path,
    backups_path: &Path,
) -> Result<PathBuf, StorageError> {
    restore_snapshot_file_with_hook(snapshot_path, database_path, backups_path, || Ok(()))
}

pub(crate) fn restore_snapshot_file_with_hook(
    snapshot_path: &Path,
    database_path: &Path,
    backups_path: &Path,
    after_replace: impl FnOnce() -> Result<(), StorageError>,
) -> Result<PathBuf, StorageError> {
    validate_snapshot_file(snapshot_path)?;
    let original = open_connection(snapshot_path)?;
    let mut migrated_dir = None;
    let source = if schema_version(&original)? < LATEST_SCHEMA_VERSION {
        fs::create_dir_all(backups_path)?;
        let dir = tempfile::Builder::new()
            .prefix("momso-snapshot-upgrade-")
            .tempdir_in(backups_path)?;
        let copied = dir.path().join("momso.db");
        let mut copy = open_connection(&copied)?;
        {
            let backup = Backup::new(&original, &mut copy)?;
            backup.run_to_completion(128, Duration::from_millis(10), None)?;
        }
        for migration in pending_migrations(&copy, MIGRATIONS)? {
            apply_migration(&mut copy, migration)?;
        }
        copy.execute("INSERT INTO app_runtime_meta(key,value,updated_at) VALUES('schema_version',?1,CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at", [LATEST_SCHEMA_VERSION.to_string()])?;
        check_integrity(&copy)?;
        migrated_dir = Some(dir);
        copy
    } else {
        original
    };
    let mut destination = open_connection(database_path)?;
    ensure_supported_schema(&destination)?;
    check_integrity(&destination)?;
    configure_writable_storage(&destination)?;
    let safety_backup = create_backup(&destination, backups_path, "pre-restore")?;

    let restore_result = (|| -> Result<(), StorageError> {
        {
            let backup = Backup::new(&source, &mut destination)?;
            backup.run_to_completion(128, Duration::from_millis(10), None)?;
        }
        after_replace()?;
        check_integrity(&destination)?;
        Ok(())
    })();
    if let Err(error) = restore_result {
        let rollback = (|| -> Result<(), StorageError> {
            let safety_source = open_connection(&safety_backup)?;
            let backup = Backup::new(&safety_source, &mut destination)?;
            backup.run_to_completion(128, Duration::from_millis(10), None)?;
            drop(backup);
            check_integrity(&destination)
        })();
        if rollback.is_err() {
            return Err(StorageError::RestoreRollbackFailed);
        }
        return Err(error);
    }
    drop(source);
    drop(migrated_dir);
    Ok(safety_backup)
}

fn open_connection(database_path: &Path) -> Result<Connection, StorageError> {
    let connection = Connection::open(database_path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    Ok(connection)
}

pub(crate) fn open_writable_connection(database_path: &Path) -> Result<Connection, StorageError> {
    let connection = open_connection(database_path)?;
    ensure_supported_schema(&connection)?;
    configure_writable_storage(&connection)?;
    Ok(connection)
}

fn configure_writable_storage(connection: &Connection) -> Result<(), StorageError> {
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    Ok(())
}

fn schema_version(connection: &Connection) -> Result<u32, StorageError> {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(StorageError::from)
}

fn pending_migrations<'a>(
    connection: &Connection,
    migrations: &'a [Migration],
) -> Result<Vec<&'a Migration>, StorageError> {
    let ledger_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_schema_migrations')",
        [], |row| row.get(0),
    )?;
    if !ledger_exists {
        if schema_version(connection)? != 0 {
            return Err(StorageError::Integrity(
                "데이터베이스 변경 기록을 찾지 못했습니다".into(),
            ));
        }
        return Ok(migrations.iter().collect());
    }
    let mut pending = Vec::new();
    for migration in migrations {
        let recorded: Option<String> = connection
            .query_row(
                "SELECT description FROM app_schema_migrations WHERE version=?1",
                [migration.version],
                |row| row.get(0),
            )
            .optional()?;
        match recorded {
            None => {
                if migration.version == 82 {
                    let has_context: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='provider_context_limits')", [], |r| r.get(0))?;
                    if has_context && !legacy_context_81_is_exact(connection)? {
                        return Err(StorageError::Integrity(
                            "모델 한도 표의 구조를 확인할 수 없습니다".into(),
                        ));
                    }
                }
                pending.push(migration)
            }
            Some(description) if description == migration.description => {}
            Some(description)
                if migration.version == 81
                    && description == "store provider model context limits"
                    && legacy_context_81_is_exact(connection)? => {}
            Some(description)
                if migration.version == 82
                    && description
                        == "reconcile imported conversations and provider model context limits"
                    && legacy_context_81_is_exact(connection)?
                    && table_is_exact(
                        connection,
                        "chat_imports",
                        include_str!("../migrations/0081_chat_import.sql"),
                    )? => {}
            Some(_) => {
                return Err(StorageError::Integrity(format!(
                    "데이터베이스 변경 번호 {}가 다른 내용에 사용됐습니다",
                    migration.version
                )))
            }
        }
    }
    Ok(pending)
}

fn legacy_context_81_is_exact(connection: &Connection) -> Result<bool, StorageError> {
    table_is_exact(
        connection,
        "provider_context_limits",
        include_str!("../migrations/0082_provider_context_limits.sql"),
    )
}

fn table_is_exact(
    connection: &Connection,
    table: &str,
    expected: &str,
) -> Result<bool, StorageError> {
    let sql: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |row| row.get(0),
        )
        .optional()?;
    let normalize = |value: &str| {
        value
            .replace("\r\n", "\n")
            .trim()
            .trim_end_matches(';')
            .trim()
            .to_owned()
    };
    Ok(sql.as_deref().is_some_and(|value| {
        normalize(value)
            == normalize(&expected.replace("CREATE TABLE IF NOT EXISTS", "CREATE TABLE"))
    }))
}

fn ensure_supported_schema(connection: &Connection) -> Result<u32, StorageError> {
    let version = schema_version(connection)?;
    if version > LATEST_SCHEMA_VERSION {
        return Err(StorageError::UnsupportedSchema {
            found: version,
            supported: LATEST_SCHEMA_VERSION,
        });
    }
    Ok(version)
}

fn apply_migration(connection: &mut Connection, migration: &Migration) -> Result<(), StorageError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let previous_version = schema_version(&transaction)?;
    transaction.execute_batch(migration.sql)?;
    transaction.execute(
        "INSERT INTO app_schema_migrations (version, description) VALUES (?1, ?2)",
        (migration.version, migration.description),
    )?;
    transaction.pragma_update(
        None,
        "user_version",
        previous_version.max(migration.version),
    )?;
    transaction.commit()?;
    Ok(())
}

fn record_runtime_state(
    connection: &mut Connection,
    app_version: &str,
) -> Result<(), StorageError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (key, value) in [
        ("app_version", app_version.to_string()),
        ("schema_version", LATEST_SCHEMA_VERSION.to_string()),
    ] {
        transaction.execute(
            "INSERT INTO app_runtime_meta (key, value, updated_at)
             VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(key) DO UPDATE SET
               value = excluded.value,
               updated_at = excluded.updated_at",
            (key, value),
        )?;
    }
    transaction.execute(
        "INSERT INTO app_runtime_meta (key, value, updated_at)
         VALUES (
           'last_opened_at',
           strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
           strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         )
         ON CONFLICT(key) DO UPDATE SET
           value = excluded.value,
           updated_at = excluded.updated_at",
        [],
    )?;
    transaction.commit()?;
    Ok(())
}

fn check_integrity(connection: &Connection) -> Result<(), StorageError> {
    let quick_check: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if quick_check != "ok" {
        return Err(StorageError::Integrity(quick_check));
    }

    let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
    let mut rows = statement.query([])?;
    if let Some(row) = rows.next()? {
        let table: String = row.get(0)?;
        return Err(StorageError::Integrity(format!(
            "foreign key violation in table {table}"
        )));
    }

    Ok(())
}

fn create_backup(
    source: &Connection,
    backups_path: &Path,
    prefix: &str,
) -> Result<PathBuf, StorageError> {
    fs::create_dir_all(backups_path)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StorageError::InvalidSystemTime)?
        .as_millis();
    let backup_path =
        backups_path.join(format!("{prefix}-{timestamp}-{}.db", uuid::Uuid::new_v4()));
    let backup_result = (|| -> Result<(), StorageError> {
        let mut destination = Connection::open(&backup_path)?;
        destination.busy_timeout(Duration::from_secs(5))?;
        {
            let backup = Backup::new(source, &mut destination)?;
            backup.run_to_completion(128, Duration::from_millis(10), None)?;
        }
        check_integrity(&destination)?;
        Ok(())
    })();

    if let Err(error) = backup_result {
        let _ = fs::remove_file(&backup_path);
        return Err(error);
    }

    Ok(backup_path)
}

fn prune_automated_backups(backups_path: &Path) -> Result<(), StorageError> {
    let mut backups = fs::read_dir(backups_path)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("pre-migration-")
        })
        .collect::<Vec<_>>();
    backups.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));

    for old_backup in backups.into_iter().skip(MAX_AUTOMATED_BACKUPS) {
        fs::remove_file(old_backup.path())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn upgrading_existing_preview_keeps_four_documents_without_reindexing() {
        use super::*;
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("app-data");
        let legacy = data.join("archive");
        std::fs::create_dir_all(legacy.join("zettels")).unwrap();
        let database = data.join("momso.db");
        let backups = data.join("backups");
        let mut conn = open_connection(&database).unwrap();
        configure_writable_storage(&conn).unwrap();
        for migration in MIGRATIONS.iter().filter(|item| item.version != 15) {
            apply_migration(&mut conn, migration).unwrap();
        }
        for second in 0..4 {
            let id = format!("zet_20261115_1200{second:02}");
            let path = format!("zettels/{id}.md");
            std::fs::write(legacy.join(&path), "existing").unwrap();
            conn.execute("INSERT INTO archive_zets(id,title,summary,kind,status,created_at,updated_at,file_path) VALUES(?1,'title','summary','note','draft','a','a',?2)", rusqlite::params![id,path]).unwrap();
        }
        drop(conn);
        initialize(&database, &backups, "test").unwrap();
        let location =
            crate::archive_location::ArchiveLocation::load(database.clone(), temp.path(), vec![])
                .unwrap();
        assert_eq!(*location.current.read().await, legacy);
        assert!(!location.needs_initial_index);
        assert_eq!(
            crate::archive_zet::list(&database, None, None)
                .unwrap()
                .len(),
            4
        );
    }
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn applies_missing_lower_migrations_without_rewinding_or_losing_claude_data() {
        let temp = tempdir().unwrap();
        let db = temp.path().join("momso.db");
        let backups = temp.path().join("backups");
        let mut conn = open_connection(&db).unwrap();
        configure_writable_storage(&conn).unwrap();
        for migration in MIGRATIONS
            .iter()
            .filter(|m| m.version <= 11 || m.version == 40)
        {
            apply_migration(&mut conn, migration).unwrap();
        }
        conn.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES ('chat_test','요가','x','x')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO chat_claude_sessions(chat_id,session_id) VALUES ('chat_test','session-test')", []).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), 40);
        drop(conn);
        assert!(validate_snapshot_file(&db).is_err());
        initialize(&db, &backups, "test").unwrap();
        let mut conn = open_connection(&db).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), LATEST_SCHEMA_VERSION);
        assert!(pending_migrations(&conn, MIGRATIONS).unwrap().is_empty());
        let session: String = conn
            .query_row(
                "SELECT session_id FROM chat_claude_sessions WHERE chat_id='chat_test'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(session, "session-test");
        let lower = [Migration {
            version: 30,
            description: "late migration",
            sql: "CREATE TABLE late_test(value TEXT)",
        }];
        for migration in pending_migrations(&conn, &lower).unwrap() {
            apply_migration(&mut conn, migration).unwrap();
        }
        assert!(pending_migrations(&conn, &lower).unwrap().is_empty());
        assert_eq!(schema_version(&conn).unwrap(), LATEST_SCHEMA_VERSION);
        assert!(backups.exists());
    }

    #[test]
    fn migration_seventy_preserves_skill_when_older_migrations_are_pending() {
        let temp = tempdir().unwrap();
        let db = temp.path().join("momso.db");
        let mut conn = open_connection(&db).unwrap();
        configure_writable_storage(&conn).unwrap();
        for migration in MIGRATIONS
            .iter()
            .filter(|m| m.version <= 11 || m.version == 70)
        {
            apply_migration(&mut conn, migration).unwrap();
        }
        conn.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES ('chat_test','요가','x','x')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at,skill) VALUES('m','chat_test','user','말','x','probe')", []).unwrap();
        drop(conn);
        initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let conn = open_connection(&db).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), LATEST_SCHEMA_VERSION);
        assert!(pending_migrations(&conn, MIGRATIONS).unwrap().is_empty());
        let skill: String = conn
            .query_row("SELECT skill FROM chat_messages WHERE id='m'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(skill, "probe");
    }

    #[test]
    fn rejects_same_migration_number_with_different_description_without_changes() {
        let temp = tempdir().unwrap();
        let db = temp.path().join("momso.db");
        let mut conn = open_connection(&db).unwrap();
        configure_writable_storage(&conn).unwrap();
        for migration in MIGRATIONS.iter().filter(|m| m.version <= 7) {
            apply_migration(&mut conn, migration).unwrap();
        }
        let different = Migration {
            version: 8,
            description: "different eight",
            sql: "CREATE TABLE preserved_marker(value TEXT)",
        };
        apply_migration(&mut conn, &different).unwrap();
        conn.execute(
            "INSERT INTO preserved_marker(value) VALUES ('untouched')",
            [],
        )
        .unwrap();
        drop(conn);
        assert!(matches!(
            initialize(&db, &temp.path().join("backups"), "test"),
            Err(StorageError::Integrity(_))
        ));
        let conn = open_connection(&db).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), 8);
        let value: String = conn
            .query_row("SELECT value FROM preserved_marker", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "untouched");
    }

    #[test]
    fn migration_seventy_two_preserves_existing_messages_with_unknown_metadata() {
        let temp = tempdir().unwrap();
        let db = temp.path().join("momso.db");
        let mut conn = open_connection(&db).unwrap();
        configure_writable_storage(&conn).unwrap();
        for migration in MIGRATIONS.iter().filter(|m| m.version <= 71) {
            apply_migration(&mut conn, migration).unwrap();
        }
        conn.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES('chat_test','class','x','x')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('old','chat_test','assistant','original','x')", []).unwrap();
        drop(conn);
        initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let conn = open_connection(&db).unwrap();
        let (text, fast, omitted, retry): (String, Option<i64>, Option<i64>, i64) = conn
            .query_row(
                "SELECT text,fast,not_handed_over,retryable_fast FROM chat_messages WHERE id='old'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (text.as_str(), fast, omitted, retry),
            ("original", None, None, 0)
        );
    }

    #[test]
    fn previous_delivery_snapshot_restores_with_unknown_finish_reason() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("previous72.db");
        let mut conn = open_connection(&snapshot).unwrap();
        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.version <= 72)
        {
            apply_migration(&mut conn, migration).unwrap();
        }
        conn.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES('chat_test','class','x','x')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('old','chat_test','assistant','original','x')", []).unwrap();
        drop(conn);
        let before = fs::read(&snapshot).unwrap();
        validate_snapshot_file(&snapshot).unwrap();
        let current = temp.path().join("current.db");
        let backups = temp.path().join("backups");
        initialize(&current, &backups, "test").unwrap();
        restore_snapshot_file(&snapshot, &current, &backups).unwrap();
        assert_eq!(fs::read(&snapshot).unwrap(), before);
        let conn = open_connection(&current).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), LATEST_SCHEMA_VERSION);
        let value: Option<String> = conn
            .query_row(
                "SELECT finish_reason FROM chat_messages WHERE id='old'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, None);
        drop(conn);
        let detail =
            serde_json::to_value(crate::chat::read(&current, "chat_test").unwrap()).unwrap();
        assert!(detail["messages"][0]["finishReason"].is_null());
    }

    #[test]
    fn snapshot_from_other_branch_keeps_its_earlier_migration() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("other-branch.db");
        let mut conn = open_connection(&snapshot).unwrap();
        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.version <= 75)
        {
            apply_migration(&mut conn, migration).unwrap();
        }
        let other = Migration { version: 76, description: "other branch fixture", sql: "CREATE TABLE other_branch_marker(value TEXT); INSERT INTO other_branch_marker VALUES('preserved');" };
        apply_migration(&mut conn, &other).unwrap();
        drop(conn);
        let before = fs::read(&snapshot).unwrap();
        validate_snapshot_file(&snapshot).unwrap();
        let current = temp.path().join("current.db");
        let backups = temp.path().join("backups");
        initialize(&current, &backups, "test").unwrap();
        restore_snapshot_file(&snapshot, &current, &backups).unwrap();
        assert_eq!(fs::read(&snapshot).unwrap(), before);
        let conn = open_connection(&current).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(
            conn.query_row("SELECT value FROM other_branch_marker", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "preserved"
        );
    }

    #[test]
    fn schema_80_snapshot_restores_without_modifying_original() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("schema80.db");
        let mut connection = open_connection(&snapshot).unwrap();
        for migration in MIGRATIONS.iter().filter(|m| m.version <= 80) {
            apply_migration(&mut connection, migration).unwrap();
        }
        connection.execute("INSERT INTO chats(id,title,created_at,updated_at) VALUES('chat_test','class','x','x')", []).unwrap();
        drop(connection);
        let before = fs::read(&snapshot).unwrap();
        let current = temp.path().join("current.db");
        let backups = temp.path().join("backups");
        initialize(&current, &backups, "test").unwrap();
        restore_snapshot_file(&snapshot, &current, &backups).unwrap();
        assert_eq!(fs::read(&snapshot).unwrap(), before);
        let connection = open_connection(&current).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM chats WHERE id='chat_test'", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn previous_reconciliation_82_upgrades_without_losing_data() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("legacy82.db");
        let mut connection = open_connection(&path).unwrap();
        for migration in MIGRATIONS.iter().filter(|m| m.version <= 81) {
            apply_migration(&mut connection, migration).unwrap();
        }
        connection.execute("INSERT INTO chats(id,title,created_at,updated_at) VALUES('chat_test','class','x','x')", []).unwrap();
        connection
            .execute(
                "INSERT INTO chat_imports VALUES('chat_test','claude','fixture')",
                [],
            )
            .unwrap();
        let previous = Migration {
            version: 82,
            description: "reconcile imported conversations and provider model context limits",
            sql: include_str!("../migrations/0083_reconcile_context_import.sql"),
        };
        apply_migration(&mut connection, &previous).unwrap();
        connection.execute("INSERT INTO provider_context_limits VALUES('chat_test','claude','fake',1000,10,'x')", []).unwrap();
        drop(connection);
        initialize(&path, &temp.path().join("backups"), "test").unwrap();
        let db = open_connection(&path).unwrap();
        assert_eq!(schema_version(&db).unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(
            db.query_row("SELECT source_id FROM chat_imports", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "fixture"
        );
        assert_eq!(
            db.query_row(
                "SELECT limit_tokens FROM provider_context_limits",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1000
        );
    }

    #[test]
    fn both_schema_81_branches_upgrade_and_restore_without_data_loss() {
        for legacy_context in [false, true] {
            let temp = tempdir().unwrap();
            let snapshot = temp.path().join("branch.db");
            let mut connection = open_connection(&snapshot).unwrap();
            for migration in MIGRATIONS.iter().filter(|m| m.version <= 80) {
                apply_migration(&mut connection, migration).unwrap();
            }
            connection.execute("INSERT INTO chats(id,title,created_at,updated_at) VALUES('chat_test','class','x','x')", []).unwrap();
            let branch = if legacy_context {
                Migration {
                    version: 81,
                    description: "store provider model context limits",
                    sql: include_str!("../migrations/0082_provider_context_limits.sql"),
                }
            } else {
                Migration {
                    version: 81,
                    description: "track imported conversations",
                    sql: include_str!("../migrations/0081_chat_import.sql"),
                }
            };
            apply_migration(&mut connection, &branch).unwrap();
            if legacy_context {
                connection.execute("INSERT INTO provider_context_limits VALUES('chat_test','claude','fake',1000,10,'x')", []).unwrap();
            } else {
                connection
                    .execute(
                        "INSERT INTO chat_imports VALUES('chat_test','claude','fixture')",
                        [],
                    )
                    .unwrap();
            }
            drop(connection);
            let before = fs::read(&snapshot).unwrap();
            let upgrade = temp.path().join("upgrade.db");
            fs::copy(&snapshot, &upgrade).unwrap();
            initialize(&upgrade, &temp.path().join("upgrade-backups"), "test").unwrap();
            let restored = temp.path().join("restored.db");
            let backups = temp.path().join("restore-backups");
            initialize(&restored, &backups, "test").unwrap();
            restore_snapshot_file(&snapshot, &restored, &backups).unwrap();
            assert_eq!(fs::read(&snapshot).unwrap(), before);
            for path in [&upgrade, &restored] {
                let connection = open_connection(path).unwrap();
                assert_eq!(schema_version(&connection).unwrap(), LATEST_SCHEMA_VERSION);
                let contexts: i64 = connection
                    .query_row("SELECT count(*) FROM provider_context_limits", [], |r| {
                        r.get(0)
                    })
                    .unwrap();
                let imports: i64 = connection
                    .query_row("SELECT count(*) FROM chat_imports", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(
                    (contexts, imports),
                    if legacy_context { (1, 0) } else { (0, 1) }
                );
            }
        }
    }

    #[test]
    fn schema_81_context_collision_requires_the_exact_prior_table() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("invalid.db");
        let mut connection = open_connection(&path).unwrap();
        for migration in MIGRATIONS.iter().filter(|m| m.version <= 80) {
            apply_migration(&mut connection, migration).unwrap();
        }
        let foreign = Migration {
            version: 81,
            description: "store provider model context limits",
            sql: "CREATE TABLE provider_context_limits (unexpected TEXT);",
        };
        apply_migration(&mut connection, &foreign).unwrap();
        assert!(matches!(
            pending_migrations(&connection, MIGRATIONS),
            Err(StorageError::Integrity(_))
        ));
    }

    #[test]
    fn restoring_previous_schema_migrates_a_copy_without_altering_snapshot() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("previous.db");
        let mut conn = open_connection(&snapshot).unwrap();
        for migration in MIGRATIONS.iter().filter(|m| m.version <= 71) {
            apply_migration(&mut conn, migration).unwrap();
        }
        conn.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES('chat_test','class','x','x')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('old','chat_test','assistant','original','x')", []).unwrap();
        drop(conn);
        let before = fs::read(&snapshot).unwrap();
        validate_snapshot_file(&snapshot).unwrap();
        let current = temp.path().join("current.db");
        let backups = temp.path().join("backups");
        initialize(&current, &backups, "test").unwrap();
        restore_snapshot_file(&snapshot, &current, &backups).unwrap();
        assert_eq!(fs::read(&snapshot).unwrap(), before);
        let conn = open_connection(&current).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), LATEST_SCHEMA_VERSION);
        let (text, fast, omitted): (String, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT text,fast,not_handed_over FROM chat_messages WHERE id='old'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((text.as_str(), fast, omitted), ("original", None, None));
    }

    #[test]
    fn initializes_a_new_database_with_runtime_metadata() {
        let temp = tempdir().expect("tempdir");
        let database_path = temp.path().join("momso.db");
        let backups_path = temp.path().join("backups");

        let status =
            initialize(&database_path, &backups_path, "0.1.0-test").expect("storage initializes");

        assert_eq!(status.schema_version, LATEST_SCHEMA_VERSION);
        assert_eq!(status.health, "ok");
        assert!(!backups_path.exists());

        let connection = open_connection(&database_path).expect("database opens");
        let version: String = connection
            .query_row(
                "SELECT value FROM app_runtime_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .expect("schema metadata exists");
        assert_eq!(version, LATEST_SCHEMA_VERSION.to_string());
    }

    #[test]
    fn backs_up_a_legacy_database_before_migrating() {
        let temp = tempdir().expect("tempdir");
        let database_path = temp.path().join("momso.db");
        let backups_path = temp.path().join("backups");
        let legacy = Connection::open(&database_path).expect("legacy database opens");
        legacy
            .execute_batch(
                "CREATE TABLE app_runtime_meta (
                   key TEXT PRIMARY KEY NOT NULL,
                   value TEXT NOT NULL,
                   updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                 );
                 INSERT INTO app_runtime_meta (key, value) VALUES ('legacy_marker', 'preserved');",
            )
            .expect("legacy schema is created");
        drop(legacy);

        initialize(&database_path, &backups_path, "0.1.0-test").expect("legacy database migrates");

        let backup_path = fs::read_dir(&backups_path)
            .expect("backup directory exists")
            .next()
            .expect("pre-migration backup exists")
            .expect("backup entry")
            .path();
        let backup = Connection::open(backup_path).expect("backup opens");
        let marker: String = backup
            .query_row(
                "SELECT value FROM app_runtime_meta WHERE key = 'legacy_marker'",
                [],
                |row| row.get(0),
            )
            .expect("legacy data is preserved in backup");
        assert_eq!(marker, "preserved");
    }

    #[test]
    fn migrates_schema_five_to_six_without_losing_existing_data() {
        let temp = tempdir().expect("tempdir");
        let database_path = temp.path().join("momso.db");
        let backups_path = temp.path().join("backups");
        let mut connection = open_connection(&database_path).expect("database opens");
        configure_writable_storage(&connection).expect("storage configures");
        for migration in MIGRATIONS.iter().filter(|migration| migration.version <= 5) {
            apply_migration(&mut connection, migration).expect("legacy migration applies");
        }
        connection
            .execute(
                "INSERT INTO momso_transcription_terms (
                   id, heard_as, corrected_to, enabled, created_at, updated_at
                 ) VALUES ('preserved', '요가아', '요가', 1, 'now', 'now')",
                [],
            )
            .expect("existing data inserts");
        drop(connection);

        let status =
            initialize(&database_path, &backups_path, "0.1.0-test").expect("schema migrates");
        assert_eq!(status.schema_version, LATEST_SCHEMA_VERSION);
        let connection = open_connection(&database_path).expect("database opens");
        let preserved: i64 = connection
            .query_row(
                "SELECT count(*) FROM momso_transcription_terms WHERE id = 'preserved'",
                [],
                |row| row.get(0),
            )
            .expect("existing data reads");
        let new_tables: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type = 'table'
                   AND name IN ('momso_source_assets', 'momso_record_source_assets', 'momso_operation_runs')",
                [],
                |row| row.get(0),
            )
            .expect("new tables read");
        let recovery_columns: i64 = connection
            .query_row(
                "SELECT count(*) FROM pragma_table_info('momso_source_assets')
                 WHERE name IN ('integrity_state', 'integrity_issue', 'quarantined_at', 'quarantined_by')",
                [],
                |row| row.get(0),
            )
            .expect("recovery columns read");
        let confirmation_column: i64 = connection
            .query_row(
                "SELECT count(*) FROM pragma_table_info('momso_backup_profiles')
                 WHERE name = 'recovery_key_confirmed_at'",
                [],
                |row| row.get(0),
            )
            .expect("confirmation column reads");
        assert_eq!(preserved, 1);
        assert_eq!(new_tables, 3);
        assert_eq!(recovery_columns, 4);
        assert_eq!(confirmation_column, 1);
        assert_eq!(
            fs::read_dir(&backups_path)
                .expect("backup directory reads")
                .count(),
            1
        );
    }

    #[test]
    fn creates_a_consistent_manual_snapshot() {
        let temp = tempdir().expect("tempdir");
        let database_path = temp.path().join("momso.db");
        let backups_path = temp.path().join("backups");
        initialize(&database_path, &backups_path, "0.1.0-test").expect("storage initializes");

        let backup_path =
            create_manual_backup(&database_path, &backups_path).expect("manual backup succeeds");
        let backup = Connection::open(backup_path).expect("backup opens");
        check_integrity(&backup).expect("backup passes integrity checks");
        assert_eq!(
            schema_version(&backup).expect("schema version"),
            LATEST_SCHEMA_VERSION
        );
    }

    #[test]
    fn user_owned_snapshot_drops_machine_local_queues_only_from_the_copy() {
        let temp = tempdir().expect("tempdir");
        let database_path = temp.path().join("momso.db");
        let snapshot_path = temp.path().join("snapshot.db");
        let backups_path = temp.path().join("backups");
        initialize(&database_path, &backups_path, "0.1.0-test").expect("storage initializes");
        let connection = open_writable_connection(&database_path).expect("database opens");
        connection
            .execute(
                "INSERT INTO feedback_outbox (
                   feedback_id, operation, rating, workflow, response_kind,
                   completion, length_bucket, app_version, platform, schema_version
                 ) VALUES (
                   'feedback', 'upsert', 'helpful', 'ask', 'generated',
                   'completed', 'under_500', '0.1.0-test', 'windows', 1
                 )",
                [],
            )
            .expect("feedback queues");
        connection
            .execute(
                "INSERT INTO momso_operation_runs (
                   id, operation_kind, state, stage, started_at, heartbeat_at
                 ) VALUES (
                   'operation', 'user_owned_backup', 'running', 'creating_archive',
                   'now', 'now'
                 )",
                [],
            )
            .expect("operation queues");
        drop(connection);

        create_snapshot_file(&database_path, &snapshot_path).expect("snapshot creates");
        sanitize_user_owned_backup_snapshot(&snapshot_path).expect("snapshot sanitizes");

        let source = open_connection(&database_path).expect("source opens");
        let snapshot = open_connection(&snapshot_path).expect("snapshot opens");
        let source_count: i64 = source
            .query_row("SELECT count(*) FROM feedback_outbox", [], |row| row.get(0))
            .expect("source queue reads");
        let snapshot_count: i64 = snapshot
            .query_row("SELECT count(*) FROM feedback_outbox", [], |row| row.get(0))
            .expect("snapshot queue reads");
        let source_operations: i64 = source
            .query_row("SELECT count(*) FROM momso_operation_runs", [], |row| {
                row.get(0)
            })
            .expect("source operations read");
        let snapshot_operations: i64 = snapshot
            .query_row("SELECT count(*) FROM momso_operation_runs", [], |row| {
                row.get(0)
            })
            .expect("snapshot operations read");
        assert_eq!(source_count, 1);
        assert_eq!(snapshot_count, 0);
        assert_eq!(source_operations, 1);
        assert_eq!(snapshot_operations, 0);
    }

    #[test]
    fn restores_a_valid_snapshot_and_keeps_a_pre_restore_copy() {
        let temp = tempdir().expect("tempdir");
        let database_path = temp.path().join("momso.db");
        let snapshot_path = temp.path().join("snapshot.db");
        let backups_path = temp.path().join("backups");
        initialize(&database_path, &backups_path, "0.1.0-test").expect("storage initializes");
        create_snapshot_file(&database_path, &snapshot_path).expect("snapshot creates");

        let connection = open_writable_connection(&database_path).expect("database opens");
        connection
            .execute(
                "INSERT INTO app_runtime_meta (key, value, updated_at)
                 VALUES ('after_snapshot', 'newer', CURRENT_TIMESTAMP)",
                [],
            )
            .expect("newer data writes");
        drop(connection);

        let safety_backup = restore_snapshot_file(&snapshot_path, &database_path, &backups_path)
            .expect("snapshot restores");
        let restored = open_connection(&database_path).expect("restored database opens");
        let restored_count: i64 = restored
            .query_row(
                "SELECT count(*) FROM app_runtime_meta WHERE key = 'after_snapshot'",
                [],
                |row| row.get(0),
            )
            .expect("restored state reads");
        assert_eq!(restored_count, 0);
        let safety = open_connection(&safety_backup).expect("safety backup opens");
        let safety_count: i64 = safety
            .query_row(
                "SELECT count(*) FROM app_runtime_meta WHERE key = 'after_snapshot'",
                [],
                |row| row.get(0),
            )
            .expect("safety state reads");
        assert_eq!(safety_count, 1);
    }

    #[test]
    fn rejects_a_database_from_a_newer_schema() {
        let temp = tempdir().expect("tempdir");
        let database_path = temp.path().join("momso.db");
        let backups_path = temp.path().join("backups");
        let connection = Connection::open(&database_path).expect("database opens");
        connection
            .pragma_update(None, "user_version", LATEST_SCHEMA_VERSION + 1)
            .expect("future version is set");
        drop(connection);

        let error = initialize(&database_path, &backups_path, "0.1.0-test")
            .err()
            .expect("future schema is rejected");
        assert!(matches!(error, StorageError::UnsupportedSchema { .. }));

        let connection = Connection::open(&database_path).expect("future database reopens");
        let journal_mode: String = connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("journal mode is readable");
        assert_eq!(journal_mode, "delete");
    }
}
