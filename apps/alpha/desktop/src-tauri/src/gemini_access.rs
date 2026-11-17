use crate::{credentials, process};
use std::{collections::HashMap, path::PathBuf};

// Session-only; never changes the user's shared settings.json.
pub(crate) const API_KEY_SESSION_SETTINGS: &str = r#"{"modelProvider":"gemini"}"#;

pub(crate) fn executable() -> Option<PathBuf> {
    process::managed_ai_tool("gemini").or_else(|| {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|base| base.join("agy").join("bin").join("agy.exe"))
            .filter(|path| path.is_file())
            .or_else(|| process::resolve_interactive_tool("agy"))
    })
}

pub(crate) fn api_key_environment(
) -> Result<Option<HashMap<String, String>>, credentials::CredentialError> {
    let key = credentials::get_secret(credentials::ai_key_name("gemini")?)?;
    Ok(key.map(|value| HashMap::from([("GEMINI_API_KEY".to_owned(), value)])))
}
