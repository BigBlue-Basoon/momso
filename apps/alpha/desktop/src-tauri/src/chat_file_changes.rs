use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use walkdir::WalkDir;

pub(crate) type Snapshot = BTreeMap<String, [u8; 32]>;

#[derive(Default)]
struct Activity {
    overlapping_chat: bool,
    app_writes: BTreeSet<String>,
    app_write_unknown: bool,
}

type ActivityKey = (PathBuf, String);
static ACTIVITY: OnceLock<Mutex<BTreeMap<ActivityKey, Activity>>> = OnceLock::new();
static SNAPSHOT_LOCK: Mutex<()> = Mutex::new(());
pub(crate) fn lock_app_write() -> std::sync::MutexGuard<'static, ()> {
    SNAPSHOT_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
fn activity() -> &'static Mutex<BTreeMap<ActivityKey, Activity>> {
    ACTIVITY.get_or_init(|| Mutex::new(BTreeMap::new()))
}
pub(crate) fn has_active(root: &Path) -> bool {
    activity()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .any(|(active_root, _)| active_root == root)
}

pub(crate) struct Tracking(ActivityKey);
pub(crate) fn begin_chat(root: &Path, db: &Path, chat: &str) -> Result<Tracking, &'static str> {
    let _guard = lock_app_write();
    if crate::archive_remote::first_receive_pending(root, db) {
        return Err("first_receive_in_progress");
    }
    Ok(Tracking::start(root, chat))
}
impl Tracking {
    pub(crate) fn start(root: &Path, chat: &str) -> Self {
        let key = (root.to_path_buf(), chat.to_owned());
        let mut all = activity().lock().unwrap_or_else(|e| e.into_inner());
        let mut overlapping_chat = false;
        for ((other_root, _), state) in all.iter_mut() {
            if other_root == root {
                state.overlapping_chat = true;
                overlapping_chat = true;
            }
        }
        all.insert(
            key.clone(),
            Activity {
                overlapping_chat,
                ..Activity::default()
            },
        );
        Self(key)
    }
    fn finish(self) -> Activity {
        activity()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0)
            .unwrap_or_default()
    }
}
impl Drop for Tracking {
    fn drop(&mut self) {
        activity()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

pub(crate) fn app_write(root: &Path, id: Option<&str>) {
    let mut all = activity().lock().unwrap_or_else(|e| e.into_inner());
    for ((active_root, _), state) in all.iter_mut() {
        if active_root == root {
            if let Some(id) = id {
                state.app_writes.insert(id.to_owned());
            } else {
                state.app_write_unknown = true;
            }
        }
    }
}

pub(crate) fn receipt_model(completed: Option<&str>, observed: Option<&str>) -> String {
    completed
        .filter(|model| !model.trim().is_empty())
        .or_else(|| observed.filter(|model| !model.trim().is_empty()))
        .unwrap_or_default()
        .to_owned()
}

pub(crate) fn reference(root: &Path, path: &str) -> Option<String> {
    let path = Path::new(path);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let root = root.canonicalize().ok()?;
    let path = match path.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            path.parent()?.canonicalize().ok()?.join(path.file_name()?)
        }
        Err(_) => return None,
    };
    let parts: Vec<_> = path
        .strip_prefix(root)
        .ok()?
        .iter()
        .map(|s| s.to_str())
        .collect::<Option<_>>()?;
    match parts.as_slice() {
        ["zettels", file] => file
            .strip_suffix(".md")
            .filter(|id| crate::archive_zet::valid_zet_id(id))
            .map(str::to_owned)
            .or_else(|| {
                let candidate = format!("document-path:{file}");
                crate::chat_recovery::valid_document_reference(&candidate).then_some(candidate)
            }),
        ["zettels", rest @ ..] if !rest.is_empty() => {
            let candidate = format!("document-path:{}", rest.join("/"));
            crate::chat_recovery::valid_document_reference(&candidate).then_some(candidate)
        }
        ["objects", "sha256", prefix, hash]
            if hash.is_ascii() && hash.len() == 64 && *prefix == &hash[..2] =>
        {
            let id = format!("sha256:{hash}");
            crate::archive_zet::valid_asset_id(&id).then_some(id)
        }
        _ => None,
    }
}

pub(crate) fn capture(root: &Path) -> Result<Snapshot, String> {
    let _guard = lock_app_write();
    capture_unlocked(root)
}
pub(crate) fn capture_unlocked(root: &Path) -> Result<Snapshot, String> {
    if !fs::metadata(root).map_err(|e| e.to_string())?.is_dir() {
        return Err("invalid archive folder".into());
    }
    let mut found = Snapshot::new();
    for folder in ["zettels", "objects"] {
        let base = root.join(folder);
        let meta = match fs::symlink_metadata(&base) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
            return Err("invalid archive folder".into());
        }
        for entry in WalkDir::new(&base).follow_links(false).min_depth(1) {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().is_file() {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&base)
                .map_err(|e| e.to_string())?;
            let parts: Vec<_> = relative.components().collect();
            let reference = if folder == "zettels" {
                let valid = (parts.len() == 1)
                    .then(|| {
                        relative
                            .file_name()
                            .and_then(|s| s.to_str())
                            .and_then(|s| s.strip_suffix(".md"))
                            .filter(|id| crate::archive_zet::valid_zet_id(id))
                            .map(str::to_owned)
                    })
                    .flatten();
                if valid.is_some() {
                    valid
                } else if relative
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case("md"))
                {
                    Some(format!(
                        "document-path:{}",
                        relative.to_str().ok_or("unsafe_path")?.replace('\\', "/")
                    ))
                } else {
                    None
                }
            } else if folder == "objects" && parts.len() == 3 && parts[0].as_os_str() == "sha256" {
                parts[2]
                    .as_os_str()
                    .to_str()
                    .filter(|hash| {
                        hash.len() == 64 && hash.is_ascii() && parts[1].as_os_str() == &hash[..2]
                    })
                    .map(|hash| format!("sha256:{hash}"))
                    .filter(|id| crate::archive_zet::valid_asset_id(id))
            } else if folder == "objects" {
                Some(format!(
                    "object-path:{}",
                    relative.to_str().ok_or("unsafe_path")?
                ))
            } else {
                None
            };
            let reference = match reference {
                Some(reference) => Some(reference),
                None if folder == "objects" => Some(format!(
                    "object-path:{}",
                    relative.to_str().ok_or("unsafe_path")?
                )),
                None => None,
            };
            let Some(reference) = reference else {
                continue;
            };
            let metadata = fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
            if crate::archive_location::is_link_or_reparse(&metadata) {
                continue;
            }
            let mut file = fs::File::open(entry.path()).map_err(|e| e.to_string())?;
            let mut hasher = Sha256::new();
            let mut buffer = [0_u8; 65536];
            loop {
                let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
            }
            found.insert(reference, hasher.finalize().into());
        }
    }
    Ok(found)
}

pub(crate) fn changes(before: &Snapshot, after: &Snapshot) -> BTreeMap<String, String> {
    let mut result: BTreeMap<String, String> = after
        .iter()
        .filter_map(|(id, hash)| match before.get(id) {
            None => Some((id.clone(), "created".into())),
            Some(old) if old != hash => Some((id.clone(), "edited".into())),
            _ => None,
        })
        .collect();
    for id in before.keys() {
        if !after.contains_key(id) {
            result.insert(id.clone(), "deleted".into());
        }
    }
    result
}

pub(crate) fn source_changes(before: &Snapshot, after: &Snapshot) -> BTreeMap<String, String> {
    changes(before, after)
        .into_iter()
        .filter(|(id, action)| {
            (id.starts_with("sha256:") || id.starts_with("object-path:")) && action != "created"
        })
        .collect()
}

pub(crate) fn source_change_details(before: &Snapshot, after: &Snapshot) -> Vec<serde_json::Value> {
    source_changes(before, after)
        .into_iter()
        .map(|(id, action)| {
            let hash = before
                .get(&id)
                .map(|bytes| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>());
            let relative = id
                .strip_prefix("object-path:")
                .map(|path| path.replace('\\', "/"))
                .or_else(|| {
                    id.strip_prefix("sha256:").and_then(|digest| {
                        digest
                            .get(..2)
                            .map(|prefix| format!("sha256/{prefix}/{digest}"))
                    })
                });
            serde_json::json!({"id":id,"action":action,"relativePath":relative,"beforeSha256":hash})
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
struct Attribution {
    actions: BTreeMap<String, String>,
    evidence: BTreeMap<String, String>,
    unknown_actor: Vec<String>,
}

fn attribute(
    actions: BTreeMap<String, String>,
    tool_writes: &BTreeSet<String>,
    app_writes: &BTreeSet<String>,
    overlapping_chat: bool,
    app_write_unknown: bool,
) -> Attribution {
    let mut evidence = BTreeMap::new();
    let mut unknown_actor = Vec::new();
    for id in actions.keys() {
        if tool_writes.contains(id) {
            evidence.insert(id.clone(), "tool".into());
        } else {
            evidence.insert(id.clone(), "diff".into());
        }
        if app_write_unknown
            || app_writes.contains(id)
            || (!tool_writes.contains(id)
                && (overlapping_chat || actions.get(id).is_some_and(|action| action == "deleted")))
        {
            unknown_actor.push(id.clone());
        }
    }
    Attribution {
        actions,
        evidence,
        unknown_actor,
    }
}

fn verified_created_assets(result: &Attribution) -> BTreeSet<String> {
    result
        .actions
        .iter()
        .filter(|(id, action)| {
            *action == "created"
                && crate::archive_zet::valid_asset_id(id)
                && result.evidence.get(*id).is_some_and(|v| v == "tool")
                && !result.unknown_actor.contains(*id)
        })
        .map(|(id, _)| id.clone())
        .collect()
}

#[cfg(test)]
pub(crate) fn record(
    root: &Path,
    database: &Path,
    mut input: crate::archive_receipt::ReceiptCreate,
    before: &Snapshot,
    after: &Snapshot,
) -> Result<bool, String> {
    let mut actions = changes(before, after);
    actions.retain(|id, _| !id.starts_with("object-path:"));
    if actions.is_empty() && input.read.is_empty() {
        return Ok(false);
    }
    input.wrote = actions.keys().cloned().collect();
    crate::archive_receipt::create_with_actions(root, database, input, actions)?;
    Ok(true)
}

#[cfg(test)]
pub(crate) fn record_tracked(
    root: &Path,
    database: &Path,
    input: crate::archive_receipt::ReceiptCreate,
    before: &Snapshot,
    after: &Snapshot,
    tool_writes: &BTreeSet<String>,
    tracking: Tracking,
) -> Result<bool, String> {
    record_tracked_with_created(root, database, input, before, after, tool_writes, tracking)
        .map(|value| value.0)
}

pub(crate) fn record_tracked_with_created(
    root: &Path,
    database: &Path,
    mut input: crate::archive_receipt::ReceiptCreate,
    before: &Snapshot,
    after: &Snapshot,
    tool_writes: &BTreeSet<String>,
    tracking: Tracking,
) -> Result<(bool, BTreeSet<String>), String> {
    let activity = tracking.finish();
    let mut actions = changes(before, after);
    actions.retain(|id, _| !id.starts_with("object-path:"));
    let result = attribute(
        actions,
        tool_writes,
        &activity.app_writes,
        activity.overlapping_chat,
        activity.app_write_unknown,
    );
    if result.actions.is_empty() && input.read.is_empty() {
        return Ok((false, BTreeSet::new()));
    }
    let created_documents: Vec<_> = result
        .actions
        .iter()
        .filter_map(|(id, action)| {
            (action == "created"
                && result.evidence.get(id).is_some_and(|e| e == "tool")
                && !result.unknown_actor.contains(id))
            .then_some(id.clone())
        })
        .collect();
    let created_assets = if activity.overlapping_chat {
        BTreeSet::new()
    } else {
        verified_created_assets(&result)
    };
    input.wrote = result.actions.keys().cloned().collect();
    let receipt = crate::archive_receipt::create_with_evidence(
        root,
        database,
        input,
        result.actions,
        result.evidence,
        result.unknown_actor,
    )?;
    save_ai_created_documents(root, database, after, &created_documents, &receipt)?;
    Ok((true, created_assets))
}

fn save_ai_created_documents(
    root: &Path,
    database: &Path,
    after: &Snapshot,
    ids: &[String],
    receipt: &str,
) -> Result<(), String> {
    let _app = lock_app_write();
    let _write = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "save_failed".to_string())?;
    crate::archive_remote::ensure_document_write_allowed(root, database)?;
    if has_active(root) {
        return Ok(());
    }
    let mut paths = Vec::new();
    for id in ids {
        let relative = if crate::archive_zet::valid_zet_id(id) {
            format!("zettels/{id}.md")
        } else if crate::chat_recovery::valid_document_reference(id) {
            format!("zettels/{}", id.strip_prefix("document-path:").unwrap())
        } else {
            continue;
        };
        let path = root.join(&relative);
        let meta = fs::symlink_metadata(&path).map_err(|_| "save_failed".to_string())?;
        if !meta.is_file()
            || crate::archive_location::is_link_or_reparse(&meta)
            || reference(root, &relative).as_deref() != Some(id.as_str())
        {
            continue;
        }
        let bytes = fs::read(&path).map_err(|_| "save_failed".to_string())?;
        if after
            .get(id)
            .is_none_or(|expected| Sha256::digest(&bytes).as_slice() != expected)
        {
            continue;
        }
        paths.push(relative);
    }
    if paths.is_empty() {
        return Ok(());
    }
    let (tool, _) = crate::archive::storage_tool(root)?;
    paths.push(format!("receipts/{receipt}.md"));
    if crate::archive_remote::fresh_archive_with_local_files(&tool, root, true) {
        for name in [
            ".gitignore",
            "AGENTS.md",
            "CLAUDE.md",
            "GEMINI.md",
            crate::yoga_dictionary::DIRECTORY,
        ] {
            if root.join(name).exists() {
                paths.push(name.to_owned());
            }
        }
    }
    for path in &paths {
        if !crate::archive_remote::git(
            &tool,
            root,
            &["--literal-pathspecs", "ls-files", "--", path],
        )
        .map_err(|_| "save_failed".to_string())?
        .is_empty()
        {
            return Err("save_failed".into());
        }
    }
    let mut args = vec!["--literal-pathspecs", "add", "--"];
    args.extend(paths.iter().map(String::as_str));
    crate::archive_remote::git(&tool, root, &args).map_err(|_| "save_failed".to_string())?;
    crate::archive::commit_paths(&tool, root, "저장: AI가 만든 글", &paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ai_created_document_is_saved_before_connecting_empty_remote() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "fixture").unwrap();
        crate::archive::initialize(&root, temp.path()).unwrap();
        let chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let tracking = begin_chat(&root, &db, &chat).unwrap();
        let before = capture(&root).unwrap();
        let id = crate::archive_zet::create(
            &root,
            &db,
            crate::archive_zet::ZetCreate {
                title: "Fixture".into(),
                summary: "Fixture".into(),
                kind: "note".into(),
                body: "fixture AI document".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        let content = fs::read_to_string(root.join(format!("zettels/{id}.md"))).unwrap();
        let after = capture(&root).unwrap();
        let input = crate::archive_receipt::ReceiptCreate {
            tool: "claude".into(),
            model: "fixture".into(),
            chat: Some(chat),
            read: vec![],
            wrote: vec![],
            prompt: "fixture".into(),
            changes: String::new(),
        };
        assert!(record_tracked(
            &root,
            &db,
            input,
            &before,
            &after,
            &BTreeSet::from([id.to_string()]),
            tracking
        )
        .unwrap());
        let tool = crate::archive::storage_tool(temp.path()).unwrap().0;
        assert_eq!(
            crate::archive_remote::git(&tool, &root, &["show", &format!("HEAD:zettels/{id}.md")])
                .unwrap(),
            content.trim_end()
        );
        let bare = temp.path().join("remote.git");
        crate::archive_remote::git(
            &tool,
            temp.path(),
            &["init", "--bare", bare.to_str().unwrap()],
        )
        .unwrap();
        crate::archive_remote::connect_local(&tool, &root, bare.to_str().unwrap()).unwrap();
        crate::archive_remote::receive_connected(&tool, &root, &db, bare.to_str().unwrap(), "main")
            .unwrap();
        assert_eq!(
            fs::read_to_string(root.join(format!("zettels/{id}.md"))).unwrap(),
            content
        );
    }

    #[test]
    fn human_changed_document_is_not_auto_committed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "fixture").unwrap();
        crate::archive::initialize(&root, temp.path()).unwrap();
        let chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let tracking = begin_chat(&root, &db, &chat).unwrap();
        let before = capture(&root).unwrap();
        let id = "zet_20261118_120001";
        fs::write(root.join(format!("zettels/{id}.md")), "human text").unwrap();
        app_write(&root, Some(id));
        let after = capture(&root).unwrap();
        let input = crate::archive_receipt::ReceiptCreate {
            tool: "claude".into(),
            model: "fixture".into(),
            chat: Some(chat),
            read: vec![],
            wrote: vec![],
            prompt: "fixture".into(),
            changes: String::new(),
        };
        assert!(record_tracked(
            &root,
            &db,
            input,
            &before,
            &after,
            &BTreeSet::from([id.to_string()]),
            tracking
        )
        .unwrap());
        let tool = crate::archive::storage_tool(temp.path()).unwrap().0;
        assert!(crate::archive_remote::git(
            &tool,
            &root,
            &["show", &format!("HEAD:zettels/{id}.md")]
        )
        .is_err());
        assert_eq!(
            fs::read_to_string(root.join(format!("zettels/{id}.md"))).unwrap(),
            "human text"
        );
    }

    #[test]
    fn generated_candidates_exclude_person_and_unreported_writes() {
        let a = format!("sha256:{}", "a".repeat(64));
        let b = format!("sha256:{}", "b".repeat(64));
        let c = format!("sha256:{}", "c".repeat(64));
        let actions = [(&a, "created"), (&b, "created"), (&c, "created")]
            .into_iter()
            .map(|(id, action)| (id.clone(), action.to_owned()))
            .collect();
        let evidence = attribute(
            actions,
            &BTreeSet::from([a.clone(), b.clone()]),
            &BTreeSet::from([b]),
            false,
            false,
        );
        assert_eq!(verified_created_assets(&evidence), BTreeSet::from([a]));
    }
    #[test]
    fn deleted_file_tool_reference_and_uncertain_actor_are_distinguished() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("zettels")).unwrap();
        let path = dir.path().join("zettels/zet_20261115_120000.md");
        assert_eq!(
            reference(dir.path(), path.to_str().unwrap()).as_deref(),
            Some("zet_20261115_120000")
        );
        let actions = BTreeMap::from([("zet_20261115_120000".to_string(), "deleted".to_string())]);
        let unknown = attribute(
            actions.clone(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            false,
            false,
        );
        assert_eq!(unknown.unknown_actor, ["zet_20261115_120000"]);
        let observed = attribute(
            actions,
            &BTreeSet::from(["zet_20261115_120000".to_string()]),
            &BTreeSet::new(),
            false,
            false,
        );
        assert!(observed.unknown_actor.is_empty());
    }
    #[test]
    fn attribution_distinguishes_tool_diff_and_app_writes_during_overlap() {
        let actions = [("a", "created"), ("b", "edited"), ("c", "edited")]
            .into_iter()
            .map(|(a, b)| (a.to_owned(), b.to_owned()))
            .collect();
        let result = attribute(
            actions,
            &BTreeSet::from(["a".into()]),
            &BTreeSet::from(["c".into()]),
            true,
            false,
        );
        assert_eq!(result.evidence.get("a").map(String::as_str), Some("tool"));
        assert_eq!(result.evidence.get("b").map(String::as_str), Some("diff"));
        assert_eq!(result.unknown_actor, ["b", "c"]);
        assert!(result.actions.contains_key("c"));
        let observed_with_app_write = attribute(
            BTreeMap::from([("c".into(), "edited".into())]),
            &BTreeSet::from(["c".into()]),
            &BTreeSet::from(["c".into()]),
            false,
            false,
        );
        assert_eq!(observed_with_app_write.evidence["c"], "tool");
        assert_eq!(observed_with_app_write.unknown_actor, ["c"]);
        let unknown_app_write = attribute(
            BTreeMap::from([("c".into(), "edited".into())]),
            &BTreeSet::from(["c".into()]),
            &BTreeSet::new(),
            false,
            true,
        );
        assert_eq!(unknown_app_write.unknown_actor, ["c"]);
    }

    #[test]
    fn overlapping_rooms_record_evidence_without_inventing_actors() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        fs::create_dir_all(root.join("zettels")).unwrap();
        fs::create_dir_all(root.join("receipts")).unwrap();
        let db = temp.path().join("test.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let first = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let second = "chat_20261115_120001";
        rusqlite::Connection::open(&db).unwrap().execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES (?1,'','2026-11-15','2026-11-15')", [second]
        ).unwrap();
        let one = Tracking::start(&root, &first);
        let two = Tracking::start(&root, second);
        let before = capture(&root).unwrap();
        let hash = "a".repeat(64);
        fs::create_dir_all(root.join("objects/sha256/aa")).unwrap();
        fs::write(
            root.join("objects/sha256/aa").join(&hash),
            b"RIFF\0\0\0\0WAVE",
        )
        .unwrap();
        for (id, title) in [
            ("zet_20261115_120000", "One"),
            ("zet_20261115_120001", "Two"),
            ("zet_20261115_120002", "User"),
        ] {
            let head = format!("---\nid: {id}\ntitle: {title}\nsummary: summary\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T12:00:00+09:00'\nupdated_at: '2026-11-15T12:00:00+09:00'\nassets: []\nedges: []\n---\nbody\n");
            fs::write(root.join("zettels").join(format!("{id}.md")), head).unwrap();
        }
        app_write(&root, Some("zet_20261115_120002"));
        let after = capture(&root).unwrap();
        let input = crate::archive_receipt::ReceiptCreate {
            tool: "codex".into(),
            model: "model".into(),
            chat: Some(first.clone()),
            read: vec![],
            wrote: vec![],
            prompt: "request".into(),
            changes: String::new(),
        };
        assert!(record_tracked(
            &root,
            &db,
            input,
            &before,
            &after,
            &BTreeSet::from(["zet_20261115_120000".into()]),
            one
        )
        .unwrap());
        let value =
            serde_json::to_value(crate::chat_items::list(&root, &db, &first).unwrap()).unwrap();
        let docs = value["documents"].as_array().unwrap();
        let tool = docs
            .iter()
            .find(|x| x["id"] == "zet_20261115_120000")
            .unwrap();
        assert_eq!(tool["evidence"], "tool");
        assert_eq!(tool["by"]["provider"], "codex");
        let diff = docs
            .iter()
            .find(|x| x["id"] == "zet_20261115_120001")
            .unwrap();
        assert_eq!(diff["evidence"], "diff");
        assert!(diff["by"].is_null());
        let shared = docs
            .iter()
            .find(|x| x["id"] == "zet_20261115_120002")
            .unwrap();
        assert!(shared["by"].is_null());
        assert_eq!(value["media"][0]["evidence"], "diff");
        assert!(value["media"][0]["by"].is_null());
        let second_input = crate::archive_receipt::ReceiptCreate {
            tool: "claude".into(),
            model: "model".into(),
            chat: Some(second.into()),
            read: vec![],
            wrote: vec![],
            prompt: "request".into(),
            changes: String::new(),
        };
        assert!(record_tracked(
            &root,
            &db,
            second_input,
            &before,
            &after,
            &BTreeSet::new(),
            two
        )
        .unwrap());
        let other =
            serde_json::to_value(crate::chat_items::list(&root, &db, second).unwrap()).unwrap();
        let other_docs = other["documents"].as_array().unwrap();
        let other_diff = other_docs
            .iter()
            .find(|x| x["id"] == "zet_20261115_120001")
            .unwrap();
        assert_eq!(other_diff["evidence"], "diff");
        assert!(other_diff["by"].is_null());
        let shared = other_docs
            .iter()
            .find(|x| x["id"] == "zet_20261115_120002")
            .unwrap();
        assert!(shared["by"].is_null());
        let reverse = serde_json::to_value(
            crate::archive_receipt::chats_for_reference(&root, &db, "zet_20261115_120001").unwrap(),
        )
        .unwrap();
        assert_eq!(reverse.as_array().unwrap().len(), 2);
        assert!(reverse
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["by"].is_null()));
        let mut media = crate::media::collect(&root.join("objects"), &db, None, None, 10).unwrap();
        crate::chat_items::enrich_media(&root, &db, &mut media).unwrap();
        let media = serde_json::to_value(media).unwrap();
        assert_eq!(media[0]["evidence"], "diff");
        assert!(media[0]["by"].is_null());
    }

    #[test]
    fn app_document_edit_is_recorded_without_claiming_ai_wrote_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        fs::create_dir_all(root.join("zettels")).unwrap();
        fs::create_dir_all(root.join("receipts")).unwrap();
        let db = temp.path().join("test.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let id = "zet_20261115_120000";
        fs::write(root.join("zettels").join(format!("{id}.md")), "---\nid: zet_20261115_120000\ntitle: One\nsummary: summary\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T12:00:00+09:00'\nupdated_at: '2026-11-15T12:00:00+09:00'\nassets: []\nedges: []\n---\nbefore\n").unwrap();
        let tracking = Tracking::start(&root, &chat);
        let before = capture(&root).unwrap();
        {
            let _guard = lock_app_write();
            app_write(&root, Some(id));
            crate::archive_zet::update_checked(
                &root,
                &db,
                id,
                crate::archive_zet::ZetUpdate {
                    title: None,
                    summary: None,
                    body: Some("edited by user".into()),
                    status: None,
                    assets: None,
                    edges: None,
                },
                None,
            )
            .unwrap();
        }
        let after = capture(&root).unwrap();
        assert_eq!(
            changes(&before, &after).get(id).map(String::as_str),
            Some("edited")
        );
        let input = crate::archive_receipt::ReceiptCreate {
            tool: "codex".into(),
            model: "model".into(),
            chat: Some(chat.clone()),
            read: vec![],
            wrote: vec![],
            prompt: String::new(),
            changes: String::new(),
        };
        assert!(record_tracked(
            &root,
            &db,
            input,
            &before,
            &after,
            &BTreeSet::new(),
            tracking
        )
        .unwrap());
        let items =
            serde_json::to_value(crate::chat_items::list(&root, &db, &chat).unwrap()).unwrap();
        let item = items["documents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .unwrap();
        assert!(item["by"].is_null());
    }
    #[test]
    fn interrupted_reply_uses_only_an_observed_model() {
        assert_eq!(receipt_model(None, Some("actual-model")), "actual-model");
        assert_eq!(
            receipt_model(Some("saved-model"), Some("actual-model")),
            "saved-model"
        );
        assert_eq!(receipt_model(None, None), "");
        assert_eq!(
            receipt_model(Some(""), Some("actual-model")),
            "actual-model"
        );
    }
    #[test]
    fn detects_created_and_modified_files_by_contents() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("zettels")).unwrap();
        let old = dir.path().join("zettels/zet_20261115_120000.md");
        let new = dir.path().join("zettels/zet_20261115_120001.md");
        fs::write(&old, "하나").unwrap();
        let before = capture(dir.path()).unwrap();
        fs::write(&old, "둘").unwrap();
        fs::write(&new, "셋").unwrap();
        let result = changes(&before, &capture(dir.path()).unwrap());
        assert_eq!(
            result.get("zet_20261115_120000").map(String::as_str),
            Some("edited")
        );
        assert_eq!(
            result.get("zet_20261115_120001").map(String::as_str),
            Some("created")
        );
    }

    #[test]
    fn unavailable_archive_is_not_reported_as_an_unchanged_empty_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("archive");
        fs::create_dir(&archive).unwrap();
        assert!(capture(&archive).unwrap().is_empty());
        fs::remove_dir(&archive).unwrap();
        assert!(capture(&archive).is_err());
        fs::write(&archive, b"not a directory").unwrap();
        assert!(capture(&archive).is_err());
        fs::remove_file(&archive).unwrap();
        fs::create_dir(&archive).unwrap();
        fs::write(archive.join("objects"), b"not a directory").unwrap();
        assert!(capture(&archive).is_err());
    }

    #[test]
    fn deletion_is_detected_but_does_not_itself_preserve_bytes() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("zettels")).unwrap();
        let file = dir.path().join("zettels/zet_20261115_120000.md");
        fs::write(&file, b"unsaved lesson").unwrap();
        let before = capture(dir.path()).unwrap();
        fs::remove_file(&file).unwrap();
        let after = capture(dir.path()).unwrap();
        assert!(!file.exists());
        assert_eq!(
            changes(&before, &after)
                .get("zet_20261115_120000")
                .map(String::as_str),
            Some("deleted")
        );
    }

    #[test]
    fn deleted_document_is_recorded_and_listed_after_file_disappears() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("zettels")).unwrap();
        fs::create_dir_all(root.join("objects")).unwrap();
        fs::create_dir_all(root.join("receipts")).unwrap();
        let db = root.join("state.db");
        crate::storage::initialize(&db, &root.join("backups"), "test").unwrap();
        let chat = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let file = root.join("zettels/zet_20261115_120000.md");
        fs::write(&file, b"private unsaved document").unwrap();
        let before = capture(root).unwrap();
        fs::remove_file(&file).unwrap();
        let after = capture(root).unwrap();
        let input = crate::archive_receipt::ReceiptCreate {
            tool: "claude".into(),
            model: "test".into(),
            chat: Some(chat.clone()),
            read: vec![],
            wrote: vec![],
            prompt: String::new(),
            changes: String::new(),
        };
        assert!(record(root, &db, input, &before, &after).unwrap());
        let result =
            serde_json::to_value(crate::chat_items::list(root, &db, &chat).unwrap()).unwrap();
        assert_eq!(result["deleted"][0]["id"], "zet_20261115_120000");
        assert_eq!(result["deleted"][0]["how"], "deleted");
        assert_eq!(result["deleted"][0]["by"]["provider"], "claude");
    }

    #[test]
    fn modified_and_missing_originals_raise_integrity_changes() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("objects/sha256/aa");
        fs::create_dir_all(&folder).unwrap();
        let hash = "a".repeat(64);
        let file = folder.join(&hash);
        fs::write(&file, b"original").unwrap();
        let before = capture(dir.path()).unwrap();
        fs::write(&file, b"changed").unwrap();
        let changed = source_changes(&before, &capture(dir.path()).unwrap());
        assert_eq!(
            changed.get(&format!("sha256:{hash}")).map(String::as_str),
            Some("edited")
        );
        fs::remove_file(&file).unwrap();
        let deleted = source_changes(&before, &capture(dir.path()).unwrap());
        assert_eq!(
            deleted.get(&format!("sha256:{hash}")).map(String::as_str),
            Some("deleted")
        );
    }

    #[test]
    fn non_content_addressed_original_changes_are_reported_by_path() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("objects/legacy");
        fs::create_dir_all(&folder).unwrap();
        let file = folder.join("recording.wav");
        fs::write(&file, b"original").unwrap();
        let before = capture(dir.path()).unwrap();
        fs::remove_file(&file).unwrap();
        let after = capture(dir.path()).unwrap();
        assert_eq!(
            source_changes(&before, &after)
                .get(&format!(
                    "object-path:{}",
                    std::path::Path::new("legacy")
                        .join("recording.wav")
                        .display()
                ))
                .map(String::as_str),
            Some("deleted")
        );
    }

    #[test]
    fn change_receipt_reaches_chat_and_media_lists() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        fs::create_dir_all(root.join("zettels")).unwrap();
        fs::create_dir_all(root.join("receipts")).unwrap();
        let hash = "a".repeat(64);
        let asset = format!("sha256:{hash}");
        fs::create_dir_all(root.join("objects/sha256/aa")).unwrap();
        let db = dir.path().join("test.db");
        crate::storage::initialize(&db, &dir.path().join("backups"), "test").unwrap();
        let id = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let document = root.join("zettels/zet_20261115_120000.md");
        let header = "---\nid: zet_20261115_120000\ntitle: 수업\nsummary: 요약\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T12:00:00+09:00'\nupdated_at: '2026-11-15T12:00:00+09:00'\nassets: []\nedges: []\n---\n";
        fs::write(&document, format!("{header}이전")).unwrap();
        let tracking = Tracking::start(&root, &id);
        let before = capture(&root).unwrap();
        fs::write(&document, format!("{header}변경")).unwrap();
        fs::write(
            root.join("objects/sha256/aa").join(&hash),
            b"RIFF\0\0\0\0WAVE",
        )
        .unwrap();
        let after = capture(&root).unwrap();
        let receipt = crate::archive_receipt::ReceiptCreate {
            tool: "codex".into(),
            model: "test-model".into(),
            chat: Some(id.clone()),
            read: vec![],
            wrote: vec![],
            prompt: "고쳐 주세요".into(),
            changes: String::new(),
        };
        assert!(record_tracked(
            &root,
            &db,
            receipt,
            &before,
            &after,
            &BTreeSet::from(["zet_20261115_120000".into()]),
            tracking
        )
        .unwrap());
        let listed =
            serde_json::to_value(crate::chat_items::list(&root, &db, &id).unwrap()).unwrap();
        assert_eq!(listed["documents"][0]["how"], "edited");
        assert_eq!(listed["documents"][0]["evidence"], "tool");
        assert_eq!(listed["documents"][0]["by"]["model"], "test-model");
        assert_eq!(listed["media"][0]["id"], asset);
        assert_eq!(listed["media"][0]["how"], "created");
        assert_eq!(listed["media"][0]["evidence"], "diff");
        let mut media = crate::media::collect(&root.join("objects"), &db, None, None, 10).unwrap();
        crate::chat_items::enrich_media(&root, &db, &mut media).unwrap();
        let media = serde_json::to_value(media).unwrap();
        assert_eq!(media[0]["how"], "created");
        assert_eq!(media[0]["evidence"], "diff");
        assert_eq!(media[0]["by"]["provider"], "codex");
    }

    #[test]
    fn records_changes_without_inventing_a_model_after_an_unfinished_reply() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        fs::create_dir_all(root.join("zettels")).unwrap();
        fs::create_dir_all(root.join("receipts")).unwrap();
        let db = dir.path().join("test.db");
        crate::storage::initialize(&db, &dir.path().join("backups"), "test").unwrap();
        let id = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let before = capture(&root).unwrap();
        fs::write(root.join("zettels/zet_20261115_120000.md"), "---\nid: zet_20261115_120000\ntitle: 수업\nsummary: 요약\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T12:00:00+09:00'\nupdated_at: '2026-11-15T12:00:00+09:00'\nassets: []\nedges: []\n---\n본문\n").unwrap();
        let after = capture(&root).unwrap();
        let input = crate::archive_receipt::ReceiptCreate {
            tool: "claude".into(),
            model: String::new(),
            chat: Some(id.clone()),
            read: vec![],
            wrote: vec![],
            prompt: "요청".into(),
            changes: String::new(),
        };
        assert!(record(&root, &db, input, &before, &after).unwrap());
        let items =
            serde_json::to_value(crate::chat_items::list(&root, &db, &id).unwrap()).unwrap();
        assert_eq!(items["documents"][0]["how"], "created");
        assert_eq!(items["documents"][0]["by"]["provider"], "claude");
        assert!(items["documents"][0]["by"]["model"].is_null());
    }
}
