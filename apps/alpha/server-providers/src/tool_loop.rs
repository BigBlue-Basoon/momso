//! Tool-enabled API conversation. The host owns policy, approvals, credentials,
//! OTP handling, and the S22/S33/S32 tool implementations.
use crate::{
    transport::{Message, Role},
    ApiKey, Provider,
};
use serde_json::{json, Value};
use std::{collections::HashSet, future::Future, pin::Pin};
use tokio::sync::watch;

pub struct Context<'a> {
    pub chat_id: &'a str,
    pub turn_id: &'a str,
}
pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, &'static str>> + Send + 'a>>;

/// Definitions and dispatch are trusted host inputs, not supplied by the model.
/// The host must use S32's approval/input broker and must redact saved credentials
/// and OTPs before returning results. Cancellation must stop host operations.
pub trait ToolHost {
    fn definitions(&self) -> Vec<Value>;
    fn dispatch<'a>(
        &'a mut self,
        context: &'a Context<'_>,
        name: &'a str,
        args: Value,
        cancel: watch::Receiver<bool>,
    ) -> ToolFuture<'a>;
}

/// S34 supplies these trusted routes using its existing BrowserTools, CrmStore,
/// skill/dictionary and S32 approval/input objects. Model arguments can never
/// select a chat, grant permission, or provide a credential to this router.
#[cfg(target_os = "linux")]
pub trait MomsoDispatch {
    fn browser<'a>(
        &'a mut self,
        context: &'a Context<'_>,
        name: &'a str,
        args: Value,
        cancel: watch::Receiver<bool>,
    ) -> ToolFuture<'a>;
    fn crm<'a>(
        &'a mut self,
        context: &'a Context<'_>,
        name: &'a str,
        args: Value,
        cancel: watch::Receiver<bool>,
    ) -> ToolFuture<'a>;
    fn skill_or_dictionary<'a>(
        &'a mut self,
        context: &'a Context<'_>,
        name: &'a str,
        args: Value,
        cancel: watch::Receiver<bool>,
    ) -> ToolFuture<'a>;
}

#[cfg(target_os = "linux")]
pub struct MomsoTools<D> {
    pub dispatcher: D,
}

#[cfg(target_os = "linux")]
impl<D: MomsoDispatch> ToolHost for MomsoTools<D> {
    fn definitions(&self) -> Vec<Value> {
        let mut tools = momso_server_browser::tools::tool_definitions()
            .as_array()
            .cloned()
            .unwrap_or_default();
        tools.extend(
            momso_server_crm::tool_definitions()
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        tools.push(json!({"name":"momso_skill_read","description":"Read an enabled momso skill by name.","inputSchema":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}}));
        tools.push(json!({"name":"momso_dictionary_search","description":"Find terms in the momso dictionary.","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}}));
        tools
    }
    fn dispatch<'a>(
        &'a mut self,
        context: &'a Context<'_>,
        name: &'a str,
        args: Value,
        cancel: watch::Receiver<bool>,
    ) -> ToolFuture<'a> {
        if momso_server_browser::tools::TOOL_NAMES.contains(&name) {
            self.dispatcher.browser(context, name, args, cancel)
        } else if momso_server_crm::TOOL_NAMES.contains(&name) {
            self.dispatcher.crm(context, name, args, cancel)
        } else if matches!(name, "momso_skill_read" | "momso_dictionary_search") {
            self.dispatcher
                .skill_or_dictionary(context, name, args, cancel)
        } else {
            Box::pin(async { Err("invalid_tool_call") })
        }
    }
}

pub struct ToolTurn<'a> {
    pub provider: Provider,
    pub model: &'a str,
    pub messages: &'a [Message],
    pub key: &'a ApiKey,
    pub context: Context<'a>,
}

fn contains_secret(value: &Value, secret: &str) -> bool {
    match value {
        Value::String(s) => s.contains(secret),
        Value::Array(a) => a.iter().any(|v| contains_secret(v, secret)),
        Value::Object(o) => o
            .iter()
            .any(|(k, v)| k.contains(secret) || contains_secret(v, secret)),
        _ => false,
    }
}

fn definitions(
    provider: Provider,
    raw: &[Value],
) -> Result<(Vec<Value>, HashSet<String>), &'static str> {
    let mut names = HashSet::new();
    if raw.len() > 128 {
        return Err("invalid_tools");
    }
    let mut out = Vec::new();
    for entry in raw {
        let name = entry["name"].as_str().ok_or("invalid_tools")?;
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || !names.insert(name.to_owned())
        {
            return Err("invalid_tools");
        }
        let description = entry["description"].as_str().ok_or("invalid_tools")?;
        let schema = entry
            .get("inputSchema")
            .filter(|v| v.is_object())
            .ok_or("invalid_tools")?;
        out.push(match provider {
            Provider::Claude => {
                json!({"name":name,"description":description,"input_schema":schema})
            }
            Provider::Gemini => {
                json!({"name":name,"description":description,"parametersJsonSchema":schema})
            }
        });
    }
    Ok((out, names))
}

/// The tool-enabled path uses complete API responses for each tool round, then
/// emits the same safe text deltas/byte offsets as S17. No response is a real
/// AI action until the host authorizes and dispatches it.
pub async fn chat_with_tools(
    turn: ToolTurn<'_>,
    host: &mut impl ToolHost,
    cancel: watch::Receiver<bool>,
    emit: impl FnMut(&str, usize),
) -> Result<(), &'static str> {
    let endpoint = match turn.provider {
        Provider::Claude => "https://api.anthropic.com/v1/messages".to_string(),
        Provider::Gemini => format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
            turn.model
        ),
    };
    run_at(turn, host, cancel, emit, &endpoint).await
}

async fn run_at(
    turn: ToolTurn<'_>,
    host: &mut impl ToolHost,
    mut cancel: watch::Receiver<bool>,
    mut emit: impl FnMut(&str, usize),
    endpoint: &str,
) -> Result<(), &'static str> {
    if turn.model.is_empty()
        || turn.model.len() > 128
        || !turn
            .model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        || turn.messages.is_empty()
        || turn.context.chat_id.is_empty()
        || turn.context.turn_id.is_empty()
    {
        return Err("invalid_arguments");
    }
    let (tools, names) = definitions(turn.provider, &host.definitions())?;
    let mut history:Vec<Value>=turn.messages.iter().map(|m| match turn.provider {
        Provider::Claude=>json!({"role":match m.role {Role::User=>"user",Role::Assistant=>"assistant"},"content":m.text}),
        Provider::Gemini=>json!({"role":match m.role {Role::User=>"user",Role::Assistant=>"model"},"parts":[{"text":m.text}]}),
    }).collect();
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|_| "unavailable")?;
    let mut offset = 0usize;
    let mut seen_ids = HashSet::new();
    let mut redactor = crate::transport::Redactor::new(turn.key.expose_to_transport());
    for _round in 0..32 {
        if *cancel.borrow() {
            return Err("cancelled");
        }
        let req = match turn.provider {
            Provider::Claude => client
                .post(endpoint)
                .header("x-api-key", turn.key.expose_to_transport())
                .header("anthropic-version", "2023-06-01")
                .json(
                    &json!({"model":turn.model,"messages":history,"max_tokens":4096,"tools":tools}),
                ),
            Provider::Gemini => client
                .post(endpoint)
                .header("x-goog-api-key", turn.key.expose_to_transport())
                .json(&json!({"contents":history,"tools":[{"functionDeclarations":tools}]})),
        };
        let mut response = tokio::select! { _=cancel.changed()=>return Err("cancelled"), r=req.send()=>r.map_err(|_|"provider_unavailable")? };
        if !response.status().is_success() {
            return Err(if matches!(response.status().as_u16(), 401 | 403) {
                "auth_failed"
            } else {
                "provider_failed"
            });
        }
        let mut bytes = Vec::new();
        loop {
            let part = tokio::select! {_=cancel.changed()=>return Err("cancelled"), r=response.chunk()=>r.map_err(|_|"provider_unavailable")?};
            let Some(part) = part else { break };
            if bytes.len().saturating_add(part.len()) > 4 * 1024 * 1024 {
                return Err("response_too_large");
            }
            bytes.extend_from_slice(&part);
        }
        let result: Value =
            serde_json::from_slice(&bytes).map_err(|_| "provider_invalid_response")?;
        if contains_secret(&result, turn.key.expose_to_transport()) {
            return Err("sensitive_output");
        }
        let (parts, stop) = match turn.provider {
            Provider::Claude => (
                result["content"]
                    .as_array()
                    .cloned()
                    .ok_or("provider_invalid_response")?,
                result["stop_reason"].as_str().unwrap_or("").to_owned(),
            ),
            Provider::Gemini => (
                result["candidates"][0]["content"]["parts"]
                    .as_array()
                    .cloned()
                    .ok_or("provider_invalid_response")?,
                result["candidates"][0]["finishReason"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned(),
            ),
        };
        let mut calls = Vec::new();
        for part in &parts {
            let text = match turn.provider {
                Provider::Claude if part["type"] == "text" => part["text"].as_str(),
                Provider::Gemini if part["thought"] != true => part["text"].as_str(),
                _ => None,
            };
            if let Some(text) = text {
                let safe = redactor.push(text, false)?;
                if offset.saturating_add(safe.len()) > 4 * 1024 * 1024 {
                    return Err("response_too_large");
                }
                if !safe.is_empty() {
                    offset += safe.len();
                    emit(&safe, offset);
                }
            }
            let call = match turn.provider {
                Provider::Claude if part["type"] == "tool_use" => Some(part),
                Provider::Gemini => part.get("functionCall"),
                _ => None,
            };
            if let Some(call) = call {
                let name = call["name"].as_str().ok_or("provider_invalid_response")?;
                let args = match turn.provider {
                    Provider::Claude => call
                        .get("input")
                        .cloned()
                        .ok_or("provider_invalid_response")?,
                    Provider::Gemini => call.get("args").cloned().unwrap_or_else(|| json!({})),
                };
                let id = call.get("id").and_then(Value::as_str).map(str::to_owned);
                if !names.contains(name)
                    || !args.is_object()
                    || (turn.provider == Provider::Claude
                        && id.as_deref().is_none_or(str::is_empty))
                {
                    return Err("invalid_tool_call");
                }
                calls.push((name.to_owned(), id, args));
            }
        }
        if calls.is_empty() {
            let success = match turn.provider {
                Provider::Claude => matches!(stop.as_str(), "end_turn" | "stop_sequence"),
                Provider::Gemini => stop == "STOP",
            };
            if !success {
                return Err("provider_incomplete");
            }
            let tail = redactor.push("", true)?;
            if offset.saturating_add(tail.len()) > 4 * 1024 * 1024 {
                return Err("response_too_large");
            }
            if !tail.is_empty() {
                offset += tail.len();
                emit(&tail, offset);
            }
            return Ok(());
        }
        if calls.len() > 32
            || (turn.provider == Provider::Claude && stop != "tool_use")
            || (turn.provider == Provider::Gemini && stop != "STOP")
        {
            return Err("invalid_tool_call");
        }
        for (_, id, _) in &calls {
            if let Some(id) = id {
                if !seen_ids.insert(id.clone()) {
                    return Err("invalid_tool_call");
                }
            }
        }
        match turn.provider {
            Provider::Claude => history.push(json!({"role":"assistant","content":parts})),
            Provider::Gemini => history.push(json!({"role":"model","parts":parts})),
        }
        let mut results = Vec::new();
        for (name, id, args) in calls {
            if *cancel.borrow() {
                return Err("cancelled");
            }
            let output = host
                .dispatch(&turn.context, &name, args, cancel.clone())
                .await?;
            if *cancel.borrow() {
                return Err("cancelled");
            }
            if contains_secret(&output, turn.key.expose_to_transport()) {
                return Err("sensitive_output");
            }
            let encoded =
                serde_json::to_string(&output).map_err(|_| "provider_invalid_response")?;
            if encoded.len() > 1024 * 1024 {
                return Err("response_too_large");
            }
            results.push(match turn.provider {
                Provider::Claude => {
                    json!({"type":"tool_result","tool_use_id":id,"content":encoded})
                }
                Provider::Gemini => {
                    let mut call = json!({"name":name,"response":{"result":output}});
                    if let Some(id) = id {
                        call["id"] = json!(id);
                    }
                    json!({"functionResponse":call})
                }
            });
        }
        history.push(match turn.provider {
            Provider::Claude => json!({"role":"user","content":results}),
            Provider::Gemini => json!({"role":"user","parts":results}),
        });
    }
    Err("tool_round_limit")
}

#[cfg(test)]
mod tests;
