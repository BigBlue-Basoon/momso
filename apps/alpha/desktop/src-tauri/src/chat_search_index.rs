//! Incremental, restartable index construction. Each transaction touches at most one batch.
use rusqlite::{params, Connection, TransactionBehavior};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
pub(crate) static RESTORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn batch(path: &Path, limit: usize) -> rusqlite::Result<bool> {
    let _guard = RESTORE_LOCK
        .lock()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let mut db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(5))?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (cursor, ready): (i64, bool) = tx.query_row(
        "SELECT last_rowid,ready FROM chat_search_index_state WHERE id=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if ready {
        return Ok(true);
    }
    let mut q = tx.prepare("SELECT m.rowid,m.chat_id,m.text,EXISTS(SELECT 1 FROM chat_message_search_map i WHERE i.message_rowid=m.rowid) FROM chat_messages m WHERE m.rowid>?1 ORDER BY m.rowid LIMIT ?2")?;
    let rows = q
        .query_map(params![cursor, limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(q);
    for (rowid, chat_id, text, already) in &rows {
        if !already {
            tx.execute(
                "INSERT INTO chat_message_search(rowid,text) VALUES(?1,?2)",
                params![rowid, text],
            )?;
            tx.execute(
                "INSERT INTO chat_message_search_map(message_rowid,chat_id) VALUES(?1,?2)",
                params![rowid, chat_id],
            )?;
        }
    }
    let done = rows.is_empty();
    let last = rows.last().map_or(cursor, |r| r.0);
    tx.execute(
        "UPDATE chat_search_index_state SET last_rowid=?1,ready=?2 WHERE id=1",
        params![last, done],
    )?;
    tx.commit()?;
    Ok(done)
}

pub(crate) fn start(path: PathBuf) {
    std::thread::spawn(move || loop {
        match batch(&path, 128) {
            Ok(true) => break,
            Ok(false) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_index_resumes_without_losing_concurrent_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.db");
        crate::storage::initialize(&path, &dir.path().join("backup"), "fixture").unwrap();
        let db = Connection::open(&path).unwrap();
        db.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES('room','test','x','x')",
            [],
        )
        .unwrap();
        db.execute_batch("DROP TRIGGER chat_message_search_insert")
            .unwrap();
        db.execute(
            "UPDATE chat_search_index_state SET ready=0,last_rowid=0 WHERE id=1",
            [],
        )
        .unwrap();
        for id in 0..3 {
            db.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES(?1,'room','user','요가아사나를 합니다','x')", [id.to_string()]).unwrap();
        }
        db.execute_batch(include_str!("../migrations/0092_chat_search_deferred.sql"))
            .unwrap();
        assert!(!batch(&path, 1).unwrap());
        db.execute(
            "UPDATE chat_messages SET text='다른아사나를 합니다' WHERE id='1'",
            [],
        )
        .unwrap();
        while !batch(&path, 1).unwrap() {}
        let saved: i64 = db
            .query_row("SELECT count(*) FROM chat_message_search_map", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(saved, 3);
        assert_eq!(crate::chat::search(&path, "다른아사나").unwrap().len(), 1);
    }

    #[test]
    fn ready_index_finds_rare_korean_substrings_after_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.db");
        crate::storage::initialize(&path, &dir.path().join("backup"), "fixture").unwrap();
        let db = Connection::open(&path).unwrap();
        db.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES('room','test','x','x')",
            [],
        )
        .unwrap();
        for i in 0..40 {
            let text = if i == 3 {
                "수리아사나를 합니다"
            } else {
                "일반 수업"
            };
            db.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES(?1,'room','user',?2,'x')", params![i.to_string(),text]).unwrap();
        }
        assert_eq!(crate::chat::search(&path, "아사나를").unwrap().len(), 1);
        db.execute(
            "UPDATE chat_messages SET text='새로운자세입니다' WHERE id='3'",
            [],
        )
        .unwrap();
        assert!(crate::chat::search(&path, "아사나를").unwrap().is_empty());
        assert_eq!(crate::chat::search(&path, "새로운자세").unwrap().len(), 1);
    }

    #[test]
    fn upgrade_defers_existing_messages_and_keeps_search_available() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.db");
        crate::storage::initialize(&path, &dir.path().join("backup"), "fixture").unwrap();
        let db = Connection::open(&path).unwrap();
        db.execute_batch("DROP TABLE generated_output_turns; DROP TABLE media_generation;")
            .unwrap();
        db.execute_batch("DROP TRIGGER chat_message_search_insert; DROP TRIGGER chat_message_search_delete; DROP TRIGGER chat_message_search_move; DROP TRIGGER chat_message_search_update; DROP TABLE chat_message_search_vocab; DROP TABLE chat_message_search; DROP TABLE chat_message_search_map; DROP TABLE chat_search_index_state; DROP INDEX chat_search_page_order; DROP TABLE chat_archive_changes; DROP TRIGGER chat_archive_room_insert; DROP TRIGGER chat_archive_room_update; DROP TRIGGER chat_archive_room_delete; DROP TRIGGER chat_archive_message_insert; DROP TRIGGER chat_archive_message_update; DROP TRIGGER chat_archive_message_delete; DROP TRIGGER chat_archive_import_insert; DROP TRIGGER chat_archive_import_update; DROP TRIGGER chat_archive_import_delete; DROP TRIGGER chat_archive_attachment_insert; DROP TRIGGER chat_archive_attachment_update; DROP TRIGGER chat_archive_attachment_delete; DROP TRIGGER chat_archive_group_update; DELETE FROM app_schema_migrations WHERE version>=89; PRAGMA user_version=88;").unwrap();
        db.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES('room','test','x','x')",
            [],
        )
        .unwrap();
        db.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('message','room','user','수리아사나를 합니다','x')", []).unwrap();
        drop(db);
        crate::storage::initialize(&path, &dir.path().join("backup"), "fixture").unwrap();
        let db = Connection::open(&path).unwrap();
        let (ready, count): (bool,i64) = db.query_row("SELECT ready,(SELECT count(*) FROM chat_message_search_map) FROM chat_search_index_state WHERE id=1", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert!(!ready);
        assert_eq!(count, 0);
        assert_eq!(crate::chat::search(&path, "아사나를").unwrap().len(), 1);
        while !batch(&path, 1).unwrap() {}
        assert_eq!(crate::chat::search(&path, "아사나를").unwrap().len(), 1);
    }
}
