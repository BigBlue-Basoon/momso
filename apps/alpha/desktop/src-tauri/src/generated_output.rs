use std::path::{Path, PathBuf};

use rusqlite::params;

#[derive(Clone, Debug)]
pub(crate) struct Turn {
    pub(crate) id: String,
    pub(crate) chat_id: String,
    pub(crate) message_id: String,
    pub(crate) provider: String,
    pub(crate) parent: Option<String>,
    pub(crate) model: Option<String>,
}

pub(crate) fn begin(
    db: &Path,
    output: &Path,
    chat_id: &str,
    message_id: &str,
    provider: &str,
    parent: Option<&str>,
) -> Result<String, String> {
    let id = output
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("unavailable")?;
    uuid::Uuid::parse_str(id).map_err(|_| "unavailable")?;
    let conn = crate::storage::open_writable_connection(db).map_err(|_| "unavailable")?;
    conn.execute("INSERT INTO generated_output_turns(turn_id,chat_id,message_id,provider,parent_asset_id) VALUES(?1,?2,?3,?4,?5)", params![id,chat_id,message_id,provider,parent]).map_err(|_| "unavailable")?;
    Ok(id.to_owned())
}

pub(crate) fn observe_model(db: &Path, id: &str, model: &str) -> Result<(), String> {
    if model.is_empty() || model.len() > 200 {
        return Ok(());
    }
    let conn = crate::storage::open_writable_connection(db).map_err(|_| "unavailable")?;
    conn.execute("UPDATE generated_output_turns SET model_ambiguous=CASE WHEN model_ambiguous=1 OR (model IS NOT NULL AND model<>?2) THEN 1 ELSE 0 END, model=CASE WHEN model_ambiguous=1 OR (model IS NOT NULL AND model<>?2) THEN NULL ELSE ?2 END WHERE turn_id=?1 AND completed=0", params![id,model]).map_err(|_| "unavailable")?;
    Ok(())
}

pub(crate) fn pending(db: &Path) -> Result<Vec<Turn>, String> {
    let conn = rusqlite::Connection::open(db).map_err(|_| "unavailable")?;
    let mut statement = conn.prepare("SELECT turn_id,chat_id,message_id,provider,parent_asset_id,model FROM generated_output_turns WHERE completed=0 ORDER BY rowid").map_err(|_| "unavailable")?;
    let rows = statement
        .query_map([], |r| {
            Ok(Turn {
                id: r.get(0)?,
                chat_id: r.get(1)?,
                message_id: r.get(2)?,
                provider: r.get(3)?,
                parent: r.get(4)?,
                model: r.get(5)?,
            })
        })
        .map_err(|_| "unavailable")?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "unavailable".into())
}

fn output_path(root: &Path, turn: &Turn) -> Result<PathBuf, String> {
    if turn.chat_id.is_empty()
        || !turn
            .chat_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || uuid::Uuid::parse_str(&turn.id).is_err()
    {
        return Err("invalid_output_directory".into());
    }
    let root_canonical = root.canonicalize().map_err(|_| "unavailable")?;
    let mut path = root.to_path_buf();
    for component in [".momso", "outputs", &turn.chat_id, &turn.id] {
        path.push(component);
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| "unavailable")?;
        if !metadata.is_dir()
            || crate::archive_location::is_link_or_reparse(&metadata)
            || !path
                .canonicalize()
                .map_err(|_| "unavailable")?
                .starts_with(&root_canonical)
        {
            return Err("invalid_output_directory".into());
        }
    }
    Ok(path)
}

pub(crate) fn scan(root: &Path, db: &Path, turn: &Turn) -> Result<usize, String> {
    let output = output_path(root, turn)?;
    let canonical = output.canonicalize().map_err(|_| "unavailable")?;
    let mut count = 0;
    for entry in walkdir::WalkDir::new(&output)
        .follow_links(false)
        .min_depth(1)
    {
        let entry = entry.map_err(|_| "unavailable")?;
        let metadata = std::fs::symlink_metadata(entry.path()).map_err(|_| "unavailable")?;
        if crate::archive_location::is_link_or_reparse(&metadata) {
            return Err("invalid_output_directory".into());
        }
        if !metadata.is_file() {
            continue;
        }
        let candidate = entry.path().canonicalize().map_err(|_| "unavailable")?;
        if !candidate.starts_with(&canonical) {
            return Err("invalid_output_directory".into());
        }
        let mut file = std::fs::File::open(&candidate).map_err(|_| "unavailable")?;
        #[cfg(windows)]
        if !crate::chat_items::opened_path_matches(&file, &candidate) {
            return Err("invalid_output_directory".into());
        }
        if !matches!(
            crate::media::classify_file(&mut file)?,
            Some(("image" | "video", _))
        ) {
            continue;
        }
        let context = crate::media_generation::GeneratedOutputTurn {
            root,
            db,
            output: &output,
            canonical_output: &canonical,
            chat_id: &turn.chat_id,
            message_id: &turn.message_id,
            provider: &turn.provider,
            model: turn.model.as_deref(),
            parent: turn.parent.as_deref(),
        };
        let (_, created) =
            crate::media_generation::register_generated_output(&context, entry.path(), "", None)?;
        if created {
            count += 1;
        }
    }
    let conn = crate::storage::open_writable_connection(db).map_err(|_| "unavailable")?;
    conn.execute(
        "UPDATE generated_output_turns SET completed=1 WHERE turn_id=?1",
        [&turn.id],
    )
    .map_err(|_| "unavailable")?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_unannounced_media_deduplicates_tool_and_recovers_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        std::fs::create_dir_all(root.join("objects")).unwrap();
        let db = temp.path().join("test.sqlite");
        let backups = temp.path().join("backups");
        crate::storage::initialize(&db, &backups, "test").unwrap();
        let chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let message = crate::chat::append(
            &db,
            &chat,
            crate::chat::Role::User,
            "user request",
            None,
            None,
        )
        .unwrap();

        let first = crate::chat_output::create(&root, &chat).unwrap();
        let id = begin(&db, &first, &chat, &message.id, "codex", None).unwrap();
        observe_model(&db, &id, "fixture-model").unwrap();
        std::fs::write(first.join("silent.jpg"), [0xff, 0xd8, 0xff, 1]).unwrap();
        let turn = pending(&db).unwrap().pop().unwrap();
        assert_eq!(scan(&root, &db, &turn).unwrap(), 1);
        assert!(pending(&db).unwrap().is_empty());
        let first_generation = &crate::media_generation::all(&db).unwrap()[0];
        assert_eq!(first_generation.model.as_deref(), Some("fixture-model"));
        assert!(first_generation.prompt.is_empty());

        let second = crate::chat_output::create(&root, &chat).unwrap();
        begin(&db, &second, &chat, &message.id, "codex", None).unwrap();
        let path = second.join("announced.jpg");
        std::fs::write(&path, [0xff, 0xd8, 0xff, 2]).unwrap();
        let canonical = second.canonicalize().unwrap();
        let tool_turn = crate::media_generation::GeneratedOutputTurn {
            root: &root,
            db: &db,
            output: &second,
            canonical_output: &canonical,
            chat_id: &chat,
            message_id: &message.id,
            provider: "codex",
            model: None,
            parent: None,
        };
        assert!(
            crate::media_generation::register_generated_output(
                &tool_turn,
                &path,
                "actual image prompt",
                None
            )
            .unwrap()
            .1
        );
        let turn = pending(&db).unwrap().pop().unwrap();
        assert_eq!(scan(&root, &db, &turn).unwrap(), 0);
        assert_eq!(crate::media_generation::all(&db).unwrap().len(), 2);

        let third = crate::chat_output::create(&root, &chat).unwrap();
        let interrupted_id = begin(&db, &third, &chat, &message.id, "claude", None).unwrap();
        observe_model(&db, &interrupted_id, "first-model").unwrap();
        observe_model(&db, &interrupted_id, "second-model").unwrap();
        std::fs::write(third.join("interrupted.jpg"), [0xff, 0xd8, 0xff, 3]).unwrap();
        crate::storage::initialize(&db, &backups, "test").unwrap();
        let interrupted = pending(&db).unwrap();
        assert_eq!(interrupted.len(), 1);
        assert!(interrupted[0].model.is_none());
        assert_eq!(scan(&root, &db, &interrupted[0]).unwrap(), 1);
        assert!(pending(&db).unwrap().is_empty());
        assert_eq!(crate::media_generation::all(&db).unwrap().len(), 3);

        let fourth = crate::chat_output::create(&root, &chat).unwrap();
        begin(&db, &fourth, &chat, &message.id, "gemini", None).unwrap();
        std::fs::write(fourth.join("unknown1.jpg"), [0xff, 0xd8, 0xff, 4]).unwrap();
        std::fs::write(fourth.join("unknown2.jpg"), [0xff, 0xd8, 0xff, 5]).unwrap();
        let turn = pending(&db).unwrap().pop().unwrap();
        assert_eq!(scan(&root, &db, &turn).unwrap(), 2);
        let unknown: Vec<_> = crate::media_generation::all(&db)
            .unwrap()
            .into_iter()
            .filter(|item| item.prompt.is_empty() && item.provider == "gemini")
            .collect();
        assert_eq!(unknown.len(), 2);
        assert_ne!(unknown[0].version_group_id, unknown[1].version_group_id);
    }
}
