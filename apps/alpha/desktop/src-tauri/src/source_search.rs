use crate::{storage, AppState};
use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchResult {
    id: String,
    original_file_name: String,
    media_type: Option<String>,
    size: u64,
    created_at: String,
    linked_document_ids: Vec<String>,
}

fn search(conn: &Connection, query: &str) -> rusqlite::Result<Vec<SearchResult>> {
    let needle = query.trim().to_lowercase();
    let mut stmt = conn.prepare("SELECT id, original_file_name, media_type, byte_length, created_at, content_sha256 FROM momso_source_assets WHERE integrity_state = 'active' ORDER BY created_at DESC, id")?;
    let mut rows = stmt.query([])?;
    let mut links = conn.prepare(
        "SELECT DISTINCT zet_id FROM archive_zets_assets WHERE asset_id = ?1 ORDER BY zet_id",
    )?;
    let mut results = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if !name.to_lowercase().contains(&needle) {
            continue;
        }
        let hash: String = row.get(5)?;
        let documents = links
            .query_map([format!("sha256:{hash}")], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        results.push(SearchResult {
            id: row.get(0)?,
            original_file_name: name,
            media_type: row.get(2)?,
            size: {
                let size: i64 = row.get(3)?;
                u64::try_from(size)
                    .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(3, size))?
            },
            created_at: row.get(4)?,
            linked_document_ids: documents,
        });
    }
    Ok(results)
}

#[tauri::command]
pub(crate) async fn source_search(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<SearchResult>, &'static str> {
    let paths = state.paths.scoped().await;
    tauri::async_runtime::spawn_blocking(move || {
        let conn =
            storage::open_writable_connection(&paths.database_path).map_err(|_| "unavailable")?;
        search(&conn, &query).map_err(|_| "unavailable")
    })
    .await
    .map_err(|_| "unavailable")?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_names_and_document_links_without_duplicates() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE momso_source_assets (id TEXT, original_file_name TEXT, media_type TEXT, byte_length INTEGER, created_at TEXT, content_sha256 TEXT, integrity_state TEXT); CREATE TABLE archive_zets_assets (zet_id TEXT, asset_id TEXT); INSERT INTO momso_source_assets VALUES ('id1','요가.MP3','audio/mpeg',12,'2026-01-01','abc','active'),('id2','other',NULL,1,'2026-01-01','def','quarantined'); INSERT INTO archive_zets_assets VALUES ('doc2','sha256:abc'),('doc1','sha256:abc'),('doc1','sha256:abc');").unwrap();
        let results = search(&db, "mp3").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "id1");
        assert_eq!(results[0].linked_document_ids, vec!["doc1", "doc2"]);
        assert_eq!(search(&db, "").unwrap().len(), 1);
        assert!(search(&db, "missing").unwrap().is_empty());
    }
}
