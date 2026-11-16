use crate::process;
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
    supported: bool,
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
        supported,
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
                        .filter(|id| !id.eq_ignore_ascii_case("ultracode"))
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
async fn codex() -> ProviderModels {
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
                return ProviderModels {
                    provider: "codex",
                    state: "ok",
                    reason: None,
                    models: latest_by_family(models),
                }
            }
        }
    }
    empty("codex", "unavailable")
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
                    toggles: vec![Toggle {
                        id: "ultracode".into(),
                        kind: "toggle",
                        label: "Ultracode".into(),
                        description: None,
                        available: false,
                        state: None,
                    }],
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
                })
            })
            .collect();
    }
    Vec::new()
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
            let models = claude_models(&output.stdout);
            if models.is_empty() {
                empty("claude", "unavailable")
            } else {
                ProviderModels {
                    provider: "claude",
                    state: "ok",
                    reason: None,
                    models: latest_by_family(models),
                }
            }
        }
        Err(_) => empty("claude", "unavailable"),
    }
}
#[cfg(test)]
fn gemini_models(output: &str) -> ProviderModels {
    let Ok(list) = serde_json::from_str::<Vec<Value>>(output) else {
        return empty("gemini", "unavailable");
    };
    let models = list
        .iter()
        .filter_map(|m| {
            Some(Model {
                id: m.get("modelId")?.as_str()?.to_owned(),
                family: None,
                label: m
                    .get("displayName")
                    .or_else(|| m.get("name"))?
                    .as_str()?
                    .to_owned(),
                resolved: None,
                efforts: vec![],
                effort_note: None,
                toggles: vec![],
                fast: fast("gemini", ""),
                default_effort: None,
                is_default: None,
            })
        })
        .collect();
    ProviderModels {
        provider: "gemini",
        state: "ok",
        reason: None,
        models,
    }
}
pub(crate) async fn all(cwd: &Path) -> Vec<ProviderModels> {
    let (a, b) = tokio::join!(codex(), claude(cwd));
    let c = empty("gemini", "unavailable");
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
    fn claude_reads_cli_models_and_supported_effort() {
        let v=json!({"type":"control_response","response":{"subtype":"success","response":{"models":[{"value":"opus","displayName":"Opus 5","resolvedModel":"actual","supportedEffortLevels":["high"]}]}}}).to_string();
        let models = claude_models(&v);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].resolved.as_deref(), Some("actual"));
        assert_eq!(models[0].efforts[0].id, "high");
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
        assert!(model.fast.supported);
        assert_eq!(model.fast.label, "고속 모드");
        assert_eq!(model.fast.state, "unavailable");
        assert!(!model.toggles[0].available);
        let wire = serde_json::to_value(model).unwrap();
        assert!(wire["efforts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["kind"] == "level"));
        assert_eq!(wire["toggles"][0]["kind"], "toggle");
    }
    #[test]
    fn fast_capabilities_do_not_assert_current_session_state() {
        let model = codex_model(
            &json!({"id":"gpt-6.1-terra","displayName":"GPT-6.1 Terra","model":"gpt-6.1-terra"}),
        )
        .unwrap();
        assert!(model.fast.supported);
        assert_eq!(model.fast.label, "Fast mode");
        assert_eq!(model.fast.state, "unavailable");
        assert!(!fast("claude", "claude-sonnet-5-5").supported);
        assert!(!fast("codex", "future-model").supported);
    }
    #[test]
    fn gemini_reads_cli_model_list() {
        let v = json!([{"modelId":"m","name":"M"}]).to_string();
        let p = gemini_models(&v);
        assert_eq!(p.state, "ok");
        assert_eq!(p.models[0].id, "m");
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
