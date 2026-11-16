use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Provider {
    Codex,
    Claude,
    Gemini,
}
impl Provider {
    fn key(self) -> &'static str {
        match self {
            Self::Codex => "permission_mode_codex",
            Self::Claude => "permission_mode_claude",
            Self::Gemini => "permission_mode_gemini",
        }
    }
    fn default(self) -> &'static str {
        match self {
            Self::Codex => "Ask for approval",
            Self::Claude | Self::Gemini => "default",
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModeAvailability {
    id: &'static str,
    label: &'static str,
    description: &'static str,
    available: bool,
    is_default: bool,
}
fn mode(
    id: &'static str,
    label: &'static str,
    description: &'static str,
    available: bool,
    is_default: bool,
) -> ModeAvailability {
    ModeAvailability {
        id,
        label,
        description,
        available,
        is_default,
    }
}
pub(crate) fn available(provider: Provider) -> Vec<ModeAvailability> {
    match provider {
        Provider::Codex => vec![
            mode(
                "Ask for approval",
                "Ask for approval",
                "ask to edit external files and use the internet",
                true,
                true,
            ),
            mode(
                "Approve for me",
                "Approve for me",
                "actions detected as potentially unsafe",
                true,
                false,
            ),
            mode(
                "Full access",
                "Full access",
                "internet and any file on your computer",
                true,
                false,
            ),
        ],
        Provider::Claude => vec![
            mode(
                "default",
                "Manual",
                "Prompts for permission on first use of each tool",
                false,
                true,
            ),
            mode(
                "acceptEdits",
                "Accept edits",
                "Automatically accepts file edits",
                false,
                false,
            ),
            mode(
                "plan",
                "Plan",
                "doesn’t edit your source files",
                false,
                false,
            ),
            mode(
                "auto",
                "Auto",
                "a background classifier checks",
                false,
                false,
            ),
            mode(
                "bypassPermissions",
                "Bypass permissions",
                "Skips permission prompts",
                false,
                false,
            ),
        ],
        Provider::Gemini => vec![
            mode("default", "Default", "Prompts for approval", false, true),
            mode(
                "auto_edit",
                "Auto Edit",
                "Auto-approves edit tools",
                false,
                false,
            ),
            mode("plan", "Plan", "Read-only mode", false, false),
            mode("yolo", "YOLO", "Auto-approves all tools", false, false),
        ],
    }
}
fn apply_requirements(
    modes: &mut [ModeAvailability],
    requirements: &serde_json::Value,
    confirmed_reviewer: Option<&str>,
) {
    fn allowed(requirements: &serde_json::Value, field: &str, value: &str) -> bool {
        match requirements.get(field) {
            None | Some(serde_json::Value::Null) => true,
            Some(serde_json::Value::Array(values)) => {
                values.iter().any(|v| v.as_str() == Some(value))
            }
            _ => false,
        }
    }
    for mode in modes {
        let Some(settings) = codex_settings(mode.id) else {
            mode.available = false;
            continue;
        };
        mode.available &= allowed(requirements, "allowedSandboxModes", settings.sandbox)
            && allowed(
                requirements,
                "allowedApprovalPolicies",
                settings.approval_policy,
            )
            && confirmed_reviewer == Some(settings.reviewer);
    }
}
pub(crate) async fn available_for_runtime(provider: Provider) -> Vec<ModeAvailability> {
    let mut modes = available(provider);
    if provider == Provider::Claude {
        if crate::chat_claude::connected().await {
            for mode in &mut modes {
                mode.available = matches!(mode.id, "default" | "acceptEdits" | "plan");
            }
        }
        return modes;
    }
    if provider != Provider::Codex {
        return modes;
    }
    let disable = |modes: &mut [ModeAvailability]| {
        for mode in modes {
            mode.available = false;
        }
    };
    if crate::process::resolve_interactive_tool("codex").is_none() {
        disable(&mut modes);
        return modes;
    }
    let request = |method, params| crate::process::JsonRpcRequest {
        initialize_params: serde_json::json!({"clientInfo":{"name":"momso","version":env!("CARGO_PKG_VERSION")}}),
        method,
        params,
    };
    let env = crate::process::empty_environment();
    let account = crate::process::run_json_rpc_tool(
        "codex",
        &["app-server"],
        None,
        &env,
        request("account/read", serde_json::json!({"refreshToken":false})),
        std::time::Duration::from_secs(12),
    )
    .await;
    if !matches!(account, Ok(v) if v.get("account").is_some_and(serde_json::Value::is_object)) {
        disable(&mut modes);
        return modes;
    }
    match crate::process::run_json_rpc_tool(
        "codex",
        &["app-server"],
        None,
        &env,
        request("configRequirements/read", serde_json::json!({})),
        std::time::Duration::from_secs(12),
    )
    .await
    {
        Ok(result) => match result.get("requirements") {
            Some(serde_json::Value::Null) => {}
            Some(value @ serde_json::Value::Object(_)) => {
                // The app-server requirements schema does not expose the allowed reviewer list.
                // Under managed requirements, offer only the currently effective reviewer.
                let config = crate::process::run_json_rpc_tool(
                    "codex",
                    &["app-server"],
                    None,
                    &env,
                    request("config/read", serde_json::json!({"includeLayers":false})),
                    std::time::Duration::from_secs(12),
                )
                .await;
                match config {
                    Ok(config) => {
                        let reviewer =
                            match config.get("config").and_then(serde_json::Value::as_object) {
                                Some(settings) => match settings.get("approvals_reviewer") {
                                    None | Some(serde_json::Value::Null) => Some("user"),
                                    Some(serde_json::Value::String(reviewer)) => {
                                        Some(reviewer.as_str())
                                    }
                                    _ => None,
                                },
                                None => None,
                            };
                        apply_requirements(&mut modes, value, reviewer);
                    }
                    Err(_) => disable(&mut modes),
                }
            }
            _ => disable(&mut modes),
        },
        Err(_) => disable(&mut modes),
    }
    modes
}
pub(crate) fn get(path: &Path, provider: Provider) -> Result<String, String> {
    let db = Connection::open(path).map_err(|_| "권한 설정을 읽지 못했습니다".to_string())?;
    let value: Option<String> = db
        .query_row(
            "SELECT value FROM app_runtime_meta WHERE key=?1",
            [provider.key()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| "권한 설정을 읽지 못했습니다".to_string())?;
    match value {
        Some(v) if available(provider).iter().any(|m| m.id == v) => Ok(v),
        Some(_) => Err("권한 설정을 읽지 못했습니다".into()),
        None => Ok(provider.default().into()),
    }
}
pub(crate) fn set(path: &Path, provider: Provider, value: &str) -> Result<(), String> {
    if !available(provider).iter().any(|m| m.id == value) {
        return Err("권한 설정을 저장하지 못했습니다".into());
    }
    let db = Connection::open(path).map_err(|_| "권한 설정을 저장하지 못했습니다".to_string())?;
    db.execute("INSERT INTO app_runtime_meta (key,value,updated_at) VALUES (?1,?2,CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",[provider.key(),value]).map_err(|_| "권한 설정을 저장하지 못했습니다".to_string())?;
    Ok(())
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CodexSettings {
    pub sandbox: &'static str,
    pub approval_policy: &'static str,
    pub reviewer: &'static str,
    pub sandbox_type: &'static str,
}
pub(crate) fn codex_settings(id: &str) -> Option<CodexSettings> {
    match id {
        "Ask for approval" => Some(CodexSettings {
            sandbox: "workspace-write",
            approval_policy: "on-request",
            reviewer: "user",
            sandbox_type: "workspaceWrite",
        }),
        "Approve for me" => Some(CodexSettings {
            sandbox: "workspace-write",
            approval_policy: "on-request",
            reviewer: "auto_review",
            sandbox_type: "workspaceWrite",
        }),
        "Full access" => Some(CodexSettings {
            sandbox: "danger-full-access",
            approval_policy: "never",
            reviewer: "user",
            sandbox_type: "dangerFullAccess",
        }),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_lists_match_their_own_modes() {
        assert_eq!(available(Provider::Codex).len(), 3);
        assert_eq!(available(Provider::Claude).len(), 5);
        assert_eq!(available(Provider::Gemini).len(), 4);
        for p in [Provider::Codex, Provider::Claude, Provider::Gemini] {
            let m = available(p);
            assert_eq!(m.iter().filter(|x| x.is_default).count(), 1);
        }
    }
    #[test]
    fn modes_are_saved_separately() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("x.db");
        let c = Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE app_runtime_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL,updated_at TEXT NOT NULL)").unwrap();
        assert_eq!(get(&path, Provider::Codex).unwrap(), "Ask for approval");
        assert_eq!(get(&path, Provider::Claude).unwrap(), "default");
        assert_eq!(get(&path, Provider::Gemini).unwrap(), "default");
        set(&path, Provider::Codex, "Full access").unwrap();
        set(&path, Provider::Claude, "acceptEdits").unwrap();
        set(&path, Provider::Gemini, "auto_edit").unwrap();
        assert_eq!(get(&path, Provider::Codex).unwrap(), "Full access");
        assert_eq!(get(&path, Provider::Claude).unwrap(), "acceptEdits");
        assert_eq!(get(&path, Provider::Gemini).unwrap(), "auto_edit");
        assert!(set(&path, Provider::Gemini, "acceptEdits").is_err());
    }
    #[test]
    fn managed_requirements_limit_modes_without_changing_them() {
        let mut modes = available(Provider::Codex);
        apply_requirements(
            &mut modes,
            &serde_json::json!({"allowedSandboxModes":["workspace-write"],"allowedApprovalPolicies":["on-request"]}),
            Some("user"),
        );
        assert!(modes[0].available);
        assert!(!modes[1].available);
        assert!(!modes[2].available);
    }
    #[tokio::test]
    #[ignore = "실제 Codex 연결 확인"]
    async fn live_codex_availability() {
        let modes = available_for_runtime(Provider::Codex).await;
        println!(
            "CODEX_AVAILABILITY={:?}",
            modes
                .iter()
                .map(|m| (m.id, m.available))
                .collect::<Vec<_>>()
        );
        assert_eq!(modes.len(), 3);
    }
    #[test]
    fn codex_configuration_preserves_native_modes() {
        let a = codex_settings("Ask for approval").unwrap();
        assert_eq!(
            (a.sandbox, a.approval_policy, a.reviewer),
            ("workspace-write", "on-request", "user")
        );
        let a = codex_settings("Approve for me").unwrap();
        assert_eq!(
            (a.sandbox, a.approval_policy, a.reviewer),
            ("workspace-write", "on-request", "auto_review")
        );
        let a = codex_settings("Full access").unwrap();
        assert_eq!(
            (a.sandbox, a.approval_policy, a.sandbox_type),
            ("danger-full-access", "never", "dangerFullAccess")
        );
        assert!(codex_settings("manual").is_none());
    }
}
