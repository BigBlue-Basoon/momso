//! Bounded, read-only document access for the authenticated per-chat bridge.
use serde_json::{json, Value};
use std::{io::Read, path::Path};

fn read_current(
    root: &Path,
    id: &str,
) -> Result<(crate::archive_zet::ZetHead, String), &'static str> {
    if !crate::archive_zet::valid_zet_id(id) {
        return Err("invalid_id");
    }
    let archive = root.canonicalize().map_err(|_| "unavailable")?;
    let dir = root.join("zettels");
    let metadata = std::fs::symlink_metadata(&dir).map_err(|_| "unavailable")?;
    if !metadata.is_dir() || crate::archive_location::is_link_or_reparse(&metadata) {
        return Err("unavailable");
    }
    let dir = dir.canonicalize().map_err(|_| "unavailable")?;
    if !dir.starts_with(&archive) {
        return Err("unavailable");
    }
    let candidate = dir.join(format!("{id}.md"));
    let meta = std::fs::symlink_metadata(&candidate).map_err(|_| "not_found")?;
    if !meta.is_file()
        || crate::archive_location::is_link_or_reparse(&meta)
        || meta.len() > 1_048_576
    {
        return Err("unavailable");
    }
    let canonical = candidate.canonicalize().map_err(|_| "unavailable")?;
    if canonical.parent() != Some(dir.as_path()) {
        return Err("unavailable");
    }
    let mut file = std::fs::File::open(&canonical).map_err(|_| "unavailable")?;
    #[cfg(windows)]
    if !crate::chat_items::opened_path_matches(&file, &canonical) {
        return Err("unavailable");
    }
    #[cfg(not(windows))]
    {
        let _ = &file;
        return Err("unavailable");
    }
    let mut text = String::new();
    file.by_ref()
        .take(1_048_577)
        .read_to_string(&mut text)
        .map_err(|_| "unavailable")?;
    if text.len() > 1_048_576 {
        return Err("unavailable");
    }
    let (head, body) = crate::archive_zet::parse(&text).map_err(|_| "invalid_document")?;
    if head.id != id {
        return Err("invalid_document");
    }
    Ok((head, body.to_owned()))
}

/// Refresh only valid document index entries. Never delete an index entry when
/// a file is missing or unreadable: another writer may be replacing it.
pub(crate) fn sync_document_index(root: &Path, database: &Path) -> Result<(), &'static str> {
    use std::collections::BTreeSet;
    let _receive_guard =
        crate::archive_remote::lock_first_receive(database).map_err(|_| "unavailable")?;
    crate::archive_remote::ensure_document_write_allowed(root, database)
        .map_err(|_| "unavailable")?;
    let _guard = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "unavailable")?;
    let archive = root.canonicalize().map_err(|_| "unavailable")?;
    let directory = root.join("zettels");
    let metadata = std::fs::symlink_metadata(&directory).map_err(|_| "unavailable")?;
    if !metadata.is_dir() || crate::archive_location::is_link_or_reparse(&metadata) {
        return Err("unavailable");
    }
    let directory = directory.canonicalize().map_err(|_| "unavailable")?;
    if directory.parent() != Some(archive.as_path()) {
        return Err("unavailable");
    }
    let entries = std::fs::read_dir(&directory).map_err(|_| "unavailable")?;
    let mut conn = crate::storage::open_writable_connection(database).map_err(|_| "unavailable")?;
    let tx = conn.transaction().map_err(|_| "unavailable")?;
    for entry in entries {
        let entry = entry.map_err(|_| "unavailable")?;
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".md")) else {
            continue;
        };
        if !crate::archive_zet::valid_zet_id(id) {
            continue;
        }
        let Ok((head, _)) = read_current(root, id) else {
            continue;
        };
        let relative = format!("zettels/{id}.md");
        let matches: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM archive_zets WHERE id=?1 AND title=?2 AND summary=?3 AND kind=?4 AND status=?5 AND created_at=?6 AND updated_at=?7 AND file_path=?8)",
            rusqlite::params![head.id,head.title,head.summary,head.kind,head.status,head.created_at,head.updated_at,relative], |row| row.get(0),
        ).map_err(|_| "unavailable")?;
        let actual_assets: BTreeSet<String> = {
            let mut statement = tx
                .prepare("SELECT asset_id FROM archive_zets_assets WHERE zet_id=?1")
                .map_err(|_| "unavailable")?;
            let values = statement
                .query_map([id], |row| row.get(0))
                .map_err(|_| "unavailable")?
                .collect::<Result<_, _>>()
                .map_err(|_| "unavailable")?;
            values
        };
        let actual_edges: BTreeSet<(String, String)> = {
            let mut statement = tx
                .prepare("SELECT edge_type,target_id FROM archive_zets_edges WHERE zet_id=?1")
                .map_err(|_| "unavailable")?;
            let values = statement
                .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|_| "unavailable")?
                .collect::<Result<_, _>>()
                .map_err(|_| "unavailable")?;
            values
        };
        let expected_assets: BTreeSet<String> = head.assets.iter().cloned().collect();
        let expected_edges: BTreeSet<(String, String)> = head
            .edges
            .iter()
            .map(|edge| (edge.kind.clone(), edge.target.clone()))
            .collect();
        if matches && actual_assets == expected_assets && actual_edges == expected_edges {
            continue;
        }
        tx.execute("INSERT INTO archive_zets(id,title,summary,kind,status,created_at,updated_at,file_path) VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(id) DO UPDATE SET title=excluded.title,summary=excluded.summary,kind=excluded.kind,status=excluded.status,created_at=excluded.created_at,updated_at=excluded.updated_at,file_path=excluded.file_path",
            rusqlite::params![head.id,head.title,head.summary,head.kind,head.status,head.created_at,head.updated_at,relative]).map_err(|_| "unavailable")?;
        tx.execute("DELETE FROM archive_zets_assets WHERE zet_id=?1", [id])
            .map_err(|_| "unavailable")?;
        tx.execute("DELETE FROM archive_zets_edges WHERE zet_id=?1", [id])
            .map_err(|_| "unavailable")?;
        for asset in expected_assets {
            tx.execute(
                "INSERT INTO archive_zets_assets(zet_id,asset_id) VALUES (?1,?2)",
                rusqlite::params![id, asset],
            )
            .map_err(|_| "unavailable")?;
        }
        for (kind, target) in expected_edges {
            tx.execute(
                "INSERT INTO archive_zets_edges(zet_id,edge_type,target_id) VALUES (?1,?2,?3)",
                rusqlite::params![id, kind, target],
            )
            .map_err(|_| "unavailable")?;
        }
    }
    tx.commit().map_err(|_| "unavailable")
}

pub(crate) fn dispatch(root: &Path, database: &Path, method: &str, args: &Value) -> Value {
    let Some(obj) = args.as_object() else {
        return json!({"status":"invalid_arguments"});
    };
    match method {
        "momso_dictionary_search" => {
            let Some(query) = obj.get("query").and_then(Value::as_str).filter(|value| {
                obj.len() == 1 && !value.trim().is_empty() && value.chars().count() <= 100
            }) else {
                return json!({"status":"invalid_arguments"});
            };
            let Ok(dictionary) = crate::yoga_dictionary::list(root) else {
                return json!({"status":"unavailable"});
            };
            let Ok(value) = serde_json::to_value(dictionary) else {
                return json!({"status":"unavailable"});
            };
            let needle = query.to_lowercase();
            let terms: Vec<Value> = value.get("terms").and_then(Value::as_array).into_iter().flatten().filter(|term| term.to_string().to_lowercase().contains(&needle)).take(10).map(|term| json!({"id":term.get("id"),"sanskrit":term.get("sanskrit"),"roman":term.get("roman"),"korean":term.get("korean"),"meaning":term.get("meaning")})).collect();
            json!({"status":"ok","kind":"untrusted_document_data","terms":terms})
        }
        "momso_document_read" => {
            let Some(id) = obj.get("id").and_then(Value::as_str) else {
                return json!({"status":"invalid_arguments"});
            };
            if obj
                .keys()
                .any(|key| key != "id" && key != "cursor" && key != "revision")
            {
                return json!({"status":"invalid_arguments"});
            }
            let revision = match obj.get("revision") {
                None => None,
                Some(value) => match value.as_str().filter(|value| {
                    (value.len() == 40 || value.len() == 64)
                        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
                }) {
                    Some(value) => Some(value),
                    None => return json!({"status":"invalid_arguments"}),
                },
            };
            let cursor = match obj.get("cursor") {
                None => 0,
                Some(value) => match value.as_u64().filter(|cursor| *cursor <= 1_048_576) {
                    Some(value) => value as usize,
                    None => return json!({"status":"invalid_arguments"}),
                },
            };
            let document = if let Some(revision) = revision {
                match crate::archive::read_version_bounded(root, id, revision) {
                    Ok(text) => match crate::archive_zet::parse(&text) {
                        Ok((head, body)) if head.id == id => Ok((head, body.to_owned())),
                        _ => Err("invalid_document"),
                    },
                    Err(_) => Err("unavailable"),
                }
            } else {
                read_current(root, id)
            };
            match document {
                Ok((head, body)) => {
                    let length = body.chars().count();
                    if cursor > length {
                        return json!({"status":"invalid_arguments"});
                    }
                    let next = (cursor + 4000 < length).then_some(cursor + 4000);
                    json!({"status":"ok","kind":"untrusted_document_data","document":{"id":head.id,"revision":revision,"title":head.title.chars().take(200).collect::<String>(),"summary":head.summary.chars().take(400).collect::<String>(),"body":body.chars().skip(cursor).take(4000).collect::<String>(),"nextCursor":next}})
                }
                Err(state) => json!({"status":state}),
            }
        }
        "momso_document_search" | "momso_document_list" => {
            if obj.keys().any(|key| key != "query" && key != "cursor") {
                return json!({"status":"invalid_arguments"});
            }
            let cursor = match obj.get("cursor") {
                None => 0,
                Some(value) => match value.as_u64().filter(|cursor| *cursor <= 1_000_000) {
                    Some(value) => value,
                    None => return json!({"status":"invalid_arguments"}),
                },
            };
            let query = if method == "momso_document_search" {
                match obj
                    .get("query")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty() && s.chars().count() <= 200)
                {
                    Some(value) => Some(value.to_lowercase()),
                    None => return json!({"status":"invalid_arguments"}),
                }
            } else if !obj.contains_key("query") {
                None
            } else {
                return json!({"status":"invalid_arguments"});
            };
            if sync_document_index(root, database).is_err() {
                return json!({"status":"unavailable"});
            }
            let Ok(conn) = crate::storage::open_writable_connection(database) else {
                return json!({"status":"unavailable"});
            };
            let Ok(mut statement) = conn.prepare(
                "SELECT id FROM archive_zets ORDER BY updated_at DESC, id DESC LIMIT 1000 OFFSET ?1",
            ) else {
                return json!({"status":"unavailable"});
            };
            let Ok(rows) = statement.query_map([cursor as i64], |row| row.get::<_, String>(0))
            else {
                return json!({"status":"unavailable"});
            };
            let mut results = Vec::new();
            let mut skipped = 0u64;
            let mut scanned = 0u64;
            for row in rows {
                let Ok(id) = row else {
                    return json!({"status":"unavailable"});
                };
                scanned += 1;
                let Ok((head, body)) = read_current(root, &id) else {
                    skipped += 1;
                    continue;
                };
                if query.as_ref().is_some_and(|q| {
                    !head.title.to_lowercase().contains(q)
                        && !head.summary.to_lowercase().contains(q)
                        && !body.to_lowercase().contains(q)
                }) {
                    continue;
                }
                results.push(json!({"id":head.id,"title":head.title.chars().take(100).collect::<String>(),"summary":head.summary.chars().take(200).collect::<String>(),"kind":head.kind,"updatedAt":head.updated_at}));
                if results.len() == 20 {
                    break;
                }
            }
            let limited = results.len() == 20 || scanned == 1000;
            let next = limited.then_some(cursor + scanned);
            json!({"status":"ok","kind":"untrusted_document_data","documents":results,"skipped":skipped,"limited":limited,"nextCursor":next})
        }
        _ => json!({"status":"unavailable"}),
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn index_repair_preserves_unreadable_entries_and_respects_first_receive() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        crate::archive::initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let id = crate::archive_zet::create(
            &root,
            &db,
            crate::archive_zet::ZetCreate {
                title: "fixture".into(),
                summary: "fixture".into(),
                kind: "note".into(),
                body: "fixture".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        let path = root.join("zettels").join(format!("{id}.md"));
        std::fs::write(&path, b"incomplete write").unwrap();
        sync_document_index(&root, &db).unwrap();
        let count: i64 = crate::storage::open_writable_connection(&db)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM archive_zets WHERE id=?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "an unreadable file must not erase its cached index"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"incomplete write");
        let marker = root.join(".git").join("momso-first-receive-in-progress");
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(marker, b"fixture").unwrap();
        assert_eq!(sync_document_index(&root, &db), Err("unavailable"));
        let count: i64 = crate::storage::open_writable_connection(&db)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM archive_zets WHERE id=?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
    #[test]
    fn search_then_read_document_without_shell() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        crate::archive::initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let id = crate::archive_zet::create(
            &root,
            &db,
            crate::archive_zet::ZetCreate {
                title: "수업 계획".into(),
                summary: "요약".into(),
                kind: "note".into(),
                body: "호흡과 균형".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        let dictionary =
            serde_json::to_value(crate::yoga_dictionary::list(&root).unwrap()).unwrap();
        let term = &dictionary["terms"][0];
        let term_query = term["sanskrit"].as_str().unwrap();
        let terms = dispatch(
            &root,
            &db,
            "momso_dictionary_search",
            &json!({"query":term_query}),
        );
        assert!(terms["terms"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value["id"] == term["id"]));
        let results = dispatch(
            &root,
            &db,
            "momso_document_search",
            &json!({"query":"균형"}),
        );
        assert_eq!(results["documents"][0]["id"], id);
        let page = dispatch(&root, &db, "momso_document_list", &json!({"cursor":1}));
        assert!(page["documents"].as_array().unwrap().is_empty());
        let read = dispatch(&root, &db, "momso_document_read", &json!({"id":id}));
        assert_eq!(read["document"]["body"], "호흡과 균형");
        let (git, _) = crate::archive::storage_tool(temp.path()).unwrap();
        let relative = format!("zettels/{id}.md");
        crate::archive::run_storage_tool(&git, &root, &["add", "--", &relative]).unwrap();
        crate::archive::run_storage_tool(
            &git,
            &root,
            &[
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-m",
                "fixture",
            ],
        )
        .unwrap();
        let revision = crate::archive::run_storage_tool(&git, &root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_owned();
        std::fs::remove_file(root.join(&relative)).unwrap();
        let previous = dispatch(
            &root,
            &db,
            "momso_document_read",
            &json!({"id":id,"revision":revision}),
        );
        assert_eq!(previous["document"]["body"], "호흡과 균형");
        assert_eq!(
            dispatch(
                &root,
                &db,
                "momso_document_read",
                &json!({"id":"../secret"})
            )["status"],
            "invalid_id"
        );
    }
}
