//! Read provider-reported limits; never estimate remaining balance.
use super::*;

async fn codex_limits(app: &App) -> Result<Value, &'static str> {
    let operation = async {
        let mut child = Command::new(&app.codex)
            .arg("app-server")
            .env("CODEX_HOME", app.data.join("codex"))
            .env_remove("MOMSO_SERVER_TOKEN")
            .env_remove("OPENAI_API_KEY")
            .env_remove("CODEX_API_KEY")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("GEMINI_API_KEY")
            .env_remove("CREDENTIALS_DIRECTORY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| "cli_unavailable")?;
        let mut input = child.stdin.take().ok_or("fetch_failed")?;
        let mut lines = BufReader::new(child.stdout.take().ok_or("fetch_failed")?).lines();
        send(
            &mut input,
            codex_protocol::request(
                1,
                "initialize",
                json!({"clientInfo":{"name":"momso","title":"momso","version":"0.1.0"}}),
            ),
        )
        .await?;
        let mut account = Value::Null;
        for _ in 0..1000 {
            let line = lines
                .next_line()
                .await
                .map_err(|_| "fetch_failed")?
                .ok_or("fetch_failed")?;
            if line.len() > 2_000_000 {
                return Err("fetch_failed");
            }
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            match value.get("id").and_then(Value::as_u64) {
                Some(1) if value.get("error").is_none() => {
                    send(&mut input, json!({"method":"initialized","params":{}})).await?;
                    send(
                        &mut input,
                        codex_protocol::request(2, "account/read", json!({"refreshToken":false})),
                    )
                    .await?;
                }
                Some(2) if value.get("error").is_none() => {
                    account = value
                        .pointer("/result/account")
                        .cloned()
                        .unwrap_or(Value::Null);
                    if account.get("type").and_then(Value::as_str) != Some("chatgpt") {
                        return Err("not_connected");
                    }
                    send(
                        &mut input,
                        codex_protocol::request(3, "account/rateLimits/read", json!({})),
                    )
                    .await?;
                }
                Some(3) if value.get("error").is_none() => {
                    return Ok(normalize(
                        value.get("result").unwrap_or(&Value::Null),
                        &account,
                    ))
                }
                Some(1..=3) => return Err("fetch_failed"),
                _ => {}
            }
        }
        Err("fetch_failed")
    };
    tokio::time::timeout(Duration::from_secs(12), operation)
        .await
        .map_err(|_| "fetch_failed")?
}

fn normalize(result: &Value, account: &Value) -> Value {
    let buckets = result
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .filter(|b| !b.is_empty());
    let fallback = result.get("rateLimits").filter(|v| v.is_object());
    let mut windows = Vec::new();
    let mut add = |id: Option<&str>, value: &Value| {
        for kind in ["primary", "secondary"] {
            if let Some(window) = value.get(kind) {
                if let Some(percent) = window
                    .get("usedPercent")
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite() && *v >= 0.0)
                {
                    windows.push(json!({"kind":kind,"limitId":id,"usedPercent":percent,"windowDurationMins":window.get("windowDurationMins").and_then(Value::as_u64),"resetsAt":window.get("resetsAt").and_then(Value::as_i64)}));
                }
            }
        }
    };
    if let Some(buckets) = buckets {
        for (id, bucket) in buckets {
            if id.len() <= 128 && !id.chars().any(char::is_control) {
                add(Some(id), bucket);
            }
        }
    } else if let Some(fallback) = fallback {
        add(None, fallback);
    }
    let plan = buckets
        .and_then(|b| b.get("codex"))
        .or(fallback)
        .and_then(|b| b.get("planType"))
        .or_else(|| account.get("planType"))
        .and_then(Value::as_str)
        .filter(|v| v.len() <= 128 && !v.chars().any(char::is_control));
    json!({"provider":"codex","state":if windows.is_empty(){"unavailable"}else{"ok"},"reason":if windows.is_empty(){Some("fetch_failed")}else{None},"windows":windows,"plan":plan,"remaining":null})
}

pub async fn usage(State(app): State<App>, h: HeaderMap) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    let _guard = ReadGuard::new(&app);
    let codex = match provider_login::status(&app).await {
        "connected" => match codex_limits(&app).await {
            Ok(v) => v,
            Err(reason) => {
                json!({"provider":"codex","state":"unavailable","reason":reason,"windows":[],"plan":null,"remaining":null})
            }
        },
        reason => {
            json!({"provider":"codex","state":"unavailable","reason":reason,"windows":[],"plan":null,"remaining":null})
        }
    };
    Ok(Json(
        json!({"providers":[codex,{"provider":"claude","state":"unavailable","reason":"not_supported","windows":[],"plan":null,"remaining":null},{"provider":"gemini","state":"unavailable","reason":"not_supported","windows":[],"plan":null,"remaining":null}]}),
    ))
}
