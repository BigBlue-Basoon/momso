//! Fake CLI, fake site and local CRM store; never connects to a real service.
use crate::{
    engine::BrowserPermission,
    tools::{ApprovalChannel, BrowserTools, CredentialError, CredentialSource},
    LoginSecret, ServerBrowsers,
};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

struct Host;
impl ApprovalChannel for Host {
    fn approve_site(&mut self, _: &str, _: &str) -> bool {
        true
    }
    fn approve_login(&mut self, _: &str, _: &str) -> bool {
        false
    }
    fn human_login(&mut self, _: &str, _: &str) {}
}
impl momso_server_crm::ApprovalChannel for Host {
    fn approve_write(&mut self, _: &str, _: &str, _: Option<usize>) -> bool {
        true
    }
    fn is_cancelled(&self) -> bool {
        false
    }
}
struct NoCredentials;
impl CredentialSource for NoCredentials {
    fn protected_values(&mut self) -> Result<Vec<String>, CredentialError> {
        Ok(vec![])
    }
    fn login_for_origin(
        &mut self,
        _: &str,
        _: &str,
    ) -> Result<Option<LoginSecret>, CredentialError> {
        Ok(None)
    }
}

#[test]
fn s33_fake_cli_child() {
    let Ok(role) = std::env::var("MOMSO_S33_ROLE") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("MOMSO_S33_ROOT").unwrap());
    let store = momso_server_crm::CrmStore::new(&root.join("data"), &root.join("archive")).unwrap();
    let mut approval = Host;
    let result = match role.as_str() {
        "collect" => {
            let executable = PathBuf::from(std::env::var_os("MOMSO_TEST_CHROMIUM").unwrap());
            let mut tools = BrowserTools::new(
                ServerBrowsers::new(executable, root.join("profiles")),
                Host,
                NoCredentials,
            );
            let page = tools.dispatch(
                "fake-chat",
                BrowserPermission::Automatic,
                "browser_navigate",
                json!({"url":std::env::var("MOMSO_S33_URL").unwrap()}),
            );
            assert_eq!(page["status"], "page");
            let text = page["text"].as_str().unwrap();
            let field = |prefix: &str| {
                text.lines()
                    .find_map(|line| line.strip_prefix(prefix))
                    .expect("fixture field")
                    .trim()
                    .to_owned()
            };
            let record = json!({"kind":"class","source":{"service":"fixture-crm","externalId":field("번호:"),"observedAt":field("관찰:")},"data":{"name":field("수업:"),"startsAt":field("시작:"),"endsAt":null,"instructor":null,"capacity":null,"reservationCount":null}});
            store.dispatch(
                "fake-chat",
                "momso_crm_upsert",
                &json!({"records":[record]}),
                &mut approval,
            )
        }
        "answer" => store.dispatch(
            "fake-chat",
            "momso_crm_search",
            &json!({"kind":"class","service":"fixture-crm"}),
            &mut approval,
        ),
        _ => panic!("invalid fake CLI role"),
    };
    println!("S33_RESULT:{}", result);
}

#[test]
#[ignore = "requires isolated headless Chromium and a loopback fixture"]
fn fake_cli_collects_schedule_then_answers_offline_from_crm() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let requests = Arc::new(AtomicUsize::new(0));
    let (end, counter) = (stop.clone(), requests.clone());
    let server = thread::spawn(move || {
        while !end.load(Ordering::Acquire) {
            let Ok((mut stream, _)) = listener.accept() else {
                thread::sleep(Duration::from_millis(10));
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let mut input = [0; 4096];
            let _ = stream.read(&mut input);
            counter.fetch_add(1, Ordering::SeqCst);
            let body = "<html><title>가짜 시간표</title><body><p>번호:lesson-101</p><p>수업:가상 요가</p><p>시작:2026-11-21T10:00:00+09:00</p><p>관찰:2026-11-21T09:00:00+09:00</p></body></html>";
            let _ = write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
        }
    });
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("data")).unwrap();
    std::fs::create_dir(temp.path().join("archive")).unwrap();
    let run = |role: &str| -> Value {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .env_clear()
            .arg("--exact")
            .arg("s33_fixture::s33_fake_cli_child")
            .arg("--nocapture");
        for name in ["PATH", "LANG", "LC_ALL", "TMPDIR", "MOMSO_TEST_CHROMIUM"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let output = command
            .env("MOMSO_S33_ROLE", role)
            .env("MOMSO_S33_ROOT", temp.path())
            .env("MOMSO_S33_URL", format!("http://{address}/schedule"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "fake CLI failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let line = stdout
            .lines()
            .find_map(|line| line.split_once("S33_RESULT:").map(|(_, value)| value))
            .expect("fake CLI result");
        serde_json::from_str(line).unwrap()
    };
    assert_eq!(run("collect")["status"], "ok");
    thread::sleep(Duration::from_millis(250));
    let before = requests.load(Ordering::SeqCst);
    assert!(before > 0);
    let answer = run("answer");
    assert_eq!(answer["records"][0]["data"]["name"], "가상 요가");
    assert_eq!(answer["records"][0]["source"]["externalId"], "lesson-101");
    assert_eq!(requests.load(Ordering::SeqCst), before);
    stop.store(true, Ordering::Release);
    server.join().unwrap();
}
