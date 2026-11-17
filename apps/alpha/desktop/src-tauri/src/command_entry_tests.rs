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
fn webview_import_rejects_unselected_and_consumes_selected_file() {
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
            selections: Default::default(),
        })
        .invoke_handler(tauri::generate_handler![
            plan_source_asset_import,
            apply_source_asset_import,
            attachment_text_read,
            chat_import_export,
            chat_import_export_preview,
            chat_import_export_progress,
            load_local_ai_conversations,
            archive_location::archive_location_set,
            object_store_restore::object_store_recover_original_copy,
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let file = root.join("unselected-fixture.txt");
    std::fs::write(&file, "synthetic-private-fixture").unwrap();
    let proposal = json!({"sourcePath":file.to_str().unwrap(),"assetKind":"raw_transcript","mediaType":"text/plain","recordLink":null,"actor":"fixture"});
    let reject = |cmd: &str, body: Value, expected: &str| {
        let result = get_ipc_response(
            &window,
            InvokeRequest {
                cmd: cmd.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "http://tauri.localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.into(),
            },
        );
        let error = result
            .expect_err("the unselected or unsafe path must be rejected")
            .to_string();
        assert!(error.contains(expected), "{cmd}: {error}");
    };
    let unselected = file.to_str().unwrap();
    reject(
        "apply_source_asset_import",
        json!({"input":{"proposal":proposal,"planDigest":"unused","importConfirmed":true}}),
        "selection_required",
    );
    for (cmd, body) in [
        (
            "chat_import_export",
            json!({"path":unselected,"provider":"chatgpt"}),
        ),
        (
            "chat_import_export_preview",
            json!({"path":unselected,"provider":"chatgpt"}),
        ),
        (
            "chat_import_export_progress",
            json!({"path":unselected,"provider":"chatgpt","requestId":"fixture"}),
        ),
        (
            "archive_location_set",
            json!({"path":root.to_str().unwrap()}),
        ),
    ] {
        reject(cmd, body, "selection_required");
    }
    reject(
        "load_local_ai_conversations",
        json!({"provider":"codex","conversationIds":[unselected]}),
        "invalid_conversation_id",
    );
    let too_long = "a".repeat(33000);
    for path in [
        "../outside",
        "C:\\outside",
        "\\\\server\\share\\file",
        "CON",
        "folder/NUL",
        "folder/end.",
        "folder/end ",
        too_long.as_str(),
    ] {
        let bad_proposal = json!({"sourcePath":path,"assetKind":"raw_transcript","mediaType":"text/plain","recordLink":null,"actor":"fixture"});
        reject(
            "plan_source_asset_import",
            json!({"proposal":bad_proposal}),
            "selection_required",
        );
        reject(
            "apply_source_asset_import",
            json!({"input":{"proposal":bad_proposal,"planDigest":"unused","importConfirmed":true}}),
            "selection_required",
        );
        reject(
            "chat_import_export",
            json!({"path":path,"provider":"chatgpt"}),
            "selection_required",
        );
        reject(
            "chat_import_export_preview",
            json!({"path":path,"provider":"chatgpt"}),
            "selection_required",
        );
        reject(
            "chat_import_export_progress",
            json!({"path":path,"provider":"chatgpt","requestId":"fixture"}),
            "selection_required",
        );
        reject(
            "archive_location_set",
            json!({"path":path}),
            "selection_required",
        );
        reject(
            "load_local_ai_conversations",
            json!({"provider":"codex","conversationIds":[path]}),
            "invalid_conversation_id",
        );
        reject(
            "object_store_recover_original_copy",
            json!({"relativePath":path,"expectedSha256":"a".repeat(64)}),
            "invalid_id",
        );
    }
    let denied = get_ipc_response(
        &window,
        InvokeRequest {
            cmd: "plan_source_asset_import".into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(json!({"proposal":proposal})),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.into(),
        },
    );
    assert!(denied.is_err());
    app.state::<AppState>().selections.remember(
        std::slice::from_ref(&file),
        selection_grants::Purpose::Source,
    );
    let plan = invoke(
        &window,
        "plan_source_asset_import",
        json!({"proposal":proposal}),
    );
    reject(
        "apply_source_asset_import",
        json!({"input":{"proposal":proposal,"planDigest":"fixture-wrong","importConfirmed":true}}),
        "changed",
    );
    let imported = invoke(
        &window,
        "apply_source_asset_import",
        json!({"input":{"proposal":proposal,"planDigest":plan["planDigest"],"importConfirmed":true}}),
    );
    let opened = invoke(
        &window,
        "attachment_text_read",
        json!({"assetId":imported["id"],"cursor":0}),
    );
    assert_eq!(opened["content"], "synthetic-private-fixture");
    assert!(app
        .state::<AppState>()
        .selections
        .check(
            file.to_str().unwrap(),
            selection_grants::Purpose::Source,
            false
        )
        .is_err());
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
            selections: Default::default(),
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
            chat_import_export_preview,
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
    while !chat_redaction_recheck::step(&archive_root, &app.state::<AppState>().paths)
        .unwrap()
        .available
    {}
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
    app.state::<AppState>().selections.remember(
        std::slice::from_ref(&export_path),
        selection_grants::Purpose::Conversation,
    );
    let preview = invoke(
        &window,
        "chat_import_export_preview",
        json!({"path":export_path.to_str().unwrap(),"provider":"chatgpt"}),
    );
    assert_eq!(preview["conversations"], 1);
    let result = invoke(
        &window,
        "chat_import_export",
        json!({"path":export_path.to_str().unwrap(),"provider":"chatgpt"}),
    );
    assert_eq!(result["imported"], 1);
    assert!(app
        .state::<AppState>()
        .selections
        .check(
            export_path.to_str().unwrap(),
            selection_grants::Purpose::Conversation,
            false
        )
        .is_err());
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
