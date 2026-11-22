use serde::{Deserialize, Serialize};

pub const BUNDLED_SKILLS: &[(&str, &str)] = &[
    (
        "marketing-preparation",
        include_str!("../../bundled-skills/marketing-preparation/SKILL.md"),
    ),
    (
        "naver-smartplace",
        include_str!("../../bundled-skills/naver-smartplace/SKILL.md"),
    ),
    (
        "onstudio-operations",
        include_str!("../../bundled-skills/onstudio-operations/SKILL.md"),
    ),
    (
        "studiomate-operations",
        include_str!("../../bundled-skills/studiomate-operations/SKILL.md"),
    ),
    (
        "soomgo-operations",
        include_str!("../../bundled-skills/soomgo-operations/SKILL.md"),
    ),
    (
        "frip-host-operations",
        include_str!("../../bundled-skills/frip-host-operations/SKILL.md"),
    ),
    (
        "sssd-host-operations",
        include_str!("../../bundled-skills/sssd-host-operations/SKILL.md"),
    ),
    (
        "conversation-import",
        include_str!("../../bundled-skills/conversation-import/SKILL.md"),
    ),
    (
        "github-connect",
        include_str!("../../bundled-skills/github-connect/SKILL.md"),
    ),
    (
        "object-store-connect",
        include_str!("../../bundled-skills/object-store-connect/SKILL.md"),
    ),
    (
        "bodycodi-operations",
        include_str!("../../bundled-skills/bodycodi-operations/SKILL.md"),
    ),
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SkillHead {
    pub name: String,
    pub description: String,
    pub title: String,
}

pub fn valid_name(name: &str) -> bool {
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

pub fn parse(name: &str, content: &str) -> Result<(SkillHead, String), &'static str> {
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

pub fn render(head: &SkillHead, body: &str) -> Result<String, &'static str> {
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

pub fn skill_path(
    root: &std::path::Path,
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

pub fn is_builtin(name: &str) -> bool {
    BUNDLED_SKILLS.iter().any(|(builtin, _)| *builtin == name)
}
