//! Skill file format and bundled skill installation.
use serde::{Deserialize, Serialize};
use std::path::Path;

pub(crate) const BUNDLED_SKILLS: &[(&str, &str)] = &[
    (
        "marketing-preparation",
        include_str!("../bundled-skills/marketing-preparation/SKILL.md"),
    ),
    (
        "naver-smartplace",
        include_str!("../bundled-skills/naver-smartplace/SKILL.md"),
    ),
    (
        "onstudio-operations",
        include_str!("../bundled-skills/onstudio-operations/SKILL.md"),
    ),
    (
        "studiomate-operations",
        include_str!("../bundled-skills/studiomate-operations/SKILL.md"),
    ),
    (
        "soomgo-operations",
        include_str!("../bundled-skills/soomgo-operations/SKILL.md"),
    ),
    (
        "frip-host-operations",
        include_str!("../bundled-skills/frip-host-operations/SKILL.md"),
    ),
    (
        "sssd-host-operations",
        include_str!("../bundled-skills/sssd-host-operations/SKILL.md"),
    ),
    (
        "conversation-import",
        include_str!("../bundled-skills/conversation-import/SKILL.md"),
    ),
    (
        "github-connect",
        include_str!("../bundled-skills/github-connect/SKILL.md"),
    ),
    (
        "object-store-connect",
        include_str!("../bundled-skills/object-store-connect/SKILL.md"),
    ),
    (
        "bodycodi-operations",
        include_str!("../bundled-skills/bodycodi-operations/SKILL.md"),
    ),
];

pub(crate) fn install_bundled(root: &Path, resources: &Path) -> Result<(), &'static str> {
    let result = install(root, resources, BUNDLED_SKILLS);
    if result == Err("save_failed") && existing_bundled_skills_are_readable(root) {
        // A failed skill transaction has already rolled back its files and
        // index; retain the usable previous skills and retry on next startup.
        log::warn!("Bundled skill update deferred after a storage failure");
        return Ok(());
    }
    result
}

fn existing_bundled_skills_are_readable(root: &Path) -> bool {
    BUNDLED_SKILLS.iter().all(|(name, _)| {
        skill_path(root, "skills", name)
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .is_some_and(|content| parse(name, &content).is_ok())
    })
}

fn install(root: &Path, resources: &Path, bundled: &[(&str, &str)]) -> Result<(), &'static str> {
    crate::skills_write::install_builtins(root, resources, bundled)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SkillHead {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) title: String,
}

pub(crate) fn valid_name(name: &str) -> bool {
    let reserved = matches!(name, "con" | "prn" | "aux" | "nul" | "conin$" | "conout$")
        || name
            .strip_prefix("com")
            .or_else(|| name.strip_prefix("lpt"))
            .is_some_and(|suffix| {
                matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            });
    !name.is_empty()
        && !reserved
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub(crate) fn parse(name: &str, content: &str) -> Result<(SkillHead, String), &'static str> {
    if !valid_name(name) {
        return Err("invalid_name");
    }
    let (yaml, body) = if let Some(rest) = content.strip_prefix("---\r\n") {
        rest.split_once("\r\n---\r\n").ok_or("invalid_format")?
    } else {
        content
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .ok_or("invalid_format")?
    };
    let head: SkillHead = serde_yaml::from_str(yaml).map_err(|_| "invalid_format")?;
    if head.name != name || head.title.trim().is_empty() || head.description.trim().is_empty() {
        return Err("invalid_format");
    }
    Ok((head, body.to_owned()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillSummary {
    name: String,
    title: String,
    description: String,
    enabled: bool,
    available: bool,
    unavailable_reason: Option<&'static str>,
    builtin: bool,
    updated_at: Option<String>,
    providers: Vec<&'static str>,
    sync_state: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillRead {
    #[serde(flatten)]
    summary: SkillSummary,
    body: String,
    content_hash: String,
}

pub(crate) fn skill_path(
    root: &Path,
    folder: &str,
    name: &str,
) -> Result<std::path::PathBuf, &'static str> {
    if !valid_name(name) {
        return Err("invalid_name");
    }
    let mut path = root.to_path_buf();
    for part in folder.split('/').chain([name, "SKILL.md"]) {
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if crate::archive_location::is_link_or_reparse(&meta) => {
                return Err("conflict")
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err("io_error"),
        }
    }
    Ok(path)
}

fn unavailable_reason(
    _root: &Path,
    name: &str,
    enabled: bool,
    conflict: bool,
    browser_available: bool,
) -> Option<&'static str> {
    if conflict {
        Some("skill_conflict")
    } else if !enabled {
        Some("disabled")
    } else if crate::browser_gateway::builtin_browser_name(name) && !browser_available {
        Some("browser_unavailable")
    } else {
        None
    }
}
fn read(root: &Path, name: &str) -> Result<SkillRead, &'static str> {
    match crate::skills_write::reconcile(root, name) {
        Ok(()) | Err("conflict") => (),
        Err(error) => return Err(error),
    }
    let path = skill_path(root, "skills", name)?;
    let content = match std::fs::read_to_string(&path) {
        Ok(value) => value,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err("not_found"),
        Err(_) => return Err("io_error"),
    };
    let (head, body) = parse(name, &content)?;
    let mut providers = Vec::new();
    let mut found = 0;
    let mut conflict = false;
    for (folder, tools) in [
        (".agents/skills", &["codex", "antigravity"][..]),
        (".claude/skills", &["claude"][..]),
    ] {
        let copy = skill_path(root, folder, name)?;
        match std::fs::read_to_string(copy) {
            Ok(value) => {
                found += 1;
                providers.extend_from_slice(tools);
                conflict |= value != content;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err("io_error"),
        }
    }
    if found == 1 {
        conflict = true;
    }
    let updated_at = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(|time| chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339());
    let content_hash = crate::archive_zet::content_hash(&content);
    let enabled = found == 2 && !conflict;
    let unavailable_reason = unavailable_reason(
        root,
        name,
        enabled,
        conflict,
        crate::browser_driver::available(),
    );
    Ok(SkillRead {
        summary: SkillSummary {
            name: name.into(),
            title: head.title,
            description: head.description,
            enabled,
            available: unavailable_reason.is_none(),
            unavailable_reason,
            builtin: BUNDLED_SKILLS.iter().any(|(builtin, _)| *builtin == name),
            updated_at,
            providers,
            sync_state: if conflict {
                "conflict"
            } else if found == 0 {
                "disabled"
            } else {
                "ok"
            },
        },
        body,
        content_hash,
    })
}

pub(crate) fn callable(root: &Path, name: &str) -> Result<bool, &'static str> {
    Ok(read(root, name)?.summary.available)
}

pub(crate) fn call_unavailable_reason(
    root: &Path,
    name: &str,
) -> Result<Option<&'static str>, &'static str> {
    Ok(read(root, name)?.summary.unavailable_reason)
}

#[tauri::command]
pub(crate) async fn skill_read(
    state: tauri::State<'_, crate::AppState>,
    name: String,
) -> Result<SkillRead, String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    tauri::async_runtime::spawn_blocking(move || read(&root, &name).map_err(str::to_owned))
        .await
        .map_err(|_| "io_error".to_string())?
}

#[tauri::command]
pub(crate) async fn skill_list(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<SkillSummary>, String> {
    let _timer = crate::startup_timing::start("skill_list");
    let root = state.paths.archive.current.clone().read_owned().await;
    tauri::async_runtime::spawn_blocking(move || list(&root).map_err(str::to_owned))
        .await
        .map_err(|_| "io_error".to_string())?
}

fn list(root: &Path) -> Result<Vec<SkillSummary>, &'static str> {
    let base = root.join("skills");
    if std::fs::symlink_metadata(&base).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("conflict");
    }
    let entries = match std::fs::read_dir(base) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("io_error"),
    };
    let mut items = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| "io_error")?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "invalid_name")?;
        if valid_name(&name) {
            match read(root, &name) {
                Ok(item) => items.push(item.summary),
                Err("not_found") => (),
                Err(error) => return Err(error),
            }
        }
    }
    items.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(items)
}

pub(crate) fn render(head: &SkillHead, body: &str) -> Result<String, &'static str> {
    if !valid_name(&head.name) {
        return Err("invalid_name");
    }
    if head.title.trim().is_empty() || head.description.trim().is_empty() {
        return Err("invalid_format");
    }
    let yaml = serde_yaml::to_string(head).map_err(|_| "invalid_format")?;
    Ok(format!(
        "---\n{}---\n{}",
        yaml.trim_start_matches("---\n"),
        body
    ))
}

pub(crate) fn is_builtin(name: &str) -> bool {
    BUNDLED_SKILLS.iter().any(|(builtin, _)| *builtin == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_skill_availability_tracks_tool_not_guide_content() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let name = "naver-smartplace";
        let path = root.join("skills").join(name);
        std::fs::create_dir_all(&path).unwrap();
        let content = BUNDLED_SKILLS.iter().find(|(n, _)| *n == name).unwrap().1;
        std::fs::write(path.join("SKILL.md"), content).unwrap();
        assert_eq!(
            unavailable_reason(root, name, true, false, false),
            Some("browser_unavailable")
        );
        assert_eq!(unavailable_reason(root, name, true, false, true), None);
        std::fs::write(path.join("SKILL.md"), format!("{content}\nmodified")).unwrap();
        assert_eq!(unavailable_reason(root, name, true, false, true), None);
        assert_eq!(
            unavailable_reason(root, name, false, false, true),
            Some("disabled")
        );
        assert_eq!(
            unavailable_reason(root, name, false, true, true),
            Some("skill_conflict")
        );
    }
    #[test]
    fn bundled_setup_skill_sources_parse() {
        for &(name, source) in BUNDLED_SKILLS {
            assert_eq!(parse(name, source).unwrap().0.name, name);
        }
    }
    #[test]
    fn bundled_skills_do_not_direct_shell_commands() {
        for &(name, source) in BUNDLED_SKILLS {
            // GitHub login is owned by the separate authentication work.
            if name == "github-connect" {
                continue;
            }
            for command in ["`gh auth", "`wrangler", "`powershell", "`ls", "`cat"] {
                assert!(
                    !source.to_lowercase().contains(command),
                    "{name}: {command}"
                );
            }
        }
    }
    #[test]
    fn setup_skills_are_installed_for_all_three_providers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        crate::archive::initialize(&root, dir.path()).unwrap();
        for &(name, _) in BUNDLED_SKILLS {
            let skill = read(&root, name).unwrap();
            assert!(skill.summary.enabled);
            assert!(skill.summary.builtin);
            assert_eq!(skill.summary.providers, ["codex", "antigravity", "claude"]);
            for folder in ["skills", ".agents/skills", ".claude/skills"] {
                assert!(root.join(folder).join(name).join("SKILL.md").is_file());
            }
        }
    }
    #[test]
    fn repeated_builtin_install_keeps_unborn_history_and_user_edits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        crate::archive::initialize(&root, dir.path()).unwrap();
        let tool = crate::archive::storage_tool(dir.path()).unwrap().0;
        let path = root.join("skills/naver-smartplace/SKILL.md");
        let edited = format!(
            "{}\nuser addition\n",
            std::fs::read_to_string(&path).unwrap()
        );
        std::fs::write(&path, &edited).unwrap();
        install_bundled(&root, dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), edited);
        assert!(
            crate::archive::run_storage_tool(&tool, &root, &["rev-parse", "--verify", "HEAD"])
                .is_err()
        );
    }
    #[test]
    fn korean_and_yaml_punctuation_round_trip() {
        let head = SkillHead {
            name: "yoga-1".into(),
            title: "요가: 수업".into(),
            description: "수업 정리\n두 번째 줄".into(),
        };
        let content = render(&head, "지시 본문\n").unwrap();
        assert_eq!(
            parse("yoga-1", &content).unwrap(),
            (head, "지시 본문\n".into())
        );
        let windows = "---\r\nname: yoga-1\r\ndescription: desc\r\ntitle: 제목\r\n---\r\n첫 줄\r\n둘째 줄\r\n";
        assert_eq!(parse("yoga-1", windows).unwrap().1, "첫 줄\r\n둘째 줄\r\n");
    }
    #[test]
    fn names_reject_paths_and_ambiguous_forms() {
        for name in ["", "../x", "Upper", "-a", "a-", "a--b", "한글", "a_b"] {
            assert!(!valid_name(name), "{name}");
        }
        assert!(valid_name("yoga-1"));
    }
    #[test]
    fn list_and_read_report_provider_presence_and_conflicts() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        assert!(list(root).unwrap().is_empty());
        let text = "---\nname: probe\ndescription: 설명\ntitle: 제목\n---\n본문\n";
        for folder in ["skills", ".agents/skills", ".claude/skills"] {
            let dir = root.join(folder).join("probe");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), text).unwrap();
        }
        let skill = read(root, "probe").unwrap();
        assert_eq!(skill.body, "본문\n");
        assert!(skill.summary.enabled);
        assert_eq!(skill.summary.providers, ["codex", "antigravity", "claude"]);
        assert_eq!(skill.summary.sync_state, "ok");
        std::fs::write(root.join(".claude/skills/probe/SKILL.md"), "changed").unwrap();
        let conflict = list(root).unwrap();
        assert!(!conflict[0].enabled);
        assert_eq!(conflict[0].sync_state, "conflict");
        assert_eq!(
            std::fs::read_to_string(root.join(".claude/skills/probe/SKILL.md")).unwrap(),
            "changed"
        );
        std::fs::remove_file(root.join(".agents/skills/probe/SKILL.md")).unwrap();
        std::fs::remove_file(root.join(".claude/skills/probe/SKILL.md")).unwrap();
        assert_eq!(read(root, "probe").unwrap().summary.sync_state, "disabled");
    }
    #[test]
    fn bundled_updates_only_an_untouched_previous_version() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        crate::archive::initialize(&root, dir.path()).unwrap();
        let old = "---\nname: probe\ndescription: Test\ntitle: Test\n---\nold";
        let new = "---\nname: probe\ndescription: Test\ntitle: Test\n---\nnew";
        install(&root, dir.path(), &[("probe", old)]).unwrap();
        let tool = crate::archive::storage_tool(dir.path()).unwrap().0;
        let tracked = crate::archive::run_storage_tool(
            &tool,
            &root,
            &["ls-tree", "-r", "--name-only", "HEAD"],
        )
        .unwrap();
        for path in [
            "skills/probe/SKILL.md",
            ".agents/skills/probe/SKILL.md",
            ".claude/skills/probe/SKILL.md",
        ] {
            assert!(tracked.lines().any(|line| line == path));
        }
        let file = root.join("skills/probe/SKILL.md");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), old);
        let agent = root.join(".agents/skills/probe/SKILL.md");
        let claude = root.join(".claude/skills/probe/SKILL.md");
        assert_eq!(std::fs::read_to_string(&agent).unwrap(), old);
        assert_eq!(std::fs::read_to_string(&claude).unwrap(), old);
        install(&root, dir.path(), &[("probe", new)]).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), new);
        assert_eq!(std::fs::read_to_string(&agent).unwrap(), new);
        assert_eq!(std::fs::read_to_string(&claude).unwrap(), new);
        std::fs::write(&file, old).unwrap();
        install(&root, dir.path(), &[("probe", new)]).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), old);
        crate::skills_write::disable_for_test(&root, "probe").unwrap();
        install(&root, dir.path(), &[("probe", new)]).unwrap();
        assert!(!agent.exists());
        assert!(!claude.exists());
    }
    #[test]
    fn builtin_failure_restores_files_and_tracking_state() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        crate::archive::initialize(&root, dir.path()).unwrap();
        let old = "---\nname: probe\ndescription: Test\ntitle: Test\n---\nold";
        let new = "---\nname: probe\ndescription: Test\ntitle: Test\n---\nnew";
        install(&root, dir.path(), &[("probe", old)]).unwrap();
        let state = std::fs::read(dir.path().join("skill-builtin-state.json")).unwrap();
        std::fs::write(root.join(".git/hooks/pre-commit"), "#!/bin/sh\nexit 1\n").unwrap();
        assert_eq!(
            install(&root, dir.path(), &[("probe", new)]),
            Err("save_failed")
        );
        assert_eq!(
            std::fs::read_to_string(root.join("skills/probe/SKILL.md")).unwrap(),
            old
        );
        assert_eq!(
            std::fs::read_to_string(root.join(".agents/skills/probe/SKILL.md")).unwrap(),
            old
        );
        assert_eq!(
            std::fs::read_to_string(root.join(".claude/skills/probe/SKILL.md")).unwrap(),
            old
        );
        assert_eq!(
            std::fs::read(dir.path().join("skill-builtin-state.json")).unwrap(),
            state
        );
    }

    #[test]
    fn startup_defers_failed_builtin_save_only_with_existing_readable_skills() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        crate::archive::initialize(&root, dir.path()).unwrap();
        crate::archive::profile_set(&root, dir.path(), "Fixture".into(), "".into(), "".into())
            .unwrap();
        let (name, current) = BUNDLED_SKILLS[0];
        let old = format!("{current}\nold fixture\n");
        let paths: Vec<String> = ["skills", ".agents/skills", ".claude/skills"]
            .iter()
            .map(|folder| format!("{folder}/{name}/SKILL.md"))
            .collect();
        for path in &paths {
            std::fs::write(root.join(path), &old).unwrap();
        }
        let hash = crate::archive_zet::content_hash(&old);
        let state_path = dir.path().join("skill-builtin-state.json");
        let mut state: std::collections::BTreeMap<String, String> =
            serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
        state.insert(name.to_owned(), hash.clone());
        std::fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
        let provider_state = dir.path().join("skill-state").join(format!("{name}.json"));
        let mut state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&provider_state).unwrap()).unwrap();
        state["hash"] = hash.into();
        std::fs::write(provider_state, serde_json::to_vec(&state).unwrap()).unwrap();
        let tool = crate::archive::storage_tool(dir.path()).unwrap().0;
        let mut args = vec!["add", "--"];
        args.extend(paths.iter().map(String::as_str));
        crate::archive::run_storage_tool(&tool, &root, &args).unwrap();
        let mut args = vec![
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--only",
            "-m",
            "fixture",
            "--",
        ];
        args.extend(paths.iter().map(String::as_str));
        crate::archive::run_storage_tool(&tool, &root, &args).unwrap();
        let head = crate::archive::run_storage_tool(&tool, &root, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(
            root.join(".git/hooks/pre-commit"),
            "#!/bin/sh\nprintf x > .git/d20-hook-ran\nexit 1\n",
        )
        .unwrap();

        assert_eq!(install_bundled(&root, dir.path()), Ok(()));
        assert!(root.join(".git/d20-hook-ran").is_file());
        for path in &paths {
            assert_eq!(std::fs::read_to_string(root.join(path)).unwrap(), old);
        }
        assert_eq!(
            crate::archive::run_storage_tool(&tool, &root, &["rev-parse", "HEAD"]).unwrap(),
            head
        );
        std::fs::remove_file(
            root.join("skills")
                .join(BUNDLED_SKILLS[1].0)
                .join("SKILL.md"),
        )
        .unwrap();
        assert_eq!(install_bundled(&root, dir.path()), Err("save_failed"));
    }
    #[test]
    fn archive_upgrades_only_unchanged_legacy_rules_with_a_profile() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        crate::archive::initialize(&root, dir.path()).unwrap();
        crate::archive::profile_set(&root, dir.path(), "봄".into(), "".into(), "".into()).unwrap();
        let line = "- `skills/` 와 `.agents/skills/`, `.claude/skills/` 는 몸소가 관리한다. AI는 사람이 시켰을 때만 스킬을 고친다.";
        for name in ["AGENTS.md", "CLAUDE.md", "GEMINI.md"] {
            let path = root.join(name);
            let current = std::fs::read_to_string(&path).unwrap();
            let legacy = current
                .replace(&format!("{line}\r\n"), "")
                .replace(&format!("{line}\n"), "");
            std::fs::write(path, legacy).unwrap();
        }
        let tool = crate::archive::storage_tool(dir.path()).unwrap().0;
        crate::archive::run_storage_tool(
            &tool,
            &root,
            &["add", "--", "AGENTS.md", "CLAUDE.md", "GEMINI.md"],
        )
        .unwrap();
        crate::archive::run_storage_tool(
            &tool,
            &root,
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "previous version",
            ],
        )
        .unwrap();
        let custom = root.join("CLAUDE.md");
        std::fs::write(&custom, "custom").unwrap();
        crate::archive::initialize(&root, dir.path()).unwrap();
        assert!(std::fs::read_to_string(root.join("AGENTS.md"))
            .unwrap()
            .contains(line));
        assert!(std::fs::read_to_string(root.join("GEMINI.md"))
            .unwrap()
            .contains(line));
        assert_eq!(std::fs::read_to_string(custom).unwrap(), "custom");
    }
}
