use crate::{process, provider_auth};
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use std::{
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tauri::Emitter;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageWindow {
    kind: String,
    scope: Option<serde_json::Value>,
    label: Option<String>,
    limit_id: Option<String>,
    used_percent: f64,
    resets_at: Option<i64>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageStatus {
    provider: &'static str,
    state: &'static str,
    reason: Option<&'static str>,
    windows: Vec<UsageWindow>,
    remaining: Option<f64>,
    plan: Option<String>,
    plan_label: Option<&'static str>,
    checked_at: String,
}
fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}
fn unavailable(provider: &'static str, state: &'static str, reason: &'static str) -> UsageStatus {
    UsageStatus {
        provider,
        state,
        reason: Some(reason),
        windows: Vec::new(),
        remaining: None,
        plan: None,
        plan_label: None,
        checked_at: now(),
    }
}
fn unavailable_without_reason(provider: &'static str) -> UsageStatus {
    unavailable(provider, "unavailable", "checking")
}
fn auth_unavailable(provider: &'static str, installed: bool, state: &str) -> Option<UsageStatus> {
    let reason = if !installed {
        "not_installed"
    } else {
        match state {
            "account" => return None,
            "apiKey" if provider == "gemini" => "no_quota_api",
            "apiKey" => "api_key",
            "signedOut" => "not_connected",
            _ => "fetch_failed",
        }
    };
    Some(unavailable(provider, "unavailable", reason))
}
async fn fetch_gemini() -> UsageStatus {
    let status = provider_auth::read_status("gemini").await;
    auth_unavailable("gemini", status.installed, status.state)
        .unwrap_or_else(|| unavailable("gemini", "unavailable", "no_quota_api"))
}
fn kind(minutes: Option<u64>, fallback: &str) -> String {
    match minutes {
        Some(300) => "five_hour".into(),
        Some(10080) => "weekly".into(),
        Some(n) if n % 1440 == 0 && n != 0 => format!("{}_days", n / 1440),
        Some(n) if n % 60 == 0 && n != 0 => format!("{}_hours", n / 60),
        Some(n) => format!("{n}_minutes"),
        None => fallback.to_string(),
    }
}
// Labels match the OpenAI Codex KnownPlan display_name mapping.
pub(crate) fn plan_label(provider: &str, raw: &str) -> Option<&'static str> {
    if provider != "codex" {
        return None;
    }
    match raw {
        "free" => Some("Free"),
        "go" => Some("Go"),
        "plus" => Some("Plus"),
        "prolite" => Some("Pro"),
        "pro" => Some("Pro (More)"),
        "promax" => Some("Pro (Max)"),
        "team" => Some("Team"),
        "self_serve_business_prolite" => Some("Self Serve Business ProLite"),
        "self_serve_business_usage_based" => Some("Self Serve Business Usage Based"),
        "business" => Some("Business"),
        "ent26" | "enterprise" => Some("Enterprise"),
        "enterprise_cbp_automation" => Some("Enterprise (Automation)"),
        "enterprise_cbp_usage_based" => Some("Enterprise CBP Usage Based"),
        "edu" => Some("Edu"),
        "edu_plus" => Some("Edu Plus"),
        "edu_pro" => Some("Edu Pro"),
        "sci" => Some("Academic Researchers"),
        "sci_plus" => Some("Academic Researchers Plus"),
        _ => None,
    }
}
fn account_plan(account: &serde_json::Value) -> Option<String> {
    account
        .get("planType")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}
fn unavailable_with_plan(reason: &'static str, plan: Option<String>) -> UsageStatus {
    let mut status = unavailable("codex", "unavailable", reason);
    status.plan_label = plan.as_deref().and_then(|p| plan_label("codex", p));
    status.plan = plan;
    status
}
fn parse_limits(value: &serde_json::Value, account: &serde_json::Value) -> UsageStatus {
    let buckets = value
        .get("rateLimitsByLimitId")
        .and_then(|v| v.as_object())
        .filter(|v| !v.is_empty());
    let fallback = value.get("rateLimits").filter(|v| v.is_object());
    let plan = buckets
        .and_then(|b| b.get("codex"))
        .or(fallback)
        .and_then(|v| v.get("planType"))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .or_else(|| account_plan(account));
    let mut windows = Vec::new();
    let mut add_bucket = |id: Option<&str>, limit: &serde_json::Value| {
        for name in ["primary", "secondary"] {
            if let Some(w) = limit.get(name) {
                if let Some(used_percent) = w.get("usedPercent").and_then(|v| v.as_f64()) {
                    windows.push(UsageWindow {
                        kind: kind(w.get("windowDurationMins").and_then(|v| v.as_u64()), name),
                        scope: None,
                        label: limit
                            .get("limitName")
                            .and_then(|v| v.as_str())
                            .map(str::to_owned),
                        limit_id: id.map(str::to_owned),
                        used_percent,
                        resets_at: w.get("resetsAt").and_then(|v| v.as_i64()),
                    });
                }
            }
        }
    };
    if let Some(buckets) = buckets {
        for (id, bucket) in buckets {
            add_bucket(Some(id), bucket);
        }
    } else if let Some(limit) = fallback {
        add_bucket(None, limit);
    }
    if windows.is_empty() {
        return unavailable_with_plan("fetch_failed", plan);
    }
    UsageStatus {
        provider: "codex",
        state: "ok",
        reason: None,
        windows,
        remaining: None,
        plan_label: plan.as_deref().and_then(|p| plan_label("codex", p)),
        plan,
        checked_at: now(),
    }
}
fn request(method: &'static str, params: serde_json::Value) -> process::JsonRpcRequest<'static> {
    process::JsonRpcRequest {
        initialize_params: serde_json::json!({"clientInfo":{"name":"momso","title":"momso","version":env!("CARGO_PKG_VERSION")}}),
        method,
        params,
    }
}
pub(crate) async fn fetch_codex() -> UsageStatus {
    let status = provider_auth::read_status("codex").await;
    if let Some(result) = auth_unavailable("codex", status.installed, status.state) {
        return result;
    }
    if process::resolve_interactive_tool("codex").is_none() {
        return unavailable("codex", "unavailable", "not_installed");
    }
    let account = match process::run_json_rpc_tool(
        "codex",
        &["app-server"],
        None,
        &process::empty_environment(),
        request("account/read", serde_json::json!({"refreshToken":false})),
        Duration::from_secs(12),
    )
    .await
    {
        Ok(value) => value,
        Err(_) => return unavailable("codex", "unavailable", "fetch_failed"),
    };
    let Some(account) = account.get("account").filter(|a| a.is_object()) else {
        return unavailable("codex", "unavailable", "not_connected");
    };
    if account.get("type").and_then(|v| v.as_str()) != Some("chatgpt") {
        return unavailable_with_plan(
            if account.get("type").and_then(|v| v.as_str()) == Some("apiKey") {
                "api_key"
            } else {
                "fetch_failed"
            },
            account_plan(account),
        );
    }
    match process::run_json_rpc_tool(
        "codex",
        &["app-server"],
        None,
        &process::empty_environment(),
        request("account/rateLimits/read", serde_json::json!({})),
        Duration::from_secs(12),
    )
    .await
    {
        Ok(value) => parse_limits(&value, account),
        Err(_) => unavailable_with_plan("fetch_failed", account_plan(account)),
    }
}
fn parse_claude_report(output: &str) -> UsageStatus {
    let mut windows = Vec::new();
    for line in output.lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let Some(limits) = message
            .get("usage_report")
            .and_then(|v| v.get("rate_limits"))
            .and_then(|v| v.get("limits"))
            .and_then(|v| v.as_array())
        else {
            continue;
        };
        for entry in limits {
            let (Some(kind), Some(used_percent)) = (
                entry
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty()),
                entry.get("percent").and_then(|v| v.as_f64()),
            ) else {
                continue;
            };
            let resets_at = entry
                .get("resets_at")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|time| time.timestamp());
            windows.push(UsageWindow {
                kind: kind.to_owned(),
                scope: entry.get("scope").cloned(),
                label: entry
                    .get("label")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                limit_id: entry
                    .get("group")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                used_percent,
                resets_at,
            });
        }
    }
    if windows.is_empty() {
        return unavailable("claude", "unavailable", "fetch_failed");
    }
    UsageStatus {
        provider: "claude",
        state: "ok",
        reason: None,
        windows,
        remaining: None,
        plan: None,
        plan_label: None,
        checked_at: now(),
    }
}

pub(crate) async fn fetch_claude(working_directory: &std::path::Path) -> UsageStatus {
    let status = provider_auth::read_status("claude").await;
    if let Some(result) = auth_unavailable("claude", status.installed, status.state) {
        return result;
    }
    if process::resolve_tool("claude").is_none() {
        return unavailable("claude", "unavailable", "not_installed");
    }
    let auth = match process::run_tool(
        "claude",
        &["auth", "status", "--json"],
        Some(working_directory),
        None,
        &process::empty_environment(),
        Duration::from_secs(20),
    )
    .await
    {
        Ok(output) => serde_json::from_str::<serde_json::Value>(&output.stdout).ok(),
        Err(_) => None,
    };
    match auth
        .as_ref()
        .and_then(|v| v.get("loggedIn"))
        .and_then(|v| v.as_bool())
    {
        Some(false) => {
            return unavailable("claude", "unavailable", "not_connected");
        }
        Some(true) => {}
        None => return unavailable("claude", "unavailable", "fetch_failed"),
    }
    let plan = auth
        .as_ref()
        .and_then(|v| v.get("subscriptionType"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("unknown"))
        .map(str::to_owned);
    // /usage is a built-in local command. Do not send an ordinary model prompt.
    let args = [
        "--safe-mode",
        "-p",
        "/usage",
        "--no-session-persistence",
        "--output-format",
        "stream-json",
        "--verbose",
        "--tools=",
    ];
    match process::run_tool(
        "claude",
        &args,
        Some(working_directory),
        None,
        &process::empty_environment(),
        Duration::from_secs(60),
    )
    .await
    {
        Ok(output) if output.success => {
            let mut status = parse_claude_report(&output.stdout);
            status.plan = plan;
            status
        }
        _ => {
            let mut status = unavailable("claude", "unavailable", "fetch_failed");
            status.plan = plan;
            status
        }
    }
}

#[cfg(test)]
pub(crate) async fn all(working_directory: &std::path::Path) -> Vec<UsageStatus> {
    let (codex, claude) = tokio::join!(fetch_codex(), fetch_claude(working_directory));
    vec![codex, claude, fetch_gemini().await]
}
// Cached results are returned immediately; each provider has its own refresh clock.
#[derive(Default)]
pub(crate) struct UsageCache {
    entries: std::sync::Mutex<std::collections::HashMap<&'static str, CacheEntry>>,
}
struct CacheEntry {
    status: UsageStatus,
    started: std::time::Instant,
    running: bool,
}
impl UsageCache {
    fn get_or_start(&self, provider: &'static str, force: bool) -> (UsageStatus, bool) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = entries.entry(provider).or_insert_with(|| CacheEntry {
            status: unavailable_without_reason(provider),
            started: std::time::Instant::now() - Duration::from_secs(301),
            running: false,
        });
        let start =
            !entry.running && (force || entry.started.elapsed() >= Duration::from_secs(300));
        if start {
            entry.running = true;
            entry.started = std::time::Instant::now();
        }
        (entry.status.clone(), start)
    }
    fn finish(&self, provider: &'static str, status: UsageStatus) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entries.get_mut(provider) {
            entry.status = status;
            entry.running = false;
        }
    }
}
pub(crate) fn cached(
    app: tauri::AppHandle,
    cache: Arc<UsageCache>,
    working_directory: Arc<tokio::sync::OwnedRwLockReadGuard<std::path::PathBuf>>,
    force_provider: Option<&str>,
) -> Vec<UsageStatus> {
    ["codex", "claude", "gemini"]
        .into_iter()
        .map(|provider| {
            let (status, start) = cache.get_or_start(provider, force_provider == Some(provider));
            if start {
                let cache = cache.clone();
                let app = app.clone();
                let working_directory = working_directory.clone();
                tauri::async_runtime::spawn(async move {
                    let result = match provider {
                        "codex" => fetch_codex().await,
                        "claude" => fetch_claude(&working_directory).await,
                        _ => {
                            log::debug!("Gemini quota CLI output is not verified");
                            fetch_gemini().await
                        }
                    };
                    if result.state == "unavailable" && provider == "claude" {
                        log::debug!("Claude usage output was unavailable or could not be parsed");
                    }
                    cache.finish(provider, result);
                    let _ = app.emit("usage-updated", serde_json::json!({"provider":provider}));
                });
            }
            status
        })
        .collect()
}
fn is_update(v: &serde_json::Value) -> bool {
    v.get("method").and_then(|v| v.as_str()) == Some("account/rateLimits/updated")
}

// One listener per app; a later usage_status call restarts it if the CLI exits.
pub(crate) fn start_monitor(app: tauri::AppHandle, running: Arc<AtomicBool>) {
    if running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    tauri::async_runtime::spawn(async move {
        monitor(&app).await;
        running.store(false, Ordering::Release);
    });
}
async fn monitor(app: &tauri::AppHandle) {
    let Some(executable) = process::resolve_interactive_tool("codex") else {
        return;
    };
    let mut command = crate::process::hidden_async_command(executable);
    command
        .arg("app-server")
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let Ok(mut child) = command.spawn() else {
        return;
    };
    let (Some(mut input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
        return;
    };
    let mut lines = BufReader::new(output).lines();
    let init = serde_json::json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"momso","title":"momso","version":env!("CARGO_PKG_VERSION")}}});
    if input
        .write_all(format!("{init}\n").as_bytes())
        .await
        .is_err()
    {
        return;
    }
    let initialized = tokio::time::timeout(Duration::from_secs(12), async {
        while let Ok(Some(line)) = lines.next_line().await {
            if line.len() > 1024 * 1024 {
                return false;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                if v.get("id").and_then(|v| v.as_u64()) == Some(1) {
                    return v.get("result").is_some();
                }
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    if !initialized {
        return;
    }
    let start="{\"method\":\"initialized\",\"params\":{}}\n{\"id\":2,\"method\":\"account/rateLimits/read\",\"params\":{}}\n";
    if input.write_all(start.as_bytes()).await.is_err() {
        return;
    }
    while let Ok(Some(line)) = lines.next_line().await {
        if line.len() > 1024 * 1024 {
            break;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            if is_update(&v) {
                let _ = app.emit("usage-updated", serde_json::json!({"provider":"codex"}));
            }
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_throttles_refresh_and_allows_explicit_refresh() {
        let cache = UsageCache::default();
        assert!(cache.get_or_start("claude", false).1);
        assert!(!cache.get_or_start("claude", true).1);
        cache.finish("claude", unavailable_without_reason("claude"));
        assert!(!cache.get_or_start("claude", false).1);
        assert!(cache.get_or_start("claude", true).1);
        assert!(cache.get_or_start("codex", false).1);
    }
    #[test]
    fn unavailable_providers_report_reason_codes() {
        for provider in ["codex", "claude", "gemini"] {
            let value = serde_json::to_value(unavailable_without_reason(provider)).unwrap();
            assert_eq!(value["provider"], provider);
            assert_eq!(value["state"], "unavailable");
            assert_eq!(value["reason"], "checking");
            assert_eq!(
                auth_unavailable(provider, false, "unavailable")
                    .unwrap()
                    .reason,
                Some("not_installed")
            );
            assert_eq!(
                auth_unavailable(provider, true, "signedOut")
                    .unwrap()
                    .reason,
                Some("not_connected")
            );
            assert_eq!(
                auth_unavailable(provider, true, "unavailable")
                    .unwrap()
                    .reason,
                Some("fetch_failed")
            );
        }
        assert_eq!(
            auth_unavailable("codex", true, "apiKey").unwrap().reason,
            Some("api_key")
        );
        assert_eq!(
            auth_unavailable("claude", true, "apiKey").unwrap().reason,
            Some("api_key")
        );
        assert_eq!(
            auth_unavailable("gemini", true, "apiKey").unwrap().reason,
            Some("no_quota_api")
        );
        assert!(auth_unavailable("codex", true, "account").is_none());
    }
    #[test]
    fn plan_labels_use_only_confirmed_values() {
        assert_eq!(plan_label("codex", "prolite"), Some("Pro"));
        assert_eq!(plan_label("codex", "pro"), Some("Pro (More)"));
        assert_eq!(plan_label("codex", "promax"), Some("Pro (Max)"));
        assert_eq!(plan_label("codex", "unknown"), None);
        assert_eq!(plan_label("codex", "future_plan"), None);
        assert_eq!(plan_label("claude", "max"), None);
        let status = parse_limits(
            &serde_json::json!({"rateLimits":{"primary":{"usedPercent":1},"planType":"promax"}}),
            &serde_json::json!({}),
        );
        let value = serde_json::to_value(status).unwrap();
        assert_eq!(value["plan"], "promax");
        assert_eq!(value["planLabel"], "Pro (Max)");
    }
    #[test]
    fn reads_exact_fields_and_plan() {
        let v = serde_json::json!({"rateLimits":{"primary":{"usedPercent":12.5,"windowDurationMins":300,"resetsAt":123},"secondary":{"usedPercent":24,"windowDurationMins":10080,"resetsAt":456}}});
        let s = parse_limits(&v, &serde_json::json!({"planType":"pro"}));
        assert_eq!(s.state, "ok");
        assert_eq!(s.plan.as_deref(), Some("pro"));
        assert_eq!(s.windows[0].kind, "five_hour");
        assert_eq!(s.windows[0].used_percent, 12.5);
        assert_eq!(s.windows[0].resets_at, Some(123));
        assert_eq!(s.windows[1].kind, "weekly");
    }
    #[test]
    fn no_invented_values() {
        let s = parse_limits(
            &serde_json::json!({"rateLimits":{"primary":{"windowDurationMins":300}}}),
            &serde_json::json!({}),
        );
        assert_eq!(s.state, "unavailable");
        assert!(s.windows.is_empty());
        let missing = parse_limits(
            &serde_json::json!({}),
            &serde_json::json!({"planType":"pro"}),
        );
        assert_eq!(missing.plan.as_deref(), Some("pro"));
        let no_windows = parse_limits(
            &serde_json::json!({"rateLimits":{"planType":"plus"}}),
            &serde_json::json!({}),
        );
        assert_eq!(no_windows.plan.as_deref(), Some("plus"));
        let s = parse_limits(
            &serde_json::json!({"rateLimits":{"primary":{"usedPercent":125}}}),
            &serde_json::json!({}),
        );
        assert_eq!(s.windows[0].used_percent, 125.0);
        assert_eq!(s.windows[0].kind, "primary");
        assert_eq!(s.windows[0].resets_at, None);
        assert_eq!(s.plan, None);
    }
    #[test]
    fn selects_codex_bucket_and_unusual_duration() {
        let s = parse_limits(
            &serde_json::json!({"rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":7,"windowDurationMins":15}}},"rateLimits":{"primary":{"usedPercent":90}}}),
            &serde_json::json!({}),
        );
        assert_eq!(s.windows[0].used_percent, 7.0);
        assert_eq!(s.windows[0].kind, "15_minutes");
    }
    #[test]
    fn preserves_all_codex_buckets() {
        let s = parse_limits(
            &serde_json::json!({"rateLimitsByLimitId": {
                "codex": {"limitName":"Codex", "primary":{"usedPercent":1,"windowDurationMins":300}},
                "other": {"limitName":"Other", "secondary":{"usedPercent":2,"windowDurationMins":10080}}
            }}),
            &serde_json::json!({}),
        );
        assert_eq!(s.windows.len(), 2);
        assert!(
            s.windows
                .iter()
                .any(|w| w.limit_id.as_deref() == Some("other")
                    && w.label.as_deref() == Some("Other"))
        );
    }
    #[test]
    fn recognizes_official_notification() {
        assert!(is_update(
            &serde_json::json!({"method":"account/rateLimits/updated","params":{}})
        ));
        assert!(!is_update(&serde_json::json!({"method":"account/updated"})));
    }
    #[test]
    fn claude_structured_usage_preserves_windows_and_reset_times() {
        let output = serde_json::json!({"type":"assistant","usage_report":{"rate_limits":{"limits":[{"kind":"weekly_all","percent":12.5,"resets_at":"2026-11-15T12:00:00Z"},{"kind":"weekly_scoped","percent":31,"resets_at":null}]}}}).to_string();
        let status = parse_claude_report(&output);
        assert_eq!(status.state, "ok");
        assert_eq!(status.windows.len(), 2);
        assert_eq!(status.windows[0].kind, "weekly_all");
        assert_eq!(status.windows[0].used_percent, 12.5);
        assert_eq!(status.windows[0].resets_at, Some(1794744000));
        assert_eq!(status.windows[1].kind, "weekly_scoped");
        assert_eq!(status.windows[1].resets_at, None);
        assert!(status.plan.is_none());
    }
    #[test]
    fn claude_missing_report_does_not_invent_usage() {
        for text in [
            "{}",
            "not json",
            "{\"type\":\"assistant\",\"usage_report\":{\"rate_limits\":null}}",
        ] {
            let status = parse_claude_report(text);
            assert_eq!(status.state, "unavailable");
            assert!(status.windows.is_empty());
            assert_eq!(status.reason, Some("fetch_failed"));
        }
    }
    #[tokio::test]
    #[ignore = "실제 Claude 로그인 상태에서만 실행하는 확인"]
    async fn live_claude_usage() {
        let temp = tempfile::tempdir().unwrap();
        let status = fetch_claude(temp.path()).await;
        println!(
            "LIVE_CLAUDE_STATE={} WINDOWS={:?}",
            status.state,
            status
                .windows
                .iter()
                .map(|w| (&w.kind, &w.scope, &w.label))
                .collect::<Vec<_>>()
        );
        assert_eq!(status.state, "ok");
        assert!(!status.windows.is_empty());
    }

    #[tokio::test]
    #[ignore = "실제 Codex 로그인 상태에서만 실행하는 확인"]
    async fn live_codex_usage() {
        let temp = tempfile::tempdir().unwrap();
        let s = all(temp.path()).await;
        println!(
            "LIVE_CODEX_STATE={} PLAN={:?} PLAN_LABEL={:?}",
            s[0].state, s[0].plan, s[0].plan_label
        );
        println!(
            "LIVE_CODEX_WINDOWS={:?}",
            s[0].windows
                .iter()
                .map(|w| (&w.kind, &w.scope, &w.label, &w.limit_id))
                .collect::<Vec<_>>()
        );
        println!(
            "LIVE_CLAUDE_WINDOWS={:?}",
            s[1].windows
                .iter()
                .map(|w| (&w.kind, &w.scope, &w.label))
                .collect::<Vec<_>>()
        );
        assert_eq!(s[0].state, "ok");
        assert_eq!(s[1].provider, "claude");
        assert_eq!(s[2].state, "unavailable");
    }
}
