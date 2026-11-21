//! Per-chat host-owned browser sessions. Browser content is untrusted data.
use crate::{
    approval::ApprovalRequest,
    browser_driver::{self, DriverError, PageState, Session},
    browser_gateway::Policy,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

pub(crate) type Notify = Arc<dyn Fn(&str, Value) + Send + Sync>;
pub(crate) type Ask = Arc<
    dyn Fn(ApprovalRequest, CancellationToken) -> Pin<Box<dyn Future<Output = bool> + Send>>
        + Send
        + Sync,
>;
pub(crate) struct RequestContext<'a> {
    pub chat_id: &'a str,
    pub provider: &'a str,
    pub skill: Option<&'a str>,
    pub selected: &'a Result<Option<Policy>, &'static str>,
    pub root: PathBuf,
    pub visible: bool,
    pub cancel: CancellationToken,
    pub notify: Notify,
    pub ask: Ask,
}
struct Entry {
    policy: Policy,
    skill: String,
    action: String,
    grant: Mutex<Option<Grant>>,
    extra_origins: Mutex<BTreeSet<String>>,
    blocked_origin: Mutex<Option<String>>,
    serial: AsyncMutex<()>,
    session: Arc<Mutex<Option<Session>>>,
    panel_lease: Option<Arc<crate::panel_ownership::PanelLease>>,
    waiting_for_human: AtomicBool,
    external_login: AtomicBool,
}
impl Entry {
    fn denied(&self) -> Value {
        if let Some(status) = self
            .panel_lease
            .as_ref()
            .and_then(|lease| lease.check().err())
        {
            return json!({"status":status});
        }
        json!({"status":"denied"})
    }
    fn allows(&self, url: &str) -> bool {
        self.policy.allows(url)
            || reqwest::Url::parse(url).ok().is_some_and(|parsed| {
                parsed.scheme() == "https"
                    && parsed.username().is_empty()
                    && parsed.password().is_none()
                    && self.extra_origins.lock().is_ok_and(|allowed| {
                        allowed.contains(&parsed.origin().ascii_serialization())
                    })
            })
    }
    fn blocked(&self, target: &str) -> Value {
        let origin = reqwest::Url::parse(target)
            .ok()
            .filter(|url| {
                url.scheme() == "https"
                    && url.port().is_none()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && crate::object_store::public_literal_host(url)
            })
            .map(|url| url.origin().ascii_serialization());
        if let Ok(mut pending) = self.blocked_origin.lock() {
            *pending = origin;
        }
        blocked(target)
    }
    fn error(&self, err: DriverError, chat_id: &str, notify: &Notify) -> Value {
        match err {
            DriverError::Blocked(target) => {
                self.blocked_or_external_login(&target, chat_id, notify)
            }
            other => error(other, chat_id, notify),
        }
    }
    fn blocked_or_external_login(&self, target: &str, chat_id: &str, notify: &Notify) -> Value {
        if self.panel_lease.is_some() && google_login_target(target) {
            notify(
                "browser-human-required",
                json!({"chatId":chat_id,"reason":"external_login","loginUrl":target}),
            );
            return json!({"status":"external_login_required"});
        }
        self.blocked(target)
    }
}
pub(crate) fn google_login_target(target: &str) -> bool {
    reqwest::Url::parse(target).ok().is_some_and(|url| {
        #[cfg(test)]
        if std::env::var_os("MOMSO_TEST_PANEL_FAKE_GOOGLE").is_some()
            && url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.path() == "/momso-test-google-login"
        {
            return true;
        }
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.host_str() == Some("accounts.google.com")
    })
}
#[derive(Clone)]
struct Grant {
    origin: String,
    action: String,
    description: String,
    element_id: String,
    element_name: String,
}
impl Grant {
    fn permits(&self, origin: &str, method: &str, id: &str, name: &str) -> bool {
        self.origin == origin
            && (method != "browser_click"
                || (id == self.element_id
                    && name == self.element_name
                    && name.contains(&self.action)))
    }
}
struct ScopeRequest<'a> {
    chat_id: &'a str,
    provider: &'a str,
    origin: &'a str,
    skill: &'a str,
    action: &'a str,
    description: &'a str,
    element_id: &'a str,
    element_name: &'a str,
}
async fn request_scope(
    policy: &Policy,
    scope: ScopeRequest<'_>,
    ask: &Ask,
    cancel: CancellationToken,
) -> Result<Grant, &'static str> {
    if !policy.allows_action(scope.action) {
        return Err("action_unavailable");
    }
    let request = crate::approval::browser_scope_request(
        scope.chat_id,
        scope.provider,
        scope.origin,
        scope.skill,
        scope.action,
        &format!("{} · 대상: {}", scope.description, scope.element_name),
    );
    if !ask(request, cancel.clone()).await || cancel.is_cancelled() {
        return Err("denied");
    }
    Ok(Grant {
        origin: scope.origin.to_owned(),
        action: scope.action.to_owned(),
        description: scope.description.to_owned(),
        element_id: scope.element_id.to_owned(),
        element_name: scope.element_name.to_owned(),
    })
}
#[derive(Default)]
pub(crate) struct BrowserManager {
    entries: AsyncMutex<HashMap<String, Arc<Entry>>>,
    host_decisions: Mutex<HashMap<(String, String), bool>>,
    external_sites: Arc<Mutex<HashMap<String, String>>>,
    external_sessions: AsyncMutex<HashMap<String, Session>>,
    #[cfg(test)]
    fake_external: Mutex<HashMap<String, Arc<AtomicBool>>>,
    turns: Mutex<HashMap<String, ResumeTurn>>,
    pub(crate) resume_lock: AsyncMutex<()>,
    pub(crate) panel_ownership: crate::panel_ownership::PanelOwnership,
    app: std::sync::OnceLock<tauri::AppHandle>,
    panel_identities: Mutex<HashMap<String, PanelIdentity>>,
}

impl BrowserManager {
    pub(crate) fn host_decision(&self, chat: &str, host: &str) -> Option<bool> {
        self.host_decisions
            .lock()
            .ok()?
            .get(&(chat.to_owned(), host.to_owned()))
            .copied()
    }

    pub(crate) fn remember_host_decision(&self, chat: &str, host: &str, allow: bool) {
        if let Ok(mut decisions) = self.host_decisions.lock() {
            decisions.insert((chat.to_owned(), host.to_owned()), allow);
        }
    }

    pub(crate) fn forget_chat_decisions(&self, chat: &str) {
        if let Ok(mut decisions) = self.host_decisions.lock() {
            decisions.retain(|(owner, _), _| owner != chat);
        }
    }
}

#[derive(Clone)]
pub(crate) struct ResumeTurn {
    pub provider: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub skill: Option<String>,
    pub fast: Option<bool>,
    pub auth_method: Option<String>,
    pub original_message_id: String,
}

async fn wait_for_panel(
    mut is_open: impl FnMut() -> bool,
    lease: &crate::panel_ownership::PanelLease,
    cancel: &CancellationToken,
) -> Result<(), &'static str> {
    loop {
        lease.check()?;
        if cancel.is_cancelled() {
            return Err("panel_closed");
        }
        if is_open() {
            return Ok(());
        }
        tokio::select! {
            _ = cancel.cancelled() => return Err("panel_closed"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
        }
    }
}

#[derive(Clone)]
struct PanelIdentity {
    provider: String,
    chat_title: Option<String>,
}

fn limited(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}
fn browser_known_values(data_root: &std::path::Path) -> Result<Vec<String>, &'static str> {
    let mut known =
        crate::chat_archive_store::known_values(data_root, &data_root.join("momso.db"))?;
    known.extend(crate::saved_logins::known_passwords()?);
    Ok(known)
}
fn page_value(page: PageState, known: &[&str]) -> Value {
    let clean = |value: &str, limit| {
        limited(
            &crate::chat_archive_redact::redact_with_known(value, known).text,
            limit,
        )
    };
    let elements: Vec<_> = page.elements.into_iter().take(40).map(|e| json!({"id":clean(&e.id,128),"role":clean(&e.role,24),"name":clean(&e.name,128),"kind":e.kind})).collect();
    json!({"status":"page","origin":clean(&page.origin,512),"title":clean(&page.title,256),"text":clean(&page.text,4000),"elements":elements,"dataProvenance":page.data_provenance})
}
fn human(reason: &'static str, chat_id: &str, notify: &Notify) -> Value {
    let reason = match reason {
        "login" | "password" | "one_time_code" | "captcha" | "verification" => reason,
        _ => "verification",
    };
    notify(
        "browser-human-required",
        json!({"chatId":chat_id,"reason":reason}),
    );
    json!({"status":"human_required"})
}
fn safe_page(page: PageState, chat_id: &str, notify: &Notify, known: &[&str]) -> Value {
    if page.elements.iter().any(|e| e.kind == "password") {
        return human("password", chat_id, notify);
    }
    if page.elements.iter().any(|e| e.kind == "one_time_code") {
        return human("one_time_code", chat_id, notify);
    }
    page_value(page, known)
}
fn blocked(target: &str) -> Value {
    let host = reqwest::Url::parse(target)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .and_then(|url| url.host_str().map(str::to_owned));
    if let Some(host) = &host {
        log::warn!("browser_policy_blocked host={host}");
    }
    json!({"status":"blocked","blockedHost":host})
}

fn web_link(target: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(target) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
}
fn read_only_link(target: &str) -> bool {
    if !web_link(target) {
        return false;
    }
    let Ok(url) = reqwest::Url::parse(target) else {
        return false;
    };
    let value = format!(
        "{}?{}#{}",
        url.path(),
        url.query().unwrap_or_default(),
        url.fragment().unwrap_or_default()
    )
    .to_ascii_lowercase();
    // Encoded text is kept on the approval path rather than guessing its meaning.
    !value.contains('%')
        && ![
            "delete",
            "remove",
            "logout",
            "log-out",
            "signout",
            "sign-out",
            "cancel",
            "unsubscribe",
        ]
        .iter()
        .any(|word| value.contains(word))
}
fn error(err: DriverError, chat_id: &str, notify: &Notify) -> Value {
    match err {
        DriverError::Blocked(target) => blocked(&target),
        DriverError::HumanRequired(reason) => human(reason, chat_id, notify),
        DriverError::Refused("password") => human("password", chat_id, notify),
        DriverError::Refused("one_time_code") => human("one_time_code", chat_id, notify),
        DriverError::Refused("cancelled") => json!({"status":"denied"}),
        DriverError::Refused("human_takeover") => json!({"status":"human_takeover"}),
        DriverError::Refused("panel_closed") => json!({"status":"panel_closed"}),
        DriverError::Refused("panel_page_changed") => json!({"status":"panel_page_changed"}),
        DriverError::TargetChanged => json!({"status":"target_changed"}),
        _ => {
            log::warn!("browser_driver stage=operation status=unavailable");
            json!({"status":"unavailable"})
        }
    }
}
fn valid(method: &str, args: &Value) -> bool {
    let Some(a) = args.as_object() else {
        return false;
    };
    let id = || {
        a.get("elementId")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty() && s.len() <= 256)
    };
    match method {
        "browser_open" | "browser_read" | "browser_close" | "browser_request_saved_login" => {
            a.is_empty()
        }
        "browser_navigate" => {
            a.len() == 1
                && a.get("url")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty() && s.len() <= 4096)
        }
        "browser_click" => a.len() == 1 && id(),
        "browser_request_host" => {
            a.len() == 1
                && a.get("host")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty() && s.len() <= 253)
        }
        "browser_request_human_login" => {
            a.len() == 1 && a.get("reason").and_then(Value::as_str) == Some("login")
        }
        "browser_request_action" => {
            a.len() == 3
                && ["action", "description"].iter().all(|key| {
                    a.get(*key).and_then(Value::as_str).is_some_and(|s| {
                        !s.trim().is_empty()
                            && s.chars().count() <= if *key == "action" { 64 } else { 200 }
                            && !s.chars().any(char::is_control)
                    })
                })
                && id()
        }
        "browser_fill" => {
            a.len() == 2
                && id()
                && a.get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.len() <= 16384)
        }
        _ => false,
    }
}
async fn call<T: Send + 'static>(
    entry: &Arc<Entry>,
    f: impl FnOnce(&mut Session) -> Result<T, DriverError> + Send + 'static,
) -> Result<T, DriverError> {
    let session = entry.session.clone();
    tokio::task::spawn_blocking(move || {
        let mut guard = session.lock().map_err(|_| DriverError::Unavailable)?;
        f(guard.as_mut().ok_or(DriverError::Unavailable)?)
    })
    .await
    .map_err(|_| DriverError::Unavailable)?
}

impl BrowserManager {
    #[cfg(test)]
    pub(crate) async fn install_fake_login_panel(
        &self,
        chat_id: &str,
        skill: &str,
        policy: Policy,
        completed: Arc<AtomicBool>,
    ) {
        let lease = Arc::new(self.panel_ownership.claim(chat_id).unwrap());
        let entry = Arc::new(Entry {
            policy,
            skill: skill.to_owned(),
            action: "fixture".into(),
            grant: Mutex::new(None),
            extra_origins: Mutex::new(BTreeSet::new()),
            blocked_origin: Mutex::new(None),
            serial: AsyncMutex::new(()),
            session: Arc::new(Mutex::new(Some(Session::fake_login_panel(completed)))),
            panel_lease: Some(lease),
            waiting_for_human: AtomicBool::new(false),
            external_login: AtomicBool::new(false),
        });
        self.entries.lock().await.insert(chat_id.to_owned(), entry);
    }
    #[cfg(test)]
    pub(crate) async fn install_fake_external_login(
        &self,
        chat_id: &str,
        completed: Arc<AtomicBool>,
    ) {
        let entry = self.entries.lock().await.get(chat_id).cloned().unwrap();
        entry.external_login.store(true, Ordering::Release);
        self.external_sites
            .lock()
            .unwrap()
            .insert(chat_id.to_owned(), "https://fixture.example".to_owned());
        self.fake_external
            .lock()
            .unwrap()
            .insert(chat_id.to_owned(), completed);
    }
    pub(crate) async fn wait_for_login_turn_end(
        &self,
        active: &crate::chat_codex::Active,
        chat_id: &str,
    ) -> Result<(), &'static str> {
        while active.lock().await.contains_key(chat_id) {
            if !self.waiting_for_login(chat_id).await {
                return Err("login_wait_ended");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        Ok(())
    }
    pub(crate) fn remember_turn(&self, chat_id: &str, turn: ResumeTurn) {
        if let Ok(mut turns) = self.turns.lock() {
            turns.entry(chat_id.to_owned()).or_insert(turn);
        }
    }
    pub(crate) fn remembered_turn(&self, chat_id: &str) -> Option<ResumeTurn> {
        self.turns.lock().ok()?.get(chat_id).cloned()
    }
    pub(crate) async fn waiting_for_login(&self, chat_id: &str) -> bool {
        self.entries.lock().await.get(chat_id).is_some_and(|entry| {
            entry.panel_lease.is_some() && entry.waiting_for_human.load(Ordering::Acquire)
        })
    }
    pub(crate) async fn restore_login_wait(&self, chat_id: &str) {
        if let Some(entry) = self.entries.lock().await.get(chat_id) {
            entry.waiting_for_human.store(true, Ordering::Release);
        }
    }
    pub(crate) async fn open_external_login(
        &self,
        chat_id: &str,
        root: &std::path::Path,
        notify: &Notify,
    ) -> Result<(), &'static str> {
        let entry = self
            .entries
            .lock()
            .await
            .get(chat_id)
            .cloned()
            .ok_or("unavailable")?;
        if !entry.waiting_for_human.load(Ordering::Acquire)
            || !entry.external_login.load(Ordering::Acquire)
        {
            return Err("unavailable");
        }
        let _serial = entry.serial.lock().await;
        if self.external_sessions.lock().await.contains_key(chat_id) {
            return Ok(());
        }
        let site = self
            .external_sites
            .lock()
            .map_err(|_| "unavailable")?
            .get(chat_id)
            .cloned()
            .ok_or("unavailable")?;
        if !entry.allows(&site) {
            return Err("unavailable");
        }
        #[cfg(test)]
        {
            let completed = self
                .fake_external
                .lock()
                .ok()
                .and_then(|fakes| fakes.get(chat_id).cloned());
            if let Some(completed) = completed {
                self.external_sessions
                    .lock()
                    .await
                    .insert(chat_id.to_owned(), Session::fake_login_panel(completed));
                notify(
                    "panel-agent-external-login-opened",
                    json!({"chatId":chat_id,"state":"login","message":"패널과 바깥 창의 로그인은 따로 유지됩니다. 바깥 창에서 로그인해 주세요."}),
                );
                return Ok(());
            }
        }
        let mut origins = entry.policy.origins().to_vec();
        origins.extend(
            entry
                .extra_origins
                .lock()
                .map_err(|_| "unavailable")?
                .iter()
                .cloned(),
        );
        let mut login_origins = entry.policy.login_origins().to_vec();
        login_origins.push("https://accounts.google.com".to_owned());
        let login_paths = entry.policy.login_paths().to_vec();
        let profile = root
            .join("browser-profiles")
            .join(format!("{:x}", Sha256::digest(chat_id.as_bytes())));
        let choice = crate::browser_choice::read(root).map_err(|_| "unavailable")?;
        let session = tokio::task::spawn_blocking(move || {
            let mut session = browser_driver::launch_with_login_choice(
                &profile,
                &origins,
                &login_origins,
                &login_paths,
                true,
                choice,
            )?;
            match session.navigate(&site) {
                Ok(_) | Err(DriverError::HumanRequired(_)) => Ok(session),
                Err(error) => Err(error),
            }
        })
        .await
        .map_err(|_| "unavailable")?
        .map_err(|_| "unavailable")?;
        if !entry.waiting_for_human.load(Ordering::Acquire) {
            return Err("unavailable");
        }
        self.external_sessions
            .lock()
            .await
            .insert(chat_id.to_owned(), session);
        notify(
            "panel-agent-external-login-opened",
            json!({"chatId":chat_id,"state":"login","message":"패널과 바깥 창의 로그인은 따로 유지됩니다. 바깥 창에서 로그인해 주세요."}),
        );
        Ok(())
    }
    pub(crate) async fn resume_after_login(
        &self,
        chat_id: &str,
        notify: &Notify,
    ) -> Result<bool, &'static str> {
        let entry = self.entries.lock().await.get(chat_id).cloned();
        let Some(entry) = entry else { return Ok(false) };
        if entry.panel_lease.is_none() || !entry.waiting_for_human.load(Ordering::Acquire) {
            return Ok(false);
        }
        let _serial = entry.serial.lock().await;
        if entry.external_login.load(Ordering::Acquire) {
            let mut external = self.external_sessions.lock().await;
            let session = external
                .get_mut(chat_id)
                .ok_or("external_login_unavailable")?;
            session
                .adopt_after_human_login()
                .map_err(|error| match error {
                    DriverError::HumanRequired(_) | DriverError::Blocked(_) => "login_incomplete",
                    _ => "unavailable",
                })?;
            let session = external.remove(chat_id).ok_or("unavailable")?;
            let old = entry
                .session
                .lock()
                .map_err(|_| "unavailable")?
                .replace(session);
            drop(old);
            entry.external_login.store(false, Ordering::Release);
            self.panel_ready(&entry, chat_id, notify);
            notify(
                "panel-agent-external-continued",
                json!({"chatId":chat_id,"state":"started","message":"패널과 바깥 창의 로그인은 따로 유지됩니다. 바깥 창에서 이어서 작업합니다."}),
            );
            return Ok(true);
        }
        call(&entry, |session| session.adopt_after_human_login())
            .await
            .map_err(|error| match error {
                DriverError::HumanRequired(_) | DriverError::Blocked(_) => "login_incomplete",
                _ => "unavailable",
            })?;
        self.panel_ready(&entry, chat_id, notify);
        Ok(true)
    }
    fn panel_ready(&self, entry: &Entry, chat_id: &str, notify: &Notify) {
        if entry.panel_lease.is_some() && entry.waiting_for_human.swap(false, Ordering::AcqRel) {
            notify(
                "panel-agent-control-changed",
                self.panel_event(chat_id, "started", None),
            );
        }
    }
    pub(crate) fn panel_event(&self, chat_id: &str, state: &str, reason: Option<&str>) -> Value {
        let identity = self
            .panel_identities
            .lock()
            .ok()
            .and_then(|v| v.get(chat_id).cloned());
        json!({"chatId":chat_id,"state":state,"reason":reason,"provider":identity.as_ref().map(|v| &v.provider),"chatTitle":identity.as_ref().and_then(|v| v.chat_title.as_ref())})
    }
    pub(crate) fn bind_app(&self, app: tauri::AppHandle) {
        let _ = self.app.set(app);
    }
    pub(crate) async fn close_panel_turn(&self, chat_id: &str, keep_login: bool, notify: &Notify) {
        if keep_login && self.waiting_for_login(chat_id).await {
            return;
        }
        let is_panel = self
            .entries
            .lock()
            .await
            .get(chat_id)
            .is_some_and(|entry| entry.panel_lease.is_some());
        if is_panel {
            self.close(chat_id, notify).await;
        } else if let Ok(mut turns) = self.turns.lock() {
            turns.remove(chat_id);
        }
    }
    pub(crate) async fn is_open(&self, chat_id: &str) -> bool {
        let entry = self.entries.lock().await.get(chat_id).cloned();
        let Some(entry) = entry else {
            return false;
        };
        let session = entry.session.clone();
        let active = tokio::task::spawn_blocking(move || {
            session
                .lock()
                .ok()
                .and_then(|s| s.as_ref().map(Session::is_active))
                .unwrap_or(false)
        })
        .await
        .unwrap_or(false);
        if !active {
            let mut entries = self.entries.lock().await;
            if entries
                .get(chat_id)
                .is_some_and(|current| Arc::ptr_eq(current, &entry))
            {
                entries.remove(chat_id);
            }
        }
        active
    }
    pub(crate) async fn close(&self, chat_id: &str, notify: &Notify) {
        if let Ok(mut sites) = self.external_sites.lock() {
            sites.remove(chat_id);
        }
        if let Ok(mut turns) = self.turns.lock() {
            turns.remove(chat_id);
        }
        let entry = self.entries.lock().await.remove(chat_id);
        if let Some(entry) = entry {
            let _serial = entry.serial.lock().await;
            self.external_sessions.lock().await.remove(chat_id);
            let session = entry.session.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(mut guard) = session.lock() {
                    if let Some(session) = guard.take() {
                        session.close();
                    }
                }
            })
            .await;
            notify(
                "browser-session-updated",
                json!({"chatId":chat_id,"open":false}),
            );
            if entry.panel_lease.is_some() {
                notify(
                    "panel-agent-control-changed",
                    self.panel_event(chat_id, "ended", None),
                );
            }
        } else {
            self.external_sessions.lock().await.remove(chat_id);
        }
    }
    pub(crate) async fn dispatch(&self, context: RequestContext<'_>, input: Value) -> Value {
        let RequestContext {
            chat_id,
            provider,
            skill,
            selected,
            root,
            visible,
            cancel,
            notify,
            ask,
        } = context;
        let Some(method) = input.get("method").and_then(Value::as_str) else {
            return json!({"status":"unavailable"});
        };
        let args = input.get("arguments").cloned().unwrap_or(Value::Null);
        if input.as_object().is_none_or(|o| o.len() != 2) || !valid(method, &args) {
            return json!({"status":"unavailable"});
        }
        if method == "browser_close" {
            if self.entries.lock().await.get(chat_id).is_some_and(|entry| {
                entry.panel_lease.is_some() && entry.waiting_for_human.load(Ordering::Acquire)
            }) {
                return json!({"status":"human_required"});
            }
            self.close(chat_id, &notify).await;
            return json!({"status":"closed"});
        }
        if cancel.is_cancelled() {
            return json!({"status":"denied"});
        }
        if selected
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .is_some_and(Policy::is_general)
            && self.app.get().is_some()
            && crate::panel_chat::active_chat().as_deref() != Some(chat_id)
        {
            return json!({"status":"panel_not_active"});
        }
        let existing = self.entries.lock().await.get(chat_id).cloned();
        if self.panel_ownership.human_controls(chat_id).unwrap_or(true) {
            return json!({"status":"human_takeover"});
        }
        if existing.is_none()
            && method != "browser_open"
            && self.panel_ownership.was_closed(chat_id).unwrap_or(true)
        {
            return json!({"status":"panel_closed"});
        }
        if let Err("browser_policy_modified") = selected {
            return json!({"status":"policy_modified"});
        }
        if selected.is_err() {
            return json!({"status":"policy_unavailable"});
        }
        if let (Some(entry), Ok(Some(policy))) = (&existing, selected) {
            if &entry.policy != policy
                || (!policy.is_general() && skill.unwrap_or("") != entry.skill)
            {
                return json!({"status":"policy_unavailable"});
            }
        }
        if method == "browser_open" {
            if let Some(entry) = &existing {
                if entry.panel_lease.is_some() && entry.waiting_for_human.load(Ordering::Acquire) {
                    return json!({"status":"human_required"});
                }
                if let Some(lease) = &entry.panel_lease {
                    if let Err(status) = lease.check() {
                        return json!({"status":status});
                    }
                }
                return json!({"status":"ok"});
            }
            let Ok(Some(policy)) = selected else {
                return json!({"status":"policy_unavailable"});
            };
            let skill = skill.unwrap_or("");
            let policy = policy.clone();
            let origins = policy.origins().to_vec();
            let login_origins = policy.login_origins().to_vec();
            let login_paths = policy.login_paths().to_vec();
            let token = cancel.clone();
            let choice = match crate::browser_choice::read(&root) {
                Ok(choice) => choice,
                Err(_) => return json!({"status":"driver_unavailable"}),
            };
            let profile = root
                .join("browser-profiles")
                .join(format!("{:x}", Sha256::digest(chat_id.as_bytes())));
            let panel_app = self.app.get().cloned();
            let use_panel = panel_app.is_some()
                && !login_origins
                    .iter()
                    .any(|origin| origin == "https://accounts.google.com");
            let mut panel_lease = None;
            let result = if use_panel {
                let Some(app) = panel_app else {
                    return json!({"status":"driver_unavailable"});
                };
                let title_db = root.join("momso.db");
                let title_root = root.clone();
                let title_chat = chat_id.to_owned();
                let title = tokio::task::spawn_blocking(move || {
                    let title = crate::chat::title(&title_db, &title_chat).ok()?;
                    let known =
                        crate::chat_archive_store::known_values(&title_root, &title_db).ok()?;
                    let refs: Vec<&str> = known.iter().map(String::as_str).collect();
                    Some(crate::chat_archive_redact::redact_with_known(&title, &refs).text)
                })
                .await
                .ok()
                .flatten();
                if let Ok(mut identities) = self.panel_identities.lock() {
                    identities.insert(
                        chat_id.to_owned(),
                        PanelIdentity {
                            provider: provider.to_owned(),
                            chat_title: title,
                        },
                    );
                }
                let lease = match self.panel_ownership.claim(chat_id) {
                    Ok(value) => Arc::new(value),
                    Err(status) => return json!({"status":status}),
                };
                let initial = crate::panel_browser::current_panel(&app)
                    .and_then(|view| view.url().ok())
                    .map(|url| url.to_string());
                if !policy.is_general() {
                    let scope = crate::approval::browser_scope_request(
                        chat_id,
                        provider,
                        &origins.join(", "),
                        skill,
                        "사이트 범위",
                        if initial.is_some() {
                            "현재 패널의 화면을 바꿀 수 있습니다"
                        } else {
                            "패널에서 사이트를 엽니다"
                        },
                    );
                    let lease_cancel = lease.cancellation();
                    let approval_cancel = cancel.child_token();
                    let pending = ask(scope, approval_cancel.clone());
                    tokio::pin!(pending);
                    let approved = tokio::select! {
                        result = &mut pending => result,
                        _ = lease_cancel.cancelled() => { approval_cancel.cancel(); let _ = pending.await; false },
                        _ = cancel.cancelled() => { approval_cancel.cancel(); let _ = pending.await; false },
                    };
                    if !approved {
                        return json!({"status":lease.check().err().unwrap_or("denied")});
                    }
                }
                if policy.is_general()
                    && crate::panel_chat::active_chat().as_deref() != Some(chat_id)
                {
                    return json!({"status":"panel_not_active"});
                }
                if let Some(before) = &initial {
                    let after = crate::panel_browser::current_panel(&app)
                        .and_then(|view| view.url().ok())
                        .map(|url| url.to_string());
                    if after.as_ref() != Some(before) {
                        return json!({"status":"panel_page_changed"});
                    }
                } else {
                    let mut request = self.panel_event(chat_id, "waiting", None);
                    if let Some(value) = request.as_object_mut() {
                        value.remove("state");
                    }
                    notify("panel-agent-open-request", request);
                    if let Err(status) = wait_for_panel(
                        || crate::panel_browser::current_panel(&app).is_some(),
                        &lease,
                        &cancel,
                    )
                    .await
                    {
                        if status == "panel_closed" {
                            notify(
                                "panel-agent-control-changed",
                                self.panel_event(chat_id, "ended", Some("panel_not_opened")),
                            );
                        }
                        return json!({"status":status});
                    }
                }
                let mut allowed = origins.clone();
                allowed.extend(login_origins.iter().cloned());
                let switch_guard = crate::panel_browser::switch_lock().lock().await;
                if policy.is_general()
                    && crate::panel_chat::active_chat().as_deref() != Some(chat_id)
                {
                    return json!({"status":"panel_not_active"});
                }
                if initial.as_ref().is_some_and(|before| {
                    crate::panel_browser::current_panel(&app)
                        .and_then(|view| view.url().ok())
                        .is_none_or(|url| url.as_str() != before)
                }) {
                    return json!({"status":"panel_page_changed"});
                }
                let attached_lease = lease.clone();
                let approved_panel_url = if policy.is_general() || skill == "bodycodi-operations" {
                    initial.clone().filter(|url| policy.allows(url))
                } else {
                    None
                };
                let result = tokio::task::spawn_blocking(move || {
                    let mut session = Session::attach_panel(
                        app,
                        &allowed,
                        &login_origins,
                        &login_paths,
                        Some(attached_lease),
                    )?;
                    if let Some(url) = approved_panel_url {
                        session.arm_existing_panel(&url)?;
                    }
                    Ok(session)
                })
                .await;
                drop(switch_guard);
                panel_lease = Some(lease);
                result
            } else {
                tokio::task::spawn_blocking(move || {
                    let session = browser_driver::launch_with_login_choice(
                        &profile,
                        &origins,
                        &login_origins,
                        &login_paths,
                        visible,
                        choice,
                    )?;
                    if token.is_cancelled() {
                        return Err(DriverError::Unavailable);
                    }
                    Ok(session)
                })
                .await
            };
            let session = match result {
                Ok(Ok(session)) => session,
                Ok(Err(DriverError::Blocked(target))) => return blocked(&target),
                _ => return json!({"status":"driver_unavailable"}),
            };
            if cancel.is_cancelled() {
                drop(session);
                return json!({"status":"denied"});
            }
            let action = crate::skills::BUNDLED_SKILLS
                .iter()
                .find(|(name, _)| *name == skill)
                .and_then(|(_, text)| crate::skills::parse(skill, text).ok())
                .map(|(head, _)| head.title)
                .unwrap_or_else(|| "브라우저 화면".into());
            let entry = Arc::new(Entry {
                policy,
                skill: skill.to_owned(),
                action,
                grant: Mutex::new(None),
                extra_origins: Mutex::new(BTreeSet::new()),
                blocked_origin: Mutex::new(None),
                serial: AsyncMutex::new(()),
                session: Arc::new(Mutex::new(Some(session))),
                panel_lease,
                waiting_for_human: AtomicBool::new(false),
                external_login: AtomicBool::new(false),
            });
            let mut entries = self.entries.lock().await;
            if cancel.is_cancelled() {
                return json!({"status":"denied"});
            }
            if entries.contains_key(chat_id) {
                return json!({"status":"unavailable"});
            }
            entries.insert(chat_id.to_owned(), entry.clone());
            drop(entries);
            notify(
                "browser-session-updated",
                json!({"chatId":chat_id,"open":true}),
            );
            if entry.panel_lease.is_some() {
                notify(
                    "panel-agent-control-changed",
                    self.panel_event(chat_id, "started", None),
                );
            }
            return json!({"status":"ok"});
        }
        let Some(entry) = existing else {
            return json!({"status":"not_open"});
        };
        let notify = if entry.panel_lease.is_some() {
            let original = notify.clone();
            let entry = entry.clone();
            let identity = self.panel_event(chat_id, "login", None);
            let app = self.app.get().cloned();
            let sites = self.external_sites.clone();
            let current_chat = chat_id.to_owned();
            Arc::new(move |name: &str, mut payload: Value| {
                if name == "browser-human-required" {
                    entry.waiting_for_human.store(true, Ordering::Release);
                    entry.external_login.store(
                        payload.get("reason").and_then(Value::as_str) == Some("external_login"),
                        Ordering::Release,
                    );
                    let reason = match payload.get("reason").and_then(Value::as_str) {
                        Some("external_login") => "google_login_external",
                        Some("login" | "password") => "login_page",
                        Some("one_time_code") => "one_time_code",
                        Some("captcha") => "captcha",
                        _ => "verification",
                    };
                    let mut event = identity.clone();
                    event["reason"] = json!(reason);
                    if reason == "google_login_external" {
                        let site = app
                            .as_ref()
                            .and_then(crate::panel_browser::current_site_url);
                        if let Some(site) = site.as_ref().filter(|site| entry.allows(site)) {
                            if let Ok(mut stored) = sites.lock() {
                                stored.insert(current_chat.clone(), site.clone());
                            }
                        }
                        payload["siteUrl"] = json!(site);
                        event["loginUrl"] = payload["loginUrl"].clone();
                        event["siteUrl"] = payload["siteUrl"].clone();
                    }
                    original("panel-agent-control-changed", event);
                }
                original(name, payload);
            }) as Notify
        } else {
            notify
        };
        if let Some(lease) = &entry.panel_lease {
            if let Err(status) = lease.check() {
                return json!({"status":status});
            }
        }
        let ask = if let Some(lease) = &entry.panel_lease {
            let original = ask.clone();
            let lease_cancel = lease.cancellation();
            Arc::new(move |request, token: CancellationToken| {
                let original = original.clone();
                let lease_cancel = lease_cancel.clone();
                Box::pin(async move {
                    let approval_cancel = token.child_token();
                    let pending = original(request, approval_cancel.clone());
                    tokio::pin!(pending);
                    tokio::select! { value = &mut pending => value, _ = lease_cancel.cancelled() => { approval_cancel.cancel(); let _ = pending.await; false } }
                }) as Pin<Box<dyn Future<Output = bool> + Send>>
            }) as Ask
        } else {
            ask
        };
        let _serial = entry.serial.lock().await;
        if cancel.is_cancelled() {
            return json!({"status":"denied"});
        }
        if entry.panel_lease.is_some()
            && entry.waiting_for_human.load(Ordering::Acquire)
            && method != "browser_request_saved_login"
        {
            return json!({"status":"human_required"});
        }
        if method == "browser_request_saved_login" {
            // This is a host-owned operation; the AI supplies neither origin, account nor secret.
            let Some(lease) = &entry.panel_lease else {
                return json!({"status":"unavailable"});
            };
            let target = match call(&entry, |session| session.saved_login_target()).await {
                Ok(target) if entry.allows(&target.origin) => target,
                Ok(_) => return json!({"status":"unavailable"}),
                Err(error) => return entry.error(error, chat_id, &notify),
            };
            let expected = target.origin.clone();
            let entries = match tokio::task::spawn_blocking(move || {
                crate::saved_logins::for_origin(&expected)
            })
            .await
            {
                Ok(Ok(entries)) => entries,
                _ => return json!({"status":"credential_store_unavailable"}),
            };
            let Ok(page_url) = reqwest::Url::parse(&target.url) else {
                return json!({"status":"unavailable"});
            };
            let entries = crate::saved_logins::match_login_path(entries, page_url.path());
            let selection_cancel = cancel.child_token();
            let lease_cancel = lease.cancellation();
            let selection = crate::saved_logins::select_for_person(
                chat_id,
                &entries,
                &notify,
                selection_cancel.clone(),
            );
            tokio::pin!(selection);
            let selected = tokio::select! {
                value = &mut selection => match value { Ok(value) => value, Err(status) => return json!({"status":status}) },
                _ = lease_cancel.cancelled() => { selection_cancel.cancel(); let _ = selection.await; return entry.denied(); },
            };
            if selected.ask_before_use {
                let request = crate::approval::browser_request(
                    chat_id,
                    provider,
                    &target.origin,
                    &entry.skill,
                    "saved_login",
                    &selected.label,
                );
                if !ask(request, cancel.clone()).await
                    || cancel.is_cancelled()
                    || lease.check().is_err()
                {
                    return entry.denied();
                }
            }
            // Install the exact-origin network fence and turn off WebView2 password autosave
            // before accessing the secret. The fence remains until this session closes.
            let target = match call(&entry, move |session| {
                session.restrict_saved_login(&target)?;
                Ok(target)
            })
            .await
            {
                Ok(target) => target,
                Err(error) => return entry.error(error, chat_id, &notify),
            };
            let used_id = selected.id.clone();
            let used_origin = target.origin.clone();
            let secret = match tokio::task::spawn_blocking(move || {
                crate::saved_logins::secret_for_origin(&selected)
            })
            .await
            {
                Ok(Ok(secret)) => secret,
                Ok(Err("not_found")) => return json!({"status":"target_changed"}),
                _ => return json!({"status":"credential_store_unavailable"}),
            };
            if crate::saved_logins::remember_used_password(&secret.password).is_err() {
                return json!({"status":"unavailable"});
            }
            if cancel.is_cancelled() || lease.check().is_err() {
                return entry.denied();
            }
            return match call(&entry, move |session| {
                session.fill_saved_login(&target, secret)
            })
            .await
            {
                Ok(()) => {
                    // Submission alone does not prove the login succeeded.
                    let mut complete = false;
                    for _ in 0..20 {
                        if cancel.is_cancelled() || lease.check().is_err() {
                            return entry.denied();
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        if call(&entry, |session| session.adopt_after_human_login())
                            .await
                            .is_ok()
                        {
                            complete = true;
                            break;
                        }
                    }
                    if complete {
                        self.panel_ready(&entry, chat_id, &notify);
                        notify(
                            "saved-login-used",
                            json!({"chatId":chat_id,"id":used_id,"origin":used_origin}),
                        );
                        json!({"status":"logged_in"})
                    } else {
                        human("verification", chat_id, &notify)
                    }
                }
                Err(error) => entry.error(error, chat_id, &notify),
            };
        }
        if method == "browser_request_human_login" {
            return human("login", chat_id, &notify);
        }
        if method == "browser_request_host" {
            let host = args["host"].as_str().unwrap();
            let pending = match entry.blocked_origin.lock() {
                Ok(value) => value.clone(),
                Err(_) => return json!({"status":"unavailable"}),
            };
            let Some(origin) = pending else {
                return json!({"status":"action_unavailable"});
            };
            let Ok(parsed) = reqwest::Url::parse(&origin) else {
                return json!({"status":"action_unavailable"});
            };
            if parsed.host_str() != Some(host)
                || matches!(
                    host,
                    "nid.naver.com"
                        | "kauth.kakao.com"
                        | "accounts.kakao.com"
                        | "auth.kakao.com"
                        | "appleid.apple.com"
                        | "idmsa.apple.com"
                        | "account.apple.com"
                        | "gsa.apple.com"
                        | "accounts.google.com"
                )
            {
                return json!({"status":"action_unavailable"});
            }
            let request = crate::approval::browser_scope_request(
                chat_id,
                provider,
                &origin,
                &entry.action,
                "사이트 범위 추가",
                "이 채팅에서 AI가 이 사이트의 화면을 읽고 조작하도록 허용합니다",
            );
            if !ask(request, cancel.clone()).await || cancel.is_cancelled() {
                return entry.denied();
            }
            let approved = origin.clone();
            let operation_cancel = cancel.clone();
            if call(&entry, move |session| {
                if operation_cancel.is_cancelled() {
                    return Err(DriverError::Refused("cancelled"));
                }
                session.allow_origin(&approved)
            })
            .await
            .is_err()
            {
                return json!({"status":"unavailable"});
            }
            if let Ok(mut extra) = entry.extra_origins.lock() {
                extra.insert(origin);
            } else {
                return json!({"status":"unavailable"});
            }
            if let Ok(mut pending) = entry.blocked_origin.lock() {
                *pending = None;
            }
            return json!({"status":"approved"});
        }
        if method == "browser_navigate" {
            if !entry.policy.allows_read_only(method) {
                return json!({"status":"read_only"});
            }
            let url = args["url"].as_str().unwrap();
            if !entry.allows(url) {
                return entry.blocked_or_external_login(url, chat_id, &notify);
            }
            if !entry.policy.is_general() && !read_only_link(url) {
                let site = reqwest::Url::parse(url)
                    .map(|value| value.origin().ascii_serialization())
                    .unwrap_or_default();
                let request = crate::approval::browser_request(
                    chat_id,
                    provider,
                    &site,
                    &entry.action,
                    "irreversible",
                    "주소 이동",
                );
                if !ask(request, cancel.clone()).await || cancel.is_cancelled() {
                    return entry.denied();
                }
            }
            let data_root = root.clone();
            let known =
                match tokio::task::spawn_blocking(move || browser_known_values(&data_root)).await {
                    Ok(Ok(known)) => known,
                    _ => return json!({"status":"unavailable"}),
                };
            let refs: Vec<&str> = known.iter().map(String::as_str).collect();
            let url = url.to_owned();
            return match call(&entry, move |s| s.navigate(&url)).await {
                Ok(page) if entry.allows(&page.origin) => safe_page(page, chat_id, &notify, &refs),
                Ok(page) => entry.blocked(&page.origin),
                Err(e) => entry.error(e, chat_id, &notify),
            };
        }
        let page = match call(&entry, |s| s.read()).await {
            Ok(page) if entry.allows(&page.origin) => page,
            Ok(page) => return entry.blocked(&page.origin),
            Err(e) => return entry.error(e, chat_id, &notify),
        };
        if page.elements.iter().any(|e| e.kind == "password") {
            return human("password", chat_id, &notify);
        }
        if page.elements.iter().any(|e| e.kind == "one_time_code") {
            return human("one_time_code", chat_id, &notify);
        }
        if method == "browser_read" {
            return if entry.policy.allows_read_only(method) {
                let data_root = root.clone();
                let known =
                    match tokio::task::spawn_blocking(move || browser_known_values(&data_root))
                        .await
                    {
                        Ok(Ok(known)) => known,
                        _ => return json!({"status":"unavailable"}),
                    };
                let refs: Vec<&str> = known.iter().map(String::as_str).collect();
                page_value(page, &refs)
            } else {
                json!({"status":"read_only"})
            };
        }
        if method == "browser_request_action" {
            if entry.policy.is_general() {
                return json!({"status":"not_required"});
            }
            let action = args["action"].as_str().unwrap();
            let description = args["description"].as_str().unwrap();
            let Some(target) = page.elements.iter().find(|element| {
                element.id == args["elementId"].as_str().unwrap()
                    && element.kind == "normal"
                    && element.name.contains(action)
            }) else {
                return json!({"status":"action_mismatch"});
            };
            match entry.grant.lock() {
                Ok(mut grant) => *grant = None,
                Err(_) => return json!({"status":"unavailable"}),
            }
            let approved = match request_scope(
                &entry.policy,
                ScopeRequest {
                    chat_id,
                    provider,
                    origin: &page.origin,
                    skill: &entry.action,
                    action,
                    description,
                    element_id: &target.id,
                    element_name: &target.name,
                },
                &ask,
                cancel.clone(),
            )
            .await
            {
                Ok(grant) => grant,
                Err("denied") => return entry.denied(),
                Err(status) => return json!({"status":status}),
            };
            let Ok(mut grant) = entry.grant.lock() else {
                return json!({"status":"unavailable"});
            };
            *grant = Some(approved);
            return json!({"status":"approved"});
        }
        if entry.policy.is_general() && (method == "browser_click" || method == "browser_fill") {
            let id = args["elementId"].as_str().unwrap();
            if !page
                .elements
                .iter()
                .any(|element| element.id == id && element.kind == "normal")
            {
                return json!({"status":"target_changed"});
            }
            let mut request = crate::approval::browser_request(
                chat_id,
                provider,
                &page.origin,
                "브라우저",
                "site_access",
                "누르기와 입력",
            );
            request.description = "이 사이트에서 누르고 입력해도 될까요".into();
            if !ask(request, cancel.clone()).await || cancel.is_cancelled() {
                return entry.denied();
            }
        }
        if method == "browser_click" && entry.policy.allows_read_only("browser_navigate") {
            let id = args["elementId"].as_str().unwrap().to_owned();
            let origin = page.origin.clone();
            match call(&entry, move |s| s.link_destination(&id, &origin)).await {
                Ok(Some(url)) if web_link(&url) => {
                    if !entry.allows(&url) {
                        return entry.blocked_or_external_login(&url, chat_id, &notify);
                    }
                    if !entry.policy.is_general() && !read_only_link(&url) {
                        let site = reqwest::Url::parse(&url)
                            .map(|value| value.origin().ascii_serialization())
                            .unwrap_or_default();
                        let request = crate::approval::browser_request(
                            chat_id,
                            provider,
                            &site,
                            &entry.action,
                            "irreversible",
                            "주소 이동",
                        );
                        if !ask(request, cancel.clone()).await || cancel.is_cancelled() {
                            return entry.denied();
                        }
                    }
                    let data_root = root.clone();
                    let known =
                        match tokio::task::spawn_blocking(move || browser_known_values(&data_root))
                            .await
                        {
                            Ok(Ok(known)) => known,
                            _ => return json!({"status":"unavailable"}),
                        };
                    let refs: Vec<&str> = known.iter().map(String::as_str).collect();
                    let operation_cancel = cancel.clone();
                    return match call(&entry, move |s| {
                        if operation_cancel.is_cancelled() {
                            Err(DriverError::Refused("cancelled"))
                        } else {
                            s.navigate(&url)
                        }
                    })
                    .await
                    {
                        Ok(page) if entry.allows(&page.origin) => {
                            safe_page(page, chat_id, &notify, &refs)
                        }
                        Ok(page) => entry.blocked(&page.origin),
                        Err(e) => entry.error(e, chat_id, &notify),
                    };
                }
                Ok(_) => {}
                Err(e) => return entry.error(e, chat_id, &notify),
            }
        }
        let grant = match entry.grant.lock() {
            Ok(grant) => grant.clone(),
            Err(_) => return json!({"status":"unavailable"}),
        };
        let grant = grant.filter(|grant| grant.origin == page.origin);
        if !entry.policy.is_general() && grant.is_none() {
            return json!({"status":"read_only"});
        }
        let id = args["elementId"].as_str().unwrap();
        let Some(element) = page
            .elements
            .iter()
            .find(|e| e.id == id && e.kind == "normal")
        else {
            return json!({"status":"target_changed"});
        };
        let expected = element.name.clone();
        if !entry.policy.is_general()
            && !grant
                .as_ref()
                .is_some_and(|grant| grant.permits(&page.origin, method, id, &expected))
        {
            return json!({"status":"action_mismatch"});
        }
        let id = id.to_owned();
        let description = grant
            .as_ref()
            .map(|grant| grant.description.as_str())
            .unwrap_or("브라우저 조작");
        let action = if method == "browser_fill" {
            format!(
                "{} · {} · 입력: {}",
                entry.action,
                description,
                args["text"].as_str().unwrap()
            )
        } else {
            format!("{} · {}", entry.action, description)
        };
        let request = crate::approval::browser_request(
            chat_id,
            provider,
            &page.origin,
            &action,
            entry.policy.effect(&expected),
            &expected,
        );
        if !entry.policy.is_general()
            && (!ask(request, cancel.clone()).await || cancel.is_cancelled())
        {
            return entry.denied();
        }
        let origin = page.origin.clone();
        let operation_cancel = cancel.clone();
        let result = if method == "browser_click" {
            call(&entry, move |s| {
                if operation_cancel.is_cancelled() {
                    Err(DriverError::Refused("cancelled"))
                } else {
                    s.click_checked(&id, &expected, Some(&origin))
                }
            })
            .await
        } else {
            let text = args["text"].as_str().unwrap().to_owned();
            call(&entry, move |s| {
                if operation_cancel.is_cancelled() {
                    Err(DriverError::Refused("cancelled"))
                } else {
                    s.fill_checked(&id, &text, Some(&expected), Some(&origin))
                }
            })
            .await
        };
        match result {
            Ok(()) => {
                // A completed action cannot authorize another final click.
                if method == "browser_click" {
                    if let Ok(mut grant) = entry.grant.lock() {
                        *grant = None;
                    }
                }
                json!({"status":"ok"})
            }
            Err(e) => entry.error(e, chat_id, &notify),
        }
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn site_decisions_are_scoped_to_chat_and_host() {
        let manager = BrowserManager::default();
        assert_eq!(manager.host_decision("one", "example.test"), None);
        manager.remember_host_decision("one", "example.test", true);
        manager.remember_host_decision("one", "other.test", false);
        assert_eq!(manager.host_decision("one", "example.test"), Some(true));
        assert_eq!(manager.host_decision("one", "other.test"), Some(false));
        assert_eq!(manager.host_decision("two", "example.test"), None);
        manager.forget_chat_decisions("one");
        assert_eq!(manager.host_decision("one", "example.test"), None);
    }
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn returning_while_turn_finishes_waits_and_rejects_duplicate() {
        let manager = Arc::new(BrowserManager::default());
        let lease = Arc::new(manager.panel_ownership.claim("chat").unwrap());
        let entry = Arc::new(Entry {
            policy: Policy::test_loopback("http://127.0.0.1:1234".into()),
            skill: "fixture-skill".into(),
            action: "fixture".into(),
            grant: Mutex::new(None),
            extra_origins: Mutex::new(BTreeSet::new()),
            blocked_origin: Mutex::new(None),
            serial: AsyncMutex::new(()),
            session: Arc::new(Mutex::new(Some(Session::fake_for_host_approval(&[])))),
            panel_lease: Some(lease),
            waiting_for_human: AtomicBool::new(true),
            external_login: AtomicBool::new(false),
        });
        manager.entries.lock().await.insert("chat".into(), entry);
        let active = crate::chat_codex::Active::default();
        active
            .lock()
            .await
            .insert("chat".into(), CancellationToken::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let task_manager = manager.clone();
        let task_active = active.clone();
        let task_calls = calls.clone();
        let pending = tokio::spawn(async move {
            let _guard = task_manager.resume_lock.try_lock().unwrap();
            ready_tx.send(()).unwrap();
            task_manager
                .wait_for_login_turn_end(&task_active, "chat")
                .await
                .unwrap();
            task_calls.fetch_add(1, Ordering::SeqCst);
        });
        ready_rx.await.unwrap();
        assert!(manager.resume_lock.try_lock().is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let notify: Notify = Arc::new(|_, _| {});
        manager.close_panel_turn("chat", true, &notify).await;
        active.lock().await.remove("chat");
        pending.await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(manager.waiting_for_login("chat").await);
    }

    #[tokio::test]
    async fn finished_login_turn_preserves_scope_and_original_request() {
        let manager = BrowserManager::default();
        let lease = Arc::new(manager.panel_ownership.claim("chat").unwrap());
        let entry = Arc::new(Entry {
            policy: Policy::test_loopback("http://127.0.0.1:1234".into()),
            skill: "fixture-skill".into(),
            action: "fixture".into(),
            grant: Mutex::new(None),
            extra_origins: Mutex::new(BTreeSet::new()),
            blocked_origin: Mutex::new(None),
            serial: AsyncMutex::new(()),
            session: Arc::new(Mutex::new(Some(Session::fake_for_host_approval(&[])))),
            panel_lease: Some(lease),
            waiting_for_human: AtomicBool::new(true),
            external_login: AtomicBool::new(false),
        });
        manager.entries.lock().await.insert("chat".into(), entry);
        manager.remember_turn(
            "chat",
            ResumeTurn {
                provider: "codex".into(),
                model: None,
                effort: None,
                skill: Some("fixture-skill".into()),
                fast: None,
                auth_method: None,
                original_message_id: "message-1".into(),
            },
        );
        let notify: Notify = Arc::new(|_, _| {});
        manager.close_panel_turn("chat", true, &notify).await;
        assert!(manager.waiting_for_login("chat").await);
        let turn = manager.remembered_turn("chat").unwrap();
        assert_eq!(turn.skill.as_deref(), Some("fixture-skill"));
        assert_eq!(turn.original_message_id, "message-1");
        assert_eq!(
            manager.resume_after_login("chat", &notify).await,
            Err("unavailable")
        );
        assert!(manager.waiting_for_login("chat").await);
        manager.close_panel_turn("chat", false, &notify).await;
        assert!(manager.remembered_turn("chat").is_none());
    }

    #[tokio::test]
    async fn panel_open_wait_ends_with_turn_cancellation() {
        let ownership = crate::panel_ownership::PanelOwnership::default();
        let lease = ownership.claim("fixture").unwrap();
        let cancel = CancellationToken::new();
        let signal = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            signal.cancel();
        });
        assert_eq!(
            wait_for_panel(|| false, &lease, &cancel).await,
            Err("panel_closed")
        );
    }

    #[test]
    fn external_google_login_requires_exact_secure_host() {
        assert!(google_login_target(
            "https://accounts.google.com/signin?state=fixture"
        ));
        for value in [
            "https://dash.cloudflare.com/login",
            "http://accounts.google.com/",
            "https://accounts.google.com.attacker.test/",
            "https://accounts.google.com@attacker.test/",
            concat!("https://user", "@accounts.google.com/"),
            "https://accounts.google.com:444/",
        ] {
            assert!(!google_login_target(value));
        }
    }

    #[test]
    fn blocked_navigation_exposes_host_only_without_path_or_credentials() {
        let value = blocked(concat!(
            "https://name:secret",
            "@outside.example/private?token=secret"
        ));
        assert_eq!(value["status"], "blocked");
        assert_eq!(value["blockedHost"], "outside.example");
        assert!(!value.to_string().contains("secret"));
        assert!(blocked("new_window")["blockedHost"].is_null());
        assert!(blocked("javascript:alert(1)")["blockedHost"].is_null());
    }

    #[tokio::test]
    async fn blocked_host_needs_approval_and_is_scoped_to_open_session() {
        let policy = Policy::test_loopback("http://127.0.0.1:1234".into());
        let selected = Ok(Some(policy.clone()));
        let entry = Arc::new(Entry {
            policy,
            skill: "fixture".into(),
            action: "fixture".into(),
            grant: Mutex::new(None),
            extra_origins: Mutex::new(BTreeSet::new()),
            blocked_origin: Mutex::new(None),
            serial: AsyncMutex::new(()),
            session: Arc::new(Mutex::new(Some(Session::fake_for_host_approval(&[])))),
            panel_lease: None,
            waiting_for_human: AtomicBool::new(false),
            external_login: AtomicBool::new(false),
        });
        let manager = BrowserManager::default();
        manager
            .entries
            .lock()
            .await
            .insert("chat".into(), entry.clone());
        let notify: Notify = Arc::new(|_, _| {});
        let request =
            json!({"method":"browser_request_host","arguments":{"host":"outside.example"}});
        let run = |ask: Ask| {
            manager.dispatch(
                RequestContext {
                    chat_id: "chat",
                    provider: "fixture",
                    skill: Some("fixture"),
                    selected: &selected,
                    root: PathBuf::new(),
                    visible: false,
                    cancel: CancellationToken::new(),
                    notify: notify.clone(),
                    ask,
                },
                request.clone(),
            )
        };
        let deny: Ask = Arc::new(|_, _| Box::pin(async { false }));
        assert_eq!(run(deny.clone()).await["status"], "action_unavailable");
        assert_eq!(
            entry.blocked("https://outside.example/private?token=secret")["blockedHost"],
            "outside.example"
        );
        assert_eq!(run(deny).await["status"], "denied");
        assert!(!entry.allows("https://outside.example/path"));
        let allow: Ask = Arc::new(|card, _| {
            assert_eq!(card.kind, "browser_action_scope");
            assert_eq!(card.targets[0].path, "https://outside.example");
            assert!(!card.description.contains("secret"));
            Box::pin(async { true })
        });
        assert_eq!(run(allow).await["status"], "approved");
        assert!(entry.allows("https://outside.example/path"));
        assert!(entry
            .session
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .test_allows_origin("https://outside.example"));
        entry.blocked("https://accounts.kakao.com/login");
        let no_card: Ask = Arc::new(|_, _| panic!("login host cannot use general approval"));
        let denied = manager
            .dispatch(
                RequestContext {
                    chat_id: "chat",
                    provider: "fixture",
                    skill: Some("fixture"),
                    selected: &selected,
                    root: PathBuf::new(),
                    visible: false,
                    cancel: CancellationToken::new(),
                    notify: notify.clone(),
                    ask: no_card,
                },
                json!({"method":"browser_request_host","arguments":{"host":"accounts.kakao.com"}}),
            )
            .await;
        assert_eq!(denied["status"], "action_unavailable");
        assert!(!entry.allows("https://accounts.kakao.com/login"));
        manager.close("chat", &notify).await;
        assert!(manager.entries.lock().await.get("chat").is_none());
    }
    #[test]
    fn read_links_reject_mutating_words_and_encoded_or_non_web_addresses() {
        assert!(read_only_link("https://example.test/classes?page=2"));
        for value in [
            "https://example.test/delete/123",
            "https://example.test/class?action=REMOVE",
            "https://example.test/logout",
            "https://example.test/cancel/1",
            "https://example.test/#/delete/1",
            "https://example.test/#/%64elete",
            "https://example.test/%64elete",
            "https://name:secret@example.test/classes",
            "javascript:alert(1)",
        ] {
            assert!(!read_only_link(value), "{value}");
        }
    }

    #[test]
    fn browser_page_hides_known_key_from_ai_response() {
        let secret = "fixture-opaque-known-secret";
        let page = PageState {
            origin: "https://example.test".into(),
            title: format!("title {secret}"),
            text: format!("body {secret}"),
            elements: vec![crate::browser_driver::Element {
                id: "fixture-id".into(),
                role: "button".into(),
                name: format!("button {secret}"),
                kind: "normal",
            }],
            data_provenance: "untrusted_page_data",
        };
        let result = serde_json::to_string(&page_value(page, &[secret])).unwrap();
        assert!(!result.contains(secret));
        assert!(result.contains("fixture-id"));
    }

    #[tokio::test]
    async fn scope_approval_is_policy_bound_and_denial_keeps_read_only() {
        let policy = Policy::test_loopback("http://127.0.0.1:1234".into());
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let deny: Ask = Arc::new(move |request, _| {
            assert_eq!(request.kind, "browser_action_scope");
            assert_eq!(request.targets[0].path, "http://127.0.0.1:1234");
            counted.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { false })
        });
        let scope = || ScopeRequest {
            chat_id: "chat",
            provider: "claude",
            origin: "http://127.0.0.1:1234",
            skill: "fixture",
            action: "게시",
            description: "내일 수업 게시",
            element_id: "button-1",
            element_name: "게시",
        };
        assert!(matches!(
            request_scope(&policy, scope(), &deny, CancellationToken::new()).await,
            Err("denied")
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let allow: Ask = Arc::new(|_, _| Box::pin(async { true }));
        let grant = request_scope(&policy, scope(), &allow, CancellationToken::new())
            .await
            .unwrap();
        assert!(grant.permits("http://127.0.0.1:1234", "browser_click", "button-1", "게시"));
        assert!(!grant.permits("http://127.0.0.1:1234", "browser_click", "button-2", "게시"));
        assert!(!grant.permits("http://127.0.0.1:1234", "browser_click", "button-1", "삭제"));
        assert!(!grant.permits("https://other.example", "browser_click", "button-1", "게시"));
        let mut bad = scope();
        bad.action = "무제한";
        assert!(matches!(
            request_scope(&policy, bad, &allow, CancellationToken::new()).await,
            Err("action_unavailable")
        ));
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::approval::{ApprovalBroker, ApprovalChoice};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        thread,
        time::Duration,
    };
    use tokio::sync::mpsc;

    #[test]
    fn page_response_redacts_saved_password_from_all_page_strings() {
        let secret = uuid::Uuid::new_v4().to_string();
        let page = PageState {
            origin: format!("https://{secret}.example"),
            title: secret.clone(),
            text: secret.clone(),
            elements: vec![crate::browser_driver::Element {
                id: secret.clone(),
                role: secret.clone(),
                name: secret.clone(),
                kind: "normal",
            }],
            data_provenance: "untrusted_page_data",
        };
        let response = page_value(page, &[secret.as_str()]).to_string();
        assert!(!response.contains(&secret));
    }

    struct Site {
        url: String,
        posted: Arc<AtomicUsize>,
        searched: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }
    impl Site {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let posted = Arc::new(AtomicUsize::new(0));
            let searched = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let posts = posted.clone();
            let searches = searched.clone();
            let done = stop.clone();
            let worker = thread::spawn(move || {
                let mut clients = Vec::new();
                while !done.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let posts = posts.clone();
                            let searches = searches.clone();
                            clients.push(thread::spawn(move || {
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                            let mut data = [0u8; 8192];
                            let n = stream.read(&mut data).unwrap_or(0);
                            if n == 0 { return; }
                            let request = String::from_utf8_lossy(&data[..n]);
                            let body = if request.starts_with("POST /posted ")
                                || request.starts_with("GET /delete ")
                            {
                                posts.fetch_add(1, Ordering::SeqCst);
                                "ok"
                            } else if request.starts_with("POST /search ") {
                                searches.fetch_add(1, Ordering::SeqCst);
                                "ok"
                            } else if request.starts_with("GET /login ") {
                                "<title>Login</title><input type='password'>"
                            } else {
                                "<title>Class</title><a href='/page2'>next</a><a href='/delete'>delete</a><input placeholder='Lesson' oninput=\"fetch('/posted',{method:'POST'})\"><button onclick=\"fetch('/posted',{method:'POST'})\">게시</button>"
                            };
                            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
                            let _ = stream.write_all(response.as_bytes());
                            }));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(_) => break,
                    }
                }
                for client in clients {
                    let _ = client.join();
                }
            });
            Self {
                url,
                posted,
                searched,
                stop,
                worker: Some(worker),
            }
        }
    }
    impl Drop for Site {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    #[tokio::test]
    async fn general_browser_uses_fake_site_and_respects_action_decisions() {
        let site = Site::start();
        let manager = BrowserManager::default();
        let policy = Policy::general();
        let entry = Arc::new(Entry {
            policy: policy.clone(),
            skill: String::new(),
            action: "fixture".into(),
            grant: Mutex::new(None),
            extra_origins: Mutex::new(BTreeSet::new()),
            blocked_origin: Mutex::new(None),
            serial: AsyncMutex::new(()),
            session: Arc::new(Mutex::new(Some(Session::fake_for_mcp_site(
                site.url.clone(),
                "https://login.fixture".into(),
            )))),
            panel_lease: None,
            waiting_for_human: AtomicBool::new(false),
            external_login: AtomicBool::new(false),
        });
        manager.entries.lock().await.insert("chat".into(), entry);
        let temp = tempfile::tempdir().unwrap();
        crate::storage::initialize(
            &temp.path().join("momso.db"),
            &temp.path().join("backups"),
            "fixture",
        )
        .unwrap();
        let selected = Ok(Some(policy));
        let notify: Notify = Arc::new(|_, _| {});
        let asks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ask_count = asks.clone();
        let allow: Ask = Arc::new(move |_, _| {
            ask_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { true })
        });
        let deny: Ask = Arc::new(|_, _| Box::pin(async { false }));
        for (input, decision, expected) in [
            (
                json!({"method":"browser_open","arguments":{}}),
                allow.clone(),
                "ok",
            ),
            (
                json!({"method":"browser_navigate","arguments":{"url":format!("{}/work",site.url)}}),
                allow.clone(),
                "page",
            ),
            (
                json!({"method":"browser_read","arguments":{}}),
                allow.clone(),
                "page",
            ),
            (
                json!({"method":"browser_click","arguments":{"elementId":"next"}}),
                allow.clone(),
                "page",
            ),
            (
                json!({"method":"browser_click","arguments":{"elementId":"menu"}}),
                allow.clone(),
                "ok",
            ),
            (
                json!({"method":"browser_fill","arguments":{"elementId":"query","text":"fixture"}}),
                allow.clone(),
                "ok",
            ),
            (
                json!({"method":"browser_click","arguments":{"elementId":"publish"}}),
                deny.clone(),
                "denied",
            ),
        ] {
            let result = manager
                .dispatch(
                    RequestContext {
                        chat_id: "chat",
                        provider: "fixture",
                        skill: None,
                        selected: &selected,
                        root: temp.path().to_path_buf(),
                        visible: false,
                        cancel: CancellationToken::new(),
                        notify: notify.clone(),
                        ask: decision,
                    },
                    input,
                )
                .await;
            assert_eq!(result["status"], expected);
        }
        assert_eq!(site.searched.load(Ordering::SeqCst), 1);
        assert_eq!(site.posted.load(Ordering::SeqCst), 0);
        // Opening, navigating and reading do not ask; link, menu and input do.
        assert_eq!(asks.load(Ordering::SeqCst), 3);
        let result = manager
            .dispatch(
                RequestContext {
                    chat_id: "chat",
                    provider: "fixture",
                    skill: None,
                    selected: &selected,
                    root: temp.path().to_path_buf(),
                    visible: false,
                    cancel: CancellationToken::new(),
                    notify,
                    ask: allow,
                },
                json!({"method":"browser_click","arguments":{"elementId":"publish"}}),
            )
            .await;
        assert_eq!(result["status"], "ok");
        assert_eq!(site.posted.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn six_bundled_skills_run_through_mcp_with_fake_sites_and_approval() {
        for &(name, content) in crate::skills::BUNDLED_SKILLS {
            if !crate::browser_gateway::builtin_browser_name(name) {
                continue;
            }
            if name == "bodycodi-operations" {
                continue;
            }
            for choice in [ApprovalChoice::AllowOnce, ApprovalChoice::Deny] {
                let site = Site::start();
                let temp = tempfile::tempdir().unwrap();
                let root = temp.path().to_path_buf();
                let skill_dir = root.join("skills").join(name);
                std::fs::create_dir_all(&skill_dir).unwrap();
                std::fs::write(skill_dir.join("SKILL.md"), content).unwrap();
                crate::storage::initialize(
                    &root.join("momso.db"),
                    &root.join("backups"),
                    "fixture",
                )
                .unwrap();
                let mut policy = Policy::from_skill(&root, name).unwrap().unwrap();
                policy.add_test_origin(site.url.clone());
                let login = "https://login.fixture".to_owned();
                policy.add_test_login_origin(login.clone());
                let manager = Arc::new(BrowserManager::default());
                let session = Session::fake_for_mcp_site(site.url.clone(), login.clone());
                let entry = Arc::new(Entry {
                    policy: policy.clone(),
                    skill: name.into(),
                    action: name.into(),
                    grant: Mutex::new(None),
                    extra_origins: Mutex::new(BTreeSet::new()),
                    blocked_origin: Mutex::new(None),
                    serial: AsyncMutex::new(()),
                    session: Arc::new(Mutex::new(Some(session))),
                    panel_lease: None,
                    waiting_for_human: AtomicBool::new(false),
                    external_login: AtomicBool::new(false),
                });
                manager.entries.lock().await.insert("chat".into(), entry);
                let broker = ApprovalBroker::default();
                let (approval_tx, mut approval_rx) = mpsc::unbounded_channel::<String>();
                let events = Arc::new(Mutex::new(Vec::<String>::new()));
                let handler: crate::setup_bridge::Handler = {
                    let manager = manager.clone();
                    let broker = broker.clone();
                    let events = events.clone();
                    let root = root.clone();
                    Arc::new(move |input| {
                        let manager = manager.clone();
                        let broker = broker.clone();
                        let events = events.clone();
                        let root = root.clone();
                        let approval_tx = approval_tx.clone();
                        let policy = policy.clone();
                        Box::pin(async move {
                            let notify: Notify =
                                Arc::new(move |event, _| events.lock().unwrap().push(event.into()));
                            let ask: Ask = Arc::new(move |request, token| {
                                let broker = broker.clone();
                                let tx = approval_tx.clone();
                                Box::pin(async move {
                                    matches!(
                                        broker
                                            .request_with(
                                                request,
                                                token,
                                                |card| {
                                                    let _ = tx.send(card.request_id.clone());
                                                    true
                                                },
                                                |_| {}
                                            )
                                            .await,
                                        Ok(ApprovalChoice::AllowOnce)
                                    )
                                })
                            });
                            let selected = Ok(Some(policy));
                            manager
                                .dispatch(
                                    RequestContext {
                                        chat_id: "chat",
                                        provider: "fixture",
                                        skill: Some(name),
                                        selected: &selected,
                                        root,
                                        visible: false,
                                        cancel: CancellationToken::new(),
                                        notify,
                                        ask,
                                    },
                                    input,
                                )
                                .await
                        })
                    })
                };
                let bridge_id = format!("fixture-{}", uuid::Uuid::new_v4());
                let guard =
                    crate::setup_bridge::start(&bridge_id, CancellationToken::new(), handler)
                        .unwrap();
                crate::setup_bridge::bind(&bridge_id, Some(std::process::id()));
                let pipe = crate::setup_bridge::name(&bridge_id).unwrap();
                let url = site.url.clone();
                let cli = tokio::task::spawn_blocking(move || {
                    let file = std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(pipe)
                        .unwrap();
                    let mut client = crate::momso_mcp::test_bridge(file);
                    let mut ready = true;
                    let mut call = |name: &str, args: Value| {
                        let reply = crate::momso_mcp::test_response(&json!({"id":1,"method":"tools/call","params":{"name":name,"arguments":args}}), &mut ready, &mut client).unwrap();
                        serde_json::from_str::<Value>(
                            reply["result"]["content"][0]["text"].as_str().unwrap(),
                        )
                        .unwrap()
                    };
                    assert_eq!(call("browser_open", json!({}))["status"], "ok");
                    assert_eq!(
                        call("browser_navigate", json!({"url":format!("{url}/work")}))["status"],
                        "page"
                    );
                    assert_eq!(
                        call("browser_click", json!({"elementId":"next"}))["status"],
                        "page"
                    );
                    assert_eq!(
                        call("browser_click", json!({"elementId":"risky"}))["status"],
                        "denied"
                    );
                    assert_eq!(
                        call("browser_navigate", json!({"url":format!("{login}/login")}))["status"],
                        "human_required"
                    );
                    assert_eq!(
                        call("browser_navigate", json!({"url":format!("{url}/work")}))["status"],
                        "page"
                    );
                    assert_eq!(
                        call(
                            "browser_request_action",
                            json!({"action":"게시","description":"fixture","elementId":"publish"})
                        )["status"],
                        "approved"
                    );
                    let result = call("browser_click", json!({"elementId":"publish"}));
                    assert_eq!(call("browser_close", json!({}))["status"], "closed");
                    result
                });
                let risky = tokio::time::timeout(Duration::from_secs(30), approval_rx.recv())
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(site.posted.load(Ordering::SeqCst), 0);
                broker.respond(&risky, ApprovalChoice::Deny).await.unwrap();
                let scope = tokio::time::timeout(Duration::from_secs(30), approval_rx.recv())
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(site.posted.load(Ordering::SeqCst), 0);
                broker
                    .respond(&scope, ApprovalChoice::AllowOnce)
                    .await
                    .unwrap();
                let click = tokio::time::timeout(Duration::from_secs(30), approval_rx.recv())
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(site.posted.load(Ordering::SeqCst), 0);
                broker.respond(&click, choice).await.unwrap();
                let result = tokio::time::timeout(Duration::from_secs(30), cli)
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
                    site.posted.load(Ordering::SeqCst),
                    usize::from(choice == ApprovalChoice::AllowOnce)
                );
                assert!(events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| event == "browser-human-required"));
                assert!(!manager.is_open("chat").await);
                drop(guard);
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires a locally installed Edge or Chrome browser"]
    async fn edge_mcp_approval_human_handoff_policy_and_close() {
        browser_mcp_approval_human_handoff_policy_and_close(
            crate::browser_choice::BrowserChoice::Auto,
        )
        .await;
    }

    #[tokio::test]
    #[ignore = "requires a locally installed Naver Whale browser"]
    async fn whale_mcp_approval_human_handoff_policy_and_close() {
        browser_mcp_approval_human_handoff_policy_and_close(
            crate::browser_choice::BrowserChoice::Whale,
        )
        .await;
    }

    #[tokio::test]
    #[ignore = "requires a locally installed Edge browser"]
    async fn explicit_edge_mcp_approval_human_handoff_policy_and_close() {
        browser_mcp_approval_human_handoff_policy_and_close(
            crate::browser_choice::BrowserChoice::Edge,
        )
        .await;
    }

    async fn browser_mcp_approval_human_handoff_policy_and_close(
        browser: crate::browser_choice::BrowserChoice,
    ) {
        for choice in [ApprovalChoice::AllowOnce, ApprovalChoice::Deny] {
            let site = Site::start();
            let other = Site::start();
            let build_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(4)
                .unwrap()
                .join("target");
            let temp = tempfile::Builder::new()
                .prefix("c19-browser-test-")
                .tempdir_in(build_root)
                .unwrap();
            let manager = Arc::new(BrowserManager::default());
            let worker_manager = manager.clone();
            let policy = Ok(Some(Policy::test_loopback(site.url.clone())));
            let broker = ApprovalBroker::default();
            let worker_broker = broker.clone();
            let (approved_tx, mut approved_rx) = mpsc::unbounded_channel::<String>();
            let observed = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
            let worker_observed = observed.clone();
            let continuation = Arc::new(AtomicBool::new(false));
            let worker_continuation = continuation.clone();
            let root = temp.path().to_path_buf();
            crate::browser_choice::write(&root, browser).unwrap();
            crate::storage::initialize(&root.join("momso.db"), &root.join("backups"), "fixture")
                .unwrap();
            let handler: crate::setup_bridge::Handler = Arc::new(move |input| {
                let manager = worker_manager.clone();
                let broker = worker_broker.clone();
                let tx = approved_tx.clone();
                let log = worker_observed.clone();
                let root = root.clone();
                let policy = policy.clone();
                let continuation = worker_continuation.clone();
                Box::pin(async move {
                    let notify: Notify = Arc::new(move |name, payload| {
                        log.lock().unwrap().push((name.to_owned(), payload))
                    });
                    let ask: Ask = Arc::new(move |request, token| {
                        let broker = broker.clone();
                        let tx = tx.clone();
                        Box::pin(async move {
                            matches!(
                                broker
                                    .request_with(
                                        request,
                                        token,
                                        |card| {
                                            let _ = tx.send(card.request_id.clone());
                                            true
                                        },
                                        |_| {}
                                    )
                                    .await,
                                Ok(ApprovalChoice::AllowOnce)
                            )
                        })
                    });
                    let continuing = continuation.load(Ordering::SeqCst);
                    let selected = if continuing { Ok(None) } else { policy };
                    let result = manager
                        .dispatch(
                            RequestContext {
                                chat_id: "chat",
                                provider: "claude",
                                skill: if continuing { None } else { Some("fixture") },
                                selected: &selected,
                                root,
                                visible: false,
                                cancel: CancellationToken::new(),
                                notify,
                                ask,
                            },
                            input,
                        )
                        .await;
                    if result["status"] == "human_required" {
                        continuation.store(true, Ordering::SeqCst);
                    }
                    result
                })
            });
            let chat_id = format!("test-{}", uuid::Uuid::new_v4());
            let guard =
                crate::setup_bridge::start(&chat_id, CancellationToken::new(), handler).unwrap();
            crate::setup_bridge::bind(&chat_id, Some(std::process::id()));
            let pipe = crate::setup_bridge::name(&chat_id).unwrap();
            let url = site.url.clone();
            let forbidden = other.url.clone();
            let cli = tokio::task::spawn_blocking(move || {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(pipe)
                    .unwrap();
                let mut client = crate::momso_mcp::test_bridge(file);
                let mut ready = true;
                let mut call = |name: &str, args: Value| {
                    let result=crate::momso_mcp::test_response(&json!({"id":1,"method":"tools/call","params":{"name":name,"arguments":args}}),&mut ready,&mut client).unwrap();
                    if result.get("error").is_some() {
                        return json!({"status":"invalid_parameters"});
                    }
                    serde_json::from_str::<Value>(
                        result["result"]["content"][0]["text"].as_str().unwrap(),
                    )
                    .unwrap()
                };
                assert_eq!(call("browser_open", json!({}))["status"], "ok");
                assert_eq!(
                    call("browser_navigate", json!({"url":format!("{url}/login")}))["status"],
                    "human_required"
                );
                assert_eq!(call("browser_open", json!({}))["status"], "ok");
                assert_eq!(
                    call("browser_navigate", json!({"url":forbidden}))["status"],
                    "blocked"
                );
                let page = call("browser_navigate", json!({"url":format!("{url}/work")}));
                assert_eq!(
                    page["kind"], "untrusted_page_data",
                    "safe status: {}",
                    page["status"]
                );
                let input = page["elements"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["name"] == "Lesson")
                    .unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                assert_eq!(
                    call("browser_fill", json!({"elementId":input,"text":"sample"}))["status"],
                    "read_only"
                );
                let id = page["elements"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["name"] == "게시")
                    .unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                assert_eq!(
                    call(
                        "browser_request_action",
                        json!({"action":"게시","description":"가짜 수업 게시", "elementId":id})
                    )["status"],
                    "approved"
                );
                assert_eq!(
                    call("browser_fill", json!({"elementId":input,"text":"sample"}))["status"],
                    "denied"
                );
                assert_eq!(
                    call("browser_click", json!({"elementId":id,"approved":true}))["status"],
                    "invalid_parameters"
                );
                let result = call("browser_click", json!({"elementId":id}));
                assert_eq!(call("browser_close", json!({}))["status"], "closed");
                result
            });
            let scope_request = tokio::time::timeout(Duration::from_secs(45), approved_rx.recv())
                .await
                .unwrap()
                .unwrap();
            broker
                .respond(&scope_request, ApprovalChoice::AllowOnce)
                .await
                .unwrap();
            let fill_request = tokio::time::timeout(Duration::from_secs(20), approved_rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(site.posted.load(Ordering::SeqCst), 0);
            broker
                .respond(&fill_request, ApprovalChoice::Deny)
                .await
                .unwrap();
            let request = tokio::time::timeout(Duration::from_secs(20), approved_rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(site.posted.load(Ordering::SeqCst), 0);
            assert!(!cli.is_finished());
            broker.respond(&request, choice).await.unwrap();
            let result = tokio::time::timeout(Duration::from_secs(30), cli)
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
                site.posted.load(Ordering::SeqCst),
                usize::from(choice == ApprovalChoice::AllowOnce)
            );
            assert_eq!(other.posted.load(Ordering::SeqCst), 0);
            assert!(!manager.is_open("chat").await);
            let events = observed.lock().unwrap();
            assert!(events.iter().any(
                |(name, value)| name == "browser-human-required" && value["reason"] == "login"
            ));
            assert!(events
                .iter()
                .any(|(name, value)| name == "browser-session-updated" && value["open"] == false));
            drop(events);
            drop(guard);
        }
    }
}
