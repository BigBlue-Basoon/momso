//! GitHub OAuth device flow for the momso desktop application.
//! Token values remain in the system credential store and never enter events.
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) const CLIENT_ID: &str = "Ov23lic2FXUtMb3WprIN";
const SECRET_NAME: &str = "github:momso:oauth";
const ACCESS_REDACTION: &str = "ai:github-oauth-access:api-key";
const REFRESH_REDACTION: &str = "ai:github-oauth-refresh:api-key";
const WEB: &str = "https://github.com";
static TOKEN_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
#[path = "github_oauth_tests.rs"]
mod tests;

#[derive(Deserialize)]
pub(crate) struct DeviceGrant {
    pub(crate) device_code: String,
    pub(crate) user_code: String,
    pub(crate) verification_uri: String,
    pub(crate) expires_in: u64,
    pub(crate) interval: u64,
}

#[derive(Deserialize)]
pub(crate) struct TokenReply {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    refresh_token_expires_in: Option<u64>,
    pub(crate) error: Option<String>,
    pub(crate) interval: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct TokenBundle {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<u64>,
    refresh_expires_at: Option<u64>,
}

fn now() -> Result<u64, &'static str> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "unavailable")?
        .as_secs())
}

fn bundle(reply: TokenReply) -> Result<TokenBundle, &'static str> {
    let access = reply
        .access_token
        .filter(|v| !v.is_empty())
        .ok_or("unavailable")?;
    match (
        reply.refresh_token,
        reply.expires_in,
        reply.refresh_token_expires_in,
    ) {
        (None, None, None) => Ok(TokenBundle {
            access_token: access,
            refresh_token: None,
            expires_at: None,
            refresh_expires_at: None,
        }),
        (Some(refresh), Some(expiry), Some(refresh_expiry))
            if !refresh.is_empty() && expiry > 0 && refresh_expiry > 0 =>
        {
            let timestamp = now()?;
            Ok(TokenBundle {
                access_token: access,
                refresh_token: Some(refresh),
                expires_at: Some(timestamp.checked_add(expiry).ok_or("unavailable")?),
                refresh_expires_at: Some(
                    timestamp.checked_add(refresh_expiry).ok_or("unavailable")?,
                ),
            })
        }
        _ => Err("unavailable"),
    }
}

fn client() -> Result<reqwest::blocking::Client, &'static str> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("momso-desktop")
        .build()
        .map_err(|_| "unavailable")
}

fn post<T: for<'de> Deserialize<'de>>(
    base: &str,
    path: &str,
    fields: &[(&str, &str)],
) -> Result<T, &'static str> {
    let response = client()?
        .post(format!("{base}{path}"))
        .header("Accept", "application/json")
        .form(fields)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|_| "unavailable")?;
    let mut body = Vec::new();
    response
        .take(32 * 1024 + 1)
        .read_to_end(&mut body)
        .map_err(|_| "unavailable")?;
    if body.len() > 32 * 1024 {
        return Err("unavailable");
    }
    serde_json::from_slice(&body).map_err(|_| "unavailable")
}

fn begin_at(base: &str) -> Result<DeviceGrant, &'static str> {
    let grant: DeviceGrant = post(
        base,
        "/login/device/code",
        &[("client_id", CLIENT_ID), ("scope", "repo")],
    )?;
    if grant.verification_uri != "https://github.com/login/device"
        || grant.device_code.is_empty()
        || grant.user_code.is_empty()
        || grant.expires_in == 0
        || grant.interval == 0
        || grant.expires_in > 3600
        || grant.interval > grant.expires_in
    {
        return Err("unavailable");
    }
    Ok(grant)
}

pub(crate) fn begin() -> Result<DeviceGrant, &'static str> {
    std::thread::spawn(|| begin_at(WEB))
        .join()
        .map_err(|_| "unavailable")?
}

fn poll_at(base: &str, code: &str) -> Result<TokenReply, &'static str> {
    post(
        base,
        "/login/oauth/access_token",
        &[
            ("client_id", CLIENT_ID),
            ("device_code", code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ],
    )
}

pub(crate) fn poll(code: &str) -> Result<TokenReply, &'static str> {
    let code = code.to_owned();
    std::thread::spawn(move || poll_at(WEB, &code))
        .join()
        .map_err(|_| "unavailable")?
}

pub(crate) fn has_access(reply: &TokenReply) -> bool {
    reply
        .access_token
        .as_ref()
        .is_some_and(|value| !value.is_empty())
}

fn store_unlocked(bundle: &TokenBundle) -> Result<(), &'static str> {
    // Separate protected entries let the existing archive redaction machinery
    // retain rotated values without ever writing either token to a file.
    for (name, next) in [
        (ACCESS_REDACTION, Some(bundle.access_token.as_str())),
        (REFRESH_REDACTION, bundle.refresh_token.as_deref()),
    ] {
        let old = crate::credentials::get_secret(name).map_err(|_| "github_storage_unknown")?;
        if let Some(old) = old.filter(|old| Some(old.as_str()) != next) {
            crate::redaction_wait::enqueue_unlocked(name, &old)
                .map_err(|_| "github_storage_unknown")?;
        }
        if let Some(next) = next {
            crate::credentials::put_secret_unlocked(name, next)
                .map_err(|_| "github_storage_unknown")?;
            crate::redaction_wait::watch_new_unlocked(name)
                .map_err(|_| "github_storage_unknown")?;
        } else {
            crate::credentials::remove_secret_unlocked(name)
                .map_err(|_| "github_storage_unknown")?;
        }
    }
    let value = serde_json::to_string(bundle).map_err(|_| "unavailable")?;
    crate::credentials::put_secret_unlocked(SECRET_NAME, &value)
        .map_err(|_| "github_storage_unknown")
}

pub(crate) fn save(
    reply: TokenReply,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(), &'static str> {
    if cancel.is_cancelled() {
        return Err("cancelled");
    }
    let _secret_guard =
        crate::credentials::prevent_secret_changes().map_err(|_| "github_storage_unknown")?;
    let _guard = TOKEN_LOCK.lock().map_err(|_| "unavailable")?;
    if cancel.is_cancelled() {
        return Err("cancelled");
    }
    store_unlocked(&bundle(reply)?)
}

pub(crate) fn known_values_with(
    get: impl Fn(&str) -> Result<Option<String>, &'static str>,
) -> Result<Vec<String>, &'static str> {
    let Some(value) = get(SECRET_NAME)? else {
        return Ok(Vec::new());
    };
    let bundle: TokenBundle =
        serde_json::from_str(&value).map_err(|_| "credentials_unavailable")?;
    let mut values = vec![bundle.access_token];
    if let Some(refresh) = bundle.refresh_token {
        values.push(refresh);
    }
    Ok(values)
}

fn resolve_token(
    base: &str,
    stored: TokenBundle,
    save: impl FnOnce(&TokenBundle) -> Result<(), &'static str>,
) -> Result<String, &'static str> {
    let timestamp = now()?;
    if stored
        .expires_at
        .is_none_or(|expires| timestamp.saturating_add(60) < expires)
    {
        return Ok(stored.access_token);
    }
    if stored
        .refresh_expires_at
        .is_some_and(|expires| timestamp >= expires)
    {
        return Err("signed_out");
    }
    let refresh = stored.refresh_token.as_deref().ok_or("signed_out")?;
    let reply: TokenReply = post(
        base,
        "/login/oauth/access_token",
        &[
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
        ],
    )?;
    if reply.error.as_deref() == Some("bad_refresh_token") {
        return Err("signed_out");
    }
    let next = bundle(reply)?;
    if next.refresh_token.is_none() {
        return Err("unavailable");
    }
    save(&next)?;
    Ok(next.access_token)
}

fn read_bundle() -> Result<TokenBundle, &'static str> {
    let stored = crate::credentials::get_secret(SECRET_NAME)
        .map_err(|_| "github_storage_unknown")?
        .ok_or("signed_out")?;
    serde_json::from_str(&stored).map_err(|_| "github_storage_unknown")
}

fn token_inner(base: &str) -> Result<String, &'static str> {
    let stored = read_bundle()?;
    if stored
        .expires_at
        .is_none_or(|expires| now().is_ok_and(|time| time.saturating_add(60) < expires))
    {
        return Ok(stored.access_token);
    }
    // A backup may already hold this lock while invoking Git. Never wait for
    // it while holding TOKEN_LOCK, and never rotate a token before it is held.
    let _secret_guard =
        crate::credentials::try_prevent_secret_changes().map_err(|_| "inProgress")?;
    let _guard = TOKEN_LOCK.lock().map_err(|_| "unavailable")?;
    resolve_token(base, read_bundle()?, store_unlocked)
}

pub(crate) fn token() -> Result<String, &'static str> {
    // reqwest's blocking runtime must not run on a Tokio runtime thread.
    std::thread::spawn(|| token_inner(WEB))
        .join()
        .map_err(|_| "unavailable")?
}

fn verify_at(base: &str, token: &str) -> Result<(), &'static str> {
    let response = client()?
        .get(format!("{base}/user"))
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|_| "unavailable")?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("signed_out");
    }
    response.error_for_status().map_err(|_| "unavailable")?;
    Ok(())
}

pub(crate) fn verify() -> Result<(), &'static str> {
    let token = token()?;
    std::thread::spawn(move || verify_at("https://api.github.com", &token))
        .join()
        .map_err(|_| "unavailable")?
}
