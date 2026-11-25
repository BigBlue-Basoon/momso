//! Host-side browser policy and driver boundary. No production driver is bundled yet.
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    version: u32,
    origins: Vec<String>,
    #[serde(default)]
    login_origins: Vec<String>,
    #[serde(default)]
    login_paths: Vec<String>,
    irreversible: Vec<String>,
    #[serde(default)]
    read_only: Vec<String>,
}

impl Policy {
    /// Host-created general browser policy. The marker is never loaded from a skill.
    pub(crate) fn general() -> Self {
        Self {
            version: 1,
            origins: vec!["*".into()],
            login_origins: vec![],
            login_paths: vec![],
            irreversible: vec![],
            read_only: vec!["browser_navigate".into(), "browser_read".into()],
        }
    }

    pub(crate) fn is_general(&self) -> bool {
        self.version == 1 && self.origins.len() == 1 && self.origins[0] == "*"
    }

    pub(crate) fn allows(&self, target: &str) -> bool {
        let Ok(url) = reqwest::Url::parse(target) else {
            return false;
        };
        let test_loopback = cfg!(test)
            && url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.username().is_empty()
            && url.password().is_none();
        if self.is_general() {
            return test_loopback || crate::external_url::validate(target).is_ok();
        }
        if (url.scheme() != "https" && !test_loopback)
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return false;
        }
        let value = url.origin().ascii_serialization();
        self.origins
            .iter()
            .chain(&self.login_origins)
            .any(|origin| origin == &value)
    }
    pub(crate) fn origins(&self) -> &[String] {
        &self.origins
    }
    pub(crate) fn login_origins(&self) -> &[String] {
        &self.login_origins
    }
    pub(crate) fn login_paths(&self) -> &[String] {
        &self.login_paths
    }
    pub(crate) fn allows_read_only(&self, method: &str) -> bool {
        matches!(method, "browser_navigate" | "browser_read")
            && self.read_only.iter().any(|allowed| allowed == method)
    }
    pub(crate) fn allows_action(&self, action: &str) -> bool {
        self.irreversible.iter().any(|allowed| allowed == action)
    }
    #[cfg(test)]
    pub(crate) fn test_loopback(origin: String) -> Self {
        Self {
            version: 1,
            origins: vec![origin],
            login_origins: vec![],
            login_paths: vec![],
            irreversible: vec!["게시".into()],
            read_only: vec!["browser_navigate".into(), "browser_read".into()],
        }
    }
    #[cfg(test)]
    pub(crate) fn add_test_login_path(&mut self, path: String) {
        self.login_paths.push(path);
    }
    pub(crate) fn effect(&self, name: &str) -> &'static str {
        let value = name.to_lowercase();
        if value.contains("게시") || value.contains("등록") || value.contains("publish") {
            "publish"
        } else if value.contains("삭제") || value.contains("delete") {
            "delete"
        } else if value.contains("알림") || value.contains("발송") || value.contains("send") {
            "notify"
        } else if value.contains("환불") || value.contains("refund") {
            "refund"
        } else if value.contains("결제") || value.contains("pay") {
            "payment"
        } else if self.irreversible.iter().any(|word| value.contains(word)) {
            "irreversible"
        } else {
            "unknown"
        }
    }
}
pub(crate) fn builtin_browser_name(name: &str) -> bool {
    matches!(
        name,
        "bodycodi-operations"
            | "naver-smartplace"
            | "onstudio-operations"
            | "studiomate-operations"
            | "soomgo-operations"
            | "frip-host-operations"
            | "sssd-host-operations"
    )
}

#[cfg(test)]
#[derive(Clone, Debug)]
struct Element {
    id: String,
    name: String,
    kind: String,
}
#[cfg(test)]
#[derive(Clone, Debug)]
struct PageState {
    origin: String,
    title: String,
    text: String,
    elements: Vec<Element>,
}
#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
enum DriverError {
    Blocked,
    HumanRequired(&'static str),
    TargetChanged,
    Refused,
}
#[cfg(test)]
trait Driver {
    fn navigate(&mut self, url: &str) -> Result<PageState, DriverError>;
    fn read(&mut self) -> Result<PageState, DriverError>;
    fn click(&mut self, id: &str, expected_name: &str) -> Result<(), DriverError>;
    fn fill(&mut self, id: &str, text: &str) -> Result<(), DriverError>;
    fn close(&mut self);
}
#[cfg(test)]
struct Gateway<D: Driver> {
    policy: Policy,
    driver: D,
}
#[cfg(test)]
impl<D: Driver> Gateway<D> {
    fn navigate(&mut self, url: &str) -> Result<PageState, DriverError> {
        if !self.policy.allows(url) {
            return Err(DriverError::Blocked);
        }
        let page = self.driver.navigate(url)?;
        if !self.policy.allows(&page.origin) {
            return Err(DriverError::Blocked);
        }
        Ok(page)
    }
    fn read(&mut self) -> Result<PageState, DriverError> {
        let page = self.driver.read()?;
        if !self.policy.allows(&page.origin) {
            return Err(DriverError::Blocked);
        }
        if page.elements.iter().any(|e| e.kind == "password") {
            return Err(DriverError::HumanRequired("password"));
        }
        if page.elements.iter().any(|e| e.kind == "one_time_code") {
            return Err(DriverError::HumanRequired("one_time_code"));
        }
        Ok(page)
    }
    // All clicks and fills require one approval, even when a policy term is not matched.
    // This avoids trusting a model's claim about whether a button is harmless.
    fn click(
        &mut self,
        id: &str,
        approve: impl FnOnce(&str, &str) -> bool,
    ) -> Result<(), DriverError> {
        let page = self.read()?;
        let element = page
            .elements
            .iter()
            .find(|e| e.id == id)
            .ok_or(DriverError::TargetChanged)?;
        if !approve(&page.origin, &element.name) {
            return Err(DriverError::Refused);
        }
        self.driver.click(id, &element.name)
    }
    async fn click_with_approval<F, Fut>(&mut self, id: &str, approve: F) -> Result<(), DriverError>
    where
        F: FnOnce(String, String) -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let page = self.read()?;
        let element = page
            .elements
            .iter()
            .find(|e| e.id == id)
            .ok_or(DriverError::TargetChanged)?;
        let name = element.name.clone();
        if !approve(page.origin, name.clone()).await {
            return Err(DriverError::Refused);
        }
        self.driver.click(id, &name)
    }
    fn fill(
        &mut self,
        id: &str,
        text: &str,
        approve: impl FnOnce(&str, &str) -> bool,
    ) -> Result<(), DriverError> {
        let page = self.read()?;
        let element = page
            .elements
            .iter()
            .find(|e| e.id == id)
            .ok_or(DriverError::TargetChanged)?;
        if element.kind != "normal" {
            return Err(DriverError::Refused);
        }
        if !approve(&page.origin, &element.name) {
            return Err(DriverError::Refused);
        }
        self.driver.fill(id, text)
    }
}
#[cfg(test)]
impl<D: Driver> Drop for Gateway<D> {
    fn drop(&mut self) {
        self.driver.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    #[test]
    fn general_policy_allows_public_sites_but_not_local_addresses() {
        let policy = Policy::general();
        assert!(policy.allows("https://crm.bodycodi.com/"));
        assert!(policy.allows("https://other.example/"));
        assert!(policy.allows("http://other.example/"));
        assert!(!policy.allows("http://localhost/"));
        assert!(!policy.allows("http://192.168.1.10/"));
        assert!(!policy.allows("file:///tmp/test"));
        assert!(!policy.allows(concat!("https://user:pass", "@", "other.example/")));
    }

    #[test]
    fn bundled_browser_skills_are_guides_without_site_lists() {
        // Skills only explain how to work; the browser itself is not limited
        // to the sites a skill mentions (product uses `Policy::general`).
        let policy = Policy::general();
        let mut count = 0;
        for &(name, content) in crate::skills::BUNDLED_SKILLS {
            if !builtin_browser_name(name) {
                continue;
            }
            count += 1;
            assert!(
                !content.contains("momso-browser-reference"),
                "{name} still carries a site list"
            );
            assert!(
                content.contains("browser_open"),
                "{name} lacks the tool guide"
            );
        }
        assert_eq!(count, 7);
        for url in [
            "https://smartplace.naver.com/",
            "https://frip.host/",
            "https://soomgo.com/login",
            "https://unrelated.example/",
        ] {
            assert!(policy.allows(url), "{url}");
        }
        assert!(policy.allows_read_only("browser_navigate"));
        assert!(policy.allows_read_only("browser_read"));
        assert!(!policy.allows_read_only("browser_click"));
        assert!(!policy.allows_read_only("browser_fill"));
    }

    #[test]
    fn read_only_policy_blocks_mutations_without_requesting_approval() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let requests = AtomicUsize::new(0);
        let policy = Policy::test_loopback("http://127.0.0.1:1234".into());
        for method in ["browser_navigate", "browser_read"] {
            assert!(policy.allows_read_only(method));
        }
        for method in ["browser_click", "browser_fill"] {
            if policy.allows_read_only(method) {
                requests.fetch_add(1, Ordering::SeqCst);
            }
            assert!(!policy.allows_read_only(method));
        }
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }
    struct FakeDriver {
        page: PageState,
        clicked: Arc<AtomicUsize>,
        closed: Arc<AtomicUsize>,
        login: bool,
        mutate_target: bool,
    }
    impl Driver for FakeDriver {
        fn navigate(&mut self, _url: &str) -> Result<PageState, DriverError> {
            if self.login {
                self.login = false;
                Err(DriverError::HumanRequired("login"))
            } else {
                Ok(self.page.clone())
            }
        }
        fn read(&mut self) -> Result<PageState, DriverError> {
            Ok(self.page.clone())
        }
        fn click(&mut self, id: &str, expected: &str) -> Result<(), DriverError> {
            if self.mutate_target
                || !self
                    .page
                    .elements
                    .iter()
                    .any(|e| e.id == id && e.name == expected)
            {
                return Err(DriverError::TargetChanged);
            }
            self.clicked.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn fill(&mut self, _: &str, _: &str) -> Result<(), DriverError> {
            Ok(())
        }
        fn close(&mut self) {
            self.closed.fetch_add(1, Ordering::SeqCst);
        }
    }
    #[test]
    fn general_policy_handles_fake_login_read_and_denial_on_each_skill_site() {
        let mut count = 0;
        for origin in [
            "https://smartplace.naver.com",
            "https://onstudio.kr",
            "https://studiomate.kr",
            "https://soomgo.com",
            "https://frip.host",
            "https://www.sssd.co.kr",
        ] {
            count += 1;
            let policy = Policy::general();
            let origin = origin.to_owned();
            let clicked = Arc::new(AtomicUsize::new(0));
            let closed = Arc::new(AtomicUsize::new(0));
            let mut gateway = Gateway {
                policy,
                driver: FakeDriver {
                    page: PageState {
                        origin: origin.clone(),
                        title: "가짜 목록".into(),
                        text: "가짜 수업".into(),
                        elements: vec![Element {
                            id: "publish".into(),
                            name: "게시".into(),
                            kind: "normal".into(),
                        }],
                    },
                    clicked: clicked.clone(),
                    closed: closed.clone(),
                    login: true,
                    mutate_target: false,
                },
            };
            assert!(matches!(
                gateway.navigate(&origin),
                Err(DriverError::HumanRequired("login"))
            ));
            assert!(gateway.navigate(&origin).is_ok());
            assert_eq!(gateway.read().unwrap().text, "가짜 수업");
            assert!(!gateway.policy.allows_read_only("browser_click"));
            assert_eq!(
                gateway.click("publish", |_, _| false),
                Err(DriverError::Refused)
            );
            assert_eq!(clicked.load(Ordering::SeqCst), 0);
            assert!(gateway.click("publish", |_, _| true).is_ok());
            assert_eq!(clicked.load(Ordering::SeqCst), 1);
            drop(gateway);
            assert_eq!(closed.load(Ordering::SeqCst), 1);
        }
        assert_eq!(count, 6);
    }
    #[test]
    fn fake_session_human_login_approval_deny_target_change_and_close() {
        let policy = Policy {
            version: 1,
            origins: vec!["https://studio.example".into()],
            login_origins: vec![],
            login_paths: vec![],
            irreversible: vec!["게시".into()],
            read_only: vec!["browser_navigate".into(), "browser_read".into()],
        };
        let clicked = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        let page = PageState {
            origin: "https://studio.example".into(),
            title: "가짜 수업".into(),
            text: "수업 자료".into(),
            elements: vec![Element {
                id: "publish".into(),
                name: "게시".into(),
                kind: "normal".into(),
            }],
        };
        {
            let driver = FakeDriver {
                page,
                clicked: clicked.clone(),
                closed: closed.clone(),
                login: true,
                mutate_target: false,
            };
            let mut gateway = Gateway { policy, driver };
            assert!(matches!(
                gateway.navigate("https://studio.example/"),
                Err(DriverError::HumanRequired("login"))
            ));
            assert!(matches!(
                gateway.navigate("https://evil.example/"),
                Err(DriverError::Blocked)
            ));
            assert!(gateway.navigate("https://studio.example/").is_ok());
            let page = gateway.read().unwrap();
            assert_eq!(
                (&page.title, &page.text),
                (&"가짜 수업".to_owned(), &"수업 자료".to_owned())
            );
            assert_eq!(
                gateway.fill("publish", "sample", |_, _| false),
                Err(DriverError::Refused)
            );
            assert_eq!(
                gateway.click("publish", |_, _| false),
                Err(DriverError::Refused)
            );
            assert_eq!(clicked.load(Ordering::SeqCst), 0);
            assert!(gateway
                .click("publish", |site, name| site == "https://studio.example"
                    && name == "게시")
                .is_ok());
            assert_eq!(clicked.load(Ordering::SeqCst), 1);
            gateway.driver.mutate_target = true;
            assert_eq!(
                gateway.click("publish", |_, _| true),
                Err(DriverError::TargetChanged)
            );
            assert_eq!(clicked.load(Ordering::SeqCst), 1);
        }
        assert_eq!(closed.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn fake_driver_waits_for_real_approval_broker_allow_or_deny() {
        use crate::approval::{ApprovalBroker, ApprovalChoice};
        use tokio::sync::oneshot;
        use tokio_util::sync::CancellationToken;
        for choice in [ApprovalChoice::AllowOnce, ApprovalChoice::Deny] {
            let broker = ApprovalBroker::default();
            let worker = broker.clone();
            let clicked = Arc::new(AtomicUsize::new(0));
            let closed = Arc::new(AtomicUsize::new(0));
            let page = PageState {
                origin: "https://studio.example".into(),
                title: "수업".into(),
                text: "자료".into(),
                elements: vec![Element {
                    id: "publish".into(),
                    name: "게시".into(),
                    kind: "normal".into(),
                }],
            };
            let driver = FakeDriver {
                page,
                clicked: clicked.clone(),
                closed: closed.clone(),
                login: true,
                mutate_target: false,
            };
            let mut gateway = Gateway {
                policy: Policy {
                    version: 1,
                    origins: vec!["https://studio.example".into()],
                    login_origins: vec![],
                    login_paths: vec![],
                    irreversible: vec!["게시".into()],
                    read_only: vec!["browser_navigate".into(), "browser_read".into()],
                },
                driver,
            };
            assert!(matches!(
                gateway.navigate("https://studio.example/"),
                Err(DriverError::HumanRequired("login"))
            ));
            assert!(gateway.navigate("https://studio.example/").is_ok());
            let (send, receive) = oneshot::channel();
            let task = tokio::spawn(async move {
                gateway
                    .click_with_approval("publish", |site, name| async move {
                        let request = crate::approval::browser_request(
                            "chat",
                            "claude",
                            &site,
                            "수업 올리기",
                            "publish",
                            &name,
                        );
                        let value = worker
                            .request_with(
                                request,
                                CancellationToken::new(),
                                |card| {
                                    let _ = send.send(card.request_id.clone());
                                    true
                                },
                                |_| {},
                            )
                            .await;
                        matches!(value, Ok(ApprovalChoice::AllowOnce))
                    })
                    .await
            });
            let id = receive.await.unwrap();
            assert_eq!(clicked.load(Ordering::SeqCst), 0);
            assert!(!task.is_finished());
            broker.respond(&id, choice).await.unwrap();
            let result = task.await.unwrap();
            assert_eq!(result.is_ok(), choice == ApprovalChoice::AllowOnce);
            assert_eq!(
                clicked.load(Ordering::SeqCst),
                usize::from(choice == ApprovalChoice::AllowOnce)
            );
            assert!(broker.respond(&id, choice).await.is_err());
            assert_eq!(closed.load(Ordering::SeqCst), 1);
        }
    }
    #[cfg(windows)]
    #[tokio::test]
    async fn fake_cli_through_authenticated_pipe_waits_for_approval_and_closes() {
        use crate::approval::{ApprovalBroker, ApprovalChoice};
        use tokio::sync::{oneshot, Mutex};
        use tokio_util::sync::CancellationToken;
        for choice in [ApprovalChoice::AllowOnce, ApprovalChoice::Deny] {
            let clicked = Arc::new(AtomicUsize::new(0));
            let closed = Arc::new(AtomicUsize::new(0));
            let page = PageState {
                origin: "https://studio.example".into(),
                title: "수업".into(),
                text: "자료".into(),
                elements: vec![Element {
                    id: "publish".into(),
                    name: "게시".into(),
                    kind: "normal".into(),
                }],
            };
            let driver = FakeDriver {
                page,
                clicked: clicked.clone(),
                closed: closed.clone(),
                login: true,
                mutate_target: false,
            };
            let gateway = Arc::new(Mutex::new(Some(Gateway {
                policy: Policy {
                    version: 1,
                    origins: vec!["https://studio.example".into()],
                    login_origins: vec![],
                    login_paths: vec![],
                    irreversible: vec!["게시".into()],
                    read_only: vec!["browser_navigate".into(), "browser_read".into()],
                },
                driver,
            })));
            let broker = ApprovalBroker::default();
            let worker_broker = broker.clone();
            let worker_gateway = gateway.clone();
            let (send, receive) = oneshot::channel::<String>();
            let send = Arc::new(std::sync::Mutex::new(Some(send)));
            let handler: crate::setup_bridge::Handler = Arc::new(move |input| {
                let gateway = worker_gateway.clone();
                let broker = worker_broker.clone();
                let send = send.clone();
                Box::pin(async move {
                    let mut locked = gateway.lock().await;
                    match input["method"].as_str() {
                        Some("browser_open") => serde_json::json!({"status":"ok"}),
                        Some("browser_navigate") => {
                            let result = locked
                                .as_mut()
                                .unwrap()
                                .navigate(input["arguments"]["url"].as_str().unwrap());
                            match result {
                                Err(DriverError::HumanRequired(_)) => {
                                    serde_json::json!({"status":"human_required"})
                                }
                                Err(DriverError::Blocked) => {
                                    serde_json::json!({"status":"blocked"})
                                }
                                Ok(_) => serde_json::json!({"status":"ok"}),
                                _ => serde_json::json!({"status":"unavailable"}),
                            }
                        }
                        Some("browser_click") => {
                            let result = locked
                                .as_mut()
                                .unwrap()
                                .click_with_approval("publish", |site, name| async move {
                                    let request = crate::approval::browser_request(
                                        "chat",
                                        "claude",
                                        &site,
                                        "수업 올리기",
                                        "publish",
                                        &name,
                                    );
                                    let result = broker
                                        .request_with(
                                            request,
                                            CancellationToken::new(),
                                            |card| {
                                                if let Some(tx) = send.lock().unwrap().take() {
                                                    let _ = tx.send(card.request_id.clone());
                                                }
                                                true
                                            },
                                            |_| {},
                                        )
                                        .await;
                                    matches!(result, Ok(ApprovalChoice::AllowOnce))
                                })
                                .await;
                            serde_json::json!({"status":if result.is_ok() { "ok" } else { "denied" }})
                        }
                        Some("browser_close") => {
                            locked.take();
                            serde_json::json!({"status":"closed"})
                        }
                        _ => serde_json::json!({"status":"unavailable"}),
                    }
                })
            });
            let id = format!("test-{}", uuid::Uuid::new_v4());
            let guard = crate::setup_bridge::start(&id, CancellationToken::new(), handler).unwrap();
            crate::setup_bridge::bind(&id, Some(std::process::id()));
            let pipe = crate::setup_bridge::name(&id).unwrap();
            let task = tokio::task::spawn_blocking(move || {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(pipe)
                    .unwrap();
                let mut client = crate::momso_mcp::test_bridge(file);
                let mut ready = true;
                let mut call = |name: &str, args: serde_json::Value| {
                    let result = crate::momso_mcp::test_response(&serde_json::json!({"id":1,"method":"tools/call","params":{"name":name,"arguments":args}}), &mut ready, &mut client).unwrap();
                    serde_json::from_str::<serde_json::Value>(
                        result["result"]["content"][0]["text"].as_str().unwrap(),
                    )
                    .unwrap()
                };
                assert_eq!(call("browser_open", serde_json::json!({}))["status"], "ok");
                assert_eq!(
                    call(
                        "browser_navigate",
                        serde_json::json!({"url":"https://studio.example/"})
                    )["status"],
                    "human_required"
                );
                assert_eq!(
                    call(
                        "browser_navigate",
                        serde_json::json!({"url":"https://evil.example/"})
                    )["status"],
                    "blocked"
                );
                assert_eq!(
                    call(
                        "browser_navigate",
                        serde_json::json!({"url":"https://studio.example/"})
                    )["status"],
                    "ok"
                );
                let clicked_result =
                    call("browser_click", serde_json::json!({"elementId":"publish"}));
                assert_eq!(
                    call("browser_close", serde_json::json!({}))["status"],
                    "closed"
                );
                clicked_result
            });
            let request_id = tokio::time::timeout(std::time::Duration::from_secs(15), receive)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(clicked.load(Ordering::SeqCst), 0);
            broker.respond(&request_id, choice).await.unwrap();
            let result = tokio::time::timeout(std::time::Duration::from_secs(15), task)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                result["status"],
                if choice == ApprovalChoice::AllowOnce {
                    "ok"
                } else {
                    "denied"
                }
            );
            assert_eq!(
                clicked.load(Ordering::SeqCst),
                usize::from(choice == ApprovalChoice::AllowOnce)
            );
            assert_eq!(closed.load(Ordering::SeqCst), 1);
            drop(guard);
        }
    }
}
