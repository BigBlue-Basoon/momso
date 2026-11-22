use super::*;
use axum::response::Response;

/// Same model capability list used by the desktop; account eligibility is unknown.
pub(super) fn codex_fast_supported(model: &str) -> bool {
    matches!(
        model,
        "gpt-6.1-sol" | "gpt-6.1-terra" | "gpt-6-astra" | "gpt-6-sol" | "gpt-6-luna"
    )
}

async fn codex_models(app: &App) -> Result<Vec<Value>, &'static str> {
    let operation = async {
        let mut child = Command::new(&app.codex)
            .arg("app-server")
            .env("CODEX_HOME", app.data.join("codex"))
            .env_remove("MOMSO_SERVER_TOKEN")
            .env_remove("CREDENTIALS_DIRECTORY")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("GEMINI_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| "unavailable")?;
        let mut input = child.stdin.take().ok_or("unavailable")?;
        let mut lines = BufReader::new(child.stdout.take().ok_or("unavailable")?).lines();
        send(
            &mut input,
            codex_protocol::request(
                1,
                "initialize",
                json!({"clientInfo":{"name":"momso","title":"momso","version":"0.1.0"}}),
            ),
        )
        .await?;
        let mut phase = 1u64;
        let mut models = Vec::new();
        let mut cursor = Value::Null;
        for _ in 0..1000 {
            let line = lines
                .next_line()
                .await
                .map_err(|_| "unavailable")?
                .ok_or("unavailable")?;
            if line.len() > 2_000_000 {
                return Err("unavailable");
            }
            let value: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if value.get("id").and_then(Value::as_u64) != Some(phase) {
                continue;
            }
            if value.get("error").is_some() {
                return Err("unavailable");
            }
            if phase == 1 {
                send(&mut input, json!({"method":"initialized","params":{}})).await?;
            } else {
                let result = value.get("result").ok_or("unavailable")?;
                for item in result
                    .get("data")
                    .and_then(Value::as_array)
                    .ok_or("unavailable")?
                {
                    let Some(id) = item
                        .get("model")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                    else {
                        continue;
                    };
                    if id.is_empty()
                        || id.len() > 128
                        || !id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
                    {
                        continue;
                    }
                    let label = item
                        .get("displayName")
                        .and_then(Value::as_str)
                        .filter(|s| s.len() <= 160 && !s.chars().any(char::is_control))
                        .unwrap_or(id);
                    let efforts: Vec<Value> = item
                        .get("supportedReasoningEfforts")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.get("reasoningEffort").and_then(Value::as_str))
                        .filter(|v| {
                            matches!(
                                *v,
                                "none"
                                    | "minimal"
                                    | "low"
                                    | "medium"
                                    | "high"
                                    | "xhigh"
                                    | "max"
                                    | "ultra"
                            )
                        })
                        .map(|v| json!({"id":v,"label":v}))
                        .collect();
                    let default_effort = item
                        .get("defaultReasoningEffort")
                        .and_then(Value::as_str)
                        .filter(|level| {
                            efforts
                                .iter()
                                .any(|entry| entry["id"].as_str() == Some(*level))
                        });
                    let fast = codex_fast_supported(id);
                    models.push(json!({"id":id,"label":label,"isDefault":item.get("isDefault").and_then(Value::as_bool),"defaultEffort":default_effort,"efforts":efforts,"fast":{"supported":fast,"state":if fast {"unknown"} else {"unavailable"}}}));
                }
                let next = result.get("nextCursor").filter(|v| !v.is_null()).cloned();
                match next {
                    None => return Ok(models),
                    Some(next) if next.is_string() && next != cursor => cursor = next,
                    _ => return Err("unavailable"),
                }
            }
            phase += 1;
            send(
                &mut input,
                codex_protocol::request(
                    phase,
                    "model/list",
                    json!({"limit":100,"includeHidden":false,"cursor":cursor}),
                ),
            )
            .await?;
        }
        Err("unavailable")
    };
    tokio::time::timeout(Duration::from_secs(15), operation)
        .await
        .map_err(|_| "unavailable")?
}

#[cfg(target_os = "linux")]
async fn api_models(app: &App, provider: momso_server_providers::Provider) -> Value {
    let vault =
        match momso_server_providers::vault::open(&app.data.join("provider-credentials"), provider)
        {
            Ok(v) => v,
            Err(_) => {
                return json!({"provider":provider.name(),"state":"storage_unavailable","models":[]})
            }
        };
    let key = match momso_server_providers::vault::load(&vault) {
        Ok(Some(v)) => v,
        Ok(None) => return json!({"provider":provider.name(),"state":"not_connected","models":[]}),
        Err(_) => {
            return json!({"provider":provider.name(),"state":"storage_unavailable","models":[]})
        }
    };
    #[cfg(all(debug_assertions, feature = "test-fixtures"))]
    let result = if let Some(endpoint) = std::env::var_os("MOMSO_TEST_MODEL_ENDPOINT") {
        momso_server_providers::models::list_fixture(provider, &key, &endpoint.to_string_lossy())
            .await
    } else {
        momso_server_providers::models::list(provider, &key).await
    };
    #[cfg(not(all(debug_assertions, feature = "test-fixtures")))]
    let result = momso_server_providers::models::list(provider, &key).await;
    match result {
        Ok(models) => json!({"provider":provider.name(),"state":"connected","models":models}),
        Err(_) => json!({"provider":provider.name(),"state":"unavailable","models":[]}),
    }
}

pub async fn list(State(app): State<App>, headers: HeaderMap) -> Result<Response, ApiError> {
    auth(&app, &headers)?;
    let codex_query = async {
        let status = provider_login::status(&app).await;
        if status != "connected" {
            json!({"provider":"codex","state":status,"models":[]})
        } else {
            match codex_models(&app).await {
                Ok(models) => json!({"provider":"codex","state":"connected","models":models}),
                Err(_) => json!({"provider":"codex","state":"unavailable","models":[]}),
            }
        }
    };
    #[cfg(target_os = "linux")]
    let (codex, claude, gemini) = tokio::join!(
        codex_query,
        api_models(&app, momso_server_providers::Provider::Claude),
        api_models(&app, momso_server_providers::Provider::Gemini)
    );
    #[cfg(not(target_os = "linux"))]
    let (codex, claude, gemini) = (
        codex_query.await,
        json!({"provider":"claude","state":"unavailable","models":[]}),
        json!({"provider":"gemini","state":"unavailable","models":[]}),
    );
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"providers":[codex,claude,gemini]})),
    )
        .into_response())
}
