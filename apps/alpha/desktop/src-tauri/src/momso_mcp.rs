//! Narrow MCP stdio endpoint. It exposes no account, filesystem, or mutation APIs.
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

pub(crate) const NAME: &str = "momso_internal_runtime_v1";
pub(crate) const TOOL: &str = "momso_version";
pub(crate) const ARG: &str = "--momso-mcp-stdio";
const GITHUB: &str = "momso_github_connect";
const R2: &str = "momso_object_store_r2_connect";
const GCP: &str = "momso_object_store_gcp_connect";
const AWS: &str = "momso_object_store_aws_connect";
const IMPORT: &str = "momso_import_conversations";

fn aws_credential_type(args: &serde_json::Map<String, Value>) -> Option<&str> {
    if !args.contains_key("account") || !args.contains_key("region") {
        return None;
    }
    match (args.len(), args.get("credentialType")) {
        (3, Some(Value::String(value))) if matches!(value.as_str(), "permanent" | "temporary") => {
            Some(value)
        }
        _ => None,
    }
}

pub(crate) fn executable() -> Result<std::path::PathBuf, String> {
    std::env::current_exe().map_err(|_| "unavailable".to_string())
}

fn args_for(chat_id: &str) -> Vec<String> {
    let mut args = vec![ARG.to_owned()];
    #[cfg(windows)]
    if let Some(pipe) = crate::setup_bridge::name(chat_id) {
        args.extend(["--momso-setup-pipe".to_owned(), pipe]);
    }
    #[cfg(not(windows))]
    let _ = chat_id;
    args
}

pub(crate) fn acp_config(path: &std::path::Path, chat_id: &str) -> Value {
    json!({"name":NAME,"command":path,"args":args_for(chat_id),"env":[]})
}

pub(crate) fn claude_config(path: &std::path::Path, chat_id: &str) -> Value {
    json!({"mcpServers":{(NAME):{"command":path,"args":args_for(chat_id)}}})
}

pub(crate) fn codex_config(path: &std::path::Path, chat_id: &str) -> Result<Vec<String>, String> {
    let path = path.to_str().ok_or("unavailable")?;
    let quoted = serde_json::to_string(path).map_err(|_| "unavailable")?;
    Ok(vec![
        format!("mcp_servers.{NAME}.command={quoted}"),
        format!(
            "mcp_servers.{NAME}.args={}",
            serde_json::to_string(&args_for(chat_id)).map_err(|_| "unavailable")?
        ),
        format!("mcp_servers.{NAME}.tools.{TOOL}.approval_mode=\"approve\""),
        format!("mcp_servers.{NAME}.tools.{GITHUB}.approval_mode=\"approve\""),
        format!("mcp_servers.{NAME}.tools.{R2}.approval_mode=\"approve\""),
        format!("mcp_servers.{NAME}.tools.{GCP}.approval_mode=\"approve\""),
        format!("mcp_servers.{NAME}.tools.{AWS}.approval_mode=\"approve\""),
        format!("mcp_servers.{NAME}.tools.{IMPORT}.approval_mode=\"approve\""),
    ])
}

pub(crate) struct BridgeClient {
    reader: io::BufReader<std::fs::File>,
    writer: std::fs::File,
}
impl BridgeClient {
    fn new(file: std::fs::File) -> io::Result<Self> {
        Ok(Self {
            reader: io::BufReader::new(file.try_clone()?),
            writer: file,
        })
    }
    fn call(&mut self, request: &Value) -> Value {
        let result = (|| -> io::Result<Value> {
            serde_json::to_writer(&mut self.writer, request)?;
            self.writer.write_all(b"\n")?;
            self.writer.flush()?;
            let mut line = String::new();
            if self.reader.read_line(&mut line)? == 0 || line.len() > 4096 {
                return Err(io::ErrorKind::InvalidData.into());
            }
            let value: Value = serde_json::from_str(&line)?;
            let status = value
                .get("status")
                .and_then(Value::as_str)
                .ok_or(io::ErrorKind::InvalidData)?;
            let mut safe = json!({"status":status});
            for key in ["imported", "duplicate", "skipped"] {
                if let Some(n) = value.get(key).and_then(Value::as_u64) {
                    safe[key] = json!(n);
                }
            }
            Ok(safe)
        })();
        result.unwrap_or_else(|_| json!({"status":"unavailable"}))
    }
}

#[cfg(test)]
pub(crate) fn test_bridge(file: std::fs::File) -> BridgeClient {
    BridgeClient::new(file).unwrap()
}
#[cfg(test)]
pub(crate) fn test_response(
    value: &Value,
    ready: &mut bool,
    bridge: &mut BridgeClient,
) -> Option<Value> {
    response_with_bridge(value, ready, Some(bridge))
}

#[cfg(test)]
fn response(v: &Value, initialized: &mut bool) -> Option<Value> {
    response_with_bridge(v, initialized, None)
}

fn response_with_bridge(
    v: &Value,
    initialized: &mut bool,
    mut bridge: Option<&mut BridgeClient>,
) -> Option<Value> {
    let method = v.get("method")?.as_str()?;
    let id = v.get("id").cloned();
    if method == "notifications/initialized" {
        *initialized = true;
        return None;
    }
    let id = id?;
    let result = match method {
        "initialize" => {
            let requested = v
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("");
            let version = if matches!(requested, "2024-11-05" | "2025-03-26" | "2025-06-18") {
                requested
            } else {
                "2025-03-26"
            };
            json!({"protocolVersion":version,"capabilities":{"tools":{}},"serverInfo":{"name":"momso","version":env!("CARGO_PKG_VERSION")}})
        }
        "ping" => json!({}),
        "tools/list" if *initialized => {
            let mut tools = vec![
                json!({"name":TOOL,"description":"Read the installed momso application version.","inputSchema":{"type":"object","properties":{},"additionalProperties":false},"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}}),
            ];
            if bridge.is_some() {
                tools.push(json!({"name":GITHUB,"description":"Connect a new private GitHub repository through momso's own approval. No credentials or parameters are accepted.","inputSchema":{"type":"object","properties":{},"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":true}}));
                tools.push(json!({"name":IMPORT,"description":"Ask the person to select an exported conversation file in momso; show counts and ask for confirmation, then import locally. Never provide file paths or conversation content.","inputSchema":{"type":"object","properties":{"provider":{"type":"string","enum":["chatgpt","claude","gemini"]}},"required":["provider"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":false,"openWorldHint":false}}));
            }
            if bridge.is_some() {
                tools.push(json!({"name":R2,"description":"Connect a new Cloudflare R2 bucket. Provide only the non-secret 32-character account ID. momso securely asks the person for credentials and approval.","inputSchema":{"type":"object","properties":{"account":{"type":"string","pattern":"^[a-fA-F0-9]{32}$"}},"required":["account"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":true}}));
                tools.push(json!({"name":GCP,"description":"Connect a new Google Cloud Storage bucket. Provide only the non-secret project ID. momso securely asks the person for credentials and approval.","inputSchema":{"type":"object","properties":{"account":{"type":"string","pattern":"^[a-z0-9-]{6,30}$"}},"required":["account"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":true}}));
                tools.push(json!({"name":AWS,"description":"Connect a new AWS S3 bucket. Provide only the 12-digit account ID, region and credential type, never credentials. Temporary credentials and us-east-1 are unavailable for this operation. momso requests credentials and approval directly.","inputSchema":{"type":"object","properties":{"account":{"type":"string","pattern":"^[0-9]{12}$"},"region":{"type":"string","pattern":"^[a-z0-9-]{1,32}$"},"credentialType":{"type":"string","enum":["permanent","temporary"]}},"required":["account","region","credentialType"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":true}}));
            }
            json!({"tools":tools})
        }
        "tools/call"
            if *initialized
                && v.pointer("/params/name").and_then(Value::as_str) == Some(IMPORT)
                && bridge.is_some() =>
        {
            let args = v
                .pointer("/params/arguments")
                .and_then(Value::as_object)
                .filter(|v| v.len() == 1);
            let provider = args
                .and_then(|v| v.get("provider"))
                .and_then(Value::as_str)
                .filter(|v| matches!(*v, "chatgpt" | "claude" | "gemini"));
            let Some(provider) = provider else {
                return Some(
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Invalid parameters"}}),
                );
            };
            let result = bridge
                .as_mut()
                .unwrap()
                .call(&json!({"method":"import_conversations","provider":provider}));
            json!({"content":[{"type":"text","text":result.to_string()}]})
        }
        "tools/call"
            if *initialized
                && v.pointer("/params/name").and_then(Value::as_str) == Some(TOOL)
                && v.pointer("/params/arguments")
                    .is_none_or(|a| a.as_object().is_some_and(|o| o.is_empty())) =>
        {
            json!({"content":[{"type":"text","text":env!("CARGO_PKG_VERSION") }]})
        }
        "tools/call"
            if *initialized
                && v.pointer("/params/name").and_then(Value::as_str) == Some(GITHUB)
                && v.pointer("/params/arguments")
                    .is_none_or(|a| a.as_object().is_some_and(|o| o.is_empty()))
                && bridge.is_some() =>
        {
            let result = bridge
                .as_mut()
                .unwrap()
                .call(&json!({"method":"github_connect"}));
            json!({"content":[{"type":"text","text":result.to_string()}]})
        }
        "tools/call"
            if *initialized
                && matches!(
                    v.pointer("/params/name").and_then(Value::as_str),
                    Some(R2 | GCP)
                )
                && bridge.is_some() =>
        {
            let args = v.pointer("/params/arguments").and_then(Value::as_object);
            let account = args
                .filter(|v| v.len() == 1)
                .and_then(|v| v.get("account"))
                .and_then(Value::as_str)
                .filter(|value| {
                    if v.pointer("/params/name").and_then(Value::as_str) == Some(R2) {
                        value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit())
                    } else {
                        (6..=30).contains(&value.len())
                            && value
                                .bytes()
                                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                    }
                });
            let Some(account) = account else {
                return Some(
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Invalid parameters"}}),
                );
            };
            let result = bridge
                .as_mut()
                .unwrap()
                .call(&json!({"method":if v.pointer("/params/name").and_then(Value::as_str) == Some(R2) { "object_store_r2_connect" } else { "object_store_gcp_connect" },"account":account}));
            json!({"content":[{"type":"text","text":result.to_string()}]})
        }
        "tools/call"
            if *initialized
                && v.pointer("/params/name").and_then(Value::as_str) == Some(AWS)
                && bridge.is_some() =>
        {
            let args = v
                .pointer("/params/arguments")
                .and_then(Value::as_object)
                .filter(|args| aws_credential_type(args).is_some());
            let account = args
                .and_then(|a| a.get("account"))
                .and_then(Value::as_str)
                .filter(|s| s.len() == 12 && s.bytes().all(|b| b.is_ascii_digit()));
            let region = args
                .and_then(|a| a.get("region"))
                .and_then(Value::as_str)
                .filter(|s| {
                    !s.is_empty()
                        && s.len() <= 32
                        && s.bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                });
            let (Some(account), Some(region)) = (account, region) else {
                return Some(
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Invalid parameters"}}),
                );
            };
            if args.and_then(aws_credential_type) == Some("temporary") {
                return Some(
                    json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":"{\"status\":\"temporary_credentials_unsupported\"}"}]}}),
                );
            }
            if region == "us-east-1" {
                return Some(
                    json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":"{\"status\":\"unsafe_existing_bucket_semantics\"}"}]}}),
                );
            }
            let result = bridge.as_mut().unwrap().call(
                &json!({"method":"object_store_aws_connect","account":account,"region":region}),
            );
            json!({"content":[{"type":"text","text":result.to_string()}]})
        }
        // Unknown or future write tools are denied. A future mutation must add
        // a host-owned approval channel before it can become callable here.
        _ => {
            return Some(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}}),
            )
        }
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
}

pub fn serve(pipe: Option<&str>) -> io::Result<()> {
    #[cfg(windows)]
    let mut bridge = pipe.and_then(|name| {
        let suffix = name.strip_prefix(r"\\.\pipe\momso-setup-")?;
        uuid::Uuid::parse_str(suffix).ok()?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name)
            .ok()
            .filter(crate::setup_pipe::trusted_server)?;
        BridgeClient::new(file).ok()
    });
    #[cfg(not(windows))]
    let mut bridge: Option<BridgeClient> = {
        let _ = pipe;
        None
    };
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    let mut initialized = false;
    for line in stdin.lock().lines() {
        let line = line?;
        if line.len() > 1_048_576 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request too large",
            ));
        }
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            if let Some(reply) = response_with_bridge(&v, &mut initialized, bridge.as_mut()) {
                serde_json::to_writer(&mut stdout, &reply)?;
                stdout.write_all(b"\n")?;
                stdout.flush()?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn temporary_aws_credentials_are_explicit_and_unknown_fields_rejected() {
        for (input, expected) in [
            (
                json!({"account":"123456789012","region":"ap-northeast-2"}),
                None,
            ),
            (
                json!({"account":"123456789012","region":"ap-northeast-2","credentialType":"permanent"}),
                Some("permanent"),
            ),
            (
                json!({"account":"123456789012","region":"ap-northeast-2","credentialType":"temporary"}),
                Some("temporary"),
            ),
            (
                json!({"account":"123456789012","region":"ap-northeast-2","sessionToken":"secret"}),
                None,
            ),
        ] {
            assert_eq!(aws_credential_type(input.as_object().unwrap()), expected);
        }
        let mut bridge = BridgeClient::new(tempfile::tempfile().unwrap()).unwrap();
        let reply = response_with_bridge(
            &json!({"id":1,"method":"tools/call","params":{"name":AWS,"arguments":{"account":"123456789012","region":"ap-northeast-2","credentialType":"temporary"}}}),
            &mut true,
            Some(&mut bridge),
        ).unwrap();
        assert_eq!(
            reply["result"]["content"][0]["text"],
            "{\"status\":\"temporary_credentials_unsupported\"}"
        );
        let reply = response_with_bridge(
            &json!({"id":2,"method":"tools/call","params":{"name":AWS,"arguments":{"account":"123456789012","region":"us-east-1","credentialType":"permanent"}}}),
            &mut true,
            Some(&mut bridge),
        ).unwrap();
        assert_eq!(
            reply["result"]["content"][0]["text"],
            "{\"status\":\"unsafe_existing_bucket_semantics\"}"
        );
    }
    #[test]
    fn handshake_has_only_one_read_tool_and_denies_unknown() {
        let mut ready = false;
        let init = response(
            &json!({"id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
            &mut ready,
        )
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(response(&json!({"method":"notifications/initialized"}), &mut ready).is_none());
        let tools = response(&json!({"id":2,"method":"tools/list"}), &mut ready).unwrap();
        assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 1);
        assert_eq!(
            tools["result"]["tools"][0]["annotations"]["readOnlyHint"],
            true
        );
        let value = response(
            &json!({"id":3,"method":"tools/call","params":{"name":TOOL,"arguments":{}}}),
            &mut ready,
        )
        .unwrap();
        assert_eq!(
            value["result"]["content"][0]["text"],
            env!("CARGO_PKG_VERSION")
        );
        let denied = response(
            &json!({"id":4,"method":"tools/call","params":{"name":"write_file","arguments":{}}}),
            &mut ready,
        )
        .unwrap();
        assert_eq!(denied["error"]["code"], -32601);
        let secret = "test-secret-never-echo";
        let denied = response(
            &json!({"id":5,"method":"tools/call","params":{"name":"setup","arguments":{"secret":secret}}}),
            &mut ready,
        ).unwrap();
        assert!(!serde_json::to_string(&denied).unwrap().contains(secret));
    }
    #[test]
    fn configurations_are_scoped_to_the_child() {
        let path = std::path::Path::new("C:\\app path\\momso.exe");
        assert_eq!(
            claude_config(path, "no-session")["mcpServers"][NAME]["args"][0],
            ARG
        );
        assert_eq!(acp_config(path, "no-session")["name"], NAME);
        assert!(codex_config(path, "no-session")
            .unwrap()
            .iter()
            .all(|v| v.starts_with("mcp_servers.")));
    }
}
