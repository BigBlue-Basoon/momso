//! Explicit generation provenance. An observed file write alone is not proof
//! that an image model produced the file or what prompt it actually received.
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use tauri::State;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GenerationInput {
    asset_id: String,
    prompt: String,
    provider: String,
    model: Option<String>,
    created_at: Option<String>,
    chat_id: String,
    message_id: Option<String>,
    parent_asset_id: Option<String>,
    version_how: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Generation {
    pub(crate) asset_id: String,
    pub(crate) kind: String,
    pub(crate) prompt: String,
    pub(crate) provider: String,
    pub(crate) model: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) chat_id: String,
    pub(crate) message_id: Option<String>,
    pub(crate) version_group_id: String,
    pub(crate) version: i64,
    pub(crate) parent_asset_id: Option<String>,
    pub(crate) version_how: Option<String>,
}

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Generation> {
    Ok(Generation {
        asset_id: row.get(0)?,
        kind: row.get(1)?,
        prompt: row.get(2)?,
        provider: row.get(3)?,
        model: row.get(4)?,
        created_at: row.get(5)?,
        chat_id: row.get(6)?,
        message_id: row.get(7)?,
        version_group_id: row.get(8)?,
        version: row.get(9)?,
        parent_asset_id: row.get(10)?,
        version_how: row.get(11)?,
    })
}

pub(crate) fn all(db: &Path) -> Result<Vec<Generation>, String> {
    let conn = rusqlite::Connection::open(db).map_err(|_| "unavailable")?;
    let mut statement = conn.prepare("SELECT asset_id,kind,prompt,provider,model,created_at,chat_id,message_id,version_group_id,version,parent_asset_id,version_how FROM media_generation ORDER BY version_group_id,version").map_err(|_| "unavailable")?;
    let rows = statement.query_map([], row).map_err(|_| "unavailable")?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "unavailable".into())
}

pub(crate) struct GeneratedOutputTurn<'a> {
    pub(crate) root: &'a Path,
    pub(crate) db: &'a Path,
    pub(crate) output: &'a Path,
    pub(crate) canonical_output: &'a Path,
    pub(crate) chat_id: &'a str,
    pub(crate) message_id: &'a str,
    pub(crate) provider: &'a str,
    pub(crate) model: Option<&'a str>,
    pub(crate) parent: Option<&'a str>,
}

pub(crate) fn register_generated_output(
    turn: &GeneratedOutputTurn<'_>,
    path: &Path,
    prompt: &str,
    requested_parent: Option<&str>,
) -> Result<(Generation, bool), String> {
    if prompt.len() > 64 * 1024 || !path.is_absolute() {
        return Err("invalid_arguments".into());
    }
    if turn.parent.is_some() && requested_parent.is_some() && turn.parent != requested_parent {
        return Err("invalid_arguments".into());
    }
    let parent = requested_parent.or(turn.parent);
    let output_meta = std::fs::symlink_metadata(turn.output).map_err(|_| "not_found")?;
    if !output_meta.is_dir()
        || crate::archive_location::is_link_or_reparse(&output_meta)
        || turn.output.canonicalize().map_err(|_| "not_found")? != turn.canonical_output
    {
        return Err("invalid_arguments".into());
    }
    let relative = path
        .strip_prefix(turn.output)
        .map_err(|_| "invalid_arguments")?;
    let mut current = turn.output.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err("invalid_arguments".into());
        };
        current.push(component);
        let metadata = std::fs::symlink_metadata(&current).map_err(|_| "not_found")?;
        if crate::archive_location::is_link_or_reparse(&metadata) {
            return Err("invalid_arguments".into());
        }
    }
    let canonical = path.canonicalize().map_err(|_| "not_found")?;
    if !canonical.starts_with(turn.canonical_output) || canonical == turn.canonical_output {
        return Err("invalid_arguments".into());
    }
    let mut source = std::fs::File::open(&canonical).map_err(|_| "not_found")?;
    if !source.metadata().map_err(|_| "not_found")?.is_file() {
        return Err("invalid_arguments".into());
    }
    #[cfg(windows)]
    if !crate::chat_items::opened_path_matches(&source, &canonical) {
        return Err("invalid_arguments".into());
    }
    let kind = match crate::media::classify_file(&mut source)? {
        Some((kind @ ("image" | "video"), _)) => kind,
        _ => return Err("invalid_arguments".into()),
    };
    if let Some(parent_id) = parent {
        if !all(turn.db)?.iter().any(|item| {
            item.asset_id == parent_id && item.chat_id == turn.chat_id && item.kind == kind
        }) {
            return Err("parent_not_found".into());
        }
    }
    use std::io::Seek;
    source.rewind().map_err(|_| "unavailable")?;
    let temp = tempfile::Builder::new()
        .prefix("momso-generated-")
        .tempdir()
        .map_err(|_| "unavailable")?;
    let name = path.file_name().ok_or("invalid_arguments")?;
    let staged = temp.path().join(name);
    let mut dest = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .map_err(|_| "unavailable")?;
    std::io::copy(&mut source, &mut dest).map_err(|_| "unavailable")?;
    drop(dest);
    let asset_id = crate::source_assets::import_generated(
        turn.db,
        &turn.root.join("objects"),
        &staged,
        turn.provider,
    )
    .map_err(|_| "unavailable")?;
    let input = GenerationInput {
        asset_id: asset_id.clone(),
        prompt: prompt.to_owned(),
        provider: turn.provider.to_owned(),
        model: turn.model.map(str::to_owned),
        created_at: Some(chrono::Utc::now().to_rfc3339()),
        chat_id: turn.chat_id.to_owned(),
        message_id: Some(turn.message_id.to_owned()),
        parent_asset_id: parent.map(str::to_owned),
        version_how: parent.map(|_| "regenerated".into()),
    };
    match register(turn.root, turn.db, input) {
        Ok(generation) => Ok((generation, true)),
        Err(error) if error == "already_recorded" => {
            let existing = all(turn.db)?
                .into_iter()
                .find(|item| {
                    item.asset_id == asset_id
                        && item.chat_id == turn.chat_id
                        && item.message_id.as_deref() == Some(turn.message_id)
                        && item.provider == turn.provider
                })
                .ok_or("invalid_arguments")?;
            Ok((existing, false))
        }
        Err(error) => Err(error),
    }
}

fn register(root: &Path, db: &Path, input: GenerationInput) -> Result<Generation, String> {
    if !crate::archive_zet::valid_asset_id(&input.asset_id)
        || !["codex", "claude", "gemini"].contains(&input.provider.as_str())
        || input.prompt.len() > 64 * 1024
        || input.model.as_ref().is_some_and(|v| v.len() > 200)
        || input
            .version_how
            .as_deref()
            .is_some_and(|v| !["regenerated", "edited"].contains(&v))
        || input
            .created_at
            .as_ref()
            .is_some_and(|v| chrono::DateTime::parse_from_rfc3339(v).is_err())
    {
        return Err("invalid_input".into());
    }
    let hash = &input.asset_id[7..];
    let objects = root
        .join("objects")
        .canonicalize()
        .map_err(|_| "asset_not_found")?;
    let path = objects.join("sha256").join(&hash[..2]).join(hash);
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| "asset_not_found")?;
    if !metadata.is_file()
        || crate::archive_location::is_link_or_reparse(&metadata)
        || !path
            .canonicalize()
            .map_err(|_| "asset_not_found")?
            .starts_with(&objects)
    {
        return Err("asset_not_found".into());
    }
    let mut file = std::fs::File::open(&path).map_err(|_| "asset_not_found")?;
    #[cfg(windows)]
    if !crate::chat_items::opened_path_matches(
        &file,
        &path.canonicalize().map_err(|_| "asset_not_found")?,
    ) {
        return Err("asset_not_found".into());
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 65_536];
    loop {
        let count = file.read(&mut buffer).map_err(|_| "asset_not_found")?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    if format!("{:x}", hasher.finalize()) != hash {
        return Err("asset_changed".into());
    }
    let kind = crate::media::classify_file(&mut file)?
        .map(|v| v.0)
        .filter(|v| *v == "image" || *v == "video")
        .ok_or("invalid_kind")?;
    let mut conn = crate::storage::open_writable_connection(db).map_err(|_| "unavailable")?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| "unavailable")?;
    let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chats WHERE id=?1) AND EXISTS(SELECT 1 FROM momso_source_assets a WHERE a.content_sha256=?2 AND a.integrity_state='active' AND NOT EXISTS(SELECT 1 FROM source_asset_deletions d WHERE d.asset_id=a.id))", params![input.chat_id,hash], |r| r.get(0)).map_err(|_| "unavailable")?;
    if !valid {
        return Err("asset_or_chat_not_found".into());
    }
    if let Some(message) = &input.message_id {
        let found: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_messages WHERE id=?1 AND chat_id=?2)",
                params![message, input.chat_id],
                |r| r.get(0),
            )
            .map_err(|_| "unavailable")?;
        if !found {
            return Err("message_not_found".into());
        }
    }
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM media_generation WHERE asset_id=?1)",
            [&input.asset_id],
            |r| r.get(0),
        )
        .map_err(|_| "unavailable")?;
    if exists {
        return Err("already_recorded".into());
    }
    let parent_group: Option<String> = if let Some(parent) = &input.parent_asset_id {
        Some(tx.query_row("SELECT version_group_id FROM media_generation WHERE asset_id=?1 AND chat_id=?2 AND kind=?3", params![parent,input.chat_id,kind], |r| r.get(0)).optional().map_err(|_| "unavailable")?.ok_or("parent_not_found")?)
    } else {
        None
    };
    let group = match parent_group {
        Some(value) => value,
        None if input.prompt.trim().is_empty() => uuid::Uuid::new_v4().to_string(),
        None => tx.query_row("SELECT version_group_id FROM media_generation WHERE chat_id=?1 AND kind=?2 AND prompt=?3 ORDER BY version LIMIT 1", params![input.chat_id,kind,input.prompt], |r| r.get(0)).optional().map_err(|_| "unavailable")?.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
    };
    let version: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(version),0)+1 FROM media_generation WHERE version_group_id=?1",
            [&group],
            |r| r.get(0),
        )
        .map_err(|_| "unavailable")?;
    let how = if version <= 1 {
        None
    } else {
        input
            .version_how
            .as_deref()
            .or(Some(if input.parent_asset_id.is_some() {
                "edited"
            } else {
                "regenerated"
            }))
    };
    tx.execute("INSERT INTO media_generation(asset_id,kind,prompt,provider,model,created_at,chat_id,message_id,version_group_id,version,parent_asset_id,version_how) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)", params![input.asset_id,kind,input.prompt,input.provider,input.model,input.created_at,input.chat_id,input.message_id,group,version,input.parent_asset_id,how]).map_err(|_| "unavailable")?;
    let result = tx.query_row("SELECT asset_id,kind,prompt,provider,model,created_at,chat_id,message_id,version_group_id,version,parent_asset_id,version_how FROM media_generation WHERE asset_id=?1", [&input.asset_id], row).map_err(|_| "unavailable")?;
    tx.commit().map_err(|_| "unavailable")?;
    Ok(result)
}

pub(crate) struct Regeneration {
    pub(crate) parent_asset_id: String,
    pub(crate) original_prompt: String,
    pub(crate) attachment: crate::chat::MessageAttachment,
}

pub(crate) fn prepare_regeneration(
    root: &Path,
    db: &Path,
    chat_id: &str,
    asset_id: &str,
) -> Result<Regeneration, String> {
    let original = all(db)?
        .into_iter()
        .find(|g| g.asset_id == asset_id && g.chat_id == chat_id)
        .ok_or("regenerate_not_found")?;
    let (_, name) = crate::media_save::prepare(root, db, asset_id)?;
    let hash = asset_id.strip_prefix("sha256:").ok_or("invalid_asset")?;
    let conn = rusqlite::Connection::open(db).map_err(|_| "unavailable")?;
    let media_type: Option<String> = conn
        .query_row(
            "SELECT media_type FROM momso_source_assets WHERE content_sha256=?1",
            [hash],
            |row| row.get(0),
        )
        .map_err(|_| "asset_not_found")?;
    Ok(Regeneration {
        parent_asset_id: asset_id.to_owned(),
        original_prompt: original.prompt,
        attachment: crate::chat::MessageAttachment {
            path: format!("objects/sha256/{}/{}", &hash[..2], hash),
            name,
            media_type,
        },
    })
}

pub(crate) struct RegenerationTurn<'a> {
    pub(crate) root: &'a Path,
    pub(crate) db: &'a Path,
    pub(crate) chat_id: &'a str,
    pub(crate) message_id: &'a str,
    pub(crate) provider: &'a str,
    pub(crate) prompt: &'a str,
    pub(crate) parent: Option<&'a str>,
}

pub(crate) fn notify_regeneration_result(
    chat_id: &str,
    message_id: &str,
    result: &Result<usize, String>,
    emit: impl FnOnce(&str, serde_json::Value),
) {
    let status = match result {
        Ok(0) => "not_detected",
        Ok(_) => "recorded",
        Err(_) => "unavailable",
    };
    emit(
        "chat-media-regeneration",
        serde_json::json!({"chatId": chat_id, "userMessageId": message_id, "status": status}),
    );
}

pub(crate) fn record_observed_regenerations(
    turn: &RegenerationTurn<'_>,
    before: &crate::chat_file_changes::Snapshot,
    after: &crate::chat_file_changes::Snapshot,
    observed: &std::collections::BTreeSet<String>,
) -> Result<usize, String> {
    let mut count = 0;
    for asset_id in observed.iter().filter(|id| {
        crate::archive_zet::valid_asset_id(id)
            && !before.contains_key(*id)
            && after.contains_key(*id)
    }) {
        let result = register(
            turn.root,
            turn.db,
            GenerationInput {
                asset_id: asset_id.clone(),
                prompt: turn.prompt.to_owned(),
                provider: turn.provider.to_owned(),
                model: None,
                created_at: None,
                chat_id: turn.chat_id.to_owned(),
                message_id: Some(turn.message_id.to_owned()),
                parent_asset_id: turn.parent.map(str::to_owned),
                version_how: turn.parent.map(|_| "regenerated".into()),
            },
        );
        match result {
            Ok(_) => count += 1,
            Err(reason) if reason == "already_recorded" => {
                let matching = all(turn.db)?.into_iter().any(|existing| {
                    existing.asset_id == *asset_id
                        && existing.chat_id == turn.chat_id
                        && existing.message_id.as_deref() == Some(turn.message_id)
                        && existing.provider == turn.provider
                        && existing.prompt == turn.prompt
                        && existing.parent_asset_id.as_deref() == turn.parent
                });
                if !matching {
                    return Err("already_recorded".into());
                }
            }
            Err(reason) if reason == "invalid_kind" => continue,
            Err(reason) => return Err(reason),
        }
    }
    Ok(count)
}

#[tauri::command]
pub(crate) async fn media_record_generation(
    state: State<'_, crate::AppState>,
    input: GenerationInput,
) -> Result<Generation, String> {
    if input.prompt.trim().is_empty() {
        return Err("invalid_input".into());
    }
    let root = std::sync::Arc::new(state.paths.archive.current.clone().read_owned().await);
    let worker_root = root.clone();
    let database = state.paths.database_path.clone();
    let _lease = root;
    tauri::async_runtime::spawn_blocking(move || register(&worker_root, &database, input))
        .await
        .map_err(|_| "unavailable".to_owned())?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VersionItem {
    #[serde(flatten)]
    generation: Generation,
    path: Option<String>,
    lineage_id: String,
    version_of: Option<String>,
}

fn versions(
    root: &Path,
    database: &Path,
    version_group_id: &str,
) -> Result<Vec<VersionItem>, String> {
    if uuid::Uuid::parse_str(version_group_id).is_err() {
        return Err("invalid_group".into());
    }
    let available = crate::media::collect(&root.join("objects"), database, None, None, usize::MAX)?;
    let mut result = Vec::new();
    let mut previous: Option<String> = None;
    for mut generation in all(database)?
        .into_iter()
        .filter(|item| item.version_group_id == version_group_id)
    {
        let path = available
            .iter()
            .find(|item| item.id == generation.asset_id)
            .map(|item| item.path.clone());
        let version_of = if generation.version > 1 {
            generation
                .parent_asset_id
                .clone()
                .or_else(|| previous.clone())
        } else {
            None
        };
        let version_how = (generation.version > 1).then(|| {
            generation.version_how.clone().unwrap_or_else(|| {
                if generation.parent_asset_id.is_some() {
                    "edited".into()
                } else {
                    "regenerated".into()
                }
            })
        });
        generation.version_how = version_how;
        previous = Some(generation.asset_id.clone());
        let lineage_id = generation.version_group_id.clone();
        result.push(VersionItem {
            generation,
            path,
            lineage_id,
            version_of,
        });
    }
    Ok(result)
}

#[tauri::command]
pub(crate) async fn media_versions(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
    version_group_id: String,
) -> Result<Vec<VersionItem>, String> {
    use tauri::Manager;
    let root = std::sync::Arc::new(state.paths.archive.current.clone().read_owned().await);
    let worker_root = root.clone();
    let database = state.paths.database_path.clone();
    let _lease = root;
    let items = tauri::async_runtime::spawn_blocking(move || {
        versions(&worker_root, &database, &version_group_id)
    })
    .await
    .map_err(|_| "unavailable")??;
    for item in &items {
        if let Some(value) = &item.path {
            app.asset_protocol_scope()
                .allow_file(value)
                .map_err(|_| "unavailable")?;
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_output_registers_once_and_rejects_outside_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        std::fs::create_dir_all(root.join("objects")).unwrap();
        let db = temp.path().join("test.sqlite");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let message =
            crate::chat::append(&db, &chat, crate::chat::Role::User, "image", None, None).unwrap();
        let output = crate::chat_output::create(&root, &chat).unwrap();
        let canonical = output.canonicalize().unwrap();
        let inside = output.join("image.jpg");
        std::fs::write(&inside, [0xff, 0xd8, 0xff, 1]).unwrap();
        let outside = temp.path().join("outside.jpg");
        std::fs::write(&outside, [0xff, 0xd8, 0xff, 2]).unwrap();
        let turn = GeneratedOutputTurn {
            root: &root,
            db: &db,
            output: &output,
            canonical_output: &canonical,
            chat_id: &chat,
            message_id: &message.id,
            provider: "codex",
            model: Some("model-fixture"),
            parent: None,
        };
        assert_eq!(
            register_generated_output(&turn, &outside, "draw", None).unwrap_err(),
            "invalid_arguments"
        );
        let (created, is_new) = register_generated_output(&turn, &inside, "draw", None).unwrap();
        assert!(is_new);
        assert_eq!(created.model.as_deref(), Some("model-fixture"));
        assert_eq!(created.message_id.as_deref(), Some(message.id.as_str()));
        let (again, is_new) = register_generated_output(&turn, &inside, "draw", None).unwrap();
        assert!(!is_new);
        assert_eq!(again.asset_id, created.asset_id);
        assert_eq!(all(&db).unwrap().len(), 1);
        assert!(std::fs::read(&inside).is_ok());
        let child = output.join("regenerated.jpg");
        std::fs::write(&child, [0xff, 0xd8, 0xff, 3]).unwrap();
        let (next, is_new) =
            register_generated_output(&turn, &child, "draw again", Some(&created.asset_id))
                .unwrap();
        assert!(is_new);
        assert_eq!(next.version_group_id, created.version_group_id);
        assert_eq!(next.version, created.version + 1);
        assert_eq!(next.version_how.as_deref(), Some("regenerated"));
    }

    #[test]
    fn observed_regeneration_uses_original_lineage_and_regenerated_kind() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("objects/sha256")).unwrap();
        std::fs::create_dir_all(root.join("receipts")).unwrap();
        let db = root.join("test.sqlite");
        crate::storage::initialize(&db, &root.join("backups"), "test").unwrap();
        let chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let create_asset = |byte: u8| {
            let bytes = [0xff, 0xd8, 0xff, byte];
            let hash = format!("{:x}", Sha256::digest(bytes));
            let path = root.join("objects/sha256").join(&hash[..2]);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join(&hash), bytes).unwrap();
            rusqlite::Connection::open(&db).unwrap().execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,media_type,captured_by,created_at) VALUES(?1,'other',?2,4,?3,'image.jpg','image/jpeg','test','2026-11-18T00:00:00Z')", params![format!("fixture-{byte}"),hash,format!("sha256/{}/{hash}",&hash[..2])]).unwrap();
            format!("sha256:{hash}")
        };
        let parent = create_asset(1);
        let first = register(
            root,
            &db,
            GenerationInput {
                asset_id: parent.clone(),
                prompt: "original prompt".into(),
                provider: "codex".into(),
                model: None,
                created_at: None,
                chat_id: chat.clone(),
                message_id: None,
                parent_asset_id: None,
                version_how: None,
            },
        )
        .unwrap();
        let request = prepare_regeneration(root, &db, &chat, &parent).unwrap();
        assert_eq!(request.original_prompt, "original prompt");
        assert!(request.attachment.path.starts_with("objects/sha256/"));
        assert!(prepare_regeneration(root, &db, "another-chat", &parent).is_err());
        let message = crate::chat::append(
            &db,
            &chat,
            crate::chat::Role::User,
            "original prompt",
            None,
            None,
        )
        .unwrap();
        let before = crate::chat_file_changes::capture(root).unwrap();
        let child = create_asset(2);
        let after = crate::chat_file_changes::capture(root).unwrap();
        let turn = RegenerationTurn {
            root,
            db: &db,
            chat_id: &chat,
            message_id: &message.id,
            provider: "codex",
            prompt: "original prompt",
            parent: Some(&parent),
        };
        let observed = std::collections::BTreeSet::from([child.clone()]);
        assert_eq!(
            record_observed_regenerations(&turn, &before, &after, &Default::default()).unwrap(),
            0
        );
        let recorded = record_observed_regenerations(&turn, &before, &after, &observed);
        assert_eq!(recorded, Ok(1));
        notify_regeneration_result(turn.chat_id, turn.message_id, &recorded, |name, value| {
            assert_eq!(name, "chat-media-regeneration");
            assert_eq!(value["chatId"], turn.chat_id);
            assert_eq!(value["userMessageId"], turn.message_id);
            assert_eq!(value["status"], "recorded");
        });
        assert_eq!(
            record_observed_regenerations(&turn, &before, &after, &observed),
            Ok(0)
        );
        let other_turn = RegenerationTurn {
            prompt: "a different request",
            ..turn
        };
        assert_eq!(
            record_observed_regenerations(&other_turn, &before, &after, &observed),
            Err("already_recorded".into())
        );
        let items = versions(root, &db, &first.version_group_id).unwrap();
        assert_eq!(items.len(), 2);
        let value = serde_json::to_value(&items[1]).unwrap();
        assert_eq!(value["assetId"], child);
        assert_eq!(value["lineageId"], first.version_group_id);
        assert_eq!(value["version"], 2);
        assert_eq!(value["versionOf"], parent);
        assert_eq!(value["versionHow"], "regenerated");
        assert_eq!(value["messageId"], message.id);
        let before = crate::chat_file_changes::capture(root).unwrap();
        let fresh = create_asset(3);
        let after = crate::chat_file_changes::capture(root).unwrap();
        let turn = RegenerationTurn {
            root,
            db: &db,
            chat_id: &chat,
            message_id: &message.id,
            provider: "claude",
            prompt: "new illustration",
            parent: None,
        };
        assert_eq!(
            record_observed_regenerations(
                &turn,
                &before,
                &after,
                &std::collections::BTreeSet::from([fresh.clone()])
            )
            .unwrap(),
            1
        );
        let gallery = serde_json::to_value(
            crate::media::gallery(root, &db, Some("image"), None, None, 40).unwrap(),
        )
        .unwrap();
        let item = gallery
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["assetId"] == fresh)
            .unwrap();
        assert_eq!(item["prompt"], "new illustration");
        assert_eq!(item["provider"], "claude");
    }

    #[test]
    fn two_recorded_versions_and_person_photo_are_distinct_in_gallery() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("objects/sha256")).unwrap();
        std::fs::create_dir_all(root.join("receipts")).unwrap();
        let db = root.join("test.sqlite");
        crate::storage::initialize(&db, &root.join("backups"), "test").unwrap();
        let chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let conn = rusqlite::Connection::open(&db).unwrap();
        let mut assets = Vec::new();
        for n in 1..=4_u8 {
            let bytes = [0xff, 0xd8, 0xff, n];
            let hash = format!("{:x}", Sha256::digest(bytes));
            let path = root.join("objects/sha256").join(&hash[..2]);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join(&hash), bytes).unwrap();
            conn.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,media_type,captured_by,created_at) VALUES(?1,'other',?2,4,?3,?4,'image/jpeg','test','2026-11-18T00:00:00Z')", params![format!("id-{n}"),hash,format!("sha256/{}/{hash}",&hash[..2]),format!("{n}.jpg")]).unwrap();
            assets.push(format!("sha256:{hash}"));
        }
        drop(conn);
        let input = |index: usize| GenerationInput {
            asset_id: assets[index].clone(),
            prompt: "fixture prompt".into(),
            provider: "codex".into(),
            model: None,
            created_at: None,
            chat_id: chat.clone(),
            message_id: None,
            parent_asset_id: None,
            version_how: None,
        };
        let first = register(root, &db, input(0)).unwrap();
        let second = register(root, &db, input(1)).unwrap();
        let mut edited_input = input(2);
        edited_input.prompt = "edit fixture".into();
        edited_input.parent_asset_id = Some(assets[1].clone());
        let edited = register(root, &db, edited_input).unwrap();
        assert_eq!(first.version_group_id, second.version_group_id);
        assert_eq!(edited.version_group_id, first.version_group_id);
        assert_eq!((first.version, second.version, edited.version), (1, 2, 3));
        assert_eq!(all(&db).unwrap().len(), 3);
        let versions = versions(root, &db, &first.version_group_id).unwrap();
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[0].generation.asset_id, assets[0]);
        assert_eq!(versions[1].generation.asset_id, assets[1]);
        let version_json = serde_json::to_value(&versions).unwrap();
        assert_eq!(version_json[1]["versionHow"], "regenerated");
        assert_eq!(version_json[1]["versionOf"], assets[0]);
        assert_eq!(version_json[2]["versionHow"], "edited");
        assert_eq!(version_json[2]["versionOf"], assets[1]);
        assert!(versions.iter().all(|item| item.path.is_some()));
        let gallery = serde_json::to_value(
            crate::media::gallery(root, &db, Some("image"), None, None, 40).unwrap(),
        )
        .unwrap();
        assert_eq!(gallery.as_array().unwrap().len(), 2);
        let generated = gallery
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["assetId"] == assets[2])
            .unwrap();
        assert_eq!(generated["version"], 3);
        assert_eq!(generated["versionCount"], 3);
        assert_eq!(generated["lineageId"], first.version_group_id);
        assert_eq!(generated["versionOf"], assets[1]);
        assert_eq!(generated["versionHow"], "edited");
        let human = gallery
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["assetId"] == assets[3])
            .unwrap();
        assert!(human["prompt"].is_null());
        assert_eq!(
            crate::media::gallery(root, &db, Some("image"), Some(&chat), None, 40)
                .unwrap()
                .len(),
            1
        );
        let other = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        crate::archive_receipt::attach_asset(root, &db, &other, "id-3").unwrap();
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute(
                "UPDATE chats SET archived_at='2026-11-18T00:00:00Z' WHERE id=?1",
                [&chat],
            )
            .unwrap();
        let archived = serde_json::to_value(
            crate::media::gallery(root, &db, Some("image"), None, None, 40).unwrap(),
        )
        .unwrap();
        let item = archived
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["assetId"] == assets[2])
            .unwrap();
        assert_eq!(item["chatId"], chat);
        assert_eq!(item["chats"][0]["id"], chat);
        assert!(item["chats"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == other));
        connection
            .execute("DELETE FROM chats WHERE id=?1", [&chat])
            .unwrap();
        let after_delete = serde_json::to_value(
            crate::media::gallery(root, &db, Some("image"), None, None, 40).unwrap(),
        )
        .unwrap();
        let item = after_delete
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["assetId"] == assets[2])
            .unwrap();
        assert!(item["chatId"].is_null());
        assert!(item["chats"].as_array().unwrap().is_empty());
        assert!(root
            .join(format!(
                "objects/sha256/{}/{}",
                &assets[2][7..9],
                &assets[2][7..]
            ))
            .is_file());
    }
}
