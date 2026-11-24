//! Pass only momso's GitHub token to the Git and GitHub CLI child processes.
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU8, Ordering},
        OnceLock,
    },
};

static ROOT: OnceLock<PathBuf> = OnceLock::new();
static SELECTED: AtomicU8 = AtomicU8::new(0);

pub(crate) fn configure(data_root: &Path) {
    let _ = ROOT.set(data_root.to_path_buf());
}

pub(crate) fn directory() -> Option<&'static Path> {
    ROOT.get().map(PathBuf::as_path)
}

pub(crate) fn select(source: u8) {
    SELECTED.store(source, Ordering::SeqCst);
}

pub(crate) fn source() -> Option<&'static str> {
    (SELECTED.load(Ordering::SeqCst) == 3).then_some("owned")
}

pub(crate) fn apply(command: &mut std::process::Command) -> std::io::Result<()> {
    command
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_ENTERPRISE_TOKEN")
        .env_remove("GITHUB_ENTERPRISE_TOKEN")
        .env_remove("GH_DEBUG")
        .env_remove("GIT_CURL_VERBOSE")
        .env_remove("GIT_TRACE_CURL")
        .env_remove("GIT_TRACE_PACKET")
        .env_remove("GIT_TRACE_REDACT");
    // Git tracing can send HTTP headers and credential-helper output to files.
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_TRACE")
        {
            command.env_remove(name);
        }
    }
    // Local Git fixtures do not initialize the app or access real credentials.
    if ROOT.get().is_none() {
        #[cfg(test)]
        return Ok(());
        #[cfg(not(test))]
        return Err(std::io::Error::other("GitHub credentials unavailable"));
    }
    let token = crate::github_oauth::token().map_err(std::io::Error::other)?;
    command.env("GH_TOKEN", token);
    select(3);
    Ok(())
}
