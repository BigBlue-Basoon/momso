use crate::{credentials, process};
use serde::Serialize;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum ConnectionError {
    #[error(transparent)]
    Process(#[from] process::ProcessError),
    #[error(transparent)]
    Credential(#[from] credentials::CredentialError),
    #[error("지원하지 않는 연결 종류입니다")]
    UnsupportedProvider,
    #[error("{0} 로그인에 실패했습니다. 브라우저 인증을 완료한 뒤 다시 시도해 주세요")]
    LoginFailed(&'static str),
    #[error("Cloudflare 연결 도구 설치에 실패했습니다")]
    InstallFailed,
    #[error("API 키는 공백 없이 20자 이상이어야 합니다")]
    InvalidApiKey,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolStatus {
    id: &'static str,
    label: &'static str,
    installed: bool,
    authenticated: bool,
    version: Option<String>,
    can_install: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionOverview {
    tools: Vec<ToolStatus>,
    openai_api_key_configured: bool,
    anthropic_api_key_configured: bool,
}

struct ToolDefinition {
    id: &'static str,
    command: &'static str,
    label: &'static str,
    status_args: &'static [&'static str],
    login_args: &'static [&'static str],
    can_install: bool,
}

const TOOLS: &[ToolDefinition] = &[
    ToolDefinition {
        id: "codex",
        command: "codex",
        label: "Codex",
        status_args: &["login", "status"],
        login_args: &["login"],
        can_install: true,
    },
    ToolDefinition {
        id: "claude",
        command: "claude",
        label: "Claude",
        status_args: &["auth", "status", "--json"],
        login_args: &["auth", "login"],
        can_install: true,
    },
    ToolDefinition {
        id: "github",
        command: "gh",
        label: "GitHub",
        status_args: &["auth", "status", "--active", "--hostname", "github.com"],
        login_args: &[
            "auth",
            "login",
            "--hostname",
            "github.com",
            "--git-protocol",
            "https",
            "--web",
            "--clipboard",
        ],
        can_install: true,
    },
    ToolDefinition {
        id: "neon",
        command: "neon",
        label: "Neon",
        status_args: &["me", "--output", "json"],
        login_args: &["auth"],
        can_install: true,
    },
    ToolDefinition {
        id: "cloudflare",
        command: "wrangler",
        label: "Cloudflare",
        status_args: &["whoami"],
        login_args: &["login"],
        can_install: true,
    },
];

fn definition(provider: &str) -> Result<&'static ToolDefinition, ConnectionError> {
    TOOLS
        .iter()
        .find(|definition| definition.id == provider)
        .ok_or(ConnectionError::UnsupportedProvider)
}

fn safe_version(output: &str) -> Option<String> {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let sanitized = line
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || " ._-/".contains(*character))
        .take(80)
        .collect::<String>();
    (!sanitized.is_empty()).then_some(sanitized)
}

async fn inspect(definition: &'static ToolDefinition) -> ToolStatus {
    if process::resolve_tool(definition.command).is_none() {
        return ToolStatus {
            id: definition.id,
            label: definition.label,
            installed: false,
            authenticated: false,
            version: None,
            can_install: definition.can_install,
        };
    }

    let environment = process::empty_environment();
    let authenticated = process::run_tool(
        definition.command,
        definition.status_args,
        None,
        None,
        &environment,
        Duration::from_secs(20),
    )
    .await
    .map(|output| output.success)
    .unwrap_or(false);
    let version = process::run_tool(
        definition.command,
        &["--version"],
        None,
        None,
        &environment,
        Duration::from_secs(10),
    )
    .await
    .ok()
    .and_then(|output| safe_version(&output.stdout));

    ToolStatus {
        id: definition.id,
        label: definition.label,
        installed: true,
        authenticated,
        version,
        can_install: definition.can_install,
    }
}

pub(crate) async fn overview() -> Result<ConnectionOverview, ConnectionError> {
    let (codex, claude, github, neon, cloudflare) = tokio::join!(
        inspect(&TOOLS[0]),
        inspect(&TOOLS[1]),
        inspect(&TOOLS[2]),
        inspect(&TOOLS[3]),
        inspect(&TOOLS[4]),
    );
    Ok(ConnectionOverview {
        tools: vec![codex, claude, github, neon, cloudflare],
        openai_api_key_configured: credentials::has_secret(credentials::ai_key_name("openai")?)?,
        anthropic_api_key_configured: credentials::has_secret(credentials::ai_key_name(
            "anthropic",
        )?)?,
    })
}

pub(crate) async fn ai_overview() -> Result<ConnectionOverview, ConnectionError> {
    let (codex, claude) = tokio::join!(inspect(&TOOLS[0]), inspect(&TOOLS[1]));
    Ok(ConnectionOverview {
        tools: vec![codex, claude],
        openai_api_key_configured: credentials::has_secret(credentials::ai_key_name("openai")?)?,
        anthropic_api_key_configured: credentials::has_secret(credentials::ai_key_name(
            "anthropic",
        )?)?,
    })
}

pub(crate) async fn connect(provider: &str) -> Result<ToolStatus, ConnectionError> {
    let definition = definition(provider)?;
    let output = process::run_tool(
        definition.command,
        definition.login_args,
        None,
        None,
        &process::empty_environment(),
        Duration::from_secs(300),
    )
    .await?;
    if !output.success {
        return Err(ConnectionError::LoginFailed(definition.label));
    }
    Ok(inspect(definition).await)
}

pub(crate) async fn install(provider: &str) -> Result<ToolStatus, ConnectionError> {
    let (installer, args): (&str, &[&str]) = match provider {
        "codex" => ("npm", &["install", "--global", "@openai/codex@latest"]),
        "claude" => (
            "npm",
            &["install", "--global", "@anthropic-ai/claude-code@latest"],
        ),
        "github" => (
            "winget",
            &[
                "install",
                "--id",
                "GitHub.cli",
                "--exact",
                "--source",
                "winget",
                "--accept-package-agreements",
                "--accept-source-agreements",
                "--silent",
            ],
        ),
        "neon" => ("npm", &["install", "--global", "neonctl@latest"]),
        "cloudflare" => ("npm", &["install", "--global", "wrangler@latest"]),
        _ => return Err(ConnectionError::UnsupportedProvider),
    };
    let output = process::run_tool(
        installer,
        args,
        None,
        None,
        &process::empty_environment(),
        Duration::from_secs(600),
    )
    .await?;
    if !output.success {
        return Err(ConnectionError::InstallFailed);
    }
    Ok(inspect(definition(provider)?).await)
}

pub(crate) fn save_api_key(provider: &str, api_key: &str) -> Result<(), ConnectionError> {
    let api_key = api_key.trim();
    if api_key.len() < 20 || api_key.chars().any(char::is_whitespace) {
        return Err(ConnectionError::InvalidApiKey);
    }
    credentials::set_secret(credentials::ai_key_name(provider)?, api_key)?;
    Ok(())
}

pub(crate) fn delete_api_key(provider: &str) -> Result<(), ConnectionError> {
    credentials::delete_secret(credentials::ai_key_name(provider)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_text_is_small_and_drops_control_characters() {
        assert_eq!(
            safe_version("tool 1.2.3\nsecret\tvalue"),
            Some("tool 1.2.3".into())
        );
    }

    #[test]
    fn unknown_provider_is_rejected() {
        assert!(matches!(
            definition("unknown"),
            Err(ConnectionError::UnsupportedProvider)
        ));
    }
}
