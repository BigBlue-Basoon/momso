//! Provider model catalogue. Only fixed HTTPS endpoints receive credentials.
use crate::{ApiKey, Provider};
use serde_json::{json, Value};
use std::time::Duration;

/// Fast mode is documented for these exact direct-API model IDs; account tier may still reject it.
pub fn claude_fast_supported(model: &str) -> bool {
    matches!(
        model,
        "claude-fable-5-2" | "claude-mythos-5-2" | "claude-opus-5-5"
    )
}

/// Exact model IDs and supported thinking levels from the Gemini generateContent documentation.
pub fn gemini_efforts(model: &str) -> &'static [&'static str] {
    match model {
        "gemini-4-carbon" | "gemini-4-argon" | "gemini-3.8-flash" | "gemini-3.1-pro-preview" => {
            &["low", "medium", "high"]
        }
        "gemini-4-neon" | "gemini-4-hydrogen" => &["low", "medium", "high", "xhigh"],
        "gemini-3.6-flash"
        | "gemini-3.5-flash-lite"
        | "gemini-3.1-flash-lite"
        | "gemini-3-flash-preview"
        | "gemini-robotics-er-2-preview" => &["minimal", "low", "medium", "high"],
        "gemini-3.1-flash-lite-image" => &["minimal", "high"],
        "gemini-3-pro-preview" => &["low", "high"],
        _ => &[],
    }
}

pub async fn list(provider: Provider, key: &ApiKey) -> Result<Vec<Value>, &'static str> {
    let url = match provider {
        Provider::Claude => "https://api.anthropic.com/v1/models?limit=100",
        Provider::Gemini => "https://generativelanguage.googleapis.com/v1beta/models?pageSize=100",
    };
    list_at(provider, key, url).await
}

#[cfg(all(debug_assertions, feature = "test-fixtures"))]
pub async fn list_fixture(
    provider: Provider,
    key: &ApiKey,
    url: &str,
) -> Result<Vec<Value>, &'static str> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "unavailable")?;
    if parsed.scheme() != "http"
        || parsed.host_str() != Some("127.0.0.1")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("unavailable");
    }
    list_at(provider, key, url).await
}

async fn list_at(provider: Provider, key: &ApiKey, url: &str) -> Result<Vec<Value>, &'static str> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|_| "unavailable")?;
    let mut models = Vec::new();
    let mut cursor = String::new();
    for _ in 0..20 {
        let mut request = client.get(url);
        request = match provider {
            Provider::Claude => request
                .header("x-api-key", key.expose_to_transport())
                .header("anthropic-version", "2023-06-01"),
            Provider::Gemini => request.header("x-goog-api-key", key.expose_to_transport()),
        };
        if !cursor.is_empty() {
            request = request.query(&[(
                match provider {
                    Provider::Claude => "after_id",
                    Provider::Gemini => "pageToken",
                },
                cursor.as_str(),
            )]);
        }
        let mut response = request.send().await.map_err(|_| "unavailable")?;
        if !response.status().is_success() {
            return Err("unavailable");
        }
        if response.content_length().is_some_and(|n| n > 2_000_000) {
            return Err("unavailable");
        }
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "unavailable")? {
            if data.len().saturating_add(chunk.len()) > 2_000_000 {
                return Err("unavailable");
            }
            data.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&data).map_err(|_| "unavailable")?;
        let list = value
            .get(match provider {
                Provider::Claude => "data",
                Provider::Gemini => "models",
            })
            .and_then(Value::as_array)
            .ok_or("unavailable")?;
        for item in list {
            if matches!(provider, Provider::Gemini)
                && !item
                    .get("supportedGenerationMethods")
                    .and_then(Value::as_array)
                    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("generateContent")))
            {
                continue;
            }
            let id = item
                .get(match provider {
                    Provider::Claude => "id",
                    Provider::Gemini => "name",
                })
                .and_then(Value::as_str)
                .unwrap_or("")
                .strip_prefix("models/")
                .unwrap_or_else(|| item.get("id").and_then(Value::as_str).unwrap_or(""));
            if id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
                || id.contains(key.expose_to_transport())
            {
                continue;
            }
            let label = item
                .get(match provider {
                    Provider::Claude => "display_name",
                    Provider::Gemini => "displayName",
                })
                .and_then(Value::as_str)
                .filter(|s| {
                    s.len() <= 160
                        && !s.chars().any(char::is_control)
                        && !s.contains(key.expose_to_transport())
                })
                .unwrap_or(id);
            let efforts: Vec<Value> = if matches!(provider, Provider::Claude) {
                ["low", "medium", "high", "xhigh", "max"]
                    .into_iter()
                    .filter(|level| {
                        item.pointer(&format!("/capabilities/effort/{level}/supported"))
                            .and_then(Value::as_bool)
                            == Some(true)
                    })
                    .map(|level| json!({"id":level,"label":level}))
                    .collect()
            } else {
                gemini_efforts(id)
                    .iter()
                    .map(|level| json!({"id":level,"label":level}))
                    .collect()
            };
            let fast = matches!(provider, Provider::Claude) && claude_fast_supported(id);
            models.push(json!({"id":id,"label":label,"isDefault":null,"defaultEffort":null,"efforts":efforts,"fast":{"supported":fast,"state":if fast {"unknown"} else {"unavailable"}}}));
        }
        let next = match provider {
            Provider::Claude if value.get("has_more").and_then(Value::as_bool) == Some(true) => {
                Some(
                    value
                        .get("last_id")
                        .and_then(Value::as_str)
                        .ok_or("unavailable")?,
                )
            }
            Provider::Gemini => value
                .get("nextPageToken")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty()),
            _ => None,
        };
        match next {
            Some(s)
                if !s.is_empty()
                    && s.len() <= 512
                    && s != cursor
                    && !s.contains(key.expose_to_transport()) =>
            {
                cursor = s.to_owned()
            }
            None => {
                return Ok(models);
            }
            _ => return Err("unavailable"),
        }
    }
    Err("unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn fixture(body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = vec![0; 8192];
            let count = stream.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..count]);
            assert!(request.to_ascii_lowercase().contains("api-key:"));
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).as_bytes()).await.unwrap();
        });
        format!("http://{address}/models")
    }

    #[test]
    fn only_documented_models_expose_thinking_or_fast() {
        assert!(claude_fast_supported("claude-opus-5-5"));
        assert!(!claude_fast_supported("claude-unknown"));
        assert!(gemini_efforts("gemini-3-pro-preview").contains(&"high"));
        assert!(!gemini_efforts("gemini-3-pro-preview").contains(&"minimal"));
        assert!(gemini_efforts("gemini-unknown").is_empty());
    }

    #[tokio::test]
    async fn missing_pagination_cursor_never_returns_partial_catalog() {
        let key = ApiKey::new("fixture-secret-never-return".into()).unwrap();
        let endpoint = fixture(r#"{"data":[],"has_more":true,"last_id":null}"#).await;
        assert_eq!(
            list_at(Provider::Claude, &key, &endpoint).await,
            Err("unavailable")
        );
    }

    #[tokio::test]
    async fn connected_providers_list_models_without_returning_credentials() {
        let key = ApiKey::new("fixture-secret-never-return".into()).unwrap();
        let claude = fixture(
            r#"{"data":[{"id":"claude-fixture","display_name":"Fixture"}],"has_more":false}"#,
        )
        .await;
        let gemini=fixture(r#"{"models":[{"name":"models/gemini-fixture","displayName":"Gemini fixture","supportedGenerationMethods":["generateContent"]},{"name":"models/embedding","supportedGenerationMethods":["embedContent"]}],"nextPageToken":""}"#).await;
        let a = list_at(Provider::Claude, &key, &claude).await.unwrap();
        let b = list_at(Provider::Gemini, &key, &gemini).await.unwrap();
        assert_eq!(a[0]["id"], "claude-fixture");
        assert_eq!(b[0]["id"], "gemini-fixture");
        assert_eq!(b.len(), 1);
        assert!(!format!("{a:?}{b:?}").contains(key.expose_to_transport()));
    }
}
