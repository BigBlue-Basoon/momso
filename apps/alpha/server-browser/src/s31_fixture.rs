//! Synthetic administrator workflow, not a copy of any provider's site.
use crate::{
    engine::BrowserPermission,
    tools::{ApprovalChannel, BrowserTools, CredentialError, CredentialSource},
    LoginSecret, ServerBrowsers,
};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

struct Approval {
    asks: Arc<AtomicUsize>,
    allow: bool,
}
impl ApprovalChannel for Approval {
    fn approve_site(&mut self, _: &str, _: &str) -> bool {
        self.asks.fetch_add(1, Ordering::SeqCst);
        self.allow
    }
    fn approve_login(&mut self, _: &str, _: &str) -> bool {
        false
    }
    fn human_login(&mut self, _: &str, _: &str) {}
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

fn read(tools: &mut BrowserTools<Approval, NoCredentials>, chat: &str) -> Value {
    tools.dispatch(
        chat,
        BrowserPermission::Automatic,
        "browser_read",
        json!({}),
    )
}
fn element(page: &Value, name: &str) -> String {
    page["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == name)
        .unwrap_or_else(|| panic!("fixture element absent: {name}"))
        .get("id")
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}
fn click(
    tools: &mut BrowserTools<Approval, NoCredentials>,
    chat: &str,
    page: &Value,
    name: &str,
    mode: BrowserPermission,
) -> Value {
    tools.dispatch(
        chat,
        mode,
        "browser_click",
        json!({"elementId":element(page, name)}),
    )
}
fn wait_for(
    tools: &mut BrowserTools<Approval, NoCredentials>,
    chat: &str,
    expected: &str,
) -> Value {
    for _ in 0..120 {
        let page = read(tools, chat);
        if page["text"]
            .as_str()
            .is_some_and(|text| text.contains(expected))
        {
            return page;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("fixture did not display expected state: {expected}")
}

#[test]
#[ignore = "requires isolated headless Chromium and a loopback fixture"]
fn s31_fake_login_schedule_and_approved_class_registration() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let writes = Arc::new(AtomicUsize::new(0));
    let name_verified = Arc::new(AtomicBool::new(false));
    let (server_stop, server_writes) = (stop.clone(), writes.clone());
    let server_name_verified = name_verified.clone();
    let server = thread::spawn(move || {
        while !server_stop.load(Ordering::Acquire) {
            let Ok((mut stream, _)) = listener.accept() else {
                thread::sleep(Duration::from_millis(10));
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let mut data = [0u8; 8192];
            let n = stream.read(&mut data).unwrap_or(0);
            let request = String::from_utf8_lossy(&data[..n]);
            let body = if request.starts_with("GET /login ") {
                "<html><title>시험 로그인</title><button onclick=\"location.href='/schedule'\">로그인</button></html>"
            } else if request.starts_with("GET /schedule ") {
                "<html><title>스케줄러</title><body><p>오늘 시간표: 가상 수업 10:00</p><button onclick=\"location.href='/classes'\">그룹수업 관리</button></body></html>"
            } else if request.starts_with("GET /classes ") {
                "<html><title>그룹수업 관리</title><body><button onclick=\"location.href='/new'\">신규수업 생성</button></body></html>"
            } else if request.starts_with("GET /new ") {
                "<html><title>신규수업 생성</title><body><input placeholder='수업 이름'><button onclick=\"document.querySelector('#confirm').hidden=false\">확인</button><div id='confirm' hidden>등록 전 확인<button onclick=\"setTimeout(()=>fetch('/register?name='+encodeURIComponent(document.querySelector('input').value),{method:'POST'}).then(()=>document.querySelector('#result').textContent='등록 완료'),200)\">등록</button></div><p id='result'></p></body></html>"
            } else if request.starts_with("POST /register?") {
                server_writes.fetch_add(1, Ordering::SeqCst);
                let path = request.split_whitespace().nth(1).unwrap_or("");
                let name = url::Url::parse(&format!("http://fixture{path}"))
                    .ok()
                    .and_then(|url| {
                        url.query_pairs()
                            .find(|(key, _)| key == "name")
                            .map(|(_, value)| value.into_owned())
                    });
                server_name_verified.store(name.as_deref() == Some("가상 요가"), Ordering::SeqCst);
                "ok"
            } else {
                "not found"
            };
            let _ = write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
        }
    });
    let root = tempfile::tempdir().unwrap();
    let executable = std::env::var_os("MOMSO_TEST_CHROMIUM").expect("isolated browser");
    let asks = Arc::new(AtomicUsize::new(0));
    let mut tools = BrowserTools::new(
        ServerBrowsers::new(executable.into(), root.path().join("profiles")),
        Approval {
            asks: asks.clone(),
            allow: true,
        },
        NoCredentials,
    );
    let chat = "s31-fixture";
    let page = tools.dispatch(
        chat,
        BrowserPermission::Automatic,
        "browser_navigate",
        json!({"url":format!("http://{address}/login")}),
    );
    assert_eq!(page["status"], "page");
    assert_eq!(
        click(
            &mut tools,
            chat,
            &page,
            "로그인",
            BrowserPermission::Automatic
        )["status"],
        "ok"
    );
    let schedule = wait_for(&mut tools, chat, "가상 수업 10:00");
    assert_eq!(
        click(
            &mut tools,
            chat,
            &schedule,
            "그룹수업 관리",
            BrowserPermission::Automatic
        )["status"],
        "ok"
    );
    let classes = wait_for(&mut tools, chat, "신규수업 생성");
    assert_eq!(
        click(
            &mut tools,
            chat,
            &classes,
            "신규수업 생성",
            BrowserPermission::Automatic
        )["status"],
        "ok"
    );
    let form = wait_for(&mut tools, chat, "확인");
    assert_eq!(
        tools.dispatch(
            chat,
            BrowserPermission::Automatic,
            "browser_fill",
            json!({"elementId":element(&form,"수업 이름"),"text":"가상 요가"})
        )["status"],
        "ok"
    );
    assert_eq!(
        click(
            &mut tools,
            chat,
            &form,
            "확인",
            BrowserPermission::Automatic
        )["status"],
        "ok"
    );
    let confirm = wait_for(&mut tools, chat, "등록 전 확인");
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert_eq!(
        click(&mut tools, chat, &confirm, "등록", BrowserPermission::Ask)["status"],
        "ok"
    );
    thread::sleep(Duration::from_secs(1));
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    assert!(name_verified.load(Ordering::SeqCst));
    assert!(read(&mut tools, chat)["text"]
        .as_str()
        .unwrap()
        .contains("등록 완료"));
    // A separate chat's denial must not send a registration request.
    tools.approval.allow = false;
    let page = tools.dispatch(
        "denied-chat",
        BrowserPermission::Automatic,
        "browser_navigate",
        json!({"url":format!("http://{address}/new")}),
    );
    assert_eq!(
        tools.dispatch(
            "denied-chat",
            BrowserPermission::Automatic,
            "browser_fill",
            json!({"elementId":element(&page,"수업 이름"),"text":"가상 요가"})
        )["status"],
        "ok"
    );
    assert_eq!(
        click(
            &mut tools,
            "denied-chat",
            &page,
            "확인",
            BrowserPermission::Automatic
        )["status"],
        "ok"
    );
    let confirm = wait_for(&mut tools, "denied-chat", "등록 전 확인");
    assert_eq!(
        click(
            &mut tools,
            "denied-chat",
            &confirm,
            "등록",
            BrowserPermission::Ask
        )["status"],
        "denied"
    );
    thread::sleep(Duration::from_secs(1));
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    assert_eq!(asks.load(Ordering::SeqCst), 2);
    drop(tools);
    stop.store(true, Ordering::Release);
    server.join().unwrap();
}
