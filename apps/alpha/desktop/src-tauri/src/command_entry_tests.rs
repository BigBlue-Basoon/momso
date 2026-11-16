use super::*;
use serde_json::{json, Value};
use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::webview::InvokeRequest;

pub(crate) fn invoke(window: &tauri::WebviewWindow<MockRuntime>, cmd: &str, body: Value) -> Value {
    get_ipc_response(
        window,
        InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.into(),
        },
    )
    .unwrap_or_else(|error| panic!("{cmd}: {error}"))
    .deserialize()
    .unwrap()
}

#[test]
fn fresh_chat_commands_use_tauri_entry_points() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let database_path = root.join("momso.db");
    let backups_path = root.join("backups");
    std::fs::create_dir_all(&backups_path).unwrap();
    storage::initialize(&database_path, &backups_path, "test").unwrap();
    let archive =
        archive_location::ArchiveLocation::load(database_path.clone(), root, vec![]).unwrap();
    let archive_root = root.join("momso");
    archive::initialize(&archive_root, root).unwrap();
    let paths = RuntimePaths {
        data_root: root.to_path_buf(),
        database_path,
        objects_path: archive_root.join("objects"),
        exports_path: root.join("exports"),
        backups_path,
        logs_path: root.join("logs"),
        archive,
        archive_lease: None,
    };
    let app = mock_builder()
        .manage(AppState {
            paths,
            approvals: approval::ApprovalBroker::default(),
            setup_secrets: setup_secret::SecretBroker::default(),
            usage_monitor: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            usage_cache: Arc::new(usage_status::UsageCache::default()),
            chat_active: Default::default(),
            browser: Default::default(),
        })
        .invoke_handler(tauri::generate_handler![
            chat_list,
            chat_create,
            chat_read,
            chat_rename,
            chat_search,
            chat_archived_list,
            chat_group_list,
            chat_group_create,
            chat_group_rename,
            chat_group_reorder,
            chat_group_remove,
            chat_set_group,
            user_profile_get,
            zet_list,
            archive_reindex,
            permission_mode_get,
            chat_import_export,
            zet_create,
            zet_read,
            zet_update,
            chat_items,
            archive_remote::momso_username_get,
            archive_remote::momso_username_set,
            archive_location::archive_location_get,
            object_store::object_store_status,
            chat_archive_status::chat_archive_sync_status
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    assert_eq!(invoke(&window, "chat_list", json!({})), json!([]));
    assert_eq!(invoke(&window, "chat_archived_list", json!({})), json!([]));
    assert_eq!(invoke(&window, "chat_group_list", json!({})), json!([]));
    assert_eq!(
        invoke(&window, "object_store_status", json!({}))["state"],
        "not_connected"
    );
    assert_eq!(
        invoke(&window, "chat_archive_sync_status", json!({}))["state"],
        "saved"
    );
    assert!(invoke(&window, "archive_location_get", json!({}))["path"].is_string());
    invoke(&window, "momso_username_set", json!({"username":"example"}));
    assert_eq!(
        invoke(&window, "momso_username_get", json!({}))["username"],
        "example"
    );
    assert_eq!(invoke(&window, "zet_list", json!({})), json!([]));
    assert_eq!(invoke(&window, "user_profile_get", json!({}))["name"], "");
    assert!(invoke(&window, "permission_mode_get", json!({"provider":"codex"})).is_string());
    invoke(&window, "archive_reindex", json!({}));
    let created = invoke(&window, "chat_create", json!({}));
    let id = created["id"].as_str().unwrap();
    let group = invoke(&window, "chat_group_create", json!({"name":"Yoga"}));
    let group_id = group["id"].as_str().unwrap();
    invoke(
        &window,
        "chat_group_rename",
        json!({"id":group_id,"name":"Practice"}),
    );
    invoke(&window, "chat_group_reorder", json!({"ids":[group_id]}));
    invoke(
        &window,
        "chat_set_group",
        json!({"chatId":id,"groupId":group_id}),
    );
    invoke(&window, "chat_group_remove", json!({"id":group_id}));
    assert_eq!(invoke(&window, "chat_read", json!({"id": id}))["id"], id);
    invoke(&window, "chat_rename", json!({"id": id, "title": "Yoga"}));
    assert_eq!(invoke(&window, "chat_list", json!({}))[0]["title"], "Yoga");
    assert_eq!(
        invoke(&window, "chat_search", json!({"query": "Yoga"}))[0]["id"],
        id
    );
    let export_path = root.join("conversations.json");
    std::fs::write(&export_path, json!([{"id":"source","current_node":"a","mapping":{"a":{"parent":null,"message":{"id":"m1","author":{"role":"user"},"content":{"parts":["hello"]}}}}}]).to_string()).unwrap();
    let result = invoke(
        &window,
        "chat_import_export",
        json!({"path":export_path.to_str().unwrap(),"provider":"chatgpt"}),
    );
    assert_eq!(result["imported"], 1);
    let rooms = invoke(&window, "chat_list", json!({}));
    let rooms = rooms.as_array().unwrap();
    assert_eq!(rooms.len(), 2);
    let imported = rooms
        .iter()
        .find(|room| room["importedFrom"] == "chatgpt")
        .unwrap();
    let imported_id = imported["id"].as_str().unwrap();
    assert!(root
        .join("momso")
        .join("chats")
        .join(format!("{imported_id}.json"))
        .is_file());
    let document = invoke(
        &window,
        "zet_create",
        json!({"input":{"title":"Yoga","summary":"Practice","kind":"note","body":"First","assets":[],"edges":[]}}),
    );
    let document_id = document.as_str().unwrap();
    assert_eq!(
        invoke(&window, "zet_read", json!({"id":document_id}))["body"],
        "First"
    );
    invoke(
        &window,
        "zet_update",
        json!({"id":document_id,"changes":{"body":"Second"}}),
    );
    assert_eq!(
        invoke(&window, "zet_read", json!({"id":document_id}))["body"],
        "Second"
    );
    assert!(invoke(&window, "chat_items", json!({"chatId":id})).is_object());
}
