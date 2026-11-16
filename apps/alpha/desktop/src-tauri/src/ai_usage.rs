use crate::{
    ai_runtime::{AiAuthMode, AiProvider},
    process, storage,
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};
use thiserror::Error;
use uuid::Uuid;

const CODEX_API_MODEL: &str = "gpt-5.4";
const CODEX_PRICING_VERSION: &str = "openai-gpt-5.4-2026-03-05";

#[derive(Debug, Error)]
pub(crate) enum AiUsageError {
    #[error(transparent)]
    Storage(#[from] storage::StorageError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("AI 사용량 값이 지원 범위를 벗어났습니다")]
    ValueOutOfRange,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AiTokenUsage {
    pub(crate) input_tokens: u64,
    pub(crate) cached_input_tokens: u64,
    pub(crate) cache_creation_input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) reasoning_output_tokens: u64,
}

impl AiTokenUsage {
    pub(crate) fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AiCostSource {
    ProviderEstimate,
    LocalPriceTable,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AiUsageSummary {
    pub(crate) provider: AiProvider,
    pub(crate) auth_mode: AiAuthMode,
    pub(crate) model: Option<String>,
    pub(crate) input_tokens: u64,
    pub(crate) cached_input_tokens: u64,
    pub(crate) cache_creation_input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) reasoning_output_tokens: u64,
    pub(crate) total_tokens: u64,
    pub(crate) estimated_cost_usd_micros: Option<u64>,
    pub(crate) cost_source: Option<AiCostSource>,
}

#[derive(Clone, Debug)]
pub(crate) struct AiUsageEvent {
    pub(crate) request_id: String,
    pub(crate) provider: AiProvider,
    pub(crate) auth_mode: AiAuthMode,
    pub(crate) model: Option<String>,
    pub(crate) tokens: AiTokenUsage,
    pub(crate) estimated_cost_usd_micros: Option<u64>,
    pub(crate) cost_source: Option<AiCostSource>,
    pub(crate) pricing_version: Option<String>,
}

impl AiUsageEvent {
    pub(crate) fn summary(&self) -> AiUsageSummary {
        AiUsageSummary {
            provider: self.provider,
            auth_mode: self.auth_mode,
            model: self.model.clone(),
            input_tokens: self.tokens.input_tokens,
            cached_input_tokens: self.tokens.cached_input_tokens,
            cache_creation_input_tokens: self.tokens.cache_creation_input_tokens,
            output_tokens: self.tokens.output_tokens,
            reasoning_output_tokens: self.tokens.reasoning_output_tokens,
            total_tokens: self.tokens.total_tokens(),
            estimated_cost_usd_micros: self.estimated_cost_usd_micros,
            cost_source: self.cost_source,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AiQuotaStatus {
    Available,
    NotApplicable,
    Unavailable,
    Restricted,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AiQuotaWindow {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) used_percent: f64,
    pub(crate) window_duration_mins: Option<u64>,
    pub(crate) resets_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AiQuotaOverview {
    pub(crate) status: AiQuotaStatus,
    pub(crate) plan_type: Option<String>,
    pub(crate) windows: Vec<AiQuotaWindow>,
    pub(crate) checked_at: Option<u64>,
    pub(crate) detail: Option<String>,
}

impl AiQuotaOverview {
    pub(crate) fn not_applicable() -> Self {
        Self {
            status: AiQuotaStatus::NotApplicable,
            plan_type: None,
            windows: Vec::new(),
            checked_at: None,
            detail: None,
        }
    }

    pub(crate) fn restricted() -> Self {
        Self {
            status: AiQuotaStatus::Restricted,
            plan_type: None,
            windows: Vec::new(),
            checked_at: None,
            detail: None,
        }
    }

    fn unavailable(detail: impl Into<String>) -> Self {
        Self {
            status: AiQuotaStatus::Unavailable,
            plan_type: None,
            windows: Vec::new(),
            checked_at: Some(unix_timestamp()),
            detail: Some(detail.into()),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AiUsageOverview {
    pub(crate) provider: AiProvider,
    pub(crate) auth_mode: AiAuthMode,
    pub(crate) last_request: Option<AiUsageSummary>,
    pub(crate) today_estimated_cost_usd_micros: u64,
    pub(crate) month_estimated_cost_usd_micros: u64,
    pub(crate) today_total_tokens: u64,
    pub(crate) month_total_tokens: u64,
    pub(crate) month_request_count: u64,
    pub(crate) quota: AiQuotaOverview,
}

pub(crate) fn record(
    database_path: &Path,
    event: &AiUsageEvent,
) -> Result<AiUsageSummary, AiUsageError> {
    let connection = storage::open_writable_connection(database_path)?;
    connection.execute(
        "INSERT OR IGNORE INTO ai_usage_events (
           id, request_id, provider, auth_mode, model,
           input_tokens, cached_input_tokens, cache_creation_input_tokens,
           output_tokens, reasoning_output_tokens,
           estimated_cost_usd_micros, cost_source, pricing_version
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        (
            Uuid::new_v4().to_string(),
            &event.request_id,
            provider_name(event.provider),
            auth_mode_name(event.auth_mode),
            event.model.as_deref(),
            to_i64(event.tokens.input_tokens)?,
            to_i64(event.tokens.cached_input_tokens)?,
            to_i64(event.tokens.cache_creation_input_tokens)?,
            to_i64(event.tokens.output_tokens)?,
            to_i64(event.tokens.reasoning_output_tokens)?,
            event.estimated_cost_usd_micros.map(to_i64).transpose()?,
            event.cost_source.map(cost_source_name),
            event.pricing_version.as_deref(),
        ),
    )?;
    Ok(event.summary())
}

pub(crate) fn local_overview(
    database_path: &Path,
    provider: AiProvider,
    auth_mode: AiAuthMode,
) -> Result<AiUsageOverview, AiUsageError> {
    let connection = storage::open_writable_connection(database_path)?;
    let provider = provider_name(provider);
    let auth_mode = auth_mode_name(auth_mode);
    let last_request = connection
        .query_row(
            "SELECT model, input_tokens, cached_input_tokens,
                    cache_creation_input_tokens, output_tokens,
                    reasoning_output_tokens, estimated_cost_usd_micros, cost_source
             FROM ai_usage_events
             WHERE provider = ?1 AND auth_mode = ?2
             ORDER BY occurred_at DESC, rowid DESC
             LIMIT 1",
            (provider, auth_mode),
            |row| {
                let input_tokens = from_i64(row.get(1)?)?;
                let output_tokens = from_i64(row.get(4)?)?;
                Ok(AiUsageSummary {
                    provider: parse_provider(provider),
                    auth_mode: parse_auth_mode(auth_mode),
                    model: row.get(0)?,
                    input_tokens,
                    cached_input_tokens: from_i64(row.get(2)?)?,
                    cache_creation_input_tokens: from_i64(row.get(3)?)?,
                    output_tokens,
                    reasoning_output_tokens: from_i64(row.get(5)?)?,
                    total_tokens: input_tokens.saturating_add(output_tokens),
                    estimated_cost_usd_micros: row
                        .get::<_, Option<i64>>(6)?
                        .map(from_i64)
                        .transpose()?,
                    cost_source: row
                        .get::<_, Option<String>>(7)?
                        .as_deref()
                        .and_then(parse_cost_source),
                })
            },
        )
        .optional()?;

    let aggregate = connection.query_row(
        "SELECT
           COALESCE(SUM(CASE
             WHEN date(occurred_at, 'localtime') = date('now', 'localtime')
             THEN estimated_cost_usd_micros ELSE 0 END), 0),
           COALESCE(SUM(CASE
             WHEN strftime('%Y-%m', occurred_at, 'localtime') = strftime('%Y-%m', 'now', 'localtime')
             THEN estimated_cost_usd_micros ELSE 0 END), 0),
           COALESCE(SUM(CASE
             WHEN date(occurred_at, 'localtime') = date('now', 'localtime')
             THEN input_tokens + output_tokens ELSE 0 END), 0),
           COALESCE(SUM(CASE
             WHEN strftime('%Y-%m', occurred_at, 'localtime') = strftime('%Y-%m', 'now', 'localtime')
             THEN input_tokens + output_tokens ELSE 0 END), 0),
           COALESCE(SUM(CASE
             WHEN strftime('%Y-%m', occurred_at, 'localtime') = strftime('%Y-%m', 'now', 'localtime')
             THEN 1 ELSE 0 END), 0)
         FROM ai_usage_events
         WHERE provider = ?1 AND auth_mode = ?2",
        (provider, auth_mode),
        |row| {
            Ok((
                from_i64(row.get(0)?)?,
                from_i64(row.get(1)?)?,
                from_i64(row.get(2)?)?,
                from_i64(row.get(3)?)?,
                from_i64(row.get(4)?)?,
            ))
        },
    )?;

    Ok(AiUsageOverview {
        provider: parse_provider(provider),
        auth_mode: parse_auth_mode(auth_mode),
        last_request,
        today_estimated_cost_usd_micros: aggregate.0,
        month_estimated_cost_usd_micros: aggregate.1,
        today_total_tokens: aggregate.2,
        month_total_tokens: aggregate.3,
        month_request_count: aggregate.4,
        quota: AiQuotaOverview::not_applicable(),
    })
}

pub(crate) async fn fetch_codex_quota() -> AiQuotaOverview {
    let result = process::run_json_rpc_tool(
        "codex",
        &["app-server"],
        None,
        &process::empty_environment(),
        process::JsonRpcRequest {
            initialize_params: serde_json::json!({
                "clientInfo": {
                    "name": "momso",
                    "title": "momso",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": { "experimentalApi": true }
            }),
            method: "account/rateLimits/read",
            params: serde_json::json!({}),
        },
        Duration::from_secs(12),
    )
    .await;

    match result {
        Ok(result) => parse_codex_quota(&result),
        Err(error) => {
            log::warn!("Codex rate-limit lookup failed: {error}");
            AiQuotaOverview::unavailable(
                "Codex 구독 한도를 불러오지 못했습니다. Codex 로그인을 확인한 뒤 다시 시도해 주세요.",
            )
        }
    }
}

fn parse_codex_quota(result: &serde_json::Value) -> AiQuotaOverview {
    let Some(rate_limits) = result
        .get("rateLimits")
        .and_then(serde_json::Value::as_object)
    else {
        return AiQuotaOverview::unavailable("Codex가 현재 구독 한도 정보를 제공하지 않았습니다.");
    };
    let mut windows = Vec::new();
    for (id, label) in [("primary", "단기 한도"), ("secondary", "장기 한도")] {
        let Some(window) = rate_limits.get(id).and_then(serde_json::Value::as_object) else {
            continue;
        };
        let Some(used_percent) = window
            .get("usedPercent")
            .and_then(serde_json::Value::as_f64)
        else {
            continue;
        };
        windows.push(AiQuotaWindow {
            id: id.to_owned(),
            label: label.to_owned(),
            used_percent: used_percent.clamp(0.0, 100.0),
            window_duration_mins: window
                .get("windowDurationMins")
                .and_then(serde_json::Value::as_u64),
            resets_at: window.get("resetsAt").and_then(serde_json::Value::as_u64),
        });
    }

    if windows.is_empty() {
        return AiQuotaOverview::unavailable("Codex가 현재 구독 한도 정보를 제공하지 않았습니다.");
    }
    AiQuotaOverview {
        status: AiQuotaStatus::Available,
        plan_type: rate_limits
            .get("planType")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        windows,
        checked_at: Some(unix_timestamp()),
        detail: None,
    }
}

pub(crate) fn codex_api_usage_event(request_id: String, tokens: AiTokenUsage) -> AiUsageEvent {
    AiUsageEvent {
        request_id,
        provider: AiProvider::Codex,
        auth_mode: AiAuthMode::ApiKey,
        model: Some(CODEX_API_MODEL.to_owned()),
        estimated_cost_usd_micros: Some(codex_api_estimated_cost(&tokens)),
        cost_source: Some(AiCostSource::LocalPriceTable),
        pricing_version: Some(CODEX_PRICING_VERSION.to_owned()),
        tokens,
    }
}

pub(crate) fn subscription_usage_event(
    request_id: String,
    provider: AiProvider,
    model: Option<String>,
    tokens: AiTokenUsage,
) -> AiUsageEvent {
    AiUsageEvent {
        request_id,
        provider,
        auth_mode: AiAuthMode::Subscription,
        model,
        tokens,
        estimated_cost_usd_micros: None,
        cost_source: None,
        pricing_version: None,
    }
}

pub(crate) fn claude_api_usage_event(
    request_id: String,
    model: Option<String>,
    tokens: AiTokenUsage,
    estimated_cost_usd: Option<f64>,
) -> AiUsageEvent {
    AiUsageEvent {
        request_id,
        provider: AiProvider::Claude,
        auth_mode: AiAuthMode::ApiKey,
        model,
        tokens,
        estimated_cost_usd_micros: estimated_cost_usd.and_then(usd_to_micros),
        cost_source: estimated_cost_usd.map(|_| AiCostSource::ProviderEstimate),
        pricing_version: None,
    }
}

fn codex_api_estimated_cost(tokens: &AiTokenUsage) -> u64 {
    let uncached_input = tokens
        .input_tokens
        .saturating_sub(tokens.cached_input_tokens);
    let hundredths_of_micros = uncached_input
        .saturating_mul(250)
        .saturating_add(tokens.cached_input_tokens.saturating_mul(25))
        .saturating_add(tokens.output_tokens.saturating_mul(1_500));
    hundredths_of_micros.saturating_add(50) / 100
}

fn usd_to_micros(value: f64) -> Option<u64> {
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    Some((value * 1_000_000.0).round().min(u64::MAX as f64) as u64)
}

fn provider_name(provider: AiProvider) -> &'static str {
    match provider {
        AiProvider::Codex => "codex",
        AiProvider::Claude => "claude",
    }
}

fn auth_mode_name(auth_mode: AiAuthMode) -> &'static str {
    match auth_mode {
        AiAuthMode::Subscription => "subscription",
        AiAuthMode::ApiKey => "api_key",
    }
}

fn cost_source_name(source: AiCostSource) -> &'static str {
    match source {
        AiCostSource::ProviderEstimate => "provider_estimate",
        AiCostSource::LocalPriceTable => "local_price_table",
    }
}

fn parse_provider(value: &str) -> AiProvider {
    match value {
        "claude" => AiProvider::Claude,
        _ => AiProvider::Codex,
    }
}

fn parse_auth_mode(value: &str) -> AiAuthMode {
    match value {
        "api_key" => AiAuthMode::ApiKey,
        _ => AiAuthMode::Subscription,
    }
}

fn parse_cost_source(value: &str) -> Option<AiCostSource> {
    match value {
        "provider_estimate" => Some(AiCostSource::ProviderEstimate),
        "local_price_table" => Some(AiCostSource::LocalPriceTable),
        _ => None,
    }
}

fn to_i64(value: u64) -> Result<i64, AiUsageError> {
    i64::try_from(value).map_err(|_| AiUsageError::ValueOutOfRange)
}

fn from_i64(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn database() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempdir().expect("tempdir");
        let database_path = temp.path().join("momso.db");
        storage::initialize(&database_path, &temp.path().join("backups"), "test")
            .expect("storage initializes");
        (temp, database_path)
    }

    #[test]
    fn records_only_usage_metadata_and_ignores_duplicate_request_ids() {
        let (_temp, database_path) = database();
        let event = codex_api_usage_event(
            Uuid::new_v4().to_string(),
            AiTokenUsage {
                input_tokens: 2_000,
                cached_input_tokens: 1_000,
                output_tokens: 500,
                ..AiTokenUsage::default()
            },
        );
        record(&database_path, &event).expect("usage records");
        record(&database_path, &event).expect("duplicate is idempotent");

        let overview = local_overview(&database_path, AiProvider::Codex, AiAuthMode::ApiKey)
            .expect("overview loads");
        assert_eq!(overview.month_request_count, 1);
        assert_eq!(overview.month_total_tokens, 2_500);
        assert_eq!(
            overview
                .last_request
                .expect("last request")
                .model
                .as_deref(),
            Some(CODEX_API_MODEL)
        );
    }

    #[test]
    fn codex_api_cost_uses_uncached_cached_and_output_rates() {
        let cost = codex_api_estimated_cost(&AiTokenUsage {
            input_tokens: 2_000_000,
            cached_input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..AiTokenUsage::default()
        });
        assert_eq!(cost, 17_750_000);
    }

    #[test]
    fn restricted_quota_has_no_explanatory_text() {
        let value = serde_json::to_value(AiQuotaOverview::restricted()).unwrap();
        assert_eq!(value["status"], "restricted");
        assert!(value.get("detail").unwrap().is_null());
    }

    #[test]
    fn parses_codex_primary_and_secondary_rate_limit_windows() {
        let quota = parse_codex_quota(&serde_json::json!({
            "rateLimits": {
                "planType": "pro",
                "primary": {
                    "usedPercent": 34.5,
                    "windowDurationMins": 300,
                    "resetsAt": 1_800_000_000
                },
                "secondary": {
                    "usedPercent": 62,
                    "windowDurationMins": 10080,
                    "resetsAt": 1_800_500_000
                }
            }
        }));

        assert_eq!(quota.status, AiQuotaStatus::Available);
        assert_eq!(quota.plan_type.as_deref(), Some("pro"));
        assert_eq!(quota.windows.len(), 2);
        assert_eq!(quota.windows[0].used_percent, 34.5);
    }
}
