use crate::{gemini_access, process, provider_auth};
use serde::Serialize;
use std::{path::Path, process::Stdio, sync::Mutex};
use tauri::{AppHandle, Emitter, Manager};

static INSTALLING: Mutex<bool> = Mutex::new(false);

fn filesystem_error(error: std::io::Error) -> &'static str {
    if cfg!(windows) && matches!(error.raw_os_error(), Some(39 | 112)) {
        "diskFull"
    } else {
        "filesystemFailed"
    }
}

fn npm_package(provider: &str) -> Option<&'static str> {
    match provider {
        "codex" => Some("@openai/codex@0.169.0"),
        "claude" => Some("@anthropic-ai/claude-code@2.1.325"),
        _ => None,
    }
}

fn installation_gate(provider: &str, existing: bool, node_available: bool) -> Option<&'static str> {
    if !matches!(provider, "codex" | "claude" | "gemini") {
        return Some("unavailable");
    }
    if existing {
        return Some("installed");
    }
    if provider != "gemini" && !node_available {
        return Some("runtimeMissing");
    }
    None
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallProgress<'a> {
    provider: &'a str,
    phase: &'static str,
    received_bytes: Option<u64>,
    total_bytes: Option<u64>,
    can_cancel: bool,
    reason: Option<&'static str>,
}

fn progress(app: &AppHandle, provider: &str, phase: &'static str, reason: Option<&'static str>) {
    let _ = app.emit(
        "provider-install-progress",
        InstallProgress {
            provider,
            phase,
            received_bytes: None,
            total_bytes: None,
            can_cancel: false,
            reason,
        },
    );
}

struct InstallReservation;
impl Drop for InstallReservation {
    fn drop(&mut self) {
        if let Ok(mut slot) = INSTALLING.lock() {
            *slot = false;
        }
    }
}

fn node_paths_in(resources: &Path) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let root = resources.join("node");
    let executable = root.join("node.exe");
    let npm = root.join("node_modules/npm/bin/npm-cli.js");
    (executable.is_file() && npm.is_file()).then_some((executable, npm))
}

fn node_paths(resources: &Path) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    node_paths_in(resources).or_else(|| {
        if cfg!(debug_assertions) {
            node_paths_in(&Path::new(env!("CARGO_MANIFEST_DIR")).join("generated"))
        } else {
            None
        }
    })
}

#[tauri::command]
pub(crate) async fn install_external_provider(app: AppHandle, provider: String) -> &'static str {
    if !matches!(provider.as_str(), "codex" | "claude" | "gemini") {
        return "unavailable";
    }
    let executable = if provider == "gemini" {
        gemini_access::executable()
    } else {
        process::resolve_interactive_tool(&provider)
    };
    let Ok(resources) = app.path().resource_dir() else {
        return "unavailable";
    };
    let Ok(data_root) = app.path().app_config_dir() else {
        return "unavailable";
    };
    if let Some(path) = executable {
        if executable_works(&path).await {
            return "installed";
        }
    }
    if let Some(state) = installation_gate(&provider, false, node_paths(&resources).is_some()) {
        if state == "runtimeMissing" {
            progress(&app, &provider, "failed", Some("runtimeMissing"));
            return "unavailable";
        }
        return state;
    }
    let _reservation = {
        let Some(mut slot) = INSTALLING.lock().ok() else {
            return "unavailable";
        };
        if *slot {
            return "inProgress";
        }
        *slot = true;
        InstallReservation
    };
    provider_auth::notify(&app, &provider, "inProgress");
    progress(&app, &provider, "installing", None);
    let result = run_install(&provider, &resources, &data_root).await;
    if result.is_ok() {
        // This checks that the installed tool runs; it does not assert a checksum.
        progress(&app, &provider, "verifying", None);
    }
    let found = if provider == "gemini" {
        gemini_access::executable()
    } else {
        process::resolve_interactive_tool(&provider)
    };
    let reason = match result {
        Ok(()) => match found {
            Some(path) if executable_works(&path).await => None,
            Some(_) => Some("toolNotRunnable"),
            None => Some("toolNotFound"),
        },
        Err(reason) => Some(reason),
    };
    let state = if reason.is_none() {
        "installed"
    } else {
        "installFailed"
    };
    progress(
        &app,
        &provider,
        if reason.is_none() {
            "completed"
        } else {
            "failed"
        },
        reason,
    );
    provider_auth::notify(&app, &provider, state);
    state
}

pub(crate) async fn executable_works(path: &Path) -> bool {
    let mut command = crate::process::hidden_async_command(path);
    command
        .arg("--version")
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .env_remove("GEMINI_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    matches!(
        tokio::time::timeout(std::time::Duration::from_secs(10), command.status()).await,
        Ok(Ok(status)) if status.success()
    )
}

async fn run_install(
    provider: &str,
    resources: &Path,
    data_root: &Path,
) -> Result<(), &'static str> {
    if !matches!(provider, "codex" | "claude" | "gemini") {
        return Err("unavailable");
    }
    let tools = data_root.join("ai-tools");
    std::fs::create_dir_all(&tools).map_err(filesystem_error)?;
    let temporary = crate::install_staging::OwnedStage::new(&tools, ".provider-install-")
        .map_err(filesystem_error)?;
    let prefix = temporary.path().join(provider);
    let command = if let Some(package) = npm_package(provider) {
        let Some((node, npm)) = node_paths(resources) else {
            return Err("runtimeMissing");
        };
        let mut paths = vec![node.parent().unwrap_or(resources).to_path_buf()];
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        let Ok(path) = std::env::join_paths(paths) else {
            return Err("unavailable");
        };
        let mut cmd = crate::process::hidden_async_command(node);
        cmd.env("PATH", path);
        cmd.env(
            "npm_config_userconfig",
            data_root.join("ai-tools/npm-userconfig"),
        );
        cmd.env(
            "npm_config_globalconfig",
            data_root.join("ai-tools/npm-globalconfig"),
        );
        cmd.env("npm_config_cache", temporary.path().join("npm-cache"));
        cmd.arg(npm)
            .args([
                "install",
                "--no-save",
                "--no-audit",
                "--no-fund",
                "--package-lock=false",
                "--prefix",
            ])
            .arg(&prefix)
            .arg(package);
        cmd
    } else if provider == "gemini" {
        let mut cmd = crate::process::hidden_async_command("powershell.exe");
        let local = temporary.path().join("local");
        std::fs::create_dir(&local).map_err(filesystem_error)?;
        cmd.env("LOCALAPPDATA", local);
        cmd.env("MOMSO_AGY_INSTALL_DIR", &prefix);
        cmd.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command",
            "$ErrorActionPreference='Stop'; $s=Invoke-RestMethod 'https://antigravity.google/cli/install.ps1'; & ([scriptblock]::Create($s)) '--dir' $env:MOMSO_AGY_INSTALL_DIR '--skip-path' '--skip-aliases'"]);
        cmd
    } else {
        return Err("unavailable");
    };
    execute_install(provider, data_root, temporary, command).await
}

async fn execute_install(
    provider: &str,
    data_root: &Path,
    mut temporary: crate::install_staging::OwnedStage,
    mut command: tokio::process::Command,
) -> Result<(), &'static str> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    // After a crash an installer child may still be running: retain that stage
    // rather than recursively deleting files a live child might be using.
    temporary.mark_process_started().map_err(filesystem_error)?;
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err("permissionDenied")
        }
        Err(_) => return Err("startFailed"),
    };
    let stderr = child.stderr.take().ok_or("startFailed")?;
    let mut reader = tokio::spawn(read_install_error(stderr));
    let result = tokio::time::timeout(std::time::Duration::from_secs(900), child.wait()).await;
    match result {
        Err(_) => {
            let _ = child.kill().await;
            reader.abort();
            temporary.retain();
            Err("timeout")
        }
        Ok(Err(_)) => {
            reader.abort();
            Err("installFailed")
        }
        Ok(Ok(status)) if status.success() => {
            reader.abort();
            promote_stage(temporary.path(), data_root, provider).await?;
            Ok(())
        }
        Ok(Ok(_)) => {
            let output =
                match tokio::time::timeout(std::time::Duration::from_secs(2), &mut reader).await {
                    Ok(Ok(output)) => output,
                    _ => {
                        reader.abort();
                        String::new()
                    }
                };
            Err(install_failure_reason(provider, &output))
        }
    }
}

async fn read_install_error(mut stream: tokio::process::ChildStderr) -> String {
    use tokio::io::AsyncReadExt;
    let mut tail = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(n) = stream.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        tail.extend_from_slice(&chunk[..n]);
        if tail.len() > 65_536 {
            tail.drain(..tail.len() - 65_536);
        }
    }
    String::from_utf8_lossy(&tail).into_owned()
}

fn install_failure_reason(provider: &str, stderr: &str) -> &'static str {
    if (matches!(provider, "codex" | "claude") && stderr.contains("EINTEGRITY"))
        || (provider == "gemini" && stderr.contains("Checksum verification failed"))
    {
        "checksumMismatch"
    } else {
        "installFailed"
    }
}

async fn promote_stage(stage: &Path, data_root: &Path, provider: &str) -> Result<(), &'static str> {
    let executable = process::local_ai_tool(stage, provider).ok_or("toolNotFound")?;
    if !executable_works(&executable).await {
        return Err("toolNotRunnable");
    }
    let prefix = stage.join(provider);
    let marker = prefix.join(".momso-provider");
    let metadata = std::fs::symlink_metadata(&prefix).map_err(filesystem_error)?;
    if !metadata.is_dir() || crate::archive_location::is_link_or_reparse(&metadata) {
        return Err("filesystemFailed");
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker)
        .map_err(filesystem_error)?;
    file.write_all(provider.as_bytes())
        .map_err(filesystem_error)?;
    file.sync_all().map_err(filesystem_error)?;
    drop(file);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "filesystemFailed")?
        .as_nanos();
    let destination = data_root.join("ai-tools").join(format!(
        ".momso-{provider}-{nanos:020}-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::rename(prefix, destination).map_err(filesystem_error)
}

#[tauri::command]
pub(crate) async fn connect_external_provider(app: AppHandle, provider: String) -> &'static str {
    if provider == "gemini" {
        return if gemini_access::executable().is_some() {
            "apiKeyRequired"
        } else {
            "notInstalled"
        };
    }
    if matches!(provider.as_str(), "codex" | "claude")
        && process::resolve_interactive_tool(&provider).is_none()
    {
        return "notInstalled";
    }
    provider_auth::provider_login_start(app, provider).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_does_not_invent_bytes_or_cancellation() {
        let event = serde_json::to_value(InstallProgress {
            provider: "gemini",
            phase: "failed",
            received_bytes: None,
            total_bytes: None,
            can_cancel: false,
            reason: Some("timeout"),
        })
        .unwrap();
        assert_eq!(event["provider"], "gemini");
        assert_eq!(event["phase"], "failed");
        assert!(event["receivedBytes"].is_null());
        assert!(event["totalBytes"].is_null());
        assert_eq!(event["canCancel"], false);
        assert_eq!(event["reason"], "timeout");
    }
    #[test]
    fn integrity_failure_is_distinguished_without_exposing_output() {
        assert_eq!(
            install_failure_reason("codex", "npm error code EINTEGRITY"),
            "checksumMismatch"
        );
        assert_eq!(
            install_failure_reason("claude", "npm error code EINTEGRITY"),
            "checksumMismatch"
        );
        assert_eq!(
            install_failure_reason("gemini", "Checksum verification failed."),
            "checksumMismatch"
        );
        assert_eq!(
            install_failure_reason("gemini", "network error"),
            "installFailed"
        );
        assert_eq!(
            install_failure_reason("codex", "Checksum verification failed."),
            "installFailed"
        );
    }
    #[tokio::test]
    async fn unknown_provider_does_not_start_installation() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            run_install("unknown", dir.path(), dir.path()).await,
            Err("unavailable")
        );
    }
    #[test]
    fn packages_are_limited_to_the_requested_tools() {
        assert_eq!(npm_package("codex"), Some("@openai/codex@0.169.0"));
        assert_eq!(
            npm_package("claude"),
            Some("@anthropic-ai/claude-code@2.1.325")
        );
        assert_eq!(npm_package("gemini"), None);
        assert_eq!(npm_package("other"), None);
    }
    #[test]
    fn installation_gate_requires_node_only_for_npm() {
        assert_eq!(
            installation_gate("codex", false, false),
            Some("runtimeMissing")
        );
        assert_eq!(installation_gate("gemini", false, false), None);
        assert_eq!(installation_gate("claude", true, false), Some("installed"));
        assert_eq!(installation_gate("other", false, true), Some("unavailable"));
    }

    #[cfg(windows)]
    #[test]
    fn disk_full_is_distinct_from_other_filesystem_errors() {
        assert_eq!(
            filesystem_error(std::io::Error::from_raw_os_error(112)),
            "diskFull"
        );
        assert_eq!(
            filesystem_error(std::io::Error::from_raw_os_error(5)),
            "filesystemFailed"
        );
    }
    #[test]
    fn bundled_node_needs_both_node_and_npm() {
        let dir = tempfile::tempdir().unwrap();
        assert!(node_paths_in(dir.path()).is_none());
        let npm = dir.path().join("node/node_modules/npm/bin");
        std::fs::create_dir_all(&npm).unwrap();
        std::fs::write(dir.path().join("node/node.exe"), b"node").unwrap();
        std::fs::write(npm.join("npm-cli.js"), b"npm").unwrap();
        assert!(node_paths_in(dir.path()).is_some());
    }

    #[tokio::test]
    async fn invalid_staged_tool_never_replaces_existing_installation() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let stage = temp.path().join("stage");
        let old = data.join("ai-tools/claude");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("user-data"), b"keep").unwrap();
        let fake = stage.join("claude/node_modules/@anthropic-ai/claude-code/bin/claude.exe");
        std::fs::create_dir_all(fake.parent().unwrap()).unwrap();
        std::fs::write(&fake, b"not an executable").unwrap();
        assert!(!executable_works(&fake).await);
        assert_eq!(
            promote_stage(&stage, &data, "claude").await,
            Err("toolNotRunnable")
        );
        assert_eq!(std::fs::read(old.join("user-data")).unwrap(), b"keep");
        assert!(fake.exists());
    }

    #[tokio::test]
    async fn empty_staging_does_not_claim_an_installed_tool() {
        let temp = tempfile::tempdir().unwrap();
        for provider in ["codex", "claude"] {
            assert_eq!(
                promote_stage(temp.path(), temp.path(), provider).await,
                Err("toolNotFound")
            );
        }
    }

    #[test]
    fn isolated_detection_child() {
        if std::env::var_os("MOMSO_EMPTY_PC_TEST").is_none() {
            return;
        }
        assert!(process::resolve_interactive_tool("codex").is_none());
        assert!(process::resolve_interactive_tool("claude").is_none());
        assert!(gemini_access::executable().is_none());
    }

    #[cfg(windows)]
    #[test]
    fn fresh_profile_does_not_discover_host_ai_tools() {
        let temp = tempfile::tempdir().unwrap();
        let windows = std::env::var_os("SystemRoot").unwrap();
        let path = std::path::PathBuf::from(&windows).join("System32");
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "provider_install::tests::isolated_detection_child",
            ])
            .current_dir(temp.path())
            .env_clear()
            .env("SystemRoot", windows)
            .env("PATH", path)
            .env("USERPROFILE", temp.path())
            .env("HOME", temp.path())
            .env("APPDATA", temp.path())
            .env("LOCALAPPDATA", temp.path())
            .env("TEMP", temp.path())
            .env("TMP", temp.path())
            .env("MOMSO_EMPTY_PC_TEST", "1")
            .output()
            .unwrap();
        assert!(result.status.success(), "isolated discovery failed");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn generated_node_is_visible_to_debug_builds_when_prepared() {
        let generated = Path::new(env!("CARGO_MANIFEST_DIR")).join("generated");
        if node_paths_in(&generated).is_some() {
            let missing = tempfile::tempdir().unwrap();
            assert!(node_paths(missing.path()).is_some());
        }
    }

    // Runs only when explicitly supplied with a prepared Node binary. The fake npm
    // records arguments and never downloads or installs a package.
    #[tokio::test]
    #[ignore = "requires MOMSO_TEST_NODE pointing to a prepared Node executable"]
    async fn bundled_node_runs_the_installer_with_isolated_npm_settings() {
        let node = std::env::var_os("MOMSO_TEST_NODE").expect("MOMSO_TEST_NODE");
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("resources/node");
        let npm_dir = runtime.join("node_modules/npm/bin");
        std::fs::create_dir_all(&npm_dir).unwrap();
        std::fs::copy(node, runtime.join("node.exe")).unwrap();
        std::fs::write(
            npm_dir.join("npm-cli.js"),
            r#"
const fs = require('fs');
const path = require('path');
const args = process.argv.slice(2);
const prefix = args[args.indexOf('--prefix') + 1];
const attempts = path.join(path.dirname(process.env.npm_config_userconfig), 'attempt-' + path.basename(prefix));
if (!fs.existsSync(attempts)) {
  fs.writeFileSync(attempts, 'first');
  process.stderr.write('npm error code EINTEGRITY');
  process.exit(1);
}
fs.mkdirSync(prefix, { recursive: true });
const pkg = args.at(-1);
const target = process.arch === 'arm64' ? 'aarch64-pc-windows-msvc' : 'x86_64-pc-windows-msvc';
const platform = process.arch === 'arm64' ? 'codex-win32-arm64' : 'codex-win32-x64';
const exe = pkg.startsWith('@openai/')
  ? path.join(prefix, 'node_modules/@openai/codex/node_modules/@openai', platform, 'vendor', target, 'bin/codex.exe')
  : path.join(prefix, 'node_modules/@anthropic-ai/claude-code/bin/claude.exe');
fs.mkdirSync(path.dirname(exe), { recursive: true });
fs.copyFileSync(process.execPath, exe);
fs.writeFileSync(path.join(path.dirname(process.env.npm_config_userconfig), 'observed-' + path.basename(prefix)), JSON.stringify({
  args, userconfig: process.env.npm_config_userconfig,
  globalconfig: process.env.npm_config_globalconfig, cache: process.env.npm_config_cache
}));
"#,
        )
        .unwrap();
        let data = dir.path().join("data");
        for provider in ["codex", "claude", "gemini"] {
            let unknown = data.join("ai-tools").join(provider);
            std::fs::create_dir_all(&unknown).unwrap();
            std::fs::write(unknown.join("user-file"), b"preserve").unwrap();
            let bad = match provider {
                "codex" => {
                    let (package, target) = if cfg!(target_arch = "aarch64") {
                        ("codex-win32-arm64", "aarch64-pc-windows-msvc")
                    } else {
                        ("codex-win32-x64", "x86_64-pc-windows-msvc")
                    };
                    unknown
                        .join("node_modules/@openai/codex/node_modules/@openai")
                        .join(package)
                        .join("vendor")
                        .join(target)
                        .join("bin/codex.exe")
                }
                "claude" => unknown.join("node_modules/@anthropic-ai/claude-code/bin/claude.exe"),
                _ => unknown.join("agy.exe"),
            };
            std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
            std::fs::write(bad, b"partial").unwrap();
            assert!(
                !executable_works(
                    &process::local_ai_tool(&data.join("ai-tools"), provider).unwrap()
                )
                .await
            );
        }
        for provider in ["codex", "claude"] {
            assert_eq!(
                run_install(provider, &dir.path().join("resources"), &data).await,
                Err("checksumMismatch")
            );
            assert_eq!(
                std::fs::read(data.join("ai-tools").join(provider).join("user-file")).unwrap(),
                b"preserve"
            );
            assert_eq!(
                run_install(provider, &dir.path().join("resources"), &data).await,
                Ok(())
            );
            let result: serde_json::Value = serde_json::from_slice(
                &std::fs::read(data.join(format!("ai-tools/observed-{provider}"))).unwrap(),
            )
            .unwrap();
            let args = result["args"].as_array().unwrap();
            assert_eq!(args[0], "install");
            assert_eq!(args.last().unwrap(), npm_package(provider).unwrap());
            assert!(args.iter().any(|arg| arg.as_str().is_some_and(|value| value
                .contains(".provider-install-")
                && value.ends_with(provider))));
            assert!(process::local_ai_tool(&data.join("ai-tools"), provider).is_some());
            assert!(
                executable_works(
                    &process::local_ai_tool(&data.join("ai-tools"), provider).unwrap()
                )
                .await
            );
            assert!(!std::fs::read_dir(data.join("ai-tools"))
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".provider-install-")));
            for (field, relative) in [
                ("userconfig", "npm-userconfig"),
                ("globalconfig", "npm-globalconfig"),
            ] {
                assert_eq!(
                    result[field],
                    data.join(format!("ai-tools/{relative}")).to_str().unwrap()
                );
            }
            assert!(result["cache"].as_str().is_some_and(|value| value
                .contains(".provider-install-")
                && value.ends_with("npm-cache")));
        }
        // Simulate the Google installer: no official script or network is run.
        for fail in [true, false] {
            let stage = crate::install_staging::OwnedStage::new(
                &data.join("ai-tools"),
                ".provider-install-",
            )
            .unwrap();
            let exe = stage.path().join("gemini/agy.exe");
            let mut command = tokio::process::Command::new(runtime.join("node.exe"));
            let script = if fail {
                "process.stderr.write('Checksum verification failed');process.exit(1)"
            } else {
                "const fs=require('fs'),path=require('path');fs.mkdirSync(path.dirname(process.argv[1]),{recursive:true});fs.copyFileSync(process.execPath,process.argv[1])"
            };
            command.args(["-e", script]).arg(exe);
            let result = execute_install("gemini", &data, stage, command).await;
            assert_eq!(
                result,
                if fail {
                    Err("checksumMismatch")
                } else {
                    Ok(())
                }
            );
        }
        assert!(process::local_ai_tool(&data.join("ai-tools"), "gemini").is_some());
        assert!(
            executable_works(&process::local_ai_tool(&data.join("ai-tools"), "gemini").unwrap())
                .await
        );
        for provider in ["codex", "claude", "gemini"] {
            assert_eq!(
                std::fs::read(data.join("ai-tools").join(provider).join("user-file")).unwrap(),
                b"preserve"
            );
            let db = data.join(format!("{provider}.db"));
            crate::storage::initialize(&db, &data.join("backups"), "test").unwrap();
            // Authentication is deliberately a fake state; no account or secret is used.
            let (state, active, key) = if provider == "gemini" {
                ("apiKey", None, true)
            } else {
                ("account", Some("account"), false)
            };
            assert!(
                provider_auth::chat_auth_method(provider, true, state, active, key, None).is_ok()
            );
            crate::archive_remote::username_set_db(&db, "teacher".into());
            assert_eq!(
                crate::archive_remote::username_get_db(&db)
                    .unwrap()
                    .as_deref(),
                Some("teacher")
            );
            crate::chat::create(&db).unwrap();
            assert_eq!(crate::chat::list(&db).unwrap().len(), 1);
        }
    }
}
