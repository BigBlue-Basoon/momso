use crate::{chat_claude, connections, credentials, gemini_access, process, usage_status};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;

enum PendingOperation {
    Login(CancellationToken),
    Logout,
}
type Pending = Mutex<HashMap<String, PendingOperation>>;
static PENDING: OnceLock<Pending> = OnceLock::new();

fn pending() -> &'static Pending {
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

struct Reservation(String);
impl Drop for Reservation {
    fn drop(&mut self) {
        if let Ok(mut tasks) = pending().lock() {
            tasks.remove(&self.0);
        }
    }
}

fn reserve(provider: &str, operation: PendingOperation) -> Result<Reservation, &'static str> {
    let mut tasks = pending().lock().map_err(|_| "unavailable")?;
    if tasks.contains_key(provider) {
        return Err("inProgress");
    }
    tasks.insert(provider.to_owned(), operation);
    Ok(Reservation(provider.to_owned()))
}

fn key_provider(provider: &str) -> Option<&'static str> {
    match provider {
        "codex" => Some("openai"),
        "claude" => Some("anthropic"),
        "gemini" => Some("gemini"),
        _ => None,
    }
}

fn gemini_connection_state(installed: bool, key_saved: Option<bool>) -> &'static str {
    if !installed {
        return "unavailable";
    }
    match key_saved {
        Some(true) => "apiKey",
        Some(false) => "signedOut",
        None => "unavailable",
    }
}

fn gemini_key_status(key: Option<&str>) -> (bool, Option<bool>) {
    match key {
        Some(value) => (true, Some(!value.trim().is_empty())),
        None => (false, Some(false)),
    }
}

fn claude_connection_state(success: bool, value: &serde_json::Value) -> &'static str {
    if value.get("loggedIn").and_then(serde_json::Value::as_bool) == Some(false) {
        "signedOut"
    } else if success
        && value.get("loggedIn").and_then(serde_json::Value::as_bool) == Some(true)
        && value.get("authMethod").and_then(serde_json::Value::as_str) == Some("claude.ai")
    {
        "account"
    } else {
        "unavailable"
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderStatus {
    provider: &'static str,
    pub(crate) installed: bool,
    pub(crate) state: &'static str,
    account_label: Option<String>,
    plan_label: Option<String>,
    pub(crate) api_key_saved: bool,
    methods: Vec<&'static str>,
    pub(crate) active: Option<&'static str>,
}

pub(crate) fn chat_auth_method(
    provider: &str,
    installed: bool,
    state: &str,
    active: Option<&str>,
    key_saved: bool,
    requested: Option<&str>,
) -> Result<&'static str, &'static str> {
    if !installed {
        return Err("unavailable");
    }
    match provider {
        "gemini" => match requested {
            None | Some("apiKey") => Ok("apiKey"),
            _ => Err("unavailable"),
        },
        "claude" => match requested.or(active) {
            Some("account") if state == "account" => Ok("account"),
            Some("apiKey") => Ok("apiKey"),
            None | Some("account") => Err("not_connected"),
            _ => Err("unavailable"),
        },
        "codex" => match (requested, active) {
            (None, Some("account")) | (Some("account"), Some("account")) => Ok("account"),
            (None, Some("apiKey")) => Ok("apiKey"),
            (Some("apiKey"), _) if key_saved => Ok("apiKey"),
            (Some("apiKey"), _) => Err("not_connected"),
            (None | Some("account"), None) if state == "signedOut" => Err("not_connected"),
            _ => Err("unavailable"),
        },
        _ => Err("unavailable"),
    }
}

#[derive(Clone, Serialize)]
struct Change<'a> {
    provider: &'a str,
    state: &'static str,
}

pub(crate) fn notify(app: &AppHandle, provider: &str, state: &'static str) {
    let _ = app.emit("provider-auth-changed", Change { provider, state });
}

async fn run_login_tool(
    name: &str,
    args: &[&str],
    token: CancellationToken,
) -> Result<process::ProcessOutput, process::ProcessError> {
    process::run_tool_cancellable(
        name,
        args,
        None,
        None,
        &process::empty_environment(),
        Duration::from_secs(300),
        Some(token),
    )
    .await
}

pub(crate) async fn read_status(provider: &'static str) -> ProviderStatus {
    let installed = if provider == "gemini" {
        gemini_access::executable().is_some()
    } else if provider == "claude" {
        chat_claude::executable().is_some()
    } else {
        process::resolve_interactive_tool(provider).is_some()
    };
    let (api_key_saved, key_usable) = if provider == "gemini" {
        match credentials::ai_key_name("gemini").and_then(credentials::get_secret) {
            Ok(key) => gemini_key_status(key.as_deref()),
            Err(_) => (false, None),
        }
    } else {
        let saved = key_provider(provider)
            .and_then(|p| credentials::ai_key_name(p).ok())
            .and_then(|name| credentials::has_secret(name).ok());
        (saved == Some(true), saved)
    };
    let mut status = ProviderStatus {
        provider,
        installed,
        state: "unavailable",
        account_label: None,
        plan_label: None,
        api_key_saved,
        methods: if provider == "gemini" {
            vec!["apiKey"]
        } else {
            vec!["account", "apiKey"]
        },
        active: None,
    };
    if provider == "gemini" {
        status.state = gemini_connection_state(installed, key_usable);
        return status;
    }
    if !installed {
        return status;
    }
    if provider == "codex" {
        let result = process::run_json_rpc_tool("codex", &["app-server"], None, &process::empty_environment(), process::JsonRpcRequest {
            initialize_params: serde_json::json!({"clientInfo": {"name":"momso", "title":"momso", "version":env!("CARGO_PKG_VERSION")}}),
            method: "account/read", params: serde_json::json!({"refreshToken":false}),
        }, Duration::from_secs(12)).await;
        if let Ok(result) = result {
            match result.get("account") {
                Some(serde_json::Value::Null) => status.state = "signedOut",
                Some(account)
                    if account.get("type").and_then(|v| v.as_str()) == Some("chatgpt") =>
                {
                    status.state = "account";
                    status.active = Some("account");
                    status.account_label = account
                        .get("email")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned);
                    status.plan_label = account
                        .get("planType")
                        .and_then(|v| v.as_str())
                        .and_then(|plan| usage_status::plan_label("codex", plan))
                        .map(str::to_owned);
                }
                Some(account) if account.get("type").and_then(|v| v.as_str()) == Some("apiKey") => {
                    status.state = "apiKey";
                    status.active = Some("apiKey");
                }
                _ => {}
            }
        }
    } else {
        let result = process::run_tool(
            "claude",
            &["auth", "status", "--json"],
            None,
            None,
            &process::empty_environment(),
            Duration::from_secs(15),
        )
        .await;
        if let Ok(output) = result {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&output.stdout) {
                status.state = claude_connection_state(output.success, &value);
                if status.state == "account" {
                    status.active = Some("account");
                    status.account_label = value
                        .get("email")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned);
                }
            }
        }
    }
    if matches!(provider, "codex" | "claude")
        && status.state == "signedOut"
        && key_usable == Some(true)
    {
        status.state = "apiKey";
        status.active = Some("apiKey");
    }
    status
}

#[tauri::command]
pub(crate) async fn provider_auth_status() -> Vec<ProviderStatus> {
    let (codex, claude, gemini) = tokio::join!(
        read_status("codex"),
        read_status("claude"),
        read_status("gemini")
    );
    vec![codex, claude, gemini]
}

#[tauri::command]
pub(crate) async fn provider_login_start(app: AppHandle, provider: String) -> &'static str {
    if !matches!(provider.as_str(), "codex" | "claude") {
        return "unavailable";
    }
    if process::resolve_interactive_tool(&provider).is_none() {
        return "unavailable";
    }
    let token = CancellationToken::new();
    let reservation = match reserve(&provider, PendingOperation::Login(token.clone())) {
        Ok(reservation) => reservation,
        Err(state) => return state,
    };
    notify(&app, &provider, "inProgress");
    tauri::async_runtime::spawn(async move {
        let args: &[&str] = if provider == "codex" {
            &["login"]
        } else {
            &["auth", "login", "--claudeai"]
        };
        let result = run_login_tool(&provider, args, token.clone()).await;
        let state = if token.is_cancelled() {
            "cancelled"
        } else if result.is_ok_and(|o| o.success) {
            if provider == "codex" {
                read_status("codex").await.state
            } else {
                read_status("claude").await.state
            }
        } else {
            "unavailable"
        };
        drop(reservation);
        notify(&app, &provider, state);
    });
    "inProgress"
}

#[tauri::command]
pub(crate) fn provider_login_cancel(provider: String) -> &'static str {
    if key_provider(&provider).is_none() {
        return "unavailable";
    }
    let Ok(tasks) = pending().lock() else {
        return "unavailable";
    };
    match tasks.get(&provider) {
        Some(PendingOperation::Login(token)) => {
            token.cancel();
            "cancelled"
        }
        Some(PendingOperation::Logout) => "inProgress",
        None => "notInProgress",
    }
}

#[tauri::command]
pub(crate) async fn provider_logout(app: AppHandle, provider: String) -> &'static str {
    let args: &[&str] = match provider.as_str() {
        "codex" => &["logout"],
        "claude" => &["auth", "logout"],
        _ => return "unavailable",
    };
    let reservation = match reserve(&provider, PendingOperation::Logout) {
        Ok(reservation) => reservation,
        Err(state) => return state,
    };
    let result = process::run_tool(
        &provider,
        args,
        None,
        None,
        &process::empty_environment(),
        Duration::from_secs(20),
    )
    .await;
    let state = if result.is_ok_and(|o| o.success) {
        "signedOut"
    } else {
        "unavailable"
    };
    drop(reservation);
    notify(&app, &provider, state);
    state
}

#[tauri::command]
pub(crate) fn provider_api_key_set(
    app: AppHandle,
    provider: String,
    api_key: String,
) -> &'static str {
    let Some(name) = key_provider(&provider) else {
        return "unavailable";
    };
    if !valid_api_key_shape(&api_key) {
        return "invalidKey";
    }
    if connections::save_api_key(name, &api_key).is_err() {
        return "unavailable";
    }
    notify(&app, &provider, "keySaved");
    "keySaved"
}

fn valid_api_key_shape(api_key: &str) -> bool {
    api_key.trim().len() >= 20 && !api_key.trim().chars().any(char::is_whitespace)
}

#[tauri::command]
pub(crate) fn provider_api_key_clear(app: AppHandle, provider: String) -> &'static str {
    let Some(name) = key_provider(&provider) else {
        return "unavailable";
    };
    if connections::delete_api_key(name).is_err() {
        return "unavailable";
    }
    notify(&app, &provider, "keyCleared");
    "keyCleared"
}

#[cfg(test)]
mod tests {
    #[test]
    fn gemini_key_shape_rejects_invalid_without_contacting_provider() {
        assert!(!super::valid_api_key_shape("fake-short"));
        assert!(!super::valid_api_key_shape("fake-key-with a-space-123456"));
        assert!(super::valid_api_key_shape(
            "fake-key-invalid-but-long-enough"
        ));
    }
    use super::*;
    #[test]
    fn chat_auth_choices_and_absent_tools_have_consistent_states() {
        for provider in ["codex", "claude", "gemini"] {
            assert_eq!(
                chat_auth_method(provider, false, "unavailable", None, false, None),
                Err("unavailable")
            );
            assert_eq!(
                chat_auth_method(provider, true, "signedOut", None, false, Some("invalid")),
                Err("unavailable")
            );
        }
        for provider in ["codex", "claude"] {
            assert_eq!(
                chat_auth_method(provider, true, "account", Some("account"), true, None),
                Ok("account")
            );
            assert_eq!(
                chat_auth_method(
                    provider,
                    true,
                    "account",
                    Some("account"),
                    true,
                    Some("apiKey")
                ),
                Ok("apiKey")
            );
            assert_eq!(
                chat_auth_method(provider, true, "signedOut", None, false, None),
                Err("not_connected")
            );
        }
        assert_eq!(
            chat_auth_method("gemini", true, "signedOut", None, false, Some("account")),
            Err("unavailable")
        );
        assert_eq!(
            chat_auth_method("gemini", true, "signedOut", None, false, None),
            Ok("apiKey")
        );
    }
    #[test]
    fn gemini_requires_both_cli_and_saved_key_to_be_connected() {
        assert_eq!(gemini_connection_state(false, Some(true)), "unavailable");
        assert_eq!(gemini_connection_state(false, Some(false)), "unavailable");
        assert_eq!(gemini_connection_state(true, Some(true)), "apiKey");
        assert_eq!(gemini_connection_state(true, Some(false)), "signedOut");
        assert_eq!(gemini_connection_state(true, None), "unavailable");
        assert_eq!(gemini_key_status(Some("  ")), (true, Some(false)));
        assert_eq!(gemini_key_status(Some("fake-key")), (true, Some(true)));
        assert_eq!(gemini_key_status(None), (false, Some(false)));
    }
    #[test]
    fn claude_status_matches_the_auth_modes_chat_can_use() {
        use serde_json::json;
        assert_eq!(
            claude_connection_state(true, &json!({"loggedIn":true,"authMethod":"claude.ai"})),
            "account"
        );
        assert_eq!(
            claude_connection_state(false, &json!({"loggedIn":true,"authMethod":"claude.ai"})),
            "unavailable"
        );
        assert_eq!(
            claude_connection_state(true, &json!({"loggedIn":true,"authMethod":"api_key"})),
            "unavailable"
        );
        assert_eq!(
            claude_connection_state(false, &json!({"loggedIn":false})),
            "signedOut"
        );
        assert_eq!(claude_connection_state(true, &json!({})), "unavailable");
    }
    #[cfg(windows)]
    fn count_test_key<R: std::io::Read>(mut input: R, secret: &[u8]) -> std::io::Result<usize> {
        let mut matches = 0usize;
        let mut tail = Vec::new();
        let mut block = [0u8; 65536];
        loop {
            let n = input.read(&mut block)?;
            if n == 0 {
                return Ok(matches);
            }
            tail.extend_from_slice(&block[..n]);
            matches += tail
                .windows(secret.len())
                .filter(|part| *part == secret)
                .count();
            let retain = (secret.len() - 1).min(tail.len());
            tail.drain(..tail.len() - retain);
        }
    }

    #[test]
    #[cfg(windows)]
    fn scan_detects_key_across_chunk_boundaries() {
        let secret = b"fake-test-key";
        let mut bytes = vec![b'x'; 65530];
        bytes.extend_from_slice(secret);
        bytes.extend_from_slice(b"other-data");
        bytes.extend_from_slice(secret);
        assert_eq!(count_test_key(bytes.as_slice(), secret).unwrap(), 2);
        assert_eq!(count_test_key(b"nothing".as_slice(), secret).unwrap(), 0);
    }
    #[test]
    fn providers_map_only_to_expected_key_names() {
        assert_eq!(key_provider("codex"), Some("openai"));
        assert_eq!(key_provider("claude"), Some("anthropic"));
        assert_eq!(key_provider("gemini"), Some("gemini"));
        assert_eq!(key_provider("other"), None);
    }

    #[test]
    fn login_and_logout_reservations_are_exclusive_and_release_on_drop() {
        let provider = format!("test:{}", uuid::Uuid::new_v4());
        let login = reserve(&provider, PendingOperation::Login(CancellationToken::new()))
            .ok()
            .unwrap();
        assert!(matches!(
            reserve(&provider, PendingOperation::Logout),
            Err("inProgress")
        ));
        drop(login);
        let logout = reserve(&provider, PendingOperation::Logout).ok().unwrap();
        assert!(matches!(
            reserve(&provider, PendingOperation::Login(CancellationToken::new())),
            Err("inProgress")
        ));
        drop(logout);
        assert!(reserve(&provider, PendingOperation::Logout).is_ok());
    }

    #[tokio::test]
    #[cfg(windows)]
    async fn fake_cli_finishes_and_cancel_stops_it() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("fake.ps1");
        std::fs::write(
            &script,
            "param($action)\nif ($action -eq 'wait') { Start-Sleep -Seconds 30 }\nexit 0\n",
        )
        .unwrap();
        let path = script.to_str().unwrap();
        for action in ["login", "logout"] {
            let output = run_login_tool(
                "powershell",
                &["-NoProfile", "-File", path, action],
                CancellationToken::new(),
            )
            .await
            .unwrap();
            assert!(output.success);
        }
        let token = CancellationToken::new();
        let args = ["-NoProfile", "-File", path, "wait"];
        let future = run_login_tool("powershell", &args, token.clone());
        tokio::pin!(future);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(200)) => token.cancel(),
            _ = &mut future => panic!("fake process exited too early"),
        }
        assert!(matches!(
            future.await,
            Err(process::ProcessError::Cancelled)
        ));
    }

    #[test]
    #[cfg(windows)]
    fn test_credential_is_not_written_to_isolated_app_or_archive() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app-data");
        let archive = root.path().join("archive");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&archive).unwrap();
        rusqlite::Connection::open(app.join("test.sqlite"))
            .unwrap()
            .execute("CREATE TABLE test_data (value TEXT)", [])
            .unwrap();
        std::fs::write(app.join("test.log"), "test log\n").unwrap();
        std::fs::write(archive.join("test.md"), "test document\n").unwrap();
        let name = format!("test:provider-auth:{}", uuid::Uuid::new_v4());
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = credentials::delete_secret(&self.0);
            }
        }
        let entry = Cleanup(name);
        let secret = format!("test-only-{}", uuid::Uuid::new_v4());
        credentials::set_secret(&entry.0, &secret).unwrap();
        assert!(credentials::get_secret(&entry.0).unwrap().as_deref() == Some(secret.as_str()));
        for item in walkdir::WalkDir::new(root.path()) {
            let item = item.unwrap();
            if item.file_type().is_file() {
                let bytes = std::fs::read(item.path()).unwrap();
                assert!(!bytes
                    .windows(secret.len())
                    .any(|part| part == secret.as_bytes()));
            }
        }
        credentials::delete_secret(&entry.0).unwrap();
    }

    #[test]
    #[cfg(windows)]
    #[ignore = "manual read-only scan of the current user's app data"]
    fn scan_real_app_data_for_test_credential() {
        let name = format!("test:provider-auth-scan:{}", uuid::Uuid::new_v4());
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = credentials::delete_secret(&self.0);
            }
        }
        let entry = Cleanup(name);
        let secret = format!("momso-test-scan-{}", uuid::Uuid::new_v4());
        credentials::set_secret(&entry.0, &secret).unwrap();
        let mut roots = Vec::new();
        for variable in ["APPDATA", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(variable) {
                roots.push(std::path::PathBuf::from(base).join("app.momso.desktop"));
            }
        }
        let archive_exists = roots.first().is_some_and(|p| p.join("archive").is_dir());
        let mut matches = 0usize;
        let mut skipped = 0usize;
        let mut files = 0usize;
        for root in &roots {
            if !root.is_dir() {
                skipped += 1;
                continue;
            }
            for item in walkdir::WalkDir::new(root).follow_links(false) {
                let Ok(item) = item else {
                    skipped += 1;
                    continue;
                };
                if item.file_type().is_symlink() {
                    skipped += 1;
                    continue;
                }
                if !item.file_type().is_file() {
                    continue;
                }
                let Ok(file) = std::fs::File::open(item.path()) else {
                    skipped += 1;
                    continue;
                };
                files += 1;
                match count_test_key(file, secret.as_bytes()) {
                    Ok(found) => matches += found,
                    Err(_) => skipped += 1,
                }
            }
        }
        credentials::delete_secret(&entry.0).unwrap();
        println!("files={files} matches={matches} skipped={skipped} default_archive_exists={archive_exists}");
        assert_eq!(matches, 0);
    }

    #[test]
    #[cfg(windows)]
    #[ignore = "manual read-only scan of the specified development archive"]
    fn scan_development_archive_for_test_credential() {
        let root = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap())
            .join("app.momso.dev-preview-20261114")
            .join("archive");
        assert!(root.is_dir());
        let name = format!("test:provider-auth-archive:{}", uuid::Uuid::new_v4());
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = credentials::delete_secret(&self.0);
            }
        }
        let entry = Cleanup(name);
        let secret = format!("momso-test-archive-{}", uuid::Uuid::new_v4());
        credentials::set_secret(&entry.0, &secret).unwrap();
        let mut files = 0usize;
        let mut matches = 0usize;
        let mut skipped = 0usize;
        for item in walkdir::WalkDir::new(&root).follow_links(false) {
            let Ok(item) = item else {
                skipped += 1;
                continue;
            };
            if item.file_type().is_symlink() {
                skipped += 1;
                continue;
            }
            if !item.file_type().is_file() {
                continue;
            }
            let Ok(file) = std::fs::File::open(item.path()) else {
                skipped += 1;
                continue;
            };
            files += 1;
            match count_test_key(file, secret.as_bytes()) {
                Ok(found) => matches += found,
                Err(_) => skipped += 1,
            }
        }
        credentials::delete_secret(&entry.0).unwrap();
        println!("files={files} matches={matches} skipped={skipped}");
        assert_eq!(matches, 0);
        assert_eq!(skipped, 0);
    }
}
