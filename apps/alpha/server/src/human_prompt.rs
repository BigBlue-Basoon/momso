//! Short-lived, authenticated human input requests. Values never enter SQLite or logs.
use super::*;
use std::sync::{mpsc, Arc, Mutex};
use zeroize::Zeroizing;

pub enum Answer {
    Code(Zeroizing<String>),
    Login {
        label: String,
        username: String,
        password: Zeroizing<String>,
    },
    Selection(String),
    Cancel,
}
#[derive(Clone)]
pub(crate) struct Pending {
    id: String,
    chat: String,
    turn: String,
    origin: String,
    kind: &'static str,
    expires_at: String,
    send: mpsc::SyncSender<Answer>,
    choices: Vec<Value>,
}
pub type Requests = Arc<Mutex<HashMap<String, Pending>>>;
pub fn requests() -> Requests {
    Arc::new(Mutex::new(HashMap::new()))
}
fn value(p: &Pending) -> Value {
    let fields = match p.kind {
        "one_time_code" => json!([{"id":"code","secret":true}]),
        "saved_login" => {
            json!([{"id":"label","secret":false},{"id":"username","secret":false},{"id":"password","secret":true}])
        }
        "select_saved_login" => json!([{"id":"selectionId","secret":false}]),
        "provider_api_key" => json!([{"id":"apiKey","secret":true}]),
        _ => json!([]),
    };
    json!({"requestId":p.id,"chatId":p.chat,"turnId":p.turn,"kind":p.kind,"origin":if p.kind == "provider_api_key" {None} else {Some(&p.origin)},"provider":if p.kind == "provider_api_key" {Some(&p.origin)} else {None},"expiresAt":p.expires_at,"fields":fields,"choices":p.choices})
}

pub async fn start_provider_key(
    State(app): State<App>,
    Path(provider): Path<String>,
    h: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    auth(&app, &h)?;
    if !matches!(provider.as_str(), "claude" | "gemini") {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_provider"));
    }
    let id = Uuid::new_v4().to_string();
    let (tx, _rx) = mpsc::sync_channel(1);
    let request = Pending {
        id: id.clone(),
        chat: String::new(),
        turn: String::new(),
        origin: provider,
        kind: "provider_api_key",
        expires_at: (chrono::Utc::now() + chrono::Duration::seconds(300)).to_rfc3339(),
        send: tx,
        choices: vec![],
    };
    let output = value(&request);
    let mut requests = app
        .human_inputs
        .lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?;
    prune(&mut requests);
    requests.insert(id, request);
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(output)))
}
pub fn count(requests: &Requests) -> Result<usize, ApiError> {
    requests
        .lock()
        .map(|mut map| {
            prune(&mut map);
            map.len()
        })
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))
}
fn prune(map: &mut HashMap<String, Pending>) {
    let now = chrono::Utc::now();
    map.retain(|_, p| {
        p.kind != "provider_api_key"
            || chrono::DateTime::parse_from_rfc3339(&p.expires_at).is_ok_and(|date| date > now)
    });
}
pub async fn provider_key_pending(
    State(app): State<App>,
    Path(provider): Path<String>,
    h: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    auth(&app, &h)?;
    if !matches!(provider.as_str(), "claude" | "gemini") {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_provider"));
    }
    let mut requests = app
        .human_inputs
        .lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?;
    prune(&mut requests);
    let pending: Vec<Value> = requests
        .values()
        .filter(|p| p.kind == "provider_api_key" && p.origin == provider)
        .map(value)
        .collect();
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"inputs":pending})),
    ))
}
pub fn list(requests: &Requests, turn: &str) -> Vec<Value> {
    requests
        .lock()
        .map(|m| m.values().filter(|p| p.turn == turn).map(value).collect())
        .unwrap_or_default()
}
pub fn wait(
    requests: &Requests,
    chat: &str,
    turn: &str,
    origin: &str,
    kind: &'static str,
    cancel: &watch::Receiver<bool>,
) -> Option<Answer> {
    wait_with_choices(requests, chat, turn, origin, kind, cancel, Vec::new())
}
pub fn wait_with_choices(
    requests: &Requests,
    chat: &str,
    turn: &str,
    origin: &str,
    kind: &'static str,
    cancel: &watch::Receiver<bool>,
    choices: Vec<Value>,
) -> Option<Answer> {
    let (tx, rx) = mpsc::sync_channel(1);
    let id = Uuid::new_v4().to_string();
    let pending = Pending {
        id: id.clone(),
        chat: chat.to_owned(),
        turn: turn.to_owned(),
        origin: origin.to_owned(),
        kind,
        expires_at: (chrono::Utc::now() + chrono::Duration::seconds(300)).to_rfc3339(),
        send: tx,
        choices,
    };
    requests.lock().ok()?.insert(id.clone(), pending);
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    let result = loop {
        if *cancel.borrow() || std::time::Instant::now() >= deadline {
            break None;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(answer) => {
                if *cancel.borrow() {
                    break None;
                }
                break Some(answer);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break None,
        }
    };
    if let Ok(mut map) = requests.lock() {
        map.remove(&id);
    }
    result
}
#[derive(Deserialize)]
pub struct Input {
    code: Option<String>,
    label: Option<String>,
    username: Option<String>,
    password: Option<String>,
    #[serde(rename = "selectionId")]
    selection_id: Option<String>,
    #[serde(rename = "apiKey")]
    api_key: Option<String>,
}
pub async fn respond(
    State(app): State<App>,
    Path(id): Path<String>,
    h: HeaderMap,
    input: Result<Json<Input>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    let Json(mut input) = input.map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_input"))?;
    let pending = app
        .human_inputs
        .lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?
        .get(&id)
        .cloned()
        .ok_or_else(|| error(StatusCode::CONFLICT, "request_closed"))?;
    if pending.kind == "provider_api_key" {
        let expires = chrono::DateTime::parse_from_rfc3339(&pending.expires_at)
            .map_err(|_| error(StatusCode::CONFLICT, "request_closed"))?;
        if chrono::Utc::now() >= expires {
            app.human_inputs.lock().ok().map(|mut m| m.remove(&id));
            return Err(error(StatusCode::CONFLICT, "request_closed"));
        }
        let key = momso_server_providers::ApiKey::new(
            input
                .api_key
                .take()
                .ok_or_else(|| error(StatusCode::BAD_REQUEST, "invalid_input"))?,
        )
        .map_err(|reason| error(StatusCode::BAD_REQUEST, reason))?;
        let provider = if pending.origin == "claude" {
            momso_server_providers::Provider::Claude
        } else {
            momso_server_providers::Provider::Gemini
        };
        let _vault_guard = app
            .vault_lock
            .lock()
            .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?;
        let mut requests = app
            .human_inputs
            .lock()
            .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?;
        if !requests.contains_key(&id) {
            return Err(error(StatusCode::CONFLICT, "request_closed"));
        }
        let vault =
            momso_server_providers::vault::open(&app.data.join("provider-credentials"), provider)
                .map_err(|reason| error(StatusCode::SERVICE_UNAVAILABLE, reason))?;
        momso_server_providers::vault::save(&vault, &key)
            .map_err(|reason| error(StatusCode::SERVICE_UNAVAILABLE, reason))?;
        requests.remove(&id);
        return Ok(Json(json!({"requestId":id,"state":"credential_saved"})));
    }
    let answer = match pending.kind {
        "one_time_code" => {
            let code = Zeroizing::new(
                input
                    .code
                    .take()
                    .ok_or_else(|| error(StatusCode::BAD_REQUEST, "invalid_input"))?,
            );
            if code.is_empty()
                || code.len() > 32
                || !code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                return Err(error(StatusCode::BAD_REQUEST, "invalid_input"));
            }
            otp_redaction::record(&app, &code)
                .map_err(|reason| error(StatusCode::SERVICE_UNAVAILABLE, reason))?;
            Answer::Code(code)
        }
        "select_saved_login" => {
            let selected = input
                .selection_id
                .take()
                .ok_or_else(|| error(StatusCode::BAD_REQUEST, "invalid_input"))?;
            if !pending
                .choices
                .iter()
                .any(|item| item["id"].as_str() == Some(selected.as_str()))
            {
                return Err(error(StatusCode::BAD_REQUEST, "invalid_input"));
            }
            Answer::Selection(selected)
        }
        "saved_login" => {
            let username = input
                .username
                .take()
                .ok_or_else(|| error(StatusCode::BAD_REQUEST, "invalid_input"))?;
            let password = Zeroizing::new(
                input
                    .password
                    .take()
                    .ok_or_else(|| error(StatusCode::BAD_REQUEST, "invalid_input"))?,
            );
            if username.is_empty() || password.is_empty() {
                return Err(error(StatusCode::BAD_REQUEST, "invalid_input"));
            }
            Answer::Login {
                label: input.label.take().unwrap_or_default(),
                username,
                password,
            }
        }
        _ => return Err(error(StatusCode::BAD_REQUEST, "invalid_input")),
    };
    let pending = app
        .human_inputs
        .lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?
        .remove(&id)
        .ok_or_else(|| error(StatusCode::CONFLICT, "request_closed"))?;
    pending
        .send
        .try_send(answer)
        .map_err(|_| error(StatusCode::CONFLICT, "request_closed"))?;
    Ok(Json(json!({"requestId":id,"state":"answered"})))
}
pub async fn cancel(
    State(app): State<App>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    let pending = app
        .human_inputs
        .lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?
        .remove(&id);
    if let Some(p) = pending {
        let _ = p.send.try_send(Answer::Cancel);
    }
    Ok(Json(json!({"requestId":id,"state":"cancelled"})))
}
pub async fn pending(
    State(app): State<App>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    turn(&app, &id)?;
    Ok(Json(
        json!({"approval":pending_approval(&app,&id)?,"inputs":list(&app.human_inputs,&id)}),
    ))
}
