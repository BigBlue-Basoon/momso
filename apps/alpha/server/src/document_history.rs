//! Read saved document versions and restore without overwriting concurrent edits.
use super::*;
use std::process::{Command, Output, Stdio};

fn run(root: &std::path::Path, args: &[&str]) -> Result<Output, ApiError> {
    let git_dir = std::fs::symlink_metadata(root.join(".git"))
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "history_unavailable"))?;
    if !git_dir.file_type().is_dir() {
        return Err(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "history_unavailable",
        ));
    }
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "history_unavailable"))?;
    Ok(output)
}

fn valid_revision(revision: &str) -> bool {
    revision.len() == 40
        && revision
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn checked_document(id: &str) -> Result<String, ApiError> {
    if !momso_core::document::valid_zet_id(id) {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_document"));
    }
    Ok(format!("zettels/{id}.md"))
}

fn history(root: &std::path::Path, id: &str) -> Result<Vec<Value>, ApiError> {
    let path = checked_document(id)?;
    if !run(root, &["rev-parse", "--verify", "HEAD"])?
        .status
        .success()
    {
        return Ok(vec![]);
    }
    let output = run(
        root,
        &[
            "log",
            "--full-history",
            "-n",
            "100",
            "--format=%H%x09%cI",
            "--",
            &path,
        ],
    )?;
    if !output.status.success() {
        return Err(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "history_unavailable",
        ));
    }
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "history_unavailable"))?;
    text.lines()
        .map(|line| {
            let (revision, time) = line
                .split_once('\t')
                .ok_or_else(|| error(StatusCode::INTERNAL_SERVER_ERROR, "history_unavailable"))?;
            if !valid_revision(revision) || chrono::DateTime::parse_from_rfc3339(time).is_err() {
                return Err(error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "history_unavailable",
                ));
            }
            Ok(json!({"revision":revision,"time":time}))
        })
        .collect()
}

fn version(root: &std::path::Path, id: &str, revision: &str) -> Result<String, ApiError> {
    let path = checked_document(id)?;
    if !valid_revision(revision) {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_revision"));
    }
    if !run(root, &["merge-base", "--is-ancestor", revision, "HEAD"])?
        .status
        .success()
    {
        return Err(error(StatusCode::NOT_FOUND, "version_not_found"));
    }
    let tree = run(root, &["ls-tree", "--", revision, &path])?;
    if !tree.status.success() {
        return Err(error(StatusCode::NOT_FOUND, "version_not_found"));
    }
    let tree = std::str::from_utf8(&tree.stdout)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "history_unavailable"))?;
    if !(tree.starts_with("100644 blob ") || tree.starts_with("100755 blob ")) {
        return Err(error(StatusCode::NOT_FOUND, "version_not_found"));
    }
    let object = format!("{revision}:{path}");
    let size = run(root, &["cat-file", "-s", &object])?;
    let size = std::str::from_utf8(&size.stdout)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok());
    if !size.is_some_and(|n| n <= 1_048_576) {
        return Err(error(StatusCode::BAD_REQUEST, "version_too_large"));
    }
    let data = run(root, &["cat-file", "blob", &object])?;
    if !data.status.success() {
        return Err(error(StatusCode::NOT_FOUND, "version_not_found"));
    }
    let text = String::from_utf8(data.stdout)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "document_invalid"))?;
    let (head, _) = momso_core::document::parse(&text)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "document_invalid"))?;
    if head.id != id {
        return Err(error(StatusCode::INTERNAL_SERVER_ERROR, "document_invalid"));
    }
    Ok(text)
}

pub async fn list(
    State(app): State<App>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    let root = app.data.join("archive");
    let items = tokio::task::spawn_blocking(move || history(&root, &id))
        .await
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "history_unavailable"))??;
    Ok(Json(json!({"versions":items})))
}

pub async fn read(
    State(app): State<App>,
    Path((id, revision)): Path<(String, String)>,
    h: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    let root = app.data.join("archive");
    let text = tokio::task::spawn_blocking(move || version(&root, &id, &revision))
        .await
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "history_unavailable"))??;
    let (head, body) = momso_core::document::parse(&text)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "document_invalid"))?;
    Ok(Json(
        json!({"id":head.id,"title":head.title,"summary":head.summary,"folder":head.folder,"body":body,"assets":head.assets,"edges":head.edges}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Restore {
    revision: String,
    expected_revision: String,
}

pub async fn restore(
    State(app): State<App>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(input): Json<Restore>,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    let guard = WorkGuard::new(&app);
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let root = app.data.join("archive");
        let path = checked_document(&id)?;
        let status = run(
            &root,
            &[
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--",
                &path,
            ],
        )?;
        if !status.status.success() {
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "history_unavailable",
            ));
        }
        if !status.stdout.is_empty() {
            return Err(error(StatusCode::CONFLICT, "unsaved_changes"));
        }
        let text = version(&root, &id, &input.revision)?;
        let (mut head, body) = momso_core::document::parse(&text)
            .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "document_invalid"))?;
        head.updated_at = chrono::Local::now().to_rfc3339();
        momso_core::filesystem::update_document(&root, &head, body, &input.expected_revision)
            .map_err(document_write_error)?;
        let current =
            momso_core::filesystem::read_document(&root, &id).map_err(document_write_error)?;
        Ok(Json(
            json!({"id":id,"revision":momso_core::document::content_hash(&current)}),
        ))
    })
    .await
    .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "history_unavailable"))?
}
