use super::*;
use axum::response::Response;
use std::sync::{Arc, Mutex};

const DEVICE_URL: &str = "https://auth.openai.com/codex/device";
#[derive(Clone)]
pub struct Login {
    pub session: String,
    pub state: &'static str,
    pub code: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub cancel: watch::Sender<bool>,
    pub done: watch::Receiver<bool>,
}
pub type LoginState = Arc<Mutex<Option<Login>>>;

fn no_store<T: IntoResponse>(value: T) -> Response {
    let mut response = value.into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}
fn own_session(app: &App, headers: &HeaderMap) -> Result<Login, ApiError> {
    auth(app, headers)?;
    let session = headers
        .get("x-momso-login-session")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let current = app
        .login
        .lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?;
    current
        .as_ref()
        .filter(|x| !session.is_empty() && x.session == session)
        .cloned()
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "login_not_found"))
}
pub(super) async fn status(app: &App) -> &'static str {
    let mut command = Command::new(&app.codex);
    command
        .args(["login", "status"])
        .env("CODEX_HOME", app.data.join("codex"))
        .env_remove("MOMSO_SERVER_TOKEN")
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(Duration::from_secs(15), command.status()).await {
        Ok(Ok(status)) if status.success() => "connected",
        Ok(Ok(_)) => "not_connected",
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => "cli_unavailable",
        _ => "cli_unavailable",
    }
}
async fn account_label(app: &App) -> Option<String> {
    let operation = async {
        let mut child = Command::new(&app.codex)
            .arg("app-server")
            .env("CODEX_HOME", app.data.join("codex"))
            .env_remove("MOMSO_SERVER_TOKEN")
            .env_remove("OPENAI_API_KEY")
            .env_remove("CODEX_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        let mut input = child.stdin.take()?;
        let mut lines = BufReader::new(child.stdout.take()?).lines();
        send(
            &mut input,
            codex_protocol::request(
                1,
                "initialize",
                json!({"clientInfo":{"name":"momso","title":"momso","version":"0.1.0"}}),
            ),
        )
        .await
        .ok()?;
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if value.get("id").and_then(Value::as_u64) == Some(1) {
                if value.get("error").is_some() {
                    return None;
                }
                send(&mut input, json!({"method":"initialized","params":{}}))
                    .await
                    .ok()?;
                send(
                    &mut input,
                    codex_protocol::request(2, "account/read", json!({"refreshToken":true})),
                )
                .await
                .ok()?;
            } else if value.get("id").and_then(Value::as_u64) == Some(2) {
                return value
                    .pointer("/result/account/email")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty() && s.len() <= 320 && !s.chars().any(char::is_control))
                    .map(str::to_owned);
            }
        }
        None
    };
    tokio::time::timeout(Duration::from_secs(12), operation)
        .await
        .ok()
        .flatten()
}
pub async fn providers(State(app): State<App>, h: HeaderMap) -> Result<Response, ApiError> {
    auth(&app, &h)?;
    let state = status(&app).await;
    let label = if state == "connected" {
        account_label(&app).await
    } else {
        None
    };
    #[cfg(target_os = "linux")]
    let provider_state = |provider| match momso_server_providers::vault::open(
        &app.data.join("provider-credentials"),
        provider,
    )
    .and_then(|v| momso_server_providers::vault::load(&v))
    {
        Ok(Some(_)) => "credential_saved",
        Ok(None) => "not_connected",
        Err(_) => "storage_unavailable",
    };
    #[cfg(not(target_os = "linux"))]
    let (claude, gemini) = ("not_supported", "not_supported");
    #[cfg(target_os = "linux")]
    let (claude, gemini) = (
        provider_state(momso_server_providers::Provider::Claude),
        provider_state(momso_server_providers::Provider::Gemini),
    );
    Ok(no_store(Json(
        json!({"providers":[{"provider":"codex","state":state,"accountLabel":label},{"provider":"claude","state":claude,"accountLabel":null},{"provider":"gemini","state":gemini,"accountLabel":null}]}),
    )))
}

/// Disconnect only the server-owned credentials, without echoing CLI output.
pub async fn disconnect(
    State(app): State<App>,
    Path(provider): Path<String>,
    h: HeaderMap,
) -> Result<Response, ApiError> {
    auth(&app, &h)?;
    if !matches!(provider.as_str(), "codex" | "claude" | "gemini") {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_provider"));
    }
    let guard = WorkGuard::new(&app);
    let result = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let active = app.active.lock().map_err(|_| "unavailable")?;
        if !active.is_empty() {
            return Err("turn_in_progress");
        }
        #[cfg(target_os = "linux")]
        let mut login = app.login.lock().map_err(|_| "unavailable")?;
        #[cfg(not(target_os = "linux"))]
        let login = app.login.lock().map_err(|_| "unavailable")?;
        if provider == "codex"
            && login
                .as_ref()
                .is_some_and(|l| matches!(l.state, "starting" | "waiting"))
        {
            return Err("login_in_progress");
        }
        #[cfg(target_os = "linux")]
        {
            if provider == "codex" {
                let mut child = std::process::Command::new(&app.codex)
                    .arg("logout")
                    .env("CODEX_HOME", app.data.join("codex"))
                    .env_remove("MOMSO_SERVER_TOKEN")
                    .env_remove("OPENAI_API_KEY")
                    .env_remove("CODEX_API_KEY")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|_| "cli_unavailable")?;
                let deadline = std::time::Instant::now() + Duration::from_secs(15);
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) if status.success() => break,
                        Ok(Some(_)) => return Err("logout_failed"),
                        Ok(None) if std::time::Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(30))
                        }
                        _ => {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err("logout_failed");
                        }
                    }
                }
                *login = None;
            } else {
                let _vault = app
                    .vault_lock
                    .lock()
                    .map_err(|_| "credential_unavailable")?;
                let kind = if provider == "claude" {
                    momso_server_providers::Provider::Claude
                } else {
                    momso_server_providers::Provider::Gemini
                };
                let vault = momso_server_providers::vault::open(
                    &app.data.join("provider-credentials"),
                    kind,
                )
                .map_err(|_| "credential_unavailable")?;
                momso_server_providers::vault::clear(&vault)
                    .map_err(|_| "credential_unavailable")?;
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err("not_supported")
        }
    })
    .await
    .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?;
    result.map_err(|reason| {
        error(
            match reason {
                "turn_in_progress" | "login_in_progress" => StatusCode::CONFLICT,
                "not_supported" => StatusCode::NOT_IMPLEMENTED,
                _ => StatusCode::SERVICE_UNAVAILABLE,
            },
            reason,
        )
    })?;
    Ok(no_store(Json(json!({"state":"not_connected"}))))
}
fn update(app: &App, session: &str, state: &'static str, code: Option<String>) {
    if let Ok(mut current) = app.login.lock() {
        if let Some(login) = current.as_mut().filter(|l| l.session == session) {
            login.state = state;
            login.code = code;
        }
    }
}
pub async fn start(State(app): State<App>, h: HeaderMap) -> Result<Response, ApiError> {
    auth(&app, &h)?;
    let session = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let (tx, mut rx) = watch::channel(false);
    let (done_tx, done_rx) = watch::channel(false);
    let previous = {
        let mut current = app
            .login
            .lock()
            .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?;
        current.replace(Login {
            session: session.clone(),
            state: "starting",
            code: None,
            expires_at: None,
            cancel: tx,
            done: done_rx,
        })
    };
    if let Some(old) = previous.as_ref() {
        let _ = old.cancel.send(true);
    }
    let a = app.clone();
    let id = session.clone();
    tokio::spawn(async move {
        if let Some(old) = previous {
            let mut previous_done = old.done;
            let already_done = *previous_done.borrow();
            if !already_done {
                let waiting = tokio::select! {
                    biased;
                    _=rx.changed()=>false,
                    result=tokio::time::timeout(Duration::from_secs(20),previous_done.changed())=>result.is_ok(),
                };
                if !waiting || *rx.borrow() {
                    update(
                        &a,
                        &id,
                        if *rx.borrow() { "cancelled" } else { "failed" },
                        None,
                    );
                    let _ = done_tx.send(true);
                    return;
                }
            }
        }
        if *rx.borrow() {
            update(&a, &id, "cancelled", None);
        } else {
            run(&a, &id, rx).await;
        }
        let _ = done_tx.send(true);
    });
    Ok(no_store(Json(json!({"sessionId":session}))))
}
fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            if it.next() == Some('[') {
                for c in it.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out
}
fn code_valid(s: &str) -> bool {
    (6..=20).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-')
}
async fn pipe<R: tokio::io::AsyncRead + Unpin>(reader: R, tx: tokio::sync::mpsc::Sender<String>) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tx.send(line).await.is_err() {
            break;
        }
    }
}
async fn run(app: &App, session: &str, mut cancel: watch::Receiver<bool>) {
    let child = Command::new(&app.codex)
        .args(["login", "--device-auth"])
        .env("CODEX_HOME", app.data.join("codex"))
        .env("NO_COLOR", "1")
        .env_remove("MOMSO_SERVER_TOKEN")
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let Ok(mut child) = child else {
        update(app, session, "cli_unavailable", None);
        return;
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    if let Some(out) = child.stdout.take() {
        tokio::spawn(pipe(out, tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(pipe(err, tx.clone()));
    }
    drop(tx);
    let mut url_seen = false;
    let mut code_next = false;
    let mut expires_in: Option<i64> = None;
    let timer = tokio::time::sleep(Duration::from_secs(900));
    tokio::pin!(timer);
    loop {
        tokio::select! {
            _=cancel.changed()=> { let _=child.kill().await; update(app,session,"cancelled",None); return; }
            _=&mut timer=> { let _=child.kill().await; update(app,session,"expired",None); return; }
            Some(line)=rx.recv()=> {
                let cleaned=strip_ansi(&line); let line=cleaned.trim();
                if line==DEVICE_URL {url_seen=true;}
                else if line.starts_with("2. Enter this one-time code") {code_next=true; expires_in=line.split("expires in ").nth(1).and_then(|rest|rest.split_whitespace().next()).and_then(|n|n.parse::<i64>().ok()).filter(|n|*n>0 && *n<=60).map(|n|n*60);}
                else if code_next && !line.is_empty() {code_next=false; if url_seen && code_valid(line) {update(app,session,"waiting",Some(line.to_owned())); if let Ok(mut state)=app.login.lock() { if let Some(login)=state.as_mut().filter(|l|l.session==session) { login.expires_at=expires_in.map(|seconds|chrono::Utc::now()+chrono::Duration::seconds(seconds)); } }}}
            },
            result=child.wait()=> {
                let state=match result {
                    Ok(s) if s.success()=>tokio::select! {biased; _=cancel.changed()=>"cancelled", status=status(app)=>if status=="connected"{"connected"}else{"failed"}},
                    _=>"failed",
                };
                update(app,session,state,None); return;
            }
        }
    }
}
fn view(login: &Login) -> Value {
    json!({"provider":"codex","state":login.state,"url":login.code.as_ref().map(|_|DEVICE_URL),"code":login.code,"expiresInSeconds":login.expires_at.map(|t|(t-chrono::Utc::now()).num_seconds().max(0)),"expiresAt":login.expires_at.map(|t|t.to_rfc3339())})
}
pub async fn get(State(app): State<App>, h: HeaderMap) -> Result<Response, ApiError> {
    Ok(no_store(Json(view(&own_session(&app, &h)?))))
}
pub async fn cancel(State(app): State<App>, h: HeaderMap) -> Result<Response, ApiError> {
    let login = own_session(&app, &h)?;
    let _ = login.cancel.send(true);
    Ok(no_store(Json(
        json!({"provider":"codex","state":"cancelling"}),
    )))
}
pub async fn events(State(app): State<App>, h: HeaderMap) -> Result<Response, ApiError> {
    let login = own_session(&app, &h)?;
    let id = login.session;
    let stream = futures_util::stream::unfold(
        (app, id, None::<String>, false),
        |(app, id, last, done)| async move {
            if done {
                return None;
            }
            loop {
                let current = app
                    .login
                    .lock()
                    .ok()
                    .and_then(|v| v.as_ref().filter(|l| l.session == id).cloned());
                let login = current?;
                let value = view(&login);
                let key = format!("{}:{}", login.state, login.code.as_deref().unwrap_or(""));
                let terminal = !matches!(login.state, "starting" | "waiting");
                if Some(&key) != last.as_ref() {
                    let kind = if terminal {
                        "provider-login-device-closed"
                    } else if login.code.is_some() {
                        "provider-login-device"
                    } else {
                        "provider-login-state"
                    };
                    let event = Event::default().event(kind).data(if terminal {
                        json!({"provider":"codex","state":login.state}).to_string()
                    } else {
                        value.to_string()
                    });
                    return Some((Ok::<_, Infallible>(event), (app, id, Some(key), terminal)));
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        },
    );
    Ok(no_store(Sse::new(stream).keep_alive(KeepAlive::default())))
}
