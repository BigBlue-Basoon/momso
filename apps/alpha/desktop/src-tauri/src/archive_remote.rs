use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
};
use tauri::{Emitter, Manager, State};

static REMOTE_LOCK: Mutex<()> = Mutex::new(());
static FIRST_RECEIVE_PROGRESS: Mutex<Vec<(PathBuf, PathBuf, FirstReceiveProgress)>> =
    Mutex::new(Vec::new());
const REMOTE: &str = "momso";
pub(crate) const FIRST_RECEIVE_JOURNAL: &str = "first_receive_v1";

pub(crate) fn lock_remote_for_restore() -> Result<std::sync::MutexGuard<'static, ()>, &'static str>
{
    REMOTE_LOCK.lock().map_err(|_| "busy")
}

pub(crate) fn lock_first_receive(db: &Path) -> Result<fs::File, &'static str> {
    let lock_path = db
        .parent()
        .ok_or("local_state_unavailable")?
        .join(".momso-first-receive.lock");
    if let Ok(metadata) = fs::symlink_metadata(&lock_path) {
        if !metadata.file_type().is_file() {
            return Err("local_state_unavailable");
        }
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|_| "local_state_unavailable")?;
    file.try_lock().map_err(|_| "busy")?;
    Ok(file)
}

fn receive_marker_present(root: &Path) -> bool {
    !matches!(fs::symlink_metadata(root.join(".git/momso-first-receive-in-progress")), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReceiveJournal {
    root: PathBuf,
    target: String,
    branch: String,
    phase: String,
    #[serde(default)]
    revision: Option<String>,
    #[serde(default)]
    has_defaults: Option<bool>,
    #[serde(default)]
    restore_intents: Vec<String>,
    #[serde(default)]
    restore_done: Vec<String>,
    #[serde(default)]
    deleted_chats: Vec<String>,
}

fn read_receive_journal(db: &Path) -> Result<Option<ReceiveJournal>, &'static str> {
    let conn = Connection::open(db).map_err(|_| "local_state_unavailable")?;
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM app_runtime_meta WHERE key=?1",
            [FIRST_RECEIVE_JOURNAL],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| "local_state_unavailable")?;
    value
        .map(|value| serde_json::from_str(&value).map_err(|_| "first_receive_interrupted"))
        .transpose()
}

fn write_receive_journal(db: &Path, journal: &ReceiveJournal) -> Result<(), &'static str> {
    let value = serde_json::to_string(journal).map_err(|_| "local_state_unavailable")?;
    Connection::open(db).and_then(|conn| conn.execute(
        "INSERT INTO app_runtime_meta(key,value,updated_at) VALUES(?1,?2,CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",
        rusqlite::params![FIRST_RECEIVE_JOURNAL, value],
    )).map_err(|_| "local_state_unavailable")?;
    Ok(())
}

fn remember_chat_deletions(db: &Path, journal: &mut ReceiveJournal) -> Result<(), &'static str> {
    let connection = Connection::open(db).map_err(|_| "local_state_unavailable")?;
    let mut statement = connection
        .prepare(
            "SELECT substr(key,21) FROM app_runtime_meta WHERE key GLOB 'chat_archive_delete:*'",
        )
        .map_err(|_| "local_state_unavailable")?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| "local_state_unavailable")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "local_state_unavailable")?;
    drop(statement);
    drop(connection);
    let mut changed = false;
    for id in ids {
        if !crate::archive_receipt::valid_chat_id(&id) {
            return Err("first_receive_interrupted");
        }
        if !journal.deleted_chats.contains(&id) {
            journal.deleted_chats.push(id);
            changed = true;
        }
    }
    if changed {
        write_receive_journal(db, journal)?;
    }
    Ok(())
}

fn finish_receive_journal(db: &Path) -> Result<(), &'static str> {
    Connection::open(db)
        .and_then(|conn| {
            conn.execute(
                "DELETE FROM app_runtime_meta WHERE key=?1",
                [FIRST_RECEIVE_JOURNAL],
            )
        })
        .map_err(|_| "local_state_unavailable")?;
    Ok(())
}

fn begin_receive_journal(
    db: &Path,
    root: &Path,
    target: &str,
    branch: &str,
) -> Result<ReceiveJournal, &'static str> {
    if let Some(mut journal) = read_receive_journal(db)? {
        if journal.root != root || journal.target != target {
            return Err("first_receive_interrupted");
        }
        if journal.phase == "discovering" && journal.branch.is_empty() {
            journal.branch = branch.to_owned();
            journal.phase = "receiving".to_owned();
            write_receive_journal(db, &journal)?;
        }
        if journal.branch != branch {
            return Err("first_receive_interrupted");
        }
        if !matches!(
            journal.phase.as_str(),
            "receiving" | "checkout" | "indexing" | "restoring"
        ) {
            return Err("first_receive_interrupted");
        }
        return Ok(journal);
    }
    let journal = ReceiveJournal {
        root: root.to_path_buf(),
        target: target.to_owned(),
        branch: branch.to_owned(),
        phase: "receiving".to_owned(),
        revision: None,
        has_defaults: None,
        restore_intents: Vec::new(),
        restore_done: Vec::new(),
        deleted_chats: Vec::new(),
    };
    write_receive_journal(db, &journal)?;
    Ok(journal)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FirstReceiveProgress {
    completed: usize,
    total: usize,
    state: &'static str,
    available: bool,
    reason: Option<&'static str>,
    conflict_paths: Vec<String>,
}

fn receive_progress(
    state: &'static str,
    completed: usize,
    available: bool,
) -> FirstReceiveProgress {
    FirstReceiveProgress {
        completed,
        total: 3,
        state,
        available,
        reason: None,
        conflict_paths: Vec::new(),
    }
}

fn remember_receive_progress(root: &Path, db: &Path, progress: &FirstReceiveProgress) {
    if let Ok(mut states) = FIRST_RECEIVE_PROGRESS.lock() {
        states.retain(|(stored_root, stored_db, _)| stored_root != root || stored_db != db);
        states.push((root.to_path_buf(), db.to_path_buf(), progress.clone()));
    }
}

fn files_equal(left: &Path, right: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    let mut left = fs::File::open(left)?;
    let mut right = fs::File::open(right)?;
    if left.metadata()?.len() != right.metadata()?.len() {
        return Ok(false);
    }
    let mut a = [0_u8; 16384];
    let mut b = [0_u8; 16384];
    loop {
        let count = left.read(&mut a)?;
        right.read_exact(&mut b[..count])?;
        if a[..count] != b[..count] {
            return Ok(false);
        }
        if count == 0 {
            return Ok(true);
        }
    }
}

// Only inspect the already validated recovery directory. Never move or remove
// the user's file while reporting a collision.
fn saved_default_conflicts(root: &Path) -> Vec<String> {
    let marker = root.join(".git/momso-first-receive-in-progress");
    let Ok(meta) = fs::symlink_metadata(&marker) else {
        return Vec::new();
    };
    if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) || meta.len() > 128 {
        return Vec::new();
    }
    let Ok(name) = fs::read_to_string(marker) else {
        return Vec::new();
    };
    let Some(id) = name.strip_prefix("momso-first-pull-") else {
        return Vec::new();
    };
    if uuid::Uuid::parse_str(id).is_err() {
        return Vec::new();
    }
    let saved = root.join(".git").join(name);
    if !fs::symlink_metadata(&saved)
        .is_ok_and(|m| m.is_dir() && !crate::archive_location::is_link_or_reparse(&m))
    {
        return Vec::new();
    }
    let mut paths = Vec::new();
    for entry in walkdir::WalkDir::new(&saved)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(&saved) else {
            continue;
        };
        let mut ancestor = root.to_path_buf();
        let mut unsafe_parent = false;
        if let Some(parent) = relative.parent() {
            for part in parent.components() {
                ancestor.push(part.as_os_str());
                if fs::symlink_metadata(&ancestor)
                    .is_ok_and(|m| crate::archive_location::is_link_or_reparse(&m))
                {
                    unsafe_parent = true;
                    break;
                }
            }
        }
        if unsafe_parent {
            paths.push(relative.to_string_lossy().replace('\\', "/"));
            continue;
        }
        let destination = root.join(relative);
        let Ok(meta) = fs::symlink_metadata(&destination) else {
            continue;
        };
        if !meta.is_file()
            || crate::archive_location::is_link_or_reparse(&meta)
            || !files_equal(entry.path(), &destination).unwrap_or(false)
        {
            paths.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

fn receive_failure_progress(root: &Path, error: &'static str) -> FirstReceiveProgress {
    let mut value = receive_progress(
        if error == "first_receive_interrupted" {
            "interrupted"
        } else {
            "failed"
        },
        0,
        false,
    );
    if error == "first_receive_interrupted" {
        value.conflict_paths = saved_default_conflicts(root);
        if !value.conflict_paths.is_empty() {
            value.reason = Some("first_receive_conflict");
        }
    }
    value
}

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
            if receive_connected_with_progress(&tool, &root, &db, &target, branch, Some(&app))
                .is_err()
            {
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
    chat_backup: &'static str,
    reason: Option<&'static str>,
    history_reason: Option<&'static str>,
    repo: Option<String>,
    visibility: Option<String>,
    last_pushed_at: Option<String>,
    ahead: Option<u64>,
    behind: Option<u64>,
    signed_in_as: Option<String>,
    auto_merged: bool,
    preserved_documents: Vec<String>,
    conflict_paths: Option<Vec<String>>,
    uploaded_key_found: Option<bool>,
    expired_key_warning: Option<bool>,
}

impl RemoteStatus {
    fn new(state: &'static str, reason: Option<&'static str>) -> Self {
        Self {
            state,
            chat_backup: "included",
            reason,
            history_reason: None,
            repo: None,
            visibility: None,
            last_pushed_at: None,
            ahead: None,
            behind: None,
            signed_in_as: None,
            auto_merged: false,
            preserved_documents: Vec::new(),
            conflict_paths: None,
            uploaded_key_found: None,
            expired_key_warning: crate::redaction_wait::counts()
                .ok()
                .map(|(_, expired)| expired > 0),
        }
    }

    fn transfer_error(error: &'static str) -> Self {
        if error == "redaction_required" {
            return Self::new("redactionRequired", Some(error));
        }
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
fn git_output(tool: &Path, root: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    hidden(&mut Command::new(tool))
        .current_dir(root)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("LANGUAGE", "C")
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
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
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
}

fn git_raw(tool: &Path, root: &Path, args: &[&str]) -> Result<String, ()> {
    let output = git_output(tool, root, args).map_err(|_| ())?;
    if !output.status.success() {
        return Err(());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn classify_remote_error(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("remote: error: gh001") || line.starts_with("remote: gh001")
    }) || lower.contains("pack exceeds maximum allowed size")
        || lower.contains("exceeds github's file size limit")
    {
        "size_limit"
    } else if lower.contains("authentication failed")
        || lower.contains("could not read username")
        || lower.contains("bad credentials")
        || lower.contains("http 401")
        || lower.contains("returned error: 401")
    {
        "authentication_failed"
    } else if lower.contains("permission to ") && lower.contains(" denied to ") {
        "permission_denied"
    } else if lower.contains("repository not found")
        || (lower.contains("repository '") && lower.contains("' not found"))
        || lower.contains("returned error: 404")
        || lower.contains("http 404")
    {
        "repository_unavailable_or_denied"
    } else if lower.contains("could not resolve host")
        || lower.contains("failed to connect")
        || lower.contains("error connecting to")
        || lower.contains("connection timed out")
        || lower.contains("connection reset by peer")
    {
        "connection_failed"
    } else {
        "remote_unavailable"
    }
}

fn remote_git(tool: &Path, root: &Path, args: &[&str]) -> Result<String, &'static str> {
    let output = git_output(tool, root, args).map_err(|_| "git_unavailable")?;
    if !output.status.success() {
        return Err(classify_remote_error(&String::from_utf8_lossy(
            &output.stderr,
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
pub(crate) fn git(tool: &Path, root: &Path, args: &[&str]) -> Result<String, ()> {
    git_raw(tool, root, args).map(|s| s.trim().to_owned())
}

fn git_index(tool: &Path, root: &Path, index: &Path, args: &[&str]) -> Result<String, ()> {
    let output = hidden(&mut Command::new(tool))
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_INDEX_FILE", index)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| ())?;
    if !output.status.success() {
        return Err(());
    }
    String::from_utf8(output.stdout).map_err(|_| ())
}

fn git_hash_object(tool: &Path, root: &Path, content: &[u8]) -> Result<String, ()> {
    git_hash_typed(tool, root, "blob", content)
}

fn git_hash_typed(tool: &Path, root: &Path, kind: &str, content: &[u8]) -> Result<String, ()> {
    use std::io::Write;
    let mut child = hidden(&mut Command::new(tool))
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .args(["hash-object", "-t", kind, "-w", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ())?;
    let written = child
        .stdin
        .take()
        .ok_or(())
        .and_then(|mut input| input.write_all(content).map_err(|_| ()));
    if written.is_err() {
        let _ = child.wait();
        return Err(());
    }
    let output = child.wait_with_output().map_err(|_| ())?;
    if !output.status.success() {
        return Err(());
    }
    Ok(String::from_utf8(output.stdout)
        .map_err(|_| ())?
        .trim()
        .to_owned())
}

fn merge_added_terms(base: &str, local: &str, remote: &str) -> Result<Vec<u8>, ()> {
    use std::collections::{BTreeMap, BTreeSet};
    fn parse(
        text: &str,
    ) -> Result<
        (
            serde_json::Map<String, serde_json::Value>,
            Vec<serde_json::Value>,
        ),
        (),
    > {
        let mut object = serde_json::from_str::<serde_json::Value>(text)
            .map_err(|_| ())?
            .as_object()
            .cloned()
            .ok_or(())?;
        let terms = object
            .remove("terms")
            .and_then(|v| v.as_array().cloned())
            .ok_or(())?;
        Ok((object, terms))
    }
    fn index(terms: &[serde_json::Value]) -> Result<BTreeMap<String, &serde_json::Value>, ()> {
        let mut map = BTreeMap::new();
        for term in terms {
            let id = term
                .pointer("/term/id")
                .and_then(serde_json::Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or(())?;
            if map.insert(id.to_owned(), term).is_some() {
                return Err(());
            }
        }
        Ok(map)
    }
    let (base_meta, base_terms) = parse(base)?;
    let (local_meta, local_terms) = parse(local)?;
    let (remote_meta, remote_terms) = parse(remote)?;
    if base_meta != local_meta || base_meta != remote_meta {
        return Err(());
    }
    let base_index = index(&base_terms)?;
    let local_index = index(&local_terms)?;
    let remote_index = index(&remote_terms)?;
    if base_index
        .iter()
        .any(|(id, term)| local_index.get(id) != Some(term) || remote_index.get(id) != Some(term))
    {
        return Err(());
    }
    let preserves_order = |items: &[serde_json::Value]| {
        items
            .iter()
            .filter(|entry| {
                entry
                    .pointer("/term/id")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|id| base_index.contains_key(id))
            })
            .eq(base_terms.iter())
    };
    if !preserves_order(&local_terms) || !preserves_order(&remote_terms) {
        return Err(());
    }
    let mut seen: BTreeSet<_> = base_index.keys().cloned().collect();
    let mut merged = base_terms;
    for term in local_terms.iter().chain(&remote_terms) {
        let id = term
            .pointer("/term/id")
            .and_then(serde_json::Value::as_str)
            .ok_or(())?;
        if !seen.insert(id.to_owned()) {
            if local_index.contains_key(id)
                && remote_index.contains_key(id)
                && local_index.get(id) != remote_index.get(id)
            {
                return Err(());
            }
            continue;
        }
        merged.push(term.clone());
    }
    let mut result = base_meta;
    result.insert("terms".into(), serde_json::Value::Array(merged));
    let mut bytes =
        serde_json::to_vec_pretty(&serde_json::Value::Object(result)).map_err(|_| ())?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn merge_people(base: &str, local: &str, remote: &str) -> Result<(Vec<u8>, bool), ()> {
    use std::collections::{BTreeMap, BTreeSet};
    type PeopleData = (
        serde_json::Map<String, serde_json::Value>,
        BTreeMap<String, serde_json::Value>,
    );
    fn parse(text: &str) -> Result<PeopleData, ()> {
        let mut obj = serde_json::from_str::<serde_json::Value>(text)
            .map_err(|_| ())?
            .as_object()
            .cloned()
            .ok_or(())?;
        let people = obj
            .remove("people")
            .and_then(|v| v.as_array().cloned())
            .ok_or(())?;
        let mut indexed = BTreeMap::new();
        for person in people {
            let id = person
                .get("id")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or(())?
                .to_owned();
            if person
                .get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .is_none()
                || indexed.insert(id, person).is_some()
            {
                return Err(());
            }
        }
        Ok((obj, indexed))
    }
    let (meta, old) = parse(base)?;
    let (left_meta, left) = parse(local)?;
    let (right_meta, right) = parse(remote)?;
    if meta != left_meta || meta != right_meta {
        return Err(());
    }
    let mut conflicts = false;
    let ids: BTreeSet<_> = old
        .keys()
        .chain(left.keys())
        .chain(right.keys())
        .cloned()
        .collect();
    let mut people = Vec::new();
    for id in ids {
        let (before, a, b) = (old.get(&id), left.get(&id), right.get(&id));
        let chosen = if a == b {
            a
        } else if a == before {
            b
        } else if b == before {
            a
        } else {
            let (a, b) = (a.ok_or(())?, b.ok_or(())?);
            let confirmed = |p: &serde_json::Value| {
                p.get("confirmed")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
            };
            match (confirmed(a), confirmed(b)) {
                (true, false) => Some(a),
                (false, true) => Some(b),
                (true, true) => {
                    conflicts = true;
                    Some(b)
                }
                (false, false) => return Err(()),
            }
        };
        if let Some(person) = chosen {
            people.push(person.clone());
        }
    }
    let mut result = meta;
    result.insert("people".into(), serde_json::Value::Array(people));
    let mut bytes =
        serde_json::to_vec_pretty(&serde_json::Value::Object(result)).map_err(|_| ())?;
    bytes.push(b'\n');
    Ok((bytes, conflicts))
}
fn gh_output(args: &[&str]) -> std::io::Result<std::process::Output> {
    hidden(&mut Command::new("gh"))
        .env("GH_HOST", "github.com")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("LANGUAGE", "C")
        .args(args)
        .stdin(Stdio::null())
        .output()
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
    let auth = gh_output(&["auth", "status", "--active", "--hostname", "github.com"])
        .map_err(|_| "gh_missing")?;
    if !auth.status.success() {
        let message = format!(
            "{}\n{}",
            String::from_utf8_lossy(&auth.stdout),
            String::from_utf8_lossy(&auth.stderr)
        )
        .to_ascii_lowercase();
        let classified = classify_remote_error(&message);
        return Err(if classified == "connection_failed" {
            classified
        } else if message.contains("not logged into")
            || message.contains("failed to log in")
            || message.contains("token is invalid")
        {
            "signed_out"
        } else {
            "authentication_unavailable"
        });
    }
    let user = gh_output(&["api", "--hostname", "github.com", "user", "--jq", ".login"])
        .map_err(|_| "gh_missing")?;
    if !user.status.success() {
        let error = classify_remote_error(&String::from_utf8_lossy(&user.stderr));
        return Err(
            if error == "connection_failed" || error == "authentication_failed" {
                error
            } else {
                "account_unavailable"
            },
        );
    }
    let login = String::from_utf8_lossy(&user.stdout).trim().to_owned();
    if login.is_empty() {
        return Err("account_unavailable");
    }
    Ok(login)
}
fn github(repo: &str) -> Result<(Repository, String), &'static str> {
    if !valid_repo(repo) {
        return Err("invalid_repository");
    }
    let user = signed_in()?;
    let result = gh_output(&[
        "repo",
        "view",
        repo,
        "--json",
        "nameWithOwner,isPrivate,defaultBranchRef",
    ])
    .map_err(|_| "gh_missing")?;
    if !result.status.success() {
        return Err(classify_remote_error(&String::from_utf8_lossy(
            &result.stderr,
        )));
    }
    let data = String::from_utf8_lossy(&result.stdout);
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
#[cfg(test)]
pub(crate) fn receive_connected(
    tool: &Path,
    root: &Path,
    db: &Path,
    target: &str,
    branch: &str,
) -> Result<(), &'static str> {
    receive_connected_with_progress::<tauri::test::MockRuntime>(
        tool, root, db, target, branch, None,
    )
}

fn receive_connected_with_progress<R: tauri::Runtime>(
    tool: &Path,
    root: &Path,
    db: &Path,
    target: &str,
    branch: &str,
    app: Option<&tauri::AppHandle<R>>,
) -> Result<(), &'static str> {
    let pending = read_receive_journal(db)?;
    let first = pending.is_some() || git(tool, root, &["rev-parse", "--verify", "HEAD"]).is_err();
    if first {
        let result = receive_first_with_progress(
            tool,
            root,
            db,
            target,
            branch,
            |progress| {
                if let Some(app) = app {
                    let _ = app.emit("archive-first-receive-progress", progress);
                }
            },
            || {
                if let Some(app) = app {
                    crate::chat_archive_store::restore_emitting(root, db, app)
                } else {
                    crate::chat_archive_store::restore(root, db)
                }
            },
        );
        if let Err(error) = result {
            let value = receive_failure_progress(root, error);
            remember_receive_progress(root, db, &value);
            if let Some(app) = app {
                let _ = app.emit("archive-first-receive-progress", value);
            }
        }
        return result;
    }
    transfer(tool, root, target, branch, "pull")?;
    crate::archive::reindex(root, db).map_err(|_| "reindex_failed")?;
    if let Some(app) = app {
        crate::chat_archive_store::restore_emitting(root, db, app)
    } else {
        crate::chat_archive_store::restore(root, db)
    }
}

fn receive_first_with_progress(
    tool: &Path,
    root: &Path,
    db: &Path,
    target: &str,
    branch: &str,
    progress: impl FnMut(FirstReceiveProgress),
    restore: impl FnMut() -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let _lock = lock_first_receive(db)?;
    receive_first_locked(tool, root, db, target, branch, progress, restore)
}

fn receive_first_locked(
    tool: &Path,
    root: &Path,
    db: &Path,
    target: &str,
    branch: &str,
    mut progress: impl FnMut(FirstReceiveProgress),
    mut restore: impl FnMut() -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let mut report = |value: FirstReceiveProgress| {
        remember_receive_progress(root, db, &value);
        progress(value);
    };
    let mut journal = {
        let _app_guard = crate::chat_file_changes::lock_app_write();
        let _write_guard = crate::archive_zet::WRITE_LOCK.lock().map_err(|_| "busy")?;
        if crate::chat_file_changes::has_active(root) {
            return Err("busy");
        }
        begin_receive_journal(db, root, target, branch)?
    };
    remember_chat_deletions(db, &mut journal)?;
    let resumed = journal.phase != "receiving" || receive_marker_present(root);
    report(receive_progress(
        if resumed { "recovering" } else { "receiving" },
        0,
        false,
    ));
    // C28 could persist the indexing phase without its checkout revision.
    // The observed remote ref identifies it only if it still equals HEAD.
    if journal.revision.is_none() && matches!(journal.phase.as_str(), "indexing" | "restoring") {
        let head = git(tool, root, &["rev-parse", "--verify", "HEAD"])
            .map_err(|_| "first_receive_interrupted")?;
        if git(
            tool,
            root,
            &["rev-parse", "--verify", "refs/remotes/momso/observed"],
        )
        .as_deref()
            != Ok(head.as_str())
        {
            return Err("first_receive_interrupted");
        }
        journal.revision = Some(head);
        write_receive_journal(db, &journal)?;
    }
    if let Some(revision) = journal.revision.as_deref() {
        if matches!(
            journal.phase.as_str(),
            "checkout" | "indexing" | "restoring"
        ) {
            let head = git(tool, root, &["rev-parse", "--verify", "HEAD"]);
            let observed = git(
                tool,
                root,
                &["rev-parse", "--verify", "refs/remotes/momso/observed"],
            );
            if head.as_deref() == Ok(revision) && observed.as_deref() == Ok(revision) {
                let _app_write = crate::chat_file_changes::lock_app_write();
                let _write = crate::archive_zet::WRITE_LOCK.lock().map_err(|_| "busy")?;
                restore_missing_received_files(tool, root, revision, &journal.deleted_chats)?;
            } else if head.is_ok() || journal.phase != "checkout" {
                return Err("first_receive_interrupted");
            }
        }
    }
    if matches!(journal.phase.as_str(), "receiving" | "checkout") {
        let head = git(tool, root, &["rev-parse", "--verify", "HEAD"]).ok();
        let transfer_result = if let Some(head) = head {
            // A completed checkout can be followed by local edits.  Do not
            // pull again or discard them; only accept the exact fetched head.
            if (journal.phase == "checkout" && journal.revision.as_deref() != Some(head.as_str()))
                || git(
                    tool,
                    root,
                    &["rev-parse", "--verify", "refs/remotes/momso/observed"],
                )
                .as_deref()
                    != Ok(head.as_str())
            {
                Err("first_receive_interrupted")
            } else if receive_marker_present(root) {
                recover_completed_checkout(tool, root, db, &journal)
            } else {
                Ok(())
            }
        } else {
            let mut ignored_merge = None;
            transfer_with_merge_revision(
                tool,
                root,
                target,
                branch,
                "pull",
                &mut ignored_merge,
                Some(db),
            )
            .map(|_| ())
        };
        if let Err(error) = transfer_result {
            report(receive_failure_progress(root, error));
            return Err(error);
        }
        if git(tool, root, &["rev-parse", "--verify", "HEAD"]).is_err() {
            finish_receive_journal(db)?;
            report(receive_progress("empty", 0, false));
            return Ok(());
        }
        // Checkout records its exact revision in the same journal. Do not
        // replace that durable update with the older in-memory receiving copy.
        journal = read_receive_journal(db)?.ok_or("first_receive_interrupted")?;
        journal.phase = "indexing".to_owned();
        write_receive_journal(db, &journal)?;
    }
    // Rebuilding the index is transactional and reads the current files.  On
    // restart repeat it even if the preceding attempt already committed.
    report(receive_progress("indexing", 1, false));
    if crate::archive::reindex(root, db).is_err() {
        report(receive_progress("failed", 1, false));
        return Err("reindex_failed");
    }
    journal.phase = "restoring".to_owned();
    remember_chat_deletions(db, &mut journal)?;
    write_receive_journal(db, &journal)?;
    report(receive_progress("restoring", 2, false));
    if let Err(error) = restore() {
        report(receive_progress("failed", 2, false));
        return Err(error);
    }
    finish_receive_journal(db)?;
    report(receive_progress("complete", 3, true));
    Ok(())
}

pub(crate) fn first_receive_pending(root: &Path, db: &Path) -> bool {
    receive_marker_present(root) || !matches!(read_receive_journal(db), Ok(None))
}

pub(crate) fn ensure_document_write_allowed(root: &Path, db: &Path) -> Result<(), String> {
    if first_receive_pending(root, db) {
        Err("first_receive_in_progress".to_owned())
    } else {
        Ok(())
    }
}

#[tauri::command]
pub(crate) async fn archive_first_receive_status(
    state: State<'_, crate::AppState>,
) -> Result<FirstReceiveProgress, String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    Ok(current_first_receive_progress(
        &root,
        &state.paths.database_path,
    ))
}

fn current_first_receive_progress(root: &Path, db: &Path) -> FirstReceiveProgress {
    let journal = read_receive_journal(db);
    if journal.is_err() || matches!(&journal, Ok(Some(saved)) if saved.root != root) {
        return receive_progress("interrupted", 0, false);
    }
    let pending = receive_marker_present(root) || !matches!(&journal, Ok(None));
    let collision = receive_failure_progress(root, "first_receive_interrupted");
    if !collision.conflict_paths.is_empty() {
        return collision;
    }
    if let Ok(states) = FIRST_RECEIVE_PROGRESS.lock() {
        if let Some((_, _, progress)) = states
            .iter()
            .find(|(stored_root, stored_db, _)| stored_root == root && stored_db == db)
        {
            if !(pending && progress.available) {
                return progress.clone();
            }
        }
    }
    match journal {
        Ok(Some(journal)) if journal.root == root => match journal.phase.as_str() {
            "discovering" | "receiving" | "checkout" => receive_progress("recovering", 0, false),
            "indexing" => receive_progress("indexing", 1, false),
            "restoring" => receive_progress("restoring", 2, false),
            _ => receive_progress("interrupted", 0, false),
        },
        Ok(None) if !receive_marker_present(root) => receive_progress("idle", 0, true),
        _ => receive_progress("interrupted", 0, false),
    }
}

pub(crate) fn start_first_receive_recovery<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    paths: crate::runtime::RuntimePaths,
    resources: PathBuf,
) {
    #[cfg(test)]
    let test_remote = app
        .try_state::<TestRemote>()
        .map(|remote| remote.inner().clone());
    tauri::async_runtime::spawn(async move {
        let mut delay = std::time::Duration::from_secs(30);
        loop {
            let root = paths.archive.current.clone().read_owned().await;
            let db = paths.database_path.clone();
            let report_root = root.to_path_buf();
            let report_db = db.clone();
            let resources = resources.clone();
            let task_app = app.clone();
            #[cfg(test)]
            let test_remote = test_remote.clone();
            let outcome = tauri::async_runtime::spawn_blocking(move || {
                let _guard = REMOTE_LOCK.lock().map_err(|_| "busy")?;
                let _receive_lock = lock_first_receive(&db)?;
                let mut pending = read_receive_journal(&db)?;
                let internal = receive_marker_present(&root);
                #[cfg(test)]
                let tool_path = test_remote
                    .as_ref()
                    .map(|remote| remote.tool.clone())
                    .or_else(|| git_path(&resources));
                #[cfg(not(test))]
                let tool_path = git_path(&resources);
                let tool = match tool_path {
                    Some(tool) => tool,
                    None if pending.is_none() && !internal => return Ok(()),
                    None => return Err("git_unavailable"),
                };
                if pending.is_none()
                    && !internal
                    && git(&tool, &root, &["rev-parse", "--verify", "HEAD"]).is_ok()
                {
                    return Ok(());
                }
                #[cfg(test)]
                let configured_repo = test_remote
                    .as_ref()
                    .map(|_| "fixture/repository".to_owned())
                    .or_else(|| configured(&tool, &root));
                #[cfg(not(test))]
                let configured_repo = configured(&tool, &root);
                let repo = match configured_repo {
                    Some(repo) => repo,
                    None if pending.is_none() && !internal => return Ok(()),
                    None => return Err("first_receive_interrupted"),
                };
                #[cfg(test)]
                let target = test_remote
                    .as_ref()
                    .map(|remote| remote.target.clone())
                    .unwrap_or_else(|| url(&repo));
                #[cfg(not(test))]
                let target = url(&repo);
                if pending.is_some() || internal {
                    let value = receive_progress("recovering", 0, false);
                    remember_receive_progress(&root, &db, &value);
                    let _ = task_app.emit("archive-first-receive-progress", value);
                }
                if internal && git(&tool, &root, &["rev-parse", "--verify", "HEAD"]).is_err() {
                    let _app_write = crate::chat_file_changes::lock_app_write();
                    let _write = crate::archive_zet::WRITE_LOCK.lock().map_err(|_| "busy")?;
                    if crate::chat_file_changes::has_active(&root) {
                        return Err("busy");
                    }
                    recover_interrupted_first_receive(&tool, &root)?;
                }
                if pending.is_none() {
                    if !internal && !fresh_archive(&tool, &root) {
                        return Ok(());
                    }
                    let journal = ReceiveJournal {
                        root: root.to_path_buf(),
                        target: target.clone(),
                        branch: String::new(),
                        phase: "discovering".into(),
                        revision: None,
                        has_defaults: None,
                        restore_intents: Vec::new(),
                        restore_done: Vec::new(),
                        deleted_chats: Vec::new(),
                    };
                    {
                        let _app_write = crate::chat_file_changes::lock_app_write();
                        let _write = crate::archive_zet::WRITE_LOCK.lock().map_err(|_| "busy")?;
                        if crate::chat_file_changes::has_active(&root) {
                            return Err("busy");
                        }
                        write_receive_journal(&db, &journal)?;
                    }
                    pending = Some(journal);
                    let value = receive_progress("recovering", 0, false);
                    remember_receive_progress(&root, &db, &value);
                    let _ = task_app.emit("archive-first-receive-progress", value);
                }
                let mut remote_verified = false;
                let branch = if let Some(journal) = pending.as_mut() {
                    if journal.root != *root || journal.target != target {
                        return Err("first_receive_interrupted");
                    }
                    if journal.phase == "discovering" {
                        #[cfg(test)]
                        let remote = if test_remote.is_some() {
                            Repository {
                                name_with_owner: repo.clone(),
                                is_private: true,
                                default_branch_ref: None,
                            }
                        } else {
                            github(&repo)?.0
                        };
                        #[cfg(not(test))]
                        let remote = github(&repo)?.0;
                        remote_verified = true;
                        journal.branch = remote
                            .default_branch_ref
                            .map(|b| b.name)
                            .unwrap_or_else(|| "main".into());
                        journal.phase = "receiving".into();
                        write_receive_journal(&db, journal)?;
                    }
                    journal.branch.clone()
                } else {
                    return Err("first_receive_interrupted");
                };
                let checkout_local = pending.as_ref().is_some_and(|journal| {
                    journal.phase == "checkout"
                        && journal.revision.as_deref().is_some_and(|revision| {
                            git(&tool, &root, &["rev-parse", "--verify", "HEAD"]).as_deref()
                                == Ok(revision)
                                && git(
                                    &tool,
                                    &root,
                                    &["rev-parse", "--verify", "refs/remotes/momso/observed"],
                                )
                                .as_deref()
                                    == Ok(revision)
                        })
                });
                if !remote_verified
                    && !checkout_local
                    && pending
                        .as_ref()
                        .is_none_or(|j| matches!(j.phase.as_str(), "receiving" | "checkout"))
                {
                    #[cfg(test)]
                    let remote = if test_remote.is_some() {
                        Repository {
                            name_with_owner: repo.clone(),
                            is_private: true,
                            default_branch_ref: None,
                        }
                    } else {
                        github(&repo)?.0
                    };
                    #[cfg(not(test))]
                    let remote = github(&repo)?.0;
                    let actual = remote
                        .default_branch_ref
                        .map(|b| b.name)
                        .unwrap_or_else(|| "main".into());
                    if actual != branch {
                        return Err("first_receive_interrupted");
                    }
                }
                receive_first_locked(
                    &tool,
                    &root,
                    &db,
                    &target,
                    &branch,
                    |value| {
                        let _ = task_app.emit("archive-first-receive-progress", value);
                    },
                    || crate::chat_archive_store::restore_emitting(&root, &db, &task_app),
                )
            })
            .await;
            match outcome {
                Ok(Ok(())) => break,
                Ok(Err(
                    "connection_failed"
                    | "remote_unavailable"
                    | "authentication_unavailable"
                    | "account_unavailable"
                    | "signed_out"
                    | "authentication_failed"
                    | "git_unavailable"
                    | "busy",
                )) => {
                    let progress = receive_progress("failed", 0, false);
                    remember_receive_progress(&report_root, &report_db, &progress);
                    let _ = app.emit("archive-first-receive-progress", progress);
                    tokio::time::sleep(delay).await;
                    delay = delay
                        .saturating_mul(2)
                        .min(std::time::Duration::from_secs(300));
                }
                _ => {
                    let progress = receive_progress("interrupted", 0, false);
                    remember_receive_progress(&report_root, &report_db, &progress);
                    let _ = app.emit("archive-first-receive-progress", progress);
                    break;
                }
            }
        }
    });
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
            || safe_chat_path(p)
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

/// Inspect committed chat snapshots in memory only; no key is passed to Git or logged.
fn chat_snapshot_at(
    tool: &Path,
    root: &Path,
    revision: &str,
    path: &str,
) -> Result<Option<serde_json::Value>, &'static str> {
    if !safe_chat_path(path) {
        return Err("unsafe_history");
    }
    let entry =
        git_raw(tool, root, &["ls-tree", revision, "--", path]).map_err(|_| "git_unavailable")?;
    if entry.is_empty() {
        return Ok(None);
    }
    let (meta, actual) = entry
        .trim_end_matches('\n')
        .split_once('\t')
        .ok_or("unsafe_history")?;
    let mut parts = meta.split_whitespace();
    if parts.next() != Some("100644") || parts.next() != Some("blob") || actual != path {
        return Err("unsafe_history");
    }
    let oid = parts.next().ok_or("unsafe_history")?;
    if parts.next().is_some() || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("unsafe_history");
    }
    let size: usize = git(tool, root, &["cat-file", "-s", oid])
        .map_err(|_| "git_unavailable")?
        .parse()
        .map_err(|_| "unsafe_history")?;
    if size > 32 * 1024 * 1024 {
        return Err("unsafe_history");
    }
    let content = git_raw(tool, root, &["cat-file", "blob", oid]).map_err(|_| "git_unavailable")?;
    let snapshot = serde_json::from_str(&content).map_err(|_| "unsafe_history")?;
    Ok(Some(snapshot))
}

fn uploaded_chat_contains_key(
    tool: &Path,
    root: &Path,
    revision: &str,
    known: &[&str],
) -> Result<bool, &'static str> {
    if known.is_empty() {
        return Ok(false);
    }
    let names = git_raw(
        tool,
        root,
        &[
            "ls-tree",
            "-r",
            "--name-only",
            "-z",
            revision,
            "--",
            "chats/",
        ],
    )
    .map_err(|_| "git_unavailable")?;
    for path in names.split('\0').filter(|p| !p.is_empty()) {
        let snapshot = chat_snapshot_at(tool, root, revision, path)?.ok_or("unsafe_history")?;
        if crate::chat_archive_snapshot::contains_stored_secret(&snapshot, known) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn specific_uploaded_key_found(any_uploaded: bool, known: &[&str]) -> Option<bool> {
    if !any_uploaded {
        return Some(false);
    }
    let distinct: std::collections::HashSet<_> = known
        .iter()
        .copied()
        .filter(|key| !key.is_empty())
        .collect();
    if distinct.len() == 1 {
        Some(true)
    } else {
        None
    }
}

fn unpushed_chat_needs_redaction(
    tool: &Path,
    root: &Path,
    revision: &str,
    base: Option<&str>,
    known: &[&str],
) -> Result<bool, &'static str> {
    if known.is_empty() {
        return Ok(false);
    }
    let mut args = vec!["rev-list", "--reverse", revision];
    let exclude = base.map(|base| format!("^{base}"));
    if let Some(exclude) = exclude.as_deref() {
        args.push(exclude);
    }
    let commits = git_raw(tool, root, &args).map_err(|_| "git_unavailable")?;
    let mut checked = std::collections::HashSet::new();
    for commit in commits.lines() {
        if !(commit.len() == 40 || commit.len() == 64)
            || !commit.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("unsafe_history");
        }
        let entries = git_raw(tool, root, &["ls-tree", "-r", "-z", commit, "--", "chats/"])
            .map_err(|_| "git_unavailable")?;
        for entry in entries.split('\0').filter(|entry| !entry.is_empty()) {
            let (metadata, path) = entry.split_once('\t').ok_or("unsafe_history")?;
            let mut fields = metadata.split_whitespace();
            if fields.next() != Some("100644")
                || fields.next() != Some("blob")
                || !safe_chat_path(path)
            {
                return Err("unsafe_history");
            }
            let oid = fields.next().ok_or("unsafe_history")?;
            if fields.next().is_some() || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("unsafe_history");
            }
            if !checked.insert(oid.to_owned()) {
                continue;
            }
            let snapshot = chat_snapshot_at(tool, root, commit, path)?.ok_or("unsafe_history")?;
            match crate::chat_archive_snapshot::redact_historical_snapshot(&snapshot, known) {
                Ok(clean) if clean == snapshot => {}
                Ok(_) | Err("sensitive_metadata") => return Ok(true),
                Err(_) => return Err("unsafe_history"),
            }
        }
    }
    Ok(false)
}

/// Build replacement commits in a private temporary index without moving any ref.
/// Document trees and original commit metadata are kept; only chat blobs are redacted.
fn redacted_history_candidate(
    tool: &Path,
    root: &Path,
    revision: &str,
    base: Option<&str>,
    known: &[&str],
) -> Result<String, &'static str> {
    use std::collections::HashMap;
    let mut args = vec!["rev-list", "--reverse", "--topo-order", revision];
    let exclude = base.map(|id| format!("^{id}"));
    if let Some(exclude) = exclude.as_deref() {
        args.push(exclude);
    }
    let commits = git_raw(tool, root, &args).map_err(|_| "git_unavailable")?;
    let temp = tempfile::tempdir().map_err(|_| "local_state_unavailable")?;
    let index = temp.path().join("index");
    let mut rewritten: HashMap<String, String> = HashMap::new();
    let mut blob_cache: HashMap<String, String> = HashMap::new();
    for commit in commits.lines() {
        if !(commit.len() == 40 || commit.len() == 64)
            || !commit.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("unsafe_history");
        }
        let output = git_output(tool, root, &["cat-file", "commit", commit])
            .map_err(|_| "git_unavailable")?;
        if !output.status.success() {
            return Err("unsafe_history");
        }
        let raw = String::from_utf8(output.stdout).map_err(|_| "unsafe_history")?;
        let (header, message) = raw.split_once("\n\n").ok_or("unsafe_history")?;
        if known.iter().any(|key| !key.is_empty() && raw.contains(key)) {
            return Err("redaction_required");
        }
        if header.lines().any(|line| {
            line.starts_with("gpgsig ")
                || line.starts_with("gpgsig-sha256 ")
                || line.starts_with("mergetag ")
        }) {
            return Err("redaction_required");
        }
        git_index(tool, root, &index, &["read-tree", "--reset", commit])
            .map_err(|_| "candidate_read_tree")?;
        let entries = git_raw(tool, root, &["ls-tree", "-r", "-z", commit, "--", "chats/"])
            .map_err(|_| "git_unavailable")?;
        for entry in entries.split('\0').filter(|entry| !entry.is_empty()) {
            let (metadata, path) = entry.split_once('\t').ok_or("unsafe_history")?;
            if !safe_chat_path(path) {
                return Err("unsafe_history");
            }
            let mut parts = metadata.split_whitespace();
            if parts.next() != Some("100644") || parts.next() != Some("blob") {
                return Err("unsafe_history");
            }
            let oid = parts.next().ok_or("unsafe_history")?;
            if parts.next().is_some() || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("unsafe_history");
            }
            let cleaned = if let Some(cleaned) = blob_cache.get(oid) {
                cleaned.clone()
            } else {
                let snapshot =
                    chat_snapshot_at(tool, root, commit, path)?.ok_or("unsafe_history")?;
                let clean =
                    crate::chat_archive_snapshot::redact_historical_snapshot(&snapshot, known)?;
                let new_oid = if clean == snapshot {
                    oid.to_owned()
                } else {
                    let bytes = serde_json::to_vec(&clean).map_err(|_| "unsafe_history")?;
                    git_hash_object(tool, root, &bytes).map_err(|_| "candidate_blob")?
                };
                blob_cache.insert(oid.to_owned(), new_oid.clone());
                new_oid
            };
            if cleaned != oid {
                let entry = format!("100644,{cleaned},{path}");
                git_index(
                    tool,
                    root,
                    &index,
                    &["update-index", "--add", "--cacheinfo", &entry],
                )
                .map_err(|_| "candidate_update_index")?;
            }
        }
        let tree = git_index(tool, root, &index, &["write-tree"])
            .map_err(|_| "candidate_write_tree")?
            .trim()
            .to_owned();
        let mut lines = Vec::new();
        let mut trees = 0;
        for line in header.split('\n') {
            if line.starts_with("tree ") {
                trees += 1;
                lines.push(format!("tree {tree}"));
            } else if let Some(parent) = line.strip_prefix("parent ") {
                let parent = if let Some(mapped) = rewritten.get(parent) {
                    mapped.as_str()
                } else if let Some(base) = base {
                    if git(tool, root, &["merge-base", "--is-ancestor", parent, base]).is_err() {
                        return Err("redaction_required");
                    }
                    parent
                } else {
                    return Err("redaction_required");
                };
                lines.push(format!("parent {parent}"));
            } else {
                lines.push(line.to_owned());
            }
        }
        if trees != 1 {
            return Err("unsafe_history");
        }
        let cleaned_commit = format!("{}\n\n{}", lines.join("\n"), message);
        let oid = git_hash_typed(tool, root, "commit", cleaned_commit.as_bytes())
            .map_err(|_| "candidate_commit")?;
        rewritten.insert(commit.to_owned(), oid);
    }
    rewritten.get(revision).cloned().ok_or("redaction_required")
}
fn safe_chat_path(path: &str) -> bool {
    path.strip_prefix("chats/")
        .and_then(|name| name.strip_suffix(".json"))
        .is_some_and(crate::archive_receipt::valid_chat_id)
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

fn preserved_documents(tool: &Path, root: &Path, revision: &str) -> Result<Vec<String>, ()> {
    let parents = git(tool, root, &["rev-list", "--parents", "-n", "1", revision])?;
    let parts: Vec<_> = parents.split_whitespace().collect();
    if parts.len() != 3 {
        return Err(());
    }
    let base = git(tool, root, &["merge-base", "--all", parts[1], parts[2]])?;
    if base.lines().count() != 1 {
        return Err(());
    }
    let left = git_raw(
        tool,
        root,
        &["diff", "--name-only", "-z", "--no-renames", &base, parts[1]],
    )?;
    let right = git_raw(
        tool,
        root,
        &["diff", "--name-only", "-z", "--no-renames", &base, parts[2]],
    )?;
    let right: std::collections::HashSet<_> = right.split('\0').collect();
    let mut result = Vec::new();
    for path in left.split('\0').filter(|path| right.contains(path)) {
        let Some(id) = path
            .strip_prefix("zettels/")
            .and_then(|p| p.strip_suffix(".md"))
            .filter(|id| crate::archive_zet::valid_zet_id(id))
        else {
            continue;
        };
        let local = git(
            tool,
            root,
            &["rev-parse", "--verify", &format!("{}:{path}", parts[1])],
        )?;
        let remote = git(
            tool,
            root,
            &["rev-parse", "--verify", &format!("{}:{path}", parts[2])],
        )?;
        if local != remote {
            result.push(id.to_owned());
        }
    }
    result.sort();
    Ok(result)
}

fn merged_people_conflict(tool: &Path, root: &Path, revision: &str) -> Result<bool, ()> {
    let parents = git(tool, root, &["rev-list", "--parents", "-n", "1", revision])?;
    let parts: Vec<_> = parents.split_whitespace().collect();
    if parts.len() != 3 {
        return Err(());
    }
    let base = git(tool, root, &["merge-base", "--all", parts[1], parts[2]])?;
    if base.lines().count() != 1 {
        return Err(());
    }
    let path = "dictionary/people.json";
    let changed = |side: &str| -> Result<bool, ()> {
        Ok(git_raw(
            tool,
            root,
            &["diff", "--name-only", "-z", "--no-renames", &base, side],
        )?
        .split('\0')
        .any(|p| p == path))
    };
    if !changed(parts[1])? || !changed(parts[2])? {
        return Ok(false);
    }
    let data = |rev: &str| git_raw(tool, root, &["show", &format!("{rev}:{path}")]);
    let (_, conflicts) = merge_people(&data(&base)?, &data(parts[1])?, &data(parts[2])?)?;
    Ok(conflicts)
}

fn conflicting_paths(tool: &Path, root: &Path) -> Result<Vec<String>, ()> {
    let local = git(tool, root, &["rev-parse", "--verify", "HEAD"])?;
    let remote = git(
        tool,
        root,
        &["rev-parse", "--verify", "refs/remotes/momso/observed"],
    )?;
    let base = git(tool, root, &["merge-base", "--all", &local, &remote])?;
    if base.lines().count() != 1 {
        return Err(());
    }
    let left = git_raw(
        tool,
        root,
        &["diff", "--name-only", "-z", "--no-renames", &base, &local],
    )?;
    let right = git_raw(
        tool,
        root,
        &["diff", "--name-only", "-z", "--no-renames", &base, &remote],
    )?;
    let right: std::collections::HashSet<_> = right.split('\0').collect();
    let mut paths: Vec<String> = left
        .split('\0')
        .filter(|p| !p.is_empty() && right.contains(p))
        .map(str::to_owned)
        .collect();
    for path in left
        .split('\0')
        .chain(right.iter().copied())
        .filter(|p| p.starts_with("chats/"))
    {
        if !paths.iter().any(|saved| saved == path) {
            paths.push(path.to_owned());
        }
    }
    let mut unresolved = Vec::new();
    for path in paths {
        let left = git_raw(
            tool,
            root,
            &["--literal-pathspecs", "ls-tree", "-z", &local, "--", &path],
        )?;
        let right = git_raw(
            tool,
            root,
            &["--literal-pathspecs", "ls-tree", "-z", &remote, "--", &path],
        )?;
        if left != right {
            unresolved.push(path);
        }
    }
    unresolved.sort();
    Ok(unresolved)
}

// Construct the merge commit in a temporary index before changing the working tree.
fn merge_disjoint(tool: &Path, root: &Path) -> Result<String, &'static str> {
    let _app_guard = crate::chat_file_changes::lock_app_write();
    let _write_guard = crate::archive_zet::WRITE_LOCK.lock().map_err(|_| "busy")?;
    let _chat_guard = crate::chat_archive_store::lock_store().map_err(|_| "busy")?;
    let clean = || -> Result<bool, &'static str> {
        let status = git(
            tool,
            root,
            &["status", "--porcelain", "--untracked-files=all"],
        )
        .map_err(|_| "git_unavailable")?;
        Ok(status
            .lines()
            .all(|line| safe_untracked_default(root, line)))
    };
    if !clean()? {
        return Err("unsaved_changes");
    }
    let local =
        git(tool, root, &["rev-parse", "--verify", "HEAD"]).map_err(|_| "git_unavailable")?;
    let remote = git(
        tool,
        root,
        &["rev-parse", "--verify", "refs/remotes/momso/observed"],
    )
    .map_err(|_| "remote_unavailable")?;
    safe_history(tool, root, &local)?;
    safe_history(tool, root, &remote)?;
    let base =
        git(tool, root, &["merge-base", "--all", &local, &remote]).map_err(|_| "diverged")?;
    if base.lines().count() != 1 {
        return Err("diverged");
    }
    let paths = |revision: &str| -> Result<Vec<String>, &'static str> {
        let data = git_raw(
            tool,
            root,
            &["diff", "--name-only", "-z", "--no-renames", &base, revision],
        )
        .map_err(|_| "git_unavailable")?;
        Ok(data
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect())
    };
    let left = paths(&local)?;
    let right = paths(&remote)?;
    if left
        .iter()
        .chain(&right)
        .any(|path| path.starts_with("chats/") && !(left.contains(path) && right.contains(path)))
    {
        return Err("diverged");
    }
    let overlap: Vec<_> = left.iter().filter(|path| right.contains(path)).collect();
    let tree = if overlap.is_empty() {
        git(tool, root, &["merge-tree", "--write-tree", &local, &remote]).map_err(|_| "diverged")?
    } else {
        let temp = tempfile::tempdir().map_err(|_| "local_state_unavailable")?;
        let index = temp.path().join("index");
        git_index(
            tool,
            root,
            &index,
            &["read-tree", "-m", &base, &local, &remote],
        )
        .map_err(|_| "diverged")?;
        for path in &overlap {
            let blob = |revision: &str| -> Result<(String, String), &'static str> {
                let entry =
                    git(tool, root, &["ls-tree", revision, "--", path]).map_err(|_| "diverged")?;
                let (metadata, actual_path) = entry.split_once('\t').ok_or("diverged")?;
                let mut fields = metadata.split_whitespace();
                if actual_path != *path
                    || fields.next() != Some("100644")
                    || fields.next() != Some("blob")
                {
                    return Err("diverged");
                }
                let oid = fields.next().ok_or("diverged")?.to_owned();
                if fields.next().is_some() {
                    return Err("diverged");
                }
                let object = format!("{revision}:{path}");
                let text = git_raw(tool, root, &["show", &object]).map_err(|_| "diverged")?;
                Ok((oid, text))
            };
            let (local_oid, local_text) = blob(&local)?;
            let (remote_oid, remote_text) = blob(&remote)?;
            let object_id = if local_oid == remote_oid {
                local_oid
            } else if let Some(id) = path
                .strip_prefix("zettels/")
                .and_then(|p| p.strip_suffix(".md"))
                .filter(|id| crate::archive_zet::valid_zet_id(id))
            {
                for text in [&local_text, &remote_text] {
                    let (head, _) = crate::archive_zet::parse(text).map_err(|_| "diverged")?;
                    if head.id != id {
                        return Err("diverged");
                    }
                }
                remote_oid
            } else if let Some(id) = path
                .strip_prefix("chats/")
                .and_then(|p| p.strip_suffix(".json"))
            {
                let a: serde_json::Value =
                    serde_json::from_str(&local_text).map_err(|_| "diverged")?;
                let b: serde_json::Value =
                    serde_json::from_str(&remote_text).map_err(|_| "diverged")?;
                if a.pointer("/room/id").and_then(|v| v.as_str()) != Some(id)
                    || b.pointer("/room/id").and_then(|v| v.as_str()) != Some(id)
                {
                    return Err("diverged");
                }
                let merged = crate::chat_archive_snapshot::merge_snapshots(&a, &b)
                    .map_err(|_| "diverged")?;
                let bytes = serde_json::to_vec_pretty(&merged).map_err(|_| "diverged")?;
                git_hash_object(tool, root, &bytes).map_err(|_| "git_unavailable")?
            } else if path.as_str() == "dictionary/yoga.json" {
                let left =
                    crate::yoga_dictionary::known_bundle_version(&local_text).ok_or("diverged")?;
                let right =
                    crate::yoga_dictionary::known_bundle_version(&remote_text).ok_or("diverged")?;
                if left > right {
                    local_oid
                } else {
                    remote_oid
                }
            } else if ["AGENTS.md", "CLAUDE.md", "GEMINI.md"].contains(&path.as_str()) {
                if crate::archive::select_managed_rule(&local_text, &remote_text)
                    .ok_or("diverged")?
                {
                    remote_oid
                } else {
                    local_oid
                }
            } else if path.as_str() == "dictionary/my-terms.json" {
                let (_, base_text) = blob(&base)?;
                let combined = merge_added_terms(&base_text, &local_text, &remote_text)
                    .map_err(|_| "diverged")?;
                git_hash_object(tool, root, &combined).map_err(|_| "git_unavailable")?
            } else if path.as_str() == "dictionary/people.json" {
                let (_, base_text) = blob(&base)?;
                let (combined, _) =
                    merge_people(&base_text, &local_text, &remote_text).map_err(|_| "diverged")?;
                git_hash_object(tool, root, &combined).map_err(|_| "git_unavailable")?
            } else {
                return Err("diverged");
            };
            let entry = format!("100644,{object_id},{path}");
            git_index(
                tool,
                root,
                &index,
                &["update-index", "--add", "--cacheinfo", &entry],
            )
            .map_err(|_| "diverged")?;
        }
        if !git_index(tool, root, &index, &["ls-files", "-u"])
            .map_err(|_| "diverged")?
            .is_empty()
        {
            return Err("diverged");
        }
        git_index(tool, root, &index, &["write-tree"])
            .map_err(|_| "diverged")?
            .trim()
            .to_owned()
    };
    if !(tree.len() == 40 || tree.len() == 64) || !tree.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("diverged");
    }
    let commit = git(
        tool,
        root,
        &[
            "-c",
            "user.name=momso",
            "-c",
            "user.email=archive@momso.local",
            "commit-tree",
            &tree,
            "-p",
            &local,
            "-p",
            &remote,
            "-m",
            "momso archive sync",
        ],
    )
    .map_err(|_| "git_unavailable")?;
    safe_history(tool, root, &commit)?;
    if git(tool, root, &["rev-parse", "--verify", "HEAD"]).as_deref() != Ok(local.as_str())
        || !clean()?
    {
        return Err("unsaved_changes");
    }
    git(
        tool,
        root,
        &["merge", "--ff-only", "--no-overwrite-ignore", &commit],
    )
    .map_err(|_| "fast_forward_failed")?;
    crate::chat_file_changes::app_write(root, None);
    Ok(commit)
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
    let remote = remote_git(tool, root, &["ls-remote", "--heads", target, &remote_ref])?;
    if remote.is_empty() {
        let count = git(tool, root, &["rev-list", "--count", "HEAD"])
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        return Ok((count, 0));
    }
    let fetch_ref = format!("+{remote_ref}:refs/remotes/momso/observed");
    remote_git(tool, root, &["fetch", "--no-tags", target, &fetch_ref])?;
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

fn safe_untracked_default(root: &Path, line: &str) -> bool {
    if line == "?? .gitignore" {
        return default_file_unchanged(&root.join(".gitignore"), ".gitignore");
    }
    let Some(path) = line.strip_prefix("?? ") else {
        return false;
    };
    if !safe_skill_path(path) {
        return false;
    }
    let folder = if path.starts_with("skills/") {
        "skills"
    } else if path.starts_with(".agents/") {
        ".agents"
    } else {
        ".claude"
    };
    default_skills_unchanged(root, folder)
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
    fresh_archive_with_local_files(tool, root, false)
}

fn fresh_archive_with_local_files(tool: &Path, root: &Path, allow_local_files: bool) -> bool {
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
                || (!allow_local_files
                    && !fs::read_dir(entry.path())
                        .is_ok_and(|mut entries| entries.next().is_none()))
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
        } else if !allow_local_files {
            return false;
        }
    }
    true
}

fn ensure_recovery_directory(root: &Path, relative: &Path) -> Result<(), &'static str> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err("first_receive_interrupted");
        };
        current.push(name);
        match fs::create_dir(&current) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err("first_receive_interrupted"),
        }
        let meta = fs::symlink_metadata(&current).map_err(|_| "first_receive_interrupted")?;
        if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
            return Err("first_receive_interrupted");
        }
    }
    Ok(())
}

// While the first receive journal exists, missing files in the received tree
// are incomplete checkout work. Never replace an existing path: its current
// contents belong to the user, while the received version stays in Git.
fn restore_missing_received_files(
    tool: &Path,
    root: &Path,
    revision: &str,
    deleted_chats: &[String],
) -> Result<(), &'static str> {
    use std::io::Write;
    safe_history(tool, root, revision)?;
    let tree = git_output(
        tool,
        root,
        &["ls-tree", "-r", "-z", "--full-tree", revision],
    )
    .map_err(|_| "first_receive_interrupted")?;
    if !tree.status.success() {
        return Err("first_receive_interrupted");
    }
    let tree = std::str::from_utf8(&tree.stdout).map_err(|_| "first_receive_interrupted")?;
    for entry in tree.split('\0').filter(|entry| !entry.is_empty()) {
        let (metadata, name) = entry.split_once('\t').ok_or("first_receive_interrupted")?;
        if let Some(id) = name
            .strip_prefix("chats/")
            .and_then(|name| name.strip_suffix(".json"))
        {
            if deleted_chats.iter().any(|deleted| deleted == id) {
                continue;
            }
        }
        let mut parts = metadata.split_whitespace();
        let mode = parts.next().ok_or("first_receive_interrupted")?;
        if !matches!(mode, "100644" | "100755") || parts.next() != Some("blob") {
            return Err("first_receive_interrupted");
        }
        let relative = Path::new(name);
        if relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err("first_receive_interrupted");
        }
        let parent = relative.parent().ok_or("first_receive_interrupted")?;
        ensure_recovery_directory(root, parent)?;
        let destination = root.join(relative);
        match fs::symlink_metadata(&destination) {
            Ok(metadata)
                if metadata.is_file()
                    && !crate::archive_location::is_link_or_reparse(&metadata) =>
            {
                continue
            }
            Ok(_) => return Err("first_receive_interrupted"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err("first_receive_interrupted"),
        }
        let spec = format!("{revision}:{name}");
        let output =
            git_output(tool, root, &["show", &spec]).map_err(|_| "first_receive_interrupted")?;
        if !output.status.success() {
            return Err("first_receive_interrupted");
        }
        let mut staging = tempfile::Builder::new()
            .prefix("momso-received-")
            .tempfile_in(root.join(".git"))
            .map_err(|_| "local_state_unavailable")?;
        staging
            .write_all(&output.stdout)
            .and_then(|_| staging.as_file().sync_all())
            .map_err(|_| "local_state_unavailable")?;
        match staging.persist_noclobber(&destination) {
            Ok(_) => (),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                let meta =
                    fs::symlink_metadata(&destination).map_err(|_| "first_receive_interrupted")?;
                if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
                    return Err("first_receive_interrupted");
                }
            }
            Err(_) => return Err("first_receive_interrupted"),
        }
        #[cfg(test)]
        if name == "dictionary/yoga.json"
            && std::env::var_os("MOMSO_C31_CRASH_DICTIONARY").is_some()
        {
            std::process::exit(73);
        }
    }
    Ok(())
}

fn restore_journaled_defaults(
    tool: &Path,
    root: &Path,
    db: &Path,
    saved: &Path,
) -> Result<(), &'static str> {
    let mut journal = read_receive_journal(db)?.ok_or("first_receive_interrupted")?;
    if journal.root != root || journal.phase != "checkout" {
        return Err("first_receive_interrupted");
    }
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
        let base = saved.join(name);
        if !base.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&base)
            .follow_links(false)
            .sort_by_file_name()
        {
            let entry = entry.map_err(|_| "first_receive_interrupted")?;
            let meta =
                fs::symlink_metadata(entry.path()).map_err(|_| "first_receive_interrupted")?;
            if crate::archive_location::is_link_or_reparse(&meta) {
                return Err("first_receive_interrupted");
            }
            if meta.is_dir() {
                continue;
            }
            if !meta.is_file() {
                return Err("first_receive_interrupted");
            }
            let relative = entry
                .path()
                .strip_prefix(saved)
                .map_err(|_| "first_receive_interrupted")?;
            let path = relative
                .to_str()
                .ok_or("first_receive_interrupted")?
                .replace('\\', "/");
            if ![".gitignore", "AGENTS.md", "CLAUDE.md", "GEMINI.md"].contains(&path.as_str())
                && !safe_skill_path(&path)
                && !crate::yoga_dictionary::is_path(&path)
            {
                return Err("first_receive_interrupted");
            }
            let destination = root.join(relative);
            let existing = match fs::symlink_metadata(&destination) {
                Ok(meta) => {
                    if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
                        return Err("first_receive_interrupted");
                    }
                    true
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(_) => return Err("first_receive_interrupted"),
            };
            if journal.restore_done.contains(&path) && existing {
                continue;
            }
            let tracked = !git(tool, root, &["ls-files", "--", &path])
                .map_err(|_| "first_receive_interrupted")?
                .trim()
                .is_empty();
            // Tracked files come from the received Git tree, not the old default.
            if !existing && tracked {
                return Err("first_receive_interrupted");
            }
            if let Some(rest) = path
                .strip_prefix(".agents/skills/")
                .or_else(|| path.strip_prefix(".claude/skills/"))
            {
                let canonical = format!("skills/{rest}");
                if !git(tool, root, &["ls-files", "--", &canonical])
                    .map_err(|_| "first_receive_interrupted")?
                    .trim()
                    .is_empty()
                {
                    continue;
                }
            }
            if !existing {
                if !journal.restore_intents.contains(&path) {
                    journal.restore_intents.push(path.clone());
                }
                write_receive_journal(db, &journal)?;
                ensure_recovery_directory(
                    root,
                    relative.parent().ok_or("first_receive_interrupted")?,
                )?;
                match fs::hard_link(entry.path(), &destination) {
                    Ok(()) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let meta = fs::symlink_metadata(&destination)
                            .map_err(|_| "first_receive_interrupted")?;
                        if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
                            return Err("first_receive_interrupted");
                        }
                    }
                    Err(_) => return Err("first_receive_interrupted"),
                }
            }
            if !journal.restore_done.contains(&path) {
                journal.restore_done.push(path);
            }
            write_receive_journal(db, &journal)?;
        }
    }
    Ok(())
}

fn recover_completed_checkout(
    tool: &Path,
    root: &Path,
    db: &Path,
    journal: &ReceiveJournal,
) -> Result<(), &'static str> {
    let _app_write = crate::chat_file_changes::lock_app_write();
    let _write = crate::archive_zet::WRITE_LOCK.lock().map_err(|_| "busy")?;
    let expected = journal
        .revision
        .as_deref()
        .ok_or("first_receive_interrupted")?;
    let head = git(tool, root, &["rev-parse", "--verify", "HEAD"])
        .map_err(|_| "first_receive_interrupted")?;
    let observed = git(
        tool,
        root,
        &["rev-parse", "--verify", "refs/remotes/momso/observed"],
    )
    .map_err(|_| "first_receive_interrupted")?;
    if journal.phase != "checkout" || expected != head || observed != head {
        return Err("first_receive_interrupted");
    }
    safe_history(tool, root, &head)?;
    let marker = root.join(".git/momso-first-receive-in-progress");
    let meta = fs::symlink_metadata(&marker).map_err(|_| "first_receive_interrupted")?;
    if !meta.is_file() || meta.len() > 128 || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("first_receive_interrupted");
    }
    let name = fs::read_to_string(&marker).map_err(|_| "first_receive_interrupted")?;
    let id = name
        .strip_prefix("momso-first-pull-")
        .ok_or("first_receive_interrupted")?;
    uuid::Uuid::parse_str(id).map_err(|_| "first_receive_interrupted")?;
    let saved = root.join(".git").join(name);
    let meta = fs::symlink_metadata(&saved).map_err(|_| "first_receive_interrupted")?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("first_receive_interrupted");
    }
    if journal.has_defaults == Some(true) {
        // Do not adopt a locally edited tracked skill as the received baseline.
        for (name, _) in crate::skills::BUNDLED_SKILLS {
            let path = format!("skills/{name}/SKILL.md");
            if !git(tool, root, &["ls-files", "--", &path])
                .map_err(|_| "first_receive_interrupted")?
                .trim()
                .is_empty()
                && git(tool, root, &["diff", "--quiet", "HEAD", "--", &path]).is_err()
            {
                return Err("first_receive_interrupted");
            }
        }
        crate::skills_write::adopt_received_builtin_states(tool, root, &saved)
            .map_err(|_| "first_receive_interrupted")?;
    } else if journal.has_defaults != Some(false) {
        return Err("first_receive_interrupted");
    }
    restore_journaled_defaults(tool, root, db, &saved)?;
    fs::remove_file(marker).map_err(|_| "first_receive_interrupted")?;
    Ok(())
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
    first_receive_with_hooks(tool, root, revision, after_check, || {})
}

fn first_receive_with_hooks(
    tool: &Path,
    root: &Path,
    revision: &str,
    after_check: impl FnOnce(),
    after_move: impl FnMut(),
) -> Result<(), &'static str> {
    first_receive_impl(tool, root, revision, after_check, after_move, None)
}

fn receive_checkout_paths_clear(
    tool: &Path,
    root: &Path,
    revision: &str,
) -> Result<(), &'static str> {
    let output = git_output(
        tool,
        root,
        &["ls-tree", "-r", "-z", "--name-only", revision],
    )
    .map_err(|_| "fast_forward_failed")?;
    if !output.status.success() {
        return Err("fast_forward_failed");
    }
    let names = std::str::from_utf8(&output.stdout).map_err(|_| "fast_forward_failed")?;
    for name in names.split('\0').filter(|name| !name.is_empty()) {
        let mut current = root.to_path_buf();
        let components = Path::new(name).components().collect::<Vec<_>>();
        for (index, component) in components.iter().enumerate() {
            let std::path::Component::Normal(part) = component else {
                return Err("fast_forward_failed");
            };
            current.push(part);
            match fs::symlink_metadata(&current) {
                Ok(meta) => {
                    if index + 1 == components.len()
                        || !meta.is_dir()
                        || crate::archive_location::is_link_or_reparse(&meta)
                    {
                        return Err("fast_forward_failed");
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(_) => return Err("fast_forward_failed"),
            }
        }
    }
    Ok(())
}

fn first_receive_impl(
    tool: &Path,
    root: &Path,
    revision: &str,
    after_check: impl FnOnce(),
    mut after_move: impl FnMut(),
    db: Option<&Path>,
) -> Result<(), &'static str> {
    use std::io::Write;
    let _write = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "unavailable")?;
    let _chat_guard = crate::chat_archive_store::lock_store().map_err(|_| "busy")?;
    let resumed_checkout = if let Some(db) = db {
        matches!(read_receive_journal(db)?, Some(journal)
            if journal.root == root
                && journal.phase == "checkout"
                && journal.revision.as_deref() == Some(revision)
                && journal.has_defaults.is_some()
                && !receive_marker_present(root))
    } else {
        false
    };
    if !fresh_archive_with_local_files(tool, root, resumed_checkout) {
        return Err("unsaved_changes");
    }
    let has_defaults =
        crate::skills_write::fresh_builtin_state_mode(root).map_err(|_| "unsaved_changes")?;
    if let Some(db) = db {
        let mut journal = read_receive_journal(db)?.ok_or("first_receive_interrupted")?;
        if journal.root != root {
            return Err("first_receive_interrupted");
        }
        journal.phase = "checkout".into();
        journal.revision = Some(revision.to_owned());
        journal.has_defaults = Some(has_defaults);
        write_receive_journal(db, &journal)?;
    }
    after_check();
    let saved = root
        .join(".git")
        .join(format!("momso-first-pull-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&saved).map_err(|_| "local_state_unavailable")?;
    let marker = root.join(".git/momso-first-receive-in-progress");
    let mut marker_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                "first_receive_interrupted"
            } else {
                "local_state_unavailable"
            }
        })?;
    marker_file
        .write_all(
            saved
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or("local_state_unavailable")?
                .as_bytes(),
        )
        .and_then(|_| marker_file.sync_all())
        .map_err(|_| "local_state_unavailable")?;
    drop(marker_file);
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
                after_move();
                #[cfg(test)]
                if name == "AGENTS.md"
                    && std::env::var_os("MOMSO_C31_CRASH_BEFORE_CHECKOUT").is_some()
                {
                    std::process::exit(73);
                }
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
        receive_checkout_paths_clear(tool, root, revision)?;
        git(
            tool,
            root,
            &["merge", "--ff-only", "--no-overwrite-ignore", revision],
        )
        .map_err(|_| "fast_forward_failed")?;
        #[cfg(test)]
        if std::env::var_os("MOMSO_C28_CRASH_AFTER_CHECKOUT").is_some() {
            std::process::exit(73);
        }
        if has_defaults {
            crate::skills_write::adopt_received_builtin_states(tool, root, &saved)
                .map_err(|_| "local_state_unavailable")?;
        }
        Ok(())
    })();
    if result.is_err()
        && db.is_some()
        && git(tool, root, &["rev-parse", "--verify", "HEAD"]).as_deref() == Ok(revision)
    {
        return result;
    }
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
    } else if let Some(db) = db {
        restore_journaled_defaults(tool, root, db, &saved)?;
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
    if result.is_ok() && db.is_none() {
        crate::yoga_dictionary::initialize(root).map_err(|_| "local_state_unavailable")?;
    }
    let removed = fs::remove_dir(&saved).is_ok();
    if result.is_ok() || removed {
        fs::remove_file(marker).map_err(|_| "local_state_unavailable")?;
    }
    result
}

pub(crate) fn transfer(
    tool: &Path,
    root: &Path,
    target: &str,
    branch: &str,
    action: &str,
) -> Result<(), &'static str> {
    transfer_report(tool, root, target, branch, action).map(|_| ())
}

fn recover_interrupted_first_receive(tool: &Path, root: &Path) -> Result<bool, &'static str> {
    let marker = root.join(".git/momso-first-receive-in-progress");
    let meta = match fs::symlink_metadata(&marker) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err("local_state_unavailable"),
    };
    if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) || meta.len() > 128 {
        return Err("first_receive_interrupted");
    }
    if git(tool, root, &["rev-parse", "--verify", "HEAD"]).is_ok() {
        return Err("first_receive_interrupted");
    }
    let name = fs::read_to_string(&marker).map_err(|_| "first_receive_interrupted")?;
    let id = name
        .strip_prefix("momso-first-pull-")
        .ok_or("first_receive_interrupted")?;
    uuid::Uuid::parse_str(id).map_err(|_| "first_receive_interrupted")?;
    let saved = root.join(".git").join(name);
    let meta = fs::symlink_metadata(&saved).map_err(|_| "first_receive_interrupted")?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("first_receive_interrupted");
    }
    let entries = fs::read_dir(&saved)
        .map_err(|_| "first_receive_interrupted")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "first_receive_interrupted")?;
    for entry in &entries {
        let name = entry.file_name();
        let name = name.to_str().ok_or("first_receive_interrupted")?;
        let meta = fs::symlink_metadata(entry.path()).map_err(|_| "first_receive_interrupted")?;
        if crate::archive_location::is_link_or_reparse(&meta) {
            return Err("first_receive_interrupted");
        }
        let safe = if [".gitignore", "AGENTS.md", "CLAUDE.md", "GEMINI.md"].contains(&name) {
            meta.is_file() && default_file_unchanged(&entry.path(), name)
        } else if ["skills", ".agents", ".claude"].contains(&name) {
            meta.is_dir() && default_skills_unchanged(&saved, name)
        } else if name == crate::yoga_dictionary::DIRECTORY {
            meta.is_dir() && crate::yoga_dictionary::defaults_unchanged(&saved)
        } else {
            false
        };
        if !safe {
            return Err("first_receive_interrupted");
        }
    }
    for entry in &entries {
        for source in walkdir::WalkDir::new(entry.path()).follow_links(false) {
            let source = source.map_err(|_| "first_receive_interrupted")?;
            let meta =
                fs::symlink_metadata(source.path()).map_err(|_| "first_receive_interrupted")?;
            if crate::archive_location::is_link_or_reparse(&meta) {
                return Err("first_receive_interrupted");
            }
            let relative = source
                .path()
                .strip_prefix(&saved)
                .map_err(|_| "first_receive_interrupted")?;
            let destination = root.join(relative);
            if meta.is_dir() {
                match fs::create_dir(&destination) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let target = fs::symlink_metadata(&destination)
                            .map_err(|_| "first_receive_interrupted")?;
                        if !target.is_dir() || crate::archive_location::is_link_or_reparse(&target)
                        {
                            return Err("first_receive_interrupted");
                        }
                    }
                    Err(_) => return Err("first_receive_interrupted"),
                }
            } else if meta.is_file() {
                match fs::hard_link(source.path(), &destination) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let target = fs::symlink_metadata(&destination)
                            .map_err(|_| "first_receive_interrupted")?;
                        if !target.is_file()
                            || crate::archive_location::is_link_or_reparse(&target)
                            || !matches!((fs::read(source.path()), fs::read(&destination)), (Ok(a), Ok(b)) if a == b)
                        {
                            return Err("first_receive_interrupted");
                        }
                    }
                    Err(_) => return Err("first_receive_interrupted"),
                }
            } else {
                return Err("first_receive_interrupted");
            }
        }
    }
    fs::remove_file(marker).map_err(|_| "first_receive_interrupted")?;
    Ok(true)
}

fn transfer_report(
    tool: &Path,
    root: &Path,
    target: &str,
    branch: &str,
    action: &str,
) -> Result<bool, &'static str> {
    let mut merge_revision = None;
    transfer_with_merge_revision(
        tool,
        root,
        target,
        branch,
        action,
        &mut merge_revision,
        None,
    )
}

fn transfer_with_merge_revision(
    tool: &Path,
    root: &Path,
    target: &str,
    branch: &str,
    action: &str,
    merge_revision: &mut Option<String>,
    journal_db: Option<&Path>,
) -> Result<bool, &'static str> {
    transfer_with_options(
        tool,
        root,
        target,
        branch,
        action,
        merge_revision,
        TransferOptions {
            journal_db,
            check: None,
        },
    )
}

struct RedactionCheck<'a> {
    known: &'a [String],
    watched: Option<&'a [String]>,
    uploaded_key_found: &'a mut Option<bool>,
}

struct TransferOptions<'a> {
    journal_db: Option<&'a Path>,
    check: Option<RedactionCheck<'a>>,
}

#[cfg(test)]
fn transfer_with_redaction_check(
    tool: &Path,
    root: &Path,
    target: &str,
    branch: &str,
    action: &str,
    merge_revision: &mut Option<String>,
    options: (Option<RedactionCheck<'_>>, Option<&Path>),
) -> Result<bool, &'static str> {
    let (check, journal_db) = options;
    transfer_with_options(
        tool,
        root,
        target,
        branch,
        action,
        merge_revision,
        TransferOptions { journal_db, check },
    )
}

fn transfer_with_options(
    tool: &Path,
    root: &Path,
    target: &str,
    branch: &str,
    action: &str,
    merge_revision: &mut Option<String>,
    options: TransferOptions<'_>,
) -> Result<bool, &'static str> {
    let _write = crate::archive_zet::WRITE_LOCK.lock().map_err(|_| "busy")?;
    recover_interrupted_first_receive(tool, root)?;
    drop(_write);
    let (ahead, behind) = relation(tool, root, target, branch)?;
    let mut merged = false;
    if action == "push" {
        if behind > 0 {
            if ahead == 0 {
                return Err("remote_ahead");
            }
            *merge_revision = Some(merge_disjoint(tool, root)?);
            merged = true;
        }
        let mut revision =
            git(tool, root, &["rev-parse", "--verify", "HEAD"]).map_err(|_| "nothing_saved")?;
        safe_history(tool, root, &revision)?;
        if let Some(check) = options.check {
            let remote_ref = format!("refs/heads/{branch}");
            let reported = remote_git(tool, root, &["ls-remote", "--heads", target, &remote_ref])?;
            let base = if reported.is_empty() {
                None
            } else {
                let mut fields = reported.split_whitespace();
                let oid = fields.next().ok_or("remote_unavailable")?;
                if fields.next() != Some(remote_ref.as_str())
                    || fields.next().is_some()
                    || git(
                        tool,
                        root,
                        &["rev-parse", "--verify", "refs/remotes/momso/observed"],
                    )
                    .as_deref()
                        != Ok(oid)
                {
                    return Err("remote_unavailable");
                }
                Some(oid.to_owned())
            };
            let refs: Vec<&str> = check.known.iter().map(String::as_str).collect();
            let watched: Vec<&str> = check
                .watched
                .unwrap_or(check.known)
                .iter()
                .map(String::as_str)
                .collect();
            let any_uploaded = match base.as_deref() {
                Some(base) => uploaded_chat_contains_key(tool, root, base, &watched)?,
                None => false,
            };
            *check.uploaded_key_found = if check.watched.is_some() {
                Some(any_uploaded)
            } else {
                specific_uploaded_key_found(any_uploaded, &watched)
            };
            if unpushed_chat_needs_redaction(tool, root, &revision, base.as_deref(), &refs)? {
                let candidate =
                    redacted_history_candidate(tool, root, &revision, base.as_deref(), &refs)
                        .map_err(|e| {
                            if e == "sensitive_metadata" {
                                "redaction_required"
                            } else {
                                e
                            }
                        })?;
                safe_history(tool, root, &candidate)?;
                let old_tree = git(tool, root, &["rev-parse", &format!("{revision}^{{tree}}")])
                    .map_err(|_| "git_unavailable")?;
                let new_tree = git(tool, root, &["rev-parse", &format!("{candidate}^{{tree}}")])
                    .map_err(|_| "git_unavailable")?;
                if old_tree != new_tree {
                    return Err("redaction_required");
                }
                let _app_guard = crate::chat_file_changes::lock_app_write();
                let _write_guard = crate::archive_zet::WRITE_LOCK.lock().map_err(|_| "busy")?;
                let _chat_guard = crate::chat_archive_store::lock_store().map_err(|_| "busy")?;
                let dirty = git(
                    tool,
                    root,
                    &["status", "--porcelain", "--untracked-files=all"],
                )
                .map_err(|_| "git_unavailable")?;
                if !dirty.lines().all(|line| safe_untracked_default(root, line))
                    || git(tool, root, &["rev-parse", "--verify", "HEAD"]).as_deref()
                        != Ok(revision.as_str())
                    || git(tool, root, &["symbolic-ref", "-q", "HEAD"]).is_err()
                {
                    return Err("redaction_required");
                }
                git(tool, root, &["update-ref", "HEAD", &candidate, &revision])
                    .map_err(|_| "redaction_required")?;
                revision = candidate;
            }
        }
        let dest = format!("{revision}:refs/heads/{branch}");
        remote_git(tool, root, &["push", "--no-follow-tags", target, &dest])?;
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
            if let Some(db) = options.journal_db {
                return first_receive_impl(tool, root, &revision, || {}, || {}, Some(db))
                    .map(|_| false);
            }
            return first_receive(tool, root, &revision).map(|_| false);
        }
        let dirty = git(
            tool,
            root,
            &["status", "--porcelain", "--untracked-files=all"],
        )
        .map_err(|_| "git_unavailable")?;
        let clean_except_default_ignore =
            dirty.lines().all(|line| safe_untracked_default(root, line));
        if !dirty.is_empty() && !clean_except_default_ignore {
            return Err("unsaved_changes");
        }
        if ahead > 0 && behind > 0 {
            *merge_revision = Some(merge_disjoint(tool, root)?);
            return Ok(true);
        }
        if behind == 0 {
            return Ok(false);
        }
        let revision = git(
            tool,
            root,
            &["rev-parse", "--verify", "refs/remotes/momso/observed"],
        )
        .map_err(|_| "remote_unavailable")?;
        safe_history(tool, root, &revision)?;
        let _guard = crate::chat_file_changes::lock_app_write();
        let _write_guard = crate::archive_zet::WRITE_LOCK.lock().map_err(|_| "busy")?;
        let _chat_guard = crate::chat_archive_store::lock_store().map_err(|_| "busy")?;
        if !crate::chat_file_changes::has_active(root) {
            return git(tool, root, &["merge", "--ff-only", &revision])
                .map(|_| false)
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
    Ok(merged)
}
fn apply_relation(result: &mut RemoteStatus, tool: &Path, root: &Path, ahead: u64, behind: u64) {
    result.state = "connected";
    result.ahead = Some(ahead);
    result.behind = Some(behind);
    if ahead > 0 && behind > 0 {
        result.reason = Some("diverged");
        result.conflict_paths = conflicting_paths(tool, root).ok();
    }
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
            apply_relation(&mut result, &tool, root, ahead, behind);
        }
        Err(e) => result.reason = Some(e),
    }
    result
}
async fn command_paths<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
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
            if let Err(e) = receive_connected_with_progress(
                &tool,
                &root,
                &db,
                &url(&remote.name_with_owner),
                branch,
                Some(&app),
            ) {
                return RemoteStatus::transfer_error(e);
            }
        }
        status(&root, &resources)
    })
    .await
    .unwrap_or_else(|_| RemoteStatus::new("unavailable", Some("local_state_unavailable")))
}
#[cfg(test)]
#[derive(Clone)]
struct TestRemote {
    tool: PathBuf,
    target: String,
}

async fn run_transfer<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, crate::AppState>,
    action: &'static str,
) -> RemoteStatus {
    let Ok((root, resources, db)) = command_paths(&app, &state).await else {
        return RemoteStatus::new("unavailable", Some("local_state_unavailable"));
    };
    let paths = state.paths.clone();
    #[cfg(test)]
    let test_remote = app
        .try_state::<TestRemote>()
        .map(|remote| remote.inner().clone());
    let progress_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Ok(_guard) = REMOTE_LOCK.lock() else {
            return RemoteStatus::new("unavailable", Some("busy"));
        };
        #[cfg(test)]
        let tool = test_remote
            .as_ref()
            .map(|remote| remote.tool.clone())
            .or_else(|| git_path(&resources));
        #[cfg(not(test))]
        let tool = git_path(&resources);
        let Some(tool) = tool else {
            return RemoteStatus::new("unavailable", Some("git_missing"));
        };
        #[cfg(test)]
        let repo = test_remote
            .as_ref()
            .map(|_| "fixture/repository".to_owned())
            .or_else(|| configured(&tool, &root));
        #[cfg(not(test))]
        let repo = configured(&tool, &root);
        let Some(repo) = repo else {
            return RemoteStatus::new("not_connected", None);
        };
        #[cfg(test)]
        let remote_result = if test_remote.is_some() {
            Ok((
                Repository {
                    name_with_owner: repo.clone(),
                    is_private: true,
                    default_branch_ref: None,
                },
                "fixture".to_owned(),
            ))
        } else {
            github(&repo)
        };
        #[cfg(not(test))]
        let remote_result = github(&repo);
        let (remote, user) = match remote_result {
            Ok(x) => x,
            Err(e) => return RemoteStatus::new("unavailable", Some(e)),
        };
        let branch = remote
            .default_branch_ref
            .map(|x| x.name)
            .unwrap_or_else(|| "main".into());
        let journal = match read_receive_journal(&db) {
            Ok(journal) => journal,
            Err(error) => return RemoteStatus::new("unavailable", Some(error)),
        };
        let retry = journal.is_some() || receive_marker_present(&root);
        if action == "push" && retry {
            return RemoteStatus::new("unavailable", Some("first_receive_in_progress"));
        }
        let first_receive = action == "pull"
            && (retry || git(&tool, &root, &["rev-parse", "--verify", "HEAD"]).is_err());
        let progress = |state, completed, available| {
            if first_receive {
                let value = receive_progress(state, completed, available);
                remember_receive_progress(&root, &db, &value);
                let _ = progress_app.emit("archive-first-receive-progress", value);
            }
        };
        if action == "push" {
            if let Err(e) = crate::chat_archive_store::save_pending(&root, &resources, &paths) {
                if e == "redaction_pending" { return RemoteStatus::new("redactionPending", None); }
                return RemoteStatus::new("unavailable", Some(e));
            }
        }
        #[cfg(test)]
        let target = test_remote
            .as_ref()
            .map(|remote| remote.target.clone())
            .unwrap_or_else(|| url(&remote.name_with_owner));
        #[cfg(not(test))]
        let target = url(&remote.name_with_owner);
        if first_receive {
            let outcome = receive_first_with_progress(
                &tool,
                &root,
                &db,
                &target,
                &branch,
                |value| {
                    let _ = progress_app.emit("archive-first-receive-progress", value);
                },
                || crate::chat_archive_store::restore_emitting(&root, &db, &app),
            );
            return match outcome {
                Ok(()) => {
                    #[cfg(test)]
                    let mut result = if test_remote.is_some() {
                        RemoteStatus::new("connected", None)
                    } else {
                        status(&root, &resources)
                    };
                    #[cfg(not(test))]
                    let mut result = status(&root, &resources);
                    result.repo = Some(remote.name_with_owner.clone());
                    result.visibility = Some("private".into());
                    result.signed_in_as = Some(user);
                    result
                }
                Err(error) => {
                    let value = receive_failure_progress(&root, error);
                    remember_receive_progress(&root, &db, &value);
                    let _ = progress_app.emit("archive-first-receive-progress", value);
                    let mut result = RemoteStatus::transfer_error(error);
                    result.repo = Some(remote.name_with_owner.clone());
                    result.visibility = Some("private".into());
                    result.signed_in_as = Some(user);
                    result
                }
            };
        }
        let before = git(&tool, &root, &["rev-parse", "--verify", "HEAD"]).ok();
        let mut merge_revision = None;
        let proof_key = if action == "push" { match crate::chat_archive_tracking::proof_key() { Ok(key) => Some(key), Err(_) => return RemoteStatus::new("unavailable", Some("credentials_unavailable")) } } else { None };
        let _credential_guard = if action == "push" {
            match crate::credentials::prevent_secret_changes() {
                Ok(guard) => Some(guard),
                Err(_) => return RemoteStatus::new("unavailable", Some("credentials_unavailable")),
            }
        } else { None };
        let known = if action == "push" {
            match crate::chat_archive_store::known_values(&paths.data_root, &paths.database_path) {
                Ok(values) => values,
                Err(_) => return RemoteStatus::new("unavailable", Some("credentials_unavailable")),
            }
        } else { Vec::new() };
        if action == "push" {
            let Some(key) = proof_key.as_deref() else { return RemoteStatus::new("unavailable", Some("credentials_unavailable")); };
            let current = crate::chat_archive_tracking::proof(&root, &known, key);
            let stored = crate::chat_archive_tracking::stored_proof(&paths.database_path);
            if !matches!((current, stored), (Ok(current), Ok(Some(stored))) if current == stored) {
                return RemoteStatus::new("redactionPending", None);
            }
        }
        let watched = if action == "push" { match crate::redaction_wait::watched_values_unlocked() { Ok(values) => values, Err(_) => return RemoteStatus::new("unavailable", Some("credentials_unavailable")) } } else { Vec::new() };
        let mut uploaded_key_found = None;
        let check = if action == "push" {
            let _ = app.emit("chat-archive-redaction-progress", serde_json::json!({"state":"checkingHistory","completed":0,"total":0,"available":false}));
            Some(RedactionCheck { known: &known, watched: if watched.is_empty() { None } else { Some(&watched) }, uploaded_key_found: &mut uploaded_key_found })
        } else { None };
        let transfer_result = transfer_with_options(
            &tool,
            &root,
            &target,
            &branch,
            action,
            &mut merge_revision,
            TransferOptions { journal_db: Some(&paths.database_path), check },
        );
        if action == "push" && transfer_result.is_ok() && crate::redaction_wait::clear_after_upload_unlocked(&paths.database_path).is_err() {
            let mut result = RemoteStatus::new("redactionCleanupPending", None);
            result.uploaded_key_found = uploaded_key_found;
            return result;
        }
        if action == "push" {
            let _ = app.emit("chat-archive-redaction-progress", serde_json::json!({"state":if transfer_result.is_ok() {"complete"} else {"failed"},"completed":0,"total":0,"available":transfer_result.is_ok()}));
        }
        let after = git(&tool, &root, &["rev-parse", "--verify", "HEAD"]).ok();
        let changed = before != after;
        let merged = merge_revision.is_some();
        if transfer_result.is_ok() {
            progress("indexing", 1, false);
        }
        let preserved_result = merge_revision
            .as_deref()
            .map(|revision| preserved_documents(&tool, &root, revision))
            .transpose();
        let people_conflict = merge_revision
            .as_deref()
            .map(|revision| merged_people_conflict(&tool, &root, revision))
            .transpose();
        let preserved = preserved_result
            .as_ref()
            .ok()
            .and_then(|docs| docs.clone())
            .unwrap_or_default();
        if (merged || changed || (action == "pull" && transfer_result.is_ok()))
            && crate::archive::reindex(&root, &db).is_err()
        {
            progress("failed", 1, false);
            let mut status = RemoteStatus::new("unavailable", Some("reindex_failed"));
            status.auto_merged = merged;
            status.preserved_documents = preserved;
            return status;
        }
        if preserved_result.is_err() || people_conflict.is_err() {
            progress("failed", 1, false);
            let mut status = RemoteStatus::new("unavailable", Some("merge_report_unavailable"));
            status.auto_merged = merged;
            return status;
        }
        if (action == "pull" && transfer_result.is_ok()) || merged {
            progress("restoring", 2, false);
            if let Err(e) = crate::chat_archive_store::restore_emitting(&root, &db, &app) {
                progress("failed", 2, false);
                return RemoteStatus::new("unavailable", Some(e));
            }
        }
        if let Err(e) = transfer_result {
            progress(
                if e == "first_receive_interrupted" {
                    "interrupted"
                } else {
                    "failed"
                },
                0,
                false,
            );
            let mut s = RemoteStatus::transfer_error(e);
            s.uploaded_key_found = uploaded_key_found;
            s.auto_merged = merged;
            s.preserved_documents = preserved;
            if e == "diverged" {
                s.conflict_paths = conflicting_paths(&tool, &root).ok();
            } else if people_conflict == Ok(Some(true)) {
                s.conflict_paths = Some(vec!["dictionary/people.json".to_owned()]);
            }
            s.repo = Some(remote.name_with_owner.clone());
            s.visibility = Some("private".into());
            s.signed_in_as = Some(user);
            s.last_pushed_at = last_push(&root, &target);
            return s;
        }
        #[cfg(test)]
        let mut result = if test_remote.is_some() {
            RemoteStatus::new("connected", None)
        } else {
            status(&root, &resources)
        };
        #[cfg(not(test))]
        let mut result = status(&root, &resources);
        if after.is_some() {
            progress("complete", 3, true);
        } else {
            progress("empty", 0, false);
        }
        result.auto_merged = merged;
        result.uploaded_key_found = uploaded_key_found;
        result.preserved_documents = preserved;
        if people_conflict == Ok(Some(true)) {
            result.conflict_paths = Some(vec!["dictionary/people.json".to_owned()]);
        }
        result
    })
    .await
    .unwrap_or_else(|_| RemoteStatus::new("unavailable", Some("local_state_unavailable")))
}
#[tauri::command]
pub(crate) async fn archive_remote_push<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, crate::AppState>,
) -> Result<RemoteStatus, String> {
    Ok(run_transfer(app, state, "push").await)
}
#[tauri::command]
pub(crate) async fn archive_remote_pull<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
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
    #[test]
    fn saved_default_conflict_is_reported_without_changing_either_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let git_dir = root.join(".git");
        std::fs::create_dir(&git_dir).unwrap();
        let name = format!("momso-first-pull-{}", uuid::Uuid::new_v4());
        let saved = git_dir.join(&name);
        std::fs::create_dir(&saved).unwrap();
        std::fs::write(git_dir.join("momso-first-receive-in-progress"), name).unwrap();
        std::fs::write(saved.join("AGENTS.md"), "saved").unwrap();
        std::fs::write(root.join("AGENTS.md"), "user").unwrap();
        let value = super::receive_failure_progress(root, "first_receive_interrupted");
        assert_eq!(value.reason, Some("first_receive_conflict"));
        assert_eq!(value.conflict_paths, vec!["AGENTS.md"]);
        assert_eq!(
            std::fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "user"
        );
        assert_eq!(
            std::fs::read_to_string(saved.join("AGENTS.md")).unwrap(),
            "saved"
        );
        std::fs::write(root.join("AGENTS.md"), "saved").unwrap();
        assert!(super::saved_default_conflicts(root).is_empty());
    }
    #[test]
    fn first_receive_status_command_matches_progress_events() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        let db = base.join("momso.db");
        let backups = base.join("backups");
        crate::storage::initialize(&db, &backups, "test").unwrap();
        let archive =
            crate::archive_location::ArchiveLocation::load(db.clone(), base, vec![]).unwrap();
        let root = base.join("momso");
        let paths = crate::runtime::RuntimePaths {
            data_root: base.to_path_buf(),
            database_path: db.clone(),
            objects_path: root.join("objects"),
            exports_path: base.join("exports"),
            backups_path: backups,
            logs_path: base.join("logs"),
            archive,
            archive_lease: None,
        };
        let app = tauri::test::mock_builder()
            .manage(crate::AppState {
                paths,
                approvals: crate::approval::ApprovalBroker::default(),
                setup_secrets: crate::setup_secret::SecretBroker::default(),
                usage_monitor: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                usage_cache: std::sync::Arc::new(crate::usage_status::UsageCache::default()),
                chat_active: Default::default(),
                browser: Default::default(),
                selections: Default::default(),
            })
            .invoke_handler(tauri::generate_handler![
                archive_first_receive_status,
                crate::zet_create,
                crate::zet_update,
                crate::zet_read
            ])
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        fs::create_dir_all(root.join("zettels")).unwrap();
        let input = serde_json::json!({"input":{"title":"Yoga","summary":"Practice","kind":"note","body":"First","assets":[],"edges":[]}});
        let id = crate::command_entry_tests::invoke(&window, "zet_create", input.clone())
            .as_str()
            .unwrap()
            .to_owned();
        begin_receive_journal(&db, &root, "fixture", "main").unwrap();
        let invoke_error = |cmd: &str, body: serde_json::Value| {
            tauri::test::get_ipc_response(
                &window,
                tauri::webview::InvokeRequest {
                    cmd: cmd.into(),
                    callback: tauri::ipc::CallbackFn(0),
                    error: tauri::ipc::CallbackFn(1),
                    url: "http://tauri.localhost".parse().unwrap(),
                    body: tauri::ipc::InvokeBody::Json(body),
                    headers: Default::default(),
                    invoke_key: tauri::test::INVOKE_KEY.into(),
                },
            )
            .unwrap_err()
        };
        assert_eq!(
            invoke_error("zet_create", input.clone()),
            "first_receive_in_progress"
        );
        assert_eq!(
            invoke_error(
                "zet_update",
                serde_json::json!({"id":id,"changes":{"body":"blocked"}})
            ),
            "first_receive_in_progress"
        );
        assert_eq!(
            crate::command_entry_tests::invoke(&window, "zet_read", serde_json::json!({"id":id}))
                ["body"],
            "First"
        );
        let fake_tool = || -> Result<(), &'static str> {
            let _tracking = crate::chat_file_changes::begin_chat(&root, &db, "test-chat")?;
            fs::write(root.join("tool-wrote"), b"written").unwrap();
            Ok(())
        };
        assert_eq!(fake_tool(), Err("first_receive_in_progress"));
        assert!(!root.join("tool-wrote").exists());
        let expected = receive_progress("receiving", 0, false);
        remember_receive_progress(&root, &db, &expected);
        assert_eq!(
            crate::command_entry_tests::invoke(
                &window,
                "archive_first_receive_status",
                serde_json::json!({})
            ),
            serde_json::to_value(expected).unwrap()
        );
        let failed = receive_progress("failed", 1, false);
        remember_receive_progress(&root, &db, &failed);
        assert_eq!(
            crate::command_entry_tests::invoke(
                &window,
                "archive_first_receive_status",
                serde_json::json!({})
            ),
            serde_json::to_value(failed).unwrap()
        );
        finish_receive_journal(&db).unwrap();
        fake_tool().unwrap();
        let tracking = crate::chat_file_changes::begin_chat(&root, &db, "active-chat").unwrap();
        assert_eq!(
            receive_first_locked(
                Path::new("unused"),
                &root,
                &db,
                "fixture",
                "main",
                |_| {},
                || Ok(())
            ),
            Err("busy")
        );
        drop(tracking);
        crate::command_entry_tests::invoke(
            &window,
            "zet_update",
            serde_json::json!({"id":id,"changes":{"body":"allowed"}}),
        );
        assert!(crate::command_entry_tests::invoke(&window, "zet_create", input).is_string());
        let complete = receive_progress("complete", 3, true);
        remember_receive_progress(&root, &db, &complete);
        assert_eq!(
            crate::command_entry_tests::invoke(
                &window,
                "archive_first_receive_status",
                serde_json::json!({})
            ),
            serde_json::to_value(complete).unwrap()
        );
    }
    #[test]
    fn first_receive_lock_excludes_other_process_and_releases_on_exit() {
        use std::io::{BufRead, BufReader, Write};
        use std::process::{Command, Stdio};

        if let Some(path) = std::env::var_os("MOMSO_TEST_RECEIVE_LOCK_DB") {
            let _lock = super::lock_first_receive(std::path::Path::new(&path)).unwrap();
            println!("MOMSO_LOCK_READY");
            std::io::stdout().flush().unwrap();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).unwrap();
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("state.sqlite");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "archive_remote::tests::first_receive_lock_excludes_other_process_and_releases_on_exit", "--nocapture"])
            .env("MOMSO_TEST_RECEIVE_LOCK_DB", &db)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert!(
                output.read_line(&mut line).unwrap() > 0,
                "child exited before acquiring lock"
            );
            if line.contains("MOMSO_LOCK_READY") {
                break;
            }
        }
        assert!(matches!(super::lock_first_receive(&db), Err("busy")));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(super::lock_first_receive(&db).is_ok());
    }
    #[test]
    fn first_receive_progress_matches_chat_restore_fields() {
        let pending =
            serde_json::to_value(super::receive_progress("recovering", 0, false)).unwrap();
        assert_eq!(
            pending,
            serde_json::json!({"completed":0,"total":3,"state":"recovering","available":false,"reason":null,"conflictPaths":[]})
        );
        let completed = serde_json::to_value(super::receive_progress("complete", 3, true)).unwrap();
        assert_eq!(completed["available"], true);
    }
    #[test]
    fn remote_error_categories_do_not_treat_ambiguous_http_as_proof() {
        use super::classify_remote_error as classify;
        assert_eq!(
            classify("fatal: Authentication failed for 'https://example.invalid/'"),
            "authentication_failed"
        );
        assert_eq!(
            classify("remote: Permission to owner/repo denied to user."),
            "permission_denied"
        );
        assert_eq!(
            classify("fatal: unable to access: Could not resolve host: example.invalid"),
            "connection_failed"
        );
        assert_eq!(
            classify("remote: error: GH001: Large files detected"),
            "size_limit"
        );
        assert_eq!(
            classify("fatal: pack exceeds maximum allowed size"),
            "size_limit"
        );
        assert_eq!(
            classify("fatal: repository not found"),
            "repository_unavailable_or_denied"
        );
        assert_eq!(classify("HTTP 404"), "repository_unavailable_or_denied");
        assert_eq!(classify("HTTP 403"), "remote_unavailable");
        assert_eq!(
            classify("fatal: the remote end hung up unexpectedly"),
            "remote_unavailable"
        );
    }
    #[test]
    fn chat_history_paths_accept_only_canonical_chat_json() {
        assert!(super::safe_chat_path("chats/chat_20261115_120000.json"));
        assert!(super::safe_chat_path(
            "chats/chat_00000000-0000-0000-0000-000000000000.json"
        ));
        for path in [
            "chats/../credentials.json",
            "chats/chat_20261115_120000.json/extra",
            "chats/chat_20261115_120000.db",
            "chats/object.json",
            "objects/chat_20261115_120000.json",
        ] {
            assert!(!super::safe_chat_path(path));
        }
    }
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

    #[test]
    fn missing_proof_without_keys_does_not_block_real_push_command() {
        for room_count in [0, 2] {
            let temp = tempfile::tempdir().unwrap();
            let base = temp.path();
            let root = base.join("archive");
            let bare = base.join("remote.git");
            let db = base.join("momso.db");
            let backups = base.join("backups");
            let tool = crate::process::resolve_tool("git.exe").unwrap();
            git(&tool, base, &["init", "--bare", bare.to_str().unwrap()]).unwrap();
            if room_count > 0 {
                crate::storage::initialize_before_chat_redaction_for_test(&db);
                let conn = rusqlite::Connection::open(&db).unwrap();
                conn.execute("INSERT INTO chats(id,title,created_at,updated_at) VALUES('chat_aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa','fixture','2020-01-01','2020-01-01')", []).unwrap();
            }
            crate::storage::initialize(&db, &backups, "fixture").unwrap();
            let location =
                crate::archive_location::ArchiveLocation::load(db.clone(), base, vec![]).unwrap();
            *location.current.try_write().unwrap() = root.clone();
            crate::archive::initialize(&root, base).unwrap();
            for _ in usize::from(room_count > 0)..room_count {
                crate::chat::create(&db).unwrap();
            }
            assert!(crate::chat_archive_tracking::stored_proof(&db)
                .unwrap()
                .is_none());
            let paths = crate::runtime::RuntimePaths {
                data_root: base.to_owned(),
                database_path: db.clone(),
                objects_path: root.join("objects"),
                exports_path: base.join("exports"),
                backups_path: backups,
                logs_path: base.join("logs"),
                archive: location,
                archive_lease: None,
            };
            let app = tauri::test::mock_builder()
                .manage(crate::AppState {
                    paths,
                    approvals: Default::default(),
                    setup_secrets: Default::default(),
                    usage_monitor: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    usage_cache: Default::default(),
                    chat_active: Default::default(),
                    browser: Default::default(),
                    selections: Default::default(),
                })
                .manage(TestRemote {
                    tool: tool.clone(),
                    target: bare.to_string_lossy().into_owned(),
                })
                .invoke_handler(tauri::generate_handler![
                    archive_remote_push,
                    crate::chat_archive_status::chat_archive_sync_status
                ])
                .build(tauri::test::mock_context(tauri::test::noop_assets()))
                .unwrap();
            let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .unwrap();
            let status = crate::command_entry_tests::invoke(
                &window,
                "chat_archive_sync_status",
                serde_json::json!({}),
            );
            assert_eq!(status["state"], "saved");
            let push = crate::command_entry_tests::invoke(
                &window,
                "archive_remote_push",
                serde_json::json!({}),
            );
            assert_eq!(push["state"], "connected", "{push}");
            assert!(crate::chat_archive_tracking::stored_proof(&db)
                .unwrap()
                .is_some());
            let refs = git(
                &tool,
                base,
                &[
                    "--git-dir",
                    bare.to_str().unwrap(),
                    "show-ref",
                    "--verify",
                    "refs/heads/main",
                ],
            )
            .unwrap();
            assert!(!refs.is_empty());
            let status = crate::command_entry_tests::invoke(
                &window,
                "chat_archive_sync_status",
                serde_json::json!({}),
            );
            assert_eq!(status["state"], "saved");
        }
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
    fn unpushed_old_chat_key_is_rewritten_before_send_without_losing_document_history() {
        let f = Fixture::new();
        f.push();
        let remote_before = git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap();
        let key = "fixture-opaque-personal-value";
        let id = "chat_20261115_120000";
        fs::create_dir(f.root.join("chats")).unwrap();
        let path = f.root.join("chats").join(format!("{id}.json"));
        let snapshot = |text| {
            serde_json::json!({"version":1,"room":{"id":id,"title":"lesson"},"messages":[{"id":"m","text":text}],"groups":[],"imports":[],"importMessages":[],"attachments":[],"redactions":0}).to_string()
        };
        fs::write(&path, snapshot(key)).unwrap();
        git(&f.tool, &f.root, &["add", "--", "chats/"]).unwrap();
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
                "chat",
            ],
        )
        .unwrap();
        let old_chat_commit = git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap();
        f.local_commit();
        fs::write(&path, snapshot("[가림]")).unwrap();
        git(&f.tool, &f.root, &["add", "--", "chats/"]).unwrap();
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
                "mask",
            ],
        )
        .unwrap();
        let head = git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(
            unpushed_chat_needs_redaction(&f.tool, &f.root, &head, Some(&remote_before), &[key]),
            Ok(true)
        );
        let candidate =
            redacted_history_candidate(&f.tool, &f.root, &head, Some(&remote_before), &[key]);
        assert!(candidate.is_ok(), "candidate: {candidate:?}");
        let mut uploaded = None;
        let keys = vec![key.to_owned(), "other-fixture-key".to_owned()];
        let watched = vec![key.to_owned()];
        let result = transfer_with_redaction_check(
            &f.tool,
            &f.root,
            f.remote(),
            "main",
            "push",
            &mut None,
            (
                Some(RedactionCheck {
                    known: &keys,
                    watched: Some(&watched),
                    uploaded_key_found: &mut uploaded,
                }),
                None,
            ),
        );
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(uploaded, Some(false));
        let pushed = git(&f.tool, &f.bare, &["rev-parse", "refs/heads/main"]).unwrap();
        assert!(!unpushed_chat_needs_redaction(
            &f.tool,
            &f.bare,
            &pushed,
            Some(&remote_before),
            &[key]
        )
        .unwrap());
        assert!(git(&f.tool, &f.bare, &["cat-file", "-e", &old_chat_commit]).is_err());
        // Inspect the actual objects received by the fake remote, decompressed.
        let received = git_output(
            &f.tool,
            &f.bare,
            &["cat-file", "--batch-all-objects", "--batch"],
        )
        .unwrap();
        assert!(received.status.success());
        assert!(!received
            .stdout
            .windows(key.len())
            .any(|bytes| bytes == key.as_bytes()));
        assert_eq!(
            git(&f.tool, &f.root, &["show", "HEAD:zettels/local.md"]).unwrap(),
            "local"
        );
        assert!(git(
            &f.tool,
            &f.root,
            &["log", "--format=%s", "--", "zettels/local.md"]
        )
        .unwrap()
        .contains("local"));
    }

    #[test]
    fn uploaded_snapshot_key_is_reported_without_rewriting_remote_history() {
        let f = Fixture::new();
        let key = "fixture-opaque-previous-value";
        let id = "chat_20261115_120000";
        fs::create_dir(f.root.join("chats")).unwrap();
        let path = f.root.join("chats").join(format!("{id}.json"));
        let snapshot = |text| {
            serde_json::json!({"version":1,"room":{"id":id,"title":"lesson"},"messages":[{"id":"m","text":text}],"groups":[],"imports":[],"importMessages":[],"attachments":[],"redactions":0}).to_string()
        };
        fs::write(&path, snapshot(key)).unwrap();
        git(&f.tool, &f.root, &["add", "--", "chats/"]).unwrap();
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
                "previous",
            ],
        )
        .unwrap();
        f.push();
        let old = git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap();
        f.local_commit();
        fs::write(&path, snapshot("[가림]")).unwrap();
        git(&f.tool, &f.root, &["add", "--", "chats/"]).unwrap();
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
                "mask",
            ],
        )
        .unwrap();
        let mut uploaded = None;
        let keys = vec![key.to_owned(), "other-fixture-key".to_owned()];
        let watched = vec![key.to_owned()];
        let result = transfer_with_redaction_check(
            &f.tool,
            &f.root,
            f.remote(),
            "main",
            "push",
            &mut None,
            (
                Some(RedactionCheck {
                    known: &keys,
                    watched: Some(&watched),
                    uploaded_key_found: &mut uploaded,
                }),
                None,
            ),
        );
        assert!(result.is_ok());
        assert_eq!(uploaded, Some(true));
        assert_eq!(
            specific_uploaded_key_found(true, &[key, "other-fixture-key"]),
            None
        );
        assert_eq!(
            specific_uploaded_key_found(false, &[key, "other-fixture-key"]),
            Some(false)
        );
        assert!(git(
            &f.tool,
            &f.bare,
            &["merge-base", "--is-ancestor", &old, "refs/heads/main"]
        )
        .is_ok());
        let pushed = git(&f.tool, &f.bare, &["rev-parse", "refs/heads/main"]).unwrap();
        assert!(
            !unpushed_chat_needs_redaction(&f.tool, &f.bare, &pushed, Some(&old), &[key]).unwrap()
        );
    }

    #[test]
    fn ipc_divergence_preserves_versions_and_allows_local_work_and_retry() {
        let f = Fixture::new();
        crate::archive::initialize(&f.root, f._dir.path()).unwrap();
        fs::remove_file(f.root.join("zettels/a.md")).unwrap();
        let db = f._dir.path().join("momso.db");
        let backups = f._dir.path().join("backups");
        crate::storage::initialize(&db, &backups, "test").unwrap();
        let now = chrono::Local::now();
        let id = format!("zet_{}", now.format("%Y%m%d_%H%M%S"));
        let timestamp = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        let path = format!("zettels/{id}.md");
        let document = |body: &str| {
            format!("---\nid: {id}\ntitle: Lesson\nsummary: Summary\nkind: note\nstatus: draft\ncreated_at: '{timestamp}'\nupdated_at: '{timestamp}'\nassets: []\nedges: []\n---\n{body}\n")
        };
        fs::write(f.root.join(&path), document("base")).unwrap();
        let profile =
            |name: &str| format!("---\nname: {name}\nabout: ''\nstyle: ''\nupdatedAt: null\n---\n");
        fs::write(f.root.join("profile.md"), profile("base")).unwrap();
        git(&f.tool, &f.root, &["add", "--", "."]).unwrap();
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
                "base",
            ],
        )
        .unwrap();
        f.push();
        let other = f._dir.path().join("other");
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
        fs::write(other.join(&path), document("remote")).unwrap();
        git(&f.tool, &other, &["add", "--", &path]).unwrap();
        git(
            &f.tool,
            &other,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "remote edit",
            ],
        )
        .unwrap();
        git(&f.tool, &other, &["push", "origin", "main"]).unwrap();
        fs::write(f.root.join(&path), document("local")).unwrap();
        git(&f.tool, &f.root, &["add", "--", &path]).unwrap();
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
                "local edit",
            ],
        )
        .unwrap();
        let location =
            crate::archive_location::ArchiveLocation::load(db.clone(), f._dir.path(), vec![])
                .unwrap();
        *location.current.try_write().unwrap() = f.root.clone();
        let paths = crate::runtime::RuntimePaths {
            data_root: f._dir.path().to_owned(),
            database_path: db,
            objects_path: f.root.join("objects"),
            exports_path: f._dir.path().join("exports"),
            backups_path: backups,
            logs_path: f._dir.path().join("logs"),
            archive: location,
            archive_lease: None,
        };
        while !crate::chat_redaction_recheck::step(&f.root, &paths)
            .unwrap()
            .available
        {}
        let app = tauri::test::mock_builder()
            .manage(crate::AppState {
                paths,
                approvals: Default::default(),
                setup_secrets: Default::default(),
                usage_monitor: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                usage_cache: Default::default(),
                chat_active: Default::default(),
                browser: Default::default(),
                selections: Default::default(),
            })
            .manage(TestRemote {
                tool: f.tool.clone(),
                target: f.remote().to_owned(),
            })
            .invoke_handler(tauri::generate_handler![
                archive_remote_push,
                archive_remote_pull,
                crate::zet_create,
                crate::archive_save,
                crate::chat_create
            ])
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let invoke = |cmd, body| crate::command_entry_tests::invoke(&window, cmd, body);
        let pulled = invoke("archive_remote_pull", serde_json::json!({}));
        assert_eq!(pulled["autoMerged"], true);
        assert!(pulled["preservedDocuments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some(id.as_str())));
        assert!(pulled["conflictPaths"].is_null());
        assert_eq!(
            fs::read_to_string(f.root.join(&path)).unwrap(),
            document("remote")
        );
        invoke(
            "zet_create",
            serde_json::json!({"input":{"title":"New","summary":"Summary","kind":"note","body":"Body","assets":[],"edges":[]}}),
        );
        invoke("archive_save", serde_json::json!({}));
        assert_eq!(
            invoke("archive_remote_push", serde_json::json!({}))["state"],
            "connected"
        );
        git(&f.tool, &other, &["pull", "--ff-only"]).unwrap();
        fs::write(other.join("profile.md"), profile("remote")).unwrap();
        git(&f.tool, &other, &["add", "--", "profile.md"]).unwrap();
        git(
            &f.tool,
            &other,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "remote conflict",
            ],
        )
        .unwrap();
        git(&f.tool, &other, &["push", "origin", "main"]).unwrap();
        fs::write(f.root.join("profile.md"), profile("local")).unwrap();
        git(&f.tool, &f.root, &["add", "--", "profile.md"]).unwrap();
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
                "local conflict",
            ],
        )
        .unwrap();
        let conflict = invoke("archive_remote_push", serde_json::json!({}));
        assert_eq!(conflict["reason"], "diverged");
        assert!(conflict["conflictPaths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "profile.md"));
        invoke("chat_create", serde_json::json!({}));
        invoke(
            "zet_create",
            serde_json::json!({"input":{"title":"Later","summary":"Summary","kind":"note","body":"Body","assets":[],"edges":[]}}),
        );
        invoke("archive_save", serde_json::json!({}));
        let retried = invoke("archive_remote_push", serde_json::json!({}));
        assert_eq!(retried["reason"], "diverged");
        assert!(retried["conflictPaths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "profile.md"));
    }

    #[test]
    fn remote_rejection_does_not_change_head_and_retry_pushes() {
        let f = Fixture::new();
        f.push();
        f.local_commit();
        let before = git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap();
        let hook = f.bare.join("hooks/pre-receive");
        fs::write(
            &hook,
            "#!/bin/sh\necho 'GH001: Large files detected' >&2\nexit 1\n",
        )
        .unwrap();
        assert_eq!(
            transfer(&f.tool, &f.root, f.remote(), "main", "push"),
            Err("size_limit")
        );
        assert_eq!(
            git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap(),
            before
        );
        fs::remove_file(&hook).unwrap();
        transfer(&f.tool, &f.root, f.remote(), "main", "push").unwrap();
        let remote_head = git(
            &f.tool,
            &f.root,
            &["ls-remote", f.remote(), "refs/heads/main"],
        )
        .unwrap();
        assert!(remote_head.starts_with(&before));
    }

    #[test]
    fn actual_git_http_404_remains_ambiguous() {
        use std::io::{Read, Write};
        let f = Fixture::new();
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = server.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut buf = [0; 4096];
            let _ = stream.read(&mut buf);
            stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });
        let target = format!("http://{address}/fixture.git");
        let result = remote_git(
            &f.tool,
            &f.root,
            &["ls-remote", "--heads", &target, "refs/heads/main"],
        );
        worker.join().unwrap();
        assert_eq!(result, Err("repository_unavailable_or_denied"));
    }

    #[test]
    fn interrupted_remote_response_keeps_local_commit_and_retry_succeeds() {
        use std::io::{Read, Write};
        let f = Fixture::new();
        f.push();
        f.local_commit();
        let before = git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap();
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = server.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut buf = [0; 4096];
            let _ = stream.read(&mut buf);
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-git-upload-pack-advertisement\r\nContent-Length: 9999\r\nConnection: close\r\n\r\n001e# service=git-upload-pack\n").unwrap();
        });
        let target = format!("http://{address}/fixture.git");
        assert!(transfer(&f.tool, &f.root, &target, "main", "push").is_err());
        worker.join().unwrap();
        assert_eq!(
            git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap(),
            before
        );
        transfer(&f.tool, &f.root, f.remote(), "main", "push").unwrap();
        assert!(git(
            &f.tool,
            &f.root,
            &["ls-remote", f.remote(), "refs/heads/main"]
        )
        .unwrap()
        .starts_with(&before));
    }

    #[test]
    fn interrupted_push_during_remote_receive_can_be_retried() {
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                if self.0.try_wait().ok().flatten().is_none() {
                    let _ = self.0.kill();
                    let _ = self.0.wait();
                }
            }
        }
        let f = Fixture::new();
        f.push();
        f.local_commit();
        let expected = git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap();
        let hook = f.bare.join("hooks/pre-receive");
        fs::write(&hook, "#!/bin/sh\necho ready > momso-test-ready\nsleep 2\necho done > momso-test-done\nexit 0\n").unwrap();
        let mut child = ChildGuard(
            Command::new(&f.tool)
                .current_dir(&f.root)
                .env("GIT_TERMINAL_PROMPT", "0")
                .args([
                    "push",
                    "--no-follow-tags",
                    f.remote(),
                    "HEAD:refs/heads/main",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let start = std::time::Instant::now();
        while !f.bare.join("momso-test-ready").exists() {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(30),
                "receive hook did not start"
            );
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "push ended before interruption"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        let start = std::time::Instant::now();
        while !f.bare.join("momso-test-done").exists() {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(15),
                "receive hook did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
        fs::remove_file(hook).unwrap();
        assert_eq!(
            git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap(),
            expected
        );
        transfer(&f.tool, &f.root, f.remote(), "main", "push").unwrap();
        assert!(git(
            &f.tool,
            &f.root,
            &["ls-remote", f.remote(), "refs/heads/main"]
        )
        .unwrap()
        .starts_with(&expected));
    }

    #[test]
    fn interrupted_first_receive_stops_retry_without_overwriting_user_files() {
        let f = Fixture::new();
        f.push();
        let saved = f.root.join(".git/momso-first-pull-fixture");
        fs::create_dir(&saved).unwrap();
        fs::write(saved.join("AGENTS.md"), "original").unwrap();
        fs::write(f.root.join("AGENTS.md"), "new user content").unwrap();
        fs::write(
            f.root.join(".git/momso-first-receive-in-progress"),
            "momso-first-pull-fixture",
        )
        .unwrap();
        assert_eq!(
            transfer(&f.tool, &f.root, f.remote(), "main", "pull"),
            Err("first_receive_interrupted")
        );
        assert_eq!(
            fs::read_to_string(saved.join("AGENTS.md")).unwrap(),
            "original"
        );
        assert_eq!(
            fs::read_to_string(f.root.join("AGENTS.md")).unwrap(),
            "new user content"
        );
    }

    #[test]
    fn first_receive_crash_child() {
        let Ok(root) = std::env::var("MOMSO_TEST_FIRST_RECEIVE_ROOT") else {
            return;
        };
        let tool = std::env::var("MOMSO_TEST_FIRST_RECEIVE_GIT").unwrap();
        let revision = std::env::var("MOMSO_TEST_FIRST_RECEIVE_REVISION").unwrap();
        let crash_after = std::env::var("MOMSO_TEST_FIRST_RECEIVE_AFTER")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1);
        let mut moved = 0;
        let _ = first_receive_with_hooks(
            Path::new(&tool),
            Path::new(&root),
            &revision,
            || {},
            || {
                moved += 1;
                if moved == crash_after {
                    std::process::exit(73);
                }
            },
        );
        panic!("expected interrupted process");
    }

    #[test]
    fn abrupt_first_receive_exit_keeps_saved_original_and_new_user_file() {
        let f = Fixture::new();
        f.push();
        let fresh = f._dir.path().join("fresh-crash");
        fs::create_dir(&fresh).unwrap();
        crate::archive::initialize(&fresh, f._dir.path()).unwrap();
        let original = fs::read(fresh.join(".gitignore")).unwrap();
        relation(&f.tool, &fresh, f.remote(), "main").unwrap();
        let revision = git(
            &f.tool,
            &fresh,
            &["rev-parse", "refs/remotes/momso/observed"],
        )
        .unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "archive_remote::tests::first_receive_crash_child",
                "--nocapture",
            ])
            .env("MOMSO_TEST_FIRST_RECEIVE_ROOT", &fresh)
            .env("MOMSO_TEST_FIRST_RECEIVE_GIT", &f.tool)
            .env("MOMSO_TEST_FIRST_RECEIVE_REVISION", &revision)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert_eq!(status.status.code(), Some(73));
        let marker =
            fs::read_to_string(fresh.join(".git/momso-first-receive-in-progress")).unwrap();
        assert!(marker.starts_with("momso-first-pull-"));
        let saved = fresh.join(".git").join(marker);
        assert_eq!(fs::read(saved.join(".gitignore")).unwrap(), original);
        fs::write(fresh.join(".gitignore"), "user data").unwrap();
        assert_eq!(
            transfer(&f.tool, &fresh, f.remote(), "main", "pull"),
            Err("first_receive_interrupted")
        );
        assert_eq!(
            fs::read_to_string(fresh.join(".gitignore")).unwrap(),
            "user data"
        );
        assert_eq!(fs::read(saved.join(".gitignore")).unwrap(), original);
        fs::remove_file(fresh.join(".gitignore")).unwrap();
        transfer(&f.tool, &fresh, f.remote(), "main", "pull").unwrap();
        assert_eq!(
            fs::read_to_string(fresh.join("zettels/a.md")).unwrap(),
            "initial"
        );
        assert!(!fresh.join(".git/momso-first-receive-in-progress").exists());
    }

    #[test]
    #[ignore = "requires a real Git executable"]
    fn abrupt_first_receive_after_multiple_moves_or_directory_recovers() {
        let f = Fixture::new();
        f.push();
        for count in [2, 5] {
            let fresh = f._dir.path().join(format!("fresh-crash-{count}"));
            fs::create_dir(&fresh).unwrap();
            crate::archive::initialize(&fresh, f._dir.path()).unwrap();
            relation(&f.tool, &fresh, f.remote(), "main").unwrap();
            let revision = git(
                &f.tool,
                &fresh,
                &["rev-parse", "refs/remotes/momso/observed"],
            )
            .unwrap();
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "archive_remote::tests::first_receive_crash_child",
                    "--nocapture",
                ])
                .env("MOMSO_TEST_FIRST_RECEIVE_ROOT", &fresh)
                .env("MOMSO_TEST_FIRST_RECEIVE_GIT", &f.tool)
                .env("MOMSO_TEST_FIRST_RECEIVE_REVISION", &revision)
                .env("MOMSO_TEST_FIRST_RECEIVE_AFTER", count.to_string())
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert_eq!(status.status.code(), Some(73));
            transfer(&f.tool, &fresh, f.remote(), "main", "pull").unwrap();
            assert_eq!(
                fs::read_to_string(fresh.join("zettels/a.md")).unwrap(),
                "initial"
            );
            assert!(!fresh.join(".git/momso-first-receive-in-progress").exists());
        }
    }

    #[test]
    fn first_receive_database_crash_child() {
        let Ok(root) = std::env::var("MOMSO_C28_ROOT") else {
            return;
        };
        let db = std::env::var("MOMSO_C28_DB").unwrap();
        let tool = std::env::var("MOMSO_C28_GIT").unwrap();
        let crash = std::env::var("MOMSO_C28_PHASE").unwrap();
        let root = Path::new(&root);
        let db = Path::new(&db);
        let _ = receive_first_with_progress(
            Path::new(&tool),
            root,
            db,
            "fixture",
            "main",
            |event| {
                if crash == "before_index" && event.state == "indexing" {
                    std::process::exit(73);
                }
                if crash == "before_restore" && event.state == "restoring" {
                    std::process::exit(73);
                }
            },
            || {
                crate::chat_archive_store::restore_with_progress(root, db, |event| {
                    if crash == "during_restore"
                        && event.state == "restoring"
                        && event.completed == 1
                    {
                        std::process::exit(73);
                    }
                })?;
                if crash == "after_restore" {
                    std::process::exit(73);
                }
                Ok(())
            },
        );
        panic!("expected interrupted process");
    }

    #[test]
    #[ignore = "requires a real Git executable"]
    fn missing_file_after_restore_intent_is_recreated() {
        let f = Fixture::new();
        let base = f._dir.path().join("ambiguous");
        let root = base.join("momso");
        let saved = base.join("saved");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&saved).unwrap();
        git(&f.tool, &root, &["init", "--initial-branch=main"]).unwrap();
        fs::write(saved.join("AGENTS.md"), "saved original").unwrap();
        let db = base.join("local.sqlite");
        crate::storage::initialize(&db, &base.join("backup"), "test").unwrap();
        let mut journal = begin_receive_journal(&db, &root, "fixture", "main").unwrap();
        journal.phase = "checkout".into();
        journal.restore_intents.push("AGENTS.md".into());
        write_receive_journal(&db, &journal).unwrap();
        restore_journaled_defaults(&f.tool, &root, &db, &saved).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "saved original"
        );
        assert_eq!(
            fs::read_to_string(saved.join("AGENTS.md")).unwrap(),
            "saved original"
        );
        assert!(read_receive_journal(&db).unwrap().is_some());
    }

    #[test]
    fn first_receive_checkout_crash_child() {
        let Ok(root) = std::env::var("MOMSO_C28_ROOT") else {
            return;
        };
        let db = std::env::var("MOMSO_C28_DB").unwrap();
        let tool = std::env::var("MOMSO_C28_GIT").unwrap();
        let target = std::env::var("MOMSO_C28_TARGET").unwrap();
        let root = Path::new(&root);
        let db = Path::new(&db);
        let _ = receive_first_with_progress(
            Path::new(&tool),
            root,
            db,
            &target,
            "main",
            |_| {},
            || crate::chat_archive_store::restore(root, db),
        );
        panic!("expected interrupted process");
    }

    #[test]
    #[ignore = "requires a real Git executable"]
    fn checkout_and_skill_crashes_restore_received_document_and_keep_local_work() {
        for phase in ["checkout", "skill"] {
            let f = Fixture::new();
            fs::remove_file(f.root.join("zettels/a.md")).unwrap();
            let remote_id = "zet_20261115_143052";
            let document = |id: &str| {
                format!("---\nid: {id}\ntitle: Example\nsummary: Example\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets: []\nedges: []\n---\nbody\n")
            };
            fs::write(
                f.root.join("zettels").join(format!("{remote_id}.md")),
                document(remote_id),
            )
            .unwrap();
            if phase == "skill" {
                let (name, content) = crate::skills::BUNDLED_SKILLS[0];
                let dir = f.root.join("skills").join(name);
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join("SKILL.md"), format!("{content}\n")).unwrap();
            }
            git(&f.tool, &f.root, &["add", "--all", "--", "zettels"]).unwrap();
            if phase == "skill" {
                git(&f.tool, &f.root, &["add", "--", "skills"]).unwrap();
            }
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
                    "receive fixture",
                ],
            )
            .unwrap();
            f.push();
            let base = f._dir.path().join("new-pc");
            let root = base.join("momso");
            fs::create_dir_all(&root).unwrap();
            crate::archive::initialize(&root, &base).unwrap();
            let db = base.join("local.sqlite");
            crate::storage::initialize(&db, &base.join("backup"), "test").unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "archive_remote::tests::first_receive_checkout_crash_child",
                    "--nocapture",
                ])
                .env("MOMSO_C28_ROOT", &root)
                .env("MOMSO_C28_DB", &db)
                .env("MOMSO_C28_GIT", &f.tool)
                .env("MOMSO_C28_TARGET", f.remote())
                .stdin(Stdio::null());
            if phase == "checkout" {
                child.env("MOMSO_C28_CRASH_AFTER_CHECKOUT", "1");
            } else {
                child.env("MOMSO_C28_CRASH_DURING_SKILL", "1");
            }
            assert_eq!(child.output().unwrap().status.code(), Some(73), "{phase}");
            assert_eq!(
                read_receive_journal(&db).unwrap().unwrap().phase,
                "checkout"
            );
            fs::remove_file(root.join("zettels").join(format!("{remote_id}.md"))).unwrap();
            let local_id = "zet_20261115_143053";
            fs::write(
                root.join("zettels").join(format!("{local_id}.md")),
                document(local_id),
            )
            .unwrap();
            let local = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
            let local_chat = local["id"].as_str().unwrap();
            receive_first_with_progress(
                &f.tool,
                &root,
                &db,
                f.remote(),
                "main",
                |_| {},
                || crate::chat_archive_store::restore(&root, &db),
            )
            .unwrap();
            assert!(root
                .join("zettels")
                .join(format!("{remote_id}.md"))
                .exists());
            assert_eq!(
                fs::read_to_string(root.join("zettels").join(format!("{local_id}.md"))).unwrap(),
                document(local_id)
            );
            assert_eq!(
                serde_json::to_value(crate::chat::list(&db).unwrap())
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            assert!(crate::chat::read(&db, local_chat).is_ok());
            assert!(read_receive_journal(&db).unwrap().is_none());
            assert!(!root.join(".git/momso-first-receive-in-progress").exists());
        }
    }

    #[test]
    #[ignore = "requires a real Git executable"]
    fn dictionary_file_recovery_survives_process_exit() {
        let f = Fixture::new();
        fs::remove_file(f.root.join("zettels/a.md")).unwrap();
        let id = "zet_20261115_143052";
        fs::write(f.root.join("zettels").join(format!("{id}.md")), format!("---\nid: {id}\ntitle: Example\nsummary: Example\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets: []\nedges: []\n---\nbody\n")).unwrap();
        crate::yoga_dictionary::initialize(&f.root).unwrap();
        git(
            &f.tool,
            &f.root,
            &["add", "--all", "--", "zettels", "dictionary"],
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
                "dictionary fixture",
            ],
        )
        .unwrap();
        f.push();
        let base = f._dir.path().join("dictionary-crash");
        let root = base.join("momso");
        fs::create_dir_all(&root).unwrap();
        crate::archive::initialize(&root, &base).unwrap();
        let db = base.join("local.sqlite");
        crate::storage::initialize(&db, &base.join("backup"), "test").unwrap();
        let child = |crash_name: &str| {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "archive_remote::tests::first_receive_checkout_crash_child",
                    "--nocapture",
                ])
                .env("MOMSO_C28_ROOT", &root)
                .env("MOMSO_C28_DB", &db)
                .env("MOMSO_C28_GIT", &f.tool)
                .env("MOMSO_C28_TARGET", f.remote())
                .env(crash_name, "1")
                .stdin(Stdio::null());
            command.output().unwrap().status.code()
        };
        assert_eq!(child("MOMSO_C28_CRASH_AFTER_CHECKOUT"), Some(73));
        fs::remove_file(root.join("dictionary/yoga.json")).unwrap();
        assert_eq!(child("MOMSO_C31_CRASH_DICTIONARY"), Some(73));
        assert!(root.join("dictionary/yoga.json").is_file());
        receive_first_with_progress(
            &f.tool,
            &root,
            &db,
            f.remote(),
            "main",
            |_| {},
            || crate::chat_archive_store::restore(&root, &db),
        )
        .unwrap();
        assert!(read_receive_journal(&db).unwrap().is_none());
        assert!(crate::yoga_dictionary::list(&root).is_ok());
    }

    #[test]
    #[ignore = "requires a real Git executable"]
    fn renamed_conflicting_default_can_resume_first_receive() {
        let f = Fixture::new();
        fs::remove_file(f.root.join("zettels/a.md")).unwrap();
        let id = "zet_20261115_143052";
        let document = |body: &str| {
            format!("---\nid: {id}\ntitle: Example\nsummary: Example\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets: []\nedges: []\n---\n{body}\n")
        };
        fs::write(
            f.root.join("zettels").join(format!("{id}.md")),
            document("received"),
        )
        .unwrap();
        fs::write(f.root.join("AGENTS.md"), "received rules").unwrap();
        git(
            &f.tool,
            &f.root,
            &["add", "--all", "--", "zettels", "AGENTS.md"],
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
                "received fixture",
            ],
        )
        .unwrap();
        f.push();
        let base = f._dir.path().join("conflict-pc");
        let root = base.join("momso");
        fs::create_dir_all(&root).unwrap();
        crate::archive::initialize(&root, &base).unwrap();
        let db = base.join("local.sqlite");
        crate::storage::initialize(&db, &base.join("backup"), "test").unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "archive_remote::tests::first_receive_checkout_crash_child",
                "--nocapture",
            ])
            .env("MOMSO_C28_ROOT", &root)
            .env("MOMSO_C28_DB", &db)
            .env("MOMSO_C28_GIT", &f.tool)
            .env("MOMSO_C28_TARGET", f.remote())
            .env("MOMSO_C31_CRASH_BEFORE_CHECKOUT", "1")
            .stdin(Stdio::null());
        assert_eq!(child.output().unwrap().status.code(), Some(73));
        fs::write(root.join("AGENTS.md"), "local rules").unwrap();
        let mut reported = None;
        let blocked = receive_first_with_progress(
            &f.tool,
            &root,
            &db,
            f.remote(),
            "main",
            |p| reported = Some(p),
            || crate::chat_archive_store::restore(&root, &db),
        );
        assert_eq!(blocked, Err("first_receive_interrupted"));
        let reported = reported.unwrap();
        assert_eq!(reported.reason, Some("first_receive_conflict"));
        assert_eq!(reported.conflict_paths, vec!["AGENTS.md"]);
        let renamed = root.join("AGENTS-local.md");
        fs::rename(root.join("AGENTS.md"), &renamed).unwrap();
        let local_document = root.join("zettels").join(format!("{id}.md"));
        fs::write(&local_document, document("local")).unwrap();
        use std::io::Write;
        writeln!(
            fs::OpenOptions::new()
                .append(true)
                .open(root.join(".git/info/exclude"))
                .unwrap(),
            "zettels/{id}.md"
        )
        .unwrap();
        let collision = receive_first_with_progress(
            &f.tool,
            &root,
            &db,
            f.remote(),
            "main",
            |_| {},
            || crate::chat_archive_store::restore(&root, &db),
        );
        assert_eq!(collision, Err("fast_forward_failed"));
        assert_eq!(
            fs::read_to_string(&local_document).unwrap(),
            document("local")
        );
        let moved_document = root.join("zettels/local-preserved.txt");
        fs::rename(&local_document, &moved_document).unwrap();
        receive_first_with_progress(
            &f.tool,
            &root,
            &db,
            f.remote(),
            "main",
            |_| {},
            || crate::chat_archive_store::restore(&root, &db),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&renamed).unwrap(), "local rules");
        assert_eq!(
            fs::read_to_string(&moved_document).unwrap(),
            document("local")
        );
        assert_eq!(
            fs::read_to_string(root.join("zettels").join(format!("{id}.md"))).unwrap(),
            document("received")
        );
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "received rules"
        );
    }

    #[test]
    #[ignore = "requires a real Git executable"]
    fn missing_received_dictionary_is_replaced_without_overwriting_other_files() {
        let f = Fixture::new();
        fs::create_dir_all(f.root.join("dictionary")).unwrap();
        fs::write(f.root.join("dictionary/yoga.json"), "received dictionary").unwrap();
        git(&f.tool, &f.root, &["add", "--", "dictionary/yoga.json"]).unwrap();
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
        let revision = git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap();
        fs::remove_file(f.root.join("dictionary/yoga.json")).unwrap();
        fs::write(f.root.join("zettels/a.md"), "user edit").unwrap();
        fs::write(f.root.join("zettels/new.md"), "new user document").unwrap();
        restore_missing_received_files(&f.tool, &f.root, &revision, &[]).unwrap();
        assert_eq!(
            fs::read_to_string(f.root.join("dictionary/yoga.json")).unwrap(),
            "received dictionary"
        );
        assert_eq!(
            fs::read_to_string(f.root.join("zettels/a.md")).unwrap(),
            "user edit"
        );
        assert_eq!(
            fs::read_to_string(f.root.join("zettels/new.md")).unwrap(),
            "new user document"
        );
        assert_eq!(
            git(&f.tool, &f.root, &["show", "HEAD:zettels/a.md"]).unwrap(),
            "initial"
        );
    }

    #[test]
    #[ignore = "requires a real Git executable"]
    fn app_start_preparation_resumes_and_emits_progress() {
        use tauri::Listener;
        let f = Fixture::new();
        fs::remove_file(f.root.join("zettels/a.md")).unwrap();
        let id = "zet_20261115_143052";
        fs::write(f.root.join("zettels").join(format!("{id}.md")), format!("---\nid: {id}\ntitle: Example\nsummary: Example\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets: []\nedges: []\n---\nbody\n")).unwrap();
        git(&f.tool, &f.root, &["add", "--all", "--", "zettels"]).unwrap();
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
                "receive fixture",
            ],
        )
        .unwrap();
        f.push();
        let base = f._dir.path().join("startup-pc");
        let root = base.join("momso");
        fs::create_dir_all(&root).unwrap();
        crate::archive::initialize(&root, &base).unwrap();
        let db = base.join("local.sqlite");
        let backups = base.join("backup");
        crate::storage::initialize(&db, &backups, "test").unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "archive_remote::tests::first_receive_checkout_crash_child",
                "--nocapture",
            ])
            .env("MOMSO_C28_ROOT", &root)
            .env("MOMSO_C28_DB", &db)
            .env("MOMSO_C28_GIT", &f.tool)
            .env("MOMSO_C28_TARGET", f.remote())
            .env("MOMSO_C28_CRASH_AFTER_CHECKOUT", "1")
            .stdin(Stdio::null());
        assert_eq!(child.output().unwrap().status.code(), Some(73));
        let received = root.join("zettels").join(format!("{id}.md"));
        fs::remove_file(&received).unwrap();
        Connection::open(&db).unwrap().execute(
            "INSERT INTO archive_location_settings(id,path,initialized) VALUES(1,?1,1) ON CONFLICT(id) DO UPDATE SET path=excluded.path,initialized=1",
            [root.to_str().unwrap()],
        ).unwrap();
        let archive =
            crate::archive_location::ArchiveLocation::load(db.clone(), &base, vec![]).unwrap();
        let paths = crate::runtime::RuntimePaths {
            data_root: base.clone(),
            database_path: db.clone(),
            objects_path: root.join("objects"),
            exports_path: base.join("exports"),
            backups_path: backups,
            logs_path: base.join("logs"),
            archive,
            archive_lease: None,
        };
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let recorded = events.clone();
        let app = tauri::test::mock_builder()
            .manage(TestRemote {
                tool: f.tool.clone(),
                target: f.remote().to_owned(),
            })
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.listen("archive-first-receive-progress", move |event| {
            recorded
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).unwrap());
        });
        crate::prepare_archive_for_start(&paths, &base).unwrap();
        assert!(!received.exists());
        start_first_receive_recovery(app.handle().clone(), paths, base);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while std::time::Instant::now() < until {
            if events
                .lock()
                .unwrap()
                .iter()
                .any(|event| event["state"] == "complete" || event["state"] == "interrupted")
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let events = events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|event| event["state"] == "recovering" && event["available"] == false),
            "events: {events:?}"
        );
        assert!(events
            .iter()
            .any(|event| event["state"] == "complete" && event["available"] == true));
        assert!(received.is_file());
        assert!(read_receive_journal(&db).unwrap().is_none());
        drop(app);
    }

    #[test]
    #[ignore = "requires a real Git executable"]
    fn receiving_journal_retries_with_an_incomplete_pack_file() {
        let f = Fixture::new();
        fs::remove_file(f.root.join("zettels/a.md")).unwrap();
        let id = "zet_20261115_143052";
        fs::write(f.root.join("zettels").join(format!("{id}.md")), format!("---\nid: {id}\ntitle: Example\nsummary: Example\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets: []\nedges: []\n---\nbody\n")).unwrap();
        git(&f.tool, &f.root, &["add", "--all", "--", "zettels"]).unwrap();
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
                "valid document",
            ],
        )
        .unwrap();
        f.push();
        for interrupted_download in [false, true] {
            let base = f._dir.path().join(format!("new-pc-{interrupted_download}"));
            let root = base.join("momso");
            fs::create_dir_all(&root).unwrap();
            crate::archive::initialize(&root, &base).unwrap();
            let db = base.join("local.sqlite");
            crate::storage::initialize(&db, &base.join("backup"), "test").unwrap();
            begin_receive_journal(&db, &root, f.remote(), "main").unwrap();
            if interrupted_download {
                let pack = root.join(".git/objects/pack");
                fs::create_dir_all(&pack).unwrap();
                fs::write(pack.join("tmp_pack_interrupted"), b"PACK\0\0").unwrap();
            }
            let mut states = Vec::new();
            receive_first_with_progress(
                &f.tool,
                &root,
                &db,
                f.remote(),
                "main",
                |value| {
                    if value.state == "indexing" {
                        assert!(read_receive_journal(&db)
                            .unwrap()
                            .unwrap()
                            .revision
                            .is_some());
                    }
                    states.push(value);
                },
                || Ok(()),
            )
            .unwrap();
            assert!(read_receive_journal(&db).unwrap().is_none());
            assert!(root.join("zettels").join(format!("{id}.md")).is_file());
            assert!(states
                .iter()
                .any(|state| state.state == "complete" && state.available));
        }
    }

    #[test]
    #[ignore = "requires real Git and local loopback sockets"]
    fn interrupted_git_fetch_retries_from_local_transport() {
        use std::io::{Read, Write};
        use std::net::{Shutdown, TcpListener, TcpStream};
        use std::sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc,
        };
        use std::time::{Duration, Instant};

        struct OwnedChild(std::process::Child);
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let f = Fixture::new();
        fs::remove_file(f.root.join("zettels/a.md")).unwrap();
        let id = "zet_20261115_143052";
        let mut content = format!("---\nid: {id}\ntitle: Example\nsummary: Example\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets: []\nedges: []\n---\n").into_bytes();
        let mut random = 0x1234_5678_9abc_def0_u64;
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        for _ in 0..(8 * 1024 * 1024) {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            content.push(alphabet[(random as usize) & 63]);
        }
        fs::write(f.root.join("zettels").join(format!("{id}.md")), content).unwrap();
        git(&f.tool, &f.root, &["add", "--all", "--", "zettels"]).unwrap();
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
                "transfer fixture",
            ],
        )
        .unwrap();
        f.push();
        let base = f._dir.path().join("fetch-crash");
        let root = base.join("momso");
        fs::create_dir_all(&root).unwrap();
        crate::archive::initialize(&root, &base).unwrap();
        let db = base.join("local.sqlite");
        crate::storage::initialize(&db, &base.join("backup"), "test").unwrap();
        begin_receive_journal(&db, &root, f.remote(), "main").unwrap();

        let daemon_probe = TcpListener::bind("127.0.0.1:0").unwrap();
        let daemon_addr = daemon_probe.local_addr().unwrap();
        drop(daemon_probe);
        let mut daemon_command = Command::new(&f.tool);
        hidden(&mut daemon_command)
            .args([
                "daemon",
                "--reuseaddr",
                "--export-all",
                "--listen=127.0.0.1",
            ])
            .arg(format!("--port={}", daemon_addr.port()))
            .arg(format!("--base-path={}", f._dir.path().display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut daemon = OwnedChild(daemon_command.spawn().unwrap());
        let until = Instant::now() + Duration::from_secs(15);
        while TcpStream::connect_timeout(&daemon_addr, Duration::from_millis(100)).is_err() {
            assert!(Instant::now() < until, "local Git daemon did not start");
            std::thread::sleep(Duration::from_millis(25));
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_addr = listener.local_addr().unwrap();
        let transmitted = Arc::new(AtomicUsize::new(0));
        let stopping = Arc::new(AtomicBool::new(false));
        let counter = transmitted.clone();
        let stop = stopping.clone();
        let proxy = std::thread::spawn(move || {
            let (mut client, _) = listener.accept().unwrap();
            let mut server = TcpStream::connect(daemon_addr).unwrap();
            server
                .set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
            let mut input = client.try_clone().unwrap();
            let mut upstream = server.try_clone().unwrap();
            let pump = std::thread::spawn(move || {
                let _ = std::io::copy(&mut input, &mut upstream);
                let _ = upstream.shutdown(Shutdown::Write);
            });
            let mut buffer = [0_u8; 8192];
            while !stop.load(Ordering::Relaxed) {
                match server.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        if client.write_all(&buffer[..count]).is_err() {
                            break;
                        }
                        counter.fetch_add(count, Ordering::Relaxed);
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(_) => break,
                }
            }
            let _ = client.shutdown(Shutdown::Both);
            let _ = server.shutdown(Shutdown::Both);
            let _ = pump.join();
        });
        let url = format!("git://127.0.0.1:{}/remote.git", proxy_addr.port());
        let mut fetch_command = Command::new(&f.tool);
        hidden(&mut fetch_command)
            .current_dir(&root)
            .args([
                "-c",
                "fetch.unpackLimit=1",
                "fetch",
                "--no-tags",
                &url,
                "+refs/heads/main:refs/remotes/momso/observed",
            ])
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut fetch = OwnedChild(fetch_command.spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(45);
        while transmitted.load(Ordering::Relaxed) < 256 * 1024 {
            assert!(
                Instant::now() < deadline,
                "Git did not start receiving the pack"
            );
            assert!(
                fetch.0.try_wait().unwrap().is_none(),
                "Git exited before interruption"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        fetch.0.kill().unwrap();
        let status = fetch.0.wait().unwrap();
        assert!(!status.success());
        stopping.store(true, Ordering::Relaxed);
        let _ = daemon.0.kill();
        let _ = daemon.0.wait();
        proxy.join().unwrap();
        assert!(read_receive_journal(&db).unwrap().is_some());
        receive_first_with_progress(&f.tool, &root, &db, f.remote(), "main", |_| {}, || Ok(()))
            .unwrap();
        assert!(read_receive_journal(&db).unwrap().is_none());
        assert!(root.join("zettels").join(format!("{id}.md")).is_file());
        git(&f.tool, &root, &["fsck", "--no-reflogs"]).unwrap();
    }

    #[test]
    #[ignore = "requires a real Git executable"]
    fn automatic_connection_emits_progress_matching_status() {
        use tauri::Listener;
        let f = Fixture::new();
        fs::remove_file(f.root.join("zettels/a.md")).unwrap();
        let id = "zet_20261115_143052";
        fs::write(f.root.join("zettels").join(format!("{id}.md")), format!("---\nid: {id}\ntitle: Example\nsummary: Example\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets: []\nedges: []\n---\nbody\n")).unwrap();
        git(&f.tool, &f.root, &["add", "--all", "--", "zettels"]).unwrap();
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
                "valid document",
            ],
        )
        .unwrap();
        f.push();
        let base = f._dir.path().join("automatic-connect");
        let root = base.join("momso");
        fs::create_dir_all(&root).unwrap();
        crate::archive::initialize(&root, &base).unwrap();
        let db = base.join("local.sqlite");
        crate::storage::initialize(&db, &base.join("backup"), "test").unwrap();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = events.clone();
        let status_root = root.clone();
        let status_db = db.clone();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.listen("archive-first-receive-progress", move |event| {
            let event: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
            let status =
                serde_json::to_value(current_first_receive_progress(&status_root, &status_db))
                    .unwrap();
            captured.lock().unwrap().push((event, status));
        });
        receive_connected_with_progress(
            &f.tool,
            &root,
            &db,
            f.remote(),
            "main",
            Some(app.handle()),
        )
        .unwrap();
        let events = events.lock().unwrap();
        assert!(events
            .iter()
            .any(|(event, _)| event["state"] == "receiving" && event["available"] == false));
        assert!(events
            .iter()
            .any(|(event, _)| event["state"] == "complete" && event["available"] == true));
        assert!(
            events.iter().all(|(event, status)| event == status),
            "{events:?}"
        );
    }

    #[test]
    #[ignore = "requires a real Git executable"]
    fn legacy_journal_without_verified_revision_never_completes() {
        let f = Fixture::new();
        let base = f._dir.path().join("unknown-revision");
        let root = base.join("momso");
        fs::create_dir_all(&root).unwrap();
        crate::archive::initialize(&root, &base).unwrap();
        let db = base.join("local.sqlite");
        crate::storage::initialize(&db, &base.join("backup"), "test").unwrap();
        let mut journal = begin_receive_journal(&db, &root, "fixture", "main").unwrap();
        journal.phase = "indexing".into();
        write_receive_journal(&db, &journal).unwrap();
        let result = receive_first_with_progress(
            &f.tool,
            &root,
            &db,
            "fixture",
            "main",
            |_| {},
            || panic!("must not restore without a verified revision"),
        );
        assert_eq!(result, Err("first_receive_interrupted"));
        assert!(read_receive_journal(&db).unwrap().is_some());
    }

    #[test]
    #[ignore = "requires a real Git executable"]
    fn interrupted_index_and_chat_restore_resume_without_duplicates_or_resurrection() {
        let f = Fixture::new();
        for phase in [
            "before_index",
            "during_index",
            "before_restore",
            "during_restore",
            "after_restore",
        ] {
            let base = f._dir.path().join(format!("journal-{phase}"));
            let root = base.join("archive");
            fs::create_dir_all(&root).unwrap();
            crate::archive::initialize(&root, &base).unwrap();
            let db = base.join("local.sqlite");
            let source = base.join("source.sqlite");
            crate::storage::initialize(&db, &base.join("backup"), "test").unwrap();
            crate::storage::initialize(&source, &base.join("source-backup"), "test").unwrap();
            let id = "zet_20261115_143052";
            fs::write(root.join("zettels").join(format!("{id}.md")), format!("---\nid: {id}\ntitle: Example\nsummary: Example\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets: []\nedges: []\n---\nbody\n")).unwrap();
            let remote = serde_json::to_value(crate::chat::create(&source).unwrap()).unwrap();
            let remote_id = remote["id"].as_str().unwrap();
            crate::chat::append(
                &source,
                remote_id,
                crate::chat::Role::User,
                "remote message",
                None,
                None,
            )
            .unwrap();
            let deleted = serde_json::to_value(crate::chat::create(&source).unwrap()).unwrap();
            let deleted_id = deleted["id"].as_str().unwrap();
            fs::create_dir_all(root.join("chats")).unwrap();
            for chat in [remote_id, deleted_id] {
                let data = crate::chat_archive_snapshot::snapshot(&source, chat).unwrap();
                fs::write(
                    root.join("chats").join(format!("{chat}.json")),
                    serde_json::to_vec(&data).unwrap(),
                )
                .unwrap();
            }
            git(&f.tool, &root, &["add", "--", "zettels", "chats"]).unwrap();
            git(
                &f.tool,
                &root,
                &[
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "commit",
                    "-m",
                    "received fixture",
                ],
            )
            .unwrap();
            let revision = git(&f.tool, &root, &["rev-parse", "HEAD"]).unwrap();
            git(
                &f.tool,
                &root,
                &["update-ref", "refs/remotes/momso/observed", &revision],
            )
            .unwrap();
            let conn = Connection::open(&db).unwrap();
            conn.execute("INSERT INTO app_runtime_meta(key,value,updated_at) VALUES(?1,'pending',CURRENT_TIMESTAMP)", [format!("chat_archive_delete:{deleted_id}")]).unwrap();
            let deleted_snapshot =
                fs::read(root.join("chats").join(format!("{deleted_id}.json"))).unwrap();
            conn.execute("INSERT INTO app_runtime_meta(key,value,updated_at) VALUES(?1,?2,CURRENT_TIMESTAMP)", rusqlite::params![format!("chat_archive_snapshot_hash:{deleted_id}"), crate::object_store::digest(&deleted_snapshot)]).unwrap();
            conn.execute(
                "INSERT INTO chat_deletions(chat_id,deleted_at) VALUES(?1,'2026-11-17T00:00:00Z')",
                [&deleted_id],
            )
            .unwrap();
            drop(conn);
            let mut journal = begin_receive_journal(&db, &root, "fixture", "main").unwrap();
            journal.phase = "indexing".into();
            write_receive_journal(&db, &journal).unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "archive_remote::tests::first_receive_database_crash_child",
                    "--nocapture",
                ])
                .env("MOMSO_C28_ROOT", &root)
                .env("MOMSO_C28_DB", &db)
                .env("MOMSO_C28_GIT", &f.tool)
                .env("MOMSO_C28_PHASE", phase)
                .stdin(Stdio::null());
            if phase == "during_index" {
                child.env("MOMSO_C28_CRASH_DURING_INDEX", "1");
            }
            assert_eq!(child.output().unwrap().status.code(), Some(73), "{phase}");
            assert!(read_receive_journal(&db).unwrap().is_some());
            let local = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap();
            let local_id = local["id"].as_str().unwrap();
            crate::chat::append(
                &db,
                local_id,
                crate::chat::Role::User,
                "local message",
                None,
                None,
            )
            .unwrap();
            receive_first_with_progress(
                &f.tool,
                &root,
                &db,
                "fixture",
                "main",
                |_| {},
                || crate::chat_archive_store::restore(&root, &db),
            )
            .unwrap();
            assert!(read_receive_journal(&db).unwrap().is_none());
            let rooms = serde_json::to_value(crate::chat::list(&db).unwrap()).unwrap();
            let rooms = rooms.as_array().unwrap();
            assert_eq!(rooms.len(), 2, "{phase}");
            assert!(rooms.iter().any(|room| room["id"] == remote_id));
            assert!(rooms.iter().any(|room| room["id"] == local_id));
            let tombstone: serde_json::Value = serde_json::from_slice(
                &fs::read(root.join("chats").join(format!("{deleted_id}.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(tombstone["deleted"], true);
            assert_eq!(
                Connection::open(&db)
                    .unwrap()
                    .query_row(
                        "SELECT count(*) FROM archive_zets WHERE id=?1",
                        [id],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                1
            );
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
    fn first_receive_keeps_missing_bundled_skills_when_remote_has_custom_skill() {
        let f = Fixture::new();
        let remote_skill = f.root.join("skills/custom/SKILL.md");
        fs::create_dir_all(remote_skill.parent().unwrap()).unwrap();
        fs::write(&remote_skill, "custom skill").unwrap();
        let remote_default = f.root.join("skills/github-connect/SKILL.md");
        fs::create_dir_all(remote_default.parent().unwrap()).unwrap();
        fs::write(&remote_default, "user edited default").unwrap();
        git(&f.tool, &f.root, &["add", "--", "skills/"]).unwrap();
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
                "custom skill",
            ],
        )
        .unwrap();
        f.push();
        let fresh = f._dir.path().join("fresh-partial-skills");
        fs::create_dir_all(&fresh).unwrap();
        crate::archive::initialize(&fresh, f._dir.path()).unwrap();
        connect_local(&f.tool, &fresh, f.remote()).unwrap();
        transfer(&f.tool, &fresh, f.remote(), "main", "pull").unwrap();
        assert_eq!(
            fs::read(fresh.join("skills/custom/SKILL.md")).unwrap(),
            b"custom skill"
        );
        for (name, text) in crate::skills::BUNDLED_SKILLS {
            let expected = if *name == "github-connect" {
                b"user edited default".as_slice()
            } else {
                text.as_bytes()
            };
            assert_eq!(
                fs::read(fresh.join("skills").join(name).join("SKILL.md")).unwrap(),
                expected
            );
        }
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
    fn merges_disjoint_divergence_without_losing_either_parent() {
        let f = Fixture::new();
        f.push();
        f.clone_and_commit();
        f.local_commit();
        assert_eq!(
            transfer_report(&f.tool, &f.root, f.remote(), "main", "push"),
            Ok(true)
        );
        assert_eq!(
            fs::read_to_string(f.root.join("zettels/b.md")).unwrap(),
            "remote"
        );
        assert_eq!(
            fs::read_to_string(f.root.join("zettels/local.md")).unwrap(),
            "local"
        );
        let parents = git(
            &f.tool,
            &f.root,
            &["rev-list", "--parents", "-n", "1", "HEAD"],
        )
        .unwrap();
        assert_eq!(parents.split_whitespace().count(), 3);
    }
    #[test]
    fn overlapping_divergence_keeps_both_histories_unchanged() {
        let f = Fixture::new();
        f.push();
        f.clone_and_commit();
        fs::write(f.root.join("zettels/b.md"), "local").unwrap();
        git(&f.tool, &f.root, &["add", "--", "zettels/b.md"]).unwrap();
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
                "local",
            ],
        )
        .unwrap();
        let before = git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(
            transfer(&f.tool, &f.root, f.remote(), "main", "push"),
            Err("diverged")
        );
        assert_eq!(
            git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap(),
            before
        );
        assert_eq!(
            fs::read_to_string(f.root.join("zettels/b.md")).unwrap(),
            "local"
        );
        assert_eq!(
            conflicting_paths(&f.tool, &f.root).unwrap(),
            vec!["zettels/b.md"]
        );
    }

    #[test]
    fn same_document_uses_remote_and_retains_local_version_in_history() {
        let f = Fixture::new();
        let id = "zet_20261115_143052";
        let path = format!("zettels/{id}.md");
        let document = |body: &str| {
            format!("---\nid: {id}\ntitle: Lesson\nsummary: Summary\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets: []\nedges: []\n---\n{body}\n")
        };
        fs::write(f.root.join(&path), document("base")).unwrap();
        git(&f.tool, &f.root, &["add", "--", &path]).unwrap();
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
                "base",
            ],
        )
        .unwrap();
        f.push();
        f.clone_and_commit();
        let other = f._dir.path().join("other");
        fs::write(other.join(&path), document("remote")).unwrap();
        git(&f.tool, &other, &["add", "--", &path]).unwrap();
        git(
            &f.tool,
            &other,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "remote edit",
            ],
        )
        .unwrap();
        git(&f.tool, &other, &["push", "origin", "main"]).unwrap();
        fs::write(f.root.join(&path), document("local")).unwrap();
        git(&f.tool, &f.root, &["add", "--", &path]).unwrap();
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
                "local edit",
            ],
        )
        .unwrap();
        let local = git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap();
        let mut merged_revision = None;
        assert_eq!(
            transfer_with_merge_revision(
                &f.tool,
                &f.root,
                f.remote(),
                "main",
                "pull",
                &mut merged_revision,
                None,
            ),
            Ok(true)
        );
        assert_eq!(
            fs::read_to_string(f.root.join(&path)).unwrap(),
            document("remote")
        );
        let merged_head = merged_revision.unwrap();
        fs::write(f.root.join("zettels/after.md"), "later").unwrap();
        git(&f.tool, &f.root, &["add", "--", "zettels/after.md"]).unwrap();
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
                "later",
            ],
        )
        .unwrap();
        assert_ne!(
            git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap(),
            merged_head
        );
        assert_eq!(
            preserved_documents(&f.tool, &f.root, &merged_head).unwrap(),
            vec![id]
        );
        let versions = crate::archive::history(&f.root, f._dir.path(), id).unwrap();
        let versions = serde_json::to_value(versions).unwrap();
        assert!(versions
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["revision"] == local));
        assert_eq!(
            crate::archive::read_version(&f.root, f._dir.path(), id, &local).unwrap(),
            document("local")
        );
    }

    #[test]
    fn same_document_previous_version_restores_through_archive_api() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        let bare = temp.path().join("remote.git");
        let other = temp.path().join("other");
        let db = temp.path().join("momso.db");
        crate::archive::initialize(&root, temp.path()).unwrap();
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let id = crate::archive_zet::create(
            &root,
            &db,
            crate::archive_zet::ZetCreate {
                title: "Lesson".into(),
                summary: "Summary".into(),
                kind: "note".into(),
                body: "base".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        crate::archive::save(&root, temp.path(), &db).unwrap();
        let tool = crate::process::resolve_tool("git.exe").unwrap();
        git(
            &tool,
            temp.path(),
            &["init", "--bare", bare.to_str().unwrap()],
        )
        .unwrap();
        transfer(&tool, &root, bare.to_str().unwrap(), "main", "push").unwrap();
        git(
            &tool,
            temp.path(),
            &[
                "clone",
                "--branch",
                "main",
                bare.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        )
        .unwrap();
        let path = format!("zettels/{id}.md");
        let base = fs::read_to_string(root.join(&path)).unwrap();
        fs::write(other.join(&path), base.replace("base", "remote")).unwrap();
        git(&tool, &other, &["add", "--", &path]).unwrap();
        git(
            &tool,
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
        git(&tool, &other, &["push", "origin", "main"]).unwrap();
        fs::write(root.join(&path), base.replace("base", "local")).unwrap();
        git(&tool, &root, &["add", "--", &path]).unwrap();
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
                "local",
            ],
        )
        .unwrap();
        let local = git(&tool, &root, &["rev-parse", "HEAD"]).unwrap();
        let dirty = git(
            &tool,
            &root,
            &["status", "--porcelain", "--untracked-files=all"],
        )
        .unwrap();
        assert!(
            dirty.lines().all(|line| line == "?? .gitignore"
                || line.strip_prefix("?? ").is_some_and(safe_skill_path)),
            "unexpected test files: {dirty}"
        );
        assert_eq!(
            transfer_report(&tool, &root, bare.to_str().unwrap(), "main", "pull"),
            Ok(true)
        );
        assert!(fs::read_to_string(root.join(&path))
            .unwrap()
            .contains("remote"));
        crate::archive::restore_version(&root, temp.path(), &db, &id, &local, false).unwrap();
        assert!(fs::read_to_string(root.join(&path))
            .unwrap()
            .contains("local"));
    }

    #[test]
    fn dictionary_additions_are_combined_but_changed_existing_terms_are_rejected() {
        let base = r#"{"terms":[{"term":{"id":"base","meaning":"original"}}]}"#;
        let local =
            r#"{"terms":[{"term":{"id":"base","meaning":"original"}},{"term":{"id":"local"}}]}"#;
        let remote =
            r#"{"terms":[{"term":{"id":"base","meaning":"original"}},{"term":{"id":"remote"}}]}"#;
        let merged: serde_json::Value =
            serde_json::from_slice(&merge_added_terms(base, local, remote).unwrap()).unwrap();
        let ids: Vec<_> = merged["terms"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["term"]["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["base", "local", "remote"]);
        let changed = r#"{"terms":[{"term":{"id":"base","meaning":"changed"}}]}"#;
        assert!(merge_added_terms(base, local, changed).is_err());
    }

    #[test]
    fn people_merge_prefers_confirmed_and_reports_two_confirmed_versions() {
        let base = r#"{"people":[{"id":"one","name":"old","confirmed":false}]}"#;
        let left = r#"{"people":[{"id":"one","name":"local","confirmed":true},{"id":"two","name":"added","confirmed":true}]}"#;
        let right = r#"{"people":[{"id":"one","name":"remote","confirmed":false}]}"#;
        let (merged, conflict) = merge_people(base, left, right).unwrap();
        let data: serde_json::Value = serde_json::from_slice(&merged).unwrap();
        assert_eq!(data["people"][0]["name"], "local");
        assert_eq!(data["people"][1]["id"], "two");
        assert!(!conflict);
        let confirmed = r#"{"people":[{"id":"one","name":"remote","confirmed":true}]}"#;
        let (merged, conflict) = merge_people(base, left, confirmed).unwrap();
        let data: serde_json::Value = serde_json::from_slice(&merged).unwrap();
        assert_eq!(data["people"][0]["name"], "remote");
        assert!(conflict);
        assert!(merge_people(r#"{"people":["legacy"]}"#, left, right).is_err());
    }

    #[test]
    fn known_builtin_dictionary_versions_are_ordered_and_custom_is_not_trusted() {
        let old = include_str!("../bundled-dictionary/yoga-v1.json");
        let current = include_str!("../bundled-dictionary/yoga.json");
        assert!(
            crate::yoga_dictionary::known_bundle_version(old).unwrap()
                < crate::yoga_dictionary::known_bundle_version(current).unwrap()
        );
        assert!(crate::yoga_dictionary::known_bundle_version("{\"bundleVersion\":999}").is_none());
    }

    #[test]
    fn divergent_dictionary_additions_and_known_bundles_survive_real_git_merge() {
        let f = Fixture::new();
        let directory = f.root.join("dictionary");
        fs::create_dir(&directory).unwrap();
        let terms = |id: &str| {
            format!(
                r#"{{"terms":[{{"term":{{"id":"{id}","sanskrit":"test","korean":[],"koreanNames":[],"meaning":"test","sources":[]}},"baseMeaning":null}}]}}"#
            )
        };
        fs::write(directory.join("my-terms.json"), r#"{"terms":[]}"#).unwrap();
        fs::write(
            directory.join("people.json"),
            r#"{"people":[{"id":"person-1","name":"old","confirmed":false}]}"#,
        )
        .unwrap();
        fs::write(
            f.root.join("AGENTS.md"),
            include_str!("testdata/archive_rules_before_fragments.md"),
        )
        .unwrap();
        fs::write(
            directory.join("yoga.json"),
            include_str!("../bundled-dictionary/yoga-v1.json"),
        )
        .unwrap();
        git(&f.tool, &f.root, &["add", "--", "dictionary/", "AGENTS.md"]).unwrap();
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
                "base dictionary",
            ],
        )
        .unwrap();
        f.push();
        f.clone_and_commit();
        let other = f._dir.path().join("other");
        fs::write(other.join("AGENTS.md"), include_str!("archive_rules.md")).unwrap();
        fs::write(other.join("dictionary/my-terms.json"), terms("remote")).unwrap();
        fs::write(
            other.join("dictionary/people.json"),
            r#"{"people":[{"id":"person-1","name":"remote","confirmed":true}]}"#,
        )
        .unwrap();
        fs::write(
            other.join("dictionary/yoga.json"),
            include_str!("../bundled-dictionary/yoga.json"),
        )
        .unwrap();
        git(&f.tool, &other, &["add", "--", "dictionary/", "AGENTS.md"]).unwrap();
        git(
            &f.tool,
            &other,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "remote dictionary",
            ],
        )
        .unwrap();
        git(&f.tool, &other, &["push", "origin", "main"]).unwrap();
        fs::write(directory.join("my-terms.json"), terms("local")).unwrap();
        fs::write(
            directory.join("people.json"),
            r#"{"people":[{"id":"person-1","name":"local","confirmed":true}]}"#,
        )
        .unwrap();
        fs::write(
            f.root.join("AGENTS.md"),
            include_str!("testdata/archive_rules_before_dictionary.md"),
        )
        .unwrap();
        fs::write(
            directory.join("yoga.json"),
            include_str!("../bundled-dictionary/yoga-v2.json"),
        )
        .unwrap();
        git(&f.tool, &f.root, &["add", "--", "dictionary/", "AGENTS.md"]).unwrap();
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
                "local dictionary",
            ],
        )
        .unwrap();
        assert_eq!(
            transfer_report(&f.tool, &f.root, f.remote(), "main", "pull"),
            Ok(true)
        );
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("my-terms.json")).unwrap()).unwrap();
        let ids: Vec<_> = saved["terms"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["term"]["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["local", "remote"]);
        assert_eq!(
            fs::read_to_string(directory.join("yoga.json")).unwrap(),
            include_str!("../bundled-dictionary/yoga.json")
        );
        assert!(crate::yoga_dictionary::list(&f.root).is_ok());
        let people: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("people.json")).unwrap()).unwrap();
        assert_eq!(people["people"][0]["name"], "remote");
        let head = git(&f.tool, &f.root, &["rev-parse", "HEAD"]).unwrap();
        assert!(merged_people_conflict(&f.tool, &f.root, &head).unwrap());
        assert_eq!(
            fs::read_to_string(f.root.join("AGENTS.md")).unwrap(),
            include_str!("archive_rules.md")
        );
    }

    #[test]
    fn divergent_chat_snapshots_merge_through_bare_remote() {
        let f = Fixture::new();
        let first = f._dir.path().join("first.sqlite");
        let second = f._dir.path().join("second.sqlite");
        crate::storage::initialize(&first, &f._dir.path().join("first-backup"), "test").unwrap();
        crate::storage::initialize(&second, &f._dir.path().join("second-backup"), "test").unwrap();
        let id = serde_json::to_value(crate::chat::create(&first).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        crate::chat::rename(&first, &id, "shared chat").unwrap();
        let initial = crate::chat_archive_snapshot::snapshot(&first, &id).unwrap();
        crate::chat_archive_snapshot::restore_new(&second, &initial).unwrap();
        let file = format!("chats/{id}.json");
        fs::create_dir(f.root.join("chats")).unwrap();
        fs::write(f.root.join(&file), serde_json::to_vec(&initial).unwrap()).unwrap();
        git(&f.tool, &f.root, &["add", "--", &file]).unwrap();
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
                "base chat",
            ],
        )
        .unwrap();
        f.push();
        f.clone_and_commit();
        let other = f._dir.path().join("other");
        crate::chat::append(&first, &id, crate::chat::Role::User, "first", None, None).unwrap();
        crate::chat::append(&second, &id, crate::chat::Role::User, "second", None, None).unwrap();
        let left = crate::chat_archive_snapshot::snapshot(&first, &id).unwrap();
        let right = crate::chat_archive_snapshot::snapshot(&second, &id).unwrap();
        assert!(crate::chat_archive_snapshot::merge_snapshots(&left, &right).is_ok());
        fs::write(f.root.join(&file), serde_json::to_vec(&left).unwrap()).unwrap();
        git(&f.tool, &f.root, &["add", "--", &file]).unwrap();
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
                "first chat",
            ],
        )
        .unwrap();
        fs::write(other.join(&file), serde_json::to_vec(&right).unwrap()).unwrap();
        git(&f.tool, &other, &["add", "--", &file]).unwrap();
        git(
            &f.tool,
            &other,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "second chat",
            ],
        )
        .unwrap();
        git(&f.tool, &other, &["push", "origin", "main"]).unwrap();
        assert_eq!(
            transfer_report(&f.tool, &f.root, f.remote(), "main", "pull"),
            Ok(true)
        );
        crate::chat_archive_store::restore(&f.root, &first).unwrap();
        let restored = serde_json::to_value(crate::chat::read(&first, &id).unwrap()).unwrap();
        let messages = restored["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert!(messages.iter().any(|m| m["text"] == "first"));
        assert!(messages.iter().any(|m| m["text"] == "second"));
    }

    #[test]
    fn unresolved_profile_and_skill_keep_local_saves_and_retry() {
        let f = Fixture::new();
        fs::create_dir_all(f.root.join("skills/custom")).unwrap();
        for (path, text) in [("profile.md", "base"), ("skills/custom/SKILL.md", "base")] {
            fs::write(f.root.join(path), text).unwrap();
        }
        git(
            &f.tool,
            &f.root,
            &["add", "--", "profile.md", "skills/custom/SKILL.md"],
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
                "base",
            ],
        )
        .unwrap();
        f.push();
        f.clone_and_commit();
        let other = f._dir.path().join("other");
        for path in ["profile.md", "skills/custom/SKILL.md"] {
            fs::write(other.join(path), "remote").unwrap();
            fs::write(f.root.join(path), "local").unwrap();
        }
        for root in [&other, &f.root] {
            git(
                &f.tool,
                root,
                &["add", "--", "profile.md", "skills/custom/SKILL.md"],
            )
            .unwrap();
            git(
                &f.tool,
                root,
                &[
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "commit",
                    "-m",
                    "edit",
                ],
            )
            .unwrap();
        }
        git(&f.tool, &other, &["push", "origin", "main"]).unwrap();
        assert_eq!(
            transfer(&f.tool, &f.root, f.remote(), "main", "push"),
            Err("diverged")
        );
        let paths = conflicting_paths(&f.tool, &f.root).unwrap();
        assert!(paths.contains(&"profile.md".to_owned()));
        assert!(paths.contains(&"skills/custom/SKILL.md".to_owned()));
        f.local_commit();
        assert_eq!(
            fs::read_to_string(f.root.join("profile.md")).unwrap(),
            "local"
        );
        assert_eq!(
            transfer(&f.tool, &f.root, f.remote(), "main", "push"),
            Err("diverged")
        );
        for (index, path) in ["profile.md", "skills/custom/SKILL.md"].iter().enumerate() {
            let mut read = RemoteStatus::new("unavailable", None);
            let (ahead, behind) = relation(&f.tool, &f.root, f.remote(), "main").unwrap();
            apply_relation(&mut read, &f.tool, &f.root, ahead, behind);
            let serialized = serde_json::to_value(&read).unwrap();
            assert_eq!(serialized["reason"], "diverged");
            assert_eq!(
                serialized["conflictPaths"].as_array().unwrap().len(),
                2 - index
            );
            assert!(serialized["conflictPaths"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p == path));
            fs::write(f.root.join(path), "remote").unwrap();
            git(&f.tool, &f.root, &["add", "--", path]).unwrap();
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
                    "resolve",
                ],
            )
            .unwrap();
        }
        let (ahead, behind) = relation(&f.tool, &f.root, f.remote(), "main").unwrap();
        let mut read = RemoteStatus::new("unavailable", None);
        apply_relation(&mut read, &f.tool, &f.root, ahead, behind);
        assert_eq!(read.conflict_paths, Some(vec![]));
        transfer(&f.tool, &f.root, f.remote(), "main", "push").unwrap();
        assert_eq!(relation(&f.tool, &f.root, f.remote(), "main"), Ok((0, 0)));
        let mut read = RemoteStatus::new("unavailable", None);
        apply_relation(&mut read, &f.tool, &f.root, 0, 0);
        assert_eq!(read.reason, None);
        assert_eq!(read.conflict_paths, None);
    }

    #[test]
    fn prepared_merge_can_be_retried_after_remote_unavailable() {
        let f = Fixture::new();
        f.push();
        f.clone_and_commit();
        f.local_commit();
        assert_eq!(relation(&f.tool, &f.root, f.remote(), "main"), Ok((1, 1)));
        merge_disjoint(&f.tool, &f.root).unwrap();
        let missing = f._dir.path().join("no-such-remote.git");
        assert_eq!(
            transfer(&f.tool, &f.root, missing.to_str().unwrap(), "main", "push"),
            Err("remote_unavailable")
        );
        transfer(&f.tool, &f.root, f.remote(), "main", "push").unwrap();
        assert_eq!(
            fs::read_to_string(f.root.join("zettels/local.md")).unwrap(),
            "local"
        );
        assert_eq!(
            fs::read_to_string(f.root.join("zettels/b.md")).unwrap(),
            "remote"
        );
    }
    #[test]
    fn diverged_push_never_overwrites_unsaved_local_work() {
        let f = Fixture::new();
        f.push();
        f.clone_and_commit();
        f.local_commit();
        fs::write(f.root.join("zettels/a.md"), "unsaved").unwrap();
        assert_eq!(
            transfer(&f.tool, &f.root, f.remote(), "main", "push"),
            Err("unsaved_changes")
        );
        assert_eq!(
            fs::read_to_string(f.root.join("zettels/a.md")).unwrap(),
            "unsaved"
        );
    }

    #[test]
    fn disjoint_divergence_can_be_received_without_losing_local_work() {
        let f = Fixture::new();
        f.push();
        f.clone_and_commit();
        f.local_commit();
        assert_eq!(
            transfer_report(&f.tool, &f.root, f.remote(), "main", "pull"),
            Ok(true)
        );
        assert_eq!(
            fs::read_to_string(f.root.join("zettels/b.md")).unwrap(),
            "remote"
        );
        assert_eq!(
            fs::read_to_string(f.root.join("zettels/local.md")).unwrap(),
            "local"
        );
    }

    #[test]
    fn divergent_chat_files_are_not_merged_automatically() {
        let f = Fixture::new();
        f.push();
        f.clone_and_commit();
        fs::create_dir(f.root.join("chats")).unwrap();
        let path = "chats/chat_20261115_120000.json";
        fs::write(f.root.join(path), "{}").unwrap();
        git(&f.tool, &f.root, &["add", "--", path]).unwrap();
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
                "chat",
            ],
        )
        .unwrap();
        assert_eq!(
            transfer(&f.tool, &f.root, f.remote(), "main", "push"),
            Err("diverged")
        );
        assert_eq!(fs::read_to_string(f.root.join(path)).unwrap(), "{}");
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
