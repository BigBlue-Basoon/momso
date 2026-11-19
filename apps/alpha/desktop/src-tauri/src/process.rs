use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::OnceLock,
    time::Duration,
};
use thiserror::Error;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, Command},
};
use tokio_util::sync::CancellationToken;

const MAX_CAPTURE_BYTES: usize = 2 * 1024 * 1024;
static LOCAL_AI_TOOLS: OnceLock<PathBuf> = OnceLock::new();

pub(crate) use crate::hidden_process::hidden_command;

fn cli_timer(name: &str) -> Option<crate::startup_timing::Timer> {
    let stage = match name {
        "codex" => "cli_codex",
        "claude" => "cli_claude",
        "agy" => "cli_agy",
        "gh" => "cli_gh",
        "wrangler" => "cli_wrangler",
        _ => return None,
    };
    Some(crate::startup_timing::start(stage))
}

pub(crate) fn hidden_async_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    Command::from(hidden_command(program))
}

pub(crate) fn remove_inherited_ai_secrets(command: &mut Command) {
    for name in [
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "MOMSO_CODEX_API_KEY",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
    ] {
        command.env_remove(name);
    }
}

pub(crate) fn set_local_ai_tools(path: PathBuf) {
    let _ = LOCAL_AI_TOOLS.set(path);
}

pub(crate) fn local_ai_tool(root: &Path, name: &str) -> Option<PathBuf> {
    let mut managed = std::fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let filename = entry.file_name().into_string().ok()?;
            let prefix = format!(".momso-{name}-");
            let suffix = filename.strip_prefix(&prefix)?;
            let (time, id) = suffix.split_once('-')?;
            if time.len() != 20
                || !time.bytes().all(|b| b.is_ascii_digit())
                || uuid::Uuid::parse_str(id).is_err()
            {
                return None;
            }
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).ok()?;
            if !metadata.is_dir() || crate::archive_location::is_link_or_reparse(&metadata) {
                return None;
            }
            let marker = path.join(".momso-provider");
            let metadata = std::fs::symlink_metadata(&marker).ok()?;
            if !metadata.is_file()
                || crate::archive_location::is_link_or_reparse(&metadata)
                || std::fs::read_to_string(marker).ok()?.trim() != name
            {
                return None;
            }
            Some((filename, path))
        })
        .collect::<Vec<_>>();
    managed.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    managed
        .into_iter()
        .find_map(|(_, prefix)| local_ai_tool_at(&prefix, name))
        .or_else(|| local_ai_tool_at(&root.join(name), name))
}

pub(crate) fn local_ai_tool_at(prefix: &Path, name: &str) -> Option<PathBuf> {
    let executable = match name {
        "codex" => npm_codex_binary(&prefix.join("codex.cmd"))?,
        "claude" => prefix.join("node_modules/@anthropic-ai/claude-code/bin/claude.exe"),
        "gemini" => prefix.join("agy.exe"),
        _ => return None,
    };
    executable.is_file().then_some(executable)
}

pub(crate) fn managed_ai_tool(name: &str) -> Option<PathBuf> {
    LOCAL_AI_TOOLS
        .get()
        .and_then(|root| local_ai_tool(root, name))
}

#[derive(Debug, Error)]
pub(crate) enum ProcessError {
    #[error("{0} 도구가 설치되어 있지 않습니다")]
    MissingTool(String),
    #[error("외부 도구를 시작하지 못했습니다")]
    Start(#[source] std::io::Error),
    #[error("외부 도구 입력을 전달하지 못했습니다")]
    Stdin(#[source] std::io::Error),
    #[error("외부 도구 응답을 읽지 못했습니다")]
    Stdout(#[source] std::io::Error),
    #[error("Windows 명령 스크립트에 안전하게 전달할 수 없는 인수가 포함되어 있습니다")]
    UnsafeScriptArgument,
    #[error("외부 도구 응답 시간이 초과되었습니다")]
    TimedOut,
    #[error("외부 도구의 응답이 너무 큽니다")]
    OutputTooLarge,
    #[error("요청을 중단했습니다")]
    Cancelled,
    #[error("외부 도구의 프로토콜 응답이 올바르지 않습니다")]
    Protocol,
}

pub(crate) struct ProcessOutput {
    pub(crate) success: bool,
    pub(crate) stdout: String,
}

pub(crate) struct JsonRpcRequest<'a> {
    pub(crate) initialize_params: serde_json::Value,
    pub(crate) method: &'a str,
    pub(crate) params: serde_json::Value,
}

pub(crate) fn resolve_tool(name: &str) -> Option<PathBuf> {
    if let Some(path) = managed_ai_tool(name) {
        return Some(path);
    }
    let output = hidden_command("where.exe").arg(name).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let candidates = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    preferred_tool(candidates)
}

pub(crate) fn resolve_interactive_tool(name: &str) -> Option<PathBuf> {
    let executable = resolve_tool(name)?;
    if name != "codex" || !cfg!(windows) {
        return Some(executable);
    }
    npm_codex_binary(&executable).or(Some(executable))
}

fn npm_codex_binary(shim: &Path) -> Option<PathBuf> {
    let is_script = shim
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        });
    if !is_script {
        return None;
    }
    let (package, target) = match std::env::consts::ARCH {
        "x86_64" => ("codex-win32-x64", "x86_64-pc-windows-msvc"),
        "aarch64" => ("codex-win32-arm64", "aarch64-pc-windows-msvc"),
        _ => return None,
    };
    let npm_root = shim.parent()?;
    let codex_root = npm_root.join("node_modules").join("@openai").join("codex");
    let binary_tail = PathBuf::from("vendor")
        .join(target)
        .join("bin")
        .join("codex.exe");
    [
        codex_root
            .join("node_modules")
            .join("@openai")
            .join(package)
            .join(&binary_tail),
        npm_root
            .join("node_modules")
            .join("@openai")
            .join(package)
            .join(&binary_tail),
        codex_root.join(&binary_tail),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn preferred_tool(candidates: Vec<PathBuf>) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| {
                    ["com", "exe", "cmd", "bat"]
                        .iter()
                        .any(|known| extension.eq_ignore_ascii_case(known))
                })
        })
        .cloned()
        .or_else(|| candidates.into_iter().next())
}

fn safe_fallback_script_argument(argument: &str) -> bool {
    !argument.chars().any(|character| {
        matches!(
            character,
            '"' | '&' | '|' | '<' | '>' | '^' | '%' | '!' | '(' | ')' | '\r' | '\n' | '\0'
        )
    })
}

pub(crate) async fn run_tool(
    name: &str,
    args: &[&str],
    cwd: Option<&Path>,
    stdin: Option<&str>,
    environment: &HashMap<String, String>,
    timeout: Duration,
) -> Result<ProcessOutput, ProcessError> {
    run_tool_cancellable(name, args, cwd, stdin, environment, timeout, None).await
}

pub(crate) async fn run_tool_cancellable(
    name: &str,
    args: &[&str],
    cwd: Option<&Path>,
    stdin: Option<&str>,
    environment: &HashMap<String, String>,
    timeout: Duration,
    cancellation: Option<CancellationToken>,
) -> Result<ProcessOutput, ProcessError> {
    let _timer = cli_timer(name);
    let executable = resolve_tool(name).ok_or_else(|| ProcessError::MissingTool(name.into()))?;
    run_resolved_tool_cancellable(
        &executable,
        args,
        cwd,
        stdin,
        environment,
        timeout,
        cancellation,
    )
    .await
}

pub(crate) async fn run_resolved_tool_cancellable(
    executable: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    stdin: Option<&str>,
    environment: &HashMap<String, String>,
    timeout: Duration,
    cancellation: Option<CancellationToken>,
) -> Result<ProcessOutput, ProcessError> {
    let is_script = executable
        .extension()
        .and_then(|value| value.to_str())
        .map(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        })
        .unwrap_or(false);

    let mut command = if is_script && executable.with_extension("ps1").is_file() {
        let mut command = hidden_async_command("powershell.exe");
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(executable.with_extension("ps1"))
            .args(args);
        command
    } else if is_script {
        if !args
            .iter()
            .all(|argument| safe_fallback_script_argument(argument))
        {
            return Err(ProcessError::UnsafeScriptArgument);
        }
        let mut command = hidden_async_command(executable);
        command.args(args);
        command
    } else {
        let mut command = hidden_async_command(executable);
        command.args(args);
        command
    };

    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    remove_inherited_ai_secrets(&mut command);
    command
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .envs(environment)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    #[cfg(windows)]
    {
        command.creation_flags(0x08000000);
    }

    let mut child = command.spawn().map_err(ProcessError::Start)?;
    if let Some(input) = stdin {
        let mut child_stdin = child.stdin.take().ok_or_else(|| {
            ProcessError::Stdin(std::io::Error::other("child stdin is unavailable"))
        })?;
        child_stdin
            .write_all(input.as_bytes())
            .await
            .map_err(ProcessError::Stdin)?;
        child_stdin.shutdown().await.map_err(ProcessError::Stdin)?;
    }

    let wait = child.wait_with_output();
    tokio::pin!(wait);
    let output = if let Some(cancellation) = cancellation {
        tokio::select! {
            _ = cancellation.cancelled() => return Err(ProcessError::Cancelled),
            result = tokio::time::timeout(timeout, &mut wait) => {
                result.map_err(|_| ProcessError::TimedOut)?.map_err(ProcessError::Start)?
            }
        }
    } else {
        tokio::time::timeout(timeout, &mut wait)
            .await
            .map_err(|_| ProcessError::TimedOut)?
            .map_err(ProcessError::Start)?
    };
    if output.stdout.len() > MAX_CAPTURE_BYTES || output.stderr.len() > MAX_CAPTURE_BYTES {
        return Err(ProcessError::OutputTooLarge);
    }

    Ok(ProcessOutput {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
    })
}

pub(crate) async fn run_json_rpc_tool(
    name: &str,
    args: &[&str],
    cwd: Option<&Path>,
    environment: &HashMap<String, String>,
    request: JsonRpcRequest<'_>,
    timeout: Duration,
) -> Result<serde_json::Value, ProcessError> {
    let _timer = cli_timer(name);
    let executable =
        resolve_interactive_tool(name).ok_or_else(|| ProcessError::MissingTool(name.into()))?;
    run_json_rpc_resolved(&executable, args, cwd, environment, request, timeout).await
}

pub(crate) async fn run_json_rpc_resolved(
    executable: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    environment: &HashMap<String, String>,
    request: JsonRpcRequest<'_>,
    timeout: Duration,
) -> Result<serde_json::Value, ProcessError> {
    let is_script = executable
        .extension()
        .and_then(|value| value.to_str())
        .map(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        })
        .unwrap_or(false);
    let mut command = if is_script {
        if !args
            .iter()
            .all(|argument| safe_fallback_script_argument(argument))
        {
            return Err(ProcessError::UnsafeScriptArgument);
        }
        let mut command = hidden_async_command(executable);
        command.args(args);
        command
    } else {
        let mut command = hidden_async_command(executable);
        command.args(args);
        command
    };

    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    remove_inherited_ai_secrets(&mut command);
    command
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .envs(environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    #[cfg(windows)]
    {
        command.creation_flags(0x08000000);
    }

    let exchange = async move {
        let mut child = command.spawn().map_err(ProcessError::Start)?;
        let mut child_stdin = child.stdin.take().ok_or_else(|| {
            ProcessError::Stdin(std::io::Error::other("child stdin is unavailable"))
        })?;
        let child_stdout = child.stdout.take().ok_or_else(|| {
            ProcessError::Stdout(std::io::Error::other("child stdout is unavailable"))
        })?;
        let mut reader = BufReader::new(child_stdout);

        write_json_rpc_message(
            &mut child_stdin,
            &serde_json::json!({
                "method": "initialize",
                "id": 1,
                "params": request.initialize_params,
            }),
        )
        .await?;
        let _ = read_json_rpc_response(&mut reader, 1).await?;

        write_json_rpc_message(
            &mut child_stdin,
            &serde_json::json!({ "method": "initialized", "params": {} }),
        )
        .await?;
        write_json_rpc_message(
            &mut child_stdin,
            &serde_json::json!({
                "method": request.method,
                "id": 2,
                "params": request.params,
            }),
        )
        .await?;
        let response = read_json_rpc_response(&mut reader, 2).await?;

        let _ = child.kill().await;
        let _ = child.wait().await;
        response
            .get("result")
            .cloned()
            .ok_or(ProcessError::Protocol)
    };

    tokio::time::timeout(timeout, exchange)
        .await
        .map_err(|_| ProcessError::TimedOut)?
}

async fn write_json_rpc_message(
    stdin: &mut ChildStdin,
    message: &serde_json::Value,
) -> Result<(), ProcessError> {
    let mut bytes = serde_json::to_vec(message).map_err(|_| ProcessError::Protocol)?;
    bytes.push(b'\n');
    stdin.write_all(&bytes).await.map_err(ProcessError::Stdin)?;
    stdin.flush().await.map_err(ProcessError::Stdin)
}

async fn read_json_rpc_response<R>(
    reader: &mut R,
    target_id: u64,
) -> Result<serde_json::Value, ProcessError>
where
    R: AsyncBufRead + Unpin,
{
    let mut captured = 0usize;
    loop {
        let mut line = String::new();
        let bytes = reader
            .read_line(&mut line)
            .await
            .map_err(ProcessError::Stdout)?;
        if bytes == 0 {
            return Err(ProcessError::Protocol);
        }
        captured = captured.saturating_add(bytes);
        if captured > MAX_CAPTURE_BYTES {
            return Err(ProcessError::OutputTooLarge);
        }
        let trimmed = line.trim().trim_start_matches('\u{feff}');
        let Ok(message) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        if message.get("id").and_then(serde_json::Value::as_u64) != Some(target_id) {
            continue;
        }
        if message.get("error").is_some() {
            return Err(ProcessError::Protocol);
        }
        return Ok(message);
    }
}

pub(crate) fn empty_environment() -> HashMap<String, String> {
    HashMap::new()
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn gui_parent_launches_cli_without_console_window() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("gui-fixture.exe");
        let report = temp.path().join("result.txt");
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/fixtures/hidden_console_gui.rs");
        let status = super::hidden_command("rustc")
            .args(["--edition=2021", "-o"])
            .arg(&executable)
            .arg(fixture)
            .status()
            .unwrap();
        assert!(status.success());
        let status = super::hidden_command(&executable)
            .arg(&report)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(std::fs::read_to_string(report).unwrap(), "ok");
    }
    #[test]
    fn product_subprocesses_use_hidden_constructors() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|value| value.to_str()) != Some("rs") {
                continue;
            }
            let name = path.file_name().unwrap().to_str().unwrap();
            if matches!(
                name,
                "process.rs"
                    | "hidden_process.rs"
                    | "chat_conformance.rs"
                    | "scale_benchmark.rs"
                    | "provider_engine_smoke.rs"
                    | "command_entry_tests.rs"
                    | "g70_login_resume_test.rs"
                    | "capability_security_tests.rs"
                    | "instagram_preview_contract_tests.rs"
            ) {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let product = source.split("\nmod tests {").next().unwrap();
            let pattern = ["Command", "::new("].concat();
            assert!(
                !product.contains(&pattern),
                "{name} uses an unwrapped process constructor"
            );
        }
        let source = include_str!("process.rs");
        let product = source
            .split("pub(crate) fn remove_inherited_ai_secrets")
            .nth(1)
            .unwrap()
            .split("\nmod tests {")
            .next()
            .unwrap();
        let pattern = ["Command", "::new("].concat();
        assert!(
            !product.contains(&pattern),
            "process.rs uses an unwrapped process constructor"
        );
    }
    use super::*;

    #[test]
    fn unrelated_provider_keys_are_removed_before_child_launch() {
        let mut command = Command::new("fixture-executable");
        for name in [
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "MOMSO_CODEX_API_KEY",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
        ] {
            command.env(name, "fixture-secret");
        }
        remove_inherited_ai_secrets(&mut command);
        command.env("GEMINI_API_KEY", "fixture-selected-secret");
        let env: HashMap<_, _> = command
            .as_std()
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(
            env["GEMINI_API_KEY"].as_deref(),
            Some("fixture-selected-secret")
        );
        for name in [
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "MOMSO_CODEX_API_KEY",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "GOOGLE_API_KEY",
        ] {
            assert!(env[name].is_none());
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn audit_fake_cli_child_inherits_selected_key() {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", r#"& $env:ComSpec /D /C "if defined ANTHROPIC_API_KEY (echo inherited) else (echo removed)""#]);
        remove_inherited_ai_secrets(&mut command);
        command.env("ANTHROPIC_API_KEY", "synthetic-key-only");
        let result = command.output().await.unwrap();
        assert!(result.status.success());
        assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), "inherited");
    }

    #[test]
    fn finds_app_local_provider_binaries_without_path() {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir
            .path()
            .join("claude/node_modules/@anthropic-ai/claude-code/bin/claude.exe");
        std::fs::create_dir_all(claude.parent().unwrap()).unwrap();
        std::fs::write(&claude, b"fixture").unwrap();
        assert_eq!(local_ai_tool(dir.path(), "claude"), Some(claude));
        assert_eq!(local_ai_tool(dir.path(), "other"), None);
    }

    #[test]
    fn managed_provider_slot_wins_without_touching_unknown_legacy_folder() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("gemini");
        std::fs::create_dir(&legacy).unwrap();
        std::fs::write(legacy.join("user-file"), b"keep").unwrap();
        let slot = dir
            .path()
            .join(format!(".momso-gemini-{:020}-{}", 1, uuid::Uuid::new_v4()));
        std::fs::create_dir(&slot).unwrap();
        std::fs::write(slot.join(".momso-provider"), "gemini").unwrap();
        std::fs::write(slot.join("agy.exe"), b"fixture").unwrap();
        assert_eq!(
            local_ai_tool(dir.path(), "gemini"),
            Some(slot.join("agy.exe"))
        );
        assert_eq!(std::fs::read(legacy.join("user-file")).unwrap(), b"keep");
        std::fs::remove_file(slot.join(".momso-provider")).unwrap();
        assert_eq!(local_ai_tool(dir.path(), "gemini"), None);
    }

    #[test]
    fn resolves_an_installed_windows_executable() {
        let powershell = resolve_tool("powershell").expect("PowerShell is installed on Windows");
        assert_eq!(
            powershell.extension().and_then(|value| value.to_str()),
            Some("exe")
        );
    }

    #[test]
    fn tool_resolution_preserves_path_precedence_between_runnable_candidates() {
        let candidates = vec![
            PathBuf::from(r"C:\npm\codex"),
            PathBuf::from(r"C:\npm\codex.cmd"),
            PathBuf::from(r"C:\desktop\codex.exe"),
        ];
        assert_eq!(
            preferred_tool(candidates),
            Some(PathBuf::from(r"C:\npm\codex.cmd"))
        );
    }

    #[test]
    fn interactive_codex_resolves_the_native_binary_behind_an_npm_shim() {
        let fixture = tempfile::tempdir().expect("fixture directory");
        let shim = fixture.path().join("codex.cmd");
        std::fs::write(&shim, "@echo off\r\n").expect("fixture shim");
        let (package, target) = match std::env::consts::ARCH {
            "x86_64" => ("codex-win32-x64", "x86_64-pc-windows-msvc"),
            "aarch64" => ("codex-win32-arm64", "aarch64-pc-windows-msvc"),
            _ => return,
        };
        let binary = fixture
            .path()
            .join("node_modules")
            .join("@openai")
            .join("codex")
            .join("node_modules")
            .join("@openai")
            .join(package)
            .join("vendor")
            .join(target)
            .join("bin")
            .join("codex.exe");
        std::fs::create_dir_all(binary.parent().expect("binary parent")).expect("fixture tree");
        std::fs::write(&binary, []).expect("fixture binary");

        assert_eq!(npm_codex_binary(&shim), Some(binary));
    }

    #[tokio::test]
    async fn runs_a_cmd_shim_with_its_argument_vector_intact() {
        let fixture = tempfile::tempdir().expect("fixture directory");
        let shim = fixture.path().join("momso-argument-fixture.cmd");
        std::fs::write(&shim, "@echo off\r\necho [%~1]\r\necho [%~2]\r\n").expect("fixture shim");

        let output = run_resolved_tool_cancellable(
            &shim,
            &["alpha beta", "--literal=$value;still-one"],
            None,
            None,
            &empty_environment(),
            Duration::from_secs(10),
            None,
        )
        .await
        .expect("fixture shim runs");
        assert!(output.success);
        assert_eq!(
            output.stdout.lines().collect::<Vec<_>>(),
            ["[alpha beta]", "[--literal=$value;still-one]"]
        );
    }

    #[tokio::test]
    async fn prefers_a_powershell_companion_for_complex_script_arguments() {
        let fixture = tempfile::tempdir().expect("fixture directory");
        let shim = fixture.path().join("momso-argument-fixture.cmd");
        let companion = shim.with_extension("ps1");
        std::fs::write(&shim, "@echo off\r\nexit /b 9\r\n").expect("fixture shim");
        std::fs::write(
            companion,
            "$args | ForEach-Object { Write-Output \"[$_]\" }\r\n",
        )
        .expect("fixture companion");

        let output = run_resolved_tool_cancellable(
            &shim,
            &["alpha beta", "--literal=$value;still-one&safe!%PATH%"],
            None,
            None,
            &empty_environment(),
            // Cold PowerShell start on a fresh CI runner can exceed 10 seconds; the shim path above
            // stays at 10 seconds because cmd.exe starts immediately.
            Duration::from_secs(60),
            None,
        )
        .await
        .expect("PowerShell companion runs");
        assert!(output.success);
        assert_eq!(
            output.stdout.lines().collect::<Vec<_>>(),
            ["[alpha beta]", "[--literal=$value;still-one&safe!%PATH%]"]
        );
    }

    #[tokio::test]
    async fn rejects_script_arguments_that_break_the_quoted_boundary() {
        let fixture = tempfile::tempdir().expect("fixture directory");
        let shim = fixture.path().join("momso-argument-fixture.cmd");
        std::fs::write(&shim, "@echo off\r\nexit /b 0\r\n").expect("fixture shim");

        let error = match run_resolved_tool_cancellable(
            &shim,
            &["unsafe&separator"],
            None,
            None,
            &empty_environment(),
            Duration::from_secs(10),
            None,
        )
        .await
        {
            Ok(_) => panic!("unsafe script argument must be rejected"),
            Err(error) => error,
        };
        assert!(matches!(error, ProcessError::UnsafeScriptArgument));
    }
}
