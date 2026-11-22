use base64::{engine::general_purpose::STANDARD, Engine as _};
use momso_saved_login_core::linux::LinuxVault;
use reqwest::blocking::{Client, RequestBuilder};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read, Seek, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

const GITHUB_WEB: &str = "https://github.com";
const GITHUB_API: &str = "https://api.github.com";
const VERIFY_URL: &str = "https://github.com/login/device";
const VAULT_KEY: &str = "github-access-token";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStart {
    pub session_id: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub provider: &'static str,
    pub state: &'static str,
    pub url: Option<String>,
    pub code: Option<String>,
    pub expires_in_seconds: Option<u64>,
}

#[derive(Deserialize)]
struct DeviceReply {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}
#[derive(Deserialize)]
struct TokenReply {
    access_token: Option<String>,
    error: Option<String>,
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct AccountReply {
    login: String,
}

#[derive(Deserialize)]
struct RepoReply {
    full_name: String,
    private: bool,
    default_branch: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Repository {
    pub name_with_owner: String,
    pub default_branch: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncOutcome {
    pub excluded_non_text: usize,
    pub uploaded_key_found: bool,
}

fn valid_part(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value != "."
        && value != ".."
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

fn valid_repo(value: &str) -> bool {
    let Some((owner, name)) = value.split_once('/') else {
        return false;
    };
    valid_part(owner) && valid_part(name) && !name.to_ascii_lowercase().ends_with(".git")
}

struct Session {
    id: String,
    device_code: String,
    user_code: String,
    expires: Instant,
    next_poll: Instant,
    interval: Duration,
    state: &'static str,
}

pub struct Github {
    http: Client,
    web_base: String,
    api_base: String,
    client_id: String,
    vault: Arc<LinuxVault>,
    session: Mutex<Option<Session>>,
    #[cfg(any(test, all(debug_assertions, feature = "test-fixtures")))]
    test_remote: Mutex<Option<String>>,
}

impl Github {
    pub fn new(client_id: String, vault: Arc<LinuxVault>) -> Result<Self, &'static str> {
        Self::with_endpoints(client_id, vault, GITHUB_WEB, GITHUB_API)
    }

    /// Fake HTTP service for isolated tests; unavailable in production builds.
    #[cfg(all(debug_assertions, feature = "test-fixtures"))]
    pub fn fixture(
        client_id: String,
        vault: Arc<LinuxVault>,
        base: &str,
    ) -> Result<Self, &'static str> {
        let url = reqwest::Url::parse(base).map_err(|_| "invalid_fixture")?;
        if url.scheme() != "http"
            || url.host_str() != Some("127.0.0.1")
            || url.port().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("invalid_fixture");
        }
        let base = base.trim_end_matches('/');
        Self::with_endpoints(client_id, vault, base, base)
    }

    #[cfg(all(debug_assertions, feature = "test-fixtures"))]
    pub fn set_fixture_git_remote(&self, path: &Path) -> Result<(), &'static str> {
        let path = path.canonicalize().map_err(|_| "invalid_fixture")?;
        if !path.is_dir() || !path.join("HEAD").is_file() || !path.join("objects").is_dir() {
            return Err("invalid_fixture");
        }
        *self.test_remote.lock().map_err(|_| "unavailable")? =
            Some(path.to_str().ok_or("invalid_fixture")?.to_owned());
        Ok(())
    }

    fn with_endpoints(
        client_id: String,
        vault: Arc<LinuxVault>,
        web: &str,
        api: &str,
    ) -> Result<Self, &'static str> {
        if client_id.trim().is_empty() {
            return Err("client_id_missing");
        }
        let http = Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("momso-server")
            .build()
            .map_err(|_| "unavailable")?;
        Ok(Self {
            http,
            web_base: web.into(),
            api_base: api.into(),
            client_id,
            vault,
            session: Mutex::new(None),
            #[cfg(any(test, all(debug_assertions, feature = "test-fixtures")))]
            test_remote: Mutex::new(None),
        })
    }

    pub fn start_device_login(&self) -> Result<DeviceStart, &'static str> {
        let mut slot = self.session.lock().map_err(|_| "unavailable")?;
        *slot = None;
        let response: DeviceReply = self
            .http
            .post(format!("{}/login/device/code", self.web_base))
            .header("Accept", "application/json")
            .form(&[("client_id", self.client_id.as_str()), ("scope", "repo")])
            .send()
            .and_then(|response| response.error_for_status())
            .map_err(|_| "provider_unavailable")?
            .json()
            .map_err(|_| "provider_invalid_response")?;
        if response.verification_uri != VERIFY_URL
            || response.device_code.is_empty()
            || response.user_code.is_empty()
            || response.expires_in == 0
        {
            return Err("provider_invalid_response");
        }
        let now = Instant::now();
        let interval = Duration::from_secs(response.interval);
        let expires = now
            .checked_add(Duration::from_secs(response.expires_in))
            .ok_or("provider_invalid_response")?;
        let next_poll = now
            .checked_add(interval)
            .ok_or("provider_invalid_response")?;
        let id = uuid::Uuid::new_v4().to_string();
        *slot = Some(Session {
            id: id.clone(),
            device_code: response.device_code,
            user_code: response.user_code,
            expires,
            next_poll,
            interval,
            state: "pending",
        });
        Ok(DeviceStart { session_id: id })
    }

    pub fn login_status(&self, id: &str) -> Result<DeviceView, &'static str> {
        let mut slot = self.session.lock().map_err(|_| "unavailable")?;
        let session = slot
            .as_mut()
            .filter(|session| session.id == id)
            .ok_or("session_not_found")?;
        let now = Instant::now();
        if session.state == "pending" && now >= session.expires {
            session.state = "expired";
            session.device_code.clear();
        }
        Ok(DeviceView {
            provider: "github",
            state: session.state,
            url: (session.state == "pending").then(|| VERIFY_URL.to_owned()),
            code: (session.state == "pending").then(|| session.user_code.clone()),
            expires_in_seconds: (session.state == "pending")
                .then(|| session.expires.saturating_duration_since(now).as_secs()),
        })
    }

    pub fn cancel_login(&self, id: &str) -> Result<(), &'static str> {
        let mut slot = self.session.lock().map_err(|_| "unavailable")?;
        let session = slot
            .as_mut()
            .filter(|session| session.id == id)
            .ok_or("session_not_found")?;
        session.state = "cancelled";
        session.device_code.clear();
        session.user_code.clear();
        Ok(())
    }

    pub fn poll_login(&self, id: &str) -> Result<DeviceView, &'static str> {
        {
            let mut slot = self.session.lock().map_err(|_| "unavailable")?;
            let session = slot
                .as_mut()
                .filter(|session| session.id == id)
                .ok_or("session_not_found")?;
            let now = Instant::now();
            if session.state == "pending" && now >= session.expires {
                session.state = "expired";
                session.device_code.clear();
            }
            if session.state == "pending" && now >= session.next_poll {
                session.next_poll = now
                    .checked_add(session.interval)
                    .ok_or("provider_invalid_response")?;
                let response: TokenReply = self
                    .http
                    .post(format!("{}/login/oauth/access_token", self.web_base))
                    .header("Accept", "application/json")
                    .form(&[
                        ("client_id", self.client_id.as_str()),
                        ("device_code", session.device_code.as_str()),
                        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ])
                    .send()
                    .and_then(|response| response.error_for_status())
                    .map_err(|_| "provider_unavailable")?
                    .json()
                    .map_err(|_| "provider_invalid_response")?;
                if let Some(token) = response.access_token {
                    if token.is_empty() {
                        return Err("provider_invalid_response");
                    }
                    self.vault
                        .store_service_secret(VAULT_KEY, &token)
                        .map_err(|_| "credential_store_unavailable")?;
                    session.state = "connected";
                    session.device_code.clear();
                    session.user_code.clear();
                } else {
                    match response.error.as_deref() {
                        Some("authorization_pending") => (),
                        Some("slow_down") => {
                            session.interval = Duration::from_secs(
                                response
                                    .interval
                                    .unwrap_or(session.interval.as_secs().saturating_add(5)),
                            );
                            session.next_poll = Instant::now()
                                .checked_add(session.interval)
                                .ok_or("provider_invalid_response")?;
                        }
                        Some("expired_token") => {
                            session.state = "expired";
                            session.device_code.clear();
                        }
                        Some("access_denied") => {
                            session.state = "failed";
                            session.device_code.clear();
                        }
                        _ => return Err("provider_invalid_response"),
                    }
                }
            }
        }
        self.login_status(id)
    }

    fn token(&self) -> Result<String, &'static str> {
        self.vault
            .read_service_secret(VAULT_KEY)
            .map_err(|_| "credential_store_unavailable")?
            .ok_or("not_connected")
    }

    fn authorized(&self, request: RequestBuilder) -> Result<RequestBuilder, &'static str> {
        Ok(request
            .bearer_auth(self.token()?)
            .header("Accept", "application/vnd.github+json"))
    }

    pub fn account_name(&self) -> Result<String, &'static str> {
        let response = self
            .authorized(self.http.get(format!("{}/user", self.api_base)))?
            .send()
            .and_then(|response| response.error_for_status())
            .map_err(|_| "provider_unavailable")?;
        let account: AccountReply = response.json().map_err(|_| "provider_invalid_response")?;
        if !valid_part(&account.login) {
            return Err("provider_invalid_response");
        }
        Ok(account.login)
    }

    /// Existing repositories are inspected, never overwritten or implicitly initialized.
    pub fn inspect_repository(&self, name: &str) -> Result<Option<Repository>, &'static str> {
        if !valid_repo(name) {
            return Err("invalid_repository");
        }
        let response = self
            .authorized(self.http.get(format!("{}/repos/{name}", self.api_base)))?
            .send()
            .map_err(|_| "provider_unavailable")?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let repo: RepoReply = response
            .error_for_status()
            .map_err(|_| "provider_unavailable")?
            .json()
            .map_err(|_| "provider_invalid_response")?;
        if !repo.full_name.eq_ignore_ascii_case(name) {
            return Err("provider_invalid_response");
        }
        if !repo.private {
            return Err("repository_not_private");
        }
        Ok(Some(Repository {
            name_with_owner: repo.full_name,
            default_branch: repo.default_branch,
        }))
    }

    /// List private repositories for an explicit user selection.
    pub fn list_private_repositories(&self) -> Result<Vec<Repository>, &'static str> {
        let mut result = Vec::new();
        let mut page = 1u64;
        loop {
            let response = self
                .authorized(self.http.get(format!(
                    "{}/user/repos?per_page=100&page={page}",
                    self.api_base
                )))?
                .send()
                .and_then(|r| r.error_for_status())
                .map_err(|_| "provider_unavailable")?;
            let entries: Vec<RepoReply> =
                response.json().map_err(|_| "provider_invalid_response")?;
            let count = entries.len();
            for repo in entries {
                if repo.private && valid_repo(&repo.full_name) {
                    result.push(Repository {
                        name_with_owner: repo.full_name,
                        default_branch: repo.default_branch,
                    });
                }
            }
            if count < 100 {
                return Ok(result);
            }
            page = page.checked_add(1).ok_or("provider_invalid_response")?;
        }
    }

    pub fn create_or_find_repository(&self, username: &str) -> Result<Repository, &'static str> {
        if username.is_empty()
            || !username
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
            || username.len() > 94
            || username.to_ascii_lowercase().ends_with(".git")
        {
            return Err("invalid_username");
        }
        let account = self.account_name()?;
        let full = format!("{account}/momso-{username}");
        if let Some(existing) = self.inspect_repository(&full)? {
            return Ok(existing);
        }
        let name = format!("momso-{username}");
        let response = self
            .authorized(self.http.post(format!("{}/user/repos", self.api_base)))?
            .json(&serde_json::json!({"name": name, "private": true, "auto_init": false}))
            .send()
            .map_err(|_| "provider_unavailable")?;
        if !response.status().is_success() {
            return self
                .inspect_repository(&full)?
                .ok_or("repository_create_failed");
        }
        self.inspect_repository(&full)?
            .ok_or("repository_create_unverified")
    }

    /// Commit local changes through momso-core first, then upload a stable Git head.
    pub fn push(&self, root: &Path, repository: &Repository) -> Result<bool, &'static str> {
        self.push_with_known(root, repository, &[])
    }

    fn push_with_known(
        &self,
        root: &Path,
        repository: &Repository,
        extra: &[&str],
    ) -> Result<bool, &'static str> {
        if !valid_repo(&repository.name_with_owner) {
            return Err("invalid_repository");
        }
        let checked = self
            .inspect_repository(&repository.name_with_owner)?
            .ok_or("repository_not_found")?;
        let branch = checked.default_branch.as_deref().unwrap_or("main");
        if !valid_branch(branch) {
            return Err("invalid_branch");
        }
        let url = format!("https://github.com/{}.git", checked.name_with_owner);
        #[cfg(any(test, all(debug_assertions, feature = "test-fixtures")))]
        let url = self
            .test_remote
            .lock()
            .map_err(|_| "unavailable")?
            .clone()
            .unwrap_or(url);
        self.push_to_with_known(root, &url, branch, extra)
    }

    /// Stage and commit changed managed files before uploading the current head.
    pub fn sync_once(
        &self,
        root: &Path,
        repository: &Repository,
    ) -> Result<SyncOutcome, &'static str> {
        self.sync_once_with_known(root, repository, &[])
    }

    /// Also scan host-supplied provider credentials; values never leave this process.
    pub fn sync_once_with_known(
        &self,
        root: &Path,
        repository: &Repository,
        extra: &[&str],
    ) -> Result<SyncOutcome, &'static str> {
        let excluded_non_text = self.save_changes_with_known(root, extra)?;
        let uploaded_key_found = self.push_with_known(root, repository, extra)?;
        Ok(SyncOutcome {
            excluded_non_text,
            uploaded_key_found,
        })
    }

    /// Join a specifically restored remote commit only after every remote path
    /// exists in the saved local tree. No working file is changed by the merge.
    pub fn verify_restored_remote(
        &self,
        root: &Path,
        repository: &Repository,
        expected: &str,
    ) -> Result<(), &'static str> {
        if expected.len() != 40
            || !expected.bytes().all(|b| b.is_ascii_hexdigit())
            || !valid_repo(&repository.name_with_owner)
        {
            return Err("restore_changed");
        }
        let checked = self
            .inspect_repository(&repository.name_with_owner)?
            .ok_or("repository_not_found")?;
        let branch = checked.default_branch.as_deref().unwrap_or("main");
        if !valid_branch(branch) {
            return Err("invalid_branch");
        }
        let url = format!("https://github.com/{}.git", checked.name_with_owner);
        #[cfg(any(test, all(debug_assertions, feature = "test-fixtures")))]
        let url = self
            .test_remote
            .lock()
            .map_err(|_| "unavailable")?
            .clone()
            .unwrap_or(url);
        let _guard = momso_core::lock::exclusive(root)?;
        self.vault.with_backup_secrets(VAULT_KEY, |token, _| {
            let reference = format!("refs/heads/{branch}");
            let fetch = git(
                root,
                &url,
                token,
                &["fetch", "--no-tags", "--", &url, &reference],
            )?;
            if !fetch.status.success() {
                return Err("remote_unavailable");
            }
            let remote = git(root, &url, token, &["rev-parse", "FETCH_HEAD"])?;
            let remote = std::str::from_utf8(&remote.stdout)
                .map_err(|_| "restore_changed")?
                .trim();
            if remote == expected {
                return Ok(());
            }
            // A previous push may have succeeded just before the process stopped.
            let local = git(root, &url, token, &["rev-parse", "HEAD"])?;
            if local.status.success()
                && std::str::from_utf8(&local.stdout)
                    .map_err(|_| "restore_changed")?
                    .trim()
                    == remote
            {
                let ancestor = git(
                    root,
                    &url,
                    token,
                    &["merge-base", "--is-ancestor", expected, "FETCH_HEAD"],
                )?;
                if ancestor.status.success() {
                    return Ok(());
                }
            }
            Err("restore_changed")
        })
    }

    pub fn merge_restored_history(
        &self,
        root: &Path,
        repository: &Repository,
        expected: &str,
        extra: &[&str],
    ) -> Result<(), &'static str> {
        if expected.len() != 40
            || !expected.bytes().all(|b| b.is_ascii_hexdigit())
            || !valid_repo(&repository.name_with_owner)
        {
            return Err("restore_changed");
        }
        let checked = self
            .inspect_repository(&repository.name_with_owner)?
            .ok_or("repository_not_found")?;
        let branch = checked.default_branch.as_deref().unwrap_or("main");
        if !valid_branch(branch) {
            return Err("invalid_branch");
        }
        let url = format!("https://github.com/{}.git", checked.name_with_owner);
        #[cfg(any(test, all(debug_assertions, feature = "test-fixtures")))]
        let url = self
            .test_remote
            .lock()
            .map_err(|_| "unavailable")?
            .clone()
            .unwrap_or(url);
        let _guard = momso_core::lock::exclusive(root)?;
        self.vault.with_backup_secrets(VAULT_KEY, |token, logins| {
            let known: Vec<&str> = std::iter::once(token)
                .chain(logins.iter().map(String::as_str))
                .chain(extra.iter().copied())
                .filter(|value| !value.is_empty())
                .collect();
            let reference = format!("refs/heads/{branch}");
            let fetched = git(
                root,
                &url,
                token,
                &["fetch", "--no-tags", "--", &url, &reference],
            )?;
            if !fetched.status.success() {
                return Err("remote_unavailable");
            }
            let head = git(root, &url, token, &["rev-parse", "FETCH_HEAD"])?;
            if !head.status.success()
                || std::str::from_utf8(&head.stdout)
                    .map_err(|_| "restore_changed")?
                    .trim()
                    != expected
            {
                return Err("restore_changed");
            }
            let remote = git(
                root,
                &url,
                token,
                &["ls-tree", "-r", "--name-only", "-z", "FETCH_HEAD"],
            )?;
            let local = git(
                root,
                &url,
                token,
                &["ls-tree", "-r", "--name-only", "-z", "HEAD"],
            )?;
            if !remote.status.success() || !local.status.success() {
                return Err("restore_changed");
            }
            let local_paths: std::collections::HashSet<&[u8]> = local
                .stdout
                .split(|c| *c == 0)
                .filter(|p| !p.is_empty())
                .collect();
            if remote
                .stdout
                .split(|c| *c == 0)
                .filter(|p| !p.is_empty())
                .any(|p| !local_paths.contains(p))
            {
                return Err("restore_changed");
            }
            ensure_secrets_absent_from_history(root, &known, &["HEAD", "^FETCH_HEAD"])?;
            let merged = git(
                root,
                &url,
                token,
                &[
                    "-c",
                    "user.name=momso",
                    "-c",
                    "user.email=momso@localhost",
                    "merge",
                    "--no-ff",
                    "--allow-unrelated-histories",
                    "-s",
                    "ours",
                    "-m",
                    "momso: connect restored history",
                    "FETCH_HEAD",
                ],
            )?;
            if !merged.status.success() {
                return Err("restore_conflict");
            }
            Ok(())
        })
    }

    #[cfg(test)]
    fn save_changes(&self, root: &Path) -> Result<usize, &'static str> {
        self.save_changes_with_known(root, &[])
    }

    fn save_changes_with_known(&self, root: &Path, extra: &[&str]) -> Result<usize, &'static str> {
        use std::os::unix::fs::OpenOptionsExt;
        if self
            .vault
            .is_within(root)
            .map_err(|_| "credential_store_unavailable")?
        {
            return Err("vault_inside_archive");
        }
        let guard = momso_core::lock::exclusive(root)?;
        let paths = momso_core::git::changed_paths_guarded(root, &guard)?;
        self.vault.with_backup_secrets(VAULT_KEY, |token, logins| {
            let known: Vec<&str> = std::iter::once(token)
                .chain(logins.iter().map(String::as_str))
                .chain(extra.iter().copied())
                .filter(|secret| !secret.is_empty())
                .collect();
            let longest = known.iter().map(|secret| secret.len()).max().unwrap_or(1);
            let mut selected = Vec::new();
            let mut excluded = 0;
            for relative in &paths {
                let path = root.join(relative);
                match std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(path)
                {
                    Ok(mut file) => {
                        let mut buffer = [0u8; 8192];
                        let mut tail = Vec::new();
                        let mut utf8_pending = Vec::new();
                        let mut text = true;
                        loop {
                            let count =
                                file.read(&mut buffer).map_err(|_| "storage_unavailable")?;
                            if count == 0 {
                                break;
                            }
                            tail.extend_from_slice(&buffer[..count]);
                            if buffer[..count].contains(&0) {
                                text = false;
                            }
                            if text {
                                utf8_pending.extend_from_slice(&buffer[..count]);
                                match std::str::from_utf8(&utf8_pending) {
                                    Ok(_) => utf8_pending.clear(),
                                    Err(error) if error.error_len().is_none() => {
                                        utf8_pending.drain(..error.valid_up_to());
                                    }
                                    Err(_) => {
                                        text = false;
                                        utf8_pending.clear();
                                    }
                                }
                            }
                            if known.iter().any(|secret| {
                                tail.windows(secret.len())
                                    .any(|part| part == secret.as_bytes())
                            }) {
                                return Err("credential_in_archive");
                            }
                            let retain = longest.saturating_sub(1).min(tail.len());
                            tail.drain(..tail.len() - retain);
                        }
                        if !utf8_pending.is_empty() {
                            text = false;
                        }
                        if !text {
                            if relative.starts_with("chats/") {
                                return Err("chat_snapshot_invalid");
                            }
                            excluded += 1;
                            continue;
                        }
                        if relative.starts_with("chats/") {
                            file.rewind().map_err(|_| "storage_unavailable")?;
                            let snapshot: serde_json::Value = serde_json::from_reader(file)
                                .map_err(|_| "chat_snapshot_invalid")?;
                            let expected = relative
                                .strip_prefix("chats/")
                                .and_then(|path| path.strip_suffix(".json"))
                                .ok_or("chat_snapshot_invalid")?;
                            if snapshot.get("version").and_then(serde_json::Value::as_u64)
                                != Some(1)
                                || snapshot
                                    .pointer("/room/id")
                                    .and_then(serde_json::Value::as_str)
                                    != Some(expected)
                            {
                                return Err("chat_snapshot_invalid");
                            }
                            if snapshot_contains_sensitive(&snapshot, &known) {
                                return Err("chat_redaction_required");
                            }
                        }
                        selected.push(relative.as_str());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        selected.push(relative.as_str())
                    }
                    Err(_) => return Err("storage_unavailable"),
                }
            }
            if !selected.is_empty() {
                momso_core::git::save_guarded(root, &selected, "몸소 자동 저장", &guard)?;
            }
            Ok(excluded)
        })
    }

    #[cfg(test)]
    fn push_to(&self, root: &Path, url: &str, branch: &str) -> Result<bool, &'static str> {
        self.push_to_with_known(root, url, branch, &[])
    }

    fn push_to_with_known(
        &self,
        root: &Path,
        url: &str,
        branch: &str,
        extra: &[&str],
    ) -> Result<bool, &'static str> {
        if self
            .vault
            .is_within(root)
            .map_err(|_| "credential_store_unavailable")?
        {
            return Err("vault_inside_archive");
        }
        let _guard = momso_core::lock::exclusive(root)?;
        self.vault.with_backup_secrets(VAULT_KEY, |token, logins| {
            let known: Vec<&str> = std::iter::once(token)
                .chain(logins.iter().map(String::as_str))
                .chain(extra.iter().copied())
                .filter(|secret| !secret.is_empty())
                .collect();
            let reference = format!("refs/heads/{branch}");
            let refs = git(root, url, token, &["ls-remote", "--heads", url, &reference])?;
            if !refs.status.success() {
                return Err("remote_unavailable");
            }
            let mut uploaded_key_found = false;
            if !refs.stdout.is_empty() {
                let fetched = git(
                    root,
                    url,
                    token,
                    &["fetch", "--no-tags", "--", url, &reference],
                )?;
                if !fetched.status.success() {
                    return Err("remote_unavailable");
                }
                let ancestry = git(
                    root,
                    url,
                    token,
                    &["merge-base", "--is-ancestor", "FETCH_HEAD", "HEAD"],
                )?;
                if !ancestry.status.success() {
                    return Err("diverged");
                }
                match ensure_secrets_absent_from_history(root, &known, &["FETCH_HEAD"]) {
                    Ok(()) => (),
                    Err("credential_in_history") => uploaded_key_found = true,
                    Err(error) => return Err(error),
                }
                ensure_secrets_absent_from_history(root, &known, &["HEAD", "^FETCH_HEAD"])?;
            } else {
                ensure_secrets_absent_from_history(root, &known, &["HEAD"])?;
            }
            let destination = format!("HEAD:{reference}");
            let pushed = git(root, url, token, &["push", "--", url, &destination])?;
            if !pushed.status.success() {
                return Err("push_failed");
            }
            Ok(uploaded_key_found)
        })
    }

    /// Clone into an absent or empty destination; never replace existing data.
    pub fn restore(&self, destination: &Path, repository: &Repository) -> Result<(), &'static str> {
        if !valid_repo(&repository.name_with_owner) {
            return Err("invalid_repository");
        }
        let checked = self
            .inspect_repository(&repository.name_with_owner)?
            .ok_or("repository_not_found")?;
        let url = format!("https://github.com/{}.git", checked.name_with_owner);
        #[cfg(any(test, all(debug_assertions, feature = "test-fixtures")))]
        let url = self
            .test_remote
            .lock()
            .map_err(|_| "unavailable")?
            .clone()
            .unwrap_or(url);
        self.restore_from(destination, &url)
    }

    fn restore_from(&self, destination: &Path, url: &str) -> Result<(), &'static str> {
        let existing_empty = match std::fs::symlink_metadata(destination) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
                if std::fs::read_dir(destination)
                    .map_err(|_| "storage_unavailable")?
                    .next()
                    .is_some()
                {
                    return Err("destination_exists");
                }
                true
            }
            Ok(_) => return Err("destination_exists"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return Err("storage_unavailable"),
        };
        let parent = destination.parent().ok_or("invalid_destination")?;
        let staging = tempfile::Builder::new()
            .prefix(".momso-receive-")
            .tempdir_in(parent)
            .map_err(|_| "storage_unavailable")?;
        let clone_path = staging.path().join("archive");
        let path = clone_path.to_str().ok_or("invalid_destination")?;
        let token = self.token()?;
        let result = git(parent, url, &token, &["clone", "--", url, path])?;
        if !result.status.success() {
            return Err("restore_failed");
        }
        // Atomic no-replace is required: another process can create the path.
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(clone_path.as_os_str().as_bytes())
            .map_err(|_| "invalid_destination")?;
        let to = std::ffi::CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| "invalid_destination")?;
        if existing_empty {
            std::fs::remove_dir(destination).map_err(|_| "destination_exists")?;
        }
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result != 0 {
            if existing_empty {
                let _ = std::fs::create_dir(destination);
            }
            return Err(
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::AlreadyExists {
                    "destination_exists"
                } else {
                    "restore_failed"
                },
            );
        }
        Ok(())
    }
}

fn valid_branch(branch: &str) -> bool {
    !branch.is_empty()
        && !branch.starts_with('-')
        && !branch.starts_with('/')
        && !branch.ends_with('/')
        && !branch.ends_with('.')
        && !branch.ends_with(".lock")
        && !branch.contains("..")
        && !branch.contains("@{")
        && branch
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_/.".contains(&byte))
}

fn git(root: &Path, url: &str, token: &str, args: &[&str]) -> Result<Output, &'static str> {
    let header = format!(
        "Authorization: Basic {}",
        STANDARD.encode(format!("x-access-token:{token}"))
    );
    let mut command = Command::new("git");
    #[cfg(test)]
    let header_key = if url.starts_with("http://127.0.0.1:") {
        format!("http.{url}/.extraheader")
    } else {
        "http.https://github.com/.extraheader".to_owned()
    };
    #[cfg(not(test))]
    let header_key = "http.https://github.com/.extraheader";
    command
        .env_clear()
        .current_dir(root)
        .args(args)
        .stdin(Stdio::null())
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_COUNT", "3")
        .env("GIT_HTTP_LOW_SPEED_LIMIT", "1")
        .env("GIT_HTTP_LOW_SPEED_TIME", "30")
        .env("GIT_CONFIG_KEY_0", header_key)
        .env("GIT_CONFIG_VALUE_0", header)
        .env("GIT_CONFIG_KEY_1", "http.followRedirects")
        .env("GIT_CONFIG_VALUE_1", "false")
        .env("GIT_CONFIG_KEY_2", "core.hooksPath")
        .env("GIT_CONFIG_VALUE_2", "/dev/null");
    // A local file remote is used only by test-only callers of push_to/restore_from.
    let _ = url;
    command.output().map_err(|_| "git_unavailable")
}

fn snapshot_contains_sensitive(value: &serde_json::Value, known: &[&str]) -> bool {
    match value {
        serde_json::Value::String(text) => {
            let redacted = crate::chat_archive_redact::redact_with_known(text, known);
            redacted.count != 0 || redacted.text != *text
        }
        serde_json::Value::Array(items) => items
            .iter()
            .any(|item| snapshot_contains_sensitive(item, known)),
        serde_json::Value::Object(items) => items.iter().any(|(key, item)| {
            let redacted = crate::chat_archive_redact::redact(key);
            redacted.count != 0
                || redacted.text != *key
                || known
                    .iter()
                    .any(|secret| !secret.is_empty() && key.contains(secret))
                || snapshot_contains_sensitive(item, known)
        }),
        _ => false,
    }
}

fn ensure_secrets_absent_from_history(
    root: &Path,
    known: &[&str],
    revisions: &[&str],
) -> Result<(), &'static str> {
    if known.is_empty() {
        return Err("credential_store_unavailable");
    }
    let mut variants: Vec<String> = known.iter().map(|secret| (*secret).to_owned()).collect();
    for secret in known {
        let encoded = serde_json::to_string(secret).map_err(|_| "unavailable")?;
        if let Some(escaped) = encoded
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
        {
            if escaped != *secret {
                variants.push(escaped.to_owned());
            }
        }
    }
    let longest = variants.iter().map(String::len).max().unwrap_or(1);
    let environment = |command: &mut Command| {
        command
            .env_clear()
            .current_dir(root)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0");
    };
    let mut list = Command::new("git");
    environment(&mut list);
    let objects = list
        .args(["rev-list", "--objects", "--no-object-names"])
        .args(revisions)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| "git_unavailable")?;
    if !objects.status.success() {
        return Err("git_failed");
    }
    let mut command = Command::new("git");
    environment(&mut command);
    let mut child = command
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "git_unavailable")?;
    let mut input = child.stdin.take().ok_or("git_failed")?;
    let writer = std::thread::spawn(move || input.write_all(&objects.stdout));
    let mut output = BufReader::new(child.stdout.take().ok_or("git_failed")?);
    let mut result = Ok(());
    loop {
        let mut header = String::new();
        match output.read_line(&mut header) {
            Ok(0) => break,
            Ok(_) => (),
            Err(_) => {
                result = Err("git_failed");
                break;
            }
        }
        let Some(size) = header
            .split_whitespace()
            .nth(2)
            .and_then(|value| value.parse::<u64>().ok())
        else {
            result = Err("git_failed");
            break;
        };
        let mut remaining = size;
        let mut carry = Vec::new();
        while remaining > 0 {
            let mut chunk = [0u8; 8192];
            let length = remaining.min(chunk.len() as u64) as usize;
            if output.read_exact(&mut chunk[..length]).is_err() {
                result = Err("git_failed");
                break;
            }
            carry.extend_from_slice(&chunk[..length]);
            if variants.iter().any(|secret| {
                !secret.is_empty()
                    && carry
                        .windows(secret.len())
                        .any(|part| part == secret.as_bytes())
            }) {
                result = Err("credential_in_history");
                break;
            }
            let retain = longest.saturating_sub(1).min(carry.len());
            carry.drain(..carry.len() - retain);
            remaining -= length as u64;
        }
        if result.is_err() {
            break;
        }
        let mut separator = [0u8; 1];
        if output.read_exact(&mut separator).is_err() || separator[0] != b'\n' {
            result = Err("git_failed");
            break;
        }
    }
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|_| "git_failed")?;
    let input_result = writer.join().map_err(|_| "git_failed")?;
    if result.is_ok() && (!status.success() || input_result.is_err()) {
        return Err("git_failed");
    }
    result
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub state: &'static str,
    pub reason: Option<&'static str>,
    pub excluded_non_text: usize,
    pub uploaded_key_found: bool,
}

pub struct SyncWorker {
    sender: mpsc::Sender<bool>,
    status: Arc<Mutex<SyncStatus>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl SyncWorker {
    /// The host saves changed files with momso-core, then calls changed().
    /// The worker also retries periodically and never logs provider output.
    pub fn start(
        github: Arc<Github>,
        root: PathBuf,
        repository: Repository,
        interval: Duration,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let status = Arc::new(Mutex::new(SyncStatus {
            state: "idle",
            reason: None,
            excluded_non_text: 0,
            uploaded_key_found: false,
        }));
        let worker_status = status.clone();
        let worker = std::thread::spawn(move || loop {
            match receiver.recv_timeout(interval) {
                Ok(false) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(true) | Err(mpsc::RecvTimeoutError::Timeout) => (),
            }
            if let Ok(mut status) = worker_status.lock() {
                *status = SyncStatus {
                    state: "syncing",
                    reason: None,
                    excluded_non_text: 0,
                    uploaded_key_found: status.uploaded_key_found,
                };
            }
            let result = github.sync_once(&root, &repository);
            if let Ok(mut status) = worker_status.lock() {
                *status = match result {
                    Ok(outcome) => SyncStatus {
                        state: if outcome.excluded_non_text == 0 {
                            "saved"
                        } else {
                            "saved_with_exclusions"
                        },
                        reason: None,
                        excluded_non_text: outcome.excluded_non_text,
                        uploaded_key_found: outcome.uploaded_key_found || status.uploaded_key_found,
                    },
                    Err(reason) => SyncStatus {
                        state: "failed",
                        reason: Some(reason),
                        excluded_non_text: 0,
                        uploaded_key_found: status.uploaded_key_found,
                    },
                };
            }
        });
        Self {
            sender,
            status,
            worker: Some(worker),
        }
    }
    pub fn changed(&self) -> Result<(), &'static str> {
        self.sender.send(true).map_err(|_| "unavailable")
    }
    pub fn status(&self) -> Result<SyncStatus, &'static str> {
        self.status
            .lock()
            .map(|status| status.clone())
            .map_err(|_| "unavailable")
    }
    pub fn stop(mut self) -> Result<(), &'static str> {
        let _ = self.sender.send(false);
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| "unavailable")?;
        }
        Ok(())
    }
}

impl Drop for SyncWorker {
    fn drop(&mut self) {
        let _ = self.sender.send(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    fn fake_github(token: String) -> (String, Arc<AtomicBool>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = std::thread::spawn(move || {
            let created = AtomicBool::new(false);
            let polls = AtomicUsize::new(0);
            while !worker_stop.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(_) => break,
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0; 4096];
                    let count = stream.read(&mut buffer).unwrap_or(0);
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(index) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&request[..index]);
                        let length = head
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if request.len() >= index + 4 + length {
                            break;
                        }
                    }
                }
                let head = String::from_utf8_lossy(&request);
                let (status, body) = if head.starts_with("POST /login/device/code ") {
                    (
                        200,
                        serde_json::json!({"device_code":"fixture-device-private","user_code":"TEST-CODE","verification_uri":VERIFY_URL,"expires_in":900,"interval":0}),
                    )
                } else if head.starts_with("POST /login/oauth/access_token ") {
                    if polls.fetch_add(1, Ordering::SeqCst) == 0 {
                        (200, serde_json::json!({"error":"authorization_pending"}))
                    } else {
                        (
                            200,
                            serde_json::json!({"access_token":token,"token_type":"bearer","scope":"repo"}),
                        )
                    }
                } else if head.starts_with("GET /user ") {
                    (200, serde_json::json!({"login":"teacher"}))
                } else if head.starts_with("GET /repos/teacher/momso-yoga ") {
                    if created.load(Ordering::SeqCst) {
                        (
                            200,
                            serde_json::json!({"full_name":"teacher/momso-yoga","private":true,"default_branch":"main"}),
                        )
                    } else {
                        (404, serde_json::json!({"message":"not found"}))
                    }
                } else if head.starts_with("GET /user/repos?per_page=100&page=1 ") {
                    (
                        200,
                        serde_json::json!([{ "full_name":"teacher/momso-yoga", "private":true, "default_branch":"main" }, { "full_name":"teacher/public", "private":false, "default_branch":"main" }]),
                    )
                } else if head.starts_with("POST /user/repos ") {
                    assert!(head.contains("\"private\":true"));
                    created.store(true, Ordering::SeqCst);
                    (201, serde_json::json!({"created":true}))
                } else {
                    (404, serde_json::json!({"message":"not found"}))
                };
                let data = body.to_string();
                let response = format!("HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}", data.len());
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (base, stop, worker)
    }

    fn vault(path: &Path) -> Arc<LinuxVault> {
        Arc::new(LinuxVault::new(path, age::x25519::Identity::generate()).unwrap())
    }

    fn assert_token_absent(path: &Path, token: &str) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                assert_token_absent(&entry.path(), token);
            } else if entry.file_type().unwrap().is_file() {
                let data = std::fs::read(entry.path()).unwrap();
                assert!(!data
                    .windows(token.len())
                    .any(|part| part == token.as_bytes()));
            }
        }
    }

    #[test]
    fn device_login_and_private_repo_with_fake_github() {
        let temp = tempfile::tempdir().unwrap();
        let token = uuid::Uuid::new_v4().to_string();
        let (base, stop, server) = fake_github(token.clone());
        let store = vault(&temp.path().join("secrets"));
        let github =
            Github::with_endpoints("fixture-client".into(), store.clone(), &base, &base).unwrap();
        let first = github.start_device_login().unwrap();
        let start = github.login_status(&first.session_id).unwrap();
        assert_eq!(start.state, "pending");
        assert_eq!(start.url.as_deref(), Some(VERIFY_URL));
        assert!(!serde_json::to_string(&start)
            .unwrap()
            .contains("fixture-device-private"));
        assert!(matches!(
            github.login_status("wrong"),
            Err("session_not_found")
        ));
        assert_eq!(
            github.poll_login(&first.session_id).unwrap().state,
            "pending"
        );
        let done = github.poll_login(&first.session_id).unwrap();
        assert_eq!(done.state, "connected");
        assert!(!serde_json::to_string(&done).unwrap().contains(&token));
        assert!(store.read_service_secret(VAULT_KEY).unwrap().as_deref() == Some(token.as_str()));
        assert_eq!(github.list_private_repositories().unwrap().len(), 1);
        let repo = github.create_or_find_repository("yoga").unwrap();
        assert_eq!(repo.name_with_owner, "teacher/momso-yoga");
        assert_eq!(
            github
                .create_or_find_repository("yoga")
                .unwrap()
                .name_with_owner,
            repo.name_with_owner
        );
        for item in std::fs::read_dir(temp.path().join("secrets")).unwrap() {
            let bytes = std::fs::read(item.unwrap().path()).unwrap();
            assert!(!bytes
                .windows(token.len())
                .any(|window| window == token.as_bytes()));
        }
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
    }

    #[test]
    fn git_http_auth_is_only_in_request_header() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let token = uuid::Uuid::new_v4().to_string();
        let expected = format!(
            "Authorization: Basic {}",
            STANDARD.encode(format!("x-access-token:{token}"))
        );
        let server = std::thread::spawn(move || {
            let start = Instant::now();
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && start.elapsed() < Duration::from_secs(10) =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => panic!("fixture request not received"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut received = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                let count = stream.read(&mut chunk).unwrap();
                if count == 0 {
                    break;
                }
                received.extend_from_slice(&chunk[..count]);
                if received.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(received).unwrap();
            assert!(request
                .lines()
                .any(|line| line.eq_ignore_ascii_case(&expected)));
            let _ = stream.write_all(
                b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        });
        let root = tempfile::tempdir().unwrap();
        let output = git(root.path(), &url, &token, &["ls-remote", &url]).unwrap();
        assert!(!output.status.success());
        assert!(!output
            .stdout
            .windows(token.len())
            .any(|part| part == token.as_bytes()));
        assert!(!output
            .stderr
            .windows(token.len())
            .any(|part| part == token.as_bytes()));
        server.join().unwrap();
    }

    #[test]
    fn local_fake_remote_push_restore_and_divergence() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        momso_core::filesystem::initialize(&root).unwrap();
        let rel = "zettels/zet_20261121_120000.md";
        std::fs::write(root.join(rel), "fixture document").unwrap();
        let remote = temp.path().join("remote.git");
        assert!(Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&remote)
            .output()
            .unwrap()
            .status
            .success());
        let store = vault(&temp.path().join("secrets"));
        let token = uuid::Uuid::new_v4().to_string();
        store.store_service_secret(VAULT_KEY, &token).unwrap();
        let github = Github::with_endpoints(
            "fixture".into(),
            store,
            "http://127.0.0.1",
            "http://127.0.0.1",
        )
        .unwrap();
        std::fs::write(root.join("수업-메모.txt"), "기록").unwrap();
        std::fs::write(root.join("fixture-binary.bin"), [0, 1, 2]).unwrap();
        std::fs::write(root.join("database.db"), "private database").unwrap();
        let provider_key = "fixture-provider-key-not-for-upload";
        std::fs::write(root.join("provider-key-test.txt"), provider_key).unwrap();
        assert_eq!(
            github.save_changes_with_known(&root, &[provider_key]),
            Err("credential_in_archive")
        );
        std::fs::write(root.join("provider-key-test.txt"), "safe fixture").unwrap();
        assert_eq!(github.save_changes(&root).unwrap(), 1);
        let url = remote.to_str().unwrap();
        github.push_to(&root, url, "main").unwrap();
        std::fs::write(root.join(rel), provider_key).unwrap();
        momso_core::git::save(&root, &[rel], "fixture secret").unwrap();
        std::fs::write(root.join(rel), "fixture document").unwrap();
        momso_core::git::save(&root, &[rel], "remove fixture secret").unwrap();
        assert_eq!(
            github.push_to_with_known(&root, url, "main", &[provider_key]),
            Err("credential_in_history")
        );
        let restored = temp.path().join("restored");
        github.restore_from(&restored, url).unwrap();
        assert_eq!(
            std::fs::read_to_string(restored.join(rel)).unwrap(),
            "fixture document"
        );
        assert!(restored.join("수업-메모.txt").exists());
        assert!(!restored.join("fixture-binary.bin").exists());
        assert!(!restored.join("database.db").exists());
        assert_eq!(
            github.restore_from(&restored, url),
            Err("destination_exists")
        );
        let empty = temp.path().join("empty-destination");
        std::fs::create_dir(&empty).unwrap();
        github.restore_from(&empty, url).unwrap();
        assert!(empty.join(rel).is_file());
        std::fs::write(restored.join(rel), "remote revision").unwrap();
        momso_core::git::save(&restored, &[rel], "remote").unwrap();
        github.push_to(&restored, url, "main").unwrap();
        assert_token_absent(&remote, &token);
        assert_token_absent(&restored, &token);
        std::fs::write(root.join(rel), "local revision").unwrap();
        momso_core::git::save(&root, &[rel], "local").unwrap();
        assert_eq!(github.push_to(&root, url, "main"), Err("diverged"));
        std::fs::write(root.join(rel), format!("prefix {token} suffix")).unwrap();
        assert_eq!(github.save_changes(&root), Err("credential_in_archive"));
        momso_core::git::save(&root, &[rel], "fixture").unwrap();
        assert_eq!(github.push_to(&root, url, "main"), Err("diverged"));
        let config = std::fs::read(restored.join(".git/config")).unwrap();
        assert!(!config
            .windows(token.len())
            .any(|part| part == token.as_bytes()));
    }

    #[test]
    fn uploaded_secret_warns_without_rewriting_history() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        momso_core::filesystem::initialize(&root).unwrap();
        let rel = "zettels/zet_20261121_120000.md";
        let old_secret = uuid::Uuid::new_v4().to_string();
        std::fs::write(root.join(rel), &old_secret).unwrap();
        momso_core::git::save(&root, &[rel], "before password was known").unwrap();
        let remote = temp.path().join("remote.git");
        assert!(Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&remote)
            .output()
            .unwrap()
            .status
            .success());
        let store = vault(&temp.path().join("secrets"));
        store
            .store_service_secret(VAULT_KEY, &uuid::Uuid::new_v4().to_string())
            .unwrap();
        let github = Github::with_endpoints(
            "fixture".into(),
            store.clone(),
            "http://127.0.0.1",
            "http://127.0.0.1",
        )
        .unwrap();
        let url = remote.to_str().unwrap();
        assert!(!github.push_to(&root, url, "main").unwrap());
        store
            .save_from_phone(momso_saved_login_core::linux::PhoneSaveRequest {
                site_url: "https://example.org".into(),
                label: "fixture".into(),
                username: "fixture".into(),
                password: old_secret,
                ask_before_use: false,
                login_path: None,
                default_for_path: false,
            })
            .unwrap();
        std::fs::write(root.join(rel), "safe replacement").unwrap();
        github.save_changes(&root).unwrap();
        assert!(github.push_to(&root, url, "main").unwrap());
    }

    #[test]
    fn worker_commits_and_uploads_changed_documents() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        momso_core::filesystem::initialize(&root).unwrap();
        let remote = temp.path().join("remote.git");
        assert!(Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&remote)
            .output()
            .unwrap()
            .status
            .success());
        let token = uuid::Uuid::new_v4().to_string();
        let (base, stop, server) = fake_github(token.clone());
        let store = vault(&temp.path().join("secrets"));
        store.store_service_secret(VAULT_KEY, &token).unwrap();
        let github =
            Arc::new(Github::with_endpoints("fixture".into(), store, &base, &base).unwrap());
        let repo = github.create_or_find_repository("yoga").unwrap();
        *github.test_remote.lock().unwrap() = Some(remote.to_str().unwrap().into());
        let rel = "zettels/zet_20261121_120000.md";
        let worker = SyncWorker::start(github, root.clone(), repo, Duration::from_secs(60));
        for text in ["first fixture", "second fixture"] {
            std::fs::write(root.join(rel), text).unwrap();
            worker.changed().unwrap();
            let start = Instant::now();
            loop {
                let show = Command::new("git")
                    .arg("--git-dir")
                    .arg(&remote)
                    .args(["show", &format!("main:{rel}")])
                    .output()
                    .unwrap();
                if show.status.success() && show.stdout == text.as_bytes() {
                    break;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(20),
                    "sync did not finish"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        worker.stop().unwrap();
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        assert_token_absent(&remote, &token);
    }

    #[test]
    fn chat_snapshot_requires_redaction_and_restores_safe_copy() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        momso_core::filesystem::initialize(&root).unwrap();
        std::fs::create_dir(root.join("chats")).unwrap();
        let id = format!("chat_{}", uuid::Uuid::new_v4());
        let rel = format!("chats/{id}.json");
        let token = uuid::Uuid::new_v4().to_string();
        let password = format!("{}\"{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        let store = vault(&temp.path().join("secrets"));
        store.store_service_secret(VAULT_KEY, &token).unwrap();
        store
            .save_from_phone(momso_saved_login_core::linux::PhoneSaveRequest {
                site_url: "https://example.org".into(),
                label: "fixture".into(),
                username: "fixture".into(),
                password: password.clone(),
                ask_before_use: false,
                login_path: None,
                default_for_path: false,
            })
            .unwrap();
        let github = Github::with_endpoints(
            "fixture".into(),
            store,
            "http://127.0.0.1",
            "http://127.0.0.1",
        )
        .unwrap();
        let snapshot = |text: &str| {
            serde_json::json!({"version":1,"room":{"id":id},"messages":[{"text":text}],"groups":[],"imports":[],"importMessages":[],"attachments":[],"redactions":0}).to_string()
        };
        std::fs::write(root.join(&rel), snapshot(&password)).unwrap();
        assert_eq!(github.save_changes(&root), Err("chat_redaction_required"));
        std::fs::write(root.join(&rel), snapshot("[비밀값으로 보여 가림]")).unwrap();
        github.save_changes(&root).unwrap();
        let remote = temp.path().join("remote.git");
        assert!(Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&remote)
            .output()
            .unwrap()
            .status
            .success());
        github
            .push_to(&root, remote.to_str().unwrap(), "main")
            .unwrap();
        let restored = temp.path().join("restored");
        github
            .restore_from(&restored, remote.to_str().unwrap())
            .unwrap();
        let saved = std::fs::read_to_string(restored.join(&rel)).unwrap();
        assert!(saved.contains("가림"));
        assert!(!saved.contains(&password));
        std::fs::write(root.join(&rel), snapshot(&password)).unwrap();
        momso_core::git::save(&root, &[&rel], "fixture").unwrap();
        assert_eq!(
            github.push_to(&root, remote.to_str().unwrap(), "main"),
            Err("credential_in_history")
        );
    }
}
