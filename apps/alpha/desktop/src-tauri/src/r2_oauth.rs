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

pub(crate) fn configure_resources(resources: &std::path::Path) {
    let roots = [
        resources.to_path_buf(),
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("generated"),
    ];
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
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|_| "unavailable")?
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
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|_| "unavailable")?;
    let result = client
        .get(target.bucket_url(true)?)
        .bearer_auth(&token.0)
        .send()
        .await
        .map_err(|_| "bucket_lookup_failed")?;
    match result.status() {
        reqwest::StatusCode::NOT_FOUND => (),
        reqwest::StatusCode::OK => return Err("bucket_exists"),
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
    state: tauri::State<'_, crate::AppState>,
    chat_id: &str,
    permit: crate::setup_approval::SetupPermit,
    cancel: CancellationToken,
) -> Result<(), &'static str> {
    let details = permit.consume(chat_id)?;
    if details.service != "cloudflare"
        || details.region.as_deref() != Some("auto")
        || cancel.is_cancelled()
    {
        return Err("unavailable");
    }
    let paths = state.paths.scoped().await;
    let _guard = crate::object_store::CONNECTION_LOCK.lock().await;
    if crate::object_store::load_config(&paths.data_root)?.is_some() {
        return Err("already_connected");
    }
    let username = crate::archive_remote::username_get_db(&paths.database_path)
        .map_err(|_| "unavailable")?
        .ok_or("username_missing")?;
    let target = R2Target::new("https://api.cloudflare.com/", &details.account, &username)?;
    if target.bucket != details.resource_name {
        return Err("approval_stale");
    }
    if crate::runtime::r2_test_enabled()
        && !crate::runtime::r2_test_target("cloudflare", &target.bucket)
    {
        return Err("invalid_bucket");
    }
    if login_account(cancel.clone()).await? != details.account {
        return Err("approval_stale");
    }
    // A newly approved account must never reuse a token from a prior login.
    clear_token_cache().await;
    let token = current_token(cancel.clone()).await?;
    if cancel.is_cancelled() {
        return Err("cancelled");
    }
    create_exact_bucket(&target, &token).await?;
    if cancel.is_cancelled() {
        return Err("created_not_connected");
    }
    let config = crate::object_store::Config {
        provider: "cloudflare".into(),
        endpoint: format!("https://{}.r2.cloudflarestorage.com", target.account),
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
    let response = request(
        &config,
        &keys,
        reqwest::Method::PUT,
        &key,
        &[],
        b"momso-probe".to_vec(),
    )
    .await
    .map_err(|_| "created_not_connected")?;
    if !response.status().is_success() {
        return Err("created_not_connected");
    }
    let response = request(&config, &keys, reqwest::Method::GET, &key, &[], vec![])
        .await
        .map_err(|_| "created_not_connected")?;
    if !response.status().is_success()
        || response
            .bytes()
            .await
            .map_err(|_| "created_not_connected")?
            .as_ref()
            != b"momso-probe"
    {
        return Err("created_not_connected");
    }
    if cancel.is_cancelled() {
        return Err("created_not_connected");
    }
    crate::object_store::save_config(&paths.data_root, &config).map_err(|_| "created_not_connected")
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

pub(crate) async fn login_account(cancel: CancellationToken) -> Result<String, &'static str> {
    let output = run_wrangler(
        &["whoami", "--json"],
        Duration::from_secs(90),
        cancel.clone(),
    )
    .await?;
    match parse_single_account(output.success, &output.stdout) {
        Ok(account) => Ok(account),
        Err("login_required") => {
            let output = run_wrangler(&["login"], Duration::from_secs(300), cancel.clone()).await?;
            if !output.success {
                return Err("login_required");
            }
            let output =
                run_wrangler(&["whoami", "--json"], Duration::from_secs(90), cancel).await?;
            parse_single_account(output.success, &output.stdout)
        }
        Err(error) => Err(error),
    }
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
    async fn creates_only_approved_bucket_and_refuses_existing_one() {
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
                        ("200 OK", "{}".to_owned())
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
            .all(|path| path.contains("/buckets/momso-fixture/objects/")));
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
