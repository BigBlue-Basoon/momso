use crate::{gemini_access, process, provider_auth};
use serde::Serialize;
use std::{path::Path, process::Stdio, sync::Mutex};
use tauri::{AppHandle, Emitter, Manager};

static INSTALLING: Mutex<bool> = Mutex::new(false);

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
    let existing = if provider == "gemini" {
        gemini_access::executable().is_some()
    } else {
        process::resolve_interactive_tool(&provider).is_some()
    };
    let Ok(resources) = app.path().resource_dir() else {
        return "unavailable";
    };
    let Ok(data_root) = app.path().app_config_dir() else {
        return "unavailable";
    };
    if let Some(state) = installation_gate(&provider, existing, node_paths(&resources).is_some()) {
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
    let found = if provider == "gemini" {
        gemini_access::executable().is_some()
    } else {
        process::resolve_interactive_tool(&provider).is_some()
    };
    let reason = match result {
        Ok(()) if found => None,
        Ok(()) => Some("toolNotFound"),
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

async fn run_install(
    provider: &str,
    resources: &Path,
    data_root: &Path,
) -> Result<(), &'static str> {
    let mut command = if let Some(package) = npm_package(provider) {
        let Some((node, npm)) = node_paths(resources) else {
            return Err("runtimeMissing");
        };
        let prefix = data_root.join("ai-tools").join(provider);
        let mut paths = vec![node.parent().unwrap_or(resources).to_path_buf()];
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        let Ok(path) = std::env::join_paths(paths) else {
            return Err("unavailable");
        };
        let mut cmd = tokio::process::Command::new(node);
        cmd.env("PATH", path);
        cmd.env(
            "npm_config_userconfig",
            data_root.join("ai-tools/npm-userconfig"),
        );
        cmd.env(
            "npm_config_globalconfig",
            data_root.join("ai-tools/npm-globalconfig"),
        );
        cmd.env("npm_config_cache", data_root.join("ai-tools/npm-cache"));
        cmd.arg(npm)
            .args([
                "install",
                "--no-save",
                "--no-audit",
                "--no-fund",
                "--package-lock=false",
                "--prefix",
            ])
            .arg(prefix)
            .arg(package);
        cmd
    } else if provider == "gemini" {
        let mut cmd = tokio::process::Command::new("powershell.exe");
        cmd.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command",
            "$ErrorActionPreference='Stop'; $s=Invoke-RestMethod 'https://antigravity.google/cli/install.ps1'; & ([scriptblock]::Create($s))"]);
        cmd
    } else {
        return Err("unavailable");
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    match tokio::time::timeout(std::time::Duration::from_secs(900), command.status()).await {
        Err(_) => Err("timeout"),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            Err("permissionDenied")
        }
        Ok(Err(_)) => Err("startFailed"),
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(_)) => Err("installFailed"),
    }
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
fs.mkdirSync(prefix, { recursive: true });
fs.writeFileSync(path.join(prefix, 'observed.json'), JSON.stringify({
  args, userconfig: process.env.npm_config_userconfig,
  globalconfig: process.env.npm_config_globalconfig, cache: process.env.npm_config_cache
}));
"#,
        )
        .unwrap();
        let data = dir.path().join("data");
        for provider in ["codex", "claude"] {
            run_install(provider, &dir.path().join("resources"), &data)
                .await
                .unwrap();
            let prefix = data.join("ai-tools").join(provider);
            let result: serde_json::Value =
                serde_json::from_slice(&std::fs::read(prefix.join("observed.json")).unwrap())
                    .unwrap();
            let args = result["args"].as_array().unwrap();
            assert_eq!(args[0], "install");
            assert_eq!(args.last().unwrap(), npm_package(provider).unwrap());
            assert!(args.iter().any(|arg| arg == prefix.to_str().unwrap()));
            for (field, relative) in [
                ("userconfig", "npm-userconfig"),
                ("globalconfig", "npm-globalconfig"),
                ("cache", "npm-cache"),
            ] {
                assert_eq!(
                    result[field],
                    data.join(format!("ai-tools/{relative}")).to_str().unwrap()
                );
            }
        }
    }
}
