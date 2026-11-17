//! Durable chat change generations and a keyed proof of the redaction inputs.
//! The proof contains no credential values and is scoped to the archive path.
use hmac::{Hmac, Mac};
use rusqlite::{Connection, OptionalExtension};
use sha2::Sha256;
use std::path::Path;

const PROOF_NAME: &str = "chat-archive:redaction-proof";
const PROOF_META: &str = "chat_archive_redaction_proof_v1";

#[cfg(not(test))]
pub(crate) fn proof_key() -> Result<String, &'static str> {
    if let Some(value) =
        crate::credentials::get_secret(PROOF_NAME).map_err(|_| "credentials_unavailable")?
    {
        if value.len() < 64 {
            return Err("credentials_unavailable");
        }
        return Ok(value);
    }
    let candidate = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    crate::credentials::set_secret(PROOF_NAME, &candidate)
        .map_err(|_| "credentials_unavailable")?;
    crate::credentials::get_secret(PROOF_NAME)
        .map_err(|_| "credentials_unavailable")?
        .filter(|s| s.len() >= 64)
        .ok_or("credentials_unavailable")
}

#[cfg(test)]
pub(crate) fn proof_key() -> Result<String, &'static str> {
    let _ = PROOF_NAME;
    Ok("fixture-proof-key-only-used-for-isolated-tests".to_owned())
}

fn proof_with_key(root: &Path, secrets: &[String], key: &[u8]) -> Result<String, &'static str> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| "credentials_unavailable")?;
    let path = root.to_string_lossy();
    mac.update(&(path.len() as u64).to_be_bytes());
    mac.update(path.as_bytes());
    mac.update(&(secrets.len() as u64).to_be_bytes());
    for value in secrets {
        mac.update(&(value.len() as u64).to_be_bytes());
        mac.update(value.as_bytes());
    }
    Ok(format!("{:x}", mac.finalize().into_bytes()))
}

pub(crate) fn proof(root: &Path, secrets: &[String], key: &str) -> Result<String, &'static str> {
    proof_with_key(root, secrets, key.as_bytes())
}

pub(crate) fn text_fingerprint(text: &str) -> String {
    use sha2::Digest;
    let mut hash = Sha256::new();
    hash.update(b"momso-preserved-chat-field-v1\0");
    hash.update(text.as_bytes());
    format!("{:x}", hash.finalize())
}

pub(crate) fn stored_proof(db: &Path) -> Result<Option<String>, &'static str> {
    let conn = Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "storage_error")?;
    conn.query_row(
        "SELECT value FROM app_runtime_meta WHERE key=?1",
        [PROOF_META],
        |r| r.get(0),
    )
    .optional()
    .map_err(|_| "storage_error")
}

pub(crate) fn changes(db: &Path) -> Result<Vec<(String, i64)>, &'static str> {
    let conn = Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "storage_error")?;
    let mut statement = conn
        .prepare("SELECT chat_id,revision FROM chat_archive_changes ORDER BY chat_id")
        .map_err(|_| "storage_error")?;
    let rows = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|_| "storage_error")?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "storage_error")
}

pub(crate) fn ensure_tracking(db: &Path) -> Result<bool, &'static str> {
    let mut conn = crate::storage::open_writable_connection(db).map_err(|_| "storage_error")?;
    let intact: bool = conn.query_row("SELECT (SELECT count(*) FROM sqlite_master WHERE type='table' AND name='chat_archive_changes')=1 AND (SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name IN ('chat_archive_room_insert','chat_archive_room_update','chat_archive_room_delete','chat_archive_message_insert','chat_archive_message_update','chat_archive_message_delete','chat_archive_import_insert','chat_archive_import_update','chat_archive_import_delete','chat_archive_attachment_insert','chat_archive_attachment_update','chat_archive_attachment_delete','chat_archive_group_update'))=13 AND EXISTS(SELECT 1 FROM app_runtime_meta WHERE key='chat_archive_tracking_version' AND value='1')", [], |r| r.get(0)).map_err(|_| "storage_error")?;
    if intact {
        return Ok(false);
    }
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| "storage_error")?;
    tx.execute_batch(include_str!("../migrations/0090_chat_archive_changes.sql"))
        .map_err(|_| "storage_error")?;
    tx.execute("DELETE FROM app_runtime_meta WHERE key=?1", [PROOF_META])
        .map_err(|_| "storage_error")?;
    tx.commit().map_err(|_| "storage_error")?;
    Ok(true)
}

pub(crate) fn requires_full_retry(db: &Path) -> Result<bool, &'static str> {
    let conn = Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "storage_error")?;
    conn.query_row("SELECT EXISTS(SELECT 1 FROM app_runtime_meta m WHERE key='chat_archive_write:all' OR (key GLOB 'chat_archive_delete:*' AND NOT EXISTS (SELECT 1 FROM chat_archive_changes d WHERE d.chat_id=substr(m.key,21))) OR (key GLOB 'chat_archive_write:*' AND key!='chat_archive_write:all' AND NOT EXISTS (SELECT 1 FROM chat_archive_changes d WHERE d.chat_id=substr(m.key,20))))", [], |r| r.get(0)).map_err(|_| "storage_error")
}

pub(crate) fn finish(
    db: &Path,
    entries: &[(String, i64)],
    proof: &str,
) -> Result<(), &'static str> {
    let mut conn = crate::storage::open_writable_connection(db).map_err(|_| "storage_error")?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| "storage_error")?;
    for (id, revision) in entries {
        tx.execute(
            "DELETE FROM chat_archive_changes WHERE chat_id=?1 AND revision=?2",
            rusqlite::params![id, revision],
        )
        .map_err(|_| "storage_error")?;
        tx.execute("DELETE FROM app_runtime_meta WHERE key=?1 AND NOT EXISTS (SELECT 1 FROM chat_archive_changes WHERE chat_id=?2)", rusqlite::params![format!("chat_archive_write:{id}"),id]).map_err(|_| "storage_error")?;
    }
    tx.execute("INSERT INTO app_runtime_meta(key,value,updated_at) VALUES(?1,?2,CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=CURRENT_TIMESTAMP", rusqlite::params![PROOF_META,proof]).map_err(|_| "storage_error")?;
    tx.commit().map_err(|_| "storage_error")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proof_changes_with_credentials_and_archive_without_disclosing_them() {
        let secrets = vec!["fixture-private-value".to_owned()];
        let a = proof_with_key(Path::new("first"), &secrets, b"fixture-key").unwrap();
        let b = proof_with_key(Path::new("second"), &secrets, b"fixture-key").unwrap();
        let c = proof_with_key(Path::new("first"), &["other".into()], b"fixture-key").unwrap();
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert!(!a.contains(&secrets[0]));
    }

    #[test]
    fn changes_survive_reopen_and_new_revision_is_not_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("momso.db");
        crate::storage::initialize(&db, &dir.path().join("backups"), "test").unwrap();
        let room = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
        let id = room["id"].as_str().unwrap();
        let captured = changes(&db).unwrap();
        assert_eq!(captured.len(), 1);
        crate::chat::append(&db, id, crate::chat::Role::User, "fixture", None, None).unwrap();
        finish(&db, &captured, "fixture-proof").unwrap();
        let later = changes(&db).unwrap();
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].0, id);
        assert!(later[0].1 > captured[0].1);
        finish(&db, &later, "fixture-proof").unwrap();
        assert!(changes(&db).unwrap().is_empty());
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("DROP TRIGGER chat_archive_message_update; DELETE FROM app_runtime_meta WHERE key='chat_archive_tracking_version';").unwrap();
        drop(conn);
        assert!(ensure_tracking(&db).unwrap());
        assert_eq!(changes(&db).unwrap().len(), 1);
        assert!(stored_proof(&db).unwrap().is_none());
        assert!(!ensure_tracking(&db).unwrap());
    }
}
