use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
};
use tauri::{Manager, State};

static REMOTE_LOCK: Mutex<()> = Mutex::new(());
const REMOTE: &str = "momso";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsernameResult {
    state: &'static str,
    username: Option<String>,
}

pub(crate) fn valid_username(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 94
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && !value.to_ascii_lowercase().ends_with(".git")
        && !value.to_ascii_lowercase().ends_with(".wiki")
}

pub(crate) fn username_get_db(path: &Path) -> Result<Option<String>, ()> {
    Connection::open(path)
        .map_err(|_| ())?
        .query_row(
            "SELECT value FROM app_runtime_meta WHERE key='momso_username'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| ())
}

#[tauri::command]
pub(crate) async fn momso_username_get(
    state: State<'_, crate::AppState>,
) -> Result<UsernameResult, String> {
    Ok(match username_get_db(&state.paths.database_path) {
        Ok(username) => UsernameResult {
            state: "ok",
            username,
        },
        Err(()) => UsernameResult {
            state: "unavailable",
            username: None,
        },
    })
}

#[tauri::command]
pub(crate) async fn momso_username_set(
    state: State<'_, crate::AppState>,
    username: String,
) -> Result<UsernameResult, String> {
    let _lease = state.paths.archive.current.clone().read_owned().await;
    Ok(username_set_db(&state.paths.database_path, username))
}

pub(crate) fn username_set_db(path: &Path, username: String) -> UsernameResult {
    if !valid_username(&username) {
        return UsernameResult {
            state: "invalid_username",
            username: None,
        };
    }
    let result = Connection::open(path).and_then(|db| db.execute(
        "INSERT INTO app_runtime_meta (key,value,updated_at) VALUES ('momso_username',?1,CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at", [&username],
    ));
    match result {
        Ok(_) => UsernameResult {
            state: "ok",
            username: Some(username),
        },
        Err(_) => UsernameResult {
            state: "unavailable",
            username: None,
        },
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutoCreateResult {
    state: &'static str,
    repo: Option<String>,
}

#[derive(Debug)]
struct GhReply {
    code: Option<i32>,
    stdout: String,
}

fn run_gh(args: &[&str]) -> Result<GhReply, ()> {
    let output = hidden(&mut Command::new("gh"))
        .env("GH_HOST", "github.com")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| ())?;
    Ok(GhReply {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
    })
}

#[allow(dead_code)]
pub(crate) fn setup_candidate(db: &Path) -> Result<(String, String), &'static str> {
    let username = username_get_db(db)
        .map_err(|_| "unavailable")?
        .ok_or("username_missing")?;
    if !valid_username(&username) {
        return Err("invalid_username");
    }
    let auth = run_gh(&["auth", "status", "--active", "--hostname", "github.com"])
        .map_err(|_| "gh_missing")?;
    if auth.code != Some(0) {
        return Err("signed_out");
    }
    let response = run_gh(&["api", "--hostname", "github.com", "user", "--jq", ".login"])
        .map_err(|_| "unavailable")?;
    if response.code != Some(0) {
        return Err("unavailable");
    }
    let account = response.stdout.trim().to_owned();
    let name = format!("momso-{username}");
    if !valid_repo(&format!("{account}/{name}")) {
        return Err("unavailable");
    }
    Ok((account, name))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutoRepo {
    name_with_owner: String,
    is_private: bool,
    is_empty: bool,
    default_branch_ref: Option<DefaultBranch>,
}

fn auto_result(state: &'static str, repo: Option<String>) -> AutoCreateResult {
    AutoCreateResult { state, repo }
}

fn inspect_gh_repo(
    call: &mut impl FnMut(&[&str]) -> Result<GhReply, ()>,
    name: &str,
) -> Result<Option<AutoRepo>, &'static str> {
    let view = call(&[
        "repo",
        "view",
        name,
        "--json",
        "nameWithOwner,isPrivate,isEmpty,defaultBranchRef",
    ])
    .map_err(|_| "gh_missing")?;
    if view.code == Some(0) {
        let repo: AutoRepo = serde_json::from_str(&view.stdout).map_err(|_| "unavailable")?;
        if !repo.name_with_owner.eq_ignore_ascii_case(name) {
            return Err("unavailable");
        }
        return Ok(Some(repo));
    }
    let endpoint = format!("repos/{name}");
    let response = call(&["api", "--hostname", "github.com", "--include", &endpoint])
        .map_err(|_| "gh_missing")?;
    let status = response.stdout.lines().find_map(|line| {
        line.strip_prefix("HTTP/")
            .and_then(|rest| rest.split_whitespace().nth(1))
    });
    if status == Some("404") && response.code != Some(0) {
        Ok(None)
    } else {
        Err("unavailable")
    }
}

#[cfg(test)]
fn create_auto(
    username: Option<String>,
    call: impl FnMut(&[&str]) -> Result<GhReply, ()>,
    connected: impl FnMut() -> bool,
    classify: impl FnMut(&AutoRepo) -> &'static str,
    connect: impl FnMut(&str) -> bool,
) -> AutoCreateResult {
    create_auto_checked(username, None, || false, call, connected, classify, connect)
}

fn create_auto_checked(
    username: Option<String>,
    expected: Option<&str>,
    mut cancelled: impl FnMut() -> bool,
    mut call: impl FnMut(&[&str]) -> Result<GhReply, ()>,
    mut connected: impl FnMut() -> bool,
    mut classify: impl FnMut(&AutoRepo) -> &'static str,
    mut connect: impl FnMut(&str) -> bool,
) -> AutoCreateResult {
    let Some(username) = username else {
        return auto_result("username_missing", None);
    };
    if !valid_username(&username) {
        return auto_result("invalid_username", None);
    }
    if connected() {
        return auto_result("already_connected", None);
    }
    if call(&["--version"]).is_err() {
        return auto_result("gh_missing", None);
    }
    let auth = match call(&["auth", "status", "--active", "--hostname", "github.com"]) {
        Ok(reply) => reply,
        Err(_) => return auto_result("gh_missing", None),
    };
    if auth.code != Some(0) {
        return auto_result("signed_out", None);
    }
    let account = match call(&["api", "--hostname", "github.com", "user", "--jq", ".login"]) {
        Ok(reply) if reply.code == Some(0) => reply.stdout.trim().to_owned(),
        _ => return auto_result("unavailable", None),
    };
    let repo = format!("{account}/momso-{username}");
    if !valid_repo(&repo) {
        return auto_result("unavailable", None);
    }
    if expected.is_some_and(|value| value != repo) {
        return auto_result("approval_stale", None);
    }
    if cancelled() {
        return auto_result("cancelled", None);
    }
    match inspect_gh_repo(&mut call, &repo) {
        Ok(Some(existing)) => {
            let state = if expected.is_some() {
                "exists"
            } else if !existing.is_private {
                "exists_public"
            } else if existing.is_empty {
                "exists_empty"
            } else {
                classify(&existing)
            };
            return auto_result(state, Some(existing.name_with_owner));
        }
        Err(error) => return auto_result(error, None),
        Ok(None) => {}
    }
    if cancelled() {
        return auto_result("cancelled", None);
    }
    let created = match call(&["repo", "create", &repo, "--private"]) {
        Ok(reply) => reply,
        Err(_) => return auto_result("gh_missing", None),
    };
    if created.code != Some(0) {
        // A name can be claimed between lookup and creation. Inspect again without replacing it.
        if let Ok(Some(existing)) = inspect_gh_repo(&mut call, &repo) {
            let state = if expected.is_some() {
                "exists"
            } else if !existing.is_private {
                "exists_public"
            } else if existing.is_empty {
                "exists_empty"
            } else {
                classify(&existing)
            };
            return auto_result(state, Some(existing.name_with_owner));
        }
        return auto_result("create_failed", None);
    }
    match inspect_gh_repo(&mut call, &repo) {
        Ok(Some(existing)) if existing.is_private && existing.is_empty => {
            if cancelled() {
                return auto_result("created_not_connected", Some(existing.name_with_owner));
            }
            if connect(&existing.name_with_owner) {
                auto_result("created", Some(existing.name_with_owner))
            } else {
                auto_result("created_not_connected", Some(existing.name_with_owner))
            }
        }
        _ => auto_result("created_unverified", Some(repo)),
    }
}

fn classify_existing(tool: &Path, root: &Path, repo: &AutoRepo) -> &'static str {
    if git(
        tool,
        root,
        &[
            "config",
            "--local",
            "--includes",
            "--get-regexp",
            "^url\\..*\\.(insteadof|pushinsteadof)$",
        ],
    )
    .is_ok()
    {
        return "exists_private_unknown";
    }
    let Some(branch) = repo
        .default_branch_ref
        .as_ref()
        .map(|value| value.name.as_str())
    else {
        return "exists_private_unknown";
    };
    if !valid_branch(branch) {
        return "exists_private_unknown";
    }
    let remote = url(&repo.name_with_owner);
    let reference = format!("refs/heads/{branch}");
    let Ok(response) = git(tool, root, &["ls-remote", "--heads", &remote, &reference]) else {
        return "exists_private_unknown";
    };
    let Some((revision, name)) = response.split_once('\t') else {
        return "exists_private_unknown";
    };
    if name != reference
        || !matches!(revision.len(), 40 | 64)
        || !revision.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return "exists_private_unknown";
    }
    if is_known_archive_history(tool, root, revision) {
        "exists_matching"
    } else {
        "exists_private_unknown"
    }
}

fn is_known_archive_history(tool: &Path, root: &Path, revision: &str) -> bool {
    git(
        tool,
        root,
        &["merge-base", "--is-ancestor", revision, "HEAD"],
    )
    .is_ok()
        && safe_history(tool, root, revision).is_ok()
}

#[tauri::command]
pub(crate) async fn archive_remote_create(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
) -> Result<AutoCreateResult, String> {
    create_internal(app, state, None, None).await
}

#[allow(dead_code)]
pub(crate) async fn create_approved(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
    chat_id: &str,
    permit: crate::setup_approval::SetupPermit,
    cancellation: tokio_util::sync::CancellationToken,
) -> Result<AutoCreateResult, String> {
    let details = permit.consume(chat_id).map_err(str::to_owned)?;
    if details.service != "github" || cancellation.is_cancelled() {
        return Ok(auto_result("cancelled", None));
    }
    let expected = format!("{}/{}", details.account, details.resource_name);
    if !valid_repo(&expected) {
        return Ok(auto_result("unavailable", None));
    }
    create_internal(app, state, Some(expected), Some(cancellation)).await
}

async fn create_internal(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
    expected: Option<String>,
    cancellation: Option<tokio_util::sync::CancellationToken>,
) -> Result<AutoCreateResult, String> {
    let Ok((root, resources, db)) = command_paths(&app, &state).await else {
        return Ok(auto_result("unavailable", None));
    };
    Ok(tauri::async_runtime::spawn_blocking(move || {
        let Ok(_guard) = REMOTE_LOCK.lock() else {
            return auto_result("unavailable", None);
        };
        let username = match username_get_db(&db) {
            Ok(value) => value,
            Err(()) => return auto_result("unavailable", None),
        };
        if run_gh(&["--version"]).is_err() {
            return auto_result("gh_missing", None);
        }
        match run_gh(&["auth", "status", "--active", "--hostname", "github.com"]) {
            Err(()) => return auto_result("gh_missing", None),
            Ok(reply) if reply.code != Some(0) => return auto_result("signed_out", None),
            _ => {}
        }
        let Some(tool) = git_path(&resources) else {
            return auto_result("git_missing", None);
        };
        let result = create_auto_checked(
            username,
            expected.as_deref(),
            || cancellation.as_ref().is_some_and(|v| v.is_cancelled()),
            run_gh,
            || git(&tool, &root, &["remote", "get-url", REMOTE]).is_ok(),
            |repo| classify_existing(&tool, &root, repo),
            |repo| connect_local(&tool, &root, &url(repo)).is_ok(),
        );
        if result.state == "exists_private_unknown" && fresh_archive(&tool, &root) {
            let Some(repo) = result.repo.as_deref() else {
                return result;
            };
            let Ok((remote, _)) = github(repo) else {
                return result;
            };
            let branch = remote
                .default_branch_ref
                .as_ref()
                .map(|x| x.name.as_str())
                .unwrap_or("main");
            let target = url(&remote.name_with_owner);
            if connect_local(&tool, &root, &target).is_err() {
                return result;
            }
            if receive_connected(&tool, &root, &db, &target, branch).is_err() {
                return auto_result("unavailable", Some(remote.name_with_owner));
            }
            return auto_result("connected", Some(remote.name_with_owner));
        }
        result
    })
    .await
    .unwrap_or_else(|_| auto_result("unavailable", None)))
}

fn hidden(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteStatus {
    state: &'static str,
    reason: Option<&'static str>,
    history_reason: Option<&'static str>,
    repo: Option<String>,
    visibility: Option<String>,
    last_pushed_at: Option<String>,
    ahead: Option<u64>,
    behind: Option<u64>,
    signed_in_as: Option<String>,
}

impl RemoteStatus {
    fn new(state: &'static str, reason: Option<&'static str>) -> Self {
        Self {
            state,
            reason,
            history_reason: None,
            repo: None,
            visibility: None,
            last_pushed_at: None,
            ahead: None,
            behind: None,
            signed_in_as: None,
        }
    }

    fn transfer_error(error: &'static str) -> Self {
        let state = if matches!(
            error,
            "remote_ahead" | "diverged" | "unsaved_changes" | "unsafe_history"
        ) {
            "connected"
        } else {
            "unavailable"
        };
        Self::new(state, Some(error))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    name_with_owner: String,
    is_private: bool,
    default_branch_ref: Option<DefaultBranch>,
}
#[derive(Debug, Deserialize)]
struct DefaultBranch {
    name: String,
}

fn valid_repo(repo: &str) -> bool {
    let parts: Vec<_> = repo.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|s| {
            !s.is_empty()
                && *s != "."
                && *s != ".."
                && !s.starts_with('-')
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}
fn valid_branch(branch: &str) -> bool {
    !branch.is_empty()
        && !branch.starts_with('-')
        && !branch.starts_with('/')
        && !branch.ends_with('/')
        && !branch.contains("..")
        && !branch.contains("@{")
        && !branch.ends_with('.')
        && !branch.ends_with(".lock")
        && branch
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'/' | b'.'))
}
fn git_path(_resources: &Path) -> Option<PathBuf> {
    crate::git_tool::resolve().map(|(path, _)| path)
}
fn git_raw(tool: &Path, root: &Path, args: &[&str]) -> Result<String, ()> {
    let output = hidden(&mut Command::new(tool))
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GCM_INTERACTIVE", "Never")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .args([
            "-c",
            "credential.helper=",
            "-c",
            "credential.https://github.com.helper=!gh auth git-credential",
            "-c",
            "http.followRedirects=false",
            "-c",
            "protocol.ext.allow=never",
        ])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| ())?;
    if !output.status.success() {
        return Err(());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
pub(crate) fn git(tool: &Path, root: &Path, args: &[&str]) -> Result<String, ()> {
    git_raw(tool, root, args).map(|s| s.trim().to_owned())
}
fn gh(args: &[&str]) -> Result<String, ()> {
    let result = hidden(&mut Command::new("gh"))
        .env("GH_HOST", "github.com")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| ())?;
    if !result.status.success() {
        return Err(());
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_owned())
}
fn signed_in() -> Result<String, &'static str> {
    if hidden(&mut Command::new("gh"))
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        return Err("gh_missing");
    }
    gh(&["auth", "status", "--active", "--hostname", "github.com"]).map_err(|_| "signed_out")?;
    gh(&["api", "--hostname", "github.com", "user", "--jq", ".login"])
        .map_err(|_| "account_unavailable")
}
fn github(repo: &str) -> Result<(Repository, String), &'static str> {
    if !valid_repo(repo) {
        return Err("invalid_repository");
    }
    let user = signed_in()?;
    let data = gh(&[
        "repo",
        "view",
        repo,
        "--json",
        "nameWithOwner,isPrivate,defaultBranchRef",
    ])
    .map_err(|_| "repository_unavailable")?;
    Ok((parse_private_repository(&data)?, user))
}
fn parse_private_repository(data: &str) -> Result<Repository, &'static str> {
    let repo: Repository = serde_json::from_str(data).map_err(|_| "repository_unavailable")?;
    if !repo.is_private {
        return Err("repository_not_private");
    }
    if !valid_repo(&repo.name_with_owner) {
        return Err("invalid_repository");
    }
    Ok(repo)
}
fn url(repo: &str) -> String {
    format!("https://github.com/{repo}.git")
}
fn configured(tool: &Path, root: &Path) -> Option<String> {
    let raw = git(
        tool,
        root,
        &["config", "--local", "--get", "remote.momso.url"],
    )
    .ok()?;
    let repo = raw
        .strip_prefix("https://github.com/")?
        .strip_suffix(".git")?;
    valid_repo(repo).then(|| repo.to_owned())
}
pub(crate) fn connect_local(tool: &Path, root: &Path, target: &str) -> Result<(), &'static str> {
    if git(tool, root, &["remote", "get-url", REMOTE]).is_ok() {
        return Err("already_connected");
    }
    git(tool, root, &["remote", "add", REMOTE, target]).map_err(|_| "git_unavailable")?;
    Ok(())
}
pub(crate) fn receive_connected(
    tool: &Path,
    root: &Path,
    db: &Path,
    target: &str,
    branch: &str,
) -> Result<(), &'static str> {
    transfer(tool, root, target, branch, "pull")?;
    crate::archive::reindex(root, db).map_err(|_| "reindex_failed")?;
    Ok(())
}
fn safe_history(tool: &Path, root: &Path, revision: &str) -> Result<(), &'static str> {
    if git(tool, root, &["rev-parse", "--is-shallow-repository"]).as_deref() != Ok("false") {
        return Err("unsafe_history");
    }
    let Ok(list) = git_raw(
        tool,
        root,
        &[
            "log",
            "--format=",
            "--name-only",
            "-m",
            "--root",
            "--no-renames",
            "-z",
            revision,
        ],
    ) else {
        return Err("unsafe_history");
    };
    if list.split('\0').all(|p| {
        p.is_empty()
            || matches!(
                p,
                "zettels"
                    | "receipts"
                    | ".gitignore"
                    | "AGENTS.md"
                    | "CLAUDE.md"
                    | "GEMINI.md"
                    | "profile.md"
            )
            || safe_skill_path(p)
            || crate::yoga_dictionary::is_path(p)
            || ((p.starts_with("zettels/") || p.starts_with("receipts/"))
                && p.ends_with(".md")
                && !p.contains("..")
                && !p.contains('\\'))
    }) {
        Ok(())
    } else {
        Err("unsafe_history")
    }
}
fn safe_skill_path(path: &str) -> bool {
    ["skills/", ".agents/skills/", ".claude/skills/"]
        .iter()
        .any(|prefix| {
            path.strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix("/SKILL.md"))
                .is_some_and(crate::skills::valid_name)
        })
}
fn last_push(root: &Path, target: &str) -> Option<String> {
    let text = fs::read_to_string(root.join(".git/momso-last-push")).ok()?;
    let (saved_target, time) = text.split_once('\n')?;
    (saved_target == target && !time.trim().is_empty()).then(|| time.trim().to_owned())
}
fn relation(
    tool: &Path,
    root: &Path,
    target: &str,
    branch: &str,
) -> Result<(u64, u64), &'static str> {
    if !valid_branch(branch) {
        return Err("invalid_branch");
    }
    if git(
        tool,
        root,
        &[
            "config",
            "--local",
            "--includes",
            "--get-regexp",
            "^url\\..*\\.(insteadof|pushinsteadof)$",
        ],
    )
    .is_ok()
    {
        return Err("unsafe_git_configuration");
    }
    let remote_ref = format!("refs/heads/{branch}");
    let remote = git(tool, root, &["ls-remote", "--heads", target, &remote_ref])
        .map_err(|_| "remote_unavailable")?;
    if remote.is_empty() {
        let count = git(tool, root, &["rev-list", "--count", "HEAD"])
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        return Ok((count, 0));
    }
    let fetch_ref = format!("+{remote_ref}:refs/remotes/momso/observed");
    git(tool, root, &["fetch", "--no-tags", target, &fetch_ref])
        .map_err(|_| "remote_unavailable")?;
    if git(tool, root, &["rev-parse", "--verify", "HEAD"]).is_err() {
        return Ok((0, 1));
    }
    let text = git(
        tool,
        root,
        &[
            "rev-list",
            "--left-right",
            "--count",
            "HEAD...refs/remotes/momso/observed",
        ],
    )
    .map_err(|_| "remote_unavailable")?;
    let mut nums = text
        .split_whitespace()
        .filter_map(|x| x.parse::<u64>().ok());
    Ok((
        nums.next().ok_or("remote_unavailable")?,
        nums.next().ok_or("remote_unavailable")?,
    ))
}

// An unborn repository is eligible only if every visible and ignored file is
// an untouched file created by archive::initialize. Never infer this from
// git status alone: objects/ and other user files can be ignored by Git.
fn default_file_unchanged(path: &Path, name: &str) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_file() || crate::archive_location::is_link_or_reparse(&metadata) {
        return false;
    }
    let Ok(contents) = fs::read_to_string(path) else {
        return false;
    };
    let expected = if name == ".gitignore" {
        "objects/\n"
    } else {
        include_str!("archive_rules.md")
    };
    contents.replace("\r\n", "\n") == expected.replace("\r\n", "\n")
}

// First receive moves only exact bundled files aside and preserves them; any
// additional file, edited byte, or link blocks first receive.
fn default_skills_unchanged(root: &Path, folder: &str) -> bool {
    let base = root.join(folder);
    for entry in walkdir::WalkDir::new(&base).follow_links(false) {
        let Ok(entry) = entry else { return false };
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            return false;
        };
        if crate::archive_location::is_link_or_reparse(&meta) {
            return false;
        }
        let Ok(relative) = entry.path().strip_prefix(&base) else {
            return false;
        };
        let parts: Vec<_> = relative.iter().collect();
        let prefix: &[&str] = if folder == "skills" { &[] } else { &["skills"] };
        let matches_prefix =
            parts.len() >= prefix.len() && prefix.iter().zip(&parts).all(|(a, b)| *b == *a);
        if relative.as_os_str().is_empty() {
            if !meta.is_dir() {
                return false;
            }
        } else if !matches_prefix {
            return false;
        } else if parts.len() == prefix.len() {
            if !meta.is_dir() {
                return false;
            }
        } else if parts.len() == prefix.len() + 1 {
            if !meta.is_dir()
                || !crate::skills::BUNDLED_SKILLS
                    .iter()
                    .any(|(name, _)| parts[prefix.len()] == *name)
            {
                return false;
            }
        } else if parts.len() == prefix.len() + 2 {
            let Some((_, content)) = crate::skills::BUNDLED_SKILLS
                .iter()
                .find(|(name, _)| parts[prefix.len()] == *name)
            else {
                return false;
            };
            if parts[prefix.len() + 1] != "SKILL.md"
                || !meta.is_file()
                || !fs::read(entry.path()).is_ok_and(|bytes| bytes == content.as_bytes())
            {
                return false;
            }
        } else {
            return false;
        }
    }
    let skill_root = if folder == "skills" {
        base
    } else {
        base.join("skills")
    };
    crate::skills::BUNDLED_SKILLS.iter().all(|(name, content)| {
        let file = skill_root.join(*name).join("SKILL.md");
        fs::symlink_metadata(&file)
            .is_ok_and(|meta| meta.is_file() && !crate::archive_location::is_link_or_reparse(&meta))
            && fs::read(file).is_ok_and(|bytes| bytes == content.as_bytes())
    })
}

fn copy_recovery_if_absent(source: &Path, target: &Path) -> Result<(), &'static str> {
    let mut original = fs::File::open(source).map_err(|_| "local_state_unavailable")?;
    let mut copy = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(_) => return Err("local_state_unavailable"),
    };
    std::io::copy(&mut original, &mut copy).map_err(|_| "local_state_unavailable")?;
    copy.sync_all().map_err(|_| "local_state_unavailable")
}

fn restore_missing_default_skills(
    tool: &Path,
    saved: &Path,
    root: &Path,
    folder: &str,
) -> Result<(), &'static str> {
    let prefix = if folder == "skills" {
        "skills".to_owned()
    } else {
        format!("{folder}/skills")
    };
    for &(name, _) in crate::skills::BUNDLED_SKILLS {
        if folder != "skills" {
            let canonical = format!("skills/{name}/SKILL.md");
            if !git(tool, root, &["ls-files", "--", &canonical])
                .map_err(|_| "local_state_unavailable")?
                .trim()
                .is_empty()
            {
                continue;
            }
        }
        let target = crate::skills::skill_path(root, &prefix, name)
            .map_err(|_| "local_state_unavailable")?;
        if fs::symlink_metadata(&target).is_ok() {
            continue;
        }
        fs::create_dir_all(target.parent().ok_or("local_state_unavailable")?)
            .map_err(|_| "local_state_unavailable")?;
        let target = crate::skills::skill_path(root, &prefix, name)
            .map_err(|_| "local_state_unavailable")?;
        let source = saved.join(&prefix).join(name).join("SKILL.md");
        copy_recovery_if_absent(&source, &target)?;
    }
    Ok(())
}

fn fresh_archive(tool: &Path, root: &Path) -> bool {
    let Ok(has_defaults) = crate::skills_write::fresh_builtin_state_mode(root) else {
        return false;
    };
    if git(tool, root, &["rev-parse", "--verify", "HEAD"]).is_ok()
        || !git(tool, root, &["ls-files", "--stage"]).is_ok_and(|s| s.is_empty())
        || (has_defaults
            && !["skills", ".agents", ".claude"]
                .iter()
                .all(|folder| default_skills_unchanged(root, folder)))
    {
        return false;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    for entry in entries {
        let Ok(entry) = entry else { return false };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return false;
        };
        let Ok(kind) = entry.file_type() else {
            return false;
        };
        if !fs::symlink_metadata(entry.path())
            .is_ok_and(|m| !crate::archive_location::is_link_or_reparse(&m))
        {
            return false;
        }
        if name == ".git" {
            if !kind.is_dir() || kind.is_symlink() {
                return false;
            }
        } else if ["zettels", "receipts", "objects"].contains(&name) {
            if !kind.is_dir()
                || kind.is_symlink()
                || !fs::read_dir(entry.path()).is_ok_and(|mut entries| entries.next().is_none())
            {
                return false;
            }
        } else if name == crate::yoga_dictionary::DIRECTORY {
            if !kind.is_dir() || !crate::yoga_dictionary::defaults_unchanged(root) {
                return false;
            }
        } else if ["skills", ".agents", ".claude"].contains(&name) {
            if !has_defaults || !kind.is_dir() || !default_skills_unchanged(root, name) {
                return false;
            }
        } else if ["AGENTS.md", "CLAUDE.md", "GEMINI.md", ".gitignore"].contains(&name) {
            if !kind.is_file() || kind.is_symlink() {
                return false;
            }
            if !default_file_unchanged(&entry.path(), name) {
                return false;
            }
        } else {
            return false;
        }
    }
    true
}

fn first_receive(tool: &Path, root: &Path, revision: &str) -> Result<(), &'static str> {
    first_receive_with_hook(tool, root, revision, || {})
}

fn first_receive_with_hook(
    tool: &Path,
    root: &Path,
    revision: &str,
    after_check: impl FnOnce(),
) -> Result<(), &'static str> {
    let _write = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "unavailable")?;
    if !fresh_archive(tool, root) {
        return Err("unsaved_changes");
    }
    let has_defaults =
        crate::skills_write::fresh_builtin_state_mode(root).map_err(|_| "unsaved_changes")?;
    after_check();
    let saved = root
        .join(".git")
        .join(format!("momso-first-pull-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&saved).map_err(|_| "local_state_unavailable")?;
    let mut moved = Vec::new();
    let result = (|| {
        for name in [
            ".gitignore",
            "AGENTS.md",
            "CLAUDE.md",
            "GEMINI.md",
            "skills",
            ".agents",
            ".claude",
            crate::yoga_dictionary::DIRECTORY,
        ] {
            if root.join(name).exists() {
                fs::rename(root.join(name), saved.join(name))
                    .map_err(|_| "local_state_unavailable")?;
                moved.push(name);
                let unchanged = if name == crate::yoga_dictionary::DIRECTORY {
                    crate::yoga_dictionary::defaults_unchanged(&saved)
                } else if ["skills", ".agents", ".claude"].contains(&name) {
                    default_skills_unchanged(&saved, name)
                } else {
                    default_file_unchanged(&saved.join(name), name)
                };
                if !unchanged {
                    return Err("unsaved_changes");
                }
            }
        }
        if crate::skills_write::fresh_builtin_state_mode(root) != Ok(has_defaults) {
            return Err("unsaved_changes");
        }
        git(tool, root, &["merge", "--ff-only", revision]).map_err(|_| "fast_forward_failed")?;
        if has_defaults {
            crate::skills_write::adopt_received_builtin_states(tool, root, &saved)
                .map_err(|_| "local_state_unavailable")?;
        }
        Ok(())
    })();
    if result.is_err() {
        for name in moved {
            if [
                "skills",
                ".agents",
                ".claude",
                crate::yoga_dictionary::DIRECTORY,
            ]
            .contains(&name)
            {
                if !root.join(name).exists() {
                    let _ = fs::rename(saved.join(name), root.join(name));
                }
                continue;
            }
            // Keep the saved original if Git or another process created the path.
            if fs::hard_link(saved.join(name), root.join(name)).is_ok() {
                let _ = fs::remove_file(saved.join(name));
            }
        }
    } else {
        for name in moved {
            if ["skills", ".agents", ".claude"].contains(&name) {
                restore_missing_default_skills(tool, &saved, root, name)?;
                continue;
            }
            if name == crate::yoga_dictionary::DIRECTORY {
                let target_dir = root.join(name);
                fs::create_dir_all(&target_dir).map_err(|_| "local_state_unavailable")?;
                let meta =
                    fs::symlink_metadata(&target_dir).map_err(|_| "local_state_unavailable")?;
                if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
                    return Err("local_state_unavailable");
                }
                for relative in crate::yoga_dictionary::paths() {
                    copy_recovery_if_absent(&saved.join(&relative), &root.join(&relative))?;
                }
                continue;
            }
            // Keep the original as a recovery copy: another process may still
            // hold it open and write after any content check.
            copy_recovery_if_absent(&saved.join(name), &root.join(name))?;
        }
    }
    if result.is_ok() {
        crate::yoga_dictionary::initialize(root).map_err(|_| "local_state_unavailable")?;
    }
    let _ = fs::remove_dir(&saved);
    result
}

pub(crate) fn transfer(
    tool: &Path,
    root: &Path,
    target: &str,
    branch: &str,
    action: &str,
) -> Result<(), &'static str> {
    let (ahead, behind) = relation(tool, root, target, branch)?;
    if action == "push" {
        if behind > 0 {
            return Err(if ahead > 0 {
                "diverged"
            } else {
                "remote_ahead"
            });
        }
        let revision =
            git(tool, root, &["rev-parse", "--verify", "HEAD"]).map_err(|_| "nothing_saved")?;
        safe_history(tool, root, &revision)?;
        let dest = format!("{revision}:refs/heads/{branch}");
        git(tool, root, &["push", "--no-follow-tags", target, &dest])
            .map_err(|_| "remote_unavailable")?;
        fs::write(
            root.join(".git/momso-last-push"),
            format!("{target}\n{}", chrono::Utc::now().to_rfc3339()),
        )
        .map_err(|_| "local_state_unavailable")?;
    } else {
        let unborn = git(tool, root, &["rev-parse", "--verify", "HEAD"]).is_err();
        if unborn && behind > 0 {
            let revision = git(
                tool,
                root,
                &["rev-parse", "--verify", "refs/remotes/momso/observed"],
            )
            .map_err(|_| "remote_unavailable")?;
            safe_history(tool, root, &revision)?;
            let _guard = crate::chat_file_changes::lock_app_write();
            return first_receive(tool, root, &revision);
        }
        let dirty = git(
            tool,
            root,
            &["status", "--porcelain", "--untracked-files=all"],
        )
        .map_err(|_| "git_unavailable")?;
        let clean_except_default_ignore = dirty.lines().all(|line| {
            line == "?? .gitignore"
                && fs::read(root.join(".gitignore"))
                    .is_ok_and(|contents| contents == b"objects/\n" || contents == b"objects/\r\n")
        });
        if !dirty.is_empty() && !clean_except_default_ignore {
            return Err("unsaved_changes");
        }
        if ahead > 0 && behind > 0 {
            return Err("diverged");
        }
        if behind == 0 {
            return Ok(());
        }
        let revision = git(
            tool,
            root,
            &["rev-parse", "--verify", "refs/remotes/momso/observed"],
        )
        .map_err(|_| "remote_unavailable")?;
        safe_history(tool, root, &revision)?;
        let _guard = crate::chat_file_changes::lock_app_write();
        if !crate::chat_file_changes::has_active(root) {
            return git(tool, root, &["merge", "--ff-only", &revision])
                .map(|_| ())
                .map_err(|_| "fast_forward_failed");
        }
        let before = crate::chat_file_changes::capture_unlocked(root);
        let result = git(tool, root, &["merge", "--ff-only", &revision]);
        let after = crate::chat_file_changes::capture_unlocked(root);
        if let (Ok(before), Ok(after)) = (before, after) {
            for id in crate::chat_file_changes::changes(&before, &after).keys() {
                crate::chat_file_changes::app_write(root, Some(id));
            }
        } else {
            crate::chat_file_changes::app_write(root, None);
        }
        result.map_err(|_| "fast_forward_failed")?;
    }
    Ok(())
}
fn status(root: &Path, resources: &Path) -> RemoteStatus {
    let Some(tool) = git_path(resources) else {
        return RemoteStatus::new("unavailable", Some("git_missing"));
    };
    let Some(repo) = configured(&tool, root) else {
        return match signed_in() {
            Ok(user) => {
                let mut s = RemoteStatus::new("not_connected", None);
                s.signed_in_as = Some(user);
                s
            }
            Err(e) => RemoteStatus::new("unavailable", Some(e)),
        };
    };
    let mut result = RemoteStatus::new("unavailable", None);
    result.repo = Some(repo.clone());
    result.last_pushed_at = last_push(root, &url(&repo));
    let (remote, user) = match github(&repo) {
        Ok(x) => x,
        Err(e) => {
            result.reason = Some(e);
            return result;
        }
    };
    result.signed_in_as = Some(user);
    result.visibility = Some("private".into());
    let branch = remote
        .default_branch_ref
        .map(|x| x.name)
        .unwrap_or_else(|| "main".into());
    match relation(&tool, root, &url(&remote.name_with_owner), &branch) {
        Ok((ahead, behind)) => {
            result.state = "connected";
            result.ahead = Some(ahead);
            result.behind = Some(behind)
        }
        Err(e) => result.reason = Some(e),
    }
    result
}
async fn command_paths(
    app: &tauri::AppHandle,
    state: &State<'_, crate::AppState>,
) -> Result<(tokio::sync::OwnedRwLockReadGuard<PathBuf>, PathBuf, PathBuf), ()> {
    Ok((
        state.paths.archive.current.clone().read_owned().await,
        app.path().resource_dir().map_err(|_| ())?,
        state.paths.database_path.clone(),
    ))
}
async fn status_impl(app: tauri::AppHandle, state: State<'_, crate::AppState>) -> RemoteStatus {
    let Ok((root, resources, _)) = command_paths(&app, &state).await else {
        return RemoteStatus::new("unavailable", Some("local_state_unavailable"));
    };
    tauri::async_runtime::spawn_blocking(move || {
        let Ok(_guard) = REMOTE_LOCK.lock() else {
            return RemoteStatus::new("unavailable", Some("busy"));
        };
        status(&root, &resources)
    })
    .await
    .unwrap_or_else(|_| RemoteStatus::new("unavailable", Some("local_state_unavailable")))
}
async fn connect_impl(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
    repo: String,
) -> RemoteStatus {
    let Ok((root, resources, db)) = command_paths(&app, &state).await else {
        return RemoteStatus::new("unavailable", Some("local_state_unavailable"));
    };
    tauri::async_runtime::spawn_blocking(move || {
        let Ok(_guard) = REMOTE_LOCK.lock() else {
            return RemoteStatus::new("unavailable", Some("busy"));
        };
        let Some(tool) = git_path(&resources) else {
            return RemoteStatus::new("unavailable", Some("git_missing"));
        };
        let (remote, _) = match github(&repo) {
            Ok(x) => x,
            Err(e) => return RemoteStatus::new("unavailable", Some(e)),
        };
        if let Err(e) = connect_local(&tool, &root, &url(&remote.name_with_owner)) {
            return RemoteStatus::new("unavailable", Some(e));
        }
        if git(&tool, &root, &["rev-parse", "--verify", "HEAD"]).is_err() {
            let branch = remote
                .default_branch_ref
                .as_ref()
                .map(|x| x.name.as_str())
                .unwrap_or("main");
            if let Err(e) =
                receive_connected(&tool, &root, &db, &url(&remote.name_with_owner), branch)
            {
                return RemoteStatus::transfer_error(e);
            }
        }
        status(&root, &resources)
    })
    .await
    .unwrap_or_else(|_| RemoteStatus::new("unavailable", Some("local_state_unavailable")))
}
async fn run_transfer(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
    action: &'static str,
) -> RemoteStatus {
    let Ok((root, resources, db)) = command_paths(&app, &state).await else {
        return RemoteStatus::new("unavailable", Some("local_state_unavailable"));
    };
    tauri::async_runtime::spawn_blocking(move || {
        let Ok(_guard) = REMOTE_LOCK.lock() else {
            return RemoteStatus::new("unavailable", Some("busy"));
        };
        let Some(tool) = git_path(&resources) else {
            return RemoteStatus::new("unavailable", Some("git_missing"));
        };
        let Some(repo) = configured(&tool, &root) else {
            return RemoteStatus::new("not_connected", None);
        };
        let (remote, user) = match github(&repo) {
            Ok(x) => x,
            Err(e) => return RemoteStatus::new("unavailable", Some(e)),
        };
        let branch = remote
            .default_branch_ref
            .map(|x| x.name)
            .unwrap_or_else(|| "main".into());
        if let Err(e) = transfer(&tool, &root, &url(&remote.name_with_owner), &branch, action) {
            let mut s = RemoteStatus::transfer_error(e);
            s.repo = Some(remote.name_with_owner.clone());
            s.visibility = Some("private".into());
            s.signed_in_as = Some(user);
            s.last_pushed_at = last_push(&root, &url(&remote.name_with_owner));
            return s;
        }
        if action == "pull" && crate::archive::reindex(&root, &db).is_err() {
            return RemoteStatus::new("unavailable", Some("reindex_failed"));
        }
        status(&root, &resources)
    })
    .await
    .unwrap_or_else(|_| RemoteStatus::new("unavailable", Some("local_state_unavailable")))
}
#[tauri::command]
pub(crate) async fn archive_remote_push(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
) -> Result<RemoteStatus, String> {
    Ok(run_transfer(app, state, "push").await)
}
#[tauri::command]
pub(crate) async fn archive_remote_pull(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
) -> Result<RemoteStatus, String> {
    Ok(run_transfer(app, state, "pull").await)
}
async fn disconnect_impl(app: tauri::AppHandle, state: State<'_, crate::AppState>) -> RemoteStatus {
    let Ok((root, resources, _)) = command_paths(&app, &state).await else {
        return RemoteStatus::new("unavailable", Some("local_state_unavailable"));
    };
    tauri::async_runtime::spawn_blocking(move || {
        let Ok(_guard) = REMOTE_LOCK.lock() else {
            return RemoteStatus::new("unavailable", Some("busy"));
        };
        let Some(tool) = git_path(&resources) else {
            return RemoteStatus::new("unavailable", Some("git_missing"));
        };
        if git(&tool, &root, &["remote", "get-url", REMOTE]).is_ok()
            && git(&tool, &root, &["remote", "remove", REMOTE]).is_err()
        {
            return RemoteStatus::new("unavailable", Some("git_unavailable"));
        }
        RemoteStatus::new("not_connected", None)
    })
    .await
    .unwrap_or_else(|_| RemoteStatus::new("unavailable", Some("local_state_unavailable")))
}

#[tauri::command]
pub(crate) async fn archive_remote_status(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
) -> Result<RemoteStatus, String> {
    Ok(status_impl(app, state).await)
}
#[tauri::command]
pub(crate) async fn archive_remote_connect(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
    repo: String,
) -> Result<RemoteStatus, String> {
    Ok(connect_impl(app, state, repo).await)
}
#[tauri::command]
pub(crate) async fn archive_remote_disconnect(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
) -> Result<RemoteStatus, String> {
    Ok(disconnect_impl(app, state).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(code: i32, stdout: &str) -> Result<GhReply, ()> {
        Ok(GhReply {
            code: Some(code),
            stdout: stdout.into(),
        })
    }

    #[test]
    fn approved_creation_refuses_changed_account_or_cancel_before_remote_changes() {
        let mut calls = Vec::new();
        let result = create_auto_checked(
            Some("teacher".into()),
            Some("expected/momso-teacher"),
            || false,
            |args| {
                calls.push(args.join(" "));
                if args.ends_with(&[".login"]) {
                    reply(0, "different")
                } else {
                    reply(0, "ok")
                }
            },
            || false,
            |_| "exists_private_unknown",
            |_| panic!("must not connect"),
        );
        assert_eq!(result.state, "approval_stale");
        assert!(!calls.iter().any(|v| v.starts_with("repo ")));
        let result = create_auto_checked(
            Some("teacher".into()),
            Some("expected/momso-teacher"),
            || true,
            |args| {
                if args.ends_with(&[".login"]) {
                    reply(0, "expected")
                } else {
                    reply(0, "ok")
                }
            },
            || false,
            |_| "exists_private_unknown",
            |_| panic!("must not connect"),
        );
        assert_eq!(result.state, "cancelled");
    }

    #[test]
    fn approved_creation_never_reads_existing_repository_contents() {
        let result = create_auto_checked(
            Some("teacher".into()),
            Some("owner/momso-teacher"),
            || false,
            |args| {
                if args.ends_with(&[".login"]) {
                    return reply(0, "owner");
                }
                if args.starts_with(&["repo", "view"]) {
                    return reply(
                        0,
                        r#"{"nameWithOwner":"owner/momso-teacher","isPrivate":true,"isEmpty":false,"defaultBranchRef":{"name":"main"}}"#,
                    );
                }
                reply(0, "ok")
            },
            || false,
            |_| panic!("must not read existing history"),
            |_| panic!("must not connect existing repository"),
        );
        assert_eq!(result.state, "exists");
    }

    #[test]
    fn username_is_separate_and_uses_github_name_limits() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("test.db");
        Connection::open(&db).unwrap().execute_batch("CREATE TABLE app_runtime_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL,updated_at TEXT NOT NULL)").unwrap();
        assert_eq!(username_get_db(&db).unwrap(), None);
        assert_eq!(username_set_db(&db, "Ab9-_.".into()).state, "ok");
        assert_eq!(username_get_db(&db).unwrap().as_deref(), Some("Ab9-_."));
        for name in ["", "한글", "a b", "a/b", "x.git", "x.wiki"] {
            assert_eq!(username_set_db(&db, name.into()).state, "invalid_username");
        }
        assert!(valid_username(&"a".repeat(94)));
        assert!(!valid_username(&"a".repeat(95)));
        assert_eq!(username_get_db(&db).unwrap().as_deref(), Some("Ab9-_."));
    }

    #[test]
    fn auto_create_uses_private_flag_and_verifies_before_connecting() {
        let mut calls = Vec::new();
        let mut linked = Vec::new();
        let mut view_count = 0;
        let result = create_auto(
            Some("tester".into()),
            |args| {
                calls.push(args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
                if args == ["--version"] || args.starts_with(&["auth", "status"]) {
                    return reply(0, "ok");
                }
                if args.ends_with(&[".login"]) {
                    return reply(0, "owner\n");
                }
                if args.starts_with(&["repo", "view"]) {
                    view_count += 1;
                    return if view_count == 1 {
                        reply(1, "")
                    } else {
                        reply(
                            0,
                            r#"{"nameWithOwner":"owner/momso-tester","isPrivate":true,"isEmpty":true,"defaultBranchRef":null}"#,
                        )
                    };
                }
                if args.starts_with(&["api", "--hostname", "github.com", "--include"]) {
                    return reply(1, "HTTP/2.0 404 Not Found\r\n");
                }
                if args.starts_with(&["repo", "create"]) {
                    return reply(0, "created");
                }
                panic!("unexpected fake command");
            },
            || false,
            |_| "exists_private_unknown",
            |repo| {
                linked.push(repo.to_owned());
                true
            },
        );
        assert_eq!(result.state, "created");
        assert_eq!(result.repo.as_deref(), Some("owner/momso-tester"));
        assert_eq!(linked, ["owner/momso-tester"]);
        assert!(calls
            .iter()
            .any(|args| args == &["repo", "create", "owner/momso-tester", "--private"]));
        assert!(!calls.iter().flatten().any(|arg| arg == "--public"));
    }

    #[test]
    fn created_repository_is_not_connected_if_visibility_changes() {
        let mut views = 0;
        let mut linked = false;
        let result = create_auto(
            Some("test".into()),
            |args| {
                if args == ["--version"] || args.starts_with(&["auth", "status"]) {
                    return reply(0, "ok");
                }
                if args.ends_with(&[".login"]) {
                    return reply(0, "owner");
                }
                if args.starts_with(&["repo", "view"]) {
                    views += 1;
                    return if views == 1 {
                        reply(1, "")
                    } else {
                        reply(
                            0,
                            r#"{"nameWithOwner":"owner/momso-test","isPrivate":false,"isEmpty":true,"defaultBranchRef":null}"#,
                        )
                    };
                }
                if args.starts_with(&["api", "--hostname", "github.com", "--include"]) {
                    return reply(1, "HTTP/2.0 404 Not Found\r\n");
                }
                if args.starts_with(&["repo", "create"]) {
                    return reply(0, "created");
                }
                panic!("unexpected fake command");
            },
            || false,
            |_| "exists_private_unknown",
            |_| {
                linked = true;
                true
            },
        );
        assert_eq!(result.state, "created_unverified");
        assert!(!linked);
    }

    #[test]
    fn existing_repositories_are_never_created_or_connected() {
        for (private, empty, classification, expected) in [
            (false, false, "exists_matching", "exists_public"),
            (true, true, "exists_matching", "exists_empty"),
            (true, false, "exists_matching", "exists_matching"),
            (
                true,
                false,
                "exists_private_unknown",
                "exists_private_unknown",
            ),
        ] {
            let mut called_create = false;
            let mut linked = false;
            let result = create_auto(
                Some("test".into()),
                |args| {
                    if args == ["--version"] || args.starts_with(&["auth", "status"]) {
                        return reply(0, "ok");
                    }
                    if args.ends_with(&[".login"]) {
                        return reply(0, "owner");
                    }
                    if args.starts_with(&["repo", "view"]) {
                        return reply(
                            0,
                            &format!(
                                r#"{{"nameWithOwner":"owner/momso-test","isPrivate":{private},"isEmpty":{empty},"defaultBranchRef":null}}"#
                            ),
                        );
                    }
                    if args.starts_with(&["repo", "create"]) {
                        called_create = true;
                    }
                    reply(1, "")
                },
                || false,
                |_| classification,
                |_| {
                    linked = true;
                    true
                },
            );
            assert_eq!(result.state, expected);
            assert!(!called_create && !linked);
        }
    }

    #[test]
    fn auto_create_fails_closed_on_auth_lookup_and_visibility_failures() {
        let missing = create_auto(
            Some("test".into()),
            |_| Err(()),
            || false,
            |_| "exists_private_unknown",
            |_| panic!("must not connect"),
        );
        assert_eq!(missing.state, "gh_missing");
        let signed_out = create_auto(
            Some("test".into()),
            |args| {
                if args == ["--version"] {
                    reply(0, "ok")
                } else {
                    reply(1, "")
                }
            },
            || false,
            |_| "exists_private_unknown",
            |_| panic!("must not connect"),
        );
        assert_eq!(signed_out.state, "signed_out");
        let mut created = false;
        let unknown = create_auto(
            Some("test".into()),
            |args| {
                if args == ["--version"] || args.starts_with(&["auth", "status"]) {
                    return reply(0, "ok");
                }
                if args.ends_with(&[".login"]) {
                    return reply(0, "owner");
                }
                if args.starts_with(&["repo", "create"]) {
                    created = true;
                }
                reply(1, "HTTP/2.0 503 Service Unavailable\r\n")
            },
            || false,
            |_| "exists_private_unknown",
            |_| panic!("must not connect"),
        );
        assert_eq!(unknown.state, "unavailable");
        assert!(!created);
    }
    #[test]
    #[ignore = "requires verified managed MinGit and a PATH without system git or gh"]
    fn managed_mingit_status_without_system_git() {
        let resources = PathBuf::from(std::env::var_os("MOMSO_GIT_TEST_DATA").unwrap());
        crate::git_tool::configure(resources.clone());
        let tool = git_path(&resources).expect("prepared MinGit");
        assert!(tool.starts_with(resources.join("git-tools")));
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        fs::create_dir(&root).unwrap();
        assert!(!Command::new("where.exe")
            .arg("git.exe")
            .output()
            .unwrap()
            .status
            .success());
        git(&tool, &root, &["init", "--initial-branch=main"]).unwrap();
        assert_eq!(
            git(&tool, &root, &["rev-parse", "--is-inside-work-tree"]).unwrap(),
            "true"
        );
        let result = status(&root, &resources);
        assert_eq!(result.state, "unavailable");
        assert_eq!(result.reason, Some("gh_missing"));
    }
    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        bare: PathBuf,
        tool: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("archive");
            let bare = dir.path().join("remote.git");
            fs::create_dir_all(root.join("zettels")).unwrap();
            let tool = crate::process::resolve_tool("git.exe").expect("git executable");
            git(
                &tool,
                dir.path(),
                &["init", "--bare", bare.to_str().unwrap()],
            )
            .unwrap();
            git(&tool, &root, &["init", "--initial-branch=main"]).unwrap();
            fs::write(root.join(".gitignore"), "objects/\n").unwrap();
            fs::write(root.join("zettels/a.md"), "initial").unwrap();
            git(&tool, &root, &["add", "--", ".gitignore", "zettels/"]).unwrap();
            git(
                &tool,
                &root,
                &[
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "commit",
                    "-m",
                    "initial",
                ],
            )
            .unwrap();
            Self {
                _dir: dir,
                root,
                bare,
                tool,
            }
        }
        fn remote(&self) -> &str {
            self.bare.to_str().unwrap()
        }
        fn push(&self) {
            transfer(&self.tool, &self.root, self.remote(), "main", "push").unwrap();
        }
        fn clone_and_commit(&self) {
            let other = self._dir.path().join("other");
            git(
                &self.tool,
                self._dir.path(),
                &[
                    "clone",
                    "--branch",
                    "main",
                    self.remote(),
                    other.to_str().unwrap(),
                ],
            )
            .unwrap();
            fs::write(other.join("zettels/b.md"), "remote").unwrap();
            git(&self.tool, &other, &["add", "--", "zettels/b.md"]).unwrap();
            git(
                &self.tool,
                &other,
                &[
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "commit",
                    "-m",
                    "remote",
                ],
            )
            .unwrap();
            git(&self.tool, &other, &["push", "origin", "main"]).unwrap();
        }
        fn local_commit(&self) {
            fs::write(self.root.join("zettels/local.md"), "local").unwrap();
            git(&self.tool, &self.root, &["add", "--", "zettels/local.md"]).unwrap();
            git(
                &self.tool,
                &self.root,
                &[
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "commit",
                    "-m",
                    "local",
                ],
            )
            .unwrap();
        }
    }

    #[test]
    fn only_known_safe_ancestor_is_reported_as_matching() {
        let f = Fixture::new();
        let head = git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap();
        assert!(is_known_archive_history(&f.tool, &f.root, &head));
        assert!(!is_known_archive_history(&f.tool, &f.root, &"0".repeat(40)));
        fs::write(f.root.join("unsafe-file"), "fictional").unwrap();
        git(&f.tool, &f.root, &["add", "--", "unsafe-file"]).unwrap();
        git(
            &f.tool,
            &f.root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "unsafe",
            ],
        )
        .unwrap();
        let unsafe_head = git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap();
        assert!(!is_known_archive_history(&f.tool, &f.root, &unsafe_head));
        assert!(is_known_archive_history(&f.tool, &f.root, &head));
    }
    #[test]
    fn connects_without_changing_files() {
        let f = Fixture::new();
        connect_local(&f.tool, &f.root, f.remote()).unwrap();
        assert_eq!(
            git(&f.tool, &f.root, &["remote", "get-url", REMOTE]).unwrap(),
            f.remote()
        );
        assert_eq!(
            fs::read_to_string(f.root.join("zettels/a.md")).unwrap(),
            "initial"
        );
    }

    #[test]
    fn fresh_install_receives_existing_history_and_default_files() {
        let f = Fixture::new();
        f.push();
        f.clone_and_commit();
        let fresh = f._dir.path().join("fresh");
        fs::create_dir_all(&fresh).unwrap();
        crate::archive::initialize(&fresh, f._dir.path()).unwrap();
        assert!(fresh_archive(&f.tool, &fresh));
        connect_local(&f.tool, &fresh, f.remote()).unwrap();
        transfer(&f.tool, &fresh, f.remote(), "main", "pull").unwrap();
        assert_eq!(
            fs::read_to_string(fresh.join("zettels/a.md")).unwrap(),
            "initial"
        );
        assert!(fresh.join("AGENTS.md").is_file());
        assert!(fresh.join("skills/github-connect/SKILL.md").is_file());
        assert!(fresh.join("dictionary/yoga.json").is_file());
        assert!(fresh
            .join(".agents/skills/github-connect/SKILL.md")
            .is_file());
        assert_eq!(
            git(&f.tool, &fresh, &["rev-list", "--count", "HEAD"]).unwrap(),
            "2"
        );
        assert_eq!(
            fs::read_to_string(fresh.join("zettels/b.md")).unwrap(),
            "remote"
        );
    }

    #[test]
    fn fresh_receive_accepts_remote_dictionary_and_preserves_user_files() {
        let f = Fixture::new();
        crate::yoga_dictionary::initialize(&f.root).unwrap();
        fs::write(
            f.root.join("dictionary/my-terms.json"),
            "remote person terms",
        )
        .unwrap();
        fs::write(
            f.root.join("dictionary/people.json"),
            "{\"people\":[{\"name\":\"fixture-person\"}]}",
        )
        .unwrap();
        git(&f.tool, &f.root, &["add", "--", "dictionary/"]).unwrap();
        git(
            &f.tool,
            &f.root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "dictionary",
            ],
        )
        .unwrap();
        f.push();
        let fresh = f._dir.path().join("fresh-dictionary");
        fs::create_dir_all(&fresh).unwrap();
        crate::archive::initialize(&fresh, f._dir.path()).unwrap();
        connect_local(&f.tool, &fresh, f.remote()).unwrap();
        transfer(&f.tool, &fresh, f.remote(), "main", "pull").unwrap();
        assert_eq!(
            fs::read(fresh.join("dictionary/my-terms.json")).unwrap(),
            b"remote person terms"
        );
        assert_eq!(
            fs::read_to_string(fresh.join("dictionary/people.json")).unwrap(),
            "{\"people\":[{\"name\":\"fixture-person\"}]}"
        );
    }

    #[test]
    fn fresh_install_refuses_user_files_and_modified_defaults() {
        for name in [
            "zettels/user.md",
            "objects/original",
            "AGENTS.md",
            "extra",
            "skills/github-connect/SKILL.md",
            "dictionary/yoga.json",
            "dictionary/my-terms.json",
            "dictionary/people.json",
            ".agents/skills/github-connect/SKILL.md",
            ".claude/skills/github-connect/custom.txt",
        ] {
            let f = Fixture::new();
            f.push();
            let fresh = f._dir.path().join("fresh");
            fs::create_dir_all(&fresh).unwrap();
            crate::archive::initialize(&fresh, f._dir.path()).unwrap();
            fs::write(fresh.join(name), "user content").unwrap();
            connect_local(&f.tool, &fresh, f.remote()).unwrap();
            assert_eq!(
                transfer(&f.tool, &fresh, f.remote(), "main", "pull"),
                Err("unsaved_changes")
            );
            assert_eq!(
                fs::read_to_string(fresh.join(name)).unwrap(),
                "user content"
            );
            assert!(git(&f.tool, &fresh, &["rev-parse", "--verify", "HEAD"]).is_err());
        }
    }

    #[test]
    fn fresh_install_refuses_deleted_default_skills() {
        for folder in ["skills", ".agents/skills", ".claude/skills"] {
            let f = Fixture::new();
            f.push();
            let fresh = f._dir.path().join("fresh-deleted-skill");
            fs::create_dir_all(&fresh).unwrap();
            crate::archive::initialize(&fresh, f._dir.path()).unwrap();
            let missing = fresh.join(folder).join("github-connect/SKILL.md");
            fs::remove_file(&missing).unwrap();
            connect_local(&f.tool, &fresh, f.remote()).unwrap();
            assert_eq!(
                transfer(&f.tool, &fresh, f.remote(), "main", "pull"),
                Err("unsaved_changes")
            );
            assert!(!missing.exists());
        }
    }

    #[test]
    fn first_receive_keeps_remote_disabled_skill_disabled() {
        let f = Fixture::new();
        let skill = f.root.join("skills/github-connect/SKILL.md");
        fs::create_dir_all(skill.parent().unwrap()).unwrap();
        let content = crate::skills::BUNDLED_SKILLS
            .iter()
            .find(|(name, _)| *name == "github-connect")
            .unwrap()
            .1;
        fs::write(&skill, format!("{content}\n")).unwrap();
        git(
            &f.tool,
            &f.root,
            &["add", "--", "skills/github-connect/SKILL.md"],
        )
        .unwrap();
        git(
            &f.tool,
            &f.root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "remote skill",
            ],
        )
        .unwrap();
        f.push();
        let fresh = f._dir.path().join("fresh-disabled-skill");
        fs::create_dir_all(&fresh).unwrap();
        crate::archive::initialize(&fresh, f._dir.path()).unwrap();
        connect_local(&f.tool, &fresh, f.remote()).unwrap();
        transfer(&f.tool, &fresh, f.remote(), "main", "pull").unwrap();
        assert!(fresh.join("skills/github-connect/SKILL.md").is_file());
        assert!(!fresh
            .join(".agents/skills/github-connect/SKILL.md")
            .exists());
        assert!(!fresh
            .join(".claude/skills/github-connect/SKILL.md")
            .exists());
        crate::skills_write::reconcile(&fresh, "github-connect").unwrap();
        assert!(!fresh
            .join(".agents/skills/github-connect/SKILL.md")
            .exists());
        assert!(!fresh
            .join(".claude/skills/github-connect/SKILL.md")
            .exists());
        assert!(fresh
            .join(".agents/skills/object-store-connect/SKILL.md")
            .is_file());
        let backups: Vec<_> = fs::read_dir(fresh.join(".git"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("momso-first-pull-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert!(default_skills_unchanged(&backups[0].path(), ".agents"));
    }

    #[test]
    fn first_receive_keeps_non_utf8_remote_skill_unchanged() {
        let f = Fixture::new();
        let skill = f.root.join("skills/github-connect/SKILL.md");
        fs::create_dir_all(skill.parent().unwrap()).unwrap();
        fs::write(&skill, [0xff, 0xfe, 0xfd]).unwrap();
        git(
            &f.tool,
            &f.root,
            &["add", "--", "skills/github-connect/SKILL.md"],
        )
        .unwrap();
        git(
            &f.tool,
            &f.root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "remote skill",
            ],
        )
        .unwrap();
        f.push();
        let fresh = f._dir.path().join("fresh-non-utf8-skill");
        fs::create_dir_all(&fresh).unwrap();
        crate::archive::initialize(&fresh, f._dir.path()).unwrap();
        connect_local(&f.tool, &fresh, f.remote()).unwrap();
        transfer(&f.tool, &fresh, f.remote(), "main", "pull").unwrap();
        assert_eq!(
            fs::read(fresh.join("skills/github-connect/SKILL.md")).unwrap(),
            [0xff, 0xfe, 0xfd]
        );
    }

    #[test]
    fn first_receive_preserves_default_edited_after_initial_check() {
        let f = Fixture::new();
        f.push();
        let fresh = f._dir.path().join("fresh-race");
        fs::create_dir_all(&fresh).unwrap();
        crate::archive::initialize(&fresh, f._dir.path()).unwrap();
        relation(&f.tool, &fresh, f.remote(), "main").unwrap();
        let revision = git(
            &f.tool,
            &fresh,
            &["rev-parse", "refs/remotes/momso/observed"],
        )
        .unwrap();
        let result = first_receive_with_hook(&f.tool, &fresh, &revision, || {
            fs::write(fresh.join("AGENTS.md"), "user edit during receive").unwrap();
        });
        assert_eq!(result, Err("unsaved_changes"));
        assert_eq!(
            fs::read_to_string(fresh.join("AGENTS.md")).unwrap(),
            "user edit during receive"
        );
        assert!(git(&f.tool, &fresh, &["rev-parse", "--verify", "HEAD"]).is_err());
    }

    #[test]
    fn first_receive_preserves_skill_edited_after_initial_check() {
        let f = Fixture::new();
        f.push();
        let fresh = f._dir.path().join("fresh-skill-race");
        fs::create_dir_all(&fresh).unwrap();
        crate::archive::initialize(&fresh, f._dir.path()).unwrap();
        relation(&f.tool, &fresh, f.remote(), "main").unwrap();
        let revision = git(
            &f.tool,
            &fresh,
            &["rev-parse", "refs/remotes/momso/observed"],
        )
        .unwrap();
        let path = fresh.join("skills/github-connect/SKILL.md");
        let result = first_receive_with_hook(&f.tool, &fresh, &revision, || {
            fs::write(&path, "user skill edit").unwrap();
        });
        assert_eq!(result, Err("unsaved_changes"));
        assert_eq!(fs::read(&path).unwrap(), b"user skill edit");
        assert!(git(&f.tool, &fresh, &["rev-parse", "--verify", "HEAD"]).is_err());
    }

    #[test]
    fn first_receive_preserves_skill_deleted_after_initial_check() {
        let f = Fixture::new();
        f.push();
        let fresh = f._dir.path().join("fresh-skill-delete-race");
        fs::create_dir_all(&fresh).unwrap();
        crate::archive::initialize(&fresh, f._dir.path()).unwrap();
        relation(&f.tool, &fresh, f.remote(), "main").unwrap();
        let revision = git(
            &f.tool,
            &fresh,
            &["rev-parse", "refs/remotes/momso/observed"],
        )
        .unwrap();
        let path = fresh.join("skills/github-connect/SKILL.md");
        let result = first_receive_with_hook(&f.tool, &fresh, &revision, || {
            fs::remove_file(&path).unwrap();
        });
        assert_eq!(result, Err("unsaved_changes"));
        assert!(!path.exists());
        assert!(git(&f.tool, &fresh, &["rev-parse", "--verify", "HEAD"]).is_err());
    }

    #[test]
    fn first_receive_preserves_skill_state_edited_after_initial_check() {
        let f = Fixture::new();
        f.push();
        let fresh = f._dir.path().join("fresh-skill-state-race");
        fs::create_dir_all(&fresh).unwrap();
        crate::archive::initialize(&fresh, f._dir.path()).unwrap();
        relation(&f.tool, &fresh, f.remote(), "main").unwrap();
        let revision = git(
            &f.tool,
            &fresh,
            &["rev-parse", "refs/remotes/momso/observed"],
        )
        .unwrap();
        let state = f._dir.path().join("skill-state/github-connect.json");
        let changed = br#"{"hash":"changed","enabled":false}"#;
        let result = first_receive_with_hook(&f.tool, &fresh, &revision, || {
            fs::write(&state, changed).unwrap();
        });
        assert_eq!(result, Err("unsaved_changes"));
        assert_eq!(fs::read(&state).unwrap(), changed);
        assert!(git(&f.tool, &fresh, &["rev-parse", "--verify", "HEAD"]).is_err());
    }

    #[test]
    fn successful_first_receive_keeps_default_recovery_copy() {
        let f = Fixture::new();
        f.push();
        let fresh = f._dir.path().join("fresh-recovery");
        fs::create_dir_all(&fresh).unwrap();
        crate::archive::initialize(&fresh, f._dir.path()).unwrap();
        relation(&f.tool, &fresh, f.remote(), "main").unwrap();
        let revision = git(
            &f.tool,
            &fresh,
            &["rev-parse", "refs/remotes/momso/observed"],
        )
        .unwrap();
        first_receive(&f.tool, &fresh, &revision).unwrap();
        let backups: Vec<_> = fs::read_dir(fresh.join(".git"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("momso-first-pull-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert!(default_file_unchanged(
            &backups[0].path().join("AGENTS.md"),
            "AGENTS.md"
        ));
        for folder in ["skills", ".agents", ".claude"] {
            assert!(default_skills_unchanged(&backups[0].path(), folder));
            assert!(default_skills_unchanged(&fresh, folder));
        }
        fs::write(fresh.join("AGENTS.md"), "later user edit").unwrap();
        fs::write(
            fresh.join("skills/github-connect/SKILL.md"),
            "later skill edit",
        )
        .unwrap();
        assert!(default_file_unchanged(
            &backups[0].path().join("AGENTS.md"),
            "AGENTS.md"
        ));
        assert!(default_skills_unchanged(&backups[0].path(), "skills"));
    }

    #[test]
    fn audit_reproduces_old_first_receive_discarding_a_late_edit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let original = root.join("AGENTS.md");
        let staged = root.join("saved");
        fs::write(&original, include_str!("archive_rules.md")).unwrap();
        assert!(default_file_unchanged(&original, "AGENTS.md"));
        fs::write(&original, "user edit during receive").unwrap();
        fs::rename(&original, &staged).unwrap();
        fs::write(&original, "remote file").unwrap();
        fs::remove_file(&staged).unwrap();
        assert_eq!(fs::read_to_string(&original).unwrap(), "remote file");
        assert!(!staged.exists());
    }
    #[test]
    fn pushes_only_saved_content() {
        let f = Fixture::new();
        fs::write(f.root.join("zettels/a.md"), "unsaved").unwrap();
        f.push();
        let text = git(
            &f.tool,
            &f.root,
            &["--git-dir", f.remote(), "show", "main:zettels/a.md"],
        )
        .unwrap();
        assert_eq!(text, "initial");
    }
    #[test]
    fn saved_skill_paths_reach_local_remote_but_unrelated_files_are_rejected() {
        let f = Fixture::new();
        let paths = [
            "skills/probe/SKILL.md",
            ".agents/skills/probe/SKILL.md",
            ".claude/skills/probe/SKILL.md",
        ];
        for path in paths {
            let dest = f.root.join(path);
            fs::create_dir_all(dest.parent().unwrap()).unwrap();
            fs::write(dest, "skill").unwrap();
        }
        git(
            &f.tool,
            &f.root,
            &["add", "--", paths[0], paths[1], paths[2]],
        )
        .unwrap();
        git(
            &f.tool,
            &f.root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "skill",
            ],
        )
        .unwrap();
        f.push();
        for path in paths {
            let reference = format!("main:{path}");
            assert_eq!(
                git(
                    &f.tool,
                    &f.root,
                    &["--git-dir", f.remote(), "show", &reference]
                )
                .unwrap(),
                "skill"
            );
        }
        assert!(!safe_skill_path("skills/probe/other.md"));
        assert!(!safe_skill_path("skills/../SKILL.md"));
    }
    #[test]
    fn refuses_remote_ahead() {
        let f = Fixture::new();
        f.push();
        f.clone_and_commit();
        assert_eq!(
            transfer(&f.tool, &f.root, f.remote(), "main", "push"),
            Err("remote_ahead")
        );
    }
    #[test]
    fn refuses_divergence() {
        let f = Fixture::new();
        f.push();
        f.clone_and_commit();
        f.local_commit();
        assert_eq!(
            transfer(&f.tool, &f.root, f.remote(), "main", "push"),
            Err("diverged")
        );
        assert_eq!(
            transfer(&f.tool, &f.root, f.remote(), "main", "pull"),
            Err("diverged")
        );
    }
    #[test]
    fn pulls_fast_forward() {
        let f = Fixture::new();
        f.push();
        f.clone_and_commit();
        transfer(&f.tool, &f.root, f.remote(), "main", "pull").unwrap();
        assert_eq!(
            fs::read_to_string(f.root.join("zettels/b.md")).unwrap(),
            "remote"
        );
    }
    #[test]
    fn refuses_dirty_pull() {
        let f = Fixture::new();
        f.push();
        f.clone_and_commit();
        fs::write(f.root.join("zettels/a.md"), "unsaved").unwrap();
        assert_eq!(
            transfer(&f.tool, &f.root, f.remote(), "main", "pull"),
            Err("unsaved_changes")
        );
        assert_eq!(
            fs::read_to_string(f.root.join("zettels/a.md")).unwrap(),
            "unsaved"
        );
    }
    #[test]
    fn allows_untracked_default_archive_ignore() {
        let f = Fixture::new();
        git(&f.tool, &f.root, &["rm", "--cached", "--", ".gitignore"]).unwrap();
        git(
            &f.tool,
            &f.root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "ignore untracked",
            ],
        )
        .unwrap();
        f.push();
        f.clone_and_commit();
        transfer(&f.tool, &f.root, f.remote(), "main", "pull").unwrap();
        assert_eq!(
            fs::read_to_string(f.root.join(".gitignore")).unwrap(),
            "objects/\n"
        );
    }
    #[test]
    fn disconnect_preserves_files_and_remote() {
        let f = Fixture::new();
        connect_local(&f.tool, &f.root, f.remote()).unwrap();
        f.push();
        git(&f.tool, &f.root, &["remote", "remove", REMOTE]).unwrap();
        assert!(f.root.join("zettels/a.md").is_file());
        assert!(git(
            &f.tool,
            &f.root,
            &["--git-dir", f.remote(), "rev-parse", "refs/heads/main"]
        )
        .is_ok());
    }
    #[test]
    fn rejects_objects_and_database_in_history() {
        let f = Fixture::new();
        fs::create_dir_all(f.root.join("objects")).unwrap();
        fs::write(f.root.join("objects/original"), "private").unwrap();
        fs::write(f.root.join("momso.db"), "private").unwrap();
        git(
            &f.tool,
            &f.root,
            &["add", "-f", "--", "objects/original", "momso.db"],
        )
        .unwrap();
        git(
            &f.tool,
            &f.root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "bad",
            ],
        )
        .unwrap();
        assert_eq!(
            transfer(&f.tool, &f.root, f.remote(), "main", "push"),
            Err("unsafe_history")
        );
    }
    #[test]
    fn rejects_original_removed_from_latest_commit() {
        let f = Fixture::new();
        fs::create_dir_all(f.root.join("objects")).unwrap();
        fs::write(f.root.join("objects/original"), "initial").unwrap();
        git(&f.tool, &f.root, &["add", "-f", "--", "objects/original"]).unwrap();
        git(
            &f.tool,
            &f.root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "add original",
            ],
        )
        .unwrap();
        git(&f.tool, &f.root, &["rm", "--", "objects/original"]).unwrap();
        git(
            &f.tool,
            &f.root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "remove original",
            ],
        )
        .unwrap();
        assert_eq!(
            transfer(&f.tool, &f.root, f.remote(), "main", "push"),
            Err("unsafe_history")
        );
    }
    #[test]
    fn rejects_incomplete_history() {
        let f = Fixture::new();
        let head = git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap();
        fs::write(f.root.join(".git/shallow"), format!("{head}\n")).unwrap();
        assert_eq!(
            transfer(&f.tool, &f.root, f.remote(), "main", "push"),
            Err("unsafe_history")
        );
    }

    #[test]
    fn profile_and_rules_backup_and_pull_preserve_exact_content() {
        let f = Fixture::new();
        crate::archive::initialize(&f.root, f._dir.path()).unwrap();
        f.push();
        let other = f._dir.path().join("other-profile");
        git(
            &f.tool,
            f._dir.path(),
            &[
                "clone",
                "--branch",
                "main",
                f.remote(),
                other.to_str().unwrap(),
            ],
        )
        .unwrap();
        crate::archive::profile_set(
            &f.root,
            f._dir.path(),
            "fictional name".into(),
            "fictional about".into(),
            "fictional style".into(),
        )
        .unwrap();
        transfer(&f.tool, &f.root, f.remote(), "main", "push").unwrap();
        transfer(&f.tool, &other, f.remote(), "main", "pull").unwrap();
        for name in ["profile.md", "AGENTS.md", "CLAUDE.md", "GEMINI.md"] {
            assert_eq!(
                fs::read(f.root.join(name)).unwrap(),
                fs::read(other.join(name)).unwrap()
            );
        }
        let result = serde_json::to_value(crate::archive::profile_get(&other).unwrap()).unwrap();
        assert_eq!(result["name"], "fictional name");
        assert_eq!(result["about"], "fictional about");
        assert_eq!(result["style"], "fictional style");
        let status = serde_json::to_value(RemoteStatus::transfer_error("unsafe_history")).unwrap();
        assert_eq!(status["reason"], "unsafe_history");
        assert!(status["historyReason"].is_null());
    }
    #[test]
    fn repo_validation_rejects_urls_and_options() {
        assert!(valid_repo("owner/private-repo"));
        assert!(!valid_repo("https://github.com/a/b"));
        assert!(!valid_repo("-owner/name"));
        assert!(!valid_repo("a/../b"));
    }
    #[test]
    fn resource_directory_cannot_override_git_selection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mingit/cmd/git.exe");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "placeholder").unwrap();
        assert_ne!(git_path(dir.path()), Some(path));
    }
    #[test]
    fn rejects_public_repository_metadata() {
        let value = r#"{"nameWithOwner":"owner/repo","isPrivate":false,"defaultBranchRef":{"name":"main"}}"#;
        assert!(matches!(
            parse_private_repository(value),
            Err("repository_not_private")
        ));
    }
    #[test]
    fn ignored_original_and_untracked_database_are_not_pushed() {
        let f = Fixture::new();
        fs::create_dir_all(f.root.join("objects")).unwrap();
        fs::write(f.root.join("objects/original"), "private").unwrap();
        fs::write(f.root.join("momso.db"), "private").unwrap();
        f.push();
        let tracked = git(
            &f.tool,
            &f.root,
            &[
                "--git-dir",
                f.remote(),
                "ls-tree",
                "-r",
                "--name-only",
                "main",
            ],
        )
        .unwrap();
        assert!(!tracked.contains("objects/"));
        assert!(!tracked.contains("momso.db"));
    }
    #[test]
    fn pulled_archive_can_rebuild_index() {
        let f = Fixture::new();
        fs::remove_file(f.root.join("zettels/a.md")).unwrap();
        let id = "zet_20261115_143052";
        let document = "---\nid: zet_20261115_143052\ntitle: 수업\nsummary: 기록\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets: []\nedges: []\n---\n본문\n";
        fs::write(f.root.join(format!("zettels/{id}.md")), document).unwrap();
        git(&f.tool, &f.root, &["add", "--", "zettels/"]).unwrap();
        git(
            &f.tool,
            &f.root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "record",
            ],
        )
        .unwrap();
        f.push();
        let other = f._dir.path().join("receive");
        fs::create_dir_all(other.join("zettels")).unwrap();
        fs::create_dir_all(other.join("receipts")).unwrap();
        git(&f.tool, &other, &["init", "--initial-branch=main"]).unwrap();
        transfer(&f.tool, &other, f.remote(), "main", "pull").unwrap();
        let db = f._dir.path().join("index.db");
        crate::storage::initialize(&db, &f._dir.path().join("backups"), "test").unwrap();
        crate::archive::reindex(&other, &db).unwrap();
        assert_eq!(crate::archive_zet::list(&db, None, None).unwrap().len(), 1);
    }
}
