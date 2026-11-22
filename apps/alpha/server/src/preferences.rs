//! Mobile settings shared between devices; accepts only non-secret preferences.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::Path as FsPath,
};

#[derive(Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Model {
    provider: String,
    model: Option<String>,
    effort: Option<String>,
    fast: Option<bool>,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Preferences {
    assistant_provider: String,
    chat_model: Option<Model>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Update {
    revision: String,
    preferences: Preferences,
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}
fn valid(value: &Preferences) -> bool {
    let provider = |s: &str| matches!(s, "codex" | "claude" | "gemini");
    provider(&value.assistant_provider)
        && value.chat_model.as_ref().is_none_or(|m| {
            provider(&m.provider)
                && m.model.as_deref().is_none_or(valid_name)
                && m.effort.as_deref().is_none_or(valid_name)
        })
}
fn read(root: &FsPath) -> Result<(Preferences, String), &'static str> {
    let path = root.join("preferences.json");
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((
                Preferences {
                    assistant_provider: "codex".into(),
                    chat_model: None,
                },
                "missing".into(),
            ))
        }
        Ok(file) => file,
        Err(_) => return Err("preferences_conflict"),
    };
    let meta = file.metadata().map_err(|_| "preferences_conflict")?;
    if !meta.is_file() || meta.len() > 4096 {
        return Err("preferences_conflict");
    }
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| "storage_unavailable")?;
    if bytes.len() > 4096 {
        return Err("preferences_conflict");
    }
    let prefs: Preferences = serde_json::from_slice(&bytes).map_err(|_| "preferences_conflict")?;
    if !valid(&prefs) {
        return Err("preferences_conflict");
    }
    Ok((prefs, format!("{:x}", Sha256::digest(&bytes))))
}
fn fail(reason: &'static str) -> ApiError {
    error(
        match reason {
            "invalid_arguments" => StatusCode::BAD_REQUEST,
            "preferences_conflict" => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        },
        reason,
    )
}
pub async fn get_preferences(
    State(app): State<App>,
    h: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    #[cfg(target_os = "linux")]
    let guard = ReadGuard::new(&app);
    #[cfg(not(target_os = "linux"))]
    let guard = WorkGuard::new(&app);
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let root = app.data.join("archive");
        let _lock = momso_core::lock::exclusive(&root).map_err(fail)?;
        let (preferences, revision) = read(&root).map_err(fail)?;
        Ok(Json(json!({"preferences":preferences,"revision":revision})))
    })
    .await
    .map_err(|_| fail("storage_unavailable"))?
}
pub async fn put_preferences(
    State(app): State<App>,
    h: HeaderMap,
    Json(input): Json<Update>,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    if !valid(&input.preferences) {
        return Err(fail("invalid_arguments"));
    }
    let guard = WorkGuard::new(&app);
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let root = app.data.join("archive");
        let _lock = momso_core::lock::exclusive(&root).map_err(fail)?;
        let (_, revision) = read(&root).map_err(fail)?;
        if revision != input.revision { return Err(fail("preferences_conflict")); }
        let bytes = serde_json::to_vec(&input.preferences).map_err(|_| fail("storage_unavailable"))?;
        let mut temp = tempfile::NamedTempFile::new_in(&root).map_err(|_| fail("storage_unavailable"))?;
        temp.write_all(&bytes).and_then(|_| temp.as_file().sync_all()).map_err(|_| fail("storage_unavailable"))?;
        temp.persist(root.join("preferences.json")).map_err(|_| fail("storage_unavailable"))?;
        #[cfg(unix)]
        fs::File::open(&root).and_then(|f| f.sync_all()).map_err(|_| fail("storage_unavailable"))?;
        Ok(Json(json!({"preferences":input.preferences,"revision":format!("{:x}",Sha256::digest(&bytes))})))
    }).await.map_err(|_| fail("storage_unavailable"))?
}
