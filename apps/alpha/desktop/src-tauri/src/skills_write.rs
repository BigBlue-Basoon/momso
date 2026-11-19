//! Transactional skill writes. The archive's Git index and unrelated files are preserved.
use crate::skills::{self, SkillHead};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SkillChanges {
    title: Option<String>,
    description: Option<String>,
    body: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct State {
    hash: String,
    enabled: bool,
}

pub(crate) fn fresh_builtin_state_mode(root: &Path) -> Result<bool, &'static str> {
    let mut present = 0;
    for &(name, content) in crate::skills::BUNDLED_SKILLS {
        let Some(bytes) = snapshot(&state_path(root, name)?)? else {
            continue;
        };
        present += 1;
        let state: State = serde_json::from_slice(&bytes).map_err(|_| "invalid_state")?;
        if !state.enabled || state.hash != crate::archive_zet::content_hash(content) {
            return Err("conflict");
        }
    }
    if present == 0 {
        Ok(false)
    } else if present == crate::skills::BUNDLED_SKILLS.len() {
        Ok(true)
    } else {
        Err("conflict")
    }
}

pub(crate) fn adopt_received_builtin_states(
    tool: &Path,
    root: &Path,
    saved: &Path,
) -> Result<(), &'static str> {
    for &(name, default) in crate::skills::BUNDLED_SKILLS {
        let canonical = relative(name);
        if git(tool, root, &["ls-files", "--", &canonical])?
            .trim()
            .is_empty()
        {
            continue;
        }
        let path = skills::skill_path(root, "skills", name)?;
        let content = snapshot(&path)?.ok_or("conflict")?;
        let Ok(text) = std::str::from_utf8(&content) else {
            continue;
        };
        let state_file = state_path(root, name)?;
        let before = snapshot(&state_file)?.ok_or("conflict")?;
        let agents = skills::skill_path(root, ".agents/skills", name)?;
        let claude = skills::skill_path(root, ".claude/skills", name)?;
        let enabled = snapshot(&agents)?.as_deref() == Some(content.as_slice())
            && snapshot(&claude)?.as_deref() == Some(content.as_slice());
        let next = State {
            hash: crate::archive_zet::content_hash(text),
            enabled,
        };
        let next_bytes = serde_json::to_vec(&next).map_err(|_| "io_error")?;
        let backup = saved.join("skill-state").join(format!("{name}.json"));
        let parent = backup.parent().ok_or("io_error")?;
        match fs::create_dir(parent) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let meta = fs::symlink_metadata(parent).map_err(|_| "io_error")?;
                if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
                    return Err("conflict");
                }
            }
            Err(_) => return Err("io_error"),
        }
        let original = match snapshot(&backup)? {
            Some(original) => original,
            None => {
                let previous: State =
                    serde_json::from_slice(&before).map_err(|_| "invalid_state")?;
                if !previous.enabled || previous.hash != crate::archive_zet::content_hash(default) {
                    return Err("conflict");
                }
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&backup)
                    .map_err(|_| "io_error")?;
                file.write_all(&before).map_err(|_| "io_error")?;
                file.sync_all().map_err(|_| "io_error")?;
                before.clone()
            }
        };
        let previous: State = serde_json::from_slice(&original).map_err(|_| "invalid_state")?;
        if !previous.enabled || previous.hash != crate::archive_zet::content_hash(default) {
            return Err("conflict");
        }
        if before == next_bytes {
            continue;
        }
        if before != original {
            return Err("conflict");
        }
        apply(&[Change {
            path: state_file,
            before: Some(before),
            after: Some(next_bytes),
        }])?;
        #[cfg(test)]
        if std::env::var_os("MOMSO_C28_CRASH_DURING_SKILL").is_some() {
            std::process::exit(73);
        }
    }
    Ok(())
}

struct Change {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}

fn snapshot(path: &Path) -> Result<Option<Vec<u8>>, &'static str> {
    match fs::symlink_metadata(path) {
        Ok(m) if crate::archive_location::is_link_or_reparse(&m) || !m.is_file() => Err("conflict"),
        Ok(_) => fs::read(path).map(Some).map_err(|_| "io_error"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("io_error"),
    }
}

fn put(path: &Path, bytes: Option<&[u8]>) -> Result<(), &'static str> {
    if let Some(bytes) = bytes {
        let parent = path.parent().ok_or("io_error")?;
        fs::create_dir_all(parent).map_err(|_| "io_error")?;
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|_| "io_error")?;
        temp.write_all(bytes).map_err(|_| "io_error")?;
        temp.as_file().sync_all().map_err(|_| "io_error")?;
        temp.persist(path).map_err(|_| "io_error")?;
    } else if path.exists() {
        fs::remove_file(path).map_err(|_| "io_error")?;
    }
    Ok(())
}

fn rollback(changes: &[Change]) -> Result<(), &'static str> {
    for change in changes.iter().rev() {
        if snapshot(&change.path)? != change.after {
            return Err("rollback_failed");
        }
        put(&change.path, change.before.as_deref()).map_err(|_| "rollback_failed")?;
    }
    Ok(())
}

fn apply(changes: &[Change]) -> Result<(), &'static str> {
    for (i, change) in changes.iter().enumerate() {
        if !matches!(snapshot(&change.path), Ok(value) if value == change.before) {
            rollback(&changes[..i])?;
            return Err("conflict");
        }
        if let Err(e) = put(&change.path, change.after.as_deref()) {
            rollback(&changes[..i])?;
            return Err(e);
        }
    }
    Ok(())
}

fn relative(name: &str) -> String {
    format!("skills/{name}/SKILL.md")
}

fn state_path(root: &Path, name: &str) -> Result<PathBuf, &'static str> {
    let parent = root.parent().ok_or("io_error")?;
    let dir = parent.join("skill-state");
    if fs::symlink_metadata(&dir).is_ok_and(|m| crate::archive_location::is_link_or_reparse(&m)) {
        return Err("conflict");
    }
    Ok(dir.join(format!("{name}.json")))
}

fn git(tool: &Path, root: &Path, args: &[&str]) -> Result<String, &'static str> {
    crate::archive::run_storage_tool(tool, root, args).map_err(|error| {
        let operation = if args.contains(&"commit") {
            "commit"
        } else if args.contains(&"add") {
            "add"
        } else if args.contains(&"ls-files") {
            "ls-files"
        } else if args.contains(&"diff") {
            "diff"
        } else {
            "check"
        };
        log::warn!("Skill storage {operation} failed: {error}");
        "save_failed"
    })
}

fn transact(
    root: &Path,
    resources: &Path,
    name: &str,
    changes: Vec<Change>,
    commit: bool,
    builtin_update: bool,
) -> Result<(), &'static str> {
    let canonical = relative(name);
    let mut paths = Vec::new();
    if changes
        .iter()
        .any(|change| change.path == root.join(&canonical))
    {
        paths.push(canonical);
    }
    for folder in [".agents/skills", ".claude/skills"] {
        let item = format!("{folder}/{name}/SKILL.md");
        if changes.iter().any(|change| {
            change.path == root.join(&item) && (change.before.is_some() || change.after.is_some())
        }) {
            paths.push(item);
        }
    }
    if paths.is_empty() {
        return apply(&changes);
    }
    let tool = if commit {
        Some(
            crate::archive::storage_tool(resources)
                .map_err(|_| "save_failed")?
                .0,
        )
    } else {
        None
    };
    let index = if let Some(tool) = &tool {
        let mut entries = Vec::new();
        for path in &paths {
            let staged = git(tool, root, &["diff", "--cached", "--name-only", "--", path])?;
            let index = git(tool, root, &["ls-files", "--stage", "--", path])?;
            if !staged.trim().is_empty() || index.lines().count() > 1 {
                return Err("conflict");
            }
            entries.push(index);
        }
        entries
    } else {
        Vec::new()
    };
    apply(&changes)?;
    if let Some(tool) = &tool {
        let result = (|| {
            let mut add = vec!["add", "-A", "--"];
            add.extend(paths.iter().map(String::as_str));
            git(tool, root, &add)?;
            for change in &changes {
                if snapshot(&change.path)? != change.after {
                    return Err("conflict");
                }
            }
            let mut diff = vec!["diff", "--cached", "--name-only", "--"];
            diff.extend(paths.iter().map(String::as_str));
            if !git(tool, root, &diff)?.trim().is_empty() {
                let mut args = vec![
                    "-c",
                    "user.name=momso",
                    "-c",
                    "user.email=archive@momso.local",
                    "commit",
                    "--only",
                    "-m",
                    if builtin_update {
                        "몸소 업데이트: 기본 스킬 갱신"
                    } else {
                        "Save skill"
                    },
                    "--",
                ];
                args.extend(paths.iter().map(String::as_str));
                git(tool, root, &args)?;
            }
            Ok::<(), &'static str>(())
        })();
        if result.is_err() {
            let mut restored = true;
            for (path, entry) in paths.iter().zip(&index) {
                let result = if let Some(line) = entry.lines().next() {
                    let fields: Vec<_> = line.split_whitespace().collect();
                    fields.len() >= 3
                        && git(
                            tool,
                            root,
                            &[
                                "update-index",
                                "--add",
                                "--cacheinfo",
                                fields[0],
                                fields[1],
                                path,
                            ],
                        )
                        .is_ok()
                } else {
                    git(tool, root, &["update-index", "--force-remove", "--", path]).is_ok()
                };
                restored &= result;
            }
            if rollback(&changes).is_err() || !restored {
                return Err("rollback_failed");
            }
            return Err("save_failed");
        }
    }
    Ok(())
}

fn change(path: PathBuf, after: Option<Vec<u8>>) -> Result<Change, &'static str> {
    let before = snapshot(&path)?;
    Ok(Change {
        path,
        before,
        after,
    })
}

fn state(root: &Path, name: &str, fallback_hash: &str) -> Result<State, &'static str> {
    match snapshot(&state_path(root, name)?)? {
        Some(bytes) => serde_json::from_slice(&bytes).map_err(|_| "invalid_state"),
        None => {
            let agents = snapshot(&skills::skill_path(root, ".agents/skills", name)?)?;
            let claude = snapshot(&skills::skill_path(root, ".claude/skills", name)?)?;
            Ok(State {
                hash: fallback_hash.into(),
                enabled: agents.is_some() || claude.is_some(),
            })
        }
    }
}

pub(crate) fn install_builtins(
    root: &Path,
    resources: &Path,
    bundled: &[(&str, &str)],
) -> Result<(), &'static str> {
    let _lock = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "io_error")?;
    let tool = crate::archive::storage_tool(resources)
        .map_err(|_| "save_failed")?
        .0;
    if git(&tool, root, &["rev-parse", "--is-inside-work-tree"])?.trim() != "true" {
        return Err("save_failed");
    }
    let mut has_history = git(&tool, root, &["rev-parse", "--verify", "HEAD"]).is_ok();
    let clean_tracked = if has_history
        && git(
            &tool,
            root,
            &[
                "diff",
                "--quiet",
                "HEAD",
                "--",
                "skills",
                ".agents/skills",
                ".claude/skills",
            ],
        )
        .is_ok()
    {
        git(
            &tool,
            root,
            &[
                "ls-tree",
                "-r",
                "--name-only",
                "HEAD",
                "--",
                "skills",
                ".agents/skills",
                ".claude/skills",
            ],
        )
        .ok()
        .map(|text| {
            text.lines()
                .map(str::to_owned)
                .collect::<std::collections::HashSet<_>>()
        })
    } else {
        None
    };
    for &(name, text) in bundled {
        install_builtin_locked(
            root,
            resources,
            name,
            text,
            has_history,
            clean_tracked.as_ref(),
        )?;
        if !has_history
            && !crate::skills::BUNDLED_SKILLS
                .iter()
                .any(|(builtin_name, builtin_text)| *builtin_name == name && *builtin_text == text)
        {
            has_history = git(&tool, root, &["rev-parse", "--verify", "HEAD"]).is_ok();
        }
    }
    Ok(())
}

fn install_builtin_locked(
    root: &Path,
    resources: &Path,
    name: &str,
    text: &str,
    has_history: bool,
    clean_tracked: Option<&std::collections::HashSet<String>>,
) -> Result<(), &'static str> {
    skills::parse(name, text)?;
    let path = skills::skill_path(root, "skills", name)?;
    let before = snapshot(&path)?;
    let builtin_path = root
        .parent()
        .ok_or("io_error")?
        .join("skill-builtin-state.json");
    let builtin_before = snapshot(&builtin_path)?;
    let mut builtins: std::collections::BTreeMap<String, String> = match &builtin_before {
        Some(bytes) => serde_json::from_slice(bytes).map_err(|_| "invalid_state")?,
        None => std::collections::BTreeMap::new(),
    };
    if let Some(old) = &before {
        let old_text = std::str::from_utf8(old).map_err(|_| "invalid_format")?;
        if builtins
            .get(name)
            .is_none_or(|hash| hash != &crate::archive_zet::content_hash(old_text))
        {
            return Ok(());
        }
    }
    let content = text.as_bytes().to_vec();
    let current_hash = crate::archive_zet::content_hash(text);
    let state_file = state_path(root, name)?;
    let previous_bytes = snapshot(&state_file)?;
    let previous = match &previous_bytes {
        Some(bytes) => serde_json::from_slice::<State>(bytes).map_err(|_| "invalid_state")?,
        None => State {
            hash: current_hash.clone(),
            enabled: true,
        },
    };
    let mut edits = copies(root, name, &previous.hash, &content, previous.enabled)?;
    let next = State {
        hash: current_hash,
        enabled: previous.enabled,
    };
    edits.push(Change {
        path: state_file,
        before: previous_bytes,
        after: Some(serde_json::to_vec(&next).map_err(|_| "io_error")?),
    });
    edits.insert(
        0,
        Change {
            path,
            before,
            after: Some(content),
        },
    );
    builtins.insert(name.to_owned(), crate::archive_zet::content_hash(text));
    edits.push(Change {
        path: builtin_path,
        before: builtin_before,
        after: Some(serde_json::to_vec(&builtins).map_err(|_| "io_error")?),
    });
    if edits.iter().all(|change| change.before == change.after) {
        if !has_history {
            return Ok(());
        }
        if clean_tracked.is_some_and(|tracked| {
            ["skills", ".agents/skills", ".claude/skills"]
                .into_iter()
                .map(|folder| format!("{folder}/{name}/SKILL.md"))
                .filter(|path| root.join(path).is_file())
                .all(|path| tracked.contains(&path))
        }) {
            return Ok(());
        }
        let tool = crate::archive::storage_tool(resources)
            .map_err(|_| "save_failed")?
            .0;
        let paths: Vec<String> = ["skills", ".agents/skills", ".claude/skills"]
            .into_iter()
            .map(|folder| format!("{folder}/{name}/SKILL.md"))
            .filter(|path| root.join(path).is_file())
            .collect();
        let mut args = vec!["ls-tree", "-r", "--name-only", "HEAD", "--"];
        args.extend(paths.iter().map(String::as_str));
        match git(&tool, root, &args) {
            Ok(tracked)
                if paths
                    .iter()
                    .all(|path| tracked.lines().any(|line| line == path)) =>
            {
                let mut diff = vec!["diff", "--quiet", "HEAD", "--"];
                diff.extend(paths.iter().map(String::as_str));
                if git(&tool, root, &diff).is_ok() {
                    return Ok(());
                }
            }
            Err(_) if git(&tool, root, &["rev-parse", "--verify", "HEAD"]).is_err() => {
                return Ok(())
            }
            _ => {}
        }
    }
    // A freshly initialized archive must remain unborn until the first remote
    // receive. Otherwise default skills create an unrelated local history.
    let is_original_builtin = crate::skills::BUNDLED_SKILLS
        .iter()
        .any(|(builtin_name, builtin_text)| *builtin_name == name && *builtin_text == text);
    transact(
        root,
        resources,
        name,
        edits,
        has_history || !is_original_builtin,
        true,
    )
}

#[cfg(test)]
pub(crate) fn disable_for_test(root: &Path, name: &str) -> Result<(), &'static str> {
    mutate(root, root, name, Operation::Enable(false))
}

pub(crate) fn reconcile(root: &Path, name: &str) -> Result<(), &'static str> {
    if !skills::valid_name(name) {
        return Err("invalid_name");
    }
    let _lock = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "io_error")?;
    let state_file = state_path(root, name)?;
    let Some(raw_state) = snapshot(&state_file)? else {
        return Ok(());
    };
    let previous: State = serde_json::from_slice(&raw_state).map_err(|_| "invalid_state")?;
    let path = skills::skill_path(root, "skills", name)?;
    let Some(content) = snapshot(&path)? else {
        return Ok(());
    };
    let text = std::str::from_utf8(&content).map_err(|_| "invalid_format")?;
    skills::parse(name, text)?;
    let current_hash = crate::archive_zet::content_hash(text);
    if current_hash == previous.hash {
        return Ok(());
    }
    let mut edits = copies(root, name, &previous.hash, &content, previous.enabled)?;
    let next = State {
        hash: current_hash,
        enabled: previous.enabled,
    };
    edits.push(Change {
        path: state_file,
        before: Some(raw_state),
        after: Some(serde_json::to_vec(&next).map_err(|_| "io_error")?),
    });
    if snapshot(&path)? != Some(content) {
        return Err("conflict");
    }
    apply(&edits)
}

pub(crate) fn save_external_changes(root: &Path, resources: &Path) -> Result<(), &'static str> {
    let tool = crate::archive::storage_tool(resources)
        .map_err(|_| "save_failed")?
        .0;
    let folders = ["skills/", ".agents/skills/", ".claude/skills/"];
    let mut status_args = vec!["status", "--porcelain", "--untracked-files=all", "--"];
    status_args.extend(folders);
    if git(&tool, root, &status_args)?.is_empty() {
        return Ok(());
    }
    let deleted = if git(&tool, root, &["rev-parse", "--verify", "HEAD"]).is_ok() {
        git(
            &tool,
            root,
            &[
                "diff",
                "--name-only",
                "--diff-filter=D",
                "HEAD",
                "--",
                "skills/",
            ],
        )?
    } else {
        String::new()
    };
    let dir = root.join("skills");
    for entry in fs::read_dir(dir).map_err(|_| "io_error")? {
        let entry = entry.map_err(|_| "io_error")?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|_| "io_error")?;
        if crate::archive_location::is_link_or_reparse(&metadata) {
            return Err("conflict");
        }
        if !metadata.is_dir() {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "invalid_name")?;
        if !skills::valid_name(&name) {
            return Err("invalid_name");
        }
        let paths = [
            relative(&name),
            format!(".agents/skills/{name}/SKILL.md"),
            format!(".claude/skills/{name}/SKILL.md"),
        ];
        let mut args = vec!["status", "--porcelain", "--untracked-files=all", "--"];
        args.extend(paths.iter().map(String::as_str));
        if git(&tool, root, &args)?.is_empty() {
            continue;
        }
        let _lock = crate::archive_zet::WRITE_LOCK
            .lock()
            .map_err(|_| "io_error")?;
        let path = skills::skill_path(root, "skills", &name)?;
        let content = match snapshot(&path)? {
            Some(content) => content,
            None if deleted.lines().any(|deleted_path| deleted_path == paths[0]) => continue,
            None => return Err("not_found"),
        };
        let text = std::str::from_utf8(&content).map_err(|_| "invalid_format")?;
        skills::parse(&name, text)?;
        let hash = crate::archive_zet::content_hash(text);
        let state_file = state_path(root, &name)?;
        let meta = state(root, &name, &hash)?;
        for folder in [".agents/skills", ".claude/skills"] {
            if snapshot(&skills::skill_path(root, folder, &name)?)?.is_some() != meta.enabled {
                return Err("conflict");
            }
        }
        let mut edits = copies(root, &name, &meta.hash, &content, meta.enabled)?;
        edits.insert(
            0,
            Change {
                path,
                before: Some(content.clone()),
                after: Some(content),
            },
        );
        edits.push(change(
            state_file,
            Some(
                serde_json::to_vec(&State {
                    hash,
                    enabled: meta.enabled,
                })
                .map_err(|_| "io_error")?,
            ),
        )?);
        transact(root, resources, &name, edits, true, false)?;
    }
    for path in deleted.lines() {
        let name = path
            .strip_prefix("skills/")
            .and_then(|value| value.strip_suffix("/SKILL.md"))
            .ok_or("unsaved_skill_changes")?;
        if !skills::valid_name(name) || skills::is_builtin(name) {
            return Err("conflict");
        }
        let _lock = crate::archive_zet::WRITE_LOCK
            .lock()
            .map_err(|_| "io_error")?;
        let canonical = skills::skill_path(root, "skills", name)?;
        if snapshot(&canonical)?.is_some() {
            return Err("conflict");
        }
        let state_file = state_path(root, name)?;
        let raw = snapshot(&state_file)?.ok_or("conflict")?;
        let meta: State = serde_json::from_slice(&raw).map_err(|_| "invalid_state")?;
        let mut edits = vec![Change {
            path: canonical,
            before: None,
            after: None,
        }];
        for folder in [".agents/skills", ".claude/skills"] {
            let path = skills::skill_path(root, folder, name)?;
            let before = snapshot(&path)?;
            if let Some(bytes) = &before {
                let text = std::str::from_utf8(bytes).map_err(|_| "conflict")?;
                if !meta.enabled || crate::archive_zet::content_hash(text) != meta.hash {
                    return Err("conflict");
                }
            } else if meta.enabled {
                return Err("conflict");
            }
            edits.push(Change {
                path,
                before,
                after: None,
            });
        }
        edits.push(Change {
            path: state_file,
            before: Some(raw),
            after: None,
        });
        transact(root, resources, name, edits, true, false)?;
    }
    if !git(&tool, root, &status_args)?.is_empty() {
        return Err("unsaved_skill_changes");
    }
    Ok(())
}

fn copies(
    root: &Path,
    name: &str,
    prior_hash: &str,
    current: &[u8],
    enabled: bool,
) -> Result<Vec<Change>, &'static str> {
    let mut result = Vec::new();
    for folder in [".agents/skills", ".claude/skills"] {
        let path = skills::skill_path(root, folder, name)?;
        let before = snapshot(&path)?;
        if let Some(bytes) = &before {
            let text = std::str::from_utf8(bytes).map_err(|_| "conflict")?;
            if crate::archive_zet::content_hash(text) != prior_hash && bytes != current {
                return Err("conflict");
            }
        }
        result.push(Change {
            path,
            before,
            after: enabled.then(|| current.to_vec()),
        });
    }
    Ok(result)
}

fn mutate(
    root: &Path,
    resources: &Path,
    name: &str,
    operation: Operation,
) -> Result<(), &'static str> {
    let _lock = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "io_error")?;
    let path = skills::skill_path(root, "skills", name)?;
    let before = snapshot(&path)?;
    let state_file = state_path(root, name)?;
    match operation {
        Operation::Create {
            title,
            description,
            body,
        } => {
            if before.is_some() {
                return Err("already_exists");
            }
            let head = SkillHead {
                name: name.into(),
                title,
                description,
            };
            let content = skills::render(&head, &body)?;
            let mut changes = copies(root, name, "", content.as_bytes(), true)?;
            if changes.iter().any(|c| c.before.is_some()) || snapshot(&state_file)?.is_some() {
                return Err("conflict");
            }
            changes.insert(
                0,
                Change {
                    path,
                    before: None,
                    after: Some(content.as_bytes().to_vec()),
                },
            );
            let meta = State {
                hash: crate::archive_zet::content_hash(&content),
                enabled: true,
            };
            changes.push(change(
                state_file,
                Some(serde_json::to_vec(&meta).map_err(|_| "io_error")?),
            )?);
            transact(root, resources, name, changes, true, false)
        }
        op => {
            let old = before.ok_or("not_found")?;
            let text = std::str::from_utf8(&old).map_err(|_| "invalid_format")?;
            let (mut head, old_body) = skills::parse(name, text)?;
            let old_hash = crate::archive_zet::content_hash(text);
            let meta = state(root, name, &old_hash)?;
            match op {
                Operation::Update {
                    changes,
                    expected_hash,
                } => {
                    if expected_hash != old_hash {
                        return Err("conflict");
                    }
                    if let Some(title) = changes.title {
                        head.title = title;
                    }
                    if let Some(description) = changes.description {
                        head.description = description;
                    }
                    let content =
                        skills::render(&head, changes.body.as_deref().unwrap_or(&old_body))?;
                    let commit = if content.as_bytes() == old {
                        let tool = crate::archive::storage_tool(resources)
                            .map_err(|_| "save_failed")?
                            .0;
                        !git(
                            &tool,
                            root,
                            &["status", "--porcelain", "--", &relative(name)],
                        )?
                        .trim()
                        .is_empty()
                    } else {
                        true
                    };
                    let mut edits =
                        copies(root, name, &meta.hash, content.as_bytes(), meta.enabled)?;
                    edits.insert(
                        0,
                        Change {
                            path,
                            before: Some(old),
                            after: Some(content.as_bytes().to_vec()),
                        },
                    );
                    let next = State {
                        hash: crate::archive_zet::content_hash(&content),
                        enabled: meta.enabled,
                    };
                    edits.push(change(
                        state_file,
                        Some(serde_json::to_vec(&next).map_err(|_| "io_error")?),
                    )?);
                    transact(root, resources, name, edits, commit, false)
                }
                Operation::Enable(enabled) => {
                    let mut edits = copies(root, name, &meta.hash, &old, enabled)?;
                    let next = State {
                        hash: old_hash,
                        enabled,
                    };
                    edits.push(change(
                        state_file,
                        Some(serde_json::to_vec(&next).map_err(|_| "io_error")?),
                    )?);
                    transact(root, resources, name, edits, true, false)
                }
                Operation::Remove => {
                    if skills::is_builtin(name) {
                        return Err("builtin");
                    }
                    let tool = crate::archive::storage_tool(resources)
                        .map_err(|_| "save_failed")?
                        .0;
                    let status = git(
                        &tool,
                        root,
                        &["status", "--porcelain", "--", &relative(name)],
                    )?;
                    if !status.trim().is_empty() {
                        return Err("conflict");
                    }
                    let mut edits = copies(root, name, &meta.hash, &old, false)?;
                    edits.insert(
                        0,
                        Change {
                            path,
                            before: Some(old),
                            after: None,
                        },
                    );
                    edits.push(change(state_file, None)?);
                    transact(root, resources, name, edits, true, false)
                }
                Operation::Create { .. } => unreachable!(),
            }
        }
    }
}

enum Operation {
    Create {
        title: String,
        description: String,
        body: String,
    },
    Update {
        changes: SkillChanges,
        expected_hash: String,
    },
    Enable(bool),
    Remove,
}

async fn command(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    name: String,
    operation: Operation,
) -> Result<serde_json::Value, String> {
    use tauri::Manager;
    let root = state.paths.archive.current.clone().read_owned().await;
    let resources = app
        .path()
        .resource_dir()
        .map_err(|_| "io_error".to_string())?;
    tauri::async_runtime::spawn_blocking(move || mutate(&root, &resources, &name, operation))
        .await
        .map_err(|_| "io_error".to_string())?
        .map_err(str::to_owned)?;
    Ok(serde_json::json!({"status":"ok"}))
}

#[tauri::command]
pub(crate) async fn skill_create(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    name: String,
    title: String,
    description: String,
    body: String,
) -> Result<serde_json::Value, String> {
    command(
        app,
        state,
        name,
        Operation::Create {
            title,
            description,
            body,
        },
    )
    .await
}

#[tauri::command]
pub(crate) async fn skill_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    name: String,
    changes: SkillChanges,
    expected_hash: String,
) -> Result<serde_json::Value, String> {
    command(
        app,
        state,
        name,
        Operation::Update {
            changes,
            expected_hash,
        },
    )
    .await
}

#[tauri::command]
pub(crate) async fn skill_set_enabled(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    name: String,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    command(app, state, name, Operation::Enable(enabled)).await
}

#[tauri::command]
pub(crate) async fn skill_remove(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    name: String,
) -> Result<serde_json::Value, String> {
    command(app, state, name, Operation::Remove).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn create() -> Operation {
        Operation::Create {
            title: "수업".into(),
            description: "정리".into(),
            body: "원래 글\n".into(),
        }
    }
    #[test]
    fn save_isolated_edits_conflicts_toggles_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        crate::archive::initialize(&root, dir.path()).unwrap();
        let tool = crate::archive::storage_tool(dir.path()).unwrap().0;
        fs::write(root.join("unrelated.txt"), "preserve").unwrap();
        git(&tool, &root, &["add", "--", "unrelated.txt"]).unwrap();
        mutate(&root, dir.path(), "probe", create()).unwrap();
        let path = root.join("skills/probe/SKILL.md");
        let initial = fs::read_to_string(&path).unwrap();
        let first_hash = crate::archive_zet::content_hash(&initial);
        let committed = git(
            &tool,
            &root,
            &["show", "--pretty=format:", "--name-only", "HEAD"],
        )
        .unwrap();
        assert!(committed.contains("skills/probe/SKILL.md"));
        assert!(!committed.contains("unrelated.txt"));
        assert!(git(&tool, &root, &["diff", "--cached", "--name-only"])
            .unwrap()
            .contains("unrelated.txt"));
        assert_eq!(
            mutate(&root, dir.path(), "probe", create()),
            Err("already_exists")
        );
        let update = |expected: String| Operation::Update {
            changes: SkillChanges {
                body: Some("고친 글\n".into()),
                ..Default::default()
            },
            expected_hash: expected,
        };
        assert_eq!(
            mutate(&root, dir.path(), "probe", update("wrong".into())),
            Err("conflict")
        );
        mutate(&root, dir.path(), "probe", update(first_hash)).unwrap();
        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.contains("고친 글"));
        let claude = root.join(".claude/skills/probe/SKILL.md");
        fs::write(&claude, "user edit").unwrap();
        assert_eq!(
            mutate(&root, dir.path(), "probe", Operation::Enable(false)),
            Err("conflict")
        );
        assert_eq!(fs::read_to_string(&claude).unwrap(), "user edit");
        fs::write(&claude, &updated).unwrap();
        mutate(&root, dir.path(), "probe", Operation::Enable(false)).unwrap();
        assert!(path.exists());
        assert!(!claude.exists());
        mutate(&root, dir.path(), "probe", Operation::Enable(true)).unwrap();
        assert_eq!(fs::read_to_string(&claude).unwrap(), updated);
        mutate(&root, dir.path(), "probe", Operation::Remove).unwrap();
        assert!(!path.exists());
        assert!(!claude.exists());
        assert!(git(&tool, &root, &["show", "HEAD^:skills/probe/SKILL.md"])
            .unwrap()
            .contains("고친 글"));
    }

    #[test]
    fn failed_git_commit_restores_canonical_and_copies() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        crate::archive::initialize(&root, dir.path()).unwrap();
        mutate(&root, dir.path(), "probe", create()).unwrap();
        let path = root.join("skills/probe/SKILL.md");
        let before = fs::read_to_string(&path).unwrap();
        fs::write(root.join(".git/hooks/pre-commit"), "#!/bin/sh\nexit 1\n").unwrap();
        let operation = Operation::Update {
            changes: SkillChanges {
                body: Some("수정".into()),
                ..Default::default()
            },
            expected_hash: crate::archive_zet::content_hash(&before),
        };
        assert_eq!(
            mutate(&root, dir.path(), "probe", operation),
            Err("save_failed")
        );
        assert_eq!(fs::read_to_string(path).unwrap(), before);
        assert_eq!(
            fs::read_to_string(root.join(".agents/skills/probe/SKILL.md")).unwrap(),
            before
        );
        assert_eq!(
            fs::read_to_string(root.join(".claude/skills/probe/SKILL.md")).unwrap(),
            before
        );
    }
    #[test]
    fn external_canonical_edits_sync_but_external_copy_edits_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        crate::archive::initialize(&root, dir.path()).unwrap();
        mutate(&root, dir.path(), "probe", create()).unwrap();
        let path = root.join("skills/probe/SKILL.md");
        let original = fs::read_to_string(&path).unwrap();
        let changed = original.replace("원래 글", "정본 수정");
        fs::write(&path, &changed).unwrap();
        reconcile(&root, "probe").unwrap();
        assert_eq!(
            mutate(&root, dir.path(), "probe", Operation::Remove),
            Err("conflict")
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), changed);
        mutate(
            &root,
            dir.path(),
            "probe",
            Operation::Update {
                changes: SkillChanges::default(),
                expected_hash: crate::archive_zet::content_hash(&changed),
            },
        )
        .unwrap();
        let tool = crate::archive::storage_tool(dir.path()).unwrap().0;
        assert!(git(
            &tool,
            &root,
            &["status", "--porcelain", "--", &relative("probe")]
        )
        .unwrap()
        .trim()
        .is_empty());
        let copy = root.join(".claude/skills/probe/SKILL.md");
        assert_eq!(fs::read_to_string(&copy).unwrap(), changed);
        fs::write(&copy, "수동 변경").unwrap();
        fs::write(&path, changed.replace("정본 수정", "다시 수정")).unwrap();
        assert_eq!(reconcile(&root, "probe"), Err("conflict"));
        assert_eq!(fs::read_to_string(&copy).unwrap(), "수동 변경");
    }

    #[test]
    fn selected_skill_is_stored_separately_from_user_text() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        crate::storage::initialize(&db, &dir.path().join("backups"), "test").unwrap();
        let c = rusqlite::Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES ('test','','a','a')",
            [],
        )
        .unwrap();
        drop(c);
        crate::chat::append_full(
            &db,
            "test",
            crate::chat::Role::User,
            "원래 글",
            None,
            None,
            None,
            Some("yoga-1".into()),
        )
        .unwrap();
        let value = serde_json::to_value(crate::chat::read(&db, "test").unwrap()).unwrap();
        assert_eq!(value["messages"][0]["text"], "원래 글");
        assert_eq!(value["messages"][0]["skill"], "yoga-1");
    }

    #[test]
    fn update_without_local_state_keeps_absent_provider_copies_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        crate::archive::initialize(&root, dir.path()).unwrap();
        mutate(&root, dir.path(), "probe", create()).unwrap();
        let path = root.join("skills/probe/SKILL.md");
        let before = fs::read_to_string(&path).unwrap();
        fs::remove_file(state_path(&root, "probe").unwrap()).unwrap();
        fs::remove_file(root.join(".agents/skills/probe/SKILL.md")).unwrap();
        fs::remove_file(root.join(".claude/skills/probe/SKILL.md")).unwrap();
        mutate(
            &root,
            dir.path(),
            "probe",
            Operation::Update {
                changes: SkillChanges {
                    body: Some("새 글".into()),
                    ..Default::default()
                },
                expected_hash: crate::archive_zet::content_hash(&before),
            },
        )
        .unwrap();
        assert!(!root.join(".agents/skills/probe/SKILL.md").exists());
        assert!(!root.join(".claude/skills/probe/SKILL.md").exists());
    }

    #[tokio::test]
    #[ignore = "uses the actual local Codex account"]
    async fn live_selected_skill_and_disabled_skill() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        crate::archive::initialize(&root, dir.path()).unwrap();
        let db = dir.path().join("test.db");
        crate::storage::initialize(&db, &dir.path().join("backups"), "test").unwrap();
        let c = rusqlite::Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES ('test','','a','a')",
            [],
        )
        .unwrap();
        drop(c);
        mutate(
            &root,
            dir.path(),
            "momso-probe",
            Operation::Create {
                title: "시험".into(),
                description: "Output the marker when selected".into(),
                body: "Reply with exactly MOMSOLIVE7184 and nothing else.".into(),
            },
        )
        .unwrap();
        crate::chat::append_full(
            &db,
            "test",
            crate::chat::Role::User,
            "Follow the selected skill.",
            None,
            None,
            None,
            Some("momso-probe".into()),
        )
        .unwrap();
        let mut answer = String::new();
        crate::chat_codex::run_with_skill(
            &db,
            &root,
            "test",
            "Follow the selected skill.",
            Some("gpt-6-luna"),
            Some("low"),
            Some("momso-probe"),
            None,
            None,
            &crate::permission_modes::codex_settings("Ask for approval").unwrap(),
            None,
            tokio_util::sync::CancellationToken::new(),
            |event, payload| {
                if event == "chat-stream" {
                    answer.push_str(payload["delta"].as_str().unwrap_or(""));
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(answer.trim(), "MOMSOLIVE7184");
        let detail = serde_json::to_value(crate::chat::read(&db, "test").unwrap()).unwrap();
        assert_eq!(detail["messages"][0]["text"], "Follow the selected skill.");
        assert_eq!(detail["messages"][0]["skill"], "momso-probe");
        assert_eq!(detail["messages"][1]["skill"], serde_json::Value::Null);
        mutate(&root, dir.path(), "momso-probe", Operation::Enable(false)).unwrap();
        assert!(!crate::skills::callable(&root, "momso-probe").unwrap());
        let result = crate::chat_codex::run_with_skill(
            &db,
            &root,
            "test",
            "Follow the selected skill.",
            Some("gpt-6-luna"),
            Some("low"),
            Some("momso-probe"),
            None,
            None,
            &crate::permission_modes::codex_settings("Ask for approval").unwrap(),
            None,
            tokio_util::sync::CancellationToken::new(),
            |_, _| {},
        )
        .await;
        assert_eq!(result, Err("skill_unavailable".into()));
    }
}
