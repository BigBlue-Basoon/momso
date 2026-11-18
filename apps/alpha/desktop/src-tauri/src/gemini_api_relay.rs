//! Per-chat loopback relay: preserve Retry-After for agy's ACP error output.
//! The random path is a capability. Requests go only to the configured Gemini
//! origin; no request, response, credential, or URL is logged or persisted.
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

const MAX_HEADER: usize = 64 * 1024;
const MAX_BODY: usize = 24 * 1024 * 1024;
const MAX_ERROR: usize = 1024 * 1024;

pub(crate) struct Relay {
    pub(crate) base: String,
    pub(crate) signals: Arc<Signals>,
    stop: CancellationToken,
    task: JoinHandle<()>,
}

#[derive(Default)]
pub(crate) struct Signals {
    pub(crate) auth: AtomicBool,
    auth_notify: tokio::sync::Notify,
    pub(crate) rate: AtomicBool,
    pub(crate) server: AtomicBool,
    pub(crate) disconnected: AtomicBool,
}

impl Signals {
    pub(crate) async fn wait_auth(&self) {
        loop {
            let notified = self.auth_notify.notified();
            if self.auth.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
}

impl Relay {
    pub(crate) async fn start(upstream: &str) -> Result<Self, String> {
        let upstream =
            reqwest::Url::parse(upstream).map_err(|_| "AI 연결을 준비하지 못했습니다")?;
        if upstream.query().is_some()
            || upstream.fragment().is_some()
            || !upstream.username().is_empty()
            || upstream.password().is_some()
        {
            return Err("AI 연결을 준비하지 못했습니다".into());
        }
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| "AI 연결을 준비하지 못했습니다")?;
        let addr = listener
            .local_addr()
            .map_err(|_| "AI 연결을 준비하지 못했습니다")?;
        let capability = uuid::Uuid::new_v4().simple().to_string();
        let base = format!("http://{addr}/{capability}");
        let builder = reqwest::Client::builder();
        #[cfg(test)]
        let builder = builder.no_proxy();
        let client = builder
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| "AI 연결을 준비하지 못했습니다")?;
        let stop = CancellationToken::new();
        let signals = Arc::new(Signals::default());
        let observed = signals.clone();
        let cancel = stop.clone();
        let task = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! { _ = cancel.cancelled() => break, item = listener.accept() => item };
                let Ok((socket, peer)) = accepted else { break };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let client = client.clone();
                let upstream = upstream.clone();
                let capability = capability.clone();
                let child = cancel.clone();
                let signals = observed.clone();
                tokio::spawn(async move {
                    let outcome = tokio::select! { _ = child.cancelled() => None, result = serve(socket, client, upstream, &capability, signals) => Some(result) };
                    #[cfg(test)]
                    if std::env::var_os("MOMSO_LOCAL_API_TRACE").is_some() {
                        if let Some(Err(error)) = &outcome {
                            println!("LOCAL_RELAY_ERROR={:?}", error.kind());
                        }
                    }
                    #[cfg(not(test))]
                    let _ = outcome;
                });
            }
        });
        Ok(Self {
            base,
            signals,
            stop,
            task,
        })
    }
}
impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.cancel();
        self.task.abort();
    }
}

fn retry_seconds(value: &str) -> Option<u64> {
    let seconds = if let Ok(n) = value.trim().parse::<u64>() {
        n
    } else {
        let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
        date.timestamp()
            .saturating_sub(chrono::Utc::now().timestamp())
            .max(0) as u64
    };
    (seconds <= 604800).then_some(seconds)
}

fn preserve_retry_info(body: &[u8], retry: Option<u64>) -> Vec<u8> {
    let Some(seconds) = retry else {
        return body.to_vec();
    };
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return body.to_vec();
    };
    let Some(error) = value
        .get_mut("error")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return body.to_vec();
    };
    let details = error
        .entry("details")
        .or_insert_with(|| serde_json::json!([]));
    let Some(details) = details.as_array_mut() else {
        return body.to_vec();
    };
    if let Some(item) = details
        .iter_mut()
        .find(|item| item["@type"] == "type.googleapis.com/google.rpc.RetryInfo")
    {
        item["retryDelay"] = serde_json::json!(format!("{seconds}s"));
    } else {
        details.push(serde_json::json!({"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":format!("{seconds}s")}));
    }
    serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec())
}

async fn serve(
    mut socket: TcpStream,
    client: reqwest::Client,
    upstream: reqwest::Url,
    capability: &str,
    signals: Arc<Signals>,
) -> io::Result<()> {
    let mut data = Vec::new();
    let mut buf = [0_u8; 8192];
    let header_end = loop {
        let n = tokio::time::timeout(Duration::from_secs(15), socket.read(&mut buf))
            .await
            .map_err(|_| io::ErrorKind::TimedOut)??;
        if n == 0 {
            return Ok(());
        }
        data.extend_from_slice(&buf[..n]);
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
        if data.len() > MAX_HEADER {
            return Err(io::ErrorKind::InvalidData.into());
        }
    };
    if header_end > MAX_HEADER {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let headers =
        std::str::from_utf8(&data[..header_end]).map_err(|_| io::ErrorKind::InvalidData)?;
    let mut lines = headers.lines();
    let mut first = lines
        .next()
        .ok_or(io::ErrorKind::InvalidData)?
        .split_whitespace();
    let method = first.next().ok_or(io::ErrorKind::InvalidData)?;
    if !matches!(method, "GET" | "POST") {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let path = first.next().ok_or(io::ErrorKind::InvalidData)?;
    let prefix = format!("/{capability}/");
    let relative = path
        .strip_prefix(&prefix)
        .ok_or(io::ErrorKind::PermissionDenied)?;
    if !(relative.starts_with("v1beta/") || relative.starts_with("v1/"))
        || relative.contains("..")
        || relative.contains('#')
        || relative.contains('\\')
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let target = upstream
        .join(relative)
        .map_err(|_| io::ErrorKind::InvalidData)?;
    if target.origin() != upstream.origin() {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let mut request = client.request(
        method.parse().map_err(|_| io::ErrorKind::InvalidData)?,
        target,
    );
    let mut content_length = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(io::ErrorKind::InvalidData.into());
            }
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| io::ErrorKind::InvalidData)?,
            );
            continue;
        }
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(io::ErrorKind::InvalidData.into());
        }
        if !["content-type", "accept", "user-agent", "authorization"]
            .iter()
            .any(|v| name.eq_ignore_ascii_case(v))
            && !name.to_ascii_lowercase().starts_with("x-goog-")
        {
            continue;
        }
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| io::ErrorKind::InvalidData)?;
        let value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|_| io::ErrorKind::InvalidData)?;
        request = request.header(name, value);
    }
    let length = content_length.unwrap_or(0);
    if length > MAX_BODY || data.len().saturating_sub(header_end) > length {
        return Err(io::ErrorKind::InvalidData.into());
    }
    while data.len() - header_end < length {
        let n = tokio::time::timeout(Duration::from_secs(30), socket.read(&mut buf))
            .await
            .map_err(|_| io::ErrorKind::TimedOut)??;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        data.extend_from_slice(&buf[..n]);
        if data.len().saturating_sub(header_end) > length {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
    if length > 0 {
        request = request.body(data.split_off(header_end));
    }
    let mut response = request.send().await.map_err(|_| {
        signals.disconnected.store(true, Ordering::SeqCst);
        io::ErrorKind::ConnectionAborted
    })?;
    let status = response.status();
    match status.as_u16() {
        401 => {
            signals.auth.store(true, Ordering::SeqCst);
            signals.auth_notify.notify_one();
        }
        429 => signals.rate.store(true, Ordering::SeqCst),
        500..=599 => signals.server.store(true, Ordering::SeqCst),
        _ => (),
    }
    let retry = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(retry_seconds);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_owned();
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nConnection: close\r\n\r\n",
        status.as_u16(),
        status.canonical_reason().unwrap_or(""),
        content_type
    );
    socket.write_all(header.as_bytes()).await?;
    if status.as_u16() == 429 && retry.is_some() {
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            signals.disconnected.store(true, Ordering::SeqCst);
            io::ErrorKind::ConnectionAborted
        })? {
            if body.len().saturating_add(chunk.len()) > MAX_ERROR {
                return Err(io::ErrorKind::InvalidData.into());
            }
            body.extend_from_slice(&chunk);
        }
        socket.write_all(&preserve_retry_info(&body, retry)).await?;
    } else {
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            signals.disconnected.store(true, Ordering::SeqCst);
            io::ErrorKind::ConnectionAborted
        })? {
            socket.write_all(&chunk).await?;
        }
    }
    socket.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_after_is_injected_and_takes_precedence_when_present() {
        let original = br#"{"error":{"code":429,"status":"RESOURCE_EXHAUSTED"}}"#;
        let output: serde_json::Value =
            serde_json::from_slice(&preserve_retry_info(original, Some(120))).unwrap();
        assert_eq!(output["error"]["details"][0]["retryDelay"], "120s");
        let original = br#"{"error":{"details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"10s"}]}}"#;
        let output: serde_json::Value =
            serde_json::from_slice(&preserve_retry_info(original, Some(120))).unwrap();
        assert_eq!(output["error"]["details"][0]["retryDelay"], "120s");
        assert_eq!(preserve_retry_info(b"not-json", Some(120)), b"not-json");
        assert_eq!(retry_seconds("120"), Some(120));
    }

    #[tokio::test]
    async fn relay_rejects_unknown_capability_and_preserves_retry_header() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let upstream = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0_u8; 4096];
            while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = socket.read(&mut buf).await.unwrap();
                assert_ne!(n, 0);
                bytes.extend_from_slice(&buf[..n]);
            }
            assert!(bytes
                .windows(b"x-goog-api-key: fixture-key".len())
                .any(|w| w.eq_ignore_ascii_case(b"x-goog-api-key: fixture-key")));
            let body = r#"{"error":{"code":429,"status":"RESOURCE_EXHAUSTED"}}"#;
            socket.write_all(format!("HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nRetry-After: 120\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        });
        let relay = Relay::start(&format!("http://{address}/")).await.unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let address = reqwest::Url::parse(&relay.base).unwrap();
        let wrong = format!(
            "http://{}/wrong/v1beta/models/test",
            address.socket_addrs(|| None).unwrap()[0]
        );
        assert!(client.get(wrong).send().await.is_err());
        let response = client
            .post(format!(
                "{}/v1beta/models/test:streamGenerateContent",
                relay.base
            ))
            .header("x-goog-api-key", "fixture-key")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 429);
        let value: serde_json::Value = response.json().await.unwrap();
        assert_eq!(value["error"]["details"][0]["retryDelay"], "120s");
        assert!(relay.signals.rate.load(Ordering::SeqCst));
        upstream.await.unwrap();
    }
}
