use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command as SyncCommand, Stdio},
    time::Duration,
};
use thiserror::Error;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, Command},
};
use tokio_util::sync::CancellationToken;

const MAX_CAPTURE_BYTES: usize = 2 * 1024 * 1024;

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
    let output = SyncCommand::new("where.exe").arg(name).output().ok()?;
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

async fn run_resolved_tool_cancellable(
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
        let mut command = Command::new("powershell.exe");
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
        let mut command = Command::new(executable);
        command.args(args);
        command
    } else {
        let mut command = Command::new(executable);
        command.args(args);
        command
    };

    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
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
    let executable =
        resolve_interactive_tool(name).ok_or_else(|| ProcessError::MissingTool(name.into()))?;
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
        let mut command = Command::new(executable);
        command.args(args);
        command
    } else {
        let mut command = Command::new(executable);
        command.args(args);
        command
    };

    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
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
    use super::*;

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
