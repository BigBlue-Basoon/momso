use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use std::{fs, path::Path};
use tauri::{Manager, State};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaItem {
    pub(crate) id: String,
    pub(crate) kind: &'static str,
    pub(crate) title: Option<String>,
    pub(crate) created_at: String,
    width: Option<u32>,
    height: Option<u32>,
    duration_sec: Option<f64>,
    pub(crate) path: String,
    poster_path: Option<String>,
    chat_id: Option<String>,
    pub(crate) by: Option<crate::archive_receipt::ChatItemBy>,
    pub(crate) evidence: Option<String>,
    pub(crate) how: Option<&'static str>,
    pub(crate) at: Option<String>,
    pub(crate) chats: Vec<crate::chat_items::ChatLink>,
    asset_id: String,
    prompt: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    generated_at: Option<String>,
    message_id: Option<String>,
    version_group_id: Option<String>,
    version: Option<i64>,
    parent_asset_id: Option<String>,
    lineage_id: Option<String>,
    version_of: Option<String>,
    version_how: Option<String>,
    version_count: usize,
}

#[cfg(test)]
use crate::media_format::classify;
pub(crate) use crate::media_format::classify_file;

pub(crate) fn collect(
    root: &Path,
    database: &Path,
    kind: Option<&str>,
    before: Option<&str>,
    limit: usize,
) -> Result<Vec<MediaItem>, String> {
    if kind.is_some_and(|k| k != "image" && k != "video" && k != "audio") {
        return Err("invalid_kind".into());
    }
    let cursor = before
        .map(|value| {
            let (time, id) = value
                .split_once('|')
                .map_or((value, None), |(t, i)| (t, Some(i)));
            DateTime::parse_from_rfc3339(time)
                .map(|t| (t.with_timezone(&Utc), id))
                .map_err(|_| "invalid_before".to_string())
        })
        .transpose()?;
    let root_metadata = match fs::symlink_metadata(root) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("unavailable".into()),
    };
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err("unavailable".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if root_metadata.file_attributes() & 0x400 != 0 {
            return Err("unavailable".into());
        }
    }
    let root = root.canonicalize().map_err(|_| "unavailable".to_string())?;
    let db = if database.exists() {
        Some(
            Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|_| "unavailable".to_string())?,
        )
    } else {
        None
    };
    let mut items = Vec::new();
    for entry in WalkDir::new(&root)
        .follow_links(false)
        .min_depth(1)
        .into_iter()
    {
        let entry = entry.map_err(|_| "unavailable".to_string())?;
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(&root) else {
            continue;
        };
        let parts: Vec<_> = relative.components().collect();
        if parts.len() != 3 || parts[0].as_os_str() != "sha256" {
            continue;
        }
        let Some(hash) = parts[2].as_os_str().to_str() else {
            continue;
        };
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || parts[1].as_os_str() != &hash[..2]
        {
            continue;
        }
        let Ok(path) = entry.path().canonicalize() else {
            continue;
        };
        if !path.starts_with(&root) {
            continue;
        }
        let info: Option<(String, String, String, bool)> = match &db {
            Some(conn) => conn.query_row(
                "SELECT a.original_file_name, a.created_at, a.integrity_state, EXISTS(SELECT 1 FROM source_asset_deletions d WHERE d.asset_id=a.id) FROM momso_source_assets a WHERE a.content_sha256 = ?1 ORDER BY a.created_at, a.id LIMIT 1",
                [hash], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .optional().map_err(|_| "unavailable".to_string())?,
            None => None,
        };
        if info
            .as_ref()
            .is_some_and(|(_, _, state, deleted)| state != "active" || *deleted)
        {
            continue;
        }
        let mut file = fs::File::open(&path).map_err(|_| "unavailable".to_string())?;
        let classification = classify_file(&mut file)?;
        let Some((detected, dimensions)) = classification else {
            continue;
        };
        if kind.is_some_and(|k| k != detected) {
            continue;
        }
        let metadata = file.metadata().map_err(|_| "unavailable".to_string())?;
        let created: DateTime<Utc> = match metadata.created() {
            Ok(time) => time.into(),
            Err(_) => continue,
        };
        let timestamp = info
            .as_ref()
            .and_then(|(_, date, _, _)| {
                DateTime::parse_from_rfc3339(date)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            })
            .unwrap_or(created);
        let id = format!("sha256:{hash}");
        if let Some((time, tie_id)) = cursor {
            if timestamp > time || (timestamp == time && tie_id.is_none_or(|t| id.as_str() >= t)) {
                continue;
            }
        }
        let (width, height) = dimensions.map_or((None, None), |(w, h)| (Some(w), Some(h)));
        items.push((
            timestamp,
            MediaItem {
                asset_id: id.clone(),
                id,
                kind: detected,
                title: info.map(|x| x.0),
                created_at: timestamp.to_rfc3339(),
                width,
                height,
                duration_sec: None,
                path: path.to_string_lossy().into_owned(),
                poster_path: None,
                chat_id: None,
                by: None,
                evidence: None,
                how: None,
                at: None,
                chats: Vec::new(),
                prompt: None,
                provider: None,
                model: None,
                generated_at: None,
                message_id: None,
                version_group_id: None,
                version: None,
                parent_asset_id: None,
                lineage_id: None,
                version_of: None,
                version_how: None,
                version_count: 1,
            },
        ));
    }
    items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.id.cmp(&a.1.id)));
    Ok(items
        .into_iter()
        .take(limit.max(1))
        .map(|(_, item)| item)
        .collect())
}

pub(crate) fn gallery(
    worker_root: &Path,
    database: &Path,
    kind: Option<&str>,
    chat_id: Option<&str>,
    before: Option<&str>,
    limit: usize,
) -> Result<Vec<MediaItem>, String> {
    let mut items = collect(
        &worker_root.join("objects"),
        database,
        kind,
        None,
        usize::MAX,
    )?;
    crate::chat_items::enrich_media(worker_root, database, &mut items)?;
    let generations = crate::media_generation::all(database)?;
    let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "unavailable")?;
    let mut query = connection
        .prepare("SELECT id, title FROM chats")
        .map_err(|_| "unavailable")?;
    let chat_titles: std::collections::HashMap<String, String> = query
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|_| "unavailable")?
        .collect::<Result<_, _>>()
        .map_err(|_| "unavailable")?;
    let available: std::collections::HashSet<_> =
        items.iter().map(|item| item.id.as_str()).collect();
    let mut counts = std::collections::HashMap::new();
    let mut latest = std::collections::HashMap::new();
    for generation in generations
        .iter()
        .filter(|g| available.contains(g.asset_id.as_str()))
    {
        *counts
            .entry(generation.version_group_id.clone())
            .or_insert(0_usize) += 1;
        let current = latest
            .entry(generation.version_group_id.clone())
            .or_insert(0_i64);
        *current = (*current).max(generation.version);
    }
    let by_asset: std::collections::HashMap<_, _> = generations
        .iter()
        .map(|g| (g.asset_id.as_str(), g))
        .collect();
    let by_version: std::collections::HashMap<_, _> = generations
        .iter()
        .map(|g| {
            (
                (g.version_group_id.as_str(), g.version),
                g.asset_id.as_str(),
            )
        })
        .collect();
    items.retain_mut(|item| {
        if let Some(g) = by_asset.get(item.id.as_str()) {
            if latest.get(&g.version_group_id) != Some(&g.version) {
                return false;
            }
            item.prompt = Some(g.prompt.clone());
            item.provider = Some(g.provider.clone());
            item.model = g.model.clone();
            item.generated_at = g.created_at.clone();
            if let Some(title) = chat_titles.get(&g.chat_id) {
                item.chat_id = Some(g.chat_id.clone());
                item.chats.retain(|link| link.id != g.chat_id);
                item.chats.insert(
                    0,
                    crate::chat_items::ChatLink {
                        id: g.chat_id.clone(),
                        title: Some(title.clone()),
                    },
                );
            } else {
                item.chat_id = None;
                item.chats.clear();
            }
            item.message_id = g.message_id.clone();
            item.version_group_id = Some(g.version_group_id.clone());
            item.version = Some(g.version);
            item.parent_asset_id = g.parent_asset_id.clone();
            item.lineage_id = Some(g.version_group_id.clone());
            if g.version > 1 {
                item.version_of = g.parent_asset_id.clone().or_else(|| {
                    by_version
                        .get(&(g.version_group_id.as_str(), g.version - 1))
                        .map(|id| (*id).to_owned())
                });
                item.version_how = Some(g.version_how.clone().unwrap_or_else(|| {
                    if g.parent_asset_id.is_some() {
                        "edited".into()
                    } else {
                        "regenerated".into()
                    }
                }));
            }
            item.version_count = counts[&g.version_group_id];
        }
        chat_id.is_none_or(|chat| {
            item.chat_id.as_deref() == Some(chat) || item.chats.iter().any(|link| link.id == chat)
        })
    });
    if let Some(cursor) = before {
        let (time, id) = cursor
            .split_once('|')
            .map_or((cursor, None), |(t, i)| (t, Some(i)));
        let time = DateTime::parse_from_rfc3339(time)
            .map_err(|_| "invalid_before")?
            .with_timezone(&Utc);
        items.retain(|item| {
            DateTime::parse_from_rfc3339(&item.created_at)
                .ok()
                .is_some_and(|created| {
                    created.with_timezone(&Utc) < time
                        || (created.with_timezone(&Utc) == time
                            && id.is_some_and(|id| item.id.as_str() < id))
                })
        });
    }
    items.truncate(limit.clamp(1, 100));
    Ok(items)
}

#[tauri::command]
pub(crate) async fn media_list(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
    kind: Option<String>,
    chat_id: Option<String>,
    before: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<MediaItem>, String> {
    let root = std::sync::Arc::new(state.paths.archive.current.clone().read_owned().await);
    let worker_root = root.clone();
    let _lease = root;
    let database = state.paths.database_path.clone();
    let items = tauri::async_runtime::spawn_blocking(move || {
        gallery(
            &worker_root,
            &database,
            kind.as_deref(),
            chat_id.as_deref(),
            before.as_deref(),
            limit.unwrap_or(40),
        )
    })
    .await
    .map_err(|_| "unavailable".to_string())??;
    for item in &items {
        app.asset_protocol_scope()
            .allow_file(&item.path)
            .map_err(|_| "unavailable".to_string())?;
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(root: &Path, n: u8, contents: &[u8]) {
        let hash = format!("{:064x}", n);
        let dir = root.join("sha256").join(&hash[..2]);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(hash), contents).unwrap();
    }
    fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut result = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        result.extend_from_slice(kind);
        result.extend_from_slice(body);
        result
    }
    fn mp4(handler: &[u8; 4]) -> Vec<u8> {
        let mut body = vec![0; 8];
        body.extend_from_slice(handler);
        let mdia = atom(b"mdia", &atom(b"hdlr", &body));
        let moov = atom(b"moov", &atom(b"trak", &mdia));
        let mut data = atom(b"ftyp", b"isom\0\0\0\0");
        data.extend(atom(b"mdat", &vec![0; 5000]));
        data.extend(moov);
        data
    }
    #[test]
    fn filters_and_returns_only_media() {
        let t = tempfile::tempdir().unwrap();
        fixture(t.path(), 1, b"\xff\xd8\xffjpeg");
        fixture(t.path(), 2, &mp4(b"vide"));
        fixture(t.path(), 3, b"not media");
        fixture(t.path(), 4, &mp4(b"soun"));
        let db = t.path().join("missing");
        assert_eq!(collect(t.path(), &db, None, None, 20).unwrap().len(), 3);
        assert_eq!(
            collect(t.path(), &db, Some("audio"), None, 20)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            collect(t.path(), &db, Some("image"), None, 20)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            collect(t.path(), &db, Some("video"), None, 20)
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn orders_and_paginates_without_losing_equal_timestamps() {
        let t = tempfile::tempdir().unwrap();
        for n in 1..=3 {
            fixture(t.path(), n, b"\xff\xd8\xffjpeg");
        }
        let db = t.path().join("missing");
        let all = collect(t.path(), &db, None, None, 10).unwrap();
        let first = collect(t.path(), &db, None, None, 1).unwrap();
        let cursor = format!("{}|{}", first[0].created_at, first[0].id);
        let next = collect(t.path(), &db, None, Some(&cursor), 10).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[1].id, next[0].id);
        assert_eq!(all[2].id, next[1].id);
    }
    #[test]
    fn missing_metadata_is_null_and_png_dimensions_are_read() {
        let t = tempfile::tempdir().unwrap();
        fixture(
            t.path(),
            1,
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x02\0\0\0\x03",
        );
        let items = collect(t.path(), &t.path().join("missing"), None, None, 4).unwrap();
        assert_eq!((items[0].width, items[0].height), (Some(2), Some(3)));
        assert!(
            items[0].title.is_none()
                && items[0].duration_sec.is_none()
                && items[0].poster_path.is_none()
                && items[0].chat_id.is_none()
        );
    }

    #[test]
    fn quarantine_is_not_returned_and_unreadable_index_is_not_ignored() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("index.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE momso_source_assets (id TEXT, content_sha256 TEXT, original_file_name TEXT, created_at TEXT, integrity_state TEXT); CREATE TABLE source_asset_deletions (asset_id TEXT);").unwrap();
        fixture(t.path(), 1, b"\xff\xd8\xffjpeg");
        let hash = format!("{:064x}", 1);
        conn.execute("INSERT INTO momso_source_assets VALUES ('id', ?1, 'photo.jpg', '2026-01-01T00:00:00Z', 'quarantined')", [&hash]).unwrap();
        assert!(collect(t.path(), &db, None, None, 4).unwrap().is_empty());
        conn.execute(
            "UPDATE momso_source_assets SET integrity_state = 'active'",
            [],
        )
        .unwrap();
        assert_eq!(collect(t.path(), &db, None, None, 4).unwrap().len(), 1);
        conn.execute("INSERT INTO source_asset_deletions VALUES ('id')", [])
            .unwrap();
        assert!(collect(t.path(), &db, None, None, 4).unwrap().is_empty());
        conn.execute("DELETE FROM source_asset_deletions", [])
            .unwrap();
        assert_eq!(collect(t.path(), &db, None, None, 4).unwrap().len(), 1);
        conn.execute_batch("DROP TABLE momso_source_assets;")
            .unwrap();
        assert!(collect(t.path(), &db, None, None, 4).is_err());
    }

    #[test]
    fn rejects_a_file_as_the_objects_root() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("objects");
        fs::write(&root, b"not a directory").unwrap();
        assert!(collect(&root, &t.path().join("missing"), None, None, 4).is_err());
    }

    #[test]
    fn reads_jpeg_and_gif_dimensions_without_guessing_truncated_values() {
        let jpeg =
            b"\xff\xd8\xff\xe0\x00\x04AB\xff\xc0\x00\x0b\x08\x00\x03\x00\x02\x01\x01\x11\x00";
        assert_eq!(classify(jpeg), Some(("image", Some((2, 3)))));
        assert_eq!(
            classify(b"GIF89a\x02\x00\x03\x00"),
            Some(("image", Some((2, 3))))
        );
        assert_eq!(
            classify(b"\xff\xd8\xff\xc0\x00\x0b\x08"),
            Some(("image", None))
        );
        assert_eq!(classify(b"GIF89a"), Some(("image", None)));
    }

    #[test]
    fn reads_webp_dimensions_without_guessing_truncated_values() {
        let extended = b"RIFF\x16\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0\x01\0\0\x02\0\0";
        let lossless = b"RIFF\x0d\0\0\0WEBPVP8L\x05\0\0\0\x2f\x01\x80\0\0";
        let lossy = b"RIFF\x16\0\0\0WEBPVP8 \x0e\0\0\0\0\0\0\x9d\x01\x2a\x02\0\x03\0";
        for bytes in [extended.as_slice(), lossless.as_slice(), lossy.as_slice()] {
            assert_eq!(classify(bytes), Some(("image", Some((2, 3)))));
            assert_eq!(classify(&bytes[..20]), Some(("image", None)));
        }
    }
}
