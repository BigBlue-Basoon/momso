use crate::{gemini_access, process};
use serde::Serialize;
use serde_json::{json, Value};
use std::{path::Path, time::Duration};

#[derive(Debug, Serialize)]
struct Effort {
    id: String,
    kind: &'static str,
    label: String,
    description: Option<String>,
    special: bool,
    available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    recommended: Option<bool>,
}
#[derive(Debug, Serialize)]
struct Fast {
    supported: Option<bool>,
    state: &'static str,
    label: &'static str,
}
#[derive(Debug, Serialize)]
struct Toggle {
    id: String,
    kind: &'static str,
    label: String,
    description: Option<String>,
    available: bool,
    state: Option<bool>,
}
fn effort(id: &str, description: Option<String>, recommended: Option<bool>) -> Effort {
    Effort {
        id: id.to_owned(),
        kind: "level",
        label: id.to_owned(),
        description,
        special: matches!(id, "xhigh" | "max" | "ultra"),
        available: true,
        recommended,
    }
}
fn claude_effort(id: &str) -> Effort {
    let mut value = effort(id, None, None);
    value.label = match id {
        "low" => "낮음",
        "medium" => "중간",
        "high" => "높음",
        "xhigh" => "엑스트라",
        "max" => "최대",
        _ => id,
    }
    .to_owned();
    value
}
// Support means documented model capability, not proof of account eligibility.
// No supported CLI query has verified the current setting for this session.
fn fast(provider: &str, model: &str) -> Fast {
    let lower = model.to_ascii_lowercase();
    let supported = match provider {
        "claude" => matches!(
            lower.as_str(),
            "claude-fable-5-2" | "claude-opus-5-5" | "claude-opus-5" | "claude-opus-4-8"
        ),
        "codex" => matches!(
            lower.as_str(),
            "gpt-6.1-sol" | "gpt-6.1-terra" | "gpt-6-astra" | "gpt-6-sol" | "gpt-6-luna"
        ),
        _ => false,
    };
    Fast {
        supported: if provider == "gemini" {
            None
        } else {
            Some(supported)
        },
        state: "unavailable",
        label: if provider == "claude" {
            "고속 모드"
        } else {
            "Fast mode"
        },
    }
}
fn sort_efforts(efforts: &mut [Effort]) {
    efforts.sort_by_key(|e| match e.id.as_str() {
        "none" => 0,
        "minimal" => 1,
        "low" => 2,
        "medium" => 3,
        "high" => 4,
        "xhigh" => 5,
        "max" => 6,
        "ultra" => 7,
        _ => 8,
    });
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Model {
    id: String,
    family: Option<String>,
    label: String,
    resolved: Option<String>,
    efforts: Vec<Effort>,
    effort_note: Option<String>,
    toggles: Vec<Toggle>,
    fast: Fast,
    default_effort: Option<String>,
    effective_default_effort: Option<String>,
    is_default: Option<bool>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderModels {
    provider: &'static str,
    state: &'static str,
    reason: Option<String>,
    models: Vec<Model>,
}
fn empty(provider: &'static str, state: &'static str) -> ProviderModels {
    ProviderModels {
        provider,
        state,
        reason: None,
        models: Vec::new(),
    }
}
fn rpc(method: &'static str, params: Value) -> process::JsonRpcRequest<'static> {
    process::JsonRpcRequest {
        initialize_params: json!({"clientInfo":{"name":"momso","title":"momso","version":env!("CARGO_PKG_VERSION")}}),
        method,
        params,
    }
}
fn codex_model(v: &Value) -> Option<Model> {
    let id = v.get("id")?.as_str()?.to_owned();
    let label = v.get("displayName")?.as_str()?.to_owned();
    let resolved = v
        .get("model")
        .and_then(Value::as_str)
        .filter(|v| *v != id)
        .map(str::to_owned);
    let default_effort = v.get("defaultReasoningEffort").and_then(Value::as_str);
    let mut efforts: Vec<Effort> = v
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    x.get("reasoningEffort")
                        .and_then(Value::as_str)
                        .filter(|id| {
                            !id.eq_ignore_ascii_case("ultracode")
                                && !id.eq_ignore_ascii_case("ultra")
                        })
                        .map(|id| {
                            effort(
                                id,
                                x.get("description")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                                if Some(id) == default_effort {
                                    Some(true)
                                } else {
                                    x.get("recommended").and_then(Value::as_bool)
                                },
                            )
                        })
                })
                .collect()
        })
        .unwrap_or_default();
    sort_efforts(&mut efforts);
    Some(Model {
        fast: fast(
            "codex",
            v.get("model").and_then(Value::as_str).unwrap_or(&id),
        ),
        effort_note: None,
        toggles: vec![],
        id,
        family: None,
        label,
        resolved,
        efforts,
        default_effort: default_effort.map(str::to_owned),
        effective_default_effort: None,
        is_default: v.get("isDefault").and_then(Value::as_bool),
    })
}
// Keep the first family position while selecting only versions returned by the CLI.
fn latest_by_family(models: Vec<Model>) -> Vec<Model> {
    fn identity(m: &Model) -> Option<(String, Vec<u64>, bool)> {
        let text = format!(
            "{} {} {}",
            m.label,
            m.id,
            m.resolved.as_deref().unwrap_or("")
        );
        let lower = text.to_ascii_lowercase();
        let family = [
            "fable", "opus", "sonnet", "haiku", "terra", "sol", "luna", "astra",
        ]
        .into_iter()
        .find(|f| {
            lower
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|w| w == *f)
        })?;
        // Parse the display version first; dates in identifiers never become version components.
        let mut version = m
            .label
            .split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .find(|s| s.chars().next().is_some_and(|c| c.is_ascii_digit()))?
            .split('.')
            .filter_map(|n| n.parse().ok())
            .collect::<Vec<u64>>();
        if version.is_empty() {
            return None;
        }
        while version.len() > 1 && version.last() == Some(&0) {
            version.pop();
        }
        Some((family.to_owned(), version, lower.contains("preview")))
    }
    fn date(m: &Model) -> Option<u32> {
        m.resolved
            .as_deref()
            .unwrap_or(&m.id)
            .split(|c: char| !c.is_ascii_digit())
            .find(|part| part.len() == 8 && part.starts_with("20"))
            .and_then(|part| part.parse().ok())
    }
    // A generic alias may resolve to a concrete entry whose label gives its family and version.
    let labels: std::collections::HashMap<String, String> = models
        .iter()
        .filter(|m| m.label.chars().any(|c| c.is_ascii_digit()))
        .filter_map(|m| m.resolved.as_ref().map(|r| (r.clone(), m.label.clone())))
        .collect();
    let mut selected: Vec<Model> = Vec::new();
    for mut model in models {
        if !model.label.chars().any(|c| c.is_ascii_digit()) {
            if let Some(label) = model.resolved.as_ref().and_then(|r| labels.get(r)) {
                model.label = label.clone();
            }
        }
        let Some((family, version, preview)) = identity(&model) else {
            selected.push(model);
            continue;
        };
        model.family = Some(family.clone());
        if let Some(index) = selected
            .iter()
            .position(|m| m.family.as_deref() == Some(&family))
        {
            let (_, old_version, old_preview) = identity(&selected[index]).unwrap();
            let newer = (!preview && old_preview)
                || (preview == old_preview
                    && (version > old_version
                        || (version == old_version && date(&model) > date(&selected[index]))));
            // Equal versions: prefer an alias only when both resolve to the same actual model.
            let same_alias = version == old_version
                && preview == old_preview
                && model.resolved.is_some()
                && model.resolved == selected[index].resolved
                && model.id != "default"
                && !model.id.contains(|c: char| c.is_ascii_digit());
            if newer || same_alias {
                selected[index] = model;
            }
        } else {
            selected.push(model);
        }
    }
    selected
}
async fn codex(cwd: &Path) -> ProviderModels {
    if process::resolve_interactive_tool("codex").is_none() {
        return empty("codex", "not_connected");
    }
    let env = process::empty_environment();
    let account = process::run_json_rpc_tool(
        "codex",
        &["app-server"],
        None,
        &env,
        rpc("account/read", json!({"refreshToken":false})),
        Duration::from_secs(15),
    )
    .await;
    match account {
        Ok(v) if v.get("account").is_some_and(Value::is_object) => {}
        Ok(_) => return empty("codex", "not_connected"),
        Err(_) => return empty("codex", "unavailable"),
    }
    let effective_config = process::run_json_rpc_tool(
        "codex",
        &["app-server"],
        Some(cwd),
        &env,
        rpc("config/read", json!({"includeLayers":false,"cwd":cwd})),
        Duration::from_secs(15),
    )
    .await
    .ok();
    let mut models = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..50 {
        let response = process::run_json_rpc_tool(
            "codex",
            &["app-server"],
            None,
            &env,
            rpc(
                "model/list",
                json!({"limit":100,"includeHidden":false,"cursor":cursor}),
            ),
            Duration::from_secs(15),
        )
        .await;
        let Ok(response) = response else {
            return empty("codex", "unavailable");
        };
        let Some(data) = response.get("data").and_then(Value::as_array) else {
            return empty("codex", "unavailable");
        };
        for m in data {
            if let Some(model) = codex_model(m) {
                models.push(model);
            }
        }
        match response.get("nextCursor").and_then(Value::as_str) {
            Some(next) if Some(next) != cursor.as_deref() => cursor = Some(next.to_owned()),
            Some(_) => return empty("codex", "unavailable"),
            None => {
                apply_codex_effective_config(&mut models, effective_config.as_ref());
                return ProviderModels {
                    provider: "codex",
                    state: "ok",
                    reason: None,
                    models: latest_by_family(models),
                };
            }
        }
    }
    empty("codex", "unavailable")
}
fn apply_codex_effective_config(models: &mut [Model], config: Option<&Value>) {
    let configured = config
        .and_then(|v| v.pointer("/config/model_reasoning_effort"))
        .and_then(Value::as_str);
    for model in models {
        model.effective_default_effort = configured
            .filter(|level| model.efforts.iter().any(|effort| effort.id == *level))
            .map(str::to_owned);
    }
}
fn claude_models(output: &str) -> Vec<Model> {
    for line in output.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) != Some("control_response")
            || v.pointer("/response/subtype").and_then(Value::as_str) != Some("success")
        {
            continue;
        }
        let Some(models) = v
            .pointer("/response/response/models")
            .and_then(Value::as_array)
        else {
            continue;
        };
        return models
            .iter()
            .filter_map(|m| {
                let id = m.get("value")?.as_str()?.to_owned();
                let label = m.get("displayName")?.as_str()?.to_owned();
                let family = label
                    .split_whitespace()
                    .next()
                    .filter(|f| matches!(*f, "Fable" | "Opus" | "Sonnet" | "Haiku"))
                    .map(str::to_owned);
                let mut efforts: Vec<Effort> = m
                    .get("supportedEffortLevels")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .filter(|id| !id.eq_ignore_ascii_case("ultracode"))
                            .map(claude_effort)
                            .collect()
                    })
                    .unwrap_or_default();
                sort_efforts(&mut efforts);
                Some(Model {
                    effort_note: Some(
                        "더 높은 노력은 더 철저한 응답을 의미하지만, 시간이 더 오래 걸리고 한도를 더 빨리 사용합니다.".into(),
                    ),
                    fast: fast("claude", m.get("resolvedModel").and_then(Value::as_str).unwrap_or(&id)),
                    // The CLI does not prove model and account eligibility. Do not
                    // advertise a toggle that may silently fall back.
                    toggles: vec![],
                    resolved: m
                        .get("resolvedModel")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    is_default: if id == "default" { Some(true) } else { None },
                    id,
                    family,
                    label,
                    efforts,
                    default_effort: None,
                    effective_default_effort: None,
                })
            })
            .collect();
    }
    Vec::new()
}

fn status_effort(output: &str, supports_auto_current: bool) -> Option<String> {
    let value: Value = serde_json::from_str(output).ok()?;
    if value.get("is_error").and_then(Value::as_bool) != Some(false)
        || value.get("local_command").and_then(Value::as_str) != Some("effort")
    {
        return None;
    }
    let text = value.get("result")?.as_str()?;
    let level = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("Effort level: "))?;
    let level = if supports_auto_current {
        level
            .strip_prefix("auto (currently ")
            .and_then(|v| v.strip_suffix(')'))
            .unwrap_or(level)
    } else {
        level
    };
    ["low", "medium", "high", "xhigh", "max"]
        .contains(&level)
        .then(|| level.to_owned())
}

fn claude_auto_current_is_reliable(output: &str) -> bool {
    output
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|value| {
            let Some(version) = value
                .pointer("/response/response/claude_code_version")
                .and_then(Value::as_str)
            else {
                return false;
            };
            let parts: Vec<u64> = version.split('.').filter_map(|v| v.parse().ok()).collect();
            parts.len() == 3 && parts.as_slice() >= [2, 1, 304].as_slice()
        })
}

async fn claude_model_effort(
    cwd: &Path,
    model: &str,
    supports_auto_current: bool,
) -> Option<String> {
    if !supports_auto_current {
        return None;
    }
    let mut args = vec![
        "--safe-mode",
        "-p",
        "/effort status",
        "--output-format",
        "json",
        "--no-session-persistence",
        "--tools=",
    ];
    if model != "default" {
        args.extend(["--model", model]);
    }
    run_effort_probe(cwd, "claude", &args, supports_auto_current).await
}
async fn run_effort_probe(
    cwd: &Path,
    tool: &str,
    args: &[&str],
    supports_auto_current: bool,
) -> Option<String> {
    let output = process::run_tool(
        tool,
        args,
        Some(cwd),
        None,
        &process::empty_environment(),
        Duration::from_secs(20),
    )
    .await
    .ok()?;
    if !output.success {
        return None;
    }
    status_effort(&output.stdout, supports_auto_current)
}
async fn claude(cwd: &Path) -> ProviderModels {
    if process::resolve_tool("claude").is_none() {
        return empty("claude", "not_connected");
    }
    let env = process::empty_environment();
    let auth = process::run_tool(
        "claude",
        &["auth", "status", "--json"],
        Some(cwd),
        None,
        &env,
        Duration::from_secs(20),
    )
    .await;
    match auth
        .ok()
        .and_then(|x| serde_json::from_str::<Value>(&x.stdout).ok())
        .and_then(|v| v.get("loggedIn").and_then(Value::as_bool))
    {
        Some(true) => {}
        Some(false) => return empty("claude", "not_connected"),
        None => return empty("claude", "unavailable"),
    }
    let args = [
        "--safe-mode",
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--no-session-persistence",
        "--tools=",
    ];
    let request = json!({"type":"control_request","request_id":"momso-models","request":{"subtype":"initialize"}});
    match process::run_tool(
        "claude",
        &args,
        Some(cwd),
        Some(&format!("{request}\n")),
        &env,
        Duration::from_secs(30),
    )
    .await
    {
        Ok(output) => {
            let supports_auto_current = claude_auto_current_is_reliable(&output.stdout);
            let mut models = latest_by_family(claude_models(&output.stdout));
            for model in &mut models {
                let level = claude_model_effort(cwd, &model.id, supports_auto_current).await;
                model.effective_default_effort =
                    level.filter(|level| model.efforts.iter().any(|effort| &effort.id == level));
            }
            if models.is_empty() {
                empty("claude", "unavailable")
            } else {
                ProviderModels {
                    provider: "claude",
                    state: "ok",
                    reason: None,
                    models,
                }
            }
        }
        Err(_) => empty("claude", "unavailable"),
    }
}
fn gemini_models(output: &str) -> ProviderModels {
    let models: Vec<Model> = output
        .lines()
        .filter_map(|line| {
            let (id, label) = line.trim().split_once('\t')?;
            if id.is_empty()
                || id.len() > 200
                || label.is_empty()
                || label.len() > 300
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
                || label.chars().any(char::is_control)
            {
                return None;
            }
            Some(Model {
                id: id.to_owned(),
                family: None,
                label: label.to_owned(),
                resolved: None,
                efforts: vec![],
                effort_note: None,
                toggles: vec![],
                fast: fast("gemini", ""),
                default_effort: None,
                effective_default_effort: None,
                is_default: None,
            })
        })
        .collect();
    if models.is_empty() {
        return empty("gemini", "unavailable");
    }
    ProviderModels {
        provider: "gemini",
        state: "ok",
        reason: None,
        models,
    }
}
async fn gemini() -> ProviderModels {
    let environment = match gemini_access::api_key_environment() {
        Ok(Some(environment)) => environment,
        Ok(None) => return empty("gemini", "not_configured"),
        Err(_) => return empty("gemini", "unavailable"),
    };
    let Some(executable) = gemini_access::executable() else {
        return empty("gemini", "unavailable");
    };
    let mut command = tokio::process::Command::new(executable);
    command.args([
        "--settings",
        gemini_access::API_KEY_SESSION_SETTINGS,
        "models",
    ]);
    run_gemini_models(command, environment).await
}
async fn run_gemini_models(
    mut command: tokio::process::Command,
    environment: std::collections::HashMap<String, String>,
) -> ProviderModels {
    command
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    for name in [
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ] {
        command.env_remove(name);
    }
    command.envs(&environment);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    match tokio::time::timeout(Duration::from_secs(20), command.output()).await {
        Ok(Ok(output)) if output.status.success() && output.stdout.len() <= 2 * 1024 * 1024 => {
            match std::str::from_utf8(&output.stdout) {
                Ok(text)
                    if !environment
                        .values()
                        .any(|secret| !secret.is_empty() && text.contains(secret)) =>
                {
                    gemini_models(text)
                }
                _ => empty("gemini", "unavailable"),
            }
        }
        _ => empty("gemini", "unavailable"),
    }
}
pub(crate) async fn all(cwd: &Path) -> Vec<ProviderModels> {
    let (a, b, c) = tokio::join!(codex(cwd), claude(cwd), gemini());
    vec![a, b, c]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selects_latest_known_family_without_dropping_unknowns() {
        let m = |id: &str, label: &str| codex_model(&json!({"id":id,"displayName":label})).unwrap();
        let selected = latest_by_family(vec![
            m("a", "GPT-5.9 Terra"),
            m("b", "GPT-5.10 Terra"),
            m("c", "GPT-10 Terra Preview"),
            m("d", "Mystery"),
        ]);
        assert_eq!(
            selected.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["b", "d"]
        );
        assert_eq!(selected[0].family.as_deref(), Some("terra"));
        assert!(selected[1].family.is_none());
    }
    #[test]
    fn codex_keeps_returned_values_and_effort() {
        let m=codex_model(&json!({"id":"a","displayName":"A","model":"a","supportedReasoningEfforts":[{"reasoningEffort":"high"}],"defaultReasoningEffort":"high","isDefault":true})).unwrap();
        assert_eq!(m.efforts[0].id, "high");
        assert!(m.family.is_none());
        assert_eq!(m.default_effort.as_deref(), Some("high"));
        let wire = serde_json::to_value(&m).unwrap();
        assert_eq!(wire["efforts"][0]["kind"], "level");
        assert_eq!(wire["efforts"][0]["recommended"], true);
    }
    #[test]
    fn codex_only_marks_recommendations_from_cli_values() {
        let model = codex_model(&json!({"id":"a", "displayName":"A", "defaultReasoningEffort":"high", "supportedReasoningEfforts":[{"reasoningEffort":"low"}, {"reasoningEffort":"high"}, {"reasoningEffort":"ultracode"}]})).unwrap();
        let wire = serde_json::to_value(model).unwrap();
        let efforts = wire["efforts"].as_array().unwrap();
        assert_eq!(efforts.len(), 2);
        assert!(efforts[0].get("recommended").is_none());
        assert_eq!(efforts[1]["recommended"], true);
        let unknown = codex_model(&json!({"id":"b", "displayName":"B", "supportedReasoningEfforts":[{"reasoningEffort":"medium"}]})).unwrap();
        assert!(serde_json::to_value(unknown).unwrap()["efforts"][0]
            .get("recommended")
            .is_none());
    }
    #[test]
    fn codex_does_not_advertise_unverified_special_modes() {
        let model = codex_model(&json!({"id":"a","displayName":"A","supportedReasoningEfforts":[{"reasoningEffort":"high"},{"reasoningEffort":"ultra"}]})).unwrap();
        assert_eq!(model.efforts.len(), 1);
        assert_eq!(model.efforts[0].id, "high");
        assert!(model.toggles.is_empty());
    }
    #[test]
    fn codex_effective_default_uses_only_supported_configured_value() {
        let make = || {
            codex_model(&json!({"id":"a","displayName":"A","defaultReasoningEffort":"medium","supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"medium"}]})).unwrap()
        };
        let mut models = vec![make()];
        apply_codex_effective_config(&mut models, None);
        assert_eq!(models[0].effective_default_effort, None);
        apply_codex_effective_config(
            &mut models,
            Some(&json!({"config":{"model_reasoning_effort":"low"}})),
        );
        assert_eq!(models[0].effective_default_effort.as_deref(), Some("low"));
        apply_codex_effective_config(
            &mut models,
            Some(&json!({"config":{"model_reasoning_effort":"xhigh"}})),
        );
        assert_eq!(models[0].effective_default_effort, None);
    }
    #[test]
    fn claude_reads_cli_models_and_supported_effort() {
        let v=json!({"type":"control_response","response":{"subtype":"success","response":{"models":[{"value":"opus","displayName":"Opus 5","resolvedModel":"actual","supportedEffortLevels":["high"]}]}}}).to_string();
        let models = claude_models(&v);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].resolved.as_deref(), Some("actual"));
        assert_eq!(models[0].efforts[0].id, "high");
    }
    #[test]
    fn reads_only_confirmed_local_effort_status() {
        let value = |text: &str| {
            json!({"type":"result","is_error":false,"local_command":"effort","num_turns":0,"result":text}).to_string()
        };
        assert_eq!(
            status_effort(&value("Effort level: high"), true).as_deref(),
            Some("high")
        );
        assert_eq!(
            status_effort(&value("Effort level: auto (currently medium)"), true).as_deref(),
            Some("medium")
        );
        assert_eq!(
            status_effort(&value("Effort level: auto (currently medium)"), false),
            None
        );
        assert_eq!(status_effort(&value("Effort level: auto"), true), None);
        assert_eq!(status_effort(&json!({"is_error":false,"local_command":"effort","num_turns":1,"result":"Effort level: medium"}).to_string(), true).as_deref(), Some("medium"));
        assert_eq!(
            status_effort(
                &json!({"is_error":false,"num_turns":1,"result":"Effort level: high"}).to_string(),
                true
            ),
            None
        );
        assert!(!claude_auto_current_is_reliable(
            r#"{"response":{"response":{"claude_code_version":"2.1.303"}}}"#
        ));
        assert!(claude_auto_current_is_reliable(
            r#"{"response":{"response":{"claude_code_version":"2.1.319"}}}"#
        ));
    }
    #[tokio::test]
    #[cfg(windows)]
    async fn fake_cli_reports_effective_default_effort() {
        let d = tempfile::tempdir().unwrap();
        let script = d.path().join("fake-status.ps1");
        std::fs::write(&script, "[Console]::WriteLine('{\"type\":\"result\",\"is_error\":false,\"local_command\":\"effort\",\"num_turns\":0,\"result\":\"Effort level: medium\"}')\n").unwrap();
        let path = script.to_str().unwrap();
        let value =
            run_effort_probe(d.path(), "powershell", &["-NoProfile", "-File", path], true).await;
        assert_eq!(value.as_deref(), Some("medium"));
    }
    #[test]
    fn claude_korean_labels_are_serialized_without_invented_recommendations() {
        let v = json!({"type":"control_response","response":{"subtype":"success","response":{"models":[{"value":"fable","displayName":"Fable 5.2","resolvedModel":"claude-fable-5-2","supportedEffortLevels":["low","medium","high","xhigh","max","ultracode"]}]}}}).to_string();
        let model = &claude_models(&v)[0];
        assert_eq!(model.effort_note.as_deref(), Some("더 높은 노력은 더 철저한 응답을 의미하지만, 시간이 더 오래 걸리고 한도를 더 빨리 사용합니다."));
        assert_eq!(
            model
                .efforts
                .iter()
                .map(|e| e.label.as_str())
                .collect::<Vec<_>>(),
            ["낮음", "중간", "높음", "엑스트라", "최대"]
        );
        assert!(model.efforts.iter().all(|e| serde_json::to_value(e)
            .unwrap()
            .get("recommended")
            .is_none()));
        assert_eq!(model.fast.supported, Some(true));
        assert_eq!(model.fast.label, "고속 모드");
        assert_eq!(model.fast.state, "unavailable");
        assert!(model.toggles.is_empty());
        let wire = serde_json::to_value(model).unwrap();
        assert!(wire["efforts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["kind"] == "level"));
        assert_eq!(wire["toggles"], json!([]));
    }
    #[test]
    fn fast_capabilities_do_not_assert_current_session_state() {
        let model = codex_model(
            &json!({"id":"gpt-6.1-terra","displayName":"GPT-6.1 Terra","model":"gpt-6.1-terra"}),
        )
        .unwrap();
        assert_eq!(model.fast.supported, Some(true));
        assert_eq!(model.fast.label, "Fast mode");
        assert_eq!(model.fast.state, "unavailable");
        assert_eq!(fast("claude", "claude-sonnet-5-5").supported, Some(false));
        assert_eq!(fast("codex", "future-model").supported, Some(false));
    }
    #[test]
    fn gemini_reads_cli_model_list() {
        let p = gemini_models("Fetching available models...\nm\tM\n");
        assert_eq!(p.state, "ok");
        assert_eq!(p.models[0].id, "m");
        assert_eq!(gemini_models("not a model").state, "unavailable");
    }
    #[cfg(windows)]
    #[tokio::test]
    async fn gemini_fake_cli_receives_key_only_in_environment() {
        let mut command = tokio::process::Command::new("powershell.exe");
        command.args(["-NoProfile", "-Command", "if ($env:GEMINI_API_KEY -ne 'momso-fake-key-for-test' -or $env:GOOGLE_API_KEY) { exit 1 }; Write-Output \"fake-model`tFake Model\""]);
        let environment = std::collections::HashMap::from([(
            "GEMINI_API_KEY".to_owned(),
            "momso-fake-key-for-test".to_owned(),
        )]);
        let result = run_gemini_models(command, environment).await;
        assert_eq!(result.state, "ok");
        assert_eq!(result.models[0].id, "fake-model");
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("momso-fake-key-for-test"));
    }
    #[tokio::test]
    #[ignore = "실제 CLI 연결 확인"]
    async fn live_models() {
        let dir = tempfile::tempdir().unwrap();
        let values = all(dir.path()).await;
        for p in values {
            println!(
                "{} {} {:?}",
                p.provider,
                p.state,
                p.models
                    .iter()
                    .map(|m| (&m.family, &m.label, &m.id, &m.fast, &m.efforts))
                    .collect::<Vec<_>>()
            );
        }
    }
}
