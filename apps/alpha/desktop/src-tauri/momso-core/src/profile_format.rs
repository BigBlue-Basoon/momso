//! The desktop and server use the same user profile format and rule suffix.
use std::{fs, path::Path};

const ARCHIVE_RULES: &str = include_str!("../../src/archive_rules.md");

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserProfile {
    pub name: String,
    pub about: String,
    pub style: String,
    pub updated_at: Option<String>,
}

pub fn profile_get(root: &Path) -> Result<UserProfile, String> {
    let path = root.join("profile.md");
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UserProfile::default())
        }
        Err(_) => return Err("사용자 정보를 읽지 못했습니다".into()),
    };
    let yaml = text
        .strip_prefix("---\n")
        .and_then(|s| s.strip_suffix("---\n"))
        .ok_or_else(|| "사용자 정보 파일을 읽지 못했습니다".to_string())?;
    serde_yaml::from_str(yaml).map_err(|_| "사용자 정보 파일을 읽지 못했습니다".into())
}

pub fn profile_rules(profile: &UserProfile) -> String {
    let mut text = ARCHIVE_RULES.replace("\r\n", "\n");
    if !profile.name.is_empty() || !profile.about.is_empty() || !profile.style.is_empty() {
        text.push_str("\n## 사용자에 대해\n\n사용자 정보가 몸소 보관함 규칙과 어긋나면 몸소 보관함 규칙을 먼저 따른다.\n");
        for (label, value) in [
            ("이름", &profile.name),
            ("나에 대해", &profile.about),
            ("답하는 말투", &profile.style),
        ] {
            if !value.is_empty() {
                text.push_str(&format!("\n### {label}\n\n{value}\n"));
            }
        }
    }
    text
}
