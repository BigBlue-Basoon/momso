//! Encrypted, rotating snapshots of the private class and student database.
use crate::{object_store, object_store_transfer as transfer, runtime::RuntimePaths};
use age::x25519;
use rusqlite::{backup::Backup, Connection, OpenFlags};
use std::{io::Write, path::Path, time::Duration};

pub(crate) const FILE_NAME: &str = "class-student-private.sqlite3";
const SLOT_COUNT: u8 = 7;
const MAGIC: &[u8; 8] = b"MOMSOCR1";

pub(crate) fn snapshot(source: &Path, destination: &Path) -> Result<(), &'static str> {
    let input = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "snapshot_failed")?;
    input
        .busy_timeout(Duration::from_secs(5))
        .map_err(|_| "snapshot_failed")?;
    let mut output = Connection::open(destination).map_err(|_| "snapshot_failed")?;
    {
        let backup = Backup::new(&input, &mut output).map_err(|_| "snapshot_failed")?;
        backup
            .run_to_completion(128, Duration::from_millis(10), None)
            .map_err(|_| "snapshot_failed")?;
    }
    check_integrity(&output)
}

fn check_integrity(db: &Connection) -> Result<(), &'static str> {
    let integrity: String = db
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .map_err(|_| "invalid_snapshot")?;
    if integrity != "ok" {
        return Err("invalid_snapshot");
    }
    Ok(())
}

struct Slot {
    number: u8,
    timestamp: i64,
    hash: String,
}
struct Slots {
    entries: Vec<Slot>,
    latest: Option<(i64, Vec<u8>)>,
}

fn decode_slot(bytes: &[u8], identity: &x25519::Identity) -> Result<(i64, Vec<u8>), &'static str> {
    let plain = transfer::decrypt_bytes(bytes, identity, bytes.len())?;
    if plain.len() < 16 || &plain[..8] != MAGIC {
        return Err("invalid_snapshot");
    }
    let timestamp = i64::from_be_bytes(plain[8..16].try_into().map_err(|_| "invalid_snapshot")?);
    Ok((timestamp, plain[16..].to_vec()))
}

async fn read_slots(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
) -> Result<Slots, &'static str> {
    let mut slots = Slots {
        entries: Vec::new(),
        latest: None,
    };
    for number in 0..SLOT_COUNT {
        let key = format!("{}/class-student/slot-{number}.age", object_store::PREFIX);
        let response =
            object_store::request(config, keys, reqwest::Method::GET, &key, &[], vec![]).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            continue;
        }
        if !response.status().is_success() {
            return Err("remote_read_failed");
        }
        let encrypted = response.bytes().await.map_err(|_| "remote_read_failed")?;
        let (timestamp, data) = decode_slot(&encrypted, identity)?;
        let hash = object_store::digest(&data);
        if slots
            .latest
            .as_ref()
            .is_none_or(|(old, _)| timestamp > *old)
        {
            slots.latest = Some((timestamp, data));
        }
        slots.entries.push(Slot {
            number,
            timestamp,
            hash,
        });
    }
    Ok(slots)
}

pub(crate) async fn backup_inner(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    paths: &RuntimePaths,
) -> Result<(), &'static str> {
    let source = paths.data_root.join(FILE_NAME);
    let metadata = match std::fs::symlink_metadata(&source) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("local_read_failed"),
    };
    if !metadata.is_file() || crate::archive_location::is_link_or_reparse(&metadata) {
        return Err("unsafe_path");
    }
    let slots = read_slots(config, keys, identity).await?;
    let now = chrono::Utc::now().timestamp_millis();
    if slots
        .latest
        .as_ref()
        .is_some_and(|(time, _)| now.saturating_sub(*time) < 86_400_000)
    {
        return Ok(());
    }
    let temp = tempfile::Builder::new()
        .prefix("momso-class-state-")
        .tempdir_in(&paths.backups_path)
        .map_err(|_| "local_write_failed")?;
    let copy = temp.path().join(FILE_NAME);
    snapshot(&source, &copy)?;
    let data = std::fs::read(copy).map_err(|_| "local_read_failed")?;
    if slots.latest.as_ref().is_some_and(|(time, _)| {
        slots
            .entries
            .iter()
            .any(|slot| slot.timestamp == *time && slot.hash == object_store::digest(&data))
    }) {
        return Ok(());
    }
    let number = (0..SLOT_COUNT)
        .find(|n| !slots.entries.iter().any(|slot| slot.number == *n))
        .unwrap_or_else(|| {
            slots
                .entries
                .iter()
                .min_by_key(|slot| slot.timestamp)
                .unwrap()
                .number
        });
    let mut plain = Vec::with_capacity(16 + data.len());
    plain.extend_from_slice(MAGIC);
    plain.extend_from_slice(&now.to_be_bytes());
    plain.extend_from_slice(&data);
    let encrypted = transfer::encrypt_bytes(&plain, identity)?;
    let response =
        object_store::overwrite_class_student_slot(config, keys, number, encrypted).await?;
    if !response.status().is_success() {
        return Err("remote_write_failed");
    }
    Ok(())
}

pub(crate) async fn backup(paths: &RuntimePaths) -> Result<(), &'static str> {
    let _guard = object_store::CONNECTION_LOCK.lock().await;
    let Some(config) = object_store::load_config(&paths.data_root)? else {
        log::info!("class_student_backup not_connected");
        return Ok(());
    };
    let keys = object_store::load_keys(&config)?;
    let identity = transfer::identity(&paths.database_path)?;
    backup_inner(&config, &keys, &identity, paths).await
}

pub(crate) async fn download(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    paths: &RuntimePaths,
) -> Result<Option<tempfile::NamedTempFile>, &'static str> {
    let Some((_, data)) = read_slots(config, keys, identity).await?.latest else {
        return Ok(None);
    };
    let mut file =
        tempfile::NamedTempFile::new_in(&paths.backups_path).map_err(|_| "local_write_failed")?;
    file.write_all(&data).map_err(|_| "local_write_failed")?;
    let db = Connection::open_with_flags(file.path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "invalid_snapshot")?;
    check_integrity(&db)?;
    drop(db);
    Ok(Some(file))
}

fn database_empty(db: &Connection) -> Result<bool, &'static str> {
    let mut query = db
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .map_err(|_| "local_read_failed")?;
    let names = query
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| "local_read_failed")?;
    for name in names {
        let name = name.map_err(|_| "local_read_failed")?.replace('"', "\"\"");
        let has_rows: bool = db
            .query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM \"{name}\" LIMIT 1)"),
                [],
                |row| row.get(0),
            )
            .map_err(|_| "local_read_failed")?;
        if has_rows {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn install(file: tempfile::NamedTempFile, data_root: &Path) -> Result<(), &'static str> {
    let _guard = crate::class_student_store::STORE_LOCK
        .lock()
        .map_err(|_| "unavailable")?;
    file.as_file()
        .sync_all()
        .map_err(|_| "local_write_failed")?;
    let destination = data_root.join(FILE_NAME);
    if !destination.exists() {
        file.persist_noclobber(&destination)
            .map_err(|_| "local_write_failed")?;
        return Ok(());
    }
    let metadata = std::fs::symlink_metadata(&destination).map_err(|_| "local_read_failed")?;
    if !metadata.is_file() || crate::archive_location::is_link_or_reparse(&metadata) {
        return Err("unsafe_path");
    }
    let mut target = Connection::open_with_flags(&destination, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|_| "local_read_failed")?;
    target
        .busy_timeout(Duration::from_secs(5))
        .map_err(|_| "local_read_failed")?;
    if !database_empty(&target)? {
        let received = transfer::hash_file(file.path())?;
        for entry in std::fs::read_dir(data_root).map_err(|_| "local_read_failed")? {
            let entry = entry.map_err(|_| "local_read_failed")?;
            let name = entry.file_name();
            if !name
                .to_string_lossy()
                .starts_with("class-student-recovered-")
            {
                continue;
            }
            let metadata =
                std::fs::symlink_metadata(entry.path()).map_err(|_| "local_read_failed")?;
            if metadata.is_file()
                && !crate::archive_location::is_link_or_reparse(&metadata)
                && transfer::hash_file(&entry.path())? == received
            {
                log::warn!("class_student_restore existing_preserved_copy");
                return Ok(());
            }
        }
        let name = format!("class-student-recovered-{}.sqlite3", uuid::Uuid::new_v4());
        file.persist_noclobber(data_root.join(&name))
            .map_err(|_| "local_write_failed")?;
        log::warn!("class_student_restore preserved_copy={name}");
        return Ok(());
    }
    let input = Connection::open_with_flags(file.path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "invalid_snapshot")?;
    let copy = Backup::new(&input, &mut target).map_err(|_| "restore_failed")?;
    copy.run_to_completion(128, Duration::from_millis(10), None)
        .map_err(|_| "restore_failed")?;
    Ok(())
}

pub(crate) async fn restore_if_missing(
    config: &object_store::Config,
    keys: &object_store::Keys,
    identity: &x25519::Identity,
    paths: &RuntimePaths,
) -> Result<(), &'static str> {
    if let Some(file) = download(config, keys, identity, paths).await? {
        install(file, &paths.data_root)?;
    }
    Ok(())
}

pub(crate) fn start(app: tauri::AppHandle) {
    use tauri::Manager;
    tauri::async_runtime::spawn(async move {
        loop {
            let state = app.state::<crate::AppState>();
            let paths = state.paths.scoped().await;
            match backup(&paths).await {
                Ok(()) => log::info!("class_student_backup checked"),
                Err(reason) => log::warn!("class_student_backup failed reason={reason}"),
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_excludes_uncommitted_writes_and_includes_committed_wal() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join(FILE_NAME);
        let db = Connection::open(&source).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE fixture(value TEXT); INSERT INTO fixture VALUES('first');").unwrap();
        let other = Connection::open(&source).unwrap();
        other
            .execute_batch("BEGIN IMMEDIATE; INSERT INTO fixture VALUES('uncommitted');")
            .unwrap();
        let copy = temp.path().join("copy.sqlite3");
        snapshot(&source, &copy).unwrap();
        let read = Connection::open(copy).unwrap();
        assert_eq!(
            read.query_row("SELECT count(*) FROM fixture", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        other.execute_batch("ROLLBACK;").unwrap();
    }
    #[test]
    fn restore_replaces_empty_and_preserves_nonempty() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join(FILE_NAME);
        Connection::open(&destination)
            .unwrap()
            .execute_batch("CREATE TABLE fixture(value TEXT);")
            .unwrap();
        let create = || {
            let file = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
            Connection::open(file.path())
                .unwrap()
                .execute_batch(
                    "CREATE TABLE fixture(value TEXT); INSERT INTO fixture VALUES('backup');",
                )
                .unwrap();
            file
        };
        install(create(), dir.path()).unwrap();
        let db = Connection::open(&destination).unwrap();
        assert_eq!(
            db.query_row("SELECT value FROM fixture", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "backup"
        );
        db.execute("INSERT INTO fixture VALUES('local')", [])
            .unwrap();
        drop(db);
        install(create(), dir.path()).unwrap();
        let db = Connection::open(&destination).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM fixture", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        let copies: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("class-student-recovered-")
            })
            .collect();
        assert_eq!(copies.len(), 1);
        let recovered = Connection::open(copies[0].path()).unwrap();
        assert_eq!(
            recovered
                .query_row("SELECT value FROM fixture", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "backup"
        );
    }
}
