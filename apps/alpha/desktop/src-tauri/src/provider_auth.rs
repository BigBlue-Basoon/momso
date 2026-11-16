use crate::{connections, credentials, process, usage_status};
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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderStatus {
    provider: &'static str,
    installed: bool,
    state: &'static str,
    account_label: Option<String>,
    plan_label: Option<String>,
    api_key_saved: bool,
    methods: Vec<&'static str>,
    active: Option<&'static str>,
}

#[derive(Clone, Serialize)]
struct Change<'a> {
    provider: &'a str,
    state: &'static str,
}

fn notify(app: &AppHandle, provider: &str, state: &'static str) {
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

async fn read_status(provider: &'static str) -> ProviderStatus {
    let mut installed = process::resolve_interactive_tool(if provider == "gemini" {
        "agy"
    } else {
        provider
    })
    .is_some();
    if provider == "gemini" && !installed {
        installed = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .is_some_and(|base| base.join("agy").join("bin").join("agy.exe").is_file());
    }
    let api_key_saved = key_provider(provider)
        .and_then(|p| credentials::ai_key_name(p).ok())
        .and_then(|name| credentials::has_secret(name).ok())
        .unwrap_or(false);
    let mut status = ProviderStatus {
        provider,
        installed,
        state: "unavailable",
        account_label: None,
        plan_label: None,
        api_key_saved,
        methods: if provider == "gemini" {
            vec![]
        } else {
            vec!["account", "apiKey"]
        },
        active: None,
    };
    if !installed || provider == "gemini" {
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
                if value.get("loggedIn").and_then(|v| v.as_bool()) == Some(false) {
                    status.state = "signedOut";
                } else if value.get("loggedIn").and_then(|v| v.as_bool()) == Some(true) {
                    let method = value.get("authMethod").and_then(|v| v.as_str());
                    if method == Some("claude.ai") {
                        status.state = "account";
                        status.active = Some("account");
                    } else if method == Some("api_key") {
                        status.state = "apiKey";
                        status.active = Some("apiKey");
                    }
                    status.account_label = value
                        .get("email")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned);
                }
            }
        }
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
    if api_key.trim().len() < 20 || api_key.trim().chars().any(char::is_whitespace) {
        return "invalidKey";
    }
    if connections::save_api_key(name, &api_key).is_err() {
        return "unavailable";
    }
    notify(&app, &provider, "keySaved");
    "keySaved"
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
    use super::*;
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
