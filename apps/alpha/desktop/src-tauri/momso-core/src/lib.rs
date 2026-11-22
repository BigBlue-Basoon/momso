//! Shared momso document and skill formats and headless archive operations.
pub mod context;
pub mod document;
pub mod filesystem;
pub mod folders;
pub mod git;
pub mod lock;
pub mod media_format;
pub mod profile_format;
pub mod profile_store;
pub mod receipt_format;
pub mod receipt_store;
pub mod safe_path;
pub mod skill;
pub mod text_encoding;

#[path = "../../src/skills_write.rs"]
mod skills_write;
#[path = "../../src/yoga_dictionary.rs"]
mod yoga_dictionary;

mod skills {
    pub(crate) use crate::skill::{
        is_builtin, parse, render, skill_path, valid_name, SkillHead, BUNDLED_SKILLS,
    };
}

mod process {
    pub(crate) fn hidden_command(path: &std::path::Path) -> std::process::Command {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let mut command = std::process::Command::new(path);
            command.creation_flags(0x08000000);
            command
        }
        #[cfg(not(windows))]
        {
            std::process::Command::new(path)
        }
    }
}

mod archive_location {
    pub(crate) fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
        }
        #[cfg(not(windows))]
        {
            metadata.file_type().is_symlink()
        }
    }
}

mod archive_zet {
    pub(crate) use crate::document::{
        parse, render, valid_asset_id, valid_zet_id, validate_folder,
    };
    pub(crate) static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    pub(crate) fn content_hash(text: &str) -> String {
        crate::document::content_hash(text)
    }
}

mod archive {
    use std::path::{Path, PathBuf};

    pub(crate) fn storage_tool(_root: &Path) -> Result<(PathBuf, bool), String> {
        let output = crate::process::hidden_command(Path::new("git"))
            .arg("--version")
            .output()
            .map_err(|_| "git_unavailable".to_owned())?;
        if !output.status.success() {
            return Err("git_unavailable".into());
        }
        Ok((PathBuf::from("git"), false))
    }

    pub(crate) fn run_storage_tool(
        tool: &Path,
        root: &Path,
        args: &[&str],
    ) -> Result<String, String> {
        let output = crate::process::hidden_command(tool)
            .current_dir(root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map_err(|_| "git_unavailable".to_owned())?;
        if !output.status.success() {
            return Err("git_failed".into());
        }
        String::from_utf8(output.stdout).map_err(|_| "git_output_invalid".into())
    }
}

/// The same transactional skill installer and edits used by desktop.
pub mod skill_storage {
    use std::path::Path;
    pub fn install(root: &Path) -> Result<(), &'static str> {
        let _file_guard = crate::lock::exclusive(root)?;
        crate::skills_write::install_builtins(root, root, crate::skill::BUNDLED_SKILLS)
    }
    pub fn reconcile(root: &Path, name: &str) -> Result<(), &'static str> {
        let _file_guard = crate::lock::exclusive(root)?;
        crate::skills_write::reconcile(root, name)
    }
    pub fn save_external_changes(root: &Path) -> Result<(), &'static str> {
        let _file_guard = crate::lock::exclusive(root)?;
        crate::skills_write::save_external_changes(root, root)
    }
    pub fn fresh_builtin_state_mode(root: &Path) -> Result<bool, &'static str> {
        crate::skills_write::fresh_builtin_state_mode(root)
    }
    pub fn adopt_received_builtin_states(
        tool: &Path,
        root: &Path,
        saved: &Path,
    ) -> Result<(), &'static str> {
        let _file_guard = crate::lock::exclusive(root)?;
        crate::skills_write::adopt_received_builtin_states(tool, root, saved)
    }
    pub fn create(
        root: &Path,
        name: &str,
        title: String,
        description: String,
        body: String,
    ) -> Result<(), &'static str> {
        let _file_guard = crate::lock::exclusive(root)?;
        crate::skills_write::core_create(root, name, title, description, body)
    }
    pub fn update(
        root: &Path,
        name: &str,
        changes: serde_json::Value,
        expected_hash: String,
    ) -> Result<(), &'static str> {
        let _file_guard = crate::lock::exclusive(root)?;
        let changes = serde_json::from_value(changes).map_err(|_| "invalid_input")?;
        crate::skills_write::core_update(root, name, changes, expected_hash)
    }
    pub fn set_enabled(root: &Path, name: &str, enabled: bool) -> Result<(), &'static str> {
        let _file_guard = crate::lock::exclusive(root)?;
        crate::skills_write::core_set_enabled(root, name, enabled)
    }
    pub fn remove(root: &Path, name: &str) -> Result<(), &'static str> {
        let _file_guard = crate::lock::exclusive(root)?;
        crate::skills_write::core_remove(root, name)
    }
}

/// The existing desktop dictionary implementation, reused without a webview.
pub mod dictionary {
    use std::path::Path;

    pub fn initialize(root: &Path) -> Result<(), String> {
        let _file_guard = crate::lock::exclusive(root).map_err(str::to_owned)?;
        crate::yoga_dictionary::initialize(root)
    }

    pub fn list(root: &Path) -> Result<serde_json::Value, String> {
        let result = crate::yoga_dictionary::list(root)?;
        serde_json::to_value(result).map_err(|_| "dictionary_serialize_failed".to_owned())
    }

    pub fn known_bundle_version(content: &str) -> Option<u64> {
        crate::yoga_dictionary::known_bundle_version(content)
    }
    pub fn user_revision(root: &Path) -> Result<String, String> {
        crate::yoga_dictionary::user_revision(root)
    }
    pub fn term_label(root: &Path, id: &str) -> Result<String, String> {
        crate::yoga_dictionary::term_label(root, id)
    }
    pub fn delete_preview(root: &Path, id: &str) -> Result<(String, bool, String), String> {
        crate::yoga_dictionary::delete_preview(root, id)
    }
    pub fn paths() -> Vec<String> {
        crate::yoga_dictionary::paths()
    }
    pub fn is_path(path: &str) -> bool {
        crate::yoga_dictionary::is_path(path)
    }
    pub fn defaults_unchanged(root: &Path) -> bool {
        crate::yoga_dictionary::defaults_unchanged(root)
    }
    pub fn edit_term(root: &Path, request: serde_json::Value) -> Result<String, String> {
        let _file_guard = crate::lock::exclusive(root).map_err(str::to_owned)?;
        let edit = serde_json::from_value(request).map_err(|_| "invalid_input".to_owned())?;
        crate::yoga_dictionary::edit_term(root, edit)
    }
    pub fn delete_term(root: &Path, id: &str, revision: &str) -> Result<&'static str, String> {
        let _file_guard = crate::lock::exclusive(root).map_err(str::to_owned)?;
        crate::yoga_dictionary::delete_term(root, id, revision)
    }
    pub fn edit_person(root: &Path, request: serde_json::Value) -> Result<String, String> {
        let _file_guard = crate::lock::exclusive(root).map_err(str::to_owned)?;
        let edit = serde_json::from_value(request).map_err(|_| "invalid_input".to_owned())?;
        crate::yoga_dictionary::edit_person(root, edit)
    }
    pub fn delete_person(root: &Path, id: &str, revision: &str) -> Result<(), String> {
        let _file_guard = crate::lock::exclusive(root).map_err(str::to_owned)?;
        crate::yoga_dictionary::delete_person(root, id, revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_root_installs_existing_bundles_and_preserves_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("momso");
        filesystem::initialize(&root).unwrap();
        let list = dictionary::list(&root).unwrap();
        assert!(list["terms"].as_array().unwrap().len() > 200);
        assert!(serde_json::to_string(&list).unwrap().contains("할라사나"));
        let (head, body) = filesystem::read_skill(&root, "marketing-preparation").unwrap();
        assert_eq!(head.name, "marketing-preparation");
        assert!(!body.is_empty());
        let path = root.join("skills/marketing-preparation/SKILL.md");
        let custom = std::fs::read_to_string(&path).unwrap() + "\n사용자 내용\n";
        std::fs::write(&path, &custom).unwrap();
        let revision = dictionary::user_revision(&root).unwrap();
        let id = dictionary::edit_term(&root, serde_json::json!({"addVariants":["내 요가말"],"removeVariants":[],"expectedRevision":revision})).unwrap();
        assert!(id.starts_with("user-"));
        filesystem::initialize(&root).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), custom);
        assert!(serde_json::to_string(&dictionary::list(&root).unwrap())
            .unwrap()
            .contains("내 요가말"));
        assert!(filesystem::read_callable_skill(&root, "conversation-import").is_ok());
        skill_storage::set_enabled(&root, "conversation-import", false).unwrap();
        assert!(matches!(
            filesystem::read_callable_skill(&root, "conversation-import"),
            Err("disabled")
        ));
        skill_storage::set_enabled(&root, "conversation-import", true).unwrap();
        assert!(filesystem::read_callable_skill(&root, "conversation-import").is_ok());
    }

    #[test]
    fn document_format_create_read_and_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("momso");
        filesystem::initialize(&root).unwrap();
        let head = document::ZetHead {
            id: "zet_20261121_120000".into(),
            title: "수업".into(),
            summary: "기록".into(),
            kind: "note".into(),
            status: "draft".into(),
            created_at: "2026-11-21T12:00:00+00:00".into(),
            updated_at: "2026-11-21T12:00:00+00:00".into(),
            assets: vec![],
            edges: vec![],
            folder: "수업/기록".into(),
            original_header: None,
        };
        filesystem::create_document(&root, &head, "첫째\n").unwrap();
        assert_eq!(folders::folders(&root).unwrap(), vec!["수업", "수업/기록"]);
        folders::create_folder(&root, "홍보/이번 주").unwrap();
        assert!(folders::folders(&root)
            .unwrap()
            .contains(&"홍보/이번 주".to_owned()));
        let before = filesystem::read_document(&root, &head.id).unwrap();
        let hash = document::content_hash(&before);
        assert!(filesystem::update_document(&root, &head, "변경\n", "wrong").is_err());
        assert_eq!(filesystem::read_document(&root, &head.id).unwrap(), before);
        filesystem::update_document(&root, &head, "변경\n", &hash).unwrap();
        assert!(filesystem::read_document(&root, &head.id)
            .unwrap()
            .ends_with("변경\n"));
        assert_eq!(
            filesystem::create_document(&root, &head, "dup"),
            Err("already_exists")
        );
        assert!(filesystem::read_document(&root, "../outside").is_err());
        let crlf = before.replace('\n', "\r\n");
        let (parsed, body) = document::parse(&crlf).unwrap();
        assert_eq!(document::render(&parsed, body).unwrap(), crlf);
        assert!(folders::rename_folder(&root, "수업", "수업/아래").is_err());
        folders::rename_folder(&root, "수업", "운동").unwrap();
        let renamed = filesystem::read_document(&root, &head.id).unwrap();
        assert_eq!(document::parse(&renamed).unwrap().0.folder, "운동/기록");
        assert!(folders::folders(&root)
            .unwrap()
            .contains(&"홍보/이번 주".to_owned()));
    }

    #[test]
    fn concurrent_document_updates_do_not_overwrite_each_other() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("momso");
        filesystem::initialize(&root).unwrap();
        let head = document::ZetHead {
            id: "zet_20261121_120000".into(),
            title: "수업".into(),
            summary: "기록".into(),
            kind: "note".into(),
            status: "draft".into(),
            created_at: "2026-11-21T12:00:00+00:00".into(),
            updated_at: "2026-11-21T12:00:00+00:00".into(),
            assets: vec![],
            edges: vec![],
            folder: "".into(),
            original_header: None,
        };
        filesystem::create_document(&root, &head, "처음").unwrap();
        let content = filesystem::read_document(&root, &head.id).unwrap();
        let hash = document::content_hash(&content);
        let mut tasks = Vec::new();
        for text in ["첫째", "둘째"] {
            let root = root.clone();
            let head = head.clone();
            let hash = hash.clone();
            tasks.push(std::thread::spawn(move || {
                filesystem::update_document(&root, &head, text, &hash)
            }));
        }
        let results: Vec<_> = tasks.into_iter().map(|task| task.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err("conflict")))
                .count(),
            1
        );
    }

    #[test]
    fn archive_lock_blocks_another_process_until_release() {
        const CHILD: &str = "MOMSO_CORE_LOCK_TEST_ROOT";
        if let Some(root) = std::env::var_os(CHILD) {
            assert!(matches!(
                lock::try_exclusive(std::path::Path::new(&root)),
                Err("lock_busy")
            ));
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let guard = lock::exclusive(tmp.path()).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::archive_lock_blocks_another_process_until_release",
                "--test-threads=1",
            ])
            .env(CHILD, tmp.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        drop(guard);
        assert!(lock::try_exclusive(tmp.path()).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn linked_skill_and_document_cannot_escape_root() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("momso");
        filesystem::initialize(&root).unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        symlink(&outside, root.join("skills/escape")).unwrap();
        assert!(filesystem::read_skill(&root, "escape").is_err());
        let id = "zet_20261121_120000";
        symlink(
            outside.join("secret"),
            root.join("zettels").join(format!("{id}.md")),
        )
        .unwrap();
        assert!(filesystem::read_document(&root, id).is_err());
    }

    #[test]
    fn local_git_save_preserves_unrelated_staged_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("momso");
        filesystem::initialize(&root).unwrap();
        let document = "zettels/zet_20261121_120000.md";
        std::fs::write(root.join(document), "첫 번째\n").unwrap();
        assert!(git::save(&root, &[document], "문서 저장").unwrap());
        std::fs::write(root.join("other.txt"), "unrelated").unwrap();
        assert!(std::process::Command::new("git")
            .current_dir(&root)
            .args(["add", "other.txt"])
            .status()
            .unwrap()
            .success());
        assert!(git::changed_paths(&root)
            .unwrap()
            .contains(&"other.txt".to_owned()));
        std::fs::write(root.join(document), "두 번째\n").unwrap();
        assert!(git::save(&root, &[document], "문서 수정").unwrap());
        let changed = std::process::Command::new("git")
            .current_dir(&root)
            .args(["diff", "--cached", "--name-only"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8(changed.stdout).unwrap().trim(),
            "other.txt"
        );
        assert_eq!(
            git::save(&root, &["../other.txt"], "저장"),
            Err("invalid_path")
        );
        assert!(!git::save(&root, &[document], "문서 저장").unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn known_dictionary_upgrade_respects_lock_and_preserves_custom_base() {
        use fs2::FileExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("momso");
        std::fs::create_dir(&root).unwrap();
        git::initialize(&root).unwrap();
        let dir = root.join("dictionary");
        std::fs::create_dir(&dir).unwrap();
        let base = dir.join("yoga.json");
        let old = include_str!("../../bundled-dictionary/yoga-v6.json");
        std::fs::write(&base, old).unwrap();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&base)
            .unwrap();
        held.lock_exclusive().unwrap();
        assert_eq!(
            dictionary::initialize(&root).unwrap_err(),
            "dictionary_busy"
        );
        FileExt::unlock(&held).unwrap();
        dictionary::initialize(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(&base).unwrap(),
            include_str!("../../bundled-dictionary/yoga.json")
        );
        let custom = format!("{}\n", std::fs::read_to_string(&base).unwrap());
        std::fs::write(&base, &custom).unwrap();
        dictionary::initialize(&root).unwrap();
        assert_eq!(std::fs::read_to_string(&base).unwrap(), custom);
    }
}
