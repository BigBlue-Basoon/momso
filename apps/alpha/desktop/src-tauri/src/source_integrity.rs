use rusqlite::params;
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IntegrityEvent {
    id: i64,
    chat_id: String,
    provider: String,
    created_at: String,
    state: String,
    changes: Vec<serde_json::Value>,
}

pub(crate) fn record(
    database: &Path,
    chat_id: &str,
    provider: &str,
    state: &str,
    changes: &[serde_json::Value],
) -> Result<i64, String> {
    if chat_id.is_empty()
        || !["codex", "claude", "gemini"].contains(&provider)
        || !["changed", "unavailable"].contains(&state)
    {
        return Err("invalid_state".into());
    }
    let conn = crate::storage::open_writable_connection(database).map_err(|_| "storage_error")?;
    let json = serde_json::to_string(changes).map_err(|_| "storage_error")?;
    conn.execute("INSERT INTO source_integrity_events(chat_id,provider,created_at,state,changes_json) VALUES(?1,?2,?3,?4,?5)",
        params![chat_id, provider, chrono::Utc::now().to_rfc3339(), state, json]).map_err(|_| "storage_error")?;
    Ok(conn.last_insert_rowid())
}

pub(crate) fn list(
    database: &Path,
    chat_id: Option<&str>,
    before_id: Option<i64>,
) -> Result<Vec<IntegrityEvent>, String> {
    if chat_id.is_some_and(str::is_empty) || before_id.is_some_and(|id| id <= 0) {
        return Err("invalid_id".into());
    }
    let conn = crate::storage::open_writable_connection(database).map_err(|_| "storage_error")?;
    let mut statement = conn.prepare("SELECT id,chat_id,provider,created_at,state,changes_json FROM source_integrity_events WHERE (?1 IS NULL OR chat_id=?1) AND (?2 IS NULL OR id<?2) ORDER BY id DESC LIMIT 100").map_err(|_| "storage_error")?;
    let rows = statement
        .query_map(params![chat_id, before_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|_| "storage_error")?;
    rows.map(|row| {
        let (id, chat_id, provider, created_at, state, json) = row.map_err(|_| "storage_error")?;
        let changes = serde_json::from_str(&json).map_err(|_| "storage_error")?;
        Ok(IntegrityEvent {
            id,
            chat_id,
            provider,
            created_at,
            state,
            changes,
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn alerts_survive_reopen_and_user_owned_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("state.db");
        let backups = temp.path().join("backups");
        crate::storage::initialize(&database, &backups, "test").unwrap();
        let changes = vec![
            serde_json::json!({"id":"object-path:source.txt","action":"edited","relativePath":"source.txt","beforeSha256":"a".repeat(64)}),
        ];
        let first = record(&database, "chat-one", "codex", "changed", &changes).unwrap();
        record(&database, "chat-two", "claude", "unavailable", &[]).unwrap();
        let items = serde_json::to_value(list(&database, Some("chat-one"), None).unwrap()).unwrap();
        assert_eq!(items[0]["changes"], serde_json::json!(changes));
        assert_eq!(items[0]["provider"], "codex");
        assert_eq!(list(&database, None, Some(first)).unwrap().len(), 0);
        let snapshot = temp.path().join("snapshot.db");
        crate::storage::create_snapshot_file(&database, &snapshot).unwrap();
        crate::storage::sanitize_user_owned_backup_snapshot(&snapshot).unwrap();
        let restored =
            serde_json::to_value(list(&snapshot, Some("chat-one"), None).unwrap()).unwrap();
        assert_eq!(restored[0]["changes"], serde_json::json!(changes));
    }

    #[test]
    fn older_alerts_remain_reachable_with_a_cursor() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("state.db");
        crate::storage::initialize(&database, &temp.path().join("backups"), "test").unwrap();
        for _ in 0..101 {
            record(&database, "chat-one", "gemini", "unavailable", &[]).unwrap();
        }
        let first_page = list(&database, Some("chat-one"), None).unwrap();
        assert_eq!(first_page.len(), 100);
        let second_page = list(
            &database,
            Some("chat-one"),
            Some(first_page.last().unwrap().id),
        )
        .unwrap();
        assert_eq!(second_page.len(), 1);
        assert!(list(&database, Some("chat-other"), None)
            .unwrap()
            .is_empty());
    }
}
