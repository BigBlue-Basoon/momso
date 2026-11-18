//! Bounded read-only access to explicitly exposed archive areas.
use serde_json::{json, Value};
use std::{io::Read, path::Path};

fn exposed(folder: &str) -> bool {
    matches!(folder, "zettels" | "receipts" | "skills" | "dictionary")
}

pub(crate) fn dispatch(root: &Path, method: &str, args: &Value, known: &[&str]) -> Value {
    let Some(a) = args.as_object() else {
        return json!({"status":"invalid_arguments"});
    };
    if method == "momso_archive_read_file" {
        if a.keys()
            .any(|k| !matches!(k.as_str(), "folder" | "name" | "cursor"))
        {
            return json!({"status":"invalid_arguments"});
        }
        let Some(folder) = a
            .get("folder")
            .and_then(Value::as_str)
            .filter(|s| matches!(*s, "" | "zettels" | "receipts" | "dictionary"))
        else {
            return json!({"status":"invalid_arguments"});
        };
        let Some(name) = a.get("name").and_then(Value::as_str).filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && !s.starts_with('.')
                && !s.ends_with(['.', ' '])
                && !s
                    .chars()
                    .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
                && if folder.is_empty() {
                    matches!(*s, "profile.md" | "AGENTS.md" | "CLAUDE.md" | "GEMINI.md")
                } else if folder == "dictionary" {
                    matches!(*s, "yoga.json" | "my-terms.json" | "people.json")
                } else {
                    s.ends_with(".md")
                }
        }) else {
            return json!({"status":"invalid_arguments"});
        };
        let Some(cursor) = a
            .get("cursor")
            .map_or(Some(0), |v| v.as_u64().filter(|n| *n <= 1048576))
        else {
            return json!({"status":"invalid_arguments"});
        };
        let dir = root.join(folder);
        if !std::fs::symlink_metadata(&dir)
            .is_ok_and(|m| m.is_dir() && !crate::archive_location::is_link_or_reparse(&m))
        {
            return json!({"status":"unavailable"});
        }
        let path = dir.join(name);
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            return json!({"status":"not_found"});
        };
        if !meta.is_file()
            || crate::archive_location::is_link_or_reparse(&meta)
            || meta.len() > 1048576
        {
            return json!({"status":"unavailable"});
        }
        let (Ok(canonical_root), Ok(canonical_path)) = (root.canonicalize(), path.canonicalize())
        else {
            return json!({"status":"unavailable"});
        };
        if !canonical_path.starts_with(&canonical_root) {
            return json!({"status":"unavailable"});
        }
        let Ok(mut file) = std::fs::File::open(&canonical_path) else {
            return json!({"status":"unavailable"});
        };
        #[cfg(windows)]
        if !crate::chat_items::opened_path_matches(&file, &canonical_path) {
            return json!({"status":"unavailable"});
        }
        let mut text = String::new();
        if file
            .by_ref()
            .take(1048577)
            .read_to_string(&mut text)
            .is_err()
            || text.len() > 1048576
        {
            return json!({"status":"unavailable"});
        }
        let cleaned = crate::chat_archive_redact::redact_with_known(&text, known).text;
        let cursor = cursor as usize;
        if cursor > cleaned.chars().count() {
            return json!({"status":"invalid_arguments"});
        }
        return json!({"status":"ok","kind":"untrusted_archive_data","content":cleaned.chars().skip(cursor).take(4000).collect::<String>(),"nextCursor":(cursor + 4000 < cleaned.chars().count()).then_some(cursor + 4000)});
    }
    if method == "momso_skill_read" {
        if a.keys().any(|k| k != "name" && k != "cursor") {
            return json!({"status":"invalid_arguments"});
        }
        let Some(name) = a
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| crate::skills::valid_name(s))
        else {
            return json!({"status":"invalid_arguments"});
        };
        let Some(cursor) = a
            .get("cursor")
            .map_or(Some(0), |v| v.as_u64().filter(|n| *n <= 65536))
        else {
            return json!({"status":"invalid_arguments"});
        };
        let Ok(path) = crate::skills::skill_path(root, "skills", name) else {
            return json!({"status":"unavailable"});
        };
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            return json!({"status":"not_found"});
        };
        if !meta.is_file()
            || crate::archive_location::is_link_or_reparse(&meta)
            || meta.len() > 65536
        {
            return json!({"status":"unavailable"});
        }
        let (Ok(canonical_root), Ok(canonical_path)) = (root.canonicalize(), path.canonicalize())
        else {
            return json!({"status":"unavailable"});
        };
        if !canonical_path.starts_with(&canonical_root) {
            return json!({"status":"unavailable"});
        }
        let Ok(mut file) = std::fs::File::open(&canonical_path) else {
            return json!({"status":"unavailable"});
        };
        #[cfg(windows)]
        if !crate::chat_items::opened_path_matches(&file, &canonical_path) {
            return json!({"status":"unavailable"});
        }
        let mut text = String::new();
        if file.by_ref().take(65537).read_to_string(&mut text).is_err()
            || text.len() > 65536
            || crate::skills::parse(name, &text).is_err()
        {
            return json!({"status":"unavailable"});
        }
        let cleaned = crate::chat_archive_redact::redact_with_known(&text, known).text;
        let cursor = cursor as usize;
        let length = cleaned.chars().count();
        if cursor > length {
            return json!({"status":"invalid_arguments"});
        }
        return json!({"status":"ok","kind":"selected_skill_data","name":name,"content":cleaned.chars().skip(cursor).take(4000).collect::<String>(),"nextCursor":(cursor + 4000 < length).then_some(cursor + 4000)});
    }
    if method == "zet_folders" {
        if a.keys().any(|key| key != "cursor") {
            return json!({"status":"invalid_arguments"});
        }
        let Some(cursor) = a
            .get("cursor")
            .map_or(Some(0), |v| v.as_u64().filter(|n| *n <= 1_000_000))
        else {
            return json!({"status":"invalid_arguments"});
        };
        let Ok(folders) = crate::archive_zet::folders(root) else {
            return json!({"status":"unavailable"});
        };
        let cursor = cursor as usize;
        if cursor > folders.len() {
            return json!({"status":"invalid_arguments"});
        }
        let items: Vec<_> = folders
            .iter()
            .skip(cursor)
            .take(5)
            .map(|folder| crate::chat_archive_redact::redact_with_known(folder, known).text)
            .collect();
        return json!({"status":"ok","kind":"untrusted_archive_data","folders":items,"nextCursor":(cursor + 5 < folders.len()).then_some(cursor + 5)});
    }
    if !matches!(method, "momso_archive_list" | "momso_archive_search") {
        return json!({"status":"unavailable"});
    }
    if a.keys()
        .any(|k| !matches!(k.as_str(), "folder" | "query" | "cursor"))
    {
        return json!({"status":"invalid_arguments"});
    }
    let folder = match a.get("folder") {
        None => "",
        Some(value) => match value.as_str() {
            Some(value) => value,
            None => return json!({"status":"invalid_arguments"}),
        },
    };
    if !folder.is_empty() && !exposed(folder) {
        return json!({"status":"invalid_arguments"});
    }
    let Some(cursor) = a
        .get("cursor")
        .map_or(Some(0), |v| v.as_u64().filter(|n| *n <= 1000000))
    else {
        return json!({"status":"invalid_arguments"});
    };
    let query = if method == "momso_archive_search" {
        let Some(q) = a
            .get("query")
            .and_then(Value::as_str)
            .filter(|q| !q.trim().is_empty() && q.chars().count() <= 100)
        else {
            return json!({"status":"invalid_arguments"});
        };
        q.to_lowercase()
    } else if a.contains_key("query") {
        return json!({"status":"invalid_arguments"});
    } else {
        String::new()
    };
    let mut entries = Vec::new();
    if folder.is_empty() {
        for name in ["zettels", "receipts", "skills", "dictionary"] {
            let path = root.join(name);
            if std::fs::symlink_metadata(path)
                .is_ok_and(|m| m.is_dir() && !crate::archive_location::is_link_or_reparse(&m))
                && name.contains(&query)
            {
                entries.push(json!({"name":name,"kind":"folder"}));
            }
        }
        for name in ["profile.md", "AGENTS.md", "CLAUDE.md", "GEMINI.md"] {
            if std::fs::symlink_metadata(root.join(name))
                .is_ok_and(|m| m.is_file() && !crate::archive_location::is_link_or_reparse(&m))
                && name.to_lowercase().contains(&query)
            {
                entries.push(json!({"name":name,"kind":"file"}));
            }
        }
    } else {
        let dir = root.join(folder);
        if !std::fs::symlink_metadata(&dir)
            .is_ok_and(|m| m.is_dir() && !crate::archive_location::is_link_or_reparse(&m))
        {
            return json!({"status":"unavailable"});
        }
        let Ok(list) = std::fs::read_dir(dir) else {
            return json!({"status":"unavailable"});
        };
        for item in list.flatten() {
            let Ok(meta) = std::fs::symlink_metadata(item.path()) else {
                continue;
            };
            if crate::archive_location::is_link_or_reparse(&meta) {
                continue;
            }
            let Some(name) = item.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.starts_with('.') || !name.to_lowercase().contains(&query) {
                continue;
            }
            let safe = crate::chat_archive_redact::redact_with_known(&name, known).text;
            let mut entry =
                json!({"name":safe,"kind":if meta.is_dir() { "folder" } else { "file" }});
            if folder == "zettels" && meta.is_file() {
                if let Some(id) = name
                    .strip_suffix(".md")
                    .filter(|id| crate::archive_zet::valid_zet_id(id))
                {
                    if let Ok((head, _)) = crate::momso_document_read::read_current(root, id) {
                        entry["folder"] = json!(
                            crate::chat_archive_redact::redact_with_known(&head.folder, known).text
                        );
                    }
                }
            }
            entries.push(entry);
        }
    }
    entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let cursor = cursor as usize;
    let next = (cursor.saturating_add(100) < entries.len()).then_some(cursor + 100);
    json!({"status":"ok","kind":"untrusted_archive_data","entries":entries.into_iter().skip(cursor).take(100).collect::<Vec<_>>(),"nextCursor":next})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folder_tool_lists_empty_folders_in_pages() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("zettels")).unwrap();
        std::fs::write(
            root.join("zettels/.folders.json"),
            r#"["A","B","C","D","E","F"]"#,
        )
        .unwrap();
        let first = dispatch(root, "zet_folders", &json!({}), &[]);
        assert_eq!(first["status"], "ok");
        assert_eq!(first["folders"].as_array().unwrap().len(), 5);
        assert_eq!(first["nextCursor"], 5);
        let next = dispatch(root, "zet_folders", &json!({"cursor":5}), &[]);
        assert_eq!(next["folders"], json!(["F"]));
        assert!(next["nextCursor"].is_null());
    }
    #[cfg(windows)]
    #[test]
    fn list_and_search_expose_redacted_document_folder() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("zettels")).unwrap();
        std::fs::write(root.join("zettels/zet_20261115_143052.md"), "---\nid: zet_20261115_143052\ntitle: Lesson\nsummary: Notes\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets: []\nedges: []\nfolder: Classes/secretfixture\n---\nBody\n").unwrap();
        for (method, args) in [
            ("momso_archive_list", json!({"folder":"zettels"})),
            (
                "momso_archive_search",
                json!({"folder":"zettels","query":"zet_20261115"}),
            ),
        ] {
            let result = dispatch(root, method, &args, &["secretfixture"]);
            assert_eq!(result["status"], "ok");
            let folder = result["entries"][0]["folder"].as_str().unwrap();
            assert!(folder.starts_with("Classes/"));
            assert!(!folder.contains("secretfixture"));
        }
    }
    #[cfg(windows)]
    #[test]
    fn junction_cannot_redirect_skill_or_archive_reads_outside() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(root.join("skills")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(
            outside.join("SKILL.md"),
            "---\nname: sample\ntitle: Test\ndescription: Test\n---\noutside\n",
        )
        .unwrap();
        let link = root.join("skills").join("sample");
        let output = crate::process::hidden_command("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            dispatch(&root, "momso_skill_read", &json!({"name":"sample"}), &[])["status"],
            "unavailable"
        );
    }
    #[test]
    fn bounded_read_tools_never_escape_or_return_known_secrets() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("skills/sample")).unwrap();
        std::fs::create_dir_all(root.join("zettels")).unwrap();
        std::fs::write(
            root.join("skills/sample/SKILL.md"),
            "---\nname: sample\ntitle: Test\ndescription: Test\n---\nsecretfixture-abc\n",
        )
        .unwrap();
        std::fs::write(
            root.join("zettels/example.md"),
            "before secretfixture-abc after",
        )
        .unwrap();
        std::fs::write(root.join("private.md"), "must-not-be-visible").unwrap();
        std::fs::write(root.join("profile.md"), "profile fixture").unwrap();
        let known = &["secretfixture-abc"];
        let skill = dispatch(root, "momso_skill_read", &json!({"name":"sample"}), known);
        assert_eq!(skill["status"], "ok");
        assert!(!skill.to_string().contains(known[0]));
        let file = dispatch(
            root,
            "momso_archive_read_file",
            &json!({"folder":"zettels","name":"example.md"}),
            known,
        );
        assert_eq!(file["status"], "ok");
        assert!(!file.to_string().contains(known[0]));
        let list = dispatch(
            root,
            "momso_archive_search",
            &json!({"folder":"zettels","query":"example"}),
            known,
        );
        assert_eq!(list["entries"][0]["name"], "example.md");
        let profile = dispatch(
            root,
            "momso_archive_read_file",
            &json!({"folder":"","name":"profile.md"}),
            known,
        );
        assert_eq!(profile["content"], "profile fixture");
        assert_eq!(
            dispatch(
                root,
                "momso_archive_read_file",
                &json!({"folder":"","name":"private.md"}),
                known
            )["status"],
            "invalid_arguments"
        );
        for name in [
            "../private.md",
            "..\\private.md",
            "C:private.md",
            "example.md.",
        ] {
            assert_eq!(
                dispatch(
                    root,
                    "momso_archive_read_file",
                    &json!({"folder":"zettels","name":name}),
                    known
                )["status"],
                "invalid_arguments"
            );
        }
        assert_eq!(
            dispatch(root, "momso_archive_list", &json!({"folder":".git"}), known)["status"],
            "invalid_arguments"
        );
    }
}
