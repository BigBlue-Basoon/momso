//! Saved site logins. The single serialized vault lives only in the OS credential store.
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, OnceLock},
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(crate) const VAULT: &str = "browser:saved-logins:v1";
// Windows generic credentials have a bounded blob size; reject rather than truncate.
const MAX_VAULT_BYTES: usize = 2200;
static LOCK: Mutex<()> = Mutex::new(());

struct PendingSelection {
    ids: HashSet<String>,
    response: oneshot::Sender<Option<String>>,
}
fn selections() -> &'static Mutex<HashMap<String, PendingSelection>> {
    static SELECTIONS: OnceLock<Mutex<HashMap<String, PendingSelection>>> = OnceLock::new();
    SELECTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

struct SelectionGuard(String);
impl Drop for SelectionGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = selections().lock() {
            map.remove(&self.0);
        }
    }
}

/// Called only by the trusted main webview after the person selects a login.
#[tauri::command]
pub(crate) fn saved_login_select(
    webview: tauri::Webview,
    request_id: String,
    login_id: String,
) -> Result<(), &'static str> {
    allowed_caller(webview.label())?;
    select_login(&request_id, login_id)
}

fn select_login(request_id: &str, login_id: String) -> Result<(), &'static str> {
    let mut map = selections().lock().map_err(|_| "unavailable")?;
    let selection = map.get(request_id).ok_or("unavailable")?;
    if !selection.ids.contains(&login_id) {
        return Err("unavailable");
    }
    let selection = map.remove(request_id).ok_or("unavailable")?;
    selection
        .response
        .send(Some(login_id))
        .map_err(|_| "unavailable")
}

/// The trusted main webview may dismiss a pending account selection.
#[tauri::command]
pub(crate) fn saved_login_cancel(
    webview: tauri::Webview,
    request_id: String,
) -> Result<(), &'static str> {
    allowed_caller(webview.label())?;
    cancel_selection(&request_id)
}

fn cancel_selection(request_id: &str) -> Result<(), &'static str> {
    let selection = selections()
        .lock()
        .map_err(|_| "unavailable")?
        .remove(request_id)
        .ok_or("unavailable")?;
    selection.response.send(None).map_err(|_| "unavailable")
}

pub(crate) async fn select_for_person(
    chat_id: &str,
    entries: &[LoginInfo],
    notify: &crate::browser_runtime::Notify,
    cancel: CancellationToken,
) -> Result<LoginInfo, &'static str> {
    select_for_person_with_timeout(
        chat_id,
        entries,
        notify,
        cancel,
        std::time::Duration::from_secs(300),
    )
    .await
}

async fn select_for_person_with_timeout(
    chat_id: &str,
    entries: &[LoginInfo],
    notify: &crate::browser_runtime::Notify,
    cancel: CancellationToken,
    timeout: std::time::Duration,
) -> Result<LoginInfo, &'static str> {
    if entries.len() == 1 {
        return Ok(entries[0].clone());
    }
    if entries.is_empty() {
        return Err("not_found");
    }
    let request_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    {
        let mut map = selections().lock().map_err(|_| "unavailable")?;
        map.insert(
            request_id.clone(),
            PendingSelection {
                ids: entries.iter().map(|e| e.id.clone()).collect(),
                response: tx,
            },
        );
    }
    let _selection_guard = SelectionGuard(request_id.clone());
    notify(
        "saved-login-selection-request",
        serde_json::json!({"requestId":request_id,"chatId":chat_id,"entries":entries}),
    );
    let result = tokio::select! {
        _ = cancel.cancelled() => Err("cancelled"),
        value = tokio::time::timeout(timeout, rx) => match value {
            Ok(Ok(Some(id))) => Ok(id),
            Ok(Ok(None)) => Err("cancelled"),
            Ok(Err(_)) => Err("cancelled"),
            Err(_) => Err("timeout"),
        },
    };
    // Once the close event is delivered, no caller may still answer this request.
    drop(_selection_guard);
    notify(
        "saved-login-selection-closed",
        serde_json::json!({"requestId":request_id,"reason":match &result { Ok(_) => "selected", Err("timeout") => "timeout", _ => "cancelled" }}),
    );
    let id = result?;
    entries
        .iter()
        .find(|entry| entry.id == id)
        .cloned()
        .ok_or("unavailable")
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    id: String,
    origin: String,
    label: String,
    username: String,
    password: String,
    #[serde(default)]
    ask_before_use: bool,
    #[serde(default)]
    login_path: Option<String>,
    #[serde(default)]
    default_for_path: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoginInfo {
    pub(crate) id: String,
    pub(crate) origin: String,
    pub(crate) label: String,
    pub(crate) username: String,
    pub(crate) ask_before_use: bool,
    pub(crate) login_path: Option<String>,
    pub(crate) default_for_path: bool,
}

pub(crate) struct LoginSecret {
    pub(crate) username: String,
    pub(crate) password: String,
}

fn matches_selected(entry: &Entry, selected: &LoginInfo) -> bool {
    entry.id == selected.id
        && entry.origin == selected.origin
        && entry.label == selected.label
        && entry.username == selected.username
        && entry.ask_before_use == selected.ask_before_use
        && entry.login_path == selected.login_path
        && entry.default_for_path == selected.default_for_path
}

pub(crate) fn for_origin(value: &str) -> Result<Vec<LoginInfo>, &'static str> {
    let _guard = LOCK.lock().map_err(|_| "credential_store_unavailable")?;
    let expected = origin(value)?;
    Ok(read()?
        .into_iter()
        .filter(|entry| entry.origin == expected)
        .map(|entry| LoginInfo {
            id: entry.id,
            origin: entry.origin,
            label: entry.label,
            username: entry.username,
            ask_before_use: entry.ask_before_use,
            login_path: entry.login_path,
            default_for_path: entry.default_for_path,
        })
        .collect())
}

pub(crate) fn match_login_path(mut entries: Vec<LoginInfo>, path: &str) -> Vec<LoginInfo> {
    let exact_exists = entries
        .iter()
        .any(|item| item.login_path.as_deref() == Some(path));
    entries.retain(|item| {
        if exact_exists {
            item.login_path.as_deref() == Some(path)
        } else {
            item.login_path.is_none()
        }
    });
    if entries.len() > 1
        && entries
            .iter()
            .filter(|entry| entry.default_for_path)
            .count()
            == 1
    {
        entries.retain(|entry| entry.default_for_path);
    }
    entries
}

pub(crate) fn secret_for_origin(selected: &LoginInfo) -> Result<LoginSecret, &'static str> {
    let _guard = LOCK.lock().map_err(|_| "credential_store_unavailable")?;
    if origin(&selected.origin)? != selected.origin {
        return Err("invalid_origin");
    }
    let entry = read()?
        .into_iter()
        .find(|entry| matches_selected(entry, selected))
        .ok_or("not_found")?;
    Ok(LoginSecret {
        username: entry.username,
        password: entry.password,
    })
}

pub(crate) fn known_passwords() -> Result<Vec<String>, &'static str> {
    let _guard = LOCK.lock().map_err(|_| "credential_store_unavailable")?;
    let mut values: Vec<String> = read()?.into_iter().map(|entry| entry.password).collect();
    values.extend(
        crate::redaction_wait::material_with(&|name| crate::credentials::get_secret(name))
            .map_err(|_| "credential_store_unavailable")?,
    );
    values.extend(
        used_passwords()
            .lock()
            .map_err(|_| "credential_store_unavailable")?
            .iter()
            .cloned(),
    );
    Ok(values)
}

/// Apply the same secret boundary to every browser tool result, including
/// errors and URLs, rather than relying only on the page reader.
fn redact_value(value: &mut serde_json::Value, known: &[&str]) {
    match value {
        serde_json::Value::String(text) => {
            let safe = crate::chat_archive_redact::redact_with_known(text, known).text;
            *text = if known
                .iter()
                .any(|secret| !secret.is_empty() && safe.contains(secret))
            {
                String::new()
            } else {
                safe
            };
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_value(item, known);
            }
        }
        serde_json::Value::Object(items) => {
            for item in items.values_mut() {
                redact_value(item, known);
            }
        }
        _ => {}
    }
}

pub(crate) fn redact_tool_result(
    mut value: serde_json::Value,
) -> Result<serde_json::Value, &'static str> {
    let known = known_passwords()?;
    let refs: Vec<&str> = known.iter().map(String::as_str).collect();
    redact_value(&mut value, &refs);
    Ok(value)
}

pub(crate) fn redact_text(text: &str, known: &[String]) -> String {
    let redacted = known
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(text.to_owned(), |value, secret| {
            value.replace(secret, "[가림]")
        });
    if known
        .iter()
        .any(|secret| !secret.is_empty() && redacted.contains(secret))
    {
        String::new()
    } else {
        redacted
    }
}

/// Retain a short suffix so that a secret split across stream chunks cannot leak.
#[derive(Default)]
pub(crate) struct SecretStream {
    pending: String,
}
impl SecretStream {
    pub(crate) fn hold(&mut self, chunk: &str) {
        self.pending.push_str(chunk);
    }
    pub(crate) fn push(&mut self, chunk: &str, known: &[String], finish: bool) -> String {
        self.pending.push_str(chunk);
        let longest = known.iter().map(String::len).max().unwrap_or(0);
        let mut end = if finish {
            self.pending.len()
        } else {
            self.pending.len().saturating_sub(longest.saturating_sub(1))
        };
        while !self.pending.is_char_boundary(end) {
            end -= 1;
        }
        loop {
            let before = end;
            for secret in known.iter().filter(|secret| !secret.is_empty()) {
                let mut offset = 0;
                while offset < self.pending.len() {
                    let Some(found) = self.pending[offset..].find(secret) else {
                        break;
                    };
                    let start = offset + found;
                    if start < end && start + secret.len() > end {
                        end = start;
                    }
                    offset = start
                        + self.pending[start..]
                            .chars()
                            .next()
                            .expect("nonempty match")
                            .len_utf8();
                }
            }
            if end == before {
                break;
            }
        }
        let result = redact_text(&self.pending[..end], known);
        self.pending.drain(..end);
        result
    }
}

// Keep a secret redacted for this process even if the person edits or deletes its
// vault entry while the site is still open. This state never reaches disk.
fn used_passwords() -> &'static Mutex<HashSet<String>> {
    static USED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    USED.get_or_init(|| Mutex::new(HashSet::new()))
}

pub(crate) fn remember_used_password(value: &str) -> Result<(), &'static str> {
    let mut used = used_passwords().lock().map_err(|_| "unavailable")?;
    if used.len() >= 1024 && !used.contains(value) {
        return Err("unavailable");
    }
    used.insert(value.to_owned());
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LoginInput {
    id: Option<String>,
    origin: String,
    label: String,
    username: String,
    password: Option<String>,
    #[serde(default)]
    ask_before_use: Option<bool>,
    #[serde(default)]
    login_path: Option<String>,
    #[serde(default)]
    default_for_path: Option<bool>,
}

fn origin(value: &str) -> Result<String, &'static str> {
    let url = reqwest::Url::parse(value).map_err(|_| "invalid_origin")?;
    let test_loopback = cfg!(test) && url.scheme() == "http" && url.host_str() == Some("127.0.0.1");
    if (url.scheme() != "https" && !test_loopback)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("invalid_origin");
    }
    Ok(url.origin().ascii_serialization())
}

fn valid_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control)
}

fn allowed_caller(label: &str) -> Result<(), &'static str> {
    if label == "main" {
        Ok(())
    } else {
        Err("unavailable")
    }
}

fn read() -> Result<Vec<Entry>, &'static str> {
    let value =
        crate::credentials::get_secret(VAULT).map_err(|_| "credential_store_unavailable")?;
    match value {
        None => Ok(Vec::new()),
        Some(text) => serde_json::from_str(&text).map_err(|_| "credential_store_unavailable"),
    }
}

pub(crate) fn passwords_from_serialized(value: &str) -> Result<Vec<String>, &'static str> {
    let entries: Vec<Entry> =
        serde_json::from_str(value).map_err(|_| "credential_store_unavailable")?;
    Ok(entries.into_iter().map(|entry| entry.password).collect())
}

fn retired_entries(old: Vec<Entry>, current: &[Entry]) -> Vec<Entry> {
    old.into_iter()
        .filter(|old| !current.iter().any(|entry| entry.password == old.password))
        .collect()
}

fn write(entries: &[Entry]) -> Result<(), &'static str> {
    let value = serde_json::to_string(entries).map_err(|_| "credential_store_unavailable")?;
    if value.len() > MAX_VAULT_BYTES {
        return Err("capacity_exceeded");
    }
    let _guard =
        crate::credentials::prevent_secret_changes().map_err(|_| "credential_store_unavailable")?;
    let previous =
        crate::credentials::get_secret(VAULT).map_err(|_| "credential_store_unavailable")?;
    if let Some(previous) = previous {
        let old: Vec<Entry> =
            serde_json::from_str(&previous).map_err(|_| "credential_store_unavailable")?;
        let retired = retired_entries(old, entries);
        if !retired.is_empty() {
            let encoded =
                serde_json::to_string(&retired).map_err(|_| "credential_store_unavailable")?;
            crate::redaction_wait::enqueue_unlocked(VAULT, &encoded)
                .map_err(|_| "credential_store_unavailable")?;
        }
    }
    // Mark the new value for history checks before making it active.
    crate::redaction_wait::watch_new_unlocked(VAULT).map_err(|_| "credential_store_unavailable")?;
    crate::credentials::put_secret_unlocked(VAULT, &value)
        .map_err(|_| "credential_store_unavailable")
}

#[cfg(all(test, windows))]
pub(crate) fn fixture_save_login(
    site: &str,
    password: &str,
    ask: bool,
) -> Result<(), &'static str> {
    if std::env::var_os("MOMSO_TEST_FAKE_CREDENTIAL_STORE").is_none() {
        return Err("unavailable");
    }
    let _guard = LOCK.lock().map_err(|_| "unavailable")?;
    // This helper is restricted to the process-local fake store above.
    crate::credentials::delete_secret(VAULT).map_err(|_| "unavailable")?;
    let mut entries = Vec::new();
    prepare_set(
        &mut entries,
        LoginInput {
            id: None,
            origin: site.into(),
            label: "fixture".into(),
            username: "fixture".into(),
            password: Some(password.into()),
            ask_before_use: Some(ask),
            login_path: None,
            default_for_path: None,
        },
    )?;
    write(&entries)
}

#[tauri::command]
pub(crate) fn saved_login_list(webview: tauri::Webview) -> Result<Vec<LoginInfo>, &'static str> {
    allowed_caller(webview.label())?;
    let _guard = LOCK.lock().map_err(|_| "credential_store_unavailable")?;
    Ok(read()?
        .into_iter()
        .map(|entry| LoginInfo {
            id: entry.id,
            origin: entry.origin,
            label: entry.label,
            username: entry.username,
            ask_before_use: entry.ask_before_use,
            login_path: entry.login_path,
            default_for_path: entry.default_for_path,
        })
        .collect())
}

#[tauri::command]
pub(crate) fn saved_login_set(
    webview: tauri::Webview,
    input: LoginInput,
) -> Result<LoginInfo, &'static str> {
    allowed_caller(webview.label())?;
    let _guard = LOCK.lock().map_err(|_| "credential_store_unavailable")?;
    let mut entries = read()?;
    let info = prepare_set(&mut entries, input)?;
    write(&entries)?;
    Ok(info)
}

fn prepare_set(entries: &mut Vec<Entry>, input: LoginInput) -> Result<LoginInfo, &'static str> {
    let origin = origin(&input.origin)?;
    if input.login_path.as_ref().is_some_and(|path| {
        !path.is_empty()
            && (!path.starts_with('/')
                || path.starts_with("//")
                || path.contains(['?', '#', '\\'])
                || path.len() > 2048)
    }) {
        return Err("invalid_input");
    }
    if !valid_text(&input.label, 100)
        || !valid_text(&input.username, 256)
        || input
            .password
            .as_ref()
            .is_some_and(|value| !valid_text(value, 1024))
    {
        return Err("invalid_input");
    }
    if entries.iter().any(|entry| {
        entry.origin == origin && entry.label == input.label && input.id.as_ref() != Some(&entry.id)
    }) {
        return Err("duplicate_label");
    }
    let requested_default = input.default_for_path;
    let id = match input.id {
        Some(id) => {
            let entry = entries
                .iter_mut()
                .find(|entry| entry.id == id)
                .ok_or("not_found")?;
            // Changing the site creates a new authorization boundary.
            if entry.origin != origin {
                return Err("origin_change_requires_new_login");
            }
            entry.label = input.label;
            entry.username = input.username;
            if let Some(ask_before_use) = input.ask_before_use {
                entry.ask_before_use = ask_before_use;
            }
            if let Some(path) = input.login_path {
                entry.login_path = (!path.is_empty()).then_some(path);
            }
            if let Some(default_for_path) = input.default_for_path {
                entry.default_for_path = default_for_path;
            }
            if let Some(password) = input.password {
                entry.password = password;
            }
            id
        }
        None => {
            let password = input.password.ok_or("invalid_input")?;
            let id = uuid::Uuid::new_v4().to_string();
            entries.push(Entry {
                id: id.clone(),
                origin,
                label: input.label,
                username: input.username,
                password,
                ask_before_use: input.ask_before_use.unwrap_or(false),
                login_path: input.login_path.filter(|path| !path.is_empty()),
                default_for_path: input.default_for_path.unwrap_or(false),
            });
            id
        }
    };
    let chosen = entries
        .iter()
        .find(|entry| entry.id == id)
        .ok_or("credential_store_unavailable")?;
    let chosen_origin = chosen.origin.clone();
    let chosen_path = chosen.login_path.clone();
    let chosen_default = chosen.default_for_path;
    if chosen_default {
        let collision = entries.iter().any(|entry| {
            entry.id != id
                && entry.origin == chosen_origin
                && entry.login_path == chosen_path
                && entry.default_for_path
        });
        if collision && requested_default != Some(true) {
            return Err("default_conflict");
        }
        if collision {
            for entry in entries.iter_mut().filter(|entry| {
                entry.id != id && entry.origin == chosen_origin && entry.login_path == chosen_path
            }) {
                entry.default_for_path = false;
            }
        }
    }
    let entry = entries
        .iter()
        .find(|entry| entry.id == id)
        .ok_or("credential_store_unavailable")?;
    Ok(LoginInfo {
        id: entry.id.clone(),
        origin: entry.origin.clone(),
        label: entry.label.clone(),
        username: entry.username.clone(),
        ask_before_use: entry.ask_before_use,
        login_path: entry.login_path.clone(),
        default_for_path: entry.default_for_path,
    })
}

#[tauri::command]
pub(crate) fn saved_login_delete(webview: tauri::Webview, id: String) -> Result<(), &'static str> {
    allowed_caller(webview.label())?;
    let _guard = LOCK.lock().map_err(|_| "credential_store_unavailable")?;
    let mut entries = read()?;
    let before = entries.len();
    entries.retain(|entry| entry.id != id);
    if entries.len() == before {
        return Err("not_found");
    }
    if entries.is_empty() {
        crate::credentials::delete_secret(VAULT).map_err(|_| "credential_store_unavailable")
    } else {
        write(&entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_exact_https_origins() {
        assert_eq!(
            origin("https://example.test"),
            Ok("https://example.test".into())
        );
        assert_ne!(
            origin("https://example.test.evil.test"),
            origin("https://example.test")
        );
        for value in [
            "http://example.test",
            "https://user@example.test",
            "https://example.test/login",
            "https://example.test/?token=x",
        ] {
            assert_eq!(origin(value), Err("invalid_origin"));
        }
    }

    #[test]
    fn panel_and_other_webviews_cannot_manage_saved_logins() {
        assert!(allowed_caller("main").is_ok());
        assert!(allowed_caller("panel-browser").is_err());
        assert!(allowed_caller("panel-popup-1").is_err());
    }

    #[test]
    fn abandoned_account_selection_is_removed() {
        let request_id = uuid::Uuid::new_v4().to_string();
        let (sender, _receiver) = oneshot::channel();
        selections().lock().unwrap().insert(
            request_id.clone(),
            PendingSelection {
                ids: HashSet::new(),
                response: sender,
            },
        );
        let guard = SelectionGuard(request_id.clone());
        drop(guard);
        assert!(!selections().lock().unwrap().contains_key(&request_id));
    }

    #[tokio::test]
    async fn selection_closes_once_for_selection_cancel_stop_and_timeout() {
        for action in ["selected", "cancelled", "stopped", "timeout"] {
            let entries: Vec<LoginInfo> = ["one", "two"]
                .into_iter()
                .map(|id| LoginInfo {
                    id: id.into(),
                    origin: "https://example.test".into(),
                    label: id.into(),
                    username: id.into(),
                    ask_before_use: false,
                    login_path: None,
                    default_for_path: false,
                })
                .collect();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let notify: crate::browser_runtime::Notify = std::sync::Arc::new(move |name, value| {
                tx.send((name.to_owned(), value)).unwrap();
            });
            let token = CancellationToken::new();
            let future = select_for_person_with_timeout(
                "chat",
                &entries,
                &notify,
                token.clone(),
                std::time::Duration::from_millis(if action == "timeout" { 50 } else { 5_000 }),
            );
            tokio::pin!(future);
            let (result, request_id) = tokio::join!(future, async {
                let (name, request) = rx.recv().await.unwrap();
                assert_eq!(name, "saved-login-selection-request");
                let id = request["requestId"].as_str().unwrap().to_owned();
                match action {
                    "selected" => select_login(&id, "one".into()).unwrap(),
                    "cancelled" => cancel_selection(&id).unwrap(),
                    "stopped" => token.cancel(),
                    _ => (),
                }
                id
            });
            let (name, closed) = rx.recv().await.unwrap();
            assert_eq!(name, "saved-login-selection-closed");
            assert_eq!(closed["requestId"], request_id);
            let reason = if action == "stopped" {
                "cancelled"
            } else {
                action
            };
            assert_eq!(closed["reason"], reason);
            assert_eq!(
                result
                    .as_ref()
                    .map(|entry| entry.id.as_str())
                    .map_err(|error| *error),
                if action == "selected" {
                    Ok("one")
                } else {
                    Err(reason)
                }
            );
            assert!(rx.try_recv().is_err());
            assert!(!selections().lock().unwrap().contains_key(&request_id));
        }
    }

    #[test]
    fn login_paths_choose_the_matching_account_and_default_to_auto() {
        let implicit: LoginInput = serde_json::from_value(serde_json::json!({
            "origin":"https://example.test", "label":"normal", "username":"person"
        }))
        .unwrap();
        assert!(implicit.ask_before_use.is_none());
        let make = |label: &str, path: Option<&str>| LoginInput {
            id: None,
            origin: "https://example.test".into(),
            label: label.into(),
            username: label.into(),
            password: Some(uuid::Uuid::new_v4().to_string()),
            ask_before_use: None,
            login_path: path.map(str::to_owned),
            default_for_path: None,
        };
        let mut vault = Vec::new();
        let normal = prepare_set(&mut vault, make("normal", Some("/login"))).unwrap();
        let admin = prepare_set(&mut vault, make("admin", Some("/admin"))).unwrap();
        let results = match_login_path(vec![normal.clone(), admin], "/admin");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].label, "admin");
        assert!(!results[0].ask_before_use);
        assert!(match_login_path(vec![normal], "/unknown").is_empty());
        let admin_id = vault[1].id.clone();
        vault[1].ask_before_use = true;
        let updated = prepare_set(
            &mut vault,
            LoginInput {
                id: Some(admin_id),
                origin: "https://example.test".into(),
                label: "admin".into(),
                username: "admin".into(),
                password: None,
                ask_before_use: None,
                login_path: None,
                default_for_path: None,
            },
        )
        .unwrap();
        assert!(updated.ask_before_use);
        assert_eq!(updated.login_path.as_deref(), Some("/admin"));
        let second = prepare_set(&mut vault, make("second", Some("/admin"))).unwrap();
        assert_eq!(
            match_login_path(vec![updated.clone(), second.clone()], "/admin").len(),
            2
        );
        vault[1].default_for_path = true;
        let chosen = prepare_set(
            &mut vault,
            LoginInput {
                id: Some(second.id),
                origin: "https://example.test".into(),
                label: "second".into(),
                username: "second".into(),
                password: None,
                ask_before_use: None,
                login_path: None,
                default_for_path: Some(true),
            },
        )
        .unwrap();
        assert!(!vault[1].default_for_path);
        let selected = match_login_path(vec![updated, chosen], "/admin");
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].label, "second");
    }

    #[test]
    fn list_shape_never_serializes_a_password() {
        let secret = uuid::Uuid::new_v4().to_string();
        let entry = Entry {
            id: "id".into(),
            origin: "https://example.test".into(),
            label: "label".into(),
            username: "user".into(),
            password: secret.clone(),
            ask_before_use: false,
            login_path: None,
            default_for_path: false,
        };
        let info = LoginInfo {
            id: entry.id,
            origin: entry.origin,
            label: entry.label,
            username: entry.username,
            ask_before_use: entry.ask_before_use,
            login_path: entry.login_path,
            default_for_path: entry.default_for_path,
        };
        assert!(!serde_json::to_string(&info).unwrap().contains(&secret));
    }

    #[test]
    fn browser_result_redacts_secret_in_nested_status_and_url() {
        let secret = uuid::Uuid::new_v4().to_string();
        let mut result = serde_json::json!({"status":"blocked", "items":[{"url":format!("https://example.test/?value={secret}")}, secret.clone()]});
        redact_value(&mut result, &[secret.as_str()]);
        assert!(!result.to_string().contains(&secret));
    }

    #[test]
    fn stream_redacts_password_split_across_chunks() {
        let secret = format!("가{}나", uuid::Uuid::new_v4());
        let text = format!("before {secret} after");
        let mut stream = SecretStream::default();
        let mut output = String::new();
        for character in text.chars() {
            output.push_str(&stream.push(
                &character.to_string(),
                std::slice::from_ref(&secret),
                false,
            ));
        }
        output.push_str(&stream.push("", std::slice::from_ref(&secret), true));
        assert!(!output.contains(&secret));
        assert_eq!(output, "before [가림] after");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires a process-local fake credential store"]
    fn saved_login_chat_database_contains_no_secret() {
        let secret = uuid::Uuid::new_v4().to_string();
        fixture_save_login("https://example.test", &secret, false).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "fixture").unwrap();
        let created =
            crate::chat::create_with_message(&db, &format!("before {secret} after"), None).unwrap();
        let summary = serde_json::to_value(created.chat).unwrap();
        let id = summary["id"].as_str().unwrap();
        let answer = crate::chat::append_finished(
            &db,
            id,
            &secret,
            "codex".into(),
            None,
            None,
            crate::chat::FinishReason::Completed,
        )
        .unwrap();
        assert!(!serde_json::to_string(&answer).unwrap().contains(&secret));
        let detail = serde_json::to_string(&crate::chat::read(&db, id).unwrap()).unwrap();
        assert!(!detail.contains(&secret));
        for item in walkdir::WalkDir::new(temp.path()) {
            let item = item.unwrap();
            if item.file_type().is_file() {
                let bytes = std::fs::read(item.path()).unwrap();
                assert!(!bytes
                    .windows(secret.len())
                    .any(|part| part == secret.as_bytes()));
            }
        }
    }

    #[test]
    fn multiple_logins_edit_without_exposing_secret_or_changing_origin() {
        let secret = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        let mut entries = Vec::new();
        let input = |label: &str, password: String| LoginInput {
            id: None,
            origin: "https://example.test".into(),
            label: label.into(),
            username: "person".into(),
            password: Some(password),
            ask_before_use: None,
            login_path: None,
            default_for_path: None,
        };
        let first = prepare_set(&mut entries, input("one", secret.clone())).unwrap();
        let other = prepare_set(&mut entries, input("two", second)).unwrap();
        assert_ne!(first.id, other.id);
        let duplicate = prepare_set(&mut entries, input("one", uuid::Uuid::new_v4().to_string()));
        assert!(matches!(duplicate, Err("duplicate_label")));
        assert!(!serde_json::to_string(&first).unwrap().contains(&secret));
        let updated = prepare_set(
            &mut entries,
            LoginInput {
                id: Some(first.id.clone()),
                origin: first.origin.clone(),
                label: "changed".into(),
                username: "person".into(),
                password: None,
                ask_before_use: None,
                login_path: None,
                default_for_path: None,
            },
        )
        .unwrap();
        assert_eq!(updated.label, "changed");
        assert_eq!(entries[0].password, secret);
        assert!(!matches_selected(&entries[0], &first));
        assert!(matches_selected(&entries[0], &updated));
        entries[0].username = "another-person".into();
        assert!(!matches_selected(&entries[0], &updated));
        let rejected = prepare_set(
            &mut entries,
            LoginInput {
                id: Some(first.id),
                origin: "https://example.test.evil.test".into(),
                label: "changed".into(),
                username: "person".into(),
                password: None,
                ask_before_use: None,
                login_path: None,
                default_for_path: None,
            },
        );
        assert!(matches!(rejected, Err("origin_change_requires_new_login")));
    }

    #[test]
    fn adding_accounts_retires_only_removed_passwords() {
        let make = |label: &str| LoginInput {
            id: None,
            origin: "https://example.test".into(),
            label: label.into(),
            username: label.into(),
            password: Some(uuid::Uuid::new_v4().to_string()),
            ask_before_use: None,
            login_path: None,
            default_for_path: None,
        };
        let mut current = Vec::new();
        prepare_set(&mut current, make("one")).unwrap();
        let old: Vec<Entry> =
            serde_json::from_str(&serde_json::to_string(&current).unwrap()).unwrap();
        prepare_set(&mut current, make("two")).unwrap();
        assert!(retired_entries(old, &current).is_empty());
        let old: Vec<Entry> =
            serde_json::from_str(&serde_json::to_string(&current).unwrap()).unwrap();
        current.remove(0);
        assert_eq!(retired_entries(old, &current).len(), 1);
    }
}
