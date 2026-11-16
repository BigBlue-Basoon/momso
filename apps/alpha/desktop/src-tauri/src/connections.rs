use crate::{credentials, process};
use serde::Serialize;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum ConnectionError {
    #[error(transparent)]
    Credential(#[from] credentials::CredentialError),
    #[error("지원하지 않는 연결 종류입니다")]
    UnsupportedProvider,
    #[error("API 키는 비어 있을 수 없습니다")]
    InvalidApiKey,
}

pub(crate) fn save_api_key(provider: &str, api_key: &str) -> Result<(), ConnectionError> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(ConnectionError::InvalidApiKey);
    }
    credentials::set_secret(credentials::ai_key_name(provider)?, api_key)?;
    Ok(())
}

pub(crate) fn delete_api_key(provider: &str) -> Result<(), ConnectionError> {
    credentials::delete_secret(credentials::ai_key_name(provider)?)?;
    Ok(())
}

/// A successful model-list response verifies API authentication only. It does not
/// prove a particular model, quota, or inference request will work.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AiApiConnectionCheck {
    provider: String,
    status: &'static str,
    runtime_available: bool,
}

fn api_probe_spec(provider: &str) -> Result<(&'static str, &'static str), ConnectionError> {
    match provider {
        "openai" => Ok(("https://api.openai.com/v1/models", "authorization")),
        "anthropic" => Ok(("https://api.anthropic.com/v1/models?limit=1", "x-api-key")),
        "gemini" => Ok((
            "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1",
            "x-goog-api-key",
        )),
        _ => Err(ConnectionError::UnsupportedProvider),
    }
}

fn probe_status(status: reqwest::StatusCode) -> &'static str {
    match status.as_u16() {
        200 => "authenticated",
        401 => "rejected",
        429 => "rate_limited",
        _ => "unavailable",
    }
}

pub(crate) async fn verify_api_key(provider: &str, key: String) -> &'static str {
    if key.chars().any(char::is_whitespace) {
        return "rejected";
    }
    match api_probe_spec(provider) {
        Ok((url, header)) => probe_api_key(provider, url, header, key).await,
        Err(_) => "not_supported",
    }
}

pub(crate) async fn check_ai_api_connection(
    provider: &str,
) -> Result<AiApiConnectionCheck, ConnectionError> {
    let (url, header) = api_probe_spec(provider)?;
    let key = credentials::get_secret(credentials::ai_key_name(provider)?)?;
    let runtime_available = match provider {
        "openai" => process::resolve_tool("codex").is_some(),
        "anthropic" => process::resolve_tool("claude").is_some(),
        "gemini" => true,
        _ => false,
    };
    let status = if let Some(key) = key {
        probe_api_key(provider, url, header, key).await
    } else {
        "not_configured"
    };
    Ok(AiApiConnectionCheck {
        provider: provider.to_owned(),
        status,
        runtime_available,
    })
}

async fn probe_api_key(provider: &str, url: &str, header: &str, key: String) -> &'static str {
    // Never forward credentials on a redirect or return response bodies through IPC.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build();
    match client {
        Ok(client) => {
            let value = if provider == "openai" {
                format!("Bearer {key}")
            } else {
                key
            };
            let mut request = client.get(url).header(header, value);
            if provider == "anthropic" {
                request = request.header("anthropic-version", "2023-06-01");
            }
            match request.send().await {
                Ok(response) if provider == "gemini" && response.status().as_u16() == 400 => {
                    // Gemini reports invalid API keys as 400 with a structured reason.
                    // Never include the response body in an error or log.
                    match response.json::<serde_json::Value>().await {
                        Ok(body)
                            if body
                                .pointer("/error/details")
                                .and_then(|v| v.as_array())
                                .is_some_and(|details| {
                                    details.iter().any(|v| {
                                        v.get("reason").and_then(|v| v.as_str())
                                            == Some("API_KEY_INVALID")
                                    })
                                }) =>
                        {
                            "rejected"
                        }
                        _ => "unavailable",
                    }
                }
                Ok(response) => probe_status(response.status()),
                Err(_) => "unavailable",
            }
        }
        Err(_) => "unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn key_with_internal_whitespace_is_rejected_without_network() {
        assert_eq!(
            verify_api_key("gemini", "fake invalid key".into()).await,
            "rejected"
        );
    }

    #[tokio::test]
    async fn gemini_invalid_key_reason_is_rejected_without_exposing_response() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            let body =
                r#"{"error":{"details":[{"reason":"API_KEY_INVALID"}],"message":"do not expose"}}"#;
            write!(
                stream,
                "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let status = probe_api_key(
            "gemini",
            &format!("http://{address}"),
            "x-goog-api-key",
            "fake-key".into(),
        )
        .await;
        server.join().unwrap();
        assert_eq!(status, "rejected");
    }

    #[tokio::test]
    async fn probe_sends_anthropic_headers_and_treats_redirect_as_unavailable() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = [0_u8; 8192];
            let count = stream.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..count]).to_lowercase();
            stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            request
        });
        let status = probe_api_key(
            "anthropic",
            &format!("http://{address}/models"),
            "x-api-key",
            "test-key".into(),
        )
        .await;
        let request = server.join().unwrap();
        assert_eq!(status, "unavailable");
        assert!(request.contains("x-api-key: test-key"));
        assert!(request.contains("anthropic-version: 2023-06-01"));
    }

    #[test]
    fn api_probe_distinguishes_authentication_from_errors() {
        use reqwest::StatusCode;
        assert_eq!(probe_status(StatusCode::OK), "authenticated");
        assert_eq!(probe_status(StatusCode::UNAUTHORIZED), "rejected");
        assert_eq!(probe_status(StatusCode::FORBIDDEN), "unavailable");
        assert_eq!(probe_status(StatusCode::TOO_MANY_REQUESTS), "rate_limited");
        assert_eq!(probe_status(StatusCode::FOUND), "unavailable");
        assert_eq!(
            probe_status(StatusCode::INTERNAL_SERVER_ERROR),
            "unavailable"
        );
        assert!(api_probe_spec("unknown").is_err());
        assert_eq!(api_probe_spec("gemini").unwrap().1, "x-goog-api-key");
    }
}
