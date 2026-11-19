//! Isolated fake CLI exercises the real login-return command and chat sender.
use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tauri::Manager;

#[test]
fn fake_mcp_child() {
    let Some(pipe_file) = std::env::var_os("MOMSO_G70_MCP_CHILD") else {
        return;
    };
    let pipe = std::fs::read_to_string(pipe_file).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe.trim())
        .unwrap();
    writeln!(
        file,
        "{}",
        serde_json::json!({"method":"browser_request_human_login","arguments":{"reason":"login"}})
    )
    .unwrap();
    let mut answer = String::new();
    BufReader::new(file).read_line(&mut answer).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&answer).unwrap()["status"],
        "human_required"
    );
}

#[test]
fn fake_cli_login_finished_resume_sends_one_scoped_turn() {
    if std::env::var_os("MOMSO_G70_ISOLATED").is_none() {
        let status = crate::hidden_process::hidden_command(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "g70_login_resume_test::fake_cli_login_finished_resume_sends_one_scoped_turn",
                "--nocapture",
            ])
            .env("MOMSO_G70_ISOLATED", "1")
            .env("MOMSO_TEST_FAKE_CREDENTIAL_STORE", "1")
            .env("MOMSO_G70_FAKE_BROWSER", "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let temp = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(target) => tempfile::tempdir_in(target).unwrap(),
        None => tempfile::tempdir().unwrap(),
    };
    let root = temp.path();
    let fake_source = root.join("fake_codex.rs");
    let fake_exe = root.join("codex.exe");
    std::fs::write(&fake_source, r###"
use std::{io::{self,BufRead,Write},fs,process::{Command,Stdio},thread,time::{Duration,Instant}};
use std::os::windows::process::CommandExt;
fn main() {
 let args: Vec<_> = std::env::args().collect();
 if args.iter().any(|s| s=="--version") { println!("codex 0.0.0"); return; }
 let mut input=io::BufReader::new(io::stdin()); let mut line=String::new();
 input.read_line(&mut line).unwrap(); println!("{{\"id\":1,\"result\":{{}}}}"); io::stdout().flush().unwrap();
 line.clear(); input.read_line(&mut line).unwrap(); line.clear(); input.read_line(&mut line).unwrap();
 if line.contains("account/read") { println!("{{\"id\":2,\"result\":{{\"account\":{{\"type\":\"chatgpt\"}}}}}}"); return; }
 if line.contains("thread/name/set") { println!("{{\"id\":2,\"result\":{{}}}}"); return; }
 println!("{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"fixture-thread\"}},\"approvalPolicy\":\"on-request\",\"approvalsReviewer\":\"user\",\"sandbox\":{{\"type\":\"workspaceWrite\"}},\"modelProvider\":\"openai\",\"model\":\"fixture\"}}}}"); io::stdout().flush().unwrap();
 line.clear(); input.read_line(&mut line).unwrap();
 let turns=std::env::var("MOMSO_G70_TURNS").unwrap();
 let mut file=fs::OpenOptions::new().create(true).append(true).open(&turns).unwrap(); writeln!(file,"{}",line.trim()).unwrap();
 let count=fs::read_to_string(&turns).unwrap().lines().count();
 if count==1 {
  let pipe=std::env::var("MOMSO_G70_PIPE_FILE").unwrap(); let deadline=Instant::now()+Duration::from_secs(30);
  while !std::path::Path::new(&pipe).exists() && Instant::now()<deadline { thread::sleep(Duration::from_millis(20)); }
  let status=Command::new(std::env::var("MOMSO_G70_TEST_EXE").unwrap()).creation_flags(0x08000000).args(["--exact","g70_login_resume_test::fake_mcp_child","--nocapture"]).env("MOMSO_G70_MCP_CHILD",&pipe).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap();
  if !status.success() { std::process::exit(9); }
  fs::write(std::env::var("MOMSO_G70_WAITING").unwrap(),"waiting").unwrap();
  let release=std::env::var("MOMSO_G70_RELEASE").unwrap(); let deadline=Instant::now()+Duration::from_secs(30);
  while !std::path::Path::new(&release).exists() && Instant::now()<deadline { thread::sleep(Duration::from_millis(20)); }
 }
 println!("{{\"method\":\"item/agentMessage/delta\",\"params\":{{\"delta\":\"fixture reply\"}}}}");
 println!("{{\"method\":\"turn/completed\",\"params\":{{\"turn\":{{\"status\":\"completed\"}}}}}}");
}
"###).unwrap();
    let status = crate::hidden_process::hidden_command("rustc")
        .arg(&fake_source)
        .arg("-o")
        .arg(&fake_exe)
        .status()
        .unwrap();
    assert!(status.success());
    let path = std::env::join_paths(
        std::iter::once(root.to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    std::env::set_var("PATH", path);
    for (key, name) in [
        ("MOMSO_G70_TURNS", "turns"),
        ("MOMSO_G70_PIPE_FILE", "pipe"),
        ("MOMSO_G70_WAITING", "waiting"),
        ("MOMSO_G70_RELEASE", "release"),
    ] {
        std::env::set_var(key, root.join(name));
    }
    std::env::set_var("MOMSO_G70_TEST_EXE", std::env::current_exe().unwrap());
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let app = tauri::Builder::default()
        .any_thread()
        .build(context)
        .unwrap();
    let handle = app.handle().clone();
    tauri::async_runtime::block_on(async {
        let data = root.join("data");
        let home = root.join("home");
        let archive_root = home.join("momso");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let db = data.join("momso.db");
        let backups = data.join("backups");
        storage::initialize(&db, &backups, "fixture").unwrap();
        let resources = handle.path().resource_dir().unwrap();
        let archive = archive_location::ArchiveLocation::load(db.clone(), &home, vec![]).unwrap();
        archive::initialize(&archive_root, &resources).unwrap();
        skills::install_bundled(&archive_root, &resources).unwrap();
        let browser = Arc::new(browser_runtime::BrowserManager::default());
        browser.bind_app(handle.clone());
        handle.manage(AppState {
            paths: RuntimePaths {
                data_root: data.clone(),
                database_path: db.clone(),
                objects_path: archive_root.join("objects"),
                exports_path: data.join("exports"),
                backups_path: backups,
                logs_path: data.join("logs"),
                archive,
                archive_lease: None,
            },
            approvals: Default::default(),
            setup_secrets: Default::default(),
            usage_monitor: Default::default(),
            usage_cache: Default::default(),
            chat_active: Default::default(),
            browser: browser.clone(),
            selections: Default::default(),
        });
        let id = serde_json::to_value(chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        chat::rename(&db, &id, "fixture").unwrap();
        let skill = "naver-smartplace";
        let policy = browser_gateway::Policy::from_skill(&archive_root, skill)
            .unwrap()
            .unwrap();
        let completed = Arc::new(AtomicBool::new(false));
        browser
            .install_fake_login_panel(&id, skill, policy, completed.clone())
            .await;
        let started = chat_send(
            handle.clone(),
            handle.state(),
            id.clone(),
            "fixture request".into(),
            "codex".into(),
            None,
            None,
            Some(skill.into()),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(started["status"], "started");
        let pipe = setup_bridge::name(&id).expect("real setup bridge");
        std::fs::write(root.join("pipe"), pipe).unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        while !root.join("waiting").exists() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        assert!(
            root.join("waiting").exists(),
            "fake CLI did not request human login"
        );
        assert!(browser.waiting_for_login(&id).await);
        if std::env::var_os("MOMSO_G75_EXTERNAL").is_some() {
            browser
                .install_fake_external_login(&id, completed.clone())
                .await;
            assert_eq!(
                panel_agent_external_login_open(handle.clone(), handle.state(), id.clone())
                    .await
                    .unwrap()["state"],
                "login"
            );
            let notify: browser_runtime::Notify = Arc::new(|_, _| {});
            assert_eq!(
                browser.resume_after_login(&id, &notify).await,
                Err("login_incomplete")
            );
        }
        completed.store(true, Ordering::Release);
        let first_app = handle.clone();
        let first_id = id.clone();
        let first = tokio::spawn(async move {
            panel_agent_resume(first_app.clone(), first_app.state(), first_id).await
        });
        let lock_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while browser.resume_lock.try_lock().is_ok() && tokio::time::Instant::now() < lock_deadline
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            browser.resume_lock.try_lock().is_err(),
            "resume did not begin waiting"
        );
        let duplicate = panel_agent_resume(handle.clone(), handle.state(), id.clone()).await;
        assert_eq!(duplicate, Err("in_progress"));
        std::fs::write(root.join("release"), "ok").unwrap();
        let result = tokio::time::timeout(Duration::from_secs(60), first)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(result["state"], "started");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        while handle
            .state::<AppState>()
            .chat_active
            .lock()
            .await
            .contains_key(&id)
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let turns = std::fs::read_to_string(root.join("turns")).unwrap();
        let turns: Vec<_> = turns
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect();
        assert_eq!(turns.len(), 2);
        let text = turns[1]["params"]["input"][0]["text"].as_str().unwrap();
        assert!(text.contains("로그인을 마쳤습니다. 이어서 해 주세요."));
        assert!(text.contains(skill));
        let detail = serde_json::to_value(chat::read(&db, &id).unwrap()).unwrap();
        assert!(detail["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["text"] == "로그인을 마쳤습니다. 이어서 해 주세요." && m["skill"] == skill));
    });
}

#[test]
fn fake_external_login_resumes_one_scoped_turn() {
    let status = crate::hidden_process::hidden_command(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "g70_login_resume_test::fake_cli_login_finished_resume_sends_one_scoped_turn",
            "--nocapture",
        ])
        .env("MOMSO_G75_EXTERNAL", "1")
        .status()
        .unwrap();
    assert!(status.success());
}
