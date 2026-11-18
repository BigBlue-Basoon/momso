use crate::{chat_claude, connections, credentials, gemini_access, process, usage_status};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
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
type NoticeSenders = Mutex<HashMap<String, (uuid::Uuid, tokio::sync::oneshot::Sender<()>)>>;
static BROWSER_NOTICES: OnceLock<NoticeSenders> = OnceLock::new();
static GH_LOGIN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static GH_RECOVERY_NOTICE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn browser_notices() -> &'static NoticeSenders {
    BROWSER_NOTICES.get_or_init(|| Mutex::new(HashMap::new()))
}

struct BrowserNoticeGuard {
    provider: String,
    id: uuid::Uuid,
}

impl Drop for BrowserNoticeGuard {
    fn drop(&mut self) {
        if let Ok(mut notices) = browser_notices().lock() {
            if notices
                .get(&self.provider)
                .is_some_and(|(id, _)| *id == self.id)
            {
                notices.remove(&self.provider);
            }
        }
    }
}

fn acknowledge_browser_notice(provider: &str) -> &'static str {
    let Ok(mut notices) = browser_notices().lock() else {
        return "unavailable";
    };
    match notices.remove(provider) {
        Some((_, sender)) => {
            if sender.send(()).is_ok() {
                "acknowledged"
            } else {
                "notWaiting"
            }
        }
        None => "notWaiting",
    }
}

#[tauri::command]
pub(crate) fn provider_login_browser_ack(provider: String) -> &'static str {
    if !matches!(provider.as_str(), "claude" | "gemini") {
        return "unavailable";
    }
    acknowledge_browser_notice(&provider)
}

#[derive(Clone)]
struct KeyCheck {
    fingerprint: [u8; 32],
    generation: uuid::Uuid,
    state: &'static str,
}
static KEY_CHECKS: OnceLock<Mutex<HashMap<String, KeyCheck>>> = OnceLock::new();
static RETRY_MONITORS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn key_checks() -> &'static Mutex<HashMap<String, KeyCheck>> {
    KEY_CHECKS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn key_fingerprint(key: &str) -> [u8; 32] {
    Sha256::digest(key.as_bytes()).into()
}

fn begin_key_check_if(
    provider: &str,
    key: &str,
    still_current: impl FnOnce() -> bool,
) -> Option<uuid::Uuid> {
    let mut checks = key_checks().lock().ok()?;
    if !still_current() {
        return None;
    }
    let generation = uuid::Uuid::new_v4();
    checks.insert(
        provider.to_owned(),
        KeyCheck {
            fingerprint: key_fingerprint(key),
            generation,
            state: "verifying",
        },
    );
    Some(generation)
}

#[cfg(test)]
fn begin_key_check(provider: &str, key: &str) -> Option<uuid::Uuid> {
    begin_key_check_if(provider, key, || true)
}

fn begin_stored_key_check(provider: &str, key: &str) -> Option<uuid::Uuid> {
    begin_key_check_if(provider, key, || {
        stored_key_matches(provider, key_fingerprint(key))
    })
}

fn store_and_begin(
    provider: &str,
    key: &str,
    save: impl FnOnce(&str) -> Result<(), ()>,
) -> Result<uuid::Uuid, ()> {
    let mut checks = key_checks().lock().map_err(|_| ())?;
    save(key)?;
    let generation = uuid::Uuid::new_v4();
    checks.insert(
        provider.to_owned(),
        KeyCheck {
            fingerprint: key_fingerprint(key.trim()),
            generation,
            state: "verifying",
        },
    );
    Ok(generation)
}

fn finish_in_checks(
    checks: &mut HashMap<String, KeyCheck>,
    provider: &str,
    generation: uuid::Uuid,
    result: &str,
) -> Option<&'static str> {
    let check = checks.get_mut(provider)?;
    if check.generation != generation {
        return None;
    }
    check.state = match result {
        "authenticated" => "verified",
        "rejected" => "rejected",
        "not_supported" => "notSupported",
        _ => "unavailable",
    };
    Some(check.state)
}

#[cfg(test)]
fn finish_key_check(provider: &str, generation: uuid::Uuid, result: &str) -> Option<&'static str> {
    let mut checks = key_checks().lock().ok()?;
    finish_in_checks(&mut checks, provider, generation, result)
}

fn finish_stored_key_check(
    provider: &str,
    fingerprint: [u8; 32],
    generation: uuid::Uuid,
    result: &str,
) -> Option<&'static str> {
    let mut checks = key_checks().lock().ok()?;
    if !stored_key_matches(provider, fingerprint) {
        return None;
    }
    finish_in_checks(&mut checks, provider, generation, result)
}

async fn key_verification(
    provider: &str,
    key: Option<String>,
    retry_unavailable: bool,
) -> &'static str {
    let Some(key) = key else {
        return "notConfigured";
    };
    let cached = key_checks()
        .lock()
        .ok()
        .and_then(|checks| checks.get(provider).cloned());
    if let Some(check) = cached.filter(|check| check.fingerprint == key_fingerprint(&key)) {
        if !should_retry(check.state, retry_unavailable) {
            return check.state;
        }
    }
    let Some(generation) = begin_stored_key_check(provider, &key) else {
        return "verifying";
    };
    let fingerprint = key_fingerprint(&key);
    let result = connections::verify_api_key(key_provider(provider).unwrap_or(""), key).await;
    finish_stored_key_check(provider, fingerprint, generation, result).unwrap_or("verifying")
}

fn should_retry(state: &str, retry_unavailable: bool) -> bool {
    state == "unavailable" && retry_unavailable
}

pub(crate) fn key_can_be_used(saved: bool, verification: &str) -> bool {
    saved && matches!(verification, "verified" | "notSupported")
}

pub(crate) fn key_matches_checked(checked: Option<[u8; 32]>, key: Option<&str>) -> bool {
    match (checked, key) {
        (Some(expected), Some(key)) => expected == key_fingerprint(key),
        _ => false,
    }
}

fn first_run_allowed(statuses: &[ProviderStatus]) -> bool {
    statuses.iter().any(|status| {
        status.runnable
            && (status.state == "account"
                || (status.api_key_saved
                    && matches!(
                        status.key_verification,
                        "verified" | "unavailable" | "notSupported"
                    )))
    })
}

fn next_retry_delay(current: Duration) -> Duration {
    current.saturating_mul(2).min(Duration::from_secs(300))
}

fn start_retry_monitor(app: AppHandle, provider: &'static str) {
    let monitors = RETRY_MONITORS.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut active) = monitors.lock() else {
        return;
    };
    if !active.insert(provider.to_owned()) {
        return;
    }
    drop(active);
    tauri::async_runtime::spawn(async move {
        let mut delay = Duration::from_secs(30);
        loop {
            tokio::time::sleep(delay).await;
            let state = key_checks()
                .lock()
                .ok()
                .and_then(|checks| checks.get(provider).map(|entry| entry.state));
            match state {
                Some("verifying") => continue,
                Some("unavailable") => {}
                _ => break,
            }
            let key = key_provider(provider)
                .and_then(|name| credentials::ai_key_name(name).ok())
                .and_then(|name| credentials::get_secret(name).ok().flatten());
            if key.is_none() {
                break;
            }
            notify(&app, provider, "verifying");
            let state = key_verification(provider, key, true).await;
            notify(&app, provider, state);
            if state == "unavailable" {
                delay = next_retry_delay(delay);
            }
        }
        if let Ok(mut active) = RETRY_MONITORS
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
        {
            active.remove(provider);
        }
    });
}

pub(crate) fn ensure_key_retry(app: &AppHandle, status: &ProviderStatus) {
    if status.api_key_saved && status.key_verification == "unavailable" {
        start_retry_monitor(app.clone(), status.provider);
    }
}

pub(crate) fn start_initial_key_rechecks(app: AppHandle) {
    for provider in ["codex", "claude", "gemini"] {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let key = key_provider(provider)
                .and_then(|name| credentials::ai_key_name(name).ok())
                .and_then(|name| credentials::get_secret(name).ok().flatten());
            if key.is_none() {
                return;
            }
            let state = key_verification(provider, key, true).await;
            notify(&app, provider, state);
            if state == "unavailable" {
                start_retry_monitor(app, provider);
            }
        });
    }
}

fn stored_key_matches(provider: &str, fingerprint: [u8; 32]) -> bool {
    key_provider(provider)
        .and_then(|name| credentials::ai_key_name(name).ok())
        .and_then(|name| credentials::get_secret(name).ok().flatten())
        .is_some_and(|key| key_fingerprint(&key) == fingerprint)
}

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

fn static_provider(provider: &str) -> Option<&'static str> {
    match provider {
        "codex" => Some("codex"),
        "claude" => Some("claude"),
        "gemini" => Some("gemini"),
        _ => None,
    }
}

pub(crate) fn reject_unverified_key(app: &AppHandle, provider: &str, fingerprint: [u8; 32]) {
    let Ok(mut checks) = key_checks().lock() else {
        return;
    };
    let Some(check) = checks.get_mut(provider) else {
        return;
    };
    if !stored_key_matches(provider, fingerprint) || !reject_if_unsupported(check, fingerprint) {
        return;
    }
    drop(checks);
    notify(app, provider, "rejected");
}

fn reject_if_unsupported(check: &mut KeyCheck, fingerprint: [u8; 32]) -> bool {
    if check.fingerprint != fingerprint || check.state != "notSupported" {
        return false;
    }
    check.state = "rejected";
    true
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

#[cfg(test)]
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
    pub(crate) runnable: bool,
    pub(crate) state: &'static str,
    account_label: Option<String>,
    plan_label: Option<String>,
    pub(crate) api_key_saved: bool,
    pub(crate) key_verification: &'static str,
    #[serde(skip)]
    pub(crate) checked_key_fingerprint: Option<[u8; 32]>,
    methods: Vec<&'static str>,
    pub(crate) active: Option<&'static str>,
}

pub(crate) fn chat_auth_method(
    provider: &str,
    installed: bool,
    state: &str,
    active: Option<&str>,
    key_verified: bool,
    requested: Option<&str>,
) -> Result<&'static str, &'static str> {
    if !installed {
        return Err("unavailable");
    }
    match provider {
        "gemini" => match requested {
            None | Some("apiKey") if key_verified && state == "apiKey" => Ok("apiKey"),
            None | Some("apiKey") => Err("not_connected"),
            _ => Err("unavailable"),
        },
        "claude" => match requested.or(active) {
            Some("account") if state == "account" => Ok("account"),
            Some("apiKey") if key_verified => Ok("apiKey"),
            Some("apiKey") => Err("not_connected"),
            None | Some("account") => Err("not_connected"),
            _ => Err("unavailable"),
        },
        "codex" => match (requested, active) {
            (None, Some("account")) | (Some("account"), Some("account")) => Ok("account"),
            (None, Some("apiKey")) | (Some("apiKey"), _) if key_verified => Ok("apiKey"),
            (None, Some("apiKey")) | (Some("apiKey"), _) => Err("not_connected"),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

pub(crate) fn notify(app: &AppHandle, provider: &str, state: &'static str) {
    notify_with_reason(app, provider, state, None);
}

fn notify_with_reason(
    app: &AppHandle,
    provider: &str,
    state: &'static str,
    reason: Option<&'static str>,
) {
    let _ = app.emit(
        "provider-auth-changed",
        Change {
            provider,
            state,
            reason,
        },
    );
}

fn github_storage_event_action(reason: &str) -> Option<&'static str> {
    match reason {
        "github_insecure_storage" => Some("removed"),
        "github_cleanup_failed" => Some("cleanup_failed"),
        "github_storage_unknown" => Some("verification_required"),
        _ => None,
    }
}

fn notify_github_storage(app: &AppHandle, reason: &str) {
    if let Some(action) = github_storage_event_action(reason) {
        let _ = app.emit(
            "provider-login-insecure-storage",
            serde_json::json!({ "provider": "github", "action": action }),
        );
    }
}

pub(crate) fn recover_gh_owned(app: &AppHandle) -> Result<(), &'static str> {
    let root = crate::gh_owned::directory().ok_or("github_storage_unknown")?;
    match crate::gh_owned::recover(root) {
        Ok(true) => {
            GH_RECOVERY_NOTICE.store(true, std::sync::atomic::Ordering::SeqCst);
            notify_github_storage(app, "github_insecure_storage");
            Err("github_insecure_storage")
        }
        Ok(false) => Ok(()),
        Err(crate::gh_owned::RecoveryError::Unknown) => {
            notify_github_storage(app, "github_storage_unknown");
            Err("github_storage_unknown")
        }
        Err(crate::gh_owned::RecoveryError::CleanupFailed) => {
            notify_github_storage(app, "github_cleanup_failed");
            Err("github_cleanup_failed")
        }
    }
}

pub(crate) fn start_gh_owned_recovery(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let _guard = GH_LOGIN_LOCK.lock().await;
        let _ = refresh_gh_owned_locked(&app).await;
    });
}

pub(crate) async fn refresh_gh_owned(app: &AppHandle) -> Result<(), &'static str> {
    let Ok(_guard) = GH_LOGIN_LOCK.try_lock() else {
        return Err("inProgress");
    };
    let result = refresh_gh_owned_locked(app).await;
    if GH_RECOVERY_NOTICE.swap(false, std::sync::atomic::Ordering::SeqCst) {
        notify_github_storage(app, "github_insecure_storage");
    }
    if result == Err("github_storage_unknown") {
        notify_github_storage(app, "github_storage_unknown");
    }
    result
}

async fn refresh_gh_owned_locked(app: &AppHandle) -> Result<(), &'static str> {
    crate::gh_owned::select(0);
    match recover_gh_owned(app) {
        Ok(()) | Err("github_insecure_storage") => {}
        Err(reason) => return Err(reason),
    }
    let default = github_authenticated(None).await;
    if default == Ok(true) {
        crate::gh_owned::select(1);
        return Ok(());
    }
    let owned = match crate::gh_owned::directory() {
        Some(directory) => github_authenticated(Some(directory)).await,
        None => Err(()),
    };
    match select_gh_source(default, owned)? {
        Some(source) => {
            crate::gh_owned::select(source);
            Ok(())
        }
        None => Err("signed_out"),
    }
}

fn select_gh_source(
    default: Result<bool, ()>,
    owned: Result<bool, ()>,
) -> Result<Option<u8>, &'static str> {
    if default == Ok(true) {
        return Ok(Some(1));
    }
    if owned == Ok(true) {
        return Ok(Some(2));
    }
    if default.is_err() || owned.is_err() {
        return Err("github_storage_unknown");
    }
    Ok(None)
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

async fn wait_for_browser_notice(
    provider: &str,
    cancel: CancellationToken,
    timeout: Duration,
    emit: impl FnOnce(),
) -> Result<(), &'static str> {
    if cancel.is_cancelled() {
        return Err("cancelled");
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    let id = uuid::Uuid::new_v4();
    {
        let mut notices = browser_notices().lock().map_err(|_| "unavailable")?;
        if notices.contains_key(provider) {
            return Err("unavailable");
        }
        notices.insert(provider.to_owned(), (id, tx));
    }
    let _guard = BrowserNoticeGuard {
        provider: provider.to_owned(),
        id,
    };
    emit();
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err("cancelled"),
        result = rx => result.map_err(|_| "unavailable"),
        _ = tokio::time::sleep(timeout) => Ok(()),
    }
}

async fn warn_default_browser(
    app: &AppHandle,
    provider: &str,
    cancel: CancellationToken,
) -> Result<(), &'static str> {
    let result = wait_for_browser_notice(provider, cancel, Duration::from_secs(30), || {
        let _ = app.emit_to(
            "main",
            "provider-login-browser",
            serde_json::json!({"provider":provider,"mode":"default_browser"}),
        );
    })
    .await;
    let _ = app.emit_to(
        "main",
        "provider-login-browser-closed",
        serde_json::json!({"provider":provider}),
    );
    result
}
async fn after_browser_notice<N, F, Fut, T>(notice: N, run: F) -> Result<T, &'static str>
where
    N: std::future::Future<Output = Result<(), &'static str>>,
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    notice.await?;
    Ok(run().await)
}

struct DevicePrompt {
    provider: &'static str,
    url: bool,
    expecting_code: bool,
    code: Option<String>,
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.next() == Some('[') {
                for ch in chars.by_ref() {
                    if ('@'..='~').contains(&ch) {
                        break;
                    }
                }
            }
        } else if !ch.is_control() || ch == '\t' {
            out.push(ch);
        }
    }
    out
}

impl DevicePrompt {
    fn new(provider: &'static str) -> Self {
        Self {
            provider,
            url: false,
            expecting_code: false,
            code: None,
        }
    }
    fn accept(&mut self, line: &str) -> Option<String> {
        let cleaned = strip_ansi(line);
        let line = cleaned.trim();
        if self.provider == "codex" && line == "https://auth.openai.com/codex/device" {
            self.url = true;
        } else if self.provider == "codex" && line.starts_with("2. Enter this one-time code") {
            self.expecting_code = true;
        } else if self.provider == "github"
            && line
                == "Open this URL to continue in your web browser: https://github.com/login/device"
        {
            self.url = true;
        } else if self.provider == "github" && line.contains("First copy your one-time code: ") {
            self.code = line
                .split_once("First copy your one-time code: ")
                .and_then(|(_, code)| valid_device_code(code.trim()));
        } else if self.provider == "codex" && self.expecting_code && !line.is_empty() {
            self.expecting_code = false;
            self.code = valid_device_code(line);
        }
        if self.url {
            self.code.take()
        } else {
            None
        }
    }
}

fn valid_device_code(value: &str) -> Option<String> {
    ((6..=20).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-'))
    .then(|| value.to_owned())
}

async fn read_device_output<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    output: tokio::sync::mpsc::Sender<String>,
) {
    use tokio::io::AsyncReadExt;
    let mut pending = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let count = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        for byte in &chunk[..count] {
            if *byte == b'\n' {
                if let Ok(line) = String::from_utf8(std::mem::take(&mut pending)) {
                    if output.send(line).await.is_err() {
                        return;
                    }
                }
            } else if pending.len() < 4096 {
                pending.push(*byte);
            } else {
                return;
            }
        }
    }
}

async fn run_device_process<F, Fut, Guard>(
    mut command: tokio::process::Command,
    provider: &'static str,
    cancel: CancellationToken,
    owned_directory: Option<&std::path::Path>,
    open: F,
) -> Result<(), &'static str>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<Guard, &'static str>>,
{
    if let Some(directory) = owned_directory {
        crate::gh_owned::begin_login(directory).map_err(|_| "github_storage_unknown")?;
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            if let Some(directory) = owned_directory {
                let _ = crate::gh_owned::finish_login(directory);
            }
            return Err("unavailable");
        }
    };
    if let Some(directory) = owned_directory {
        if child
            .id()
            .is_none_or(|pid| crate::gh_owned::record_login_pid(directory, pid).is_err())
        {
            let _ = child.kill().await;
            if child.wait().await.is_ok() {
                let _ = crate::gh_owned::finish_login(directory);
            }
            return Err("github_storage_unknown");
        }
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let stdout = child.stdout.take().ok_or("unavailable")?;
    let stderr = child.stderr.take().ok_or("unavailable")?;
    let a = tokio::spawn(read_device_output(stdout, tx.clone()));
    let b = tokio::spawn(read_device_output(stderr, tx));
    let mut prompt = DevicePrompt::new(provider);
    let mut guard = None;
    let mut open = Some(open);
    let mut opened = false;
    let mut receiving = true;
    let timeout = tokio::time::sleep(Duration::from_secs(900));
    tokio::pin!(timeout);
    let result = loop {
        tokio::select! {
            _ = cancel.cancelled() => break Err("cancelled"),
            _ = &mut timeout => break Err("timeout"),
            status = child.wait() => break if opened && status.is_ok_and(|s| s.success()) { Ok(()) } else { Err("unavailable") },
            line = rx.recv(), if receiving => if let Some(line) = line {
                if !opened {
                    if let Some(code) = prompt.accept(&line) {
                        let Some(open) = open.take() else { break Err("unavailable") };
                        match open(code).await {
                            Ok(value) => guard = Some(value),
                            Err(error) => break Err(error),
                        }
                        opened = true;
                    }
                }
            } else { receiving = false },
        }
    };
    a.abort();
    b.abort();
    drop(guard);
    if let Some(directory) = owned_directory {
        let _ = child.kill().await;
        if child.wait().await.is_ok() && crate::gh_owned::finish_login(directory).is_err() {
            return Err("github_storage_unknown");
        }
    }
    result
}

async fn run_device_login(
    app: &AppHandle,
    provider: &'static str,
    cancel: CancellationToken,
) -> Result<(), &'static str> {
    use std::process::Stdio;
    let tool = if provider == "github" { "gh" } else { "codex" };
    let executable = process::resolve_interactive_tool(tool).ok_or("unavailable")?;
    if cfg!(windows)
        && provider == "github"
        && !executable
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("gh.exe"))
    {
        return Err("github_storage_unknown");
    }
    let mut command = process::hidden_async_command(executable);
    process::remove_inherited_ai_secrets(&mut command);
    if provider == "github" {
        let directory = crate::gh_owned::directory().ok_or("unavailable")?;
        std::fs::create_dir_all(directory).map_err(|_| "unavailable")?;
        crate::gh_owned::recover(directory).map_err(|_| "github_storage_unknown")?;
        command
            .env("GH_CONFIG_DIR", directory)
            .args([
                "auth",
                "login",
                "--hostname",
                "github.com",
                "--git-protocol",
                "https",
                "--web",
                "--skip-ssh-key",
            ])
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_ENTERPRISE_TOKEN")
            .env_remove("GITHUB_ENTERPRISE_TOKEN");
    } else {
        command.args(["login", "--device-auth"]);
    }
    command
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let url = if provider == "github" {
        "https://github.com/login/device"
    } else {
        "https://auth.openai.com/codex/device"
    };
    let app2 = app.clone();
    let panel_cancel = cancel.clone();
    let owned_directory = if provider == "github" {
        crate::gh_owned::directory()
    } else {
        None
    };
    let result = run_device_process(command, provider, cancel, owned_directory, move |code| async move {
        let url = reqwest::Url::parse(url).map_err(|_| "unavailable")?;
        let expires = if provider == "codex" { Some(900) } else { None };
        app2.emit_to("main", "provider-login-device", serde_json::json!({"provider":provider,"url":url.as_str(),"code":code,"expiresInSeconds":expires})).map_err(|_| "unavailable")?;
        match crate::panel_browser::open_device_login_panel(app2.clone(), url.clone(), panel_cancel).await {
            Ok(guard) => Ok(Some(guard)),
            Err("cancelled") => Err("cancelled"),
            Err(_) => {
                crate::panel_browser::panel_browser_open_human(app2, url.to_string()).await.map_err(|_| "unavailable")?;
                Ok(None)
            }
        }
    }).await;
    let _ = app.emit_to(
        "main",
        "provider-login-device-closed",
        serde_json::json!({"provider":provider}),
    );
    result
}

async fn github_authenticated(config: Option<&std::path::Path>) -> Result<bool, ()> {
    use std::process::Stdio;
    let Some(executable) = process::resolve_interactive_tool("gh") else {
        return Err(());
    };
    let mut command = process::hidden_async_command(executable);
    process::remove_inherited_ai_secrets(&mut command);
    if let Some(config) = config {
        command.env("GH_CONFIG_DIR", config);
    }
    command
        .args([
            "auth",
            "status",
            "--hostname",
            "github.com",
            "--active",
            "--json",
            "hosts",
        ])
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_ENTERPRISE_TOKEN")
        .env_remove("GITHUB_ENTERPRISE_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let Ok(Ok(output)) = tokio::time::timeout(Duration::from_secs(15), command.output()).await
    else {
        return Err(());
    };
    if !output.status.success() {
        return Err(());
    }
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return Err(());
    };
    parse_gh_status(&json)
}

fn parse_gh_status(json: &serde_json::Value) -> Result<bool, ()> {
    let hosts = json.get("hosts").and_then(|v| v.as_object()).ok_or(())?;
    let Some(entries) = hosts.get("github.com") else {
        return Ok(false);
    };
    let entry = entries
        .as_array()
        .and_then(|v| {
            v.iter()
                .find(|entry| entry.get("active").and_then(|v| v.as_bool()) == Some(true))
        })
        .ok_or(())?;
    let source = entry
        .get("tokenSource")
        .and_then(|v| v.as_str())
        .ok_or(())?;
    if source != "keyring" {
        return if source.ends_with("hosts.yml") {
            Ok(false)
        } else {
            Err(())
        };
    }
    match entry.get("state").and_then(|v| v.as_str()) {
        Some("success") => Ok(true),
        Some("error")
            if entry
                .get("error")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.contains("HTTP 401")) =>
        {
            Ok(false)
        }
        _ => Err(()),
    }
}

async fn gemini_account_login_with(
    executable: &std::path::Path,
    args: &[&str],
    cancel: CancellationToken,
) -> Result<(), &'static str> {
    let environment = process::empty_environment();
    let work = process::run_json_rpc_resolved(
        executable,
        args,
        None,
        &environment,
        process::JsonRpcRequest {
            initialize_params: serde_json::json!({"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"momso","version":env!("CARGO_PKG_VERSION")}}),
            method: "authenticate",
            params: serde_json::json!({"methodId":"oauth-personal"}),
        },
        Duration::from_secs(300),
    );
    tokio::select! {
        _ = cancel.cancelled() => Err("cancelled"),
        result = work => result.map(|_| ()).map_err(|error| match error {
            process::ProcessError::TimedOut => "timeout",
            _ => "unavailable",
        }),
    }
}

fn login_reason(
    result: &Result<process::ProcessOutput, process::ProcessError>,
    cancelled: bool,
) -> Option<&'static str> {
    if matches!(result, Err(process::ProcessError::TimedOut)) && !cancelled {
        Some("timeout")
    } else {
        None
    }
}

pub(crate) async fn read_status(provider: &'static str) -> ProviderStatus {
    read_status_inner(provider, false).await
}

pub(crate) async fn read_status_for_chat(provider: &'static str) -> ProviderStatus {
    read_status_inner(provider, true).await
}

async fn retry_codex_account_read<F, Fut>(
    mut read: F,
    retry: bool,
) -> Result<serde_json::Value, process::ProcessError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<serde_json::Value, process::ProcessError>>,
{
    let first = read().await;
    if first.is_ok() || !retry {
        return first;
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
    read().await
}

async fn read_status_inner(provider: &'static str, retry_unavailable: bool) -> ProviderStatus {
    let executable = if provider == "gemini" {
        gemini_access::executable()
    } else if provider == "claude" {
        chat_claude::executable()
    } else {
        process::resolve_interactive_tool(provider)
    };
    let installed = executable.is_some();
    let key = key_provider(provider)
        .and_then(|p| credentials::ai_key_name(p).ok())
        .map(credentials::get_secret);
    let (api_key_saved, key_verification, checked_key_fingerprint) = match key {
        Some(Ok(key)) => {
            let saved = key.is_some();
            let fingerprint = key.as_deref().map(key_fingerprint);
            (
                saved,
                key_verification(provider, key, retry_unavailable).await,
                fingerprint,
            )
        }
        _ => (false, "unavailable", None),
    };
    let mut status = ProviderStatus {
        provider,
        installed,
        runnable: false,
        state: "unavailable",
        account_label: None,
        plan_label: None,
        api_key_saved,
        key_verification,
        checked_key_fingerprint,
        methods: if provider == "gemini" {
            vec!["apiKey"]
        } else {
            vec!["account", "apiKey"]
        },
        active: None,
    };
    if let Some(path) = executable {
        if !crate::provider_install::executable_works(&path).await {
            return status;
        }
        status.runnable = true;
    }
    if provider == "gemini" {
        let saved = if !api_key_saved && key_verification == "unavailable" {
            None
        } else {
            Some(api_key_saved)
        };
        status.state = gemini_connection_state(installed, saved);
        return status;
    }
    if !installed {
        return status;
    }
    if provider == "codex" {
        let result = retry_codex_account_read(|| async { process::run_json_rpc_tool("codex", &["app-server"], None, &process::empty_environment(), process::JsonRpcRequest {
            initialize_params: serde_json::json!({"clientInfo": {"name":"momso", "title":"momso", "version":env!("CARGO_PKG_VERSION")}}),
            method: "account/read", params: serde_json::json!({"refreshToken":true}),
        }, Duration::from_secs(12)).await }, retry_unavailable).await;
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
    if matches!(provider, "codex" | "claude") && status.state == "signedOut" && api_key_saved {
        status.state = "apiKey";
        status.active = Some("apiKey");
    }
    status
}

#[tauri::command]
pub(crate) async fn provider_auth_status(app: AppHandle) -> Vec<ProviderStatus> {
    let (codex, claude, gemini) = tokio::join!(
        read_status("codex"),
        read_status("claude"),
        read_status("gemini")
    );
    let statuses = vec![codex, claude, gemini];
    for status in &statuses {
        ensure_key_retry(&app, status);
    }
    statuses
}

#[tauri::command]
pub(crate) async fn provider_first_run_eligible(app: AppHandle) -> bool {
    first_run_allowed(&provider_auth_status(app).await)
}

#[tauri::command]
pub(crate) async fn provider_login_start(app: AppHandle, provider: String) -> &'static str {
    if !matches!(provider.as_str(), "codex" | "claude" | "github" | "gemini") {
        return "unavailable";
    }
    let installed = if provider == "gemini" {
        gemini_access::executable().is_some()
    } else {
        process::resolve_interactive_tool(if provider == "github" {
            "gh"
        } else {
            &provider
        })
        .is_some()
    };
    if !installed {
        return "unavailable";
    }
    let token = CancellationToken::new();
    let reservation = match reserve(&provider, PendingOperation::Login(token.clone())) {
        Ok(reservation) => reservation,
        Err(state) => return state,
    };
    notify(&app, &provider, "inProgress");
    tauri::async_runtime::spawn(async move {
        if provider == "codex" || provider == "github" {
            let which = if provider == "github" {
                "github"
            } else {
                "codex"
            };
            let _gh_guard = if which == "github" {
                Some(GH_LOGIN_LOCK.lock().await)
            } else {
                None
            };
            if which == "github" {
                match refresh_gh_owned_locked(&app).await {
                    Ok(()) => {
                        drop(reservation);
                        notify(&app, &provider, "account");
                        return;
                    }
                    Err("signed_out") => {}
                    Err(reason) => {
                        notify_github_storage(&app, reason);
                        drop(reservation);
                        notify_with_reason(&app, &provider, "unavailable", Some(reason));
                        return;
                    }
                }
            }
            let result = run_device_login(&app, which, token.clone()).await;
            let storage_check = if which == "github" {
                recover_gh_owned(&app)
            } else {
                Ok(())
            };
            let state = match result {
                Ok(()) if which == "github" => {
                    if storage_check.is_ok()
                        && github_authenticated(crate::gh_owned::directory()).await == Ok(true)
                    {
                        crate::gh_owned::select(2);
                        "account"
                    } else {
                        "unavailable"
                    }
                }
                Ok(()) => read_status("codex").await.state,
                Err("cancelled") => "cancelled",
                _ => "unavailable",
            };
            let reason = if storage_check.is_err() {
                storage_check.err()
            } else if which == "github" && result.is_ok() && state != "account" {
                Some("github_storage_unknown")
            } else if result == Err("timeout") {
                Some("timeout")
            } else {
                None
            };
            if let Some(reason) = reason {
                notify_github_storage(&app, reason);
            }
            drop(reservation);
            notify_with_reason(&app, &provider, state, reason);
            return;
        }
        if provider == "gemini" {
            let result = after_browser_notice(
                warn_default_browser(&app, &provider, token.clone()),
                || async {
                    let executable = gemini_access::executable().ok_or("unavailable")?;
                    gemini_account_login_with(&executable, &["acp"], token.clone()).await
                },
            )
            .await;
            let (state, reason) = match result {
                Ok(Ok(())) => ("accountLoginComplete", None),
                Ok(Err("cancelled")) | Err("cancelled") => ("cancelled", None),
                Ok(Err("timeout")) => ("unavailable", Some("timeout")),
                _ => ("unavailable", None),
            };
            drop(reservation);
            notify_with_reason(&app, &provider, state, reason);
            return;
        }
        let result =
            match after_browser_notice(warn_default_browser(&app, &provider, token.clone()), || {
                run_login_tool(&provider, &["auth", "login", "--claudeai"], token.clone())
            })
            .await
            {
                Ok(result) => result,
                Err(reason) => {
                    drop(reservation);
                    notify(
                        &app,
                        &provider,
                        if token.is_cancelled() || reason == "cancelled" {
                            "cancelled"
                        } else {
                            "unavailable"
                        },
                    );
                    return;
                }
            };
        let reason = login_reason(&result, token.is_cancelled());
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
        notify_with_reason(&app, &provider, state, reason);
    });
    "inProgress"
}

#[tauri::command]
pub(crate) fn provider_login_cancel(provider: String) -> &'static str {
    if provider != "github" && key_provider(&provider).is_none() {
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
    let Ok(generation) = store_and_begin(&provider, &api_key, |key| {
        connections::save_api_key(name, key).map_err(|_| ())
    }) else {
        notify(&app, &provider, "unavailable");
        return "unavailable";
    };
    notify(&app, &provider, "verifying");
    tauri::async_runtime::spawn(async move {
        let fingerprint = key_fingerprint(api_key.trim());
        let result = connections::verify_api_key(name, api_key.trim().to_owned()).await;
        if let Some(state) = finish_stored_key_check(&provider, fingerprint, generation, result) {
            notify(&app, &provider, state);
            if let ("unavailable", Some(provider)) = (state, static_provider(&provider)) {
                start_retry_monitor(app.clone(), provider);
            }
        }
    });
    "verifying"
}

fn valid_api_key_shape(api_key: &str) -> bool {
    !api_key.trim().is_empty()
}

#[tauri::command]
pub(crate) fn provider_api_key_verify(app: AppHandle, provider: String) -> &'static str {
    let Some(name) = key_provider(&provider) else {
        return "notSupported";
    };
    let key = match credentials::ai_key_name(name).and_then(credentials::get_secret) {
        Ok(Some(key)) => key,
        Ok(None) => return "notConfigured",
        Err(_) => return "unavailable",
    };
    let Some(generation) = begin_stored_key_check(&provider, &key) else {
        return "unavailable";
    };
    notify(&app, &provider, "verifying");
    tauri::async_runtime::spawn(async move {
        let fingerprint = key_fingerprint(&key);
        let result = connections::verify_api_key(name, key).await;
        if let Some(state) = finish_stored_key_check(&provider, fingerprint, generation, result) {
            notify(&app, &provider, state);
            if let ("unavailable", Some(provider)) = (state, static_provider(&provider)) {
                start_retry_monitor(app.clone(), provider);
            }
        }
    });
    "verifying"
}

#[tauri::command]
pub(crate) fn provider_api_key_clear(app: AppHandle, provider: String) -> &'static str {
    let Some(name) = key_provider(&provider) else {
        return "unavailable";
    };
    let Ok(mut checks) = key_checks().lock() else {
        return "unavailable";
    };
    if connections::delete_api_key(name).is_err() {
        return "unavailable";
    }
    checks.remove(&provider);
    drop(checks);
    notify(&app, &provider, "keyCleared");
    "keyCleared"
}

#[cfg(test)]
mod tests {
    #[test]
    fn login_timeout_has_a_distinct_reason() {
        assert_eq!(
            super::login_reason(&Err(crate::process::ProcessError::TimedOut), false),
            Some("timeout")
        );
        assert_eq!(
            super::login_reason(&Err(crate::process::ProcessError::TimedOut), true),
            None
        );
        assert_eq!(
            super::login_reason(&Err(crate::process::ProcessError::Cancelled), false),
            None
        );
    }
    #[test]
    fn first_run_and_retry_distinguish_offline_rejected_and_unsupported() {
        use super::*;
        let status = |saved, verification, state| ProviderStatus {
            provider: "gemini",
            installed: true,
            runnable: true,
            state,
            account_label: None,
            plan_label: None,
            api_key_saved: saved,
            key_verification: verification,
            checked_key_fingerprint: None,
            methods: vec!["apiKey"],
            active: None,
        };
        for value in ["verified", "unavailable", "notSupported"] {
            assert!(first_run_allowed(&[status(true, value, "apiKey")]));
            let mut missing_tool = status(true, value, "apiKey");
            missing_tool.installed = false;
            missing_tool.runnable = false;
            assert!(!first_run_allowed(&[missing_tool]));
        }
        for value in ["rejected", "notConfigured", "verifying"] {
            assert!(!first_run_allowed(&[status(true, value, "apiKey")]));
        }
        assert!(!first_run_allowed(&[status(
            false,
            "unavailable",
            "signedOut"
        )]));
        assert!(first_run_allowed(&[status(
            false,
            "notConfigured",
            "account"
        )]));
        assert!(should_retry("unavailable", true));
        assert!(!should_retry("unavailable", false));
        assert!(!should_retry("verifying", true));
        assert!(key_can_be_used(true, "notSupported"));
        assert!(!key_can_be_used(true, "unavailable"));
        assert!(!key_can_be_used(false, "notSupported"));
        assert!(key_matches_checked(
            Some(key_fingerprint("first-key")),
            Some("first-key")
        ));
        assert!(!key_matches_checked(
            Some(key_fingerprint("first-key")),
            Some("replacement-key")
        ));
        assert_eq!(
            next_retry_delay(Duration::from_secs(30)),
            Duration::from_secs(60)
        );
        assert_eq!(
            next_retry_delay(Duration::from_secs(240)),
            Duration::from_secs(300)
        );
        assert_eq!(
            next_retry_delay(Duration::from_secs(300)),
            Duration::from_secs(300)
        );
        let fingerprint = key_fingerprint("fake-key");
        let mut key = KeyCheck {
            fingerprint,
            generation: uuid::Uuid::new_v4(),
            state: "notSupported",
        };
        assert!(!reject_if_unsupported(
            &mut key,
            key_fingerprint("different-key")
        ));
        assert!(reject_if_unsupported(&mut key, fingerprint));
        assert_eq!(key.state, "rejected");
    }

    #[test]
    fn offline_key_recheck_updates_state_after_connection_returns() {
        use super::*;
        let provider = "test-offline-retry";
        let generation = begin_key_check(provider, "test-key").unwrap();
        assert_eq!(
            finish_key_check(provider, generation, "unavailable"),
            Some("unavailable")
        );
        assert!(should_retry("unavailable", true));
        let retry = begin_key_check(provider, "test-key").unwrap();
        assert_eq!(
            finish_key_check(provider, retry, "authenticated"),
            Some("verified")
        );
        let retry = begin_key_check(provider, "test-key").unwrap();
        assert_eq!(
            finish_key_check(provider, retry, "rejected"),
            Some("rejected")
        );
        assert!(!key_can_be_used(true, "rejected"));
    }

    #[test]
    fn stale_retry_cannot_replace_the_new_key_check() {
        use super::*;
        let provider = "test-stale-retry";
        let current = begin_key_check(provider, "new-key").unwrap();
        assert_eq!(begin_key_check_if(provider, "old-key", || false), None);
        assert_eq!(
            finish_key_check(provider, current, "authenticated"),
            Some("verified")
        );
    }
    #[cfg(windows)]
    #[test]
    fn fake_login_child() {
        if std::env::var_os("MOMSO_FAKE_LOGIN_TEST").is_none() {
            return;
        }
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            for (tool, args) in [
                ("codex", &["login"][..]),
                ("claude", &["auth", "login", "--claudeai"][..]),
            ] {
                let result =
                    super::run_login_tool(tool, args, tokio_util::sync::CancellationToken::new())
                        .await
                        .unwrap();
                assert!(result.success);
                assert_eq!(result.stdout.trim(), "fake_browser_requested");
            }
        });
    }

    #[cfg(windows)]
    #[test]
    fn fake_device_login_child() {
        use std::io::Write;
        let Some(provider) = std::env::var_os("MOMSO_TEST_DEVICE_PROVIDER") else {
            return;
        };
        let marker = std::env::var_os("MOMSO_TEST_DEVICE_MARKER").unwrap();
        if provider == "github" {
            eprintln!("\n! First copy your one-time code: ABCD-1234");
            eprintln!(
                "Open this URL to continue in your web browser: https://github.com/login/device"
            );
            std::io::stderr().flush().unwrap();
        } else {
            println!("\n1. Open this link in your browser and sign in to your account");
            println!("   \u{1b}[34mhttps://auth.openai.com/codex/device\u{1b}[0m");
            println!("2. Enter this one-time code (expires in 15 minutes)");
            println!("   \u{1b}[34mABCD-1234\u{1b}[0m");
            std::io::stdout().flush().unwrap();
        }
        for _ in 0..200 {
            if std::path::Path::new(&marker).is_file() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("device code was not delivered");
    }

    #[cfg(windows)]
    #[test]
    fn fake_gemini_account_login_child() {
        use std::io::{BufRead, Write};
        if std::env::var_os("MOMSO_TEST_GEMINI_LOGIN").is_none() {
            return;
        }
        let input = std::io::stdin();
        let mut input = input.lock();
        let mut line = String::new();
        input.read_line(&mut line).unwrap();
        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["method"], "initialize");
        println!("\n{{\"id\":1,\"result\":{{}}}}");
        std::io::stdout().flush().unwrap();
        line.clear();
        input.read_line(&mut line).unwrap();
        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["method"], "initialized");
        line.clear();
        input.read_line(&mut line).unwrap();
        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["method"], "authenticate");
        assert_eq!(request["params"]["methodId"], "oauth-personal");
        println!("{{\"id\":2,\"result\":{{}}}}");
        std::io::stdout().flush().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn fake_gemini_account_login_runs_after_warning() {
        if std::env::var_os("MOMSO_TEST_GEMINI_PARENT").is_some() {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async {
                let notice_seen = std::sync::atomic::AtomicBool::new(false);
                let result = after_browser_notice(
                    async {
                        notice_seen.store(true, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    },
                    || async {
                        assert!(notice_seen.load(std::sync::atomic::Ordering::SeqCst));
                        gemini_account_login_with(
                            &std::env::current_exe().unwrap(),
                            &[
                                "--exact",
                                "provider_auth::tests::fake_gemini_account_login_child",
                                "--nocapture",
                                "--test-threads=1",
                            ],
                            CancellationToken::new(),
                        )
                        .await
                    },
                )
                .await;
                assert_eq!(result, Ok(Ok(())));
            });
            return;
        }
        let output = crate::process::hidden_command(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "provider_auth::tests::fake_gemini_account_login_runs_after_warning",
                "--test-threads=1",
            ])
            .env("MOMSO_TEST_GEMINI_PARENT", "1")
            .env("MOMSO_TEST_GEMINI_LOGIN", "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "fake ACP login failed");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn fake_device_cli_routes_github_and_codex_codes_without_default_browser() {
        use std::process::Stdio;
        for provider in ["github", "codex"] {
            let temp = tempfile::tempdir().unwrap();
            let marker = temp.path().join("opened");
            let mut command =
                crate::process::hidden_async_command(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "provider_auth::tests::fake_device_login_child",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("MOMSO_TEST_DEVICE_PROVIDER", provider)
                .env("MOMSO_TEST_DEVICE_MARKER", &marker)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            let opened = marker.clone();
            run_device_process(
                command,
                provider,
                CancellationToken::new(),
                (provider == "github").then_some(temp.path()),
                move |code| async move {
                    assert_eq!(code, "ABCD-1234");
                    std::fs::write(opened, b"opened").unwrap();
                    Ok(())
                },
            )
            .await
            .unwrap();
            assert!(marker.is_file());
            if provider == "github" {
                assert_eq!(crate::gh_owned::recover(temp.path()), Ok(false));
            }
        }
    }

    #[test]
    fn device_prompt_rejects_untrusted_urls_and_codes() {
        let mut codex = DevicePrompt::new("codex");
        assert_eq!(
            codex.accept("https://auth.openai.com.evil.test/codex/device"),
            None
        );
        assert_eq!(codex.accept("2. Enter this one-time code"), None);
        assert_eq!(codex.accept("ABCD-1234"), None);
        assert_eq!(
            codex.accept("https://auth.openai.com/codex/device"),
            Some("ABCD-1234".into())
        );
        let mut github = DevicePrompt::new("github");
        assert_eq!(github.accept("! First copy your one-time code: A B"), None);
        assert_eq!(
            github.accept(
                "Open this URL to continue in your web browser: https://github.com/login/device"
            ),
            None
        );
    }

    #[tokio::test]
    async fn default_browser_notice_precedes_fake_login() {
        let steps = std::sync::Mutex::new(Vec::new());
        let result = after_browser_notice(
            async {
                steps.lock().unwrap().push("notice");
                Ok(())
            },
            || async {
                steps.lock().unwrap().push("fake_cli");
                "ok"
            },
        )
        .await;
        assert_eq!(result, Ok("ok"));
        assert_eq!(*steps.lock().unwrap(), ["notice", "fake_cli"]);
        let stopped =
            after_browser_notice(async { Err("cancelled") }, || async { "launched" }).await;
        assert_eq!(stopped, Err("cancelled"));
    }

    #[tokio::test]
    async fn browser_notice_ack_timeout_and_cancel_release_waiters() {
        let id = "claude".to_owned();
        let acknowledged = wait_for_browser_notice(
            &id,
            CancellationToken::new(),
            Duration::from_secs(1),
            || assert_eq!(provider_login_browser_ack(id.clone()), "acknowledged"),
        )
        .await;
        assert_eq!(acknowledged, Ok(()));
        assert_eq!(provider_login_browser_ack(id.clone()), "notWaiting");

        let start = std::time::Instant::now();
        let timed_out = wait_for_browser_notice(
            &id,
            CancellationToken::new(),
            Duration::from_millis(25),
            || {},
        )
        .await;
        assert_eq!(timed_out, Ok(()));
        assert!(start.elapsed() >= Duration::from_millis(25));
        assert_eq!(acknowledge_browser_notice(&id), "notWaiting");

        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            wait_for_browser_notice(&id, cancel, Duration::from_secs(1), || {}).await,
            Err("cancelled"),
        );
        assert_eq!(acknowledge_browser_notice(&id), "notWaiting");
        assert_eq!(provider_login_browser_ack("unknown".into()), "unavailable");
        let simultaneous = CancellationToken::new();
        let request_cancel = simultaneous.clone();
        assert_eq!(
            wait_for_browser_notice(&id, simultaneous, Duration::from_secs(1), || {
                request_cancel.cancel();
                assert_eq!(provider_login_browser_ack(id.clone()), "acknowledged");
            })
            .await,
            Err("cancelled"),
        );
    }

    #[cfg(windows)]
    #[test]
    fn fake_login_requests_browser_without_real_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let tools = temp.path().join("tools");
        std::fs::create_dir(&tools).unwrap();
        for name in ["codex", "claude"] {
            std::fs::write(
                tools.join(format!("{name}.cmd")),
                b"@echo off\r\necho fake_browser_requested\r\nexit /b 0\r\n",
            )
            .unwrap();
        }
        let windows = std::env::var_os("SystemRoot").unwrap();
        let path =
            std::env::join_paths([tools, std::path::PathBuf::from(&windows).join("System32")])
                .unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "provider_auth::tests::fake_login_child"])
            .current_dir(temp.path())
            .env_clear()
            .env("SystemRoot", &windows)
            .env(
                "COMSPEC",
                std::path::PathBuf::from(&windows).join("System32/cmd.exe"),
            )
            .env("PATH", path)
            .env("PATHEXT", ".COM;.EXE;.BAT;.CMD")
            .env("USERPROFILE", temp.path())
            .env("HOME", temp.path())
            .env("APPDATA", temp.path())
            .env("LOCALAPPDATA", temp.path())
            .env("TEMP", temp.path())
            .env("TMP", temp.path())
            .env("MOMSO_FAKE_LOGIN_TEST", "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "fake login failed: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn gemini_key_shape_rejects_invalid_without_contacting_provider() {
        assert!(super::valid_api_key_shape("fake-short"));
        assert!(super::valid_api_key_shape("fake-key-with a-space-123456"));
        assert!(super::valid_api_key_shape(
            "fake-key-invalid-but-long-enough"
        ));
    }
    use super::*;
    #[test]
    fn rejected_key_is_saved_but_never_connected() {
        let mut saved = None;
        let generation = store_and_begin("test-rejected", "wrong-key", |key| {
            saved = Some(key.to_owned());
            Ok(())
        })
        .unwrap();
        assert_eq!(saved.as_deref(), Some("wrong-key"));
        assert_eq!(
            finish_key_check("test-rejected", generation, "rejected"),
            Some("rejected")
        );
        for provider in ["codex", "claude", "gemini"] {
            let active = if provider == "gemini" {
                None
            } else {
                Some("apiKey")
            };
            assert_eq!(
                chat_auth_method(provider, true, "apiKey", active, false, None),
                Err("not_connected")
            );
            assert_eq!(
                chat_auth_method(provider, true, "apiKey", active, true, None),
                Ok("apiKey")
            );
        }
        let newer = begin_key_check("test-rejected", "new-key").unwrap();
        assert_eq!(
            finish_key_check("test-rejected", generation, "authenticated"),
            None
        );
        assert_eq!(
            finish_key_check("test-rejected", newer, "unavailable"),
            Some("unavailable")
        );
    }
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
            Err("not_connected")
        );
        assert_eq!(
            chat_auth_method("gemini", true, "apiKey", None, true, None),
            Ok("apiKey")
        );
    }
    #[tokio::test]
    async fn codex_account_read_retries_transient_errors_but_not_signed_out() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = AtomicUsize::new(0);
        let result = retry_codex_account_read(
            || async {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(process::ProcessError::TimedOut)
                } else {
                    Ok(serde_json::json!({"account":{"type":"chatgpt"}}))
                }
            },
            true,
        )
        .await
        .unwrap();
        assert_eq!(result["account"]["type"], "chatgpt");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let signed_out = AtomicUsize::new(0);
        let result = retry_codex_account_read(
            || async {
                signed_out.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({"account":null}))
            },
            true,
        )
        .await
        .unwrap();
        assert!(result["account"].is_null());
        assert_eq!(signed_out.load(Ordering::SeqCst), 1);
        let consecutive = AtomicUsize::new(0);
        for _ in 0..9 {
            let result = retry_codex_account_read(
                || async {
                    let attempt = consecutive.fetch_add(1, Ordering::SeqCst);
                    if matches!(attempt, 6 | 8) {
                        Err(process::ProcessError::TimedOut)
                    } else {
                        Ok(serde_json::json!({"account":{"type":"chatgpt"}}))
                    }
                },
                true,
            )
            .await
            .unwrap();
            assert_eq!(result["account"]["type"], "chatgpt");
        }
        assert_eq!(consecutive.load(Ordering::SeqCst), 11);
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

    #[test]
    fn github_storage_actions_distinguish_removed_failed_and_unknown() {
        assert_eq!(
            github_storage_event_action("github_insecure_storage"),
            Some("removed")
        );
        assert_eq!(
            github_storage_event_action("github_cleanup_failed"),
            Some("cleanup_failed")
        );
        assert_eq!(
            github_storage_event_action("github_storage_unknown"),
            Some("verification_required")
        );
        assert_eq!(github_storage_event_action("timeout"), None);
    }

    #[test]
    fn github_status_does_not_treat_failed_checks_as_authenticated() {
        let status = |source: &str, state: &str| serde_json::json!({"hosts":{"github.com":[{"active":true,"tokenSource":source,"state":state}]}});
        assert_eq!(parse_gh_status(&status("keyring", "success")), Ok(true));
        assert_eq!(parse_gh_status(&status("keyring", "timeout")), Err(()));
        assert_eq!(parse_gh_status(&status("hosts.yml", "success")), Ok(false));
        assert_eq!(parse_gh_status(&serde_json::json!({"hosts":{}})), Ok(false));
        assert_eq!(parse_gh_status(&serde_json::json!({})), Err(()));
        assert_eq!(parse_gh_status(&status("unrecognized", "success")), Err(()));
        assert_eq!(
            parse_gh_status(
                &serde_json::json!({"hosts":{"github.com":[{"active":true,"tokenSource":"keyring","state":"error","error":"HTTP 401: Bad credentials"}]}})
            ),
            Ok(false)
        );
        assert_eq!(select_gh_source(Err(()), Ok(true)), Ok(Some(2)));
        assert_eq!(select_gh_source(Ok(true), Ok(true)), Ok(Some(1)));
        assert_eq!(
            select_gh_source(Err(()), Ok(false)),
            Err("github_storage_unknown")
        );
        assert_eq!(select_gh_source(Ok(false), Ok(false)), Ok(None));
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
        let database = app.join("test.sqlite");
        crate::storage::initialize(&database, &app.join("backups"), "test").unwrap();
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
        let room = serde_json::to_value(crate::chat::create(&database).unwrap()).unwrap();
        let id = room["id"].as_str().unwrap();
        crate::chat::append(
            &database,
            id,
            crate::chat::Role::User,
            "synthetic message",
            None,
            None,
        )
        .unwrap();
        crate::chat_archive_store::write_known(&archive, &database, id, &[&secret]).unwrap();
        let safe_error = crate::chat_archive_redact::redact_with_known(
            &format!("fixture error {secret}"),
            &[&secret],
        )
        .text;
        assert!(!safe_error.contains(&secret));
        let output = std::process::Command::new("cmd.exe")
            .args(["/C", "exit", "0"])
            .env("MOMSO_TEST_FIXTURE_KEY", &secret)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(!output
            .stdout
            .windows(secret.len())
            .any(|v| v == secret.as_bytes()));
        assert!(!output
            .stderr
            .windows(secret.len())
            .any(|v| v == secret.as_bytes()));
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
