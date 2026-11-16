use chrono::Local;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use std::{collections::HashSet, path::Path, time::Duration};
use uuid::Uuid;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatGroup {
    id: String,
    name: String,
    order: i64,
    chat_count: i64,
}

fn db(path: &Path) -> Result<Connection, String> {
    let connection =
        Connection::open(path).map_err(|_| "채팅방 묶음을 열지 못했습니다".to_string())?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|_| "채팅방 묶음을 열지 못했습니다".to_string())?;
    connection
        .execute_batch("PRAGMA foreign_keys=ON")
        .map_err(|_| "채팅방 묶음을 열지 못했습니다".to_string())?;
    Ok(connection)
}
fn read_group(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatGroup> {
    Ok(ChatGroup {
        id: row.get(0)?,
        name: row.get(1)?,
        order: row.get(2)?,
        chat_count: row.get(3)?,
    })
}
const GROUP_SELECT: &str = "SELECT g.id,g.name,g.position,(SELECT count(*) FROM chats c WHERE c.group_id=g.id) FROM chat_groups g";
fn one(connection: &Connection, id: &str) -> Result<ChatGroup, String> {
    connection
        .query_row(&format!("{GROUP_SELECT} WHERE g.id=?1"), [id], read_group)
        .optional()
        .map_err(|_| "채팅방 묶음을 읽지 못했습니다".to_string())?
        .ok_or_else(|| "채팅방 묶음을 찾을 수 없습니다".to_string())
}
pub(crate) fn list(path: &Path) -> Result<Vec<ChatGroup>, String> {
    let connection = db(path)?;
    let mut query = connection
        .prepare(&format!("{GROUP_SELECT} ORDER BY g.position,g.rowid"))
        .map_err(|_| "채팅방 묶음을 읽지 못했습니다".to_string())?;
    let rows = query
        .query_map([], read_group)
        .map_err(|_| "채팅방 묶음을 읽지 못했습니다".to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "채팅방 묶음을 읽지 못했습니다".into())
}
pub(crate) fn create(path: &Path, name: &str) -> Result<ChatGroup, String> {
    if name.trim().is_empty() {
        return Err("묶음 이름을 입력해 주세요".into());
    }
    let mut connection = db(path)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| "묶음을 만들지 못했습니다".to_string())?;
    let id = Uuid::new_v4().to_string();
    tx.execute("INSERT INTO chat_groups(id,name,position,created_at) VALUES (?1,?2,(SELECT COALESCE(MAX(position),-1)+1 FROM chat_groups),?3)",params![id,name,Local::now().to_rfc3339()]).map_err(|_| "묶음을 만들지 못했습니다".to_string())?;
    let result = one(&tx, &id)?;
    tx.commit()
        .map_err(|_| "묶음을 만들지 못했습니다".to_string())?;
    Ok(result)
}
pub(crate) fn rename(path: &Path, id: &str, name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("묶음 이름을 입력해 주세요".into());
    }
    let connection = db(path)?;
    let n = connection
        .execute(
            "UPDATE chat_groups SET name=?1 WHERE id=?2",
            params![name, id],
        )
        .map_err(|_| "묶음 이름을 바꾸지 못했습니다".to_string())?;
    if n == 0 {
        return Err("채팅방 묶음을 찾을 수 없습니다".into());
    }
    Ok(())
}
pub(crate) fn reorder(path: &Path, ids: &[String]) -> Result<(), String> {
    let mut connection = db(path)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| "묶음 순서를 바꾸지 못했습니다".to_string())?;
    let mut query = tx
        .prepare("SELECT id FROM chat_groups")
        .map_err(|_| "묶음 순서를 바꾸지 못했습니다".to_string())?;
    let existing: HashSet<String> = query
        .query_map([], |row| row.get(0))
        .map_err(|_| "묶음 순서를 바꾸지 못했습니다".to_string())?
        .collect::<Result<_, _>>()
        .map_err(|_| "묶음 순서를 바꾸지 못했습니다".to_string())?;
    drop(query);
    let received: HashSet<&str> = ids.iter().map(String::as_str).collect();
    if received.len() != ids.len()
        || ids.len() != existing.len()
        || !ids.iter().all(|id| existing.contains(id))
    {
        return Err("묶음 목록이 바뀌었습니다. 다시 확인해 주세요".into());
    }
    for (index, id) in ids.iter().enumerate() {
        tx.execute(
            "UPDATE chat_groups SET position=?1 WHERE id=?2",
            params![index as i64, id],
        )
        .map_err(|_| "묶음 순서를 바꾸지 못했습니다".to_string())?;
    }
    tx.commit()
        .map_err(|_| "묶음 순서를 바꾸지 못했습니다".into())
}
pub(crate) fn remove(path: &Path, id: &str) -> Result<(), String> {
    let connection = db(path)?;
    let n = connection
        .execute("DELETE FROM chat_groups WHERE id=?1", [id])
        .map_err(|_| "묶음을 없애지 못했습니다".to_string())?;
    if n == 0 {
        return Err("채팅방 묶음을 찾을 수 없습니다".into());
    }
    Ok(())
}
pub(crate) fn set_group(
    path: &Path,
    chat_id: &str,
    group_id: Option<String>,
) -> Result<(), String> {
    let mut connection = db(path)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| "채팅방 묶음을 바꾸지 못했습니다".to_string())?;
    let group_id = group_id.filter(|id| !id.is_empty());
    if let Some(id) = &group_id {
        one(&tx, id)?;
    }
    let n = tx
        .execute(
            "UPDATE chats SET group_id=?1 WHERE id=?2",
            params![group_id, chat_id],
        )
        .map_err(|_| "채팅방 묶음을 바꾸지 못했습니다".to_string())?;
    if n == 0 {
        return Err("채팅방을 찾을 수 없습니다".into());
    }
    tx.commit()
        .map_err(|_| "채팅방 묶음을 바꾸지 못했습니다".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn database() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("momso.db");
        crate::storage::initialize(&path, &temp.path().join("backups"), "test").unwrap();
        (temp, path)
    }
    #[test]
    fn creates_renames_and_allows_same_name() {
        let (_temp, path) = database();
        assert!(create(&path, "  ").is_err());
        let a = create(&path, "수련생").unwrap();
        let b = create(&path, "수련생").unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!((a.order, b.order), (0, 1));
        rename(&path, &a.id, "수업").unwrap();
        assert_eq!(list(&path).unwrap()[0].name, "수업");
        assert!(rename(&path, &a.id, " ").is_err());
    }
    #[test]
    fn reorders_all_and_rejects_bad_lists_without_changes() {
        let (_temp, path) = database();
        let a = create(&path, "가").unwrap();
        let b = create(&path, "나").unwrap();
        reorder(&path, &[b.id.clone(), a.id.clone()]).unwrap();
        assert_eq!(
            list(&path)
                .unwrap()
                .iter()
                .map(|g| g.id.as_str())
                .collect::<Vec<_>>(),
            vec![b.id.as_str(), a.id.as_str()]
        );
        assert!(reorder(&path, &[a.id.clone(), a.id.clone()]).is_err());
        assert!(reorder(&path, std::slice::from_ref(&a.id)).is_err());
        assert_eq!(list(&path).unwrap()[0].id, b.id);
    }
    #[test]
    fn assigns_unassigns_and_lists_counts() {
        let (_temp, path) = database();
        let g = create(&path, "묶음").unwrap();
        let chat = crate::chat::create(&path).unwrap();
        let chat_id = serde_json::to_value(chat).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        set_group(&path, &chat_id, Some(g.id.clone())).unwrap();
        assert_eq!(list(&path).unwrap()[0].chat_count, 1);
        let value = serde_json::to_value(crate::chat::list(&path).unwrap()).unwrap();
        assert_eq!(value[0]["groupId"], g.id);
        let found = serde_json::to_value(crate::chat::search(&path, "").unwrap()).unwrap();
        assert_eq!(found[0]["groupId"], g.id);
        set_group(&path, &chat_id, None).unwrap();
        assert_eq!(list(&path).unwrap()[0].chat_count, 0);
        set_group(&path, &chat_id, Some(String::new())).unwrap();
        assert!(
            serde_json::to_value(crate::chat::list(&path).unwrap()).unwrap()[0]["groupId"]
                .is_null()
        );
    }
    #[test]
    fn removing_group_keeps_chats_and_messages() {
        let (_temp, path) = database();
        let g = create(&path, "묶음").unwrap();
        let chat = serde_json::to_value(crate::chat::create(&path).unwrap()).unwrap();
        let id = chat["id"].as_str().unwrap();
        crate::chat::append(&path, id, crate::chat::Role::User, "안녕하세요", None, None).unwrap();
        set_group(&path, id, Some(g.id.clone())).unwrap();
        remove(&path, &g.id).unwrap();
        let chats = serde_json::to_value(crate::chat::list(&path).unwrap()).unwrap();
        assert!(chats[0]["groupId"].is_null());
        assert_eq!(chats[0]["messageCount"], 1);
        assert!(list(&path).unwrap().is_empty());
    }
    #[test]
    fn missing_ids_report_errors() {
        let (_temp, path) = database();
        assert!(rename(&path, "none", "이름").is_err());
        assert!(remove(&path, "none").is_err());
        assert!(set_group(&path, "missing", None).is_err());
        let chat = serde_json::to_value(crate::chat::create(&path).unwrap()).unwrap();
        assert!(set_group(&path, chat["id"].as_str().unwrap(), Some("missing".into())).is_err());
    }
}
