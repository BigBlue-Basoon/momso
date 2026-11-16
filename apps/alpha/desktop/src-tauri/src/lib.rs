mod ai_import;
mod ai_models;
mod ai_runtime;
mod ai_usage;
mod app_updates;
mod approval;
mod archive;
mod archive_location;
mod archive_receipt;
mod archive_remote;
mod archive_zet;
mod backup;
mod backup_archive;
mod backup_profile;
mod backup_remote;
mod chat;
mod chat_claude;
mod chat_claude_items;
mod chat_codex;
mod chat_context;
mod chat_file_changes;
mod chat_group;
mod chat_handoff;
mod chat_items;
mod connections;
mod credentials;
mod dictionary;
mod media;
mod object_store;
mod object_store_restore;
mod object_store_state;
mod object_store_transfer;
mod operations;
mod permission_modes;
mod process;
mod provider_auth;
mod records;
mod runtime;
mod skills;
mod skills_write;
mod source_assets;
mod source_search;
mod storage;
mod text_paging;
mod usage_status;

use runtime::RuntimePaths;
use serde::Serialize;
use std::{collections::HashMap, sync::Arc};
use tauri::{Emitter, Manager, State};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};

#[tauri::command]
async fn app_state_restore() -> Result<(), &'static str> {
    Err("unavailable")
}

#[derive(Clone)]
struct AppState {
    paths: RuntimePaths,
    approvals: approval::ApprovalBroker,
    usage_monitor: Arc<std::sync::atomic::AtomicBool>,
    usage_cache: Arc<usage_status::UsageCache>,
    chat_active: chat_codex::Active,
    ai_cancellations: Arc<tokio::sync::Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
    operation_cancellations:
        Arc<tokio::sync::Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeInfo {
    app_version: String,
    platform: String,
    data_root: String,
    database_path: String,
    objects_path: String,
    exports_path: String,
    backups_path: String,
    logs_path: String,
    schema_version: u32,
    database_health: &'static str,
    wom_adapter_status: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupInfo {
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ZetReadResult {
    head: archive_zet::ZetHead,
    body: String,
    content_hash: String,
}

#[tauri::command]
async fn zet_restore_version(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    revision: String,
    discard_unsaved: Option<bool>,
) -> Result<(), String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    let database = state.paths.database_path.clone();
    let resources = app
        .path()
        .resource_dir()
        .map_err(|_| "몸소 폴더 경로를 확인하지 못했습니다".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = chat_file_changes::lock_app_write();
        chat_file_changes::app_write(&root, Some(&id));
        archive::restore_version(
            &root,
            &resources,
            &database,
            &id,
            &revision,
            discard_unsaved.unwrap_or(false),
        )
    })
    .await
    .map_err(|_| "이전 글을 복원하지 못했습니다".to_string())?
}

#[tauri::command]
async fn user_profile_get(state: State<'_, AppState>) -> Result<archive::UserProfile, String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    archive::profile_get(&root)
}

#[tauri::command]
async fn user_profile_set(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    name: String,
    about: String,
    style: String,
) -> Result<archive::UserProfile, String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    let resources = app
        .path()
        .resource_dir()
        .map_err(|_| "사용자 정보를 저장하지 못했습니다".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        archive::profile_set(&root, &resources, name, about, style)
    })
    .await
    .map_err(|_| "사용자 정보를 저장하지 못했습니다".to_string())?
}

#[tauri::command]
async fn archive_save(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    let database = state.paths.database_path.clone();
    let resources = app
        .path()
        .resource_dir()
        .map_err(|_| "몸소 폴더 경로를 확인하지 못했습니다".to_string())?;
    tauri::async_runtime::spawn_blocking(move || archive::save(&root, &resources, &database))
        .await
        .map_err(|_| "저장을 완료하지 못했습니다".to_string())?
}

#[tauri::command]
async fn archive_check(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<archive::ArchiveIssue>, String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    let resources = app
        .path()
        .resource_dir()
        .map_err(|_| "몸소 폴더 경로를 확인하지 못했습니다".to_string())?;
    tauri::async_runtime::spawn_blocking(move || archive::check(&root, &resources))
        .await
        .map_err(|_| "몸소 폴더를 검사하지 못했습니다".to_string())?
}

#[tauri::command]
async fn zet_history(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<archive::HistoryEntry>, String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    let resources = app
        .path()
        .resource_dir()
        .map_err(|_| "몸소 폴더 경로를 확인하지 못했습니다".to_string())?;
    tauri::async_runtime::spawn_blocking(move || archive::history(&root, &resources, &id))
        .await
        .map_err(|_| "이력을 읽지 못했습니다".to_string())?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ZetVersionResult {
    head: Option<archive_zet::ZetHead>,
    body: String,
    head_unreadable: bool,
}

#[tauri::command]
async fn zet_read_version(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    revision: String,
) -> Result<ZetVersionResult, String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    let resources = app
        .path()
        .resource_dir()
        .map_err(|_| "몸소 폴더 경로를 확인하지 못했습니다".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let text = archive::read_version(&root, &resources, &id, &revision)?;
        Ok(split_zet_version(text))
    })
    .await
    .map_err(|_| "이력을 읽지 못했습니다".to_string())?
}

fn split_zet_version(text: String) -> ZetVersionResult {
    match archive_zet::parse(&text) {
        Ok((head, body)) => ZetVersionResult {
            head: Some(head),
            body: body.to_string(),
            head_unreadable: false,
        },
        Err(_) => ZetVersionResult {
            head: None,
            body: text,
            head_unreadable: true,
        },
    }
}

#[cfg(test)]
#[test]
fn version_split_preserves_body_and_handles_invalid_header() {
    let valid = "---\nid: zet_20261115_143052\ntitle: 수업\nsummary: 요약\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets: []\nedges: []\n---\n본문\n";
    let parsed = split_zet_version(valid.into());
    assert_eq!(parsed.head.unwrap().title, "수업");
    assert_eq!(parsed.body, "본문\n");
    assert!(!parsed.head_unreadable);
    let invalid = split_zet_version("옛 문서".into());
    assert!(invalid.head.is_none() && invalid.head_unreadable);
    assert_eq!(invalid.body, "옛 문서");
}

#[tauri::command]
async fn archive_reindex(state: State<'_, AppState>) -> Result<archive::ReindexResult, String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    let database = state.paths.database_path.clone();
    tauri::async_runtime::spawn_blocking(move || archive::reindex(&root, &database))
        .await
        .map_err(|_| "색인을 다시 만들지 못했습니다".to_string())?
}

#[tauri::command]
async fn chat_attach_asset(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    chat_id: String,
    asset_id: String,
) -> Result<String, String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    let database = state.paths.database_path.clone();
    let event_chat = chat_id.clone();
    let receipt = tauri::async_runtime::spawn_blocking(move || {
        archive_receipt::attach_asset(&root, &database, &chat_id, &asset_id)
    })
    .await
    .map_err(|_| "원자료를 연결하지 못했습니다".to_string())??;
    let _ = app.emit(
        "chat-items-updated",
        serde_json::json!({"chatId":event_chat}),
    );
    Ok(receipt)
}

#[tauri::command]
async fn chat_items(
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<chat_items::ChatItems, String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    let database = state.paths.database_path.clone();
    tauri::async_runtime::spawn_blocking(move || chat_items::list(&root, &database, &chat_id))
        .await
        .map_err(|_| "unavailable".to_string())?
}

#[tauri::command]
async fn receipt_create(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    input: archive_receipt::ReceiptCreate,
) -> Result<String, String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    let database = state.paths.database_path.clone();
    let chat_id = input.chat.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        archive_receipt::create(&root, &database, input)
    })
    .await
    .map_err(|_| "작업 기록을 만들지 못했습니다".to_string())??;
    if let Some(chat_id) = chat_id {
        let _ = app.emit("chat-items-updated", serde_json::json!({"chatId":chat_id}));
    }
    Ok(result)
}

#[tauri::command]
async fn receipt_read(
    state: State<'_, AppState>,
    id: String,
) -> Result<archive_receipt::ReceiptReadResult, String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    let database = state.paths.database_path.clone();
    tauri::async_runtime::spawn_blocking(move || archive_receipt::read(&root, &database, &id))
        .await
        .map_err(|_| "작업 기록을 읽지 못했습니다".to_string())?
}

#[tauri::command]
async fn zet_linked(
    state: State<'_, AppState>,
    id: String,
) -> Result<archive_zet::ZetLinked, String> {
    let database = state.paths.database_path.clone();
    let root = state.paths.archive.current.clone().read_owned().await;
    tauri::async_runtime::spawn_blocking(move || archive_zet::linked(&root, &database, &id))
        .await
        .map_err(|_| "연결을 읽지 못했습니다".to_string())?
}

#[tauri::command]
async fn zet_update(
    state: State<'_, AppState>,
    id: String,
    changes: archive_zet::ZetUpdate,
    expected_hash: Option<String>,
) -> Result<(), String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    let database = state.paths.database_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = chat_file_changes::lock_app_write();
        chat_file_changes::app_write(&root, Some(&id));
        archive_zet::update_checked(&root, &database, &id, changes, expected_hash.as_deref())
    })
    .await
    .map_err(|_| "글을 고치지 못했습니다".to_string())?
}

#[tauri::command]
async fn zet_save(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    changes: archive_zet::ZetUpdate,
    expected_hash: Option<String>,
) -> Result<(), String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    let database = state.paths.database_path.clone();
    let resources = app
        .path()
        .resource_dir()
        .map_err(|_| "몸소 폴더 경로를 확인하지 못했습니다".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = chat_file_changes::lock_app_write();
        chat_file_changes::app_write(&root, Some(&id));
        archive::save_one(
            &root,
            &resources,
            &database,
            &id,
            changes,
            expected_hash.as_deref(),
        )
    })
    .await
    .map_err(|_| "글을 저장하지 못했습니다".to_string())?
}

#[tauri::command]
async fn zet_list(
    state: State<'_, AppState>,
    kind: Option<String>,
    status: Option<String>,
) -> Result<Vec<archive_zet::ZetListEntry>, String> {
    let database = state.paths.database_path.clone();
    tauri::async_runtime::spawn_blocking(move || archive_zet::list(&database, kind, status))
        .await
        .map_err(|_| "글 목록을 읽지 못했습니다".to_string())?
}

#[tauri::command]
async fn zet_create(
    state: State<'_, AppState>,
    input: archive_zet::ZetCreate,
) -> Result<String, String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    let database = state.paths.database_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = chat_file_changes::lock_app_write();
        let result = archive_zet::create(&root, &database, input);
        if let Ok(id) = &result {
            chat_file_changes::app_write(&root, Some(id));
        }
        result
    })
    .await
    .map_err(|_| "글을 만들지 못했습니다".to_string())?
}

#[tauri::command]
async fn zet_read(state: State<'_, AppState>, id: String) -> Result<ZetReadResult, String> {
    if !archive_zet::valid_zet_id(&id) {
        return Err("글 번호가 올바르지 않습니다".into());
    }
    let root = state.paths.archive.current.clone().read_owned().await;
    let path = root.join("zettels").join(format!("{id}.md"));
    let text = std::fs::read_to_string(path).map_err(|_| "글을 읽지 못했습니다".to_string())?;
    let (head, body) = archive_zet::parse(&text)?;
    if head.id != id {
        return Err("글 번호와 파일 이름이 다릅니다".into());
    }
    Ok(ZetReadResult {
        head,
        body: body.to_string(),
        content_hash: archive_zet::content_hash(&text),
    })
}

#[tauri::command]
async fn archive_status(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<archive::ArchiveStatus, String> {
    let root = state.paths.archive.current.clone().read_owned().await;
    let resources = app
        .path()
        .resource_dir()
        .map_err(|_| "몸소 폴더 경로를 확인하지 못했습니다".to_string())?;
    tauri::async_runtime::spawn_blocking(move || archive::status(&root, &resources))
        .await
        .map_err(|_| "몸소 폴더 상태를 확인하지 못했습니다".to_string())?
}

#[tauri::command]
async fn discover_local_ai_conversations(
    provider: String,
) -> Result<Vec<ai_import::AiConversationPreview>, String> {
    tauri::async_runtime::spawn_blocking(move || ai_import::discover_local(&provider))
        .await
        .map_err(|error| format!("로컬 AI 채팅방 검색 작업에 실패했습니다: {error}"))?
}

#[tauri::command]
async fn load_local_ai_conversations(
    provider: String,
    source_locators: Vec<String>,
) -> Result<Vec<ai_import::AiConversation>, String> {
    tauri::async_runtime::spawn_blocking(move || ai_import::load_local(&provider, &source_locators))
        .await
        .map_err(|error| format!("로컬 AI 채팅방 가져오기 작업에 실패했습니다: {error}"))?
}

#[tauri::command]
async fn runtime_info(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<RuntimeInfo, String> {
    let paths = state.paths.scoped().await;
    let database_path = paths.database_path.clone();
    let app_version = app.package_info().version.to_string();
    let storage_status = tauri::async_runtime::spawn_blocking({
        let app_version = app_version.clone();
        move || storage::inspect_and_touch(&database_path, &app_version)
    })
    .await
    .map_err(|error| format!("Local storage task failed: {error}"))?
    .map_err(|error| error.to_string())?;

    Ok(RuntimeInfo {
        app_version,
        platform: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        data_root: paths.data_root.to_string_lossy().into_owned(),
        database_path: paths.database_path.to_string_lossy().into_owned(),
        objects_path: paths.objects_path.to_string_lossy().into_owned(),
        exports_path: paths.exports_path.to_string_lossy().into_owned(),
        backups_path: paths.backups_path.to_string_lossy().into_owned(),
        logs_path: paths.logs_path.to_string_lossy().into_owned(),
        schema_version: storage_status.schema_version,
        database_health: storage_status.health,
        wom_adapter_status: "waiting_for_stable_contract",
    })
}

#[tauri::command]
async fn create_data_backup(state: State<'_, AppState>) -> Result<BackupInfo, String> {
    let lease = state.paths.archive.current.clone().read_owned().await;
    let database_path = state.paths.database_path.clone();
    let backups_path = state.paths.backups_path.clone();
    let backup_path = tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        storage::create_manual_backup(&database_path, &backups_path)
    })
    .await
    .map_err(|error| format!("Local backup task failed: {error}"))?
    .map_err(|error| error.to_string())?;

    log::info!("Manual local backup completed");
    Ok(BackupInfo {
        path: backup_path.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
async fn connection_overview() -> Result<connections::ConnectionOverview, String> {
    connections::overview()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn connect_external_provider(provider: String) -> Result<connections::ToolStatus, String> {
    connections::connect(&provider)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn install_external_provider(provider: String) -> Result<connections::ToolStatus, String> {
    connections::install(&provider)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn save_ai_api_key(provider: String, api_key: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || connections::save_api_key(&provider, &api_key))
        .await
        .map_err(|error| format!("API 키 저장 작업에 실패했습니다: {error}"))?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn delete_ai_api_key(provider: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || connections::delete_api_key(&provider))
        .await
        .map_err(|error| format!("API 키 삭제 작업에 실패했습니다: {error}"))?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn list_transcription_terms(
    state: State<'_, AppState>,
) -> Result<Vec<dictionary::TranscriptionTerm>, String> {
    let database_path = state.paths.database_path.clone();
    tauri::async_runtime::spawn_blocking(move || dictionary::list(&database_path))
        .await
        .map_err(|error| format!("전사 보정 사전 조회 작업에 실패했습니다: {error}"))?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn create_transcription_term(
    state: State<'_, AppState>,
    input: dictionary::CreateTranscriptionTermInput,
) -> Result<dictionary::TranscriptionTerm, String> {
    let database_path = state.paths.database_path.clone();
    let lease = state.paths.archive.current.clone().read_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        dictionary::create(&database_path, input)
    })
    .await
    .map_err(|error| format!("전사 보정 규칙 저장 작업에 실패했습니다: {error}"))?
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn set_transcription_term_enabled(
    state: State<'_, AppState>,
    term_id: String,
    enabled: bool,
) -> Result<dictionary::TranscriptionTerm, String> {
    let database_path = state.paths.database_path.clone();
    let lease = state.paths.archive.current.clone().read_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        dictionary::set_enabled(&database_path, &term_id, enabled)
    })
    .await
    .map_err(|error| format!("전사 보정 규칙 변경 작업에 실패했습니다: {error}"))?
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn delete_transcription_term(
    state: State<'_, AppState>,
    term_id: String,
) -> Result<(), String> {
    let database_path = state.paths.database_path.clone();
    let lease = state.paths.archive.current.clone().read_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        dictionary::delete(&database_path, &term_id)
    })
    .await
    .map_err(|error| format!("전사 보정 규칙 삭제 작업에 실패했습니다: {error}"))?
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn user_owned_backup_overview(
    state: State<'_, AppState>,
) -> Result<backup::BackupOverview, String> {
    let paths = state.paths.scoped().await;
    backup::overview(&paths)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn user_owned_backup_evidence(
    state: State<'_, AppState>,
) -> Result<backup_profile::BackupEvidenceReport, String> {
    let database_path = state.paths.database_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        backup_profile::local_backup_evidence(&database_path)
    })
    .await
    .map_err(|error| format!("Local backup evidence task failed: {error}"))?
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn prepare_user_owned_backup(
    state: State<'_, AppState>,
    selected_neon_organization_id: Option<String>,
) -> Result<backup::BackupOverview, String> {
    let paths = state.paths.scoped().await;
    backup::prepare_resources(&paths, selected_neon_organization_id.as_deref())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn discover_user_owned_backups() -> Result<Vec<backup_remote::BackupCandidate>, String> {
    backup::discover_existing()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn adopt_user_owned_backup(
    state: State<'_, AppState>,
    repository: String,
) -> Result<backup::BackupOverview, String> {
    let paths = state.paths.scoped().await;
    backup::adopt_existing(&paths, &repository)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn run_user_owned_backup(state: State<'_, AppState>) -> Result<backup::BackupResult, String> {
    let paths = state.paths.scoped().await;
    backup::run_backup(&paths)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn start_user_owned_backup(
    state: State<'_, AppState>,
) -> Result<operations::OperationStatus, String> {
    let paths = state.paths.scoped().await;
    let database_path = paths.database_path.clone();
    let started = tauri::async_runtime::spawn_blocking({
        let database_path = database_path.clone();
        move || operations::start(&database_path, "user_owned_backup", "queued", 5)
    })
    .await
    .map_err(|error| format!("Backup operation start failed: {error}"))?
    .map_err(|error| error.to_string())?;
    let operation_id = started.id.clone();
    let cancellation = tokio_util::sync::CancellationToken::new();
    state
        .operation_cancellations
        .lock()
        .await
        .insert(operation_id.clone(), cancellation.clone());
    let cancellations = state.operation_cancellations.clone();

    tauri::async_runtime::spawn(async move {
        let progress_path = database_path.clone();
        let progress_id = operation_id.clone();
        let result = backup::run_backup_cancellable(
            &paths,
            cancellation.clone(),
            move |stage, completed, total| {
                if let Err(error) =
                    operations::progress(&progress_path, &progress_id, stage, completed, total)
                {
                    log::warn!("Backup progress persistence failed: {error}");
                }
            },
        )
        .await;
        match result {
            Ok(backup_result) => {
                let _ = operations::succeed(
                    &database_path,
                    &operation_id,
                    "complete",
                    5,
                    5,
                    &backup_result,
                );
            }
            Err(error) if cancellation.is_cancelled() => {
                log::info!("User-owned backup cancelled: {error}");
                let _ = operations::finish_without_result(
                    &database_path,
                    &operation_id,
                    operations::OperationState::Cancelled,
                    "cancelled",
                    "user_cancelled",
                );
            }
            Err(error) => {
                log::warn!("User-owned backup failed: {error}");
                let _ = operations::finish_without_result(
                    &database_path,
                    &operation_id,
                    operations::OperationState::Failed,
                    "failed",
                    "backup_failed",
                );
            }
        }
        cancellations.lock().await.remove(&operation_id);
    });

    Ok(started)
}

#[tauri::command]
async fn plan_user_owned_restore(
    state: State<'_, AppState>,
) -> Result<backup::RestorePlan, String> {
    let paths = state.paths.scoped().await;
    backup::plan_restore(&paths)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn apply_user_owned_restore(
    state: State<'_, AppState>,
    plan_digest: String,
    confirm_overwrite: bool,
) -> Result<backup::RestoreResult, String> {
    let paths = state.paths.scoped().await;
    backup::apply_restore(&paths, &plan_digest, confirm_overwrite)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn start_user_owned_restore(
    state: State<'_, AppState>,
    plan_digest: String,
    confirm_overwrite: bool,
) -> Result<operations::OperationStatus, String> {
    let paths = state.paths.scoped().await;
    let database_path = paths.database_path.clone();
    let started = tauri::async_runtime::spawn_blocking({
        let database_path = database_path.clone();
        move || operations::start(&database_path, "user_owned_restore", "queued", 5)
    })
    .await
    .map_err(|error| format!("Restore operation start failed: {error}"))?
    .map_err(|error| error.to_string())?;
    let operation_id = started.id.clone();
    let cancellation = tokio_util::sync::CancellationToken::new();
    state
        .operation_cancellations
        .lock()
        .await
        .insert(operation_id.clone(), cancellation.clone());
    let cancellations = state.operation_cancellations.clone();

    tauri::async_runtime::spawn(async move {
        let progress_path = database_path.clone();
        let progress_id = operation_id.clone();
        let result = backup::apply_restore_cancellable(
            &paths,
            &plan_digest,
            confirm_overwrite,
            cancellation.clone(),
            move |stage, completed, total| {
                if let Err(error) =
                    operations::progress(&progress_path, &progress_id, stage, completed, total)
                {
                    log::warn!("Restore progress persistence failed: {error}");
                }
            },
            Some(&operation_id),
        )
        .await;
        match result {
            Ok(restore_result) => {
                let _ = operations::succeed(
                    &database_path,
                    &operation_id,
                    "complete",
                    5,
                    5,
                    &restore_result,
                );
            }
            Err(error) if cancellation.is_cancelled() => {
                log::info!("User-owned restore cancelled: {error}");
                let _ = operations::finish_without_result(
                    &database_path,
                    &operation_id,
                    operations::OperationState::Cancelled,
                    "cancelled",
                    "user_cancelled",
                );
            }
            Err(error) => {
                log::warn!("User-owned restore failed: {error}");
                let _ = operations::finish_without_result(
                    &database_path,
                    &operation_id,
                    operations::OperationState::Failed,
                    "failed",
                    "restore_failed",
                );
            }
        }
        cancellations.lock().await.remove(&operation_id);
    });

    Ok(started)
}

#[tauri::command]
async fn export_backup_recovery_key(state: State<'_, AppState>) -> Result<String, String> {
    let paths = state.paths.scoped().await;
    tauri::async_runtime::spawn_blocking(move || backup::export_recovery_key(&paths))
        .await
        .map_err(|error| format!("복구 키 조회 작업에 실패했습니다: {error}"))?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn import_backup_recovery_key(
    state: State<'_, AppState>,
    recovery_key: String,
) -> Result<(), String> {
    let paths = state.paths.scoped().await;
    tauri::async_runtime::spawn_blocking(move || backup::import_recovery_key(&paths, &recovery_key))
        .await
        .map_err(|error| format!("복구 키 저장 작업에 실패했습니다: {error}"))?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn confirm_backup_recovery_key(
    state: State<'_, AppState>,
    recovery_key: String,
) -> Result<(), String> {
    let paths = state.paths.scoped().await;
    tauri::async_runtime::spawn_blocking(move || {
        backup::confirm_recovery_key(&paths, &recovery_key)
    })
    .await
    .map_err(|error| format!("복구 키 확인 작업에 실패했습니다: {error}"))?
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn usage_status(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<usage_status::UsageStatus>, String> {
    let archive = Arc::new(state.paths.archive.current.clone().read_owned().await);
    usage_status::start_monitor(app.clone(), state.usage_monitor.clone());
    Ok(usage_status::cached(
        app,
        state.usage_cache.clone(),
        archive,
        None,
    ))
}

#[tauri::command]
async fn usage_refresh(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    provider: String,
) -> Result<Vec<usage_status::UsageStatus>, String> {
    if !["codex", "claude", "gemini"].contains(&provider.as_str()) {
        return Err("AI를 확인할 수 없습니다".into());
    }
    let archive = Arc::new(state.paths.archive.current.clone().read_owned().await);
    Ok(usage_status::cached(
        app,
        state.usage_cache.clone(),
        archive,
        Some(&provider),
    ))
}

#[tauri::command]
async fn ai_models(state: State<'_, AppState>) -> Result<Vec<ai_models::ProviderModels>, String> {
    let archive = state.paths.archive.current.clone().read_owned().await;
    Ok(ai_models::all(&archive).await)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn chat_send(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    chat_id: String,
    text: String,
    provider: String,
    model: Option<String>,
    effort: Option<String>,
    skill: Option<String>,
    fast: Option<bool>,
) -> Result<serde_json::Value, String> {
    if !chat_codex::supported(&provider) && provider != "claude" {
        return Ok(serde_json::json!({"status":"unavailable"}));
    }
    if provider == "claude" && !chat_claude::connected().await {
        return Ok(serde_json::json!({"status":"not_connected"}));
    }
    let archive = state.paths.archive.current.clone().read_owned().await;
    let skill = skill.filter(|name| !name.is_empty());
    if let Some(name) = skill.as_deref() {
        match skills::callable(&archive, name) {
            Ok(true) => (),
            Ok(false) | Err("not_found") => {
                return Ok(serde_json::json!({"status":"skill_unavailable"}))
            }
            Err(error) => return Err(error.into()),
        }
    }
    let db = state.paths.database_path.clone();
    let permission_mode = permission_modes::get(
        &db,
        if provider == "claude" {
            permission_modes::Provider::Claude
        } else {
            permission_modes::Provider::Codex
        },
    )?;
    let settings = if provider == "codex" {
        Some(
            permission_modes::codex_settings(&permission_mode)
                .ok_or("권한 설정을 읽지 못했습니다")?,
        )
    } else {
        None
    };
    if provider == "claude" && fast == Some(true) {
        match chat_claude::probe_fast(
            &archive,
            model.as_deref(),
            effort.as_deref(),
            &permission_mode,
        )
        .await
        {
            chat_claude::FastProbe::Unavailable => {
                return Ok(serde_json::json!({"status":"fast_unavailable"}))
            }
            chat_claude::FastProbe::Failed => {
                return Ok(serde_json::json!({"status":"unavailable"}))
            }
            chat_claude::FastProbe::Available | chat_claude::FastProbe::Unknown => {}
        }
    }
    let mut active = state.chat_active.lock().await;
    if active.contains_key(&chat_id) {
        return Err("이 채팅방에서 이미 응답을 기다리고 있습니다".into());
    }
    let has_session = if provider == "claude" {
        chat_claude::previous(&db, &chat_id)?.is_some()
    } else {
        chat::codex_thread(&db, &chat_id)?.is_some()
    };
    let retry_id = chat::find_fast_retry(&db, &chat_id, &text, skill.as_deref())?;
    let handoff = chat_handoff::prepare_excluding(
        &db,
        &chat_id,
        &provider,
        has_session,
        &text,
        retry_id.as_deref(),
    )?;
    let fallback = if has_session {
        Some(chat_handoff::prepare_excluding(
            &db,
            &chat_id,
            &provider,
            false,
            &text,
            retry_id.as_deref(),
        )?)
    } else {
        None
    };
    let provider_text = handoff.prompt;
    let not_handed_over = handoff.not_handed_over;
    let user_message_id = if let Some(id) = retry_id {
        id
    } else {
        chat::append_full(
            &db,
            &chat_id,
            chat::Role::User,
            &text,
            None,
            None,
            None,
            skill.clone(),
        )?
        .id
    };
    if let Some(context) = chat_context::switch_provider(&db, &chat_id, &provider)? {
        let _ = app.emit(
            "chat-context-updated",
            serde_json::json!({"chatId":chat_id,"context":context}),
        );
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    let tracking = chat_file_changes::Tracking::start(&archive, &chat_id);
    active.insert(chat_id.clone(), cancel.clone());
    drop(active);
    let active = state.chat_active.clone();
    let approval_app = app.clone();
    let approval_broker = state.approvals.clone();
    let approvals: chat_codex::ApprovalChannel = Box::new(move |request, cancel| {
        let app = approval_app.clone();
        let broker = approval_broker.clone();
        Box::pin(async move { broker.request(&app, request, cancel).await })
    });
    let model = chat_codex::valid_model(model);
    tauri::async_runtime::spawn(async move {
        let snapshot_root = archive.to_path_buf();
        let before = match tauri::async_runtime::spawn_blocking(move || {
            chat_file_changes::capture(&snapshot_root)
        })
        .await
        {
            Ok(Ok(snapshot)) => snapshot,
            _ => {
                active.lock().await.remove(&chat_id);
                let _ = app.emit(
                    "chat-error",
                    chat_handoff::with_omitted_count(
                        chat_codex::error_payload(&chat_id, "몸소 폴더를 확인하지 못했습니다"),
                        not_handed_over,
                    ),
                );
                return;
            }
        };
        let mut done = None;
        let mut observed_reads = std::collections::BTreeSet::new();
        let mut observed_writes = std::collections::BTreeSet::new();
        let mut observed_model: Option<String> = None;
        let mut used_fallback = false;
        let mut emit = |name: &str, payload: serde_json::Value| {
            if name == "chat-done" {
                done = Some(payload);
            } else if name == "momso-internal-file-writes" {
                if let Some(items) = payload.as_array() {
                    observed_writes.extend(
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_owned)),
                    );
                }
            } else if name == "momso-internal-observed-model" {
                observed_model = payload.as_str().map(str::to_owned);
            } else if name == "momso-internal-handoff-fallback" {
                used_fallback = true;
            } else if name == "momso-internal-file-reads" {
                if let Some(items) = payload.as_array() {
                    observed_reads.extend(
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_owned)),
                    );
                }
            } else {
                let _ = app.emit(name, payload);
            }
        };
        let outcome = if provider == "claude" {
            chat_claude::run_with_skill(
                &db,
                &archive,
                &chat_id,
                &provider_text,
                model.as_deref(),
                effort.as_deref(),
                &permission_mode,
                fast.unwrap_or(false),
                skill.as_deref(),
                fallback.as_ref().map(|value| value.prompt.as_str()),
                Some(approvals),
                cancel,
                &mut emit,
            )
            .await
        } else {
            chat_codex::run_with_skill(
                &db,
                &archive,
                &chat_id,
                &provider_text,
                model.as_deref(),
                effort.as_deref(),
                skill.as_deref(),
                fast,
                fallback.as_ref().map(|value| value.prompt.as_str()),
                settings.as_ref().expect("valid Codex permission"),
                Some(approvals),
                cancel,
                &mut emit,
            )
            .await
        };
        let snapshot_root = archive.to_path_buf();
        let after = tauri::async_runtime::spawn_blocking(move || {
            chat_file_changes::capture(&snapshot_root)
        })
        .await;
        if let Ok(Ok(after)) = after {
            let completed_model = done
                .as_ref()
                .and_then(|payload| payload.get("messageId"))
                .and_then(serde_json::Value::as_str)
                .and_then(|id| {
                    rusqlite::Connection::open(&db)
                        .ok()?
                        .query_row(
                            "SELECT model FROM chat_messages WHERE id=?1 AND chat_id=?2",
                            rusqlite::params![id, chat_id],
                            |r| r.get::<_, Option<String>>(0),
                        )
                        .ok()
                        .flatten()
                });
            let actual_model = chat_file_changes::receipt_model(
                completed_model.as_deref(),
                observed_model.as_deref(),
            );
            let receipt = archive_receipt::ReceiptCreate {
                tool: provider.clone(),
                model: actual_model,
                chat: Some(chat_id.clone()),
                read: observed_reads.into_iter().collect(),
                wrote: vec![],
                prompt: text.clone(),
                changes: String::new(),
            };
            match chat_file_changes::record_tracked(
                &archive,
                &db,
                receipt,
                &before,
                &after,
                &observed_writes,
                tracking,
            ) {
                Ok(true) => {
                    let _ = app.emit("chat-items-updated", serde_json::json!({"chatId":chat_id}));
                }
                Ok(false) => {}
                Err(error) => log::warn!("Could not save file change record: {error}"),
            }
        } else {
            log::warn!("Could not compare file changes");
        }
        let omitted = if used_fallback {
            fallback
                .as_ref()
                .map_or(not_handed_over, |value| value.not_handed_over)
        } else {
            not_handed_over
        };
        match outcome {
            Ok(()) => {
                if let Some(mut payload) = done {
                    if let Some(message_id) =
                        payload.get("messageId").and_then(serde_json::Value::as_str)
                    {
                        let actual_fast =
                            match payload.get("fastState").and_then(serde_json::Value::as_str) {
                                Some("on") => Some(true),
                                Some("off") => Some(false),
                                _ => None,
                            };
                        let saved = chat_codex::finish_active(&active, &chat_id, || {
                            chat::set_delivery_metadata(
                                &db,
                                &chat_id,
                                message_id,
                                actual_fast,
                                omitted,
                            )?;
                            if let Err(error) =
                                chat_handoff::mark_delivered(&db, &chat_id, &provider, message_id)
                            {
                                eprintln!("Failed to record chat delivery: {error}");
                            }
                            Ok::<(), String>(())
                        })
                        .await;
                        if let Err(error) = saved {
                            let _ = app.emit(
                                "chat-error",
                                chat_handoff::with_omitted_count(
                                    chat_codex::error_payload(&chat_id, &error),
                                    omitted,
                                ),
                            );
                            return;
                        }
                    } else {
                        active.lock().await.remove(&chat_id);
                    }
                    payload["notHandedOver"] = serde_json::json!(omitted);
                    let _ = app.emit("chat-items-updated", serde_json::json!({"chatId":chat_id}));
                    let _ = app.emit("chat-done", payload);
                } else {
                    active.lock().await.remove(&chat_id);
                }
            }
            Err(error) => {
                let marked = if fast == Some(true) {
                    chat::mark_fast_retry(&db, &chat_id, &user_message_id)
                } else {
                    Ok(())
                };
                let payload = if let Err(error) = marked {
                    chat_codex::error_payload(&chat_id, &error)
                } else if error == "__momso_fast_unavailable" {
                    serde_json::json!({"chatId":chat_id,"status":"fast_unavailable","reason":""})
                } else {
                    chat_codex::error_payload(&chat_id, &error)
                };
                active.lock().await.remove(&chat_id);
                let _ = app.emit(
                    "chat-error",
                    chat_handoff::with_omitted_count(payload, omitted),
                );
            }
        }
    });
    Ok(serde_json::json!({"status":"started"}))
}

#[tauri::command]
async fn chat_cancel(state: State<'_, AppState>, chat_id: String) -> Result<(), String> {
    if let Some(token) = state.chat_active.lock().await.get(&chat_id) {
        token.cancel();
    }
    Ok(())
}

#[tauri::command]
async fn chat_group_list(state: State<'_, AppState>) -> Result<Vec<chat_group::ChatGroup>, String> {
    let path = state.paths.database_path.clone();
    tauri::async_runtime::spawn_blocking(move || chat_group::list(&path))
        .await
        .map_err(|_| "채팅방 묶음을 읽지 못했습니다".to_string())?
}
#[tauri::command]
async fn chat_group_create(
    state: State<'_, AppState>,
    name: String,
) -> Result<chat_group::ChatGroup, String> {
    let path = state.paths.database_path.clone();
    let lease = state.paths.archive.current.clone().read_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        chat_group::create(&path, &name)
    })
    .await
    .map_err(|_| "묶음을 만들지 못했습니다".to_string())?
}
#[tauri::command]
async fn chat_group_rename(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> Result<(), String> {
    let path = state.paths.database_path.clone();
    let lease = state.paths.archive.current.clone().read_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        chat_group::rename(&path, &id, &name)
    })
    .await
    .map_err(|_| "묶음 이름을 바꾸지 못했습니다".to_string())?
}
#[tauri::command]
async fn chat_group_reorder(state: State<'_, AppState>, ids: Vec<String>) -> Result<(), String> {
    let path = state.paths.database_path.clone();
    let lease = state.paths.archive.current.clone().read_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        chat_group::reorder(&path, &ids)
    })
    .await
    .map_err(|_| "묶음 순서를 바꾸지 못했습니다".to_string())?
}
/// 묶음만 없애고 그 안의 채팅방은 묶이지 않은 상태로 남긴다.
#[tauri::command]
async fn chat_group_remove(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let path = state.paths.database_path.clone();
    let lease = state.paths.archive.current.clone().read_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        chat_group::remove(&path, &id)
    })
    .await
    .map_err(|_| "묶음을 없애지 못했습니다".to_string())?
}
#[tauri::command]
async fn chat_set_group(
    state: State<'_, AppState>,
    chat_id: String,
    group_id: Option<String>,
) -> Result<(), String> {
    let path = state.paths.database_path.clone();
    let lease = state.paths.archive.current.clone().read_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        chat_group::set_group(&path, &chat_id, group_id)
    })
    .await
    .map_err(|_| "채팅방 묶음을 바꾸지 못했습니다".to_string())?
}
#[tauri::command]
async fn chat_context(
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<chat_context::Context, String> {
    let db = state.paths.database_path.clone();
    tauri::async_runtime::spawn_blocking(move || chat_context::get(&db, &chat_id))
        .await
        .map_err(|_| "채팅방 정보를 읽지 못했습니다".to_string())?
}
#[tauri::command]
async fn chat_list(state: State<'_, AppState>) -> Result<Vec<chat::ChatSummary>, String> {
    let path = state.paths.database_path.clone();
    tauri::async_runtime::spawn_blocking(move || chat::list(&path))
        .await
        .map_err(|_| "채팅방 목록을 읽지 못했습니다".to_string())?
}
#[tauri::command]
async fn chat_create(state: State<'_, AppState>) -> Result<chat::ChatSummary, String> {
    let path = state.paths.database_path.clone();
    let lease = state.paths.archive.current.clone().read_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        chat::create(&path)
    })
    .await
    .map_err(|_| "채팅방을 만들지 못했습니다".to_string())?
}
#[tauri::command]
async fn chat_read(state: State<'_, AppState>, id: String) -> Result<chat::ChatDetail, String> {
    let path = state.paths.database_path.clone();
    tauri::async_runtime::spawn_blocking(move || chat::read(&path, &id))
        .await
        .map_err(|_| "채팅방을 읽지 못했습니다".to_string())?
}
#[tauri::command]
async fn chat_append(
    state: State<'_, AppState>,
    id: String,
    role: chat::Role,
    text: String,
    provider: Option<String>,
    model: Option<String>,
) -> Result<chat::ChatMessage, String> {
    let path = state.paths.database_path.clone();
    let lease = state.paths.archive.current.clone().read_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        chat::append(&path, &id, role, &text, provider, model)
    })
    .await
    .map_err(|_| "말을 저장하지 못했습니다".to_string())?
}
#[tauri::command]
async fn chat_rename(state: State<'_, AppState>, id: String, title: String) -> Result<(), String> {
    let path = state.paths.database_path.clone();
    let lease = state.paths.archive.current.clone().read_owned().await;
    let (thread_id, name) = tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        chat::rename(&path, &id, &title)?;
        Ok::<_, String>((chat::codex_thread(&path, &id)?, title))
    })
    .await
    .map_err(|_| "채팅방 이름을 바꾸지 못했습니다".to_string())??;
    if let Some(thread_id) = thread_id {
        chat_codex::rename_thread(&thread_id, &name).await?;
    }
    Ok(())
}
#[tauri::command]
async fn chat_search(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<chat::ChatSearchResult>, String> {
    let path = state.paths.database_path.clone();
    tauri::async_runtime::spawn_blocking(move || chat::search(&path, &query))
        .await
        .map_err(|_| "채팅방을 검색하지 못했습니다".to_string())?
}

#[tauri::command]
async fn agent_approval_respond(
    state: State<'_, AppState>,
    request_id: String,
    choice: approval::ApprovalChoice,
) -> Result<(), String> {
    state.approvals.respond(&request_id, choice).await
}

#[tauri::command]
async fn permission_mode_get(
    state: State<'_, AppState>,
    provider: permission_modes::Provider,
) -> Result<String, String> {
    permission_modes::get(&state.paths.database_path, provider)
}
#[tauri::command]
async fn permission_mode_set(
    state: State<'_, AppState>,
    provider: permission_modes::Provider,
    mode: String,
) -> Result<(), String> {
    let _lease = state.paths.archive.current.clone().read_owned().await;
    permission_modes::set(&state.paths.database_path, provider, &mode)
}
#[tauri::command]
async fn permission_modes_available(
    provider: permission_modes::Provider,
) -> Vec<permission_modes::ModeAvailability> {
    permission_modes::available_for_runtime(provider).await
}

#[tauri::command]
async fn run_ai_assistant(
    state: State<'_, AppState>,
    request: ai_runtime::AiAssistantRequest,
) -> Result<ai_runtime::AiAssistantResponse, String> {
    let _lease = state.paths.archive.current.clone().read_owned().await;
    let request_id = request.request_id.clone();
    let cancellation = tokio_util::sync::CancellationToken::new();
    state
        .ai_cancellations
        .lock()
        .await
        .insert(request_id.clone(), cancellation.clone());
    let result = ai_runtime::run(
        &state.paths.database_path,
        &state.paths.data_root,
        request,
        cancellation,
    )
    .await
    .map_err(|error| error.to_string());
    state.ai_cancellations.lock().await.remove(&request_id);
    result
}

#[tauri::command]
async fn cancel_ai_assistant(state: State<'_, AppState>, request_id: String) -> Result<(), String> {
    if let Some(cancellation) = state.ai_cancellations.lock().await.get(&request_id) {
        cancellation.cancel();
    }
    Ok(())
}

#[tauri::command]
async fn ai_connection_overview() -> Result<connections::ConnectionOverview, String> {
    connections::ai_overview()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ai_usage_overview(
    state: State<'_, AppState>,
    provider: ai_runtime::AiProvider,
    auth_mode: ai_runtime::AiAuthMode,
) -> Result<ai_usage::AiUsageOverview, String> {
    let database_path = state.paths.database_path.clone();
    let mut overview = tauri::async_runtime::spawn_blocking(move || {
        ai_usage::local_overview(&database_path, provider, auth_mode)
    })
    .await
    .map_err(|error| format!("Local AI usage task failed: {error}"))?
    .map_err(|error| error.to_string())?;

    overview.quota = match (provider, auth_mode) {
        (ai_runtime::AiProvider::Codex, ai_runtime::AiAuthMode::Subscription) => {
            ai_usage::fetch_codex_quota().await
        }
        (ai_runtime::AiProvider::Claude, ai_runtime::AiAuthMode::Subscription) => {
            ai_usage::AiQuotaOverview::restricted()
        }
        (_, ai_runtime::AiAuthMode::ApiKey) => ai_usage::AiQuotaOverview::not_applicable(),
    };
    Ok(overview)
}

async fn run_record_task<T, F>(task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, records::RecordError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| format!("Local record task failed: {error}"))?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn create_record_draft(
    state: State<'_, AppState>,
    input: records::CreateRecordDraftInput,
) -> Result<records::RecordDetail, String> {
    let database_path = state.paths.database_path.clone();
    let lease = state.paths.archive.current.clone().read_owned().await;
    run_record_task(move || {
        let _lease = lease;
        records::create_draft(&database_path, input)
    })
    .await
}

#[tauri::command]
async fn list_record_summaries(
    state: State<'_, AppState>,
) -> Result<records::RecordReadinessReport, String> {
    let database_path = state.paths.database_path.clone();
    run_record_task(move || records::list_summaries(&database_path)).await
}

#[tauri::command]
async fn get_record_detail(
    state: State<'_, AppState>,
    record_id: String,
) -> Result<records::RecordDetail, String> {
    let database_path = state.paths.database_path.clone();
    run_record_task(move || records::get_record(&database_path, &record_id)).await
}

#[tauri::command]
async fn read_record_body_page(
    state: State<'_, AppState>,
    record_id: String,
    version: Option<i64>,
    cursor: u64,
    expected_body_sha256: Option<String>,
    max_bytes: u64,
) -> Result<records::RecordBodyPage, String> {
    let database_path = state.paths.database_path.clone();
    run_record_task(move || {
        records::read_body_page(
            &database_path,
            &record_id,
            version,
            cursor,
            expected_body_sha256.as_deref(),
            max_bytes,
        )
    })
    .await
}

async fn run_source_asset_task<T, F>(task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, source_assets::SourceAssetError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| format!("Local source file task failed: {error}"))?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn plan_source_asset_import(
    state: State<'_, AppState>,
    proposal: source_assets::SourceAssetImportProposal,
) -> Result<source_assets::SourceAssetImportPlan, String> {
    let database_path = state.paths.database_path.clone();
    let root = state.paths.archive.current.clone().read_owned().await;
    run_source_asset_task(move || {
        source_assets::plan_import(&database_path, &root.join("objects"), proposal)
    })
    .await
}

#[tauri::command]
async fn apply_source_asset_import(
    state: State<'_, AppState>,
    input: source_assets::ApplySourceAssetImportInput,
) -> Result<source_assets::SourceAsset, String> {
    let database_path = state.paths.database_path.clone();
    let root = state.paths.archive.current.clone().read_owned().await;
    run_source_asset_task(move || {
        let _guard = chat_file_changes::lock_app_write();
        source_assets::apply_import(&database_path, &root.join("objects"), input)
    })
    .await
}

#[tauri::command]
async fn plan_source_asset_quarantine(
    state: State<'_, AppState>,
    proposal: source_assets::SourceAssetQuarantineProposal,
) -> Result<source_assets::SourceAssetQuarantinePlan, String> {
    let database_path = state.paths.database_path.clone();
    let root = state.paths.archive.current.clone().read_owned().await;
    run_source_asset_task(move || {
        source_assets::plan_quarantine(&database_path, &root.join("objects"), proposal)
    })
    .await
}

#[tauri::command]
async fn apply_source_asset_quarantine(
    state: State<'_, AppState>,
    input: source_assets::ApplySourceAssetQuarantineInput,
) -> Result<source_assets::SourceAsset, String> {
    let database_path = state.paths.database_path.clone();
    let root = state.paths.archive.current.clone().read_owned().await;
    run_source_asset_task(move || {
        source_assets::apply_quarantine(&database_path, &root.join("objects"), input)
    })
    .await
}

#[tauri::command]
async fn list_source_assets(
    state: State<'_, AppState>,
    record_id: Option<String>,
) -> Result<Vec<source_assets::SourceAsset>, String> {
    let database_path = state.paths.database_path.clone();
    run_source_asset_task(move || source_assets::list(&database_path, record_id.as_deref())).await
}

#[tauri::command]
async fn read_source_text_page(
    state: State<'_, AppState>,
    asset_id: String,
    cursor: u64,
    expected_content_sha256: Option<String>,
    max_bytes: u64,
) -> Result<source_assets::SourceTextPage, String> {
    let database_path = state.paths.database_path.clone();
    let root = state.paths.archive.current.clone().read_owned().await;
    run_source_asset_task(move || {
        source_assets::read_text_page(
            &database_path,
            &root.join("objects"),
            &asset_id,
            cursor,
            expected_content_sha256.as_deref(),
            max_bytes,
        )
    })
    .await
}

#[tauri::command]
async fn start_source_asset_audit(
    state: State<'_, AppState>,
) -> Result<operations::OperationStatus, String> {
    let root = Arc::new(state.paths.archive.current.clone().read_owned().await);
    let database_path = state.paths.database_path.clone();
    let started = tauri::async_runtime::spawn_blocking({
        let database_path = database_path.clone();
        move || operations::start(&database_path, "source_asset_integrity", "preparing", 0)
    })
    .await
    .map_err(|error| format!("Source file audit start failed: {error}"))?
    .map_err(|error| error.to_string())?;
    let operation_id = started.id.clone();
    let cancellation = tokio_util::sync::CancellationToken::new();
    state
        .operation_cancellations
        .lock()
        .await
        .insert(operation_id.clone(), cancellation.clone());
    let cancellations = state.operation_cancellations.clone();

    tauri::async_runtime::spawn(async move {
        let worker_root = root.clone();
        let _lease = root;
        let task_operation_id = operation_id.clone();
        let task_database_path = database_path.clone();
        let task_cancellation = cancellation.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            source_assets::audit_integrity(
                &task_database_path,
                &worker_root.join("objects"),
                |completed, total| {
                    operations::progress(
                        &task_database_path,
                        &task_operation_id,
                        "checking_source_files",
                        completed,
                        total,
                    )
                    .map_err(|error| source_assets::SourceAssetError::Integrity(error.to_string()))
                },
                || task_cancellation.is_cancelled(),
            )
        })
        .await;

        match result {
            Ok(Ok(audit)) => {
                let total = audit.total_assets.saturating_add(audit.total_object_files);
                if let Err(error) = operations::succeed(
                    &database_path,
                    &operation_id,
                    "complete",
                    total,
                    total,
                    &audit,
                ) {
                    log::error!("Source file audit result persistence failed: {error}");
                }
            }
            Ok(Err(source_assets::SourceAssetError::Cancelled)) => {
                let _ = operations::finish_without_result(
                    &database_path,
                    &operation_id,
                    operations::OperationState::Cancelled,
                    "cancelled",
                    "user_cancelled",
                );
            }
            Ok(Err(error)) => {
                log::error!("Source file audit failed: {}", error);
                let _ = operations::finish_without_result(
                    &database_path,
                    &operation_id,
                    operations::OperationState::Failed,
                    "failed",
                    "source_asset_audit_failed",
                );
            }
            Err(error) => {
                log::error!("Source file audit worker failed: {error}");
                let _ = operations::finish_without_result(
                    &database_path,
                    &operation_id,
                    operations::OperationState::Failed,
                    "failed",
                    "worker_join_failed",
                );
            }
        }
        cancellations.lock().await.remove(&operation_id);
    });
    Ok(started)
}

#[tauri::command]
async fn get_operation_status(
    state: State<'_, AppState>,
    operation_id: String,
) -> Result<operations::OperationStatus, String> {
    let database_path = state.paths.database_path.clone();
    tauri::async_runtime::spawn_blocking(move || operations::get(&database_path, &operation_id))
        .await
        .map_err(|error| format!("Operation status task failed: {error}"))?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn cancel_operation(state: State<'_, AppState>, operation_id: String) -> Result<(), String> {
    if let Some(cancellation) = state
        .operation_cancellations
        .lock()
        .await
        .get(&operation_id)
    {
        cancellation.cancel();
    }
    Ok(())
}

#[tauri::command]
async fn plan_record_change(
    state: State<'_, AppState>,
    proposal: records::RecordChangeProposal,
) -> Result<records::RecordChangePlan, String> {
    let database_path = state.paths.database_path.clone();
    run_record_task(move || records::plan_change(&database_path, proposal)).await
}

#[tauri::command]
async fn apply_record_change(
    state: State<'_, AppState>,
    input: records::ApplyRecordChangeInput,
) -> Result<records::RecordDetail, String> {
    let database_path = state.paths.database_path.clone();
    let lease = state.paths.archive.current.clone().read_owned().await;
    run_record_task(move || {
        let _lease = lease;
        records::apply_change(&database_path, input)
    })
    .await
}

#[tauri::command]
async fn list_record_history(
    state: State<'_, AppState>,
    record_id: String,
) -> Result<records::RecordHistory, String> {
    let database_path = state.paths.database_path.clone();
    run_record_task(move || records::list_history(&database_path, &record_id)).await
}

#[tauri::command]
async fn plan_record_restore(
    state: State<'_, AppState>,
    proposal: records::RecordRestoreProposal,
) -> Result<records::RecordRestorePlan, String> {
    let database_path = state.paths.database_path.clone();
    run_record_task(move || records::plan_restore(&database_path, proposal)).await
}

#[tauri::command]
async fn apply_record_restore(
    state: State<'_, AppState>,
    input: records::ApplyRecordRestoreInput,
) -> Result<records::RecordDetail, String> {
    let database_path = state.paths.database_path.clone();
    let lease = state.paths.archive.current.clone().read_owned().await;
    run_record_task(move || {
        let _lease = lease;
        records::apply_restore(&database_path, input)
    })
    .await
}

#[tauri::command]
async fn audit_record_summaries(
    state: State<'_, AppState>,
) -> Result<records::SummaryFreshnessReport, String> {
    let database_path = state.paths.database_path.clone();
    run_record_task(move || records::audit_summary_freshness(&database_path)).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .clear_targets()
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::LogDir {
                        file_name: Some("momso".to_string()),
                    }),
                ])
                .level(log::LevelFilter::Info)
                .max_file_size(5_000_000)
                .rotation_strategy(RotationStrategy::KeepSome(5))
                .timezone_strategy(TimezoneStrategy::UseLocal)
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let (paths, status) = RuntimePaths::prepare(app.handle())?;
            let resources = app.path().resource_dir()?;
            let archive_root = paths
                .archive
                .current
                .try_read()
                .map_err(std::io::Error::other)?
                .clone();
            archive::initialize(&archive_root, &resources).map_err(std::io::Error::other)?;
            if paths.archive.needs_initial_index {
                archive::reindex(&archive_root, &paths.database_path)
                    .map_err(std::io::Error::other)?;
                let connection = storage::open_writable_connection(&paths.database_path)?;
                connection.execute(
                    "UPDATE archive_location_settings SET initialized=1 WHERE id=1",
                    [],
                )?;
            }
            let interrupted_operations = operations::recover_interrupted(&paths.database_path)?;
            if interrupted_operations > 0 {
                log::warn!("Recovered {interrupted_operations} interrupted local operation(s)");
            }
            match source_assets::cleanup_staging(&paths.objects_path) {
                Ok(removed) if removed > 0 => {
                    log::warn!("Removed {removed} interrupted source import temporary file(s)");
                }
                Ok(_) => {}
                Err(error) => log::warn!("Source import staging cleanup failed: {error}"),
            }
            log::info!(
                "momso desktop runtime ready: schema={}, health={}",
                status.schema_version,
                status.health
            );
            app.manage(app_updates::UpdateState::default());
            app_updates::start_monitor(app.handle().clone());
            app.manage(AppState {
                paths,
                approvals: approval::ApprovalBroker::default(),
                usage_monitor: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                usage_cache: Arc::new(usage_status::UsageCache::default()),
                chat_active: Default::default(),
                ai_cancellations: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
                operation_cancellations: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_updates::app_version,
            app_updates::update_check,
            app_updates::update_install,
            app_updates::update_restart,
            media::media_list,
            archive_status,
            archive_remote::archive_remote_status,
            archive_remote::momso_username_get,
            archive_remote::momso_username_set,
            archive_remote::archive_remote_create,
            archive_remote::archive_remote_connect,
            archive_remote::archive_remote_push,
            archive_remote::archive_remote_pull,
            archive_remote::archive_remote_disconnect,
            archive_location::archive_location_get,
            archive_location::archive_location_set,
            archive_location::archive_location_open,
            zet_read,
            zet_create,
            zet_list,
            zet_update,
            zet_save,
            zet_linked,
            receipt_read,
            receipt_create,
            chat_attach_asset,
            chat_items,
            archive_reindex,
            archive_check,
            user_profile_get,
            user_profile_set,
            archive_save,
            zet_history,
            zet_read_version,
            zet_restore_version,
            runtime_info,
            create_data_backup,
            connection_overview,
            ai_connection_overview,
            connect_external_provider,
            install_external_provider,
            save_ai_api_key,
            delete_ai_api_key,
            provider_auth::provider_auth_status,
            provider_auth::provider_login_start,
            provider_auth::provider_login_cancel,
            provider_auth::provider_logout,
            provider_auth::provider_api_key_set,
            provider_auth::provider_api_key_clear,
            list_transcription_terms,
            create_transcription_term,
            set_transcription_term_enabled,
            delete_transcription_term,
            user_owned_backup_overview,
            user_owned_backup_evidence,
            prepare_user_owned_backup,
            discover_user_owned_backups,
            adopt_user_owned_backup,
            run_user_owned_backup,
            start_user_owned_backup,
            plan_user_owned_restore,
            apply_user_owned_restore,
            start_user_owned_restore,
            export_backup_recovery_key,
            import_backup_recovery_key,
            confirm_backup_recovery_key,
            usage_status,
            usage_refresh,
            ai_models,
            chat_context,
            chat_send,
            skills::skill_list,
            skills::skill_read,
            skills_write::skill_create,
            skills_write::skill_update,
            skills_write::skill_set_enabled,
            skills_write::skill_remove,
            chat_cancel,
            chat_group_list,
            chat_group_create,
            chat_group_rename,
            chat_group_reorder,
            chat_group_remove,
            chat_set_group,
            chat_list,
            chat_create,
            chat_read,
            chat_append,
            chat_rename,
            chat_search,
            agent_approval_respond,
            permission_mode_get,
            permission_mode_set,
            permission_modes_available,
            run_ai_assistant,
            cancel_ai_assistant,
            ai_usage_overview,
            create_record_draft,
            list_record_summaries,
            get_record_detail,
            read_record_body_page,
            plan_record_change,
            apply_record_change,
            list_record_history,
            plan_record_restore,
            apply_record_restore,
            audit_record_summaries,
            plan_source_asset_import,
            apply_source_asset_import,
            plan_source_asset_quarantine,
            apply_source_asset_quarantine,
            list_source_assets,
            source_search::source_search,
            object_store::object_store_status,
            object_store::object_store_connect,
            object_store::object_store_disconnect,
            object_store_transfer::object_store_backup,
            object_store_restore::object_store_restore,
            object_store_restore::object_store_verify,
            object_store_state::app_state_backup,
            app_state_restore,
            read_source_text_page,
            start_source_asset_audit,
            get_operation_status,
            cancel_operation,
            discover_local_ai_conversations,
            load_local_ai_conversations,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the momso desktop application");
}

#[cfg(test)]
mod feedback_retention_tests {
    use super::storage;
    use rusqlite::Connection;

    #[test]
    fn migration_and_restart_preserve_existing_feedback_outbox() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let database = temp.path().join("momso.db");
        let backups = temp.path().join("backups");
        let connection = Connection::open(&database).expect("open legacy database");
        for migration in [
            include_str!("../migrations/0001_runtime_metadata.sql"),
            include_str!("../migrations/0002_record_lifecycle.sql"),
            include_str!("../migrations/0003_feedback_outbox.sql"),
        ] {
            connection
                .execute_batch(migration)
                .expect("legacy migration");
        }
        for (version, description) in [
            (1, "create runtime metadata tables"),
            (2, "create record lifecycle tables"),
            (3, "create content-free feedback outbox"),
        ] {
            connection
                .execute(
                    "INSERT INTO app_schema_migrations(version,description) VALUES (?1,?2)",
                    rusqlite::params![version, description],
                )
                .expect("record legacy migration");
        }
        connection
            .pragma_update(None, "user_version", 3)
            .expect("legacy version");
        connection.execute(
            "INSERT INTO feedback_outbox (feedback_id, operation, rating, workflow, response_kind, completion, length_bucket, app_version, platform, schema_version) VALUES ('preserved', 'upsert', 'helpful', 'ask', 'generated', 'completed', 'under_500', 'old', 'windows', 1)",
            [],
        ).expect("insert old feedback");
        drop(connection);

        storage::initialize(&database, &backups, "new").expect("migrate database");
        storage::initialize(&database, &backups, "new").expect("restart database");
        let connection = Connection::open(&database).expect("reopen database");
        let row: (String, String, i64) = connection
            .query_row(
                "SELECT feedback_id, rating, revision FROM feedback_outbox",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("old feedback remains");
        assert_eq!(row, ("preserved".into(), "helpful".into(), 1));
    }
}
