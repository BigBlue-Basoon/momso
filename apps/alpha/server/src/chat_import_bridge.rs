//! Import an authenticated uploaded export without sending its contents to an AI.
use super::*;
use momso_server_attachments::Store;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

static PROGRESS: OnceLock<Mutex<HashMap<String, Value>>> = OnceLock::new();
fn progress_map() -> &'static Mutex<HashMap<String, Value>> {
    PROGRESS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn set_progress(id: &str, value: Value) {
    if let Ok(mut map) = progress_map().lock() {
        if let Some(entry) = map.get_mut(id) {
            *entry = value;
        }
    }
}
pub async fn progress(
    State(app): State<App>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    let map = progress_map().lock().map_err(|_| failed("storage_error"))?;
    map.get(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| failed("not_found"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportRequest {
    asset_id: String,
    provider: String,
    request_id: Option<String>,
}

fn copy_with_cancel(
    source: &mut impl Read,
    target: &mut impl Write,
    cancel: Option<&AtomicBool>,
) -> Result<(), &'static str> {
    let mut buffer = [0u8; 65536];
    loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
            return Err("cancelled");
        }
        let count = source
            .read(&mut buffer)
            .map_err(|_| "storage_unavailable")?;
        if count == 0 {
            return Ok(());
        }
        target
            .write_all(&buffer[..count])
            .map_err(|_| "storage_unavailable")?;
    }
}

fn input(
    app: &App,
    id: &str,
    cancel: Option<&AtomicBool>,
) -> Result<tempfile::NamedTempFile, &'static str> {
    if cancel.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
        return Err("cancelled");
    }
    let data = app.data.join("attachments");
    let metadata = std::fs::symlink_metadata(&data).map_err(|_| "storage_unavailable")?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err("unsafe_location");
    }
    let store = Store::open(&data, &app.data.join("archive"))?;
    let (asset, mut file) = store.open_visible(id)?;
    let ext = std::path::Path::new(&asset.original_file_name)
        .extension()
        .and_then(|v| v.to_str())
        .filter(|v| {
            ["zip", "json", "html"]
                .iter()
                .any(|a| v.eq_ignore_ascii_case(a))
        })
        .ok_or("unsupported_format")?
        .to_ascii_lowercase();
    let mut target = tempfile::Builder::new()
        .prefix(".chat-import-")
        .suffix(&format!(".{ext}"))
        .tempfile_in(&app.data)
        .map_err(|_| "storage_unavailable")?;
    copy_with_cancel(&mut file, target.as_file_mut(), cancel)?;
    target.flush().map_err(|_| "storage_unavailable")?;
    Ok(target)
}

fn failed(reason: &str) -> ApiError {
    let status = match reason {
        "invalid_format" | "unsupported_format" | "unsupported_provider" | "invalid_request_id" => {
            StatusCode::BAD_REQUEST
        }
        "request_busy" | "asset_trashed" => StatusCode::CONFLICT,
        "cancelled" => StatusCode::CONFLICT,
        "not_found" => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(
        status,
        match reason {
            "invalid_format" => "invalid_format",
            "unsupported_format" => "unsupported_format",
            "unsupported_provider" => "unsupported_provider",
            "invalid_request_id" => "invalid_request_id",
            "request_busy" => "request_busy",
            "asset_trashed" => "asset_trashed",
            "cancelled" => "cancelled",
            "not_found" => "not_found",
            "archive_limit" => "archive_limit",
            _ => "storage_error",
        },
    )
}

pub async fn preview(
    State(app): State<App>,
    h: HeaderMap,
    Json(body): Json<ImportRequest>,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    let _guard = ReadGuard::new(&app);
    tokio::task::spawn_blocking(move || {
        let file = input(&app, &body.asset_id, None).map_err(failed)?;
        let result = chat_export_import::preview(
            &app.data.join("server.sqlite3"),
            file.path(),
            &body.provider,
        )
        .map_err(|e| failed(&e))?;
        Ok(Json(json!(result)))
    })
    .await
    .map_err(|_| failed("storage_error"))?
}

pub async fn run(
    State(app): State<App>,
    h: HeaderMap,
    Json(body): Json<ImportRequest>,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    let guard = WorkGuard::new(&app);
    let reservation = body
        .request_id
        .as_deref()
        .map(chat_export_import::reserve)
        .transpose()
        .map_err(|e| failed(&e))?;
    if let Some(id) = &body.request_id {
        let mut map = progress_map().lock().map_err(|_| failed("storage_error"))?;
        if map.len() >= 128 {
            if let Some(old) = map
                .iter()
                .find(|(_, v)| v["state"] != "running")
                .map(|(k, _)| k.clone())
            {
                map.remove(&old);
            } else {
                return Err(failed("request_busy"));
            }
        }
        map.insert(
            id.clone(),
            json!({"requestId":id,"state":"running","processed":0,"total":null}),
        );
    }
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let cancel = reservation.as_ref().map(|(_, flag)| flag.as_ref());
        let result = (|| {
            let file = input(&app, &body.asset_id, cancel).map_err(|e| e.to_owned())?;
            let db = app.data.join("server.sqlite3");
            if let Some((_, flag)) = reservation.as_ref() {
                let preview = chat_export_import::preview_controlled(&db, file.path(), &body.provider, flag)?;
                let total = preview.conversations;
                let id = body.request_id.as_deref().ok_or("invalid_request_id")?;
                set_progress(id, json!({"requestId":id,"state":"running","processed":0,"total":total}));
                let result = chat_export_import::import_controlled(&db, file.path(), &body.provider, flag, &|n| {
                    set_progress(id, json!({"requestId":id,"state":"running","processed":n,"total":total}));
                })?;
                set_progress(id, json!({"requestId":id,"state":"completed","processed":total,"total":total}));
                Ok(result)
            } else {
                chat_export_import::import(&db, file.path(), &body.provider)
            }
        })();
        match result {
            Ok(result) => Ok(Json(json!(result))),
            Err(reason) => {
                if let Some(id) = &body.request_id { set_progress(id, json!({"requestId":id,"state":if reason == "cancelled" { "cancelled" } else { "failed" },"error":if reason == "cancelled" { "cancelled" } else { "import_failed" }})); }
                Err(failed(&reason))
            }
        }
    })
    .await
    .map_err(|_| failed("storage_error"))?
}

pub async fn cancel(
    State(app): State<App>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    chat_export_import::cancel(&id);
    Ok(Json(json!({"requestId":id})))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_stops_file_preparation_without_writing() {
        let flag = AtomicBool::new(true);
        let mut output = Vec::new();
        assert_eq!(
            copy_with_cancel(&mut &b"fixture"[..], &mut output, Some(&flag)),
            Err("cancelled")
        );
        assert!(output.is_empty());
    }

    #[test]
    fn cancellation_stops_preview_before_import() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("export.json");
        std::fs::write(&file, "[]").unwrap();
        let cancelled = AtomicBool::new(true);
        assert!(
            matches!(chat_export_import::preview_controlled(&temp.path().join("missing.sqlite"), &file, "claude", &cancelled), Err(reason) if reason == "cancelled")
        );
    }

    #[test]
    fn cancellation_rolls_back_imported_chats() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("server.sqlite3");
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection.execute_batch("CREATE TABLE chats(id TEXT PRIMARY KEY,title TEXT NOT NULL,created_at TEXT,updated_at TEXT); CREATE TABLE chat_messages(id TEXT PRIMARY KEY,chat_id TEXT NOT NULL,role TEXT NOT NULL,text TEXT NOT NULL,created_at TEXT NOT NULL,provider TEXT,model TEXT); CREATE TABLE chat_imports(chat_id TEXT PRIMARY KEY,source_provider TEXT NOT NULL,source_id TEXT NOT NULL,UNIQUE(source_provider,source_id));").unwrap();
        let file = temp.path().join("export.json");
        std::fs::write(&file,json!([{"uuid":"first","chat_messages":[{"sender":"human","text":"one"}]},{"uuid":"second","chat_messages":[{"sender":"human","text":"two"}]}]).to_string()).unwrap();
        let (_guard, token) = chat_export_import::reserve("cancel-fixture").unwrap();
        let result =
            chat_export_import::import_controlled(&db, &file, "claude", &token, &|count| {
                if count == 1 {
                    chat_export_import::cancel("cancel-fixture");
                }
            });
        assert!(matches!(result,Err(ref reason) if reason=="cancelled"));
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM chats", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
