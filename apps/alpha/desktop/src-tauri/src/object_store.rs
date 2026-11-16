use crate::{credentials, AppState};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::Path};
use tauri::State;
use uuid::Uuid;

#[path = "object_store_bucket.rs"]
pub(crate) mod bucket;

type HmacSha256 = Hmac<Sha256>;
pub(crate) const PREFIX: &str = "momso/object-store/v1";
pub(crate) static CONNECTION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn r2_test_target(provider: &str, bucket: &str) -> bool {
    provider == "cloudflare" && bucket == "momso-basoon"
}

fn r2_test_connect(provider: &str, automatic: bool, bucket: &str) -> bool {
    automatic && r2_test_target(provider, bucket)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WriteSafety {
    Conditional,
    CheckThenWrite,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Config {
    pub(crate) provider: String,
    pub(crate) endpoint: String,
    pub(crate) region: String,
    pub(crate) bucket: String,
    pub(crate) credential_id: String,
    pub(crate) last_backup_at: Option<String>,
    #[serde(default)]
    pub(crate) write_safety: Option<WriteSafety>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectInput {
    provider: String,
    endpoint: String,
    region: String,
    bucket: String,
    access_key_id: String,
    secret_access_key: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Keys {
    pub(crate) access_key_id: String,
    pub(crate) secret_access_key: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoreStatus {
    state: &'static str,
    provider: Option<String>,
    bucket: Option<String>,
    last_backup_at: Option<String>,
    pending_count: Option<u64>,
    pending_bytes: Option<u64>,
    write_safety: Option<WriteSafety>,
}

fn status(state: &'static str, config: Option<Config>) -> StoreStatus {
    StoreStatus {
        state,
        provider: config.as_ref().map(|c| c.provider.clone()),
        bucket: config.as_ref().map(|c| c.bucket.clone()),
        last_backup_at: config.as_ref().and_then(|c| c.last_backup_at.clone()),
        pending_count: None,
        pending_bytes: None,
        write_safety: config.and_then(|c| c.write_safety),
    }
}

fn config_path(root: &Path) -> std::path::PathBuf {
    root.join("object-store.json")
}

pub(crate) fn load_config(root: &Path) -> Result<Option<Config>, &'static str> {
    match fs::read(config_path(root)) {
        Ok(data) => {
            let config: Config = serde_json::from_slice(&data).map_err(|_| "unavailable")?;
            if crate::runtime::r2_test_enabled()
                && !r2_test_target(&config.provider, &config.bucket)
            {
                return Err("invalid_bucket");
            }
            let id = Uuid::parse_str(&config.credential_id).map_err(|_| "unavailable")?;
            if id.to_string() != config.credential_id {
                return Err("unavailable");
            }
            validate(&ConnectInput {
                provider: config.provider.clone(),
                endpoint: config.endpoint.clone(),
                region: config.region.clone(),
                bucket: config.bucket.clone(),
                access_key_id: "validation".into(),
                secret_access_key: "validation".into(),
            })
            .map_err(|_| "unavailable")?;
            Ok(Some(config))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("unavailable"),
    }
}

pub(crate) fn save_config(root: &Path, config: &Config) -> Result<(), &'static str> {
    let mut temp = tempfile::NamedTempFile::new_in(root).map_err(|_| "unavailable")?;
    serde_json::to_writer(&mut temp, config).map_err(|_| "unavailable")?;
    temp.flush().map_err(|_| "unavailable")?;
    temp.as_file().sync_all().map_err(|_| "unavailable")?;
    temp.persist(config_path(root)).map_err(|_| "unavailable")?;
    Ok(())
}

fn key_name(config: &Config) -> String {
    format!("object-store:{}", config.credential_id)
}

pub(crate) fn load_keys(config: &Config) -> Result<Keys, &'static str> {
    let secret = credentials::get_secret(&key_name(config))
        .map_err(|_| "unavailable")?
        .ok_or("unavailable")?;
    serde_json::from_str(&secret).map_err(|_| "unavailable")
}

fn validate(input: &ConnectInput) -> Result<Config, &'static str> {
    if !matches!(
        input.provider.as_str(),
        "aws" | "gcp" | "cloudflare" | "ncp" | "s3_compatible"
    ) {
        return Err("invalid_provider");
    }
    let url = reqwest::Url::parse(input.endpoint.trim()).map_err(|_| "invalid_endpoint")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("invalid_endpoint");
    }
    if input.region.is_empty()
        || !input
            .region
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        return Err("invalid_region");
    }
    if input.provider == "gcp" && !bucket::valid_name("gcp", &input.bucket) {
        return Err("invalid_bucket");
    }
    if input.provider != "gcp"
        && (input.bucket.len() < 3
            || input.bucket.len() > 63
            || !input
                .bucket
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'.'))
    {
        return Err("invalid_bucket");
    }
    if input.access_key_id.is_empty()
        || input.secret_access_key.is_empty()
        || input.access_key_id.bytes().any(|b| !b.is_ascii_graphic())
    {
        return Err("invalid_credentials");
    }
    let config = Config {
        provider: input.provider.clone(),
        endpoint: url.as_str().trim_end_matches('/').into(),
        region: input.region.clone(),
        bucket: input.bucket.clone(),
        credential_id: Uuid::new_v4().to_string(),
        last_backup_at: None,
        write_safety: None,
    };
    if matches!(config.provider.as_str(), "aws" | "gcp" | "cloudflare")
        && !write_safety_known(&config)
    {
        return Err("invalid_endpoint");
    }
    Ok(config)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn digest(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hmac(key: &[u8], message: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts arbitrary key length");
    mac.update(message.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn encode(value: &str, slash: bool) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'.' | b'~')
            || (slash && b == b'/')
        {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

struct SignedRequest {
    url: String,
    host: String,
    date: String,
    payload_hash: String,
    authorization: String,
}

fn sign(
    config: &Config,
    keys: &Keys,
    method: &str,
    object: &str,
    query: &[(&str, &str)],
    body: &[u8],
    now: DateTime<Utc>,
) -> Result<SignedRequest, &'static str> {
    let endpoint = reqwest::Url::parse(&config.endpoint).map_err(|_| "invalid_endpoint")?;
    let host = match endpoint.port() {
        Some(port) => format!("{}:{port}", endpoint.host_str().ok_or("invalid_endpoint")?),
        None => endpoint.host_str().ok_or("invalid_endpoint")?.to_owned(),
    };
    let path = format!(
        "/{}/{}",
        encode(&config.bucket, false),
        encode(object, true)
    );
    let path = if object.is_empty() {
        format!("/{}", encode(&config.bucket, false))
    } else {
        path
    };
    let mut pairs: Vec<_> = query
        .iter()
        .map(|(k, v)| (encode(k, false), encode(v, false)))
        .collect();
    pairs.sort();
    let query = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let day = now.format("%Y%m%d").to_string();
    let payload_hash = digest(body);
    let google = config.provider == "gcp";
    let prefix = if google { "goog" } else { "amz" };
    let google_condition =
        method == "PUT" && google && config.write_safety == Some(WriteSafety::Conditional);
    let headers = format!(
        "host:{host}\nx-{prefix}-content-sha256:{payload_hash}\nx-{prefix}-date:{date}\n{}",
        if google_condition {
            "x-goog-if-generation-match:0\n"
        } else {
            ""
        }
    );
    let signed_headers = format!(
        "host;x-{prefix}-content-sha256;x-{prefix}-date{}",
        if google_condition {
            ";x-goog-if-generation-match"
        } else {
            ""
        }
    );
    let canonical =
        format!("{method}\n{path}\n{query}\n{headers}\n{signed_headers}\n{payload_hash}");
    let (algorithm, key_prefix, service, terminator) = if google {
        ("GOOG4-HMAC-SHA256", "GOOG4", "storage", "goog4_request")
    } else if config.provider == "aws-sts" {
        ("AWS4-HMAC-SHA256", "AWS4", "sts", "aws4_request")
    } else {
        ("AWS4-HMAC-SHA256", "AWS4", "s3", "aws4_request")
    };
    let scope = format!("{day}/{}/{service}/{terminator}", config.region);
    let string_to_sign = format!(
        "{algorithm}\n{date}\n{scope}\n{}",
        digest(canonical.as_bytes())
    );
    let day_key = hmac(
        format!("{key_prefix}{}", keys.secret_access_key).as_bytes(),
        &day,
    );
    let region_key = hmac(&day_key, &config.region);
    let service_key = hmac(&region_key, service);
    let signing_key = hmac(&service_key, terminator);
    let signature = hex(&hmac(&signing_key, &string_to_sign));
    let authorization = format!(
        "{algorithm} Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        keys.access_key_id
    );
    let url = format!(
        "{}{path}{}",
        config.endpoint,
        if query.is_empty() {
            String::new()
        } else {
            format!("?{query}")
        }
    );
    Ok(SignedRequest {
        url,
        host,
        date,
        payload_hash,
        authorization,
    })
}

#[cfg(windows)]
async fn verify_aws_account(
    keys: &Keys,
    expected: &str,
    endpoint: &str,
) -> Result<(), &'static str> {
    // AWS GetCallerIdentity returns the account associated with these credentials.
    // https://docs.aws.amazon.com/STS/latest/APIReference/API_GetCallerIdentity.html
    let config = Config {
        provider: "aws-sts".into(),
        endpoint: endpoint.into(),
        region: "us-east-1".into(),
        bucket: String::new(),
        credential_id: String::new(),
        last_backup_at: None,
        write_safety: None,
    };
    let response = request(
        &config,
        keys,
        reqwest::Method::GET,
        "",
        &[("Action", "GetCallerIdentity"), ("Version", "2011-06-15")],
        vec![],
    )
    .await
    .map_err(|_| "account_unverified")?;
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        if body.contains("InvalidClientTokenId") || body.contains("SignatureDoesNotMatch") {
            return Err("credentials_rejected_before_create");
        }
        return Err("account_unverified");
    }
    #[derive(Deserialize)]
    struct CallerIdentity {
        #[serde(rename = "GetCallerIdentityResult")]
        result: CallerIdentityResult,
    }
    #[derive(Deserialize)]
    struct CallerIdentityResult {
        #[serde(rename = "Account")]
        account: String,
    }
    let body = response.text().await.map_err(|_| "account_unverified")?;
    let identity: CallerIdentity =
        quick_xml::de::from_str(&body).map_err(|_| "account_unverified")?;
    if identity.result.account != expected {
        return Err("account_unverified");
    }
    Ok(())
}

pub(crate) async fn request(
    config: &Config,
    keys: &Keys,
    method: reqwest::Method,
    object: &str,
    query: &[(&str, &str)],
    body: Vec<u8>,
) -> Result<reqwest::Response, &'static str> {
    if config.provider == "cloudflare" && config.bucket.is_empty() {
        return Err("invalid_bucket");
    }
    if method == reqwest::Method::PUT && config.write_safety.is_none() {
        return Err("write_safety_unverified");
    }
    let signed = sign(
        config,
        keys,
        method.as_str(),
        object,
        query,
        &body,
        Utc::now(),
    )?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|_| "unavailable")?;
    let is_put = method == reqwest::Method::PUT;
    let mut builder = client
        .request(method, signed.url)
        .header("host", signed.host)
        .header("authorization", signed.authorization);
    builder = if config.provider == "gcp" {
        builder
            .header("x-goog-date", signed.date)
            .header("x-goog-content-sha256", signed.payload_hash)
    } else {
        builder
            .header("x-amz-date", signed.date)
            .header("x-amz-content-sha256", signed.payload_hash)
    };
    if is_put && config.write_safety == Some(WriteSafety::Conditional) {
        builder = if config.provider == "gcp" {
            builder.header("x-goog-if-generation-match", "0")
        } else {
            builder.header("if-none-match", "*")
        };
    }
    builder
        .body(body)
        .send()
        .await
        .map_err(|_| "invalid_endpoint")
}

fn write_safety_known(config: &Config) -> bool {
    if let Ok(url) = reqwest::Url::parse(&config.endpoint) {
        if url.scheme() == "https" {
            if let Some(host) = url.host_str() {
                let known = match config.provider.as_str() {
                    "aws" => {
                        host == format!("s3.{}.amazonaws.com", config.region)
                            || host == format!("s3.{}.amazonaws.com.cn", config.region)
                            || (config.region == "us-east-1" && host == "s3.amazonaws.com")
                    }
                    "gcp" => host == "storage.googleapis.com",
                    "cloudflare" => host.ends_with(".r2.cloudflarestorage.com"),
                    _ => false,
                };
                if known {
                    return true;
                }
            }
        }
    }
    // Tests can exercise S3-compatible fake servers only on loopback.
    if cfg!(test) {
        if let Ok(url) = reqwest::Url::parse(&config.endpoint) {
            return url
                .host_str()
                .is_some_and(|host| matches!(host, "127.0.0.1" | "[::1]" | "::1"));
        }
    }
    false
}

async fn check_response(response: reqwest::Response, write: bool) -> Result<(), &'static str> {
    let code = response.status();
    if code.is_success() {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    if body.contains("NoSuchBucket") || code == reqwest::StatusCode::NOT_FOUND {
        return Err("bucket_not_found");
    }
    if body.contains("InvalidAccessKeyId")
        || body.contains("SignatureDoesNotMatch")
        || body.contains("InvalidToken")
        || code == reqwest::StatusCode::UNAUTHORIZED
    {
        return Err("invalid_credentials");
    }
    if code == reqwest::StatusCode::FORBIDDEN {
        return Err(if write {
            "write_denied"
        } else {
            "access_denied"
        });
    }
    Err("unavailable")
}

pub(crate) async fn probe_connection(
    config: &Config,
    keys: &Keys,
) -> Result<WriteSafety, &'static str> {
    let list = request(
        config,
        keys,
        reqwest::Method::GET,
        "",
        &[("list-type", "2"), ("max-keys", "1")],
        vec![],
    )
    .await?;
    check_response(list, false).await?;
    let probe = format!("{PREFIX}/probes/{}", Uuid::new_v4());
    let mut conditional = config.clone();
    conditional.write_safety = Some(WriteSafety::Conditional);
    let mut plain = config.clone();
    plain.write_safety = Some(WriteSafety::CheckThenWrite);
    let response = request(
        &plain,
        keys,
        reqwest::Method::PUT,
        &probe,
        &[],
        b"first".to_vec(),
    )
    .await?;
    check_response(response, true).await?;
    let response = request(
        &conditional,
        keys,
        reqwest::Method::PUT,
        &probe,
        &[],
        b"second".to_vec(),
    )
    .await?;
    let rejected = response.status() == reqwest::StatusCode::PRECONDITION_FAILED;
    let unsupported = matches!(response.status().as_u16(), 400 | 501);
    if !rejected && !unsupported && !response.status().is_success() {
        check_response(response, true).await?;
    }
    let response = request(config, keys, reqwest::Method::GET, &probe, &[], vec![]).await?;
    if !response.status().is_success() {
        return Err("remote_read_failed");
    }
    let body = response.bytes().await.map_err(|_| "remote_read_failed")?;
    if rejected && body.as_ref() == b"first" {
        Ok(WriteSafety::Conditional)
    } else if (!rejected && body.as_ref() == b"second")
        || (unsupported && body.as_ref() == b"first")
    {
        Ok(WriteSafety::CheckThenWrite)
    } else {
        Err("write_safety_unverified")
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn object_store_connect(
    state: State<'_, AppState>,
    provider: String,
    endpoint: String,
    region: String,
    bucket: Option<String>,
    access_key_id: String,
    secret_access_key: String,
    gcp_project_id: Option<String>,
    gcp_oauth_token: Option<String>,
) -> Result<StoreStatus, &'static str> {
    let paths = state.paths.scoped().await;
    let _guard = CONNECTION_LOCK.lock().await;
    let requested_bucket = bucket.unwrap_or_default();
    let automatic = requested_bucket.trim().is_empty()
        || requested_bucket.trim().eq_ignore_ascii_case("auto")
        || requested_bucket.trim() == "\u{c790}\u{b3d9}";
    let bucket = if automatic {
        stored_automatic_name(&paths.database_path, &provider)?
    } else {
        requested_bucket
    };
    if crate::runtime::r2_test_enabled() && !r2_test_connect(&provider, automatic, &bucket) {
        return Err("invalid_bucket");
    }
    let input = ConnectInput {
        provider,
        endpoint,
        region,
        bucket,
        access_key_id,
        secret_access_key,
    };
    let mut config = validate(&input)?;
    let keys = Keys {
        access_key_id: input.access_key_id,
        secret_access_key: input.secret_access_key,
    };
    if automatic {
        bucket::ensure(
            &config,
            &keys,
            gcp_project_id.as_deref(),
            gcp_oauth_token.as_deref(),
        )
        .await?;
    }
    config.write_safety = Some(probe_connection(&config, &keys).await?);
    let prior = load_config(&paths.data_root)?;
    let stored = serde_json::to_string(&keys).map_err(|_| "unavailable")?;
    credentials::set_secret(&key_name(&config), &stored).map_err(|_| "unavailable")?;
    if let Err(error) = save_config(&paths.data_root, &config) {
        let _ = credentials::delete_secret(&key_name(&config));
        return Err(error);
    }
    if let Some(old) = prior {
        let _ = credentials::delete_secret(&key_name(&old));
    }
    Ok(status("connected", Some(config)))
}

fn stored_automatic_name(path: &std::path::Path, provider: &str) -> Result<String, &'static str> {
    let username = crate::archive_remote::username_get_db(path).map_err(|_| "unavailable")?;
    bucket::automatic_name(provider, username.as_deref())
}

/// The skill path can only create a new bucket for the approved account.
#[cfg(windows)]
pub(crate) async fn connect_approved(
    state: State<'_, AppState>,
    chat_id: &str,
    permit: crate::setup_approval::SetupPermit,
    secret: crate::setup_secret::SecretLease,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<(), &'static str> {
    let details = permit.consume(chat_id)?;
    let valid_account = match details.service.as_str() {
        "cloudflare" => {
            details.account.len() == 32 && details.account.bytes().all(|b| b.is_ascii_hexdigit())
        }
        "gcp" => {
            (6..=30).contains(&details.account.len())
                && details
                    .account
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        }
        "aws" => details.account.len() == 12 && details.account.bytes().all(|b| b.is_ascii_digit()),
        _ => false,
    };
    let region = details.region.as_deref().ok_or("invalid_region")?;
    let valid_region = match details.service.as_str() {
        "aws" => {
            region != "us-east-1"
                && region.len() <= 32
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        }
        _ => region == "auto",
    };
    if !valid_account || !valid_region || cancel.is_cancelled() {
        return Err("unavailable");
    }
    let paths = state.paths.scoped().await;
    let _guard = CONNECTION_LOCK.lock().await;
    if load_config(&paths.data_root)?.is_some() {
        return Err("already_connected");
    }
    if stored_automatic_name(&paths.database_path, &details.service)? != details.resource_name {
        return Err("approval_stale");
    }
    let secret = secret.take()?;
    let input = ConnectInput {
        provider: details.service.clone(),
        endpoint: match details.service.as_str() {
            "gcp" => "https://storage.googleapis.com".into(),
            "aws" => format!("https://s3.{region}.amazonaws.com"),
            _ => format!("https://{}.r2.cloudflarestorage.com", details.account),
        },
        region: region.into(),
        bucket: details.resource_name,
        access_key_id: secret.access_key_id,
        secret_access_key: secret.secret_access_key,
    };
    let mut config = validate(&input)?;
    let keys = Keys {
        access_key_id: input.access_key_id,
        secret_access_key: input.secret_access_key,
    };
    if cancel.is_cancelled() {
        return Err("cancelled");
    }
    if config.provider == "aws" {
        verify_aws_account(&keys, &details.account, "https://sts.amazonaws.com").await?;
        if cancel.is_cancelled() {
            return Err("cancelled");
        }
    }
    bucket::ensure_new(
        &config,
        &keys,
        Some(&details.account),
        secret.gcp_oauth_token.as_deref(),
    )
    .await
    .map_err(|error| match error {
        "invalid_credentials" | "gcp_oauth_invalid" => "credentials_rejected_before_create",
        _ => error,
    })?;
    if cancel.is_cancelled() {
        return Err("created_not_connected");
    }
    config.write_safety = Some(
        probe_connection(&config, &keys)
            .await
            .map_err(|_| "created_not_connected")?,
    );
    if cancel.is_cancelled() {
        return Err("created_not_connected");
    }
    let stored = serde_json::to_string(&keys).map_err(|_| "unavailable")?;
    credentials::set_secret(&key_name(&config), &stored).map_err(|_| "unavailable")?;
    if let Err(error) = save_config(&paths.data_root, &config) {
        let _ = credentials::delete_secret(&key_name(&config));
        return Err(error);
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn object_store_status(
    state: State<'_, AppState>,
) -> Result<StoreStatus, &'static str> {
    let paths = state.paths.scoped().await;
    let _guard = CONNECTION_LOCK.lock().await;
    Ok(match load_config(&paths.data_root) {
        Ok(None) => status("not_connected", None),
        Ok(Some(config)) if config.write_safety.is_none() => status("unavailable", Some(config)),
        Ok(Some(config)) => match load_keys(&config) {
            Ok(keys) => match request(
                &config,
                &keys,
                reqwest::Method::GET,
                "",
                &[("list-type", "2"), ("max-keys", "1")],
                vec![],
            )
            .await
            {
                Ok(response) if response.status().is_success() => {
                    let mut result = status("connected", Some(config.clone()));
                    if let Ok(identity) =
                        crate::object_store_transfer::identity(&paths.database_path)
                    {
                        if let Ok(pending) = crate::object_store_transfer::pending_files(
                            &config,
                            &keys,
                            &identity,
                            &paths.objects_path,
                            |_| {},
                        )
                        .await
                        {
                            result.pending_count = Some(pending.len() as u64);
                            result.pending_bytes = pending
                                .iter()
                                .try_fold(0u64, |sum, item| sum.checked_add(item.2));
                        } else {
                            result.state = "unavailable";
                        }
                    }
                    result
                }
                _ => status("unavailable", Some(config)),
            },
            Err(_) => status("unavailable", Some(config)),
        },
        Err(_) => status("unavailable", None),
    })
}

#[tauri::command]
pub(crate) async fn object_store_disconnect(
    state: State<'_, AppState>,
) -> Result<(), &'static str> {
    let paths = state.paths.scoped().await;
    let _guard = CONNECTION_LOCK.lock().await;
    let root = &paths.data_root;
    if let Some(config) = load_config(root)? {
        fs::remove_file(config_path(root)).map_err(|_| "unavailable")?;
        credentials::delete_secret(&key_name(&config)).map_err(|_| "unavailable")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[tokio::test]
    async fn aws_account_check_uses_fake_sts_and_rejects_wrong_account() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let fake = std::thread::spawn(move || {
            for index in 0..3 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = [0u8; 8192];
                let size = socket.read(&mut request).unwrap();
                let text = std::str::from_utf8(&request[..size]).unwrap();
                assert!(text.contains("Action=GetCallerIdentity"));
                assert!(text.contains("/sts/aws4_request"));
                let (status, body) = if index == 2 {
                    ("403 Forbidden", "<ErrorResponse><Error><Code>InvalidClientTokenId</Code></Error></ErrorResponse>")
                } else {
                    ("200 OK", "<GetCallerIdentityResponse><GetCallerIdentityResult><Account>123456789012</Account></GetCallerIdentityResult></GetCallerIdentityResponse>")
                };
                write!(
                    socket,
                    "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    body.len(),
                    body
                )
                .unwrap();
            }
        });
        let keys = Keys {
            access_key_id: "fake-id".into(),
            secret_access_key: "fake-secret".into(),
        };
        assert!(verify_aws_account(&keys, "123456789012", &endpoint)
            .await
            .is_ok());
        assert_eq!(
            verify_aws_account(&keys, "999999999999", &endpoint).await,
            Err("account_unverified")
        );
        assert_eq!(
            verify_aws_account(&keys, "123456789012", &endpoint).await,
            Err("credentials_rejected_before_create")
        );
        fake.join().unwrap();
    }

    #[test]
    fn live_test_only_allows_the_automatic_basoon_bucket() {
        assert!(r2_test_connect("cloudflare", true, "momso-basoon"));
        assert!(!r2_test_connect("cloudflare", false, "momso-basoon"));
        assert!(!r2_test_connect("cloudflare", true, "momso-other"));
        assert!(!r2_test_connect("aws", true, "momso-basoon"));
        assert!(r2_test_target("cloudflare", "momso-basoon"));
        assert!(!r2_test_target("cloudflare", "someone-elses-private-bucket"));
        assert!(!r2_test_target("cloudflare", "someone-elses-archive-bucket"));
    }
    use chrono::TimeZone;
    use std::io::{Read, Write};

    #[test]
    fn automatic_bucket_uses_the_saved_username() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("app.db");
        crate::storage::initialize(&db, &dir.path().join("backups"), "test").unwrap();
        assert_eq!(
            stored_automatic_name(&db, "cloudflare"),
            Err("username_required")
        );
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection.execute("INSERT INTO app_runtime_meta(key, value, updated_at) VALUES('momso_username', 'basoon', 'now')", []).unwrap();
        assert_eq!(
            stored_automatic_name(&db, "cloudflare").unwrap(),
            "momso-basoon"
        );
    }

    #[test]
    fn provider_endpoints_and_signatures_are_deterministic() {
        let cases = [
            (
                "aws",
                "https://s3.ap-northeast-2.amazonaws.com",
                "ap-northeast-2",
                "108af7bf8ba7c9940ea43d1e2a54fc36ed46bd7a0602db3e03ff146244b5bf53",
            ),
            (
                "gcp",
                "https://storage.googleapis.com",
                "auto",
                "e2279370666d467a4d45f73d5f21dabf5b9938b033a2b32ae11c5ec9cea37dd3",
            ),
            (
                "cloudflare",
                "https://account.r2.cloudflarestorage.com",
                "auto",
                "fbda2cecb0f661ce197a2af926d0a720716ee01867254636d7f09f47b6b34f75",
            ),
            (
                "ncp",
                "https://kr.object.ncloudstorage.com",
                "kr-standard",
                "ed306f5182bd89d269e68750feaeeeab96b4dd1960e15a5f6b70073420788406",
            ),
        ];
        let keys = Keys {
            access_key_id: "TESTKEY".into(),
            secret_access_key: "test-secret".into(),
        };
        for (provider, endpoint, region, expected_signature) in cases {
            let input = ConnectInput {
                provider: provider.into(),
                endpoint: endpoint.into(),
                region: region.into(),
                bucket: "test-bucket".into(),
                access_key_id: keys.access_key_id.clone(),
                secret_access_key: keys.secret_access_key.clone(),
            };
            let c = validate(&input).unwrap();
            let r = sign(
                &c,
                &keys,
                "GET",
                "a b/한글",
                &[("prefix", "a b"), ("list-type", "2")],
                b"",
                Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap(),
            )
            .unwrap();
            assert_eq!(
                r.url,
                format!("{endpoint}/test-bucket/a%20b/%ED%95%9C%EA%B8%80?list-type=2&prefix=a%20b")
            );
            let scope = if provider == "gcp" {
                "storage/goog4_request"
            } else {
                "s3/aws4_request"
            };
            assert!(r
                .authorization
                .contains(&format!("/20260102/{region}/{scope}")));
            assert!(r.authorization.ends_with(expected_signature));
            assert_eq!(
                r.payload_hash,
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            );
        }
    }

    #[test]
    fn config_has_no_key_material_and_rejects_unsafe_endpoint() {
        let mut input = ConnectInput {
            provider: "aws".into(),
            endpoint: "https://s3.ap-northeast-2.amazonaws.com".into(),
            region: "ap-northeast-2".into(),
            bucket: "test-bucket".into(),
            access_key_id: "access-secret-example".into(),
            secret_access_key: "private-secret-example".into(),
        };
        let config = validate(&input).unwrap();
        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains("secret-example"));
        input.endpoint = "https://user:password@example.com".into();
        assert!(matches!(validate(&input), Err("invalid_endpoint")));
    }

    #[test]
    fn google_put_signs_the_create_only_precondition() {
        let config = Config {
            provider: "gcp".into(),
            endpoint: "https://storage.googleapis.com".into(),
            region: "auto".into(),
            bucket: "test-bucket".into(),
            credential_id: Uuid::new_v4().to_string(),
            last_backup_at: None,
            write_safety: Some(WriteSafety::Conditional),
        };
        let keys = Keys {
            access_key_id: "TESTKEY".into(),
            secret_access_key: "test-secret".into(),
        };
        let signed = sign(
            &config,
            &keys,
            "PUT",
            "test",
            &[],
            b"",
            Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap(),
        )
        .unwrap();
        assert!(signed.authorization.starts_with("GOOG4-HMAC-SHA256 "));
        assert!(signed.authorization.contains("x-goog-if-generation-match"));
        assert!(signed
            .authorization
            .ends_with("f490b6bfa58abbded8a6f8c07b1f93e8dfaf0e45042e7e90a74ed135ff57713f"));
    }

    #[test]
    fn unverified_providers_cannot_write_to_remote_endpoints() {
        let mut config = Config {
            provider: "ncp".into(),
            endpoint: "https://kr.object.ncloudstorage.com".into(),
            region: "kr-standard".into(),
            bucket: "bucket".into(),
            credential_id: Uuid::new_v4().to_string(),
            last_backup_at: None,
            write_safety: Some(WriteSafety::Conditional),
        };
        assert!(!write_safety_known(&config));
        config.provider = "s3_compatible".into();
        assert!(!write_safety_known(&config));
        config.provider = "aws".into();
        config.endpoint = "https://s3.ap-northeast-2.amazonaws.com".into();
        config.region = "ap-northeast-2".into();
        assert!(write_safety_known(&config));
    }

    #[tokio::test]
    async fn local_server_accepts_good_key_and_rejects_wrong_key() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for index in 0..5 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 1024];
                while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buffer).unwrap();
                    assert_ne!(n, 0);
                    bytes.extend_from_slice(&buffer[..n]);
                }
                let request = String::from_utf8(bytes).unwrap();
                if index == 2 {
                    assert!(request.to_ascii_lowercase().contains("if-none-match: *"));
                }
                let response = if request.contains("Credential=GOOD/") && index == 2 {
                    "HTTP/1.1 412 Precondition Failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                } else if request.contains("Credential=GOOD/") && index == 3 {
                    "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nfirst"
                } else if request.contains("Credential=GOOD/") {
                    "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                } else {
                    "HTTP/1.1 403 Forbidden\r\nContent-Length: 46\r\nConnection: close\r\n\r\n<Error><Code>InvalidAccessKeyId</Code></Error>"
                };
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        let config = Config {
            provider: "s3_compatible".into(),
            endpoint: format!("http://{address}"),
            region: "auto".into(),
            bucket: "bucket".into(),
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: Some(WriteSafety::Conditional),
        };
        let good = Keys {
            access_key_id: "GOOD".into(),
            secret_access_key: "test".into(),
        };
        let bad = Keys {
            access_key_id: "BAD".into(),
            secret_access_key: "test".into(),
        };
        assert_eq!(
            probe_connection(&config, &good).await,
            Ok(WriteSafety::Conditional)
        );
        assert_eq!(
            probe_connection(&config, &bad).await,
            Err("invalid_credentials")
        );
        server.join().unwrap();
    }
}
