//! Explicitly feature-gated, windowless real-Codex integration probe.
use std::{path::PathBuf, sync::Arc, time::Duration};
use tauri::{Listener, Manager};

pub fn run() {
    run_cases(false);
}

pub fn run_g62() {
    if std::env::var_os("MOMSO_G62_PROBE").is_none() {
        eprintln!("G62_PROBE missing isolation flag");
        return;
    }
    run_cases(true);
}

fn run_cases(g62: bool) {
    if std::env::var_os("MOMSO_TEST_FAKE_CREDENTIAL_STORE").is_none() {
        eprintln!("G61_PROBE missing isolated credential store");
        return;
    }
    let Some(target) = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from) else {
        eprintln!("G61_PROBE missing target");
        return;
    };
    let Ok(temp) = tempfile::tempdir_in(target) else {
        eprintln!("G61_PROBE temporary directory failed");
        return;
    };
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let Ok(app) = tauri::Builder::default().build(context) else {
        eprintln!("G61_PROBE app initialization failed");
        return;
    };
    let app_handle = app.handle().clone();
    let result = tauri::async_runtime::block_on(async {
        let data = temp.path().join("data");
        let home = temp.path().join("home");
        let root = home.join("momso");
        std::fs::create_dir_all(&data).map_err(|_| "data")?;
        std::fs::create_dir_all(&home).map_err(|_| "home")?;
        let database = data.join("momso.db");
        crate::storage::initialize_with_progress(
            &database,
            &data.join("backups"),
            "fixture",
            |_| {},
        )
        .map_err(|_| "database")?;
        let resources = app_handle.path().resource_dir().map_err(|_| "resources")?;
        let archive =
            crate::archive_location::ArchiveLocation::load(database.clone(), &home, vec![])
                .map_err(|_| "location")?;
        crate::archive::initialize(&root, &resources).map_err(|_| "archive")?;
        crate::skills::install_bundled(&root, &resources).map_err(|_| "skills")?;
        let browser = Arc::new(crate::browser_runtime::BrowserManager::default());
        browser.bind_app(app_handle.clone());
        app_handle.manage(crate::AppState {
            paths: crate::runtime::RuntimePaths {
                data_root: data.clone(),
                database_path: database.clone(),
                objects_path: root.join("objects"),
                exports_path: data.join("exports"),
                backups_path: data.join("backups"),
                logs_path: data.join("logs"),
                archive,
                archive_lease: None,
            },
            approvals: Default::default(),
            setup_secrets: Default::default(),
            usage_monitor: Default::default(),
            usage_cache: Default::default(),
            chat_active: Default::default(),
            browser,
            selections: Default::default(),
        });
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, serde_json::Value)>();
        for name in [
            "agent-approval-request",
            "panel-agent-open-request",
            "chat-stream",
            "chat-done",
            "chat-error",
            "g62-probe-tool",
            "g62-probe-shell",
        ] {
            let tx = tx.clone();
            app_handle.listen(name, move |event| {
                if let Ok(value) = serde_json::from_str(event.payload()) {
                    let _ = tx.send((name.to_owned(), value));
                }
            });
        }
        let cases = if g62 {
            vec![
                (
                    "conversation-import",
                    "ChatGPT에서 내보낸 대화를 가져와 줘",
                    Some("conversation-import".to_owned()),
                ),
                (
                    "github-connect",
                    "GitHub 비공개 저장소를 연결해 줘",
                    Some("github-connect".to_owned()),
                ),
                (
                    "object-store-connect",
                    "Cloudflare R2 원자료 저장소를 연결해 줘",
                    Some("object-store-connect".to_owned()),
                ),
                (
                    "naver-smartplace",
                    "네이버 스마트플레이스 스킬로 브라우저를 열어 줘",
                    Some("naver-smartplace".to_owned()),
                ),
                (
                    "soomgo-operations",
                    "숨고 스킬로 브라우저를 열어 줘",
                    Some("soomgo-operations".to_owned()),
                ),
                (
                    "sssd-host-operations",
                    "솜씨당 스킬로 브라우저를 열어 줘",
                    Some("sssd-host-operations".to_owned()),
                ),
                (
                    "frip-host-operations",
                    "프립 스킬로 브라우저를 열어 줘",
                    Some("frip-host-operations".to_owned()),
                ),
                (
                    "onstudio-operations",
                    "온스튜디오 스킬로 브라우저를 열어 줘",
                    Some("onstudio-operations".to_owned()),
                ),
                (
                    "studiomate-operations",
                    "스튜디오메이트 스킬로 브라우저를 열어 줘",
                    Some("studiomate-operations".to_owned()),
                ),
            ]
        } else {
            vec![
                ("document", "몸소 문서로 '시험' 제목 문서 만들어 줘", None),
                (
                    "browser",
                    "네이버 스마트플레이스 스킬로 브라우저를 열어 줘",
                    Some("naver-smartplace".to_owned()),
                ),
            ]
        };
        for (label, prompt, skill) in cases {
            if g62 && std::env::var("MOMSO_G62_SKILL").is_ok_and(|filter| filter != label) {
                continue;
            }
            let chat = serde_json::to_value(crate::chat::create(&database).map_err(|_| "chat")?)
                .map_err(|_| "chat")?["id"]
                .as_str()
                .ok_or("chat")?
                .to_owned();
            let started = crate::chat_send(
                app_handle.clone(),
                app_handle.state(),
                chat.clone(),
                prompt.into(),
                "codex".into(),
                None,
                None,
                skill,
                None,
                None,
                None,
                None,
            )
            .await;
            if !started
                .as_ref()
                .is_ok_and(|value| value["status"] == "started")
            {
                println!("G61_PROBE {label} start={started:?}");
                continue;
            }
            let mut seen = false;
            let mut answer = String::new();
            let wait = tokio::time::timeout(Duration::from_secs(120), async {
                while let Some((name, payload)) = rx.recv().await {
                    if payload
                        .get("chatId")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|id| id != chat)
                    {
                        continue;
                    }
                    if name == "chat-stream" {
                        if let Some(delta) = payload["delta"].as_str() {
                            answer.push_str(delta);
                        }
                    }
                    if name == "g62-probe-tool" && g62 {
                        seen = true;
                        if let Some(token) = app_handle
                            .state::<crate::AppState>()
                            .chat_active
                            .lock()
                            .await
                            .get(&chat)
                        {
                            token.cancel();
                        }
                    }
                    if name == "g62-probe-shell" && g62 {
                        println!("G62_PROBE {label} shell_attempt=true");
                        break;
                    }
                    if name == "agent-approval-request" {
                        let is_document = payload["kind"] == "file_change"
                            && payload["targets"]
                                .as_array()
                                .is_some_and(|t| t.iter().any(|v| v["path"] == "zettels/"));
                        if label == "document" && is_document {
                            seen = true;
                        }
                        let choice = if (label == "browser" || g62)
                            && payload["kind"] == "browser_action_scope"
                        {
                            crate::approval::ApprovalChoice::AllowOnce
                        } else {
                            crate::approval::ApprovalChoice::Deny
                        };
                        if let Some(id) = payload["requestId"].as_str() {
                            let _ = app_handle
                                .state::<crate::AppState>()
                                .approvals
                                .respond(id, choice)
                                .await;
                        }
                    }
                    if name == "panel-agent-open-request" && (label == "browser" || g62) {
                        seen = true;
                        if let Some(token) = app_handle
                            .state::<crate::AppState>()
                            .chat_active
                            .lock()
                            .await
                            .get(&chat)
                        {
                            token.cancel();
                        }
                    }
                    if name == "chat-error" || name == "chat-done" {
                        break;
                    }
                }
            })
            .await;
            if let Some(token) = app_handle
                .state::<crate::AppState>()
                .chat_active
                .lock()
                .await
                .get(&chat)
            {
                token.cancel();
            }
            let safe = crate::chat_archive_redact::redact_with_known(&answer, &[]).text;
            println!(
                "G61_PROBE {label} reached={seen} timeout={} answer={}",
                wait.is_err(),
                safe.chars().take(2000).collect::<String>()
            );
        }
        Ok::<(), &'static str>(())
    });
    if let Err(error) = result {
        eprintln!("G61_PROBE setup failed: {error}");
    }
}
