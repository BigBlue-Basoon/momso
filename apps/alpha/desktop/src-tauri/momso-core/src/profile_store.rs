//! Recoverable server-side profile writes, using the desktop profile format.
use crate::profile_format::{profile_get, profile_rules, UserProfile};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const FILES: [&str; 4] = ["profile.md", "AGENTS.md", "CLAUDE.md", "GEMINI.md"];
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    old: Option<String>,
    new: String,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u8,
    files: Vec<Entry>,
}

fn read(root: &Path, name: &str) -> Result<Option<String>, &'static str> {
    let path = root.join(name);
    match fs::symlink_metadata(&path) {
        Ok(meta)
            if !meta.is_file()
                || crate::archive_location::is_link_or_reparse(&meta)
                || meta.len() > 128 * 1024 =>
        {
            return Err("profile_conflict")
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("storage_unavailable"),
        _ => {}
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|_| "storage_unavailable")
}
fn atomic_write(path: &Path, text: &str) -> Result<(), &'static str> {
    let parent = path.parent().ok_or("unsafe_path")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| "storage_unavailable")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| "storage_unavailable")?;
    }
    file.write_all(text.as_bytes())
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| "storage_unavailable")?;
    file.persist(path).map_err(|_| "storage_unavailable")?;
    #[cfg(unix)]
    fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|_| "storage_unavailable")?;
    Ok(())
}
fn journal_path(root: &Path) -> PathBuf {
    root.join(".momso/profile-write-journal.json")
}
fn remove_journal(root: &Path) -> Result<(), &'static str> {
    let path = journal_path(root);
    fs::remove_file(&path).map_err(|_| "storage_unavailable")?;
    #[cfg(unix)]
    fs::File::open(path.parent().ok_or("unsafe_path")?)
        .and_then(|dir| dir.sync_all())
        .map_err(|_| "storage_unavailable")?;
    Ok(())
}
fn recover_locked(root: &Path) -> Result<(), &'static str> {
    let path = journal_path(root);
    let bytes = match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(meta)
            if meta.is_file()
                && !crate::archive_location::is_link_or_reparse(&meta)
                && meta.len() <= 2 * 1024 * 1024 =>
        {
            fs::read(&path).map_err(|_| "storage_unavailable")?
        }
        _ => return Err("profile_recovery_conflict"),
    };
    let journal: Journal =
        serde_json::from_slice(&bytes).map_err(|_| "profile_recovery_conflict")?;
    if journal.version != 1
        || journal.files.len() != FILES.len()
        || journal
            .files
            .iter()
            .zip(FILES)
            .any(|(entry, name)| entry.name != name)
    {
        return Err("profile_recovery_conflict");
    }
    for entry in &journal.files {
        let current = read(root, &entry.name)?;
        if current != entry.old && current.as_deref() != Some(entry.new.as_str()) {
            return Err("profile_recovery_conflict");
        }
    }
    for entry in &journal.files {
        let current = read(root, &entry.name)?;
        if current == entry.old {
            continue;
        }
        if current.as_deref() != Some(entry.new.as_str()) {
            return Err("profile_recovery_conflict");
        }
        match &entry.old {
            Some(value) => atomic_write(&root.join(&entry.name), value)?,
            None => fs::remove_file(root.join(&entry.name)).map_err(|_| "storage_unavailable")?,
        }
    }
    remove_journal(root)?;
    Ok(())
}
pub fn recover(root: &Path) -> Result<(), &'static str> {
    let _guard = crate::lock::exclusive(root)?;
    recover_locked(root)
}
pub fn get(root: &Path) -> Result<UserProfile, &'static str> {
    let _guard = crate::lock::exclusive(root)?;
    recover_locked(root)?;
    read(root, "profile.md")?;
    profile_get(root).map_err(|_| "profile_invalid")
}
pub fn set(
    root: &Path,
    name: String,
    about: String,
    style: String,
) -> Result<UserProfile, &'static str> {
    if name.contains(['\n', '\r'])
        || [&name, &about, &style]
            .iter()
            .any(|v| v.chars().count() > 4000)
    {
        return Err("invalid_arguments");
    }
    let _guard = crate::lock::exclusive(root)?;
    recover_locked(root)?;
    let mut old = Vec::new();
    for name in FILES {
        old.push(read(root, name)?);
    }
    let previous = profile_get(root).map_err(|_| "profile_invalid")?;
    let expected = profile_rules(&previous);
    if old[1..]
        .iter()
        .flatten()
        .any(|value| value.replace("\r\n", "\n") != expected)
    {
        return Err("profile_conflict");
    }
    let profile = UserProfile {
        name,
        about,
        style,
        updated_at: Some(chrono::Utc::now().to_rfc3339()),
    };
    let yaml = serde_yaml::to_string(&profile).map_err(|_| "storage_unavailable")?;
    let content = format!("---\n{}---\n", yaml.strip_prefix("---\n").unwrap_or(&yaml));
    let rules = profile_rules(&profile);
    let journal = Journal {
        version: 1,
        files: FILES
            .iter()
            .enumerate()
            .map(|(i, name)| Entry {
                name: (*name).to_owned(),
                old: old[i].clone(),
                new: if i == 0 {
                    content.clone()
                } else {
                    rules.clone()
                },
            })
            .collect(),
    };
    let text = serde_json::to_string(&journal).map_err(|_| "storage_unavailable")?;
    atomic_write(&journal_path(root), &text)?;
    for entry in &journal.files {
        if atomic_write(&root.join(&entry.name), &entry.new).is_err() {
            return match recover_locked(root) {
                Ok(()) => Err("storage_unavailable"),
                Err(_) => Err("profile_recovery_conflict"),
            };
        }
    }
    remove_journal(root)?;
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_preserves_custom_rules_and_recovers_interrupted_write() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        set(root, "선생님".into(), "수업".into(), "공손하게".into()).unwrap();
        assert_eq!(get(root).unwrap().name, "선생님");
        let previous: Vec<Option<String>> =
            FILES.iter().map(|name| read(root, name).unwrap()).collect();
        for name in &FILES[1..] {
            assert_eq!(
                fs::read_to_string(root.join(name)).unwrap(),
                profile_rules(&get(root).unwrap())
            );
        }
        let replacement = UserProfile {
            name: "다른 이름".into(),
            ..Default::default()
        };
        let yaml = serde_yaml::to_string(&replacement).unwrap();
        let profile = format!("---\n{}---\n", yaml.strip_prefix("---\n").unwrap_or(&yaml));
        let rules = profile_rules(&replacement);
        let journal = Journal {
            version: 1,
            files: FILES
                .iter()
                .enumerate()
                .map(|(i, name)| Entry {
                    name: (*name).into(),
                    old: previous[i].clone(),
                    new: if i == 0 {
                        profile.clone()
                    } else {
                        rules.clone()
                    },
                })
                .collect(),
        };
        atomic_write(
            &journal_path(root),
            &serde_json::to_string(&journal).unwrap(),
        )
        .unwrap();
        atomic_write(&root.join("profile.md"), &profile).unwrap();
        recover(root).unwrap();
        assert_eq!(get(root).unwrap().name, "선생님");
        fs::write(root.join("AGENTS.md"), "사용자가 바꿈").unwrap();
        assert!(matches!(
            set(root, "새 이름".into(), String::new(), String::new()),
            Err("profile_conflict")
        ));
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "사용자가 바꿈"
        );
        assert_eq!(get(root).unwrap().name, "선생님");
        atomic_write(
            &journal_path(root),
            &serde_json::to_string(&journal).unwrap(),
        )
        .unwrap();
        assert_eq!(recover(root), Err("profile_recovery_conflict"));
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "사용자가 바꿈"
        );
    }
}
