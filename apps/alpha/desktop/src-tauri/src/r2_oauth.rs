//! Wrangler OAuth authentication and Cloudflare R2 transport.

use serde::Deserialize;
use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    sync::OnceLock,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

// Deliberately no Debug or Serialize: credentials must not be returned to the AI.
#[derive(Clone)]
pub(crate) struct OAuthToken(String);

const TOKEN_CACHE_TIME: Duration = Duration::from_secs(30);

#[derive(Default)]
struct TokenCache {
    value: Option<(OAuthToken, Instant, u64)>,
    generation: u64,
}

impl TokenCache {
    fn invalidate_if_matches(&mut self, token: &OAuthToken) {
        if self
            .value
            .as_ref()
            .is_some_and(|(cached, _, _)| cached.0 == token.0)
        {
            self.value = None;
        }
    }
}

static TOKEN_CACHE: OnceLock<tokio::sync::Mutex<TokenCache>> = OnceLock::new();

fn token_cache() -> &'static tokio::sync::Mutex<TokenCache> {
    TOKEN_CACHE.get_or_init(|| tokio::sync::Mutex::new(TokenCache::default()))
}

async fn acquire_cached_token<F, Fut>(
    cache: &tokio::sync::Mutex<TokenCache>,
    cancel: CancellationToken,
    fetch: F,
) -> Result<(OAuthToken, Option<u64>), &'static str>
where
    F: FnOnce(CancellationToken) -> Fut,
    Fut: Future<Output = Result<OAuthToken, &'static str>>,
{
    if cancel.is_cancelled() {
        return Err("cancelled");
    }
    let mut cache = tokio::select! {
        _ = cancel.cancelled() => return Err("cancelled"),
        cache = cache.lock() => cache,
    };
    if cancel.is_cancelled() {
        return Err("cancelled");
    }
    if let Some((token, until, _)) = &cache.value {
        if Instant::now() < *until {
            return Ok((token.clone(), None));
        }
    }
    cache.value = None;
    let token = fetch(cancel.clone()).await?;
    if cancel.is_cancelled() {
        return Err("cancelled");
    }
    cache.generation = cache.generation.wrapping_add(1);
    let generation = cache.generation;
    cache.value = Some((token.clone(), Instant::now() + TOKEN_CACHE_TIME, generation));
    Ok((token, Some(generation)))
}

async fn invalidate_token(token: &OAuthToken) {
    let mut cache = token_cache().lock().await;
    cache.invalidate_if_matches(token);
}

async fn clear_token_cache() {
    token_cache().lock().await.value = None;
}

static BUNDLED_NODE: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
static HTTP_CLIENT: OnceLock<Result<reqwest::Client, ()>> = OnceLock::new();

fn http_client() -> Result<&'static reqwest::Client, &'static str> {
    HTTP_CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(60))
                .build()
                .map_err(|_| ())
        })
        .as_ref()
        .map_err(|_| "unavailable")
}

pub(crate) fn configure_resources(resources: &std::path::Path) {
    #[cfg(debug_assertions)]
    let roots = [
        resources.to_path_buf(),
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("generated"),
    ];
    #[cfg(not(debug_assertions))]
    let roots = [resources.to_path_buf()];
    for root in roots {
        let node = root.join("node/node.exe");
        let npx = root.join("node/node_modules/npm/bin/npx-cli.js");
        if node.is_file() && npx.is_file() {
            let _ = BUNDLED_NODE.set((node, npx));
            break;
        }
    }
}

async fn run_wrangler(
    args: &[&str],
    timeout: Duration,
    cancel: CancellationToken,
) -> Result<crate::process::ProcessOutput, &'static str> {
    let _timer = crate::startup_timing::start("cli_wrangler");
    let mut environment = HashMap::new();
    if let Some((node, npx)) = BUNDLED_NODE.get() {
        let node_dir = node.parent().ok_or("runtime_missing")?;
        let mut dirs = vec![node_dir.to_path_buf()];
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        let path = std::env::join_paths(dirs).map_err(|_| "runtime_missing")?;
        environment.insert("PATH".to_owned(), path.to_string_lossy().into_owned());
        let script = npx.to_str().ok_or("runtime_missing")?;
        let mut arguments = vec![script, "--yes", "wrangler@4.152.0"];
        arguments.extend_from_slice(args);
        crate::process::run_resolved_tool_cancellable(
            node,
            &arguments,
            None,
            None,
            &environment,
            timeout,
            Some(cancel),
        )
        .await
        .map_err(|_| "wrangler_auth_unavailable")
    } else if cfg!(debug_assertions) {
        let mut arguments = vec!["--yes", "wrangler@4.152.0"];
        arguments.extend_from_slice(args);
        crate::process::run_tool_cancellable(
            "npx",
            &arguments,
            None,
            None,
            &environment,
            timeout,
            Some(cancel),
        )
        .await
        .map_err(|_| "wrangler_auth_unavailable")
    } else {
        Err("runtime_missing")
    }
}

fn login_url(line: &str) -> Option<reqwest::Url> {
    let value = line
        .trim()
        .strip_prefix("Visit this link to authenticate: ")?;
    let url = reqwest::Url::parse(value).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("dash.cloudflare.com")
        || url.port().is_some()
        || url.path() != "/oauth2/auth"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let pairs: Vec<_> = url.query_pairs().collect();
    if pairs
        .iter()
        .filter(|(key, value)| {
            key == "redirect_uri" && value == "http://localhost:8976/oauth/callback"
        })
        .count()
        != 1
        || pairs
            .iter()
            .filter(|(key, _)| key == "redirect_uri")
            .count()
            != 1
        || !pairs
            .iter()
            .any(|(key, value)| key == "state" && !value.is_empty())
        || !pairs
            .iter()
            .any(|(key, value)| key == "response_type" && value == "code")
    {
        return None;
    }
    Some(url)
}

async fn read_login_output<R, F, Fut>(mut reader: R, mut open: F) -> Result<(), &'static str>
where
    R: tokio::io::AsyncRead + Unpin,
    F: FnMut(reqwest::Url) -> Fut,
    Fut: Future<Output = Result<(), &'static str>>,
{
    use tokio::io::AsyncReadExt;
    let mut pending = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut total = 0usize;
    let mut opened = false;
    loop {
        let size = reader
            .read(&mut chunk)
            .await
            .map_err(|_| "wrangler_auth_unavailable")?;
        if size == 0 {
            break;
        }
        total = total.checked_add(size).ok_or("wrangler_auth_unavailable")?;
        if total > 65536 {
            return Err("wrangler_auth_unavailable");
        }
        pending.extend_from_slice(&chunk[..size]);
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<_> = pending.drain(..=end).collect();
            let text = std::str::from_utf8(&line).map_err(|_| "wrangler_auth_unavailable")?;
            if let Some(url) = login_url(text) {
                if opened {
                    return Err("wrangler_auth_unavailable");
                }
                open(url).await?;
                opened = true;
            }
        }
    }
    if opened {
        Ok(())
    } else {
        Err("wrangler_auth_unavailable")
    }
}

async fn run_interactive_login(
    app: tauri::AppHandle,
    chat_id: &str,
    cancel: CancellationToken,
) -> Result<crate::process::ProcessOutput, &'static str> {
    use std::process::Stdio;
    let mut command = if let Some((node, npx)) = BUNDLED_NODE.get() {
        let mut command = crate::process::hidden_async_command(node);
        command
            .arg(npx)
            .args(["--yes", "wrangler@4.152.0", "login", "--browser=false"]);
        let mut dirs = vec![node.parent().ok_or("runtime_missing")?.to_path_buf()];
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        command.env(
            "PATH",
            std::env::join_paths(dirs).map_err(|_| "runtime_missing")?,
        );
        command
    } else if cfg!(debug_assertions) {
        let executable = crate::process::resolve_tool("npx").ok_or("runtime_missing")?;
        let mut command = if executable.with_extension("ps1").is_file() {
            let mut command = crate::process::hidden_async_command("powershell.exe");
            command
                .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-File"])
                .arg(executable.with_extension("ps1"));
            command
        } else {
            crate::process::hidden_async_command(executable)
        };
        command.args(["--yes", "wrangler@4.152.0", "login", "--browser=false"]);
        command
    } else {
        return Err("runtime_missing");
    };
    crate::process::remove_inherited_ai_secrets(&mut command);
    command
        .env_remove("WRANGLER_AUTH_URL")
        .env_remove("WRANGLER_AUTH_DOMAIN")
        .env_remove("WRANGLER_TOKEN_URL")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let panel = std::sync::Arc::new(std::sync::Mutex::new(None));
    let opened = panel.clone();
    let panel_cancel = cancel.clone();
    let chat_id = chat_id.to_owned();
    run_login_process(command, cancel, |url| {
        let app = app.clone();
        let panel = opened.clone();
        let cancel = panel_cancel.clone();
        let chat_id = chat_id.clone();
        async move {
            match crate::panel_browser::open_oauth_panel(app.clone(), &chat_id, url.clone(), cancel)
                .await
            {
                Ok(guard) => {
                    *panel.lock().map_err(|_| "browser_unavailable")? = Some(guard);
                    Ok(())
                }
                Err("cancelled") => Err("cancelled"),
                Err(_) => crate::panel_browser::panel_browser_open_human(app, url.to_string())
                    .await
                    .map_err(|_| "browser_unavailable"),
            }
        }
    })
    .await
}

async fn run_login_process<F, Fut>(
    mut command: tokio::process::Command,
    cancel: CancellationToken,
    open: F,
) -> Result<crate::process::ProcessOutput, &'static str>
where
    F: FnMut(reqwest::Url) -> Fut,
    Fut: Future<Output = Result<(), &'static str>>,
{
    let mut child = command.spawn().map_err(|_| "wrangler_auth_unavailable")?;
    let stdout = child.stdout.take().ok_or("wrangler_auth_unavailable")?;
    let work = async {
        let read = read_login_output(stdout, open);
        let wait = async { child.wait().await.map_err(|_| "wrangler_auth_unavailable") };
        let (_, status) = tokio::try_join!(read, wait)?;
        Ok(crate::process::ProcessOutput {
            success: status.success(),
            stdout: String::new(),
        })
    };
    tokio::select! {
        _ = cancel.cancelled() => Err("cancelled"),
        result = tokio::time::timeout(Duration::from_secs(300), work) => result.map_err(|_| "login_required")?,
    }
}

struct R2Target {
    api: reqwest::Url,
    account: String,
    bucket: String,
}

pub(crate) fn uses_oauth(config: &crate::object_store::Config) -> bool {
    config.provider == "cloudflare" && config.region == "wrangleroauth"
}

fn target_from_config(config: &crate::object_store::Config) -> Result<R2Target, &'static str> {
    let username = config
        .bucket
        .strip_prefix("momso-")
        .ok_or("invalid_bucket")?;
    let endpoint = reqwest::Url::parse(&config.endpoint).map_err(|_| "invalid_endpoint")?;
    let host = endpoint.host_str().ok_or("invalid_endpoint")?;
    if cfg!(test) && endpoint.scheme() == "http" && host == "127.0.0.1" {
        return R2Target::new(
            &format!(
                "http://{host}:{}/",
                endpoint.port().ok_or("invalid_endpoint")?
            ),
            &"a".repeat(32),
            username,
        );
    }
    if endpoint.scheme() != "https"
        || endpoint.port().is_some()
        || endpoint.path() != "/"
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
    {
        return Err("invalid_endpoint");
    }
    let account = host
        .strip_suffix(".r2.cloudflarestorage.com")
        .ok_or("invalid_endpoint")?;
    R2Target::new("https://api.cloudflare.com/", account, username)
}

pub(crate) async fn request(
    config: &crate::object_store::Config,
    keys: &crate::object_store::Keys,
    method: reqwest::Method,
    key: &str,
    query: &[(&str, &str)],
    body: Vec<u8>,
) -> Result<reqwest::Response, &'static str> {
    if !uses_oauth(config)
        || config.write_safety != Some(crate::object_store::WriteSafety::CheckThenWrite)
    {
        return Err("write_safety_unverified");
    }
    let target = target_from_config(config)?;
    #[cfg(test)]
    let token = if target.api.scheme() == "http" {
        OAuthToken(keys.secret_access_key.clone())
    } else {
        current_token(CancellationToken::new()).await?
    };
    #[cfg(not(test))]
    let token = {
        let _ = keys;
        current_token(CancellationToken::new()).await?
    };
    let mut url = if key.is_empty() {
        target.list_url()?
    } else {
        target.object_url(key)?
    };
    if key.is_empty() {
        for (name, value) in query {
            match *name {
                "prefix" => {
                    url.query_pairs_mut().append_pair("prefix", value);
                }
                "continuation-token" => {
                    url.query_pairs_mut().append_pair("cursor", value);
                }
                "max-keys" => {
                    url.query_pairs_mut().append_pair("per_page", value);
                }
                "list-type" => (),
                _ => return Err("invalid_query"),
            }
        }
    } else if !query.is_empty() {
        return Err("invalid_query");
    }
    if method == reqwest::Method::PUT && (key.is_empty() || body.len() > 128 * 1024 * 1024) {
        return Err("invalid_remote_object");
    }
    if !matches!(
        method,
        reqwest::Method::GET
            | reqwest::Method::HEAD
            | reqwest::Method::PUT
            | reqwest::Method::DELETE
    ) {
        return Err("invalid_method");
    }
    if key.is_empty() && method != reqwest::Method::GET {
        return Err("invalid_method");
    }
    // The REST API documents GET but not HEAD. A GET gives the same presence
    // and length information; callers subsequently verify the body.
    let actual_method = if method == reqwest::Method::HEAD {
        reqwest::Method::GET
    } else {
        method
    };
    let mut builder = http_client()?
        .request(actual_method.clone(), url)
        .bearer_auth(&token.0);
    if actual_method == reqwest::Method::PUT {
        builder = builder.header(reqwest::header::CONTENT_TYPE, "application/octet-stream");
    }
    let response = builder
        .body(body)
        .send()
        .await
        .map_err(|_| "invalid_endpoint")?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_token(&token).await;
    }
    Ok(response)
}

impl R2Target {
    fn new(api: &str, account: &str, username: &str) -> Result<Self, &'static str> {
        if account.len() != 32 || !account.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("invalid_account");
        }
        let bucket = crate::object_store::bucket::automatic_name("cloudflare", Some(username))?;
        let api = reqwest::Url::parse(api).map_err(|_| "invalid_endpoint")?;
        let production = api.scheme() == "https"
            && api.host_str() == Some("api.cloudflare.com")
            && api.port().is_none();
        let local_test =
            cfg!(test) && api.scheme() == "http" && api.host_str() == Some("127.0.0.1");
        if !(production || local_test)
            || api.path() != "/"
            || api.query().is_some()
            || api.fragment().is_some()
            || !api.username().is_empty()
            || api.password().is_some()
        {
            return Err("invalid_endpoint");
        }
        Ok(Self {
            api,
            account: account.to_owned(),
            bucket,
        })
    }

    fn object_url(&self, key: &str) -> Result<reqwest::Url, &'static str> {
        if !key.starts_with("momso/object-store/v1/")
            || key
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return Err("invalid_key");
        }
        let mut url = self.api.clone();
        {
            let mut parts = url.path_segments_mut().map_err(|_| "invalid_endpoint")?;
            parts.clear().extend([
                "client",
                "v4",
                "accounts",
                &self.account,
                "r2",
                "buckets",
                &self.bucket,
                "objects",
            ]);
            parts.extend(key.split('/'));
        }
        Ok(url)
    }

    fn list_url(&self) -> Result<reqwest::Url, &'static str> {
        let mut url = self.api.clone();
        url.path_segments_mut()
            .map_err(|_| "invalid_endpoint")?
            .clear()
            .extend([
                "client",
                "v4",
                "accounts",
                &self.account,
                "r2",
                "buckets",
                &self.bucket,
                "objects",
            ]);
        Ok(url)
    }

    fn bucket_url(&self, include_name: bool) -> Result<reqwest::Url, &'static str> {
        let mut url = self.api.clone();
        let mut segments = url.path_segments_mut().map_err(|_| "invalid_endpoint")?;
        segments
            .clear()
            .extend(["client", "v4", "accounts", &self.account, "r2", "buckets"]);
        if include_name {
            segments.push(&self.bucket);
        }
        drop(segments);
        Ok(url)
    }
}

async fn create_exact_bucket(target: &R2Target, token: &OAuthToken) -> Result<(), &'static str> {
    let client = http_client()?;
    let result = client
        .get(target.bucket_url(true)?)
        .bearer_auth(&token.0)
        .send()
        .await
        .map_err(|_| "bucket_lookup_failed")?;
    match result.status() {
        reqwest::StatusCode::NOT_FOUND => (),
        reqwest::StatusCode::OK => {
            let bytes = crate::object_store_transfer::read_bounded(result, 64 * 1024)
                .await
                .map_err(|_| "bucket_lookup_failed")?;
            let metadata: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| "bucket_exists")?;
            return if metadata.get("success").and_then(|v| v.as_bool()) == Some(true)
                && metadata.pointer("/result/name").and_then(|v| v.as_str())
                    == Some(target.bucket.as_str())
            {
                Ok(())
            } else {
                Err("bucket_exists")
            };
        }
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            return Err("access_denied")
        }
        _ => return Err("bucket_lookup_failed"),
    }
    let result = client
        .post(target.bucket_url(false)?)
        .bearer_auth(&token.0)
        .json(&serde_json::json!({"name":target.bucket}))
        .send()
        .await
        .map_err(|_| "bucket_create_unknown")?;
    if result.status() == reqwest::StatusCode::CONFLICT {
        return Err("bucket_name_taken");
    }
    if result.status() == reqwest::StatusCode::UNAUTHORIZED
        || result.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err("bucket_create_denied");
    }
    if !result.status().is_success() {
        return Err("bucket_create_unknown");
    }
    let result: serde_json::Value = result.json().await.map_err(|_| "bucket_create_unknown")?;
    if result.get("success").and_then(|v| v.as_bool()) != Some(true)
        || result.pointer("/result/name").and_then(|v| v.as_str()) != Some(&target.bucket)
    {
        return Err("bucket_create_unknown");
    }
    Ok(())
}

#[cfg(all(test, windows))]
pub(crate) async fn fake_wrangler_and_create(endpoint: &str) -> Result<String, &'static str> {
    let dir = tempfile::tempdir().map_err(|_| "unavailable")?;
    let executable = dir.path().join("fake-wrangler.cmd");
    std::fs::write(
        &executable,
        b"@echo off\r\necho {\"type\":\"oauth\",\"token\":\"test-token\"}\r\n",
    )
    .map_err(|_| "unavailable")?;
    let output = crate::process::run_resolved_tool_cancellable(
        &executable,
        &[],
        None,
        None,
        &HashMap::new(),
        Duration::from_secs(15),
        None,
    )
    .await
    .map_err(|_| "unavailable")?;
    let token = parse_auth_output(output.success, &output.stdout)?;
    let target = R2Target::new(endpoint, &"a".repeat(32), "fixture")?;
    create_exact_bucket(&target, &token).await?;
    Ok(token.0)
}

#[cfg(windows)]
pub(crate) async fn connect_approved(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    chat_id: &str,
    permit: crate::setup_approval::SetupPermit,
    cancel: CancellationToken,
) -> Result<(), &'static str> {
    let paths = state.paths.scoped().await;
    ApprovedConnection {
        data_root: &paths.data_root,
        database: &paths.database_path,
        api: "https://api.cloudflare.com/",
        chat_id,
        cancel,
    }
    .execute(
        permit,
        |cancel| login_account(app.clone(), chat_id, cancel),
        |cancel| async move {
            // A newly approved account must never reuse a token from a prior login.
            clear_token_cache().await;
            current_token(cancel).await
        },
    )
    .await
}

#[cfg(windows)]
struct ApprovedConnection<'a> {
    data_root: &'a std::path::Path,
    database: &'a std::path::Path,
    api: &'a str,
    chat_id: &'a str,
    cancel: CancellationToken,
}

#[cfg(windows)]
impl ApprovedConnection<'_> {
    async fn execute<F, Fut, G, Gf>(
        self,
        permit: crate::setup_approval::SetupPermit,
        login: F,
        fetch_token: G,
    ) -> Result<(), &'static str>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = Result<String, &'static str>>,
        G: FnOnce(CancellationToken) -> Gf,
        Gf: Future<Output = Result<OAuthToken, &'static str>>,
    {
        let details = consume_approval(permit, self.chat_id, &self.cancel)?;
        let _guard = crate::object_store::CONNECTION_LOCK.lock().await;
        if crate::object_store::load_config(self.data_root)?.is_some() {
            return Err("already_connected");
        }
        let username = crate::archive_remote::username_get_db(self.database)
            .map_err(|_| "unavailable")?
            .ok_or("username_missing")?;
        let target = approved_target(&details, &username, self.api)?;
        if login(self.cancel.clone()).await? != details.account {
            return Err("approval_stale");
        }
        let token = fetch_token(self.cancel.clone()).await?;
        if self.cancel.is_cancelled() {
            return Err("cancelled");
        }
        complete_connection(self.data_root, target, token, self.cancel).await
    }
}

#[cfg(windows)]
fn consume_approval(
    permit: crate::setup_approval::SetupPermit,
    chat_id: &str,
    cancel: &CancellationToken,
) -> Result<crate::approval::SetupApproval, &'static str> {
    let details = permit.consume(chat_id)?;
    if details.service != "cloudflare"
        || details.region.as_deref() != Some("auto")
        || cancel.is_cancelled()
    {
        return Err("unavailable");
    }
    Ok(details)
}

#[cfg(windows)]
fn approved_target(
    details: &crate::approval::SetupApproval,
    username: &str,
    api: &str,
) -> Result<R2Target, &'static str> {
    let target = R2Target::new(api, &details.account, username)?;
    if target.bucket != details.resource_name {
        return Err("approval_stale");
    }
    if crate::runtime::r2_test_enabled()
        && !crate::runtime::r2_test_target("cloudflare", &target.bucket)
    {
        return Err("invalid_bucket");
    }
    Ok(target)
}

#[cfg(windows)]
async fn complete_connection(
    data_root: &std::path::Path,
    target: R2Target,
    token: OAuthToken,
    cancel: CancellationToken,
) -> Result<(), &'static str> {
    create_exact_bucket(&target, &token).await?;
    if cancel.is_cancelled() {
        return Err("created_not_connected");
    }
    let config = crate::object_store::Config {
        provider: "cloudflare".into(),
        endpoint: if cfg!(test) && target.api.scheme() == "http" {
            target.api.as_str().trim_end_matches('/').to_owned()
        } else {
            format!("https://{}.r2.cloudflarestorage.com", target.account)
        },
        region: "wrangleroauth".into(),
        bucket: target.bucket,
        credential_id: uuid::Uuid::new_v4().to_string(),
        last_backup_at: None,
        write_safety: Some(crate::object_store::WriteSafety::CheckThenWrite),
    };
    let keys = crate::object_store::load_keys(&config)?;
    crate::object_store_transfer::list_keys(
        &config,
        &keys,
        &format!("{}/manifests/", crate::object_store::PREFIX),
    )
    .await
    .map_err(|_| "created_not_connected")?;
    let key = format!(
        "{}/probes/{}",
        crate::object_store::PREFIX,
        uuid::Uuid::new_v4()
    );
    crate::object_store_transfer::put_once(&config, &keys, &key, b"momso-probe".to_vec())
        .await
        .map_err(|_| "created_not_connected")?;
    if cancel.is_cancelled() {
        return Err("created_not_connected");
    }
    crate::object_store::save_config(data_root, &config).map_err(|_| "created_not_connected")
}

#[cfg(all(test, windows))]
pub(crate) async fn fake_reconnect_existing(
    endpoint: &str,
    data_root: &std::path::Path,
) -> Result<crate::object_store::Config, &'static str> {
    let target = R2Target::new(endpoint, &"a".repeat(32), "fixture")?;
    complete_connection(
        data_root,
        target,
        OAuthToken("test-token".into()),
        CancellationToken::new(),
    )
    .await?;
    let bytes = std::fs::read(data_root.join("object-store.json")).map_err(|_| "unavailable")?;
    serde_json::from_slice(&bytes).map_err(|_| "unavailable")
}

#[cfg(test)]
async fn get_object(
    target: &R2Target,
    token: &OAuthToken,
    key: &str,
    max: usize,
) -> Result<Option<Vec<u8>>, &'static str> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|_| "unavailable")?;
    let mut response = client
        .get(target.object_url(key)?)
        .bearer_auth(&token.0)
        .send()
        .await
        .map_err(|_| "remote_read_failed")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err("remote_read_failed");
    }
    if response
        .content_length()
        .is_some_and(|size| size > max as u64)
    {
        return Err("remote_conflict");
    }
    let mut result = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "remote_read_failed")? {
        if chunk.len() > max.saturating_sub(result.len()) {
            return Err("remote_conflict");
        }
        result.extend_from_slice(&chunk);
    }
    Ok(Some(result))
}

#[cfg(test)]
async fn put_verified(
    target: &R2Target,
    token: &OAuthToken,
    key: &str,
    bytes: &[u8],
) -> Result<(), &'static str> {
    if bytes.len() > 5 * 1024 * 1024 + 1024 * 1024 {
        return Err("object_too_large");
    }
    if let Some(existing) = get_object(target, token, key, bytes.len()).await? {
        return if existing == bytes {
            Ok(())
        } else {
            Err("remote_conflict")
        };
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|_| "unavailable")?;
    let response = client
        .put(target.object_url(key)?)
        .bearer_auth(&token.0)
        .body(bytes.to_vec())
        .send()
        .await
        .map_err(|_| "remote_write_failed")?;
    if !response.status().is_success() {
        return Err("remote_write_failed");
    }
    match get_object(target, token, key, bytes.len()).await? {
        Some(actual)
            if actual.len() == bytes.len()
                && crate::object_store::digest(&actual) == crate::object_store::digest(bytes) =>
        {
            Ok(())
        }
        _ => Err("remote_conflict"),
    }
}

#[derive(Deserialize)]
struct AuthOutput {
    #[serde(rename = "type")]
    kind: String,
    token: Option<String>,
}

fn parse_auth_output(success: bool, stdout: &str) -> Result<OAuthToken, &'static str> {
    if !success || stdout.len() > 16 * 1024 {
        return Err("wrangler_auth_unavailable");
    }
    let output: AuthOutput =
        serde_json::from_str(stdout).map_err(|_| "wrangler_auth_unavailable")?;
    if output.kind != "oauth" {
        return Err("wrangler_oauth_required");
    }
    let token = output.token.ok_or("wrangler_auth_unavailable")?;
    if token.is_empty() || token.len() > 8192 || token.bytes().any(|b| b <= 32 || b >= 127) {
        return Err("wrangler_auth_unavailable");
    }
    Ok(OAuthToken(token))
}

pub(crate) async fn current_token(cancel: CancellationToken) -> Result<OAuthToken, &'static str> {
    let (token, generation) = acquire_cached_token(token_cache(), cancel, fetch_token).await?;
    if let Some(generation) = generation {
        tokio::spawn(async move {
            tokio::time::sleep(TOKEN_CACHE_TIME).await;
            let mut cache = token_cache().lock().await;
            if cache.value.as_ref().is_some_and(|(_, until, current)| {
                *current == generation && Instant::now() >= *until
            }) {
                cache.value = None;
            }
        });
    }
    Ok(token)
}

async fn fetch_token(cancel: CancellationToken) -> Result<OAuthToken, &'static str> {
    let output = run_wrangler(
        &["auth", "token", "--json"],
        Duration::from_secs(90),
        cancel,
    )
    .await?;
    parse_auth_output(output.success, &output.stdout)
}

fn parse_single_account(success: bool, stdout: &str) -> Result<String, &'static str> {
    if !success || stdout.len() > 1024 * 1024 {
        return Err("login_required");
    }
    let value: serde_json::Value =
        serde_json::from_str(stdout).map_err(|_| "wrangler_auth_unavailable")?;
    if value.get("loggedIn").and_then(|v| v.as_bool()) != Some(true) {
        return Err("login_required");
    }
    if value.get("authType").and_then(|v| v.as_str()) != Some("OAuth Token") {
        return Err("wrangler_oauth_required");
    }
    let accounts = value
        .get("accounts")
        .and_then(|v| v.as_array())
        .ok_or("wrangler_auth_unavailable")?;
    if accounts.len() != 1 {
        return Err("account_selection_required");
    }
    let id = accounts[0]
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("wrangler_auth_unavailable")?;
    if id.len() != 32 || !id.bytes().all(|v| v.is_ascii_hexdigit()) {
        return Err("wrangler_auth_unavailable");
    }
    Ok(id.to_ascii_lowercase())
}

pub(crate) async fn login_account(
    app: tauri::AppHandle,
    chat_id: &str,
    cancel: CancellationToken,
) -> Result<String, &'static str> {
    login_account_with(cancel, run_wrangler, |cancel| {
        run_interactive_login(app.clone(), chat_id, cancel)
    })
    .await
}

async fn login_account_with<F, Fut, G, Gf>(
    cancel: CancellationToken,
    run: F,
    login: G,
) -> Result<String, &'static str>
where
    F: Fn(&'static [&'static str], Duration, CancellationToken) -> Fut,
    Fut: std::future::Future<Output = Result<crate::process::ProcessOutput, &'static str>>,
    G: Fn(CancellationToken) -> Gf,
    Gf: Future<Output = Result<crate::process::ProcessOutput, &'static str>>,
{
    let output = run(
        &["whoami", "--json"],
        Duration::from_secs(90),
        cancel.clone(),
    )
    .await?;
    match parse_single_account(output.success, &output.stdout) {
        Ok(account) => Ok(account),
        Err("login_required") => {
            let output = login(cancel.clone()).await?;
            if !output.success {
                return Err("login_required");
            }
            let output = run(&["whoami", "--json"], Duration::from_secs(90), cancel).await?;
            parse_single_account(output.success, &output.stdout)
        }
        Err(error) => Err(error),
    }
}

#[cfg(all(test, windows))]
pub(crate) async fn fake_setup_for_flow_test(
    root: &std::path::Path,
    database: &std::path::Path,
    endpoint: &str,
    chat_id: &str,
    broker: &crate::approval::ApprovalBroker,
    cancel: CancellationToken,
) -> Result<(), &'static str> {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use tokio::io::AsyncWriteExt;
    let logged_in = Arc::new(AtomicBool::new(false));
    let runner_state = logged_in.clone();
    let runner = move |_: &'static [&'static str], _: Duration, _: CancellationToken| {
        let logged_in = runner_state.clone();
        async move {
            let stdout = if logged_in.load(Ordering::SeqCst) {
                serde_json::json!({"loggedIn":true,"authType":"OAuth Token","accounts":[{"id":"a".repeat(32)}]}).to_string()
            } else {
                serde_json::json!({"loggedIn":false}).to_string()
            };
            Ok(crate::process::ProcessOutput {
                success: true,
                stdout,
            })
        }
    };
    let fake_endpoint = endpoint.to_owned();
    let login_state = logged_in.clone();
    let login = move |_: CancellationToken| {
        let logged_in = login_state.clone();
        let endpoint = fake_endpoint.clone();
        async move {
            let (mut writer, reader) = tokio::io::duplex(4096);
            tokio::spawn(async move {
                let _ = writer.write_all(b"Visit this link to authenticate: https://dash.cloudflare.com/oauth2/auth?response_type=code&state=fixture&redirect_uri=http%3A%2F%2Flocalhost%3A8976%2Foauth%2Fcallback\n").await;
            });
            read_login_output(reader, move |url| {
                let endpoint = endpoint.clone();
                let logged_in = logged_in.clone();
                async move {
                    let state = url
                        .query_pairs()
                        .find(|(key, _)| key == "state")
                        .ok_or("invalid_url")?
                        .1
                        .into_owned();
                    let callback = format!("{endpoint}oauth/callback?code=fixture&state={state}");
                    let response = reqwest::get(callback).await.map_err(|_| "unavailable")?;
                    if !response.status().is_success() {
                        return Err("unavailable");
                    }
                    logged_in.store(true, Ordering::SeqCst);
                    Ok(())
                }
            })
            .await?;
            Ok(crate::process::ProcessOutput {
                success: true,
                stdout: String::new(),
            })
        }
    };
    let username = crate::archive_remote::username_get_db(database)
        .map_err(|_| "unavailable")?
        .ok_or("username_missing")?;
    let responder = broker.clone();
    crate::setup_actions::run_r2_flow(
        username,
        cancel,
        move |cancel| login_account_with(cancel, runner, login),
        move |plan, cancel| async move {
            plan.authorize_with(
                broker,
                chat_id,
                "claude",
                cancel,
                move |request| {
                    assert_eq!(
                        request.setup.as_ref().unwrap().resource_name,
                        "momso-basoon"
                    );
                    let id = request.request_id.clone();
                    tokio::spawn(async move {
                        responder
                            .respond(&id, crate::approval::ApprovalChoice::AllowOnce)
                            .await
                            .unwrap();
                    });
                    true
                },
                |_| {},
            )
            .await
        },
        move |permit, cancel| async move {
            ApprovedConnection {
                data_root: root,
                database,
                api: endpoint,
                chat_id,
                cancel,
            }
            .execute(
                permit,
                move |_| async move {
                    if logged_in.load(Ordering::SeqCst) {
                        Ok("a".repeat(32))
                    } else {
                        Err("login_required")
                    }
                },
                |_| async { Ok(OAuthToken("test-token".into())) },
            )
            .await
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cached_oauth_is_shared_expires_and_rejects_stale_invalidation() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let cache = tokio::sync::Mutex::new(TokenCache::default());
        let fetches = AtomicUsize::new(0);
        let fetch = |_: CancellationToken| async {
            let number = fetches.fetch_add(1, Ordering::SeqCst) + 1;
            tokio::task::yield_now().await;
            Ok(OAuthToken(format!("fake-token-{number}")))
        };
        let (one, two, three) = tokio::join!(
            acquire_cached_token(&cache, CancellationToken::new(), fetch),
            acquire_cached_token(&cache, CancellationToken::new(), fetch),
            acquire_cached_token(&cache, CancellationToken::new(), fetch)
        );
        let (old, _) = one.unwrap();
        assert_eq!(two.unwrap().0 .0, old.0);
        assert_eq!(three.unwrap().0 .0, old.0);
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        {
            let mut state = cache.lock().await;
            state.value.as_mut().unwrap().1 = Instant::now() - Duration::from_secs(1);
        }
        let (new, _) = acquire_cached_token(&cache, CancellationToken::new(), fetch)
            .await
            .unwrap();
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
        {
            let mut state = cache.lock().await;
            state.invalidate_if_matches(&old);
            assert!(state.value.is_some());
            state.invalidate_if_matches(&new);
            assert!(state.value.is_none());
        }
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(matches!(
            acquire_cached_token(&cache, cancelled, fetch).await,
            Err("cancelled")
        ));
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn unauthorized_response_discards_token_without_retrying_request() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut request = [0u8; 8192];
            let n = stream.read(&mut request).unwrap();
            assert!(String::from_utf8_lossy(&request[..n]).starts_with("PUT "));
            stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });
        let config = crate::object_store::Config {
            provider: "cloudflare".into(),
            endpoint: format!("http://{address}"),
            region: "wrangleroauth".into(),
            bucket: "momso-fixture".into(),
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(crate::object_store::WriteSafety::CheckThenWrite),
        };
        let keys = crate::object_store::Keys {
            access_key_id: "fake".into(),
            secret_access_key: "fixture-expired-token".into(),
        };
        {
            let mut cache = token_cache().lock().await;
            cache.value = Some((
                OAuthToken(keys.secret_access_key.clone()),
                Instant::now() + TOKEN_CACHE_TIME,
                99,
            ));
        }
        let response = request(
            &config,
            &keys,
            reqwest::Method::PUT,
            "momso/object-store/v1/data/test",
            &[],
            b"fixture".to_vec(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
        assert!(token_cache().lock().await.value.is_none());
        server.join().unwrap();
    }

    #[tokio::test]
    async fn shared_connection_keeps_authorization_per_request() {
        use std::{
            io::{BufRead, BufReader, Write},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut stream = BufReader::new(stream);
            for (index, expected) in ["fake-token-one", "fake-token-two"].into_iter().enumerate() {
                let mut headers = Vec::new();
                loop {
                    let mut line = String::new();
                    assert_ne!(stream.read_line(&mut line).unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                    headers.push(line);
                }
                assert!(headers[0].starts_with("GET "));
                assert!(headers.iter().any(|line| line
                    .trim()
                    .eq_ignore_ascii_case(&format!("authorization: Bearer {expected}"))));
                let connection = if index == 0 { "keep-alive" } else { "close" };
                write!(
                    stream.get_mut(),
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: {connection}\r\n\r\nok"
                )
                .unwrap();
                stream.get_mut().flush().unwrap();
            }
        });
        let config = crate::object_store::Config {
            provider: "cloudflare".into(),
            endpoint: format!("http://{address}"),
            region: "wrangleroauth".into(),
            bucket: "momso-fixture".into(),
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(crate::object_store::WriteSafety::CheckThenWrite),
        };
        for token in ["fake-token-one", "fake-token-two"] {
            let keys = crate::object_store::Keys {
                access_key_id: "fake".into(),
                secret_access_key: token.into(),
            };
            let response = tokio::time::timeout(
                Duration::from_secs(5),
                request(
                    &config,
                    &keys,
                    reqwest::Method::GET,
                    "momso/object-store/v1/data/test",
                    &[],
                    vec![],
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(response.bytes().await.unwrap().as_ref(), b"ok");
        }
        server.join().unwrap();
    }

    #[tokio::test]
    async fn checked_upload_reuses_connection_after_small_error_and_write_bodies() {
        use std::{
            io::{BufRead, BufReader, Read, Write},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut stream = BufReader::new(stream);
            for (index, method) in ["GET", "PUT", "GET"].into_iter().enumerate() {
                let mut first = String::new();
                assert_ne!(stream.read_line(&mut first).unwrap(), 0);
                assert!(first.starts_with(&format!("{method} ")));
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    assert_ne!(stream.read_line(&mut line).unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut request_body = vec![0; length];
                stream.read_exact(&mut request_body).unwrap();
                if method == "PUT" {
                    assert_eq!(request_body, b"payload");
                }
                let (status, body) = match index {
                    0 => ("404 Not Found", "{\"success\":false}"),
                    1 => ("200 OK", "{\"success\":true}"),
                    _ => ("200 OK", "payload"),
                };
                let connection = if index == 2 { "close" } else { "keep-alive" };
                write!(stream.get_mut(), "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: {connection}\r\n\r\n{body}", body.len()).unwrap();
                stream.get_mut().flush().unwrap();
            }
        });
        let config = crate::object_store::Config {
            provider: "cloudflare".into(),
            endpoint: format!("http://{address}"),
            region: "wrangleroauth".into(),
            bucket: "momso-fixture".into(),
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(crate::object_store::WriteSafety::CheckThenWrite),
        };
        let keys = crate::object_store::Keys {
            access_key_id: "fake".into(),
            secret_access_key: "fake-token".into(),
        };
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            crate::object_store_transfer::put_once(
                &config,
                &keys,
                "momso/object-store/v1/data/test",
                b"payload".to_vec(),
            ),
        )
        .await
        .unwrap();
        assert!(result.is_ok());
        server.join().unwrap();
    }

    #[tokio::test]
    async fn creates_only_approved_bucket_and_refuses_mismatched_existing() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            sync::{Arc, Mutex},
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let paths = Arc::new(Mutex::new(Vec::new()));
        let seen = paths.clone();
        let server = std::thread::spawn(move || {
            for index in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut data = Vec::new();
                let mut buffer = [0u8; 2048];
                let end = loop {
                    let count = stream.read(&mut buffer).unwrap();
                    assert_ne!(count, 0);
                    data.extend_from_slice(&buffer[..count]);
                    if let Some(i) = data.windows(4).position(|p| p == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let head = String::from_utf8(data[..end].to_vec()).unwrap();
                let line = head.lines().next().unwrap().to_owned();
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|s| s.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while data.len() - end < length {
                    let n = stream.read(&mut buffer).unwrap();
                    assert_ne!(n, 0);
                    data.extend_from_slice(&buffer[..n]);
                }
                seen.lock().unwrap().push(line.clone());
                let (status, body) = match index {
                    0 => {
                        assert!(
                            line.starts_with("GET ") && line.contains("/buckets/momso-fixture ")
                        );
                        ("404 Not Found", "{}".to_owned())
                    }
                    1 => {
                        assert!(line.starts_with("POST ") && line.contains("/r2/buckets "));
                        assert_eq!(
                            serde_json::from_slice::<serde_json::Value>(&data[end..end + length])
                                .unwrap(),
                            serde_json::json!({"name":"momso-fixture"})
                        );
                        (
                            "200 OK",
                            serde_json::json!({"success":true,"result":{"name":"momso-fixture"}})
                                .to_string(),
                        )
                    }
                    _ => {
                        assert!(
                            line.starts_with("GET ") && line.contains("/buckets/momso-fixture ")
                        );
                        ("200 OK", serde_json::json!({"success":true,"result":{"name":"someone-elses-archive-bucket"}}).to_string())
                    }
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let target =
            R2Target::new(&format!("http://{address}/"), &"a".repeat(32), "fixture").unwrap();
        let token = OAuthToken("fake-token".into());
        create_exact_bucket(&target, &token).await.unwrap();
        assert_eq!(
            create_exact_bucket(&target, &token).await,
            Err("bucket_exists")
        );
        server.join().unwrap();
        let paths = paths.lock().unwrap();
        assert_eq!(paths.len(), 3);
        assert!(!paths
            .iter()
            .any(|line| line.contains("someone-elses-private-bucket")
                || line.contains("someone-elses-archive-bucket")));
    }

    #[tokio::test]
    async fn uncertain_creation_never_reports_confirmed_or_retries() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            sync::{Arc, Mutex},
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let paths = Arc::new(Mutex::new(Vec::new()));
        let seen = paths.clone();
        let server = std::thread::spawn(move || {
            for index in 0..6 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut request = [0u8; 8192];
                let n = stream.read(&mut request).unwrap();
                let first = String::from_utf8_lossy(&request[..n])
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned();
                seen.lock().unwrap().push(first.clone());
                if index % 2 == 0 {
                    assert!(first.starts_with("GET ") && first.contains("/buckets/momso-fixture "));
                    stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                } else {
                    assert!(first.starts_with("POST ") && first.contains("/r2/buckets "));
                    match index {
                        1 => (), // The connection closes after the request, without a response.
                        3 => stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap(),
                        _ => stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap(),
                    }
                }
            }
        });
        let target =
            R2Target::new(&format!("http://{address}/"), &"a".repeat(32), "fixture").unwrap();
        let token = OAuthToken("test-token".into());
        for _ in 0..3 {
            assert_eq!(
                create_exact_bucket(&target, &token).await,
                Err("bucket_create_unknown")
            );
        }
        server.join().unwrap();
        assert_eq!(paths.lock().unwrap().len(), 6);
    }

    #[test]
    fn account_discovery_refuses_ambiguous_or_non_oauth_sessions() {
        let id = "a".repeat(32);
        let good =
            serde_json::json!({"loggedIn":true,"authType":"OAuth Token","accounts":[{"id":id}]})
                .to_string();
        assert_eq!(parse_single_account(true, &good).unwrap(), "a".repeat(32));
        let many = serde_json::json!({"loggedIn":true,"authType":"OAuth Token","accounts":[{"id":"a".repeat(32)},{"id":"b".repeat(32)}]}).to_string();
        assert_eq!(
            parse_single_account(true, &many),
            Err("account_selection_required")
        );
        let api = serde_json::json!({"loggedIn":true,"authType":"User API Token","accounts":[{"id":"a".repeat(32)}]}).to_string();
        assert_eq!(
            parse_single_account(true, &api),
            Err("wrangler_oauth_required")
        );
        assert_eq!(parse_single_account(false, ""), Err("login_required"));
    }

    #[tokio::test]
    async fn login_required_uses_login_then_rechecks_single_account() {
        use std::sync::{Arc, Mutex};
        let calls = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
        let observed = calls.clone();
        let runner = move |args: &'static [&'static str], _: Duration, _: CancellationToken| {
            let calls = observed.clone();
            async move {
                let mut calls = calls.lock().unwrap();
                calls.push(args.iter().map(|s| (*s).to_owned()).collect());
                let stdout = if calls.len() == 1 {
                    r#"{"loggedIn":false}"#.to_owned()
                } else {
                    serde_json::json!({"loggedIn":true,"authType":"OAuth Token","accounts":[{"id":"a".repeat(32)}]}).to_string()
                };
                Ok(crate::process::ProcessOutput {
                    success: true,
                    stdout,
                })
            }
        };
        let login_calls = calls.clone();
        let login = move |_: CancellationToken| {
            let calls = login_calls.clone();
            async move {
                calls
                    .lock()
                    .unwrap()
                    .push(vec!["login".into(), "--browser=false".into()]);
                Ok(crate::process::ProcessOutput {
                    success: true,
                    stdout: String::new(),
                })
            }
        };
        assert_eq!(
            login_account_with(CancellationToken::new(), runner, login)
                .await
                .unwrap(),
            "a".repeat(32)
        );
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                vec!["whoami", "--json"],
                vec!["login", "--browser=false"],
                vec!["whoami", "--json"]
            ]
        );
        let target = R2Target::new("http://127.0.0.1:1234/", &"a".repeat(32), "basoon").unwrap();
        assert_eq!(target.bucket, "momso-basoon");
        for invalid in ["../other", "other/bucket", "UPPER"] {
            assert!(R2Target::new("http://127.0.0.1:1234/", &"a".repeat(32), invalid).is_err());
        }
    }

    #[tokio::test]
    async fn fake_oauth_output_opens_only_cloudflare_login() {
        use tokio::io::AsyncWriteExt;
        let good = "https://dash.cloudflare.com/oauth2/auth?response_type=code&state=fixture&redirect_uri=http%3A%2F%2Flocalhost%3A8976%2Foauth%2Fcallback";
        for invalid in ["https://dash.cloudflare.com.evil.test/oauth2/auth?response_type=code&state=x&redirect_uri=http%3A%2F%2Flocalhost%3A8976%2Foauth%2Fcallback", "https://dash.cloudflare.com/oauth2/auth?response_type=code&state=x&redirect_uri=https%3A%2F%2Fevil.test", "https://dash.cloudflare.com/oauth2/auth?response_type=code&state=x&redirect_uri=http%3A%2F%2Flocalhost%3A8976%2Foauth%2Fcallback&redirect_uri=https%3A%2F%2Fevil.test"] {
            assert!(login_url(&format!("Visit this link to authenticate: {invalid}")).is_none());
        }
        let (mut writer, reader) = tokio::io::duplex(4096);
        let output = format!("fixture output\nVisit this link to authenticate: {good}\n");
        tokio::spawn(async move {
            writer.write_all(output.as_bytes()).await.unwrap();
        });
        let opened = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let capture = opened.clone();
        read_login_output(reader, move |url| {
            capture.lock().unwrap().push(url);
            async { Ok(()) }
        })
        .await
        .unwrap();
        assert_eq!(opened.lock().unwrap().len(), 1);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn fake_wrangler_waits_for_owned_browser_without_default_browser() {
        use std::process::Stdio;
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("opened");
        let script = temp.path().join("fake-login.ps1");
        std::fs::write(&script, r#"param($marker)
[Console]::Out.WriteLine('Visit this link to authenticate: https://dash.cloudflare.com/oauth2/auth?response_type=code&state=fixture&redirect_uri=http%3A%2F%2Flocalhost%3A8976%2Foauth%2Fcallback')
for ($i = 0; $i -lt 100; $i++) { if (Test-Path -LiteralPath $marker) { exit 0 }; Start-Sleep -Milliseconds 25 }
exit 1
"#).unwrap();
        let mut command = crate::process::hidden_async_command("powershell.exe");
        command
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-File"])
            .arg(&script)
            .arg(&marker)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let opened = marker.clone();
        let result = run_login_process(command, CancellationToken::new(), move |url| {
            assert_eq!(url.host_str(), Some("dash.cloudflare.com"));
            std::fs::write(&opened, b"opened").unwrap();
            async { Ok(()) }
        })
        .await
        .unwrap();
        assert!(result.success && result.stdout.is_empty() && marker.is_file());
    }

    #[tokio::test]
    async fn fake_r2_checks_content_round_trip_and_exact_bucket() {
        use std::{
            collections::HashMap,
            io::{Read, Write},
            net::TcpListener,
            sync::{Arc, Mutex},
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let objects = Arc::new(Mutex::new(HashMap::<String, Vec<u8>>::new()));
        let received = Arc::new(Mutex::new(Vec::<String>::new()));
        let server_objects = objects.clone();
        let server_paths = received.clone();
        let server = std::thread::spawn(move || {
            for _ in 0..6 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut data = Vec::new();
                let mut buffer = [0u8; 4096];
                let end = loop {
                    let count = stream.read(&mut buffer).unwrap();
                    assert_ne!(count, 0);
                    data.extend_from_slice(&buffer[..count]);
                    if let Some(index) = data.windows(4).position(|p| p == b"\r\n\r\n") {
                        break index + 4;
                    }
                };
                let headers = String::from_utf8(data[..end].to_vec()).unwrap();
                let mut first = headers.lines().next().unwrap().split_whitespace();
                let method = first.next().unwrap();
                let path = first.next().unwrap();
                server_paths.lock().unwrap().push(path.to_string());
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while data.len() - end < length {
                    let count = stream.read(&mut buffer).unwrap();
                    assert_ne!(count, 0);
                    data.extend_from_slice(&buffer[..count]);
                }
                let mut map = server_objects.lock().unwrap();
                let (status, body) = if method == "PUT" {
                    map.insert(path.to_owned(), data[end..end + length].to_vec());
                    ("200 OK", Vec::new())
                } else if let Some(body) = map.get(path) {
                    ("200 OK", body.clone())
                } else {
                    ("404 Not Found", Vec::new())
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let target =
            R2Target::new(&format!("http://{address}/"), &"a".repeat(32), "fixture").unwrap();
        #[cfg(windows)]
        let token = {
            let dir = tempfile::tempdir().unwrap();
            let executable = dir.path().join("fake-wrangler.cmd");
            std::fs::write(
                &executable,
                b"@echo off\r\necho {\"type\":\"oauth\",\"token\":\"test-token\"}\r\n",
            )
            .unwrap();
            let output = crate::process::run_resolved_tool_cancellable(
                &executable,
                &[],
                None,
                None,
                &HashMap::new(),
                Duration::from_secs(15),
                None,
            )
            .await
            .unwrap();
            parse_auth_output(output.success, &output.stdout).unwrap()
        };
        #[cfg(not(windows))]
        let token = parse_auth_output(true, r#"{"type":"oauth","token":"test-token"}"#).unwrap();
        let identity = age::x25519::Identity::generate();
        let original = b"synthetic source";
        let encrypted = crate::object_store_transfer::encrypt_bytes(original, &identity).unwrap();
        let key = "momso/object-store/v1/data/example";
        put_verified(&target, &token, key, &encrypted)
            .await
            .unwrap();
        put_verified(&target, &token, key, &encrypted)
            .await
            .unwrap();
        assert_eq!(
            put_verified(&target, &token, key, b"different").await,
            Err("remote_conflict")
        );
        let downloaded = get_object(&target, &token, key, encrypted.len())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::object_store_transfer::decrypt_bytes(&downloaded, &identity, 1024).unwrap(),
            original
        );
        server.join().unwrap();
        assert!(received
            .lock()
            .unwrap()
            .iter()
            .all(|path| path == &format!(
                "/client/v4/accounts/{}/r2/buckets/momso-fixture/objects/momso/object-store/v1/data/example",
                "a".repeat(32)
            )));
        assert!(target.object_url("../../outside").is_err());
        assert!(R2Target::new("https://example.com/", &"a".repeat(32), "fixture").is_err());
    }

    #[test]
    fn accepts_only_a_valid_oauth_response() {
        assert!(parse_auth_output(true, r#"{"type":"oauth","token":"fake-token"}"#).is_ok());
        for output in [
            r#"{"type":"api_token","token":"fake-token"}"#,
            concat!(
                r#"{"type":"api_key","key":"fake-key","email":"x"#,
                "@",
                r#"example.test"}"#
            ),
            r#"{"type":"oauth","token":"bad\ntoken"}"#,
            r#"{"type":"oauth","token":""}"#,
            "not json",
        ] {
            assert!(parse_auth_output(true, output).is_err());
        }
        assert!(parse_auth_output(false, r#"{"type":"oauth","token":"fake-token"}"#).is_err());
    }
}
