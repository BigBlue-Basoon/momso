use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
};

fn request(mut s: &std::net::TcpStream) -> String {
    s.set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let mut buf = Vec::new();
    let mut temp = [0u8; 8192];
    loop {
        let n = match s.read(&mut temp) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buf.extend_from_slice(&temp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..i]).to_ascii_lowercase();
            let len = head
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length:")
                        .and_then(|x| x.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if buf.len() >= i + 4 + len {
                break;
            }
        }
    }
    String::from_utf8(buf).unwrap()
}
fn reply(mut s: std::net::TcpStream, body: &str) {
    write!(s,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
}

struct FakeHost {
    allow: bool,
    approved: Arc<AtomicBool>,
    site: String,
    calls: usize,
}
impl ToolHost for FakeHost {
    fn definitions(&self) -> Vec<Value> {
        vec![
            json!({"name":"browser_click","description":"click","inputSchema":{"type":"object","properties":{"elementId":{"type":"string"}},"required":["elementId"]}}),
        ]
    }
    fn dispatch<'a>(
        &'a mut self,
        ctx: &'a Context<'_>,
        name: &'a str,
        args: Value,
        _cancel: watch::Receiver<bool>,
    ) -> ToolFuture<'a> {
        Box::pin(async move {
            assert_eq!(
                (ctx.chat_id, ctx.turn_id, name),
                ("chat", "turn", "browser_click")
            );
            assert_eq!(args["elementId"], "register");
            self.calls += 1;
            if !self.allow {
                return Ok(json!({"status":"denied"}));
            }
            self.approved.store(true, Ordering::SeqCst);
            let mut s = std::net::TcpStream::connect(&self.site).unwrap();
            s.write_all(b"POST /register HTTP/1.1\r\nHost: fixture\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            let mut data = String::new();
            s.read_to_string(&mut data).unwrap();
            Ok(json!({"status":"ok","result":"registered"}))
        })
    }
}

#[tokio::test]
async fn fake_cli_api_site_approval_and_answer_for_both_providers() {
    for provider in [Provider::Claude, Provider::Gemini] {
        let approved = Arc::new(AtomicBool::new(false));
        let writes = Arc::new(AtomicUsize::new(0));
        let site = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = site.local_addr().unwrap();
        let (flag, count) = (approved.clone(), writes.clone());
        let site_worker = thread::spawn(move || {
            let (s, _) = site.accept().unwrap();
            let req = request(&s);
            assert!(req.starts_with("POST /register"));
            assert!(flag.load(Ordering::SeqCst));
            count.fetch_add(1, Ordering::SeqCst);
            reply(s, "ok");
        });
        let api = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", api.local_addr().unwrap());
        let api_worker = thread::spawn(move || {
            let (s, _) = api.accept().unwrap();
            let first = request(&s);
            assert!(first.contains("browser_click"));
            let call = match provider {
                Provider::Claude => {
                    json!({"content":[{"type":"tool_use","id":"call1","name":"browser_click","input":{"elementId":"register"}}],"stop_reason":"tool_use"})
                }
                Provider::Gemini => {
                    json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"browser_click","args":{"elementId":"register"}},"thoughtSignature":"fixture-signature"}]},"finishReason":"STOP"}]})
                }
            };
            reply(s, &call.to_string());
            let (s, _) = api.accept().unwrap();
            let second = request(&s);
            assert!(second.contains("registered"));
            if provider == Provider::Gemini {
                assert!(second.contains("fixture-signature"));
            }
            let final_answer = match provider {
                Provider::Claude => {
                    json!({"content":[{"type":"text","text":"등록 완료"}],"stop_reason":"end_turn"})
                }
                Provider::Gemini => {
                    json!({"candidates":[{"content":{"parts":[{"text":"등록 완료"}]},"finishReason":"STOP"}]})
                }
            };
            reply(s, &final_answer.to_string());
        });
        let fake_cli = std::process::Command::new("/bin/sh")
            .args(["-c", "printf 'register a class'"])
            .env_clear()
            .output()
            .unwrap();
        assert!(fake_cli.status.success());
        let messages = vec![Message {
            role: Role::User,
            text: String::from_utf8(fake_cli.stdout).unwrap(),
            attachments: Vec::new(),
        }];
        let key = ApiKey::new("fake-key".into()).unwrap();
        let (_tx, rx) = watch::channel(false);
        let mut host = FakeHost {
            allow: true,
            approved: approved.clone(),
            site: addr.to_string(),
            calls: 0,
        };
        let mut result = String::new();
        run_at(
            ToolTurn {
                provider,
                model: "fake",
                messages: &messages,
                key: &key,
                context: Context {
                    chat_id: "chat",
                    turn_id: "turn",
                },
            },
            &mut host,
            rx,
            |s, _| result.push_str(s),
            &url,
        )
        .await
        .unwrap();
        assert_eq!(result, "등록 완료");
        assert_eq!(host.calls, 1);
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        api_worker.join().unwrap();
        site_worker.join().unwrap();
    }
}

#[test]
fn schemas_and_unknown_tool_names_are_checked() {
    let host = FakeHost {
        allow: false,
        approved: Arc::new(AtomicBool::new(false)),
        site: String::new(),
        calls: 0,
    };
    let (claude, names) = definitions(Provider::Claude, &host.definitions()).unwrap();
    assert!(claude[0].get("input_schema").is_some());
    assert!(!names.contains("shell"));
    let (gemini, _) = definitions(Provider::Gemini, &host.definitions()).unwrap();
    assert!(gemini[0].get("parametersJsonSchema").is_some());
}

#[tokio::test]
async fn gemini_function_without_args_uses_empty_object() {
    struct Empty;
    impl ToolHost for Empty {
        fn definitions(&self) -> Vec<Value> {
            vec![
                json!({"name":"browser_open","description":"open","inputSchema":{"type":"object","properties":{}}}),
            ]
        }
        fn dispatch<'a>(
            &'a mut self,
            _: &'a Context<'_>,
            name: &'a str,
            args: Value,
            _: watch::Receiver<bool>,
        ) -> ToolFuture<'a> {
            Box::pin(async move {
                assert_eq!(name, "browser_open");
                assert_eq!(args, json!({}));
                Ok(json!({"status":"ok"}))
            })
        }
    }
    let api = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", api.local_addr().unwrap());
    let worker = thread::spawn(move || {
        let (s, _) = api.accept().unwrap();
        let _ = request(&s);
        reply(s,&json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"browser_open"}}]},"finishReason":"STOP"}]}).to_string());
        let (s, _) = api.accept().unwrap();
        assert!(request(&s).contains("functionResponse"));
        reply(
            s,
            &json!({"candidates":[{"content":{"parts":[{"text":"done"}]},"finishReason":"STOP"}]})
                .to_string(),
        );
    });
    let key = ApiKey::new("fake-key".into()).unwrap();
    let messages = vec![Message {
        role: Role::User,
        text: "open".into(),
        attachments: Vec::new(),
    }];
    let (_tx, rx) = watch::channel(false);
    run_at(
        ToolTurn {
            provider: Provider::Gemini,
            model: "fake",
            messages: &messages,
            key: &key,
            context: Context {
                chat_id: "chat",
                turn_id: "turn",
            },
        },
        &mut Empty,
        rx,
        |_, _| {},
        &url,
    )
    .await
    .unwrap();
    worker.join().unwrap();
}

#[tokio::test]
async fn denied_tool_call_never_reaches_the_site() {
    for provider in [Provider::Claude, Provider::Gemini] {
        let api = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", api.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let (s, _) = api.accept().unwrap();
            let _ = request(&s);
            let call = match provider {
                Provider::Claude => {
                    json!({"content":[{"type":"tool_use","id":"call1","name":"browser_click","input":{"elementId":"register"}}],"stop_reason":"tool_use"})
                }
                Provider::Gemini => {
                    json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"browser_click","args":{"elementId":"register"}}}]},"finishReason":"STOP"}]})
                }
            };
            reply(s, &call.to_string());
            let (s, _) = api.accept().unwrap();
            assert!(request(&s).contains("denied"));
            let done = match provider {
                Provider::Claude => {
                    json!({"content":[{"type":"text","text":"denied"}],"stop_reason":"end_turn"})
                }
                Provider::Gemini => {
                    json!({"candidates":[{"content":{"parts":[{"text":"denied"}]},"finishReason":"STOP"}]})
                }
            };
            reply(s, &done.to_string());
        });
        let flag = Arc::new(AtomicBool::new(false));
        let mut host = FakeHost {
            allow: false,
            approved: flag.clone(),
            site: "127.0.0.1:1".into(),
            calls: 0,
        };
        let key = ApiKey::new("fake-key".into()).unwrap();
        let messages = vec![Message {
            role: Role::User,
            text: "test".into(),
            attachments: Vec::new(),
        }];
        let (_tx, rx) = watch::channel(false);
        let mut answer = String::new();
        run_at(
            ToolTurn {
                provider,
                model: "fake",
                messages: &messages,
                key: &key,
                context: Context {
                    chat_id: "chat",
                    turn_id: "turn",
                },
            },
            &mut host,
            rx,
            |s, _| answer.push_str(s),
            &url,
        )
        .await
        .unwrap();
        worker.join().unwrap();
        assert_eq!(answer, "denied");
        assert!(!flag.load(Ordering::SeqCst));
    }
}

#[cfg(target_os = "linux")]
mod real_browser {
    use super::*;
    use momso_server_browser::{
        engine::BrowserPermission,
        tools::{ApprovalChannel, BrowserTools, CredentialError, CredentialSource},
        LoginSecret, ServerBrowsers,
    };
    use std::time::Duration;
    struct Approval(Arc<AtomicBool>);
    impl ApprovalChannel for Approval {
        fn approve_site(&mut self, _: &str, _: &str) -> bool {
            self.0.store(true, Ordering::SeqCst);
            true
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
    struct Host {
        tools: BrowserTools<Approval, NoCredentials>,
    }
    impl ToolHost for Host {
        fn definitions(&self) -> Vec<Value> {
            let mut v = momso_server_browser::tools::tool_definitions()
                .as_array()
                .unwrap()
                .clone();
            v.extend(
                momso_server_crm::tool_definitions()
                    .as_array()
                    .unwrap()
                    .clone(),
            );
            for name in ["momso_skill_read", "momso_dictionary_search"] {
                v.push(json!({"name":name,"description":"host-provided","inputSchema":{"type":"object","properties":{}}}));
            }
            v
        }
        fn dispatch<'a>(
            &'a mut self,
            ctx: &'a Context<'_>,
            name: &'a str,
            args: Value,
            cancel: watch::Receiver<bool>,
        ) -> ToolFuture<'a> {
            Box::pin(async move {
                if *cancel.borrow() {
                    return Err("cancelled");
                }
                Ok(self
                    .tools
                    .dispatch(ctx.chat_id, BrowserPermission::Ask, name, args))
            })
        }
    }
    fn call(provider: Provider, name: &str, args: Value) -> Value {
        match provider {
            Provider::Claude => {
                json!({"content":[{"type":"tool_use","id":format!("call-{name}"),"name":name,"input":args}],"stop_reason":"tool_use"})
            }
            Provider::Gemini => {
                json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":name,"args":args},"thoughtSignature":"sig"}]},"finishReason":"STOP"}]})
            }
        }
    }
    fn body(req: &str) -> Value {
        serde_json::from_str(req.split_once("\r\n\r\n").unwrap().1).unwrap()
    }
    fn result(v: &Value, provider: Provider) -> Value {
        match provider {
            Provider::Claude => serde_json::from_str(
                v["messages"].as_array().unwrap().last().unwrap()["content"][0]["content"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap(),
            Provider::Gemini => v["contents"].as_array().unwrap().last().unwrap()["parts"][0]
                ["functionResponse"]["response"]["result"]
                .clone(),
        }
    }
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires isolated headless Chromium and loopback fixtures"]
    async fn both_providers_approved_registration_with_s22_browser() {
        for provider in [Provider::Claude, Provider::Gemini] {
            let root = tempfile::tempdir().unwrap();
            let approved = Arc::new(AtomicBool::new(false));
            let writes = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let site = TcpListener::bind("127.0.0.1:0").unwrap();
            site.set_nonblocking(true).unwrap();
            let site_url = format!("http://{}/", site.local_addr().unwrap());
            let (flag, count, stop2) = (approved.clone(), writes.clone(), stop.clone());
            let site_worker = thread::spawn(move || {
                while !stop2.load(Ordering::SeqCst) {
                    if let Ok((mut s, _)) = site.accept() {
                        let req = request(&s);
                        if req.starts_with("POST /register") {
                            assert!(flag.load(Ordering::SeqCst));
                            count.fetch_add(1, Ordering::SeqCst);
                            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
                        } else {
                            let html="<html><button onclick=\"document.getElementById('r').textContent='registered';fetch('/register',{method:'POST'})\">Register</button><p id='r'></p></html>";
                            let _ = write!(s,"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",html.len(),html);
                        }
                    } else {
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            });
            let api = TcpListener::bind("127.0.0.1:0").unwrap();
            let api_url = format!("http://{}", api.local_addr().unwrap());
            let api_worker = thread::spawn(move || {
                let (s, _) = api.accept().unwrap();
                let first = body(&request(&s));
                assert!(first.to_string().contains("momso_crm_search"));
                assert!(first.to_string().contains("momso_skill_read"));
                reply(
                    s,
                    &call(provider, "browser_navigate", json!({"url":site_url})).to_string(),
                );
                let (s, _) = api.accept().unwrap();
                let second = body(&request(&s));
                let page = result(&second, provider);
                assert_eq!(page["status"], "page");
                let id = page["elements"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["name"] == "Register")
                    .unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                reply(
                    s,
                    &call(provider, "browser_click", json!({"elementId":id})).to_string(),
                );
                let (s, _) = api.accept().unwrap();
                let third = body(&request(&s));
                assert_eq!(result(&third, provider)["status"], "ok");
                reply(s, &call(provider, "browser_read", json!({})).to_string());
                let (s, _) = api.accept().unwrap();
                let fourth = body(&request(&s));
                assert!(result(&fourth, provider)["text"]
                    .as_str()
                    .unwrap()
                    .contains("registered"));
                let done = match provider {
                    Provider::Claude => {
                        json!({"content":[{"type":"text","text":"registered"}],"stop_reason":"end_turn"})
                    }
                    Provider::Gemini => {
                        json!({"candidates":[{"content":{"parts":[{"text":"registered"}]},"finishReason":"STOP"}]})
                    }
                };
                reply(s, &done.to_string());
            });
            let binary = std::env::var_os("MOMSO_TEST_CHROMIUM").expect("isolated Chromium");
            let mut host = Host {
                tools: BrowserTools::new(
                    ServerBrowsers::new(binary.into(), root.path().join("profiles")),
                    Approval(approved.clone()),
                    NoCredentials,
                ),
            };
            let key = ApiKey::new("fake-key".into()).unwrap();
            let fake_cli = std::process::Command::new("/bin/sh")
                .args(["-c", "printf 'Register a class'"])
                .env_clear()
                .output()
                .unwrap();
            assert!(fake_cli.status.success());
            let messages = vec![Message {
                role: Role::User,
                text: String::from_utf8(fake_cli.stdout).unwrap(),
                attachments: Vec::new(),
            }];
            let (_tx, rx) = watch::channel(false);
            let mut answer = String::new();
            run_at(
                ToolTurn {
                    provider,
                    model: "fake",
                    messages: &messages,
                    key: &key,
                    context: Context {
                        chat_id: "chat",
                        turn_id: "turn",
                    },
                },
                &mut host,
                rx,
                |s, _| answer.push_str(s),
                &api_url,
            )
            .await
            .unwrap();
            api_worker.join().unwrap();
            for _ in 0..500 {
                if writes.load(Ordering::SeqCst) > 0 {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(writes.load(Ordering::SeqCst), 1);
            assert_eq!(answer, "registered");
            stop.store(true, Ordering::SeqCst);
            site_worker.join().unwrap();
        }
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn momso_router_forwards_all_tool_families_to_the_host() {
    struct Routes;
    impl MomsoDispatch for Routes {
        fn browser<'a>(
            &'a mut self,
            ctx: &'a Context<'_>,
            _: &'a str,
            _: Value,
            _: watch::Receiver<bool>,
        ) -> ToolFuture<'a> {
            Box::pin(async move {
                assert_eq!(ctx.chat_id, "chat");
                Ok(json!({"route":"browser"}))
            })
        }
        fn crm<'a>(
            &'a mut self,
            _: &'a Context<'_>,
            _: &'a str,
            _: Value,
            _: watch::Receiver<bool>,
        ) -> ToolFuture<'a> {
            Box::pin(async { Ok(json!({"route":"crm"})) })
        }
        fn skill_or_dictionary<'a>(
            &'a mut self,
            _: &'a Context<'_>,
            _: &'a str,
            _: Value,
            _: watch::Receiver<bool>,
        ) -> ToolFuture<'a> {
            Box::pin(async { Ok(json!({"route":"extra"})) })
        }
    }
    let mut router = MomsoTools { dispatcher: Routes };
    let (_tx, rx) = watch::channel(false);
    let ctx = Context {
        chat_id: "chat",
        turn_id: "turn",
    };
    for (name, expected) in [
        ("browser_read", "browser"),
        ("momso_crm_search", "crm"),
        ("momso_skill_read", "extra"),
        ("momso_dictionary_search", "extra"),
    ] {
        assert_eq!(
            router
                .dispatch(&ctx, name, json!({}), rx.clone())
                .await
                .unwrap()["route"],
            expected
        );
    }
    assert_eq!(
        router
            .dispatch(&ctx, "shell", json!({}), rx)
            .await
            .unwrap_err(),
        "invalid_tool_call"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn crm_route_uses_real_store_and_its_approval() {
    struct Deny;
    impl momso_server_crm::ApprovalChannel for Deny {
        fn approve_write(&mut self, _: &str, _: &str, _: Option<usize>) -> bool {
            false
        }
        fn is_cancelled(&self) -> bool {
            false
        }
    }
    struct Routes(momso_server_crm::CrmStore);
    impl MomsoDispatch for Routes {
        fn browser<'a>(
            &'a mut self,
            _: &'a Context<'_>,
            _: &'a str,
            _: Value,
            _: watch::Receiver<bool>,
        ) -> ToolFuture<'a> {
            Box::pin(async { Err("unavailable") })
        }
        fn crm<'a>(
            &'a mut self,
            ctx: &'a Context<'_>,
            name: &'a str,
            args: Value,
            _: watch::Receiver<bool>,
        ) -> ToolFuture<'a> {
            Box::pin(async move { Ok(self.0.dispatch(ctx.chat_id, name, &args, &mut Deny)) })
        }
        fn skill_or_dictionary<'a>(
            &'a mut self,
            _: &'a Context<'_>,
            _: &'a str,
            _: Value,
            _: watch::Receiver<bool>,
        ) -> ToolFuture<'a> {
            Box::pin(async { Err("unavailable") })
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let archive = tmp.path().join("archive");
    std::fs::create_dir(&data).unwrap();
    std::fs::create_dir(&archive).unwrap();
    let store = momso_server_crm::CrmStore::new(&data, &archive).unwrap();
    let mut tools = MomsoTools {
        dispatcher: Routes(store),
    };
    let (_tx, rx) = watch::channel(false);
    let ctx = Context {
        chat_id: "chat",
        turn_id: "turn",
    };
    let record = json!({"kind":"student","source":{"service":"fixture","externalId":"1","observedAt":"2026-11-20T09:00:00+09:00"},"data":{"name":"fixture"}});
    let denied = tools
        .dispatch(
            &ctx,
            "momso_crm_upsert",
            json!({"records":[record]}),
            rx.clone(),
        )
        .await
        .unwrap();
    assert_eq!(denied["status"], "denied");
    let found = tools
        .dispatch(&ctx, "momso_crm_search", json!({"kind":"student"}), rx)
        .await
        .unwrap();
    assert_eq!(found["records"].as_array().unwrap().len(), 0);
}
