//! Trusted server-side adapter. Browser tool arguments never grant approval or contain login secrets.
use super::*;
use momso_saved_login_core::linux::{LinuxVault, LoginInfo, PhoneSaveRequest};
use momso_server_browser::{
    engine::BrowserPermission,
    tools::{ApprovalChannel, BrowserTools, CredentialError, CredentialSource},
    vault::{LoginSelector, VaultCredentials},
    LoginSecret, ServerBrowsers,
};
use std::sync::{Arc, Mutex};

fn secure_origin(origin: &str) -> bool {
    origin.starts_with("https://")
        || (cfg!(all(debug_assertions, feature = "test-fixtures"))
            && origin.starts_with("http://127.0.0.1:"))
}
struct Selector {
    requests: human_prompt::Requests,
    chat: String,
    turn: String,
    cancel: watch::Receiver<bool>,
}
impl LoginSelector for Selector {
    fn select(&mut self, origin: &str, choices: &[LoginInfo]) -> Option<String> {
        if choices.len() == 1 {
            return Some(choices[0].id.clone());
        }
        if choices.is_empty() {
            return None;
        }
        let choices = choices
            .iter()
            .map(|entry| json!({"id":entry.id,"label":entry.label,"username":entry.username}))
            .collect();
        match human_prompt::wait_with_choices(
            &self.requests,
            &self.chat,
            &self.turn,
            origin,
            "select_saved_login",
            &self.cancel,
            choices,
        ) {
            Some(human_prompt::Answer::Selection(id)) => Some(id),
            _ => None,
        }
    }
}
pub(crate) struct Credentials {
    data: PathBuf,
    requests: human_prompt::Requests,
    chat: String,
    turn: String,
    cancel: watch::Receiver<bool>,
    credential_file: Option<PathBuf>,
    vault_lock: Arc<Mutex<()>>,
}
impl Credentials {
    fn vault(&self) -> Result<VaultCredentials<Selector>, CredentialError> {
        let directory = self.data.join("saved-logins");
        let vault = match &self.credential_file {
            Some(path) => LinuxVault::from_credential_file(&directory, path),
            None => LinuxVault::from_systemd(&directory),
        }
        .map_err(|_| CredentialError)?;
        Ok(VaultCredentials {
            vault,
            selector: Selector {
                requests: self.requests.clone(),
                chat: self.chat.clone(),
                turn: self.turn.clone(),
                cancel: self.cancel.clone(),
            },
        })
    }
}
impl CredentialSource for Credentials {
    fn protected_values(&mut self) -> Result<Vec<String>, CredentialError> {
        self.vault()?.protected_values()
    }
    fn login_for_origin(
        &mut self,
        origin: &str,
        url: &str,
    ) -> Result<Option<LoginSecret>, CredentialError> {
        if !secure_origin(origin) {
            return Ok(None);
        }
        let mut adapter = self.vault()?;
        if let Some(login) = adapter.login_for_origin(origin, url)? {
            return Ok(Some(login));
        }
        let existing = adapter.vault.list().map_err(|_| CredentialError)?;
        if existing.iter().any(|entry| entry.origin == origin) {
            return Ok(None);
        }
        let Some(human_prompt::Answer::Login {
            label,
            username,
            password,
        }) = human_prompt::wait(
            &self.requests,
            &self.chat,
            &self.turn,
            origin,
            "saved_login",
            &self.cancel,
        )
        else {
            return Ok(None);
        };
        let _guard = self.vault_lock.lock().map_err(|_| CredentialError)?;
        let saved = adapter
            .vault
            .save_from_phone(PhoneSaveRequest {
                site_url: origin.to_owned(),
                label,
                username,
                password: password.to_string(),
                ask_before_use: false,
                login_path: None,
                default_for_path: false,
            })
            .map_err(|_| CredentialError)?;
        let login = adapter
            .vault
            .browser_login(&saved.id, url, url, true)
            .map_err(|_| CredentialError)?;
        Ok(Some(LoginSecret::new(login.username, login.password)))
    }
}
pub(crate) struct HostApproval {
    data: PathBuf,
    turn: String,
    cancel: watch::Receiver<bool>,
    requests: human_prompt::Requests,
}
impl HostApproval {
    pub(crate) fn new(app: &App, turn: &str, cancel: watch::Receiver<bool>) -> Self {
        Self {
            data: app.data.clone(),
            turn: turn.to_owned(),
            cancel,
            requests: app.human_inputs.clone(),
        }
    }
    pub(crate) fn ask(&self, chat: &str, kind: &str, site: &str) -> bool {
        let Ok(conn) = Connection::open(self.data.join("server.sqlite3")) else {
            return false;
        };
        let id = Uuid::new_v4().to_string();
        let options = json!(["allow_once", "deny"]);
        if conn
            .execute(
                "INSERT INTO approvals(id,turn_id,chat_id,kind,description,options,state) VALUES (?1,?2,?3,?4,?5,?6,'pending')",
                params![id, self.turn, chat, kind, site, options.to_string()],
            )
            .is_err()
        {
            return false;
        }
        let deadline = std::time::Instant::now() + approval_timeout();
        loop {
            if *self.cancel.borrow() || std::time::Instant::now() >= deadline {
                let reason = if *self.cancel.borrow() {
                    "provider_cancelled"
                } else {
                    "timeout"
                };
                let _ = conn.execute(
                    "UPDATE approvals SET state='cancel',closed_reason=?2 WHERE id=?1 AND state='pending'",
                    params![id,reason],
                );
                return false;
            }
            let state: Result<String, _> =
                conn.query_row("SELECT state FROM approvals WHERE id=?1", [&id], |r| {
                    r.get(0)
                });
            match state.as_deref() {
                Ok("allow_once") => return true,
                Ok("pending") => {}
                _ => return false,
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
impl momso_server_crm::ApprovalChannel for HostApproval {
    fn approve_write(&mut self, chat: &str, method: &str, count: Option<usize>) -> bool {
        self.ask(
            chat,
            "crm_write",
            &format!("{} ({})", method, count.unwrap_or(0)),
        )
    }
    fn is_cancelled(&self) -> bool {
        *self.cancel.borrow()
    }
}
impl HostApproval {
    fn notify(&self, chat: &str, origin: &str, reason: &str) {
        if let Ok(conn) = Connection::open(self.data.join("server.sqlite3")) {
            let _ = conn.execute(
                "INSERT INTO browser_events(turn_id,chat_id,reason,origin) VALUES (?1,?2,?3,?4)",
                params![self.turn, chat, reason, origin],
            );
        }
    }
}
impl ApprovalChannel for HostApproval {
    fn approve_site(&mut self, chat: &str, host: &str) -> bool {
        self.ask(chat, "browser_site", host)
    }
    fn approve_login(&mut self, chat: &str, origin: &str) -> bool {
        self.ask(chat, "browser_login", origin)
    }
    fn human_login(&mut self, chat: &str, origin: &str) {
        self.notify(chat, origin, "verification");
    }
    fn one_time_code(&mut self, chat: &str, origin: &str) -> Option<zeroize::Zeroizing<String>> {
        if !secure_origin(origin) {
            return None;
        }
        match human_prompt::wait(
            &self.requests,
            chat,
            &self.turn,
            origin,
            "one_time_code",
            &self.cancel,
        ) {
            Some(human_prompt::Answer::Code(code)) => Some(code),
            _ => None,
        }
    }
}
type ToolSession = BrowserTools<HostApproval, Credentials>;
fn browser_executable_with(
    override_path: Option<PathBuf>,
    exists: impl Fn(&std::path::Path) -> bool,
) -> PathBuf {
    override_path.unwrap_or_else(|| {
        [
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
        ]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| exists(path))
        .unwrap_or_else(|| PathBuf::from("/usr/bin/google-chrome"))
    })
}
pub type Sessions = Arc<Mutex<HashMap<String, Arc<Mutex<ToolSession>>>>>;
pub fn sessions() -> Sessions {
    Arc::new(Mutex::new(HashMap::new()))
}
pub async fn dispatch(
    app: &App,
    chat: &str,
    turn: &str,
    name: &str,
    args: Value,
    source: (&str, &str),
    cancel: watch::Receiver<bool>,
) -> Value {
    let (permission_mode, provider) = source;
    let session = {
        let Ok(mut sessions) = app.browsers.lock() else {
            return json!({"status":"unavailable"});
        };
        sessions
            .entry(chat.to_owned())
            .or_insert_with(|| {
                let binary = browser_executable_with(
                    std::env::var_os("MOMSO_CHROMIUM_PATH").map(PathBuf::from),
                    |path| path.is_file(),
                );
                Arc::new(Mutex::new(BrowserTools::new(
                    ServerBrowsers::new(binary, app.data.join("browser-profiles")),
                    HostApproval {
                        data: app.data.clone(),
                        turn: turn.to_owned(),
                        cancel: cancel.clone(),
                        requests: app.human_inputs.clone(),
                    },
                    Credentials {
                        data: app.data.clone(),
                        requests: app.human_inputs.clone(),
                        chat: chat.to_owned(),
                        turn: turn.to_owned(),
                        cancel: cancel.clone(),
                        credential_file: None,
                        vault_lock: app.vault_lock.clone(),
                    },
                )))
            })
            .clone()
    };
    let chat = chat.to_owned();
    let turn = turn.to_owned();
    let name = name.to_owned();
    let mode = BrowserPermission::from_provider_mode("codex", permission_mode);
    let provider = provider.to_owned();
    let redact_app = app.clone();
    match tokio::task::spawn_blocking(move || {
        let Ok(mut tools) = session.lock() else {
            return json!({"status":"unavailable"});
        };
        tools.approval.turn = turn.clone();
        tools.approval.cancel = cancel.clone();
        tools.credentials.chat = chat.clone();
        tools.credentials.turn = turn;
        tools.credentials.cancel = cancel;
        let mut check = json!("");
        if otp_redaction::redact(&redact_app, &mut check).is_err() {
            return json!({"status":"redaction_unavailable"});
        }
        let mut result = tools.dispatch(&chat, mode, &name, args);
        if otp_redaction::redact(&redact_app, &mut result).is_err() {
            return json!({"status":"redaction_unavailable"});
        }
        if matches!(name.as_str(), "browser_click" | "browser_fill") && result["status"] == "ok" {
            let portable = format!("chat_{chat}");
            let record = momso_core::receipt_store::record_action(
                &redact_app.data.join("archive"),
                &provider,
                Some(&portable),
                &name,
            )
            .ok();
            result["recordStatus"] = json!(if record.is_some() {
                "recorded"
            } else {
                "unavailable"
            });
            result["recordId"] = json!(record);
        }
        result
    })
    .await
    {
        Ok(value) => value,
        Err(_) => json!({"status":"unavailable"}),
    }
}
pub async fn save_login(
    State(app): State<App>,
    h: HeaderMap,
    input: Result<Json<PhoneSaveRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<axum::response::Response, ApiError> {
    auth(&app, &h)?;
    let Json(input) = input.map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_input"))?;
    let data = app.data.clone();
    let work = WorkGuard::new(&app);
    let vault_lock = app.vault_lock.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _work = work;
        let _guard = vault_lock.lock().map_err(|_| "storage_unavailable")?;
        let vault = LinuxVault::from_systemd(&data.join("saved-logins"))?;
        vault.save_from_phone(input)
    })
    .await
    .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_unavailable"))?;
    match result {
        Ok(info) => {
            let mut response = Json(json!({"login":info})).into_response();
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-store"),
            );
            Ok(response)
        }
        Err(reason) => Err(error(StatusCode::BAD_REQUEST, reason)),
    }
}
fn safe_login_response(value: Value) -> axum::response::Response {
    let mut response = Json(value).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}
pub async fn list_logins(
    State(app): State<App>,
    h: HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    auth(&app, &h)?;
    let data = app.data.clone();
    let list = tokio::task::spawn_blocking(move || {
        LinuxVault::from_systemd(&data.join("saved-logins"))?.list()
    })
    .await
    .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_unavailable"))?
    .map_err(|reason| error(StatusCode::SERVICE_UNAVAILABLE, reason))?;
    Ok(safe_login_response(json!({"logins":list})))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateLogin {
    site_url: Option<String>,
    label: Option<String>,
    username: Option<String>,
    password: Option<String>,
    ask_before_use: Option<bool>,
    login_path: Option<String>,
    default_for_path: Option<bool>,
    favorite: Option<bool>,
}
pub async fn update_login(
    State(app): State<App>,
    Path(id): Path<String>,
    h: HeaderMap,
    input: Result<Json<UpdateLogin>, axum::extract::rejection::JsonRejection>,
) -> Result<axum::response::Response, ApiError> {
    auth(&app, &h)?;
    let Json(mut input) = input.map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_input"))?;
    let data = app.data.clone();
    let guard = app.vault_lock.clone();
    let work = WorkGuard::new(&app);
    let updated = tokio::task::spawn_blocking(move || {
        let _work = work;
        let _guard = guard.lock().map_err(|_| "storage_unavailable")?;
        let vault = LinuxVault::from_systemd(&data.join("saved-logins"))?;
        let existing = vault
            .list()?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or("login_not_found")?;
        let password = match input.password.take() {
            Some(value) => value,
            None => {
                vault
                    .browser_login(&id, &existing.site_url, &existing.site_url, true)?
                    .password
            }
        };
        if password.is_empty() {
            return Err("invalid_input");
        }
        let login_path = match input.login_path.take() {
            Some(value) if value.is_empty() => None,
            Some(value) => Some(value),
            None => existing.login_path,
        };
        let update = PhoneSaveRequest {
            site_url: input.site_url.take().unwrap_or(existing.site_url),
            label: input.label.take().unwrap_or(existing.label),
            username: input.username.take().unwrap_or(existing.username),
            password,
            ask_before_use: input.ask_before_use.unwrap_or(existing.ask_before_use),
            login_path,
            default_for_path: input.default_for_path.unwrap_or(existing.default_for_path),
        };
        vault.update_from_phone(&id, update, input.favorite.unwrap_or(existing.favorite))
    })
    .await
    .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_unavailable"))?
    .map_err(|reason| {
        error(
            if reason == "login_not_found" {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            },
            reason,
        )
    })?;
    Ok(safe_login_response(json!({"login":updated})))
}
pub async fn delete_login(
    State(app): State<App>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    auth(&app, &h)?;
    let data = app.data.clone();
    let guard = app.vault_lock.clone();
    let work = WorkGuard::new(&app);
    tokio::task::spawn_blocking(move || {
        let _work = work;
        let _guard = guard.lock().map_err(|_| "storage_unavailable")?;
        LinuxVault::from_systemd(&data.join("saved-logins"))?.delete(&id)
    })
    .await
    .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_unavailable"))?
    .map_err(|reason| error(StatusCode::BAD_REQUEST, reason))?;
    Ok(safe_login_response(json!({"state":"deleted"})))
}
#[cfg(test)]
mod tests {
    #[test]
    fn installed_chromium_and_explicit_browser_are_selected() {
        use super::browser_executable_with;
        use std::path::PathBuf;
        assert_eq!(
            browser_executable_with(None, |path| path
                == std::path::Path::new("/usr/bin/chromium")),
            PathBuf::from("/usr/bin/chromium")
        );
        assert_eq!(
            browser_executable_with(Some(PathBuf::from("/fixture/browser")), |_| true),
            PathBuf::from("/fixture/browser")
        );
    }
    use super::*;
    use age::secrecy::ExposeSecret;
    #[tokio::test]
    async fn saved_login_prompt_reconnects_and_stores_without_echo() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let identity = age::x25519::Identity::generate().to_string();
        let key = temp.path().join("key");
        std::fs::write(&key, identity.expose_secret()).unwrap();
        let app = App {
            data: data.clone(),
            token: Arc::from("fixture-token"),
            codex: PathBuf::new(),
            active: Arc::new(Mutex::new(HashMap::new())),
            writes: Arc::new(AtomicUsize::new(0)),
            maintenance: maintenance::new(),
            github_logins: Arc::new(AtomicUsize::new(0)),
            vault_lock: Arc::new(Mutex::new(())),
            web: None,
            login: Arc::new(Mutex::new(None)),
            browsers: sessions(),
            human_inputs: human_prompt::requests(),
            github: Arc::new(Mutex::new(None)),
            backup: github_worker::Control::new(),
        };
        initialize(&app).unwrap();
        let (_tx, rx) = watch::channel(false);
        let mut credentials = Credentials {
            data: data.clone(),
            requests: app.human_inputs.clone(),
            chat: "chat".into(),
            turn: "turn".into(),
            cancel: rx,
            credential_file: Some(key.clone()),
            vault_lock: app.vault_lock.clone(),
        };
        let worker = tokio::task::spawn_blocking(move || {
            credentials.login_for_origin("https://example.test", "https://example.test/login")
        });
        let request = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let list = human_prompt::list(&app.human_inputs, "turn");
                if let Some(first) = list.first() {
                    break first.clone();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(request["kind"], "saved_login");
        assert_eq!(
            human_prompt::list(&app.human_inputs, "turn")[0]["requestId"],
            request["requestId"]
        );
        let secret = "fixture-secret-not-for-chat";
        let input: human_prompt::Input = serde_json::from_value(
            json!({"label":"fixture","username":"person","password":secret}),
        )
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_static("Bearer fixture-token"),
        );
        let result = human_prompt::respond(
            State(app.clone()),
            Path(request["requestId"].as_str().unwrap().to_owned()),
            headers,
            Ok(Json(input)),
        )
        .await
        .unwrap();
        assert!(!result.0.to_string().contains(secret));
        assert!(worker.await.unwrap().unwrap().is_some());
        assert!(human_prompt::list(&app.human_inputs, "turn").is_empty());
        let vault = LinuxVault::from_credential_file(&data.join("saved-logins"), &key).unwrap();
        let entries = vault.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            vault
                .browser_login(
                    &entries[0].id,
                    "https://example.test/login",
                    "https://example.test/login",
                    true
                )
                .unwrap()
                .password,
            secret
        );
        assert!(!std::fs::read(data.join("saved-logins/vault.age"))
            .unwrap()
            .windows(secret.len())
            .any(|w| w == secret.as_bytes()));
    }
}
