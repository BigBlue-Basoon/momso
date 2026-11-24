use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn fake_github_device_login_and_refresh_never_put_tokens_in_urls() {
    if std::env::var_os("MOMSO_TEST_S110_CHILD").is_none() {
        let output = crate::process::hidden_command(std::env::current_exe().unwrap())
            .args(["--exact", "github_oauth::tests::fake_github_device_login_and_refresh_never_put_tokens_in_urls", "--test-threads=1"])
            .env("MOMSO_TEST_S110_CHILD", "1")
            .env("MOMSO_TEST_FAKE_CREDENTIAL_STORE", "1")
            .output().unwrap();
        assert!(output.status.success(), "isolated fake GitHub flow failed");
        return;
    }
    let access = format!("fixture-access-{}", uuid::Uuid::new_v4());
    let refresh = format!("fixture-refresh-{}", uuid::Uuid::new_v4());
    let next_access = format!("fixture-next-access-{}", uuid::Uuid::new_v4());
    let next_refresh = format!("fixture-next-refresh-{}", uuid::Uuid::new_v4());
    let expected_refresh = refresh.clone();
    let token1 = serde_json::json!({"access_token":access,"refresh_token":refresh,"expires_in":1,"refresh_token_expires_in":3600});
    let token2 = serde_json::json!({"access_token":next_access,"refresh_token":next_refresh,"expires_in":3600,"refresh_token_expires_in":7200});
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        for (step, body) in [
            serde_json::json!({"device_code":"device-fixture","user_code":"ABCD-EFGH","verification_uri":"https://github.com/login/device","expires_in":900,"interval":5}),
            token1,
            token2,
        ].into_iter().enumerate() {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 4096];
            let headers_end = loop {
                let n = stream.read(&mut buf).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buf[..n]);
                if let Some(offset) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    break offset + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..headers_end]);
            let first = headers.lines().next().unwrap();
            assert!(first.starts_with("POST /login/"));
            assert!(!first.contains('?'));
            let length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|n| n.trim().parse::<usize>().ok())
                })
                .unwrap();
            while request.len() < headers_end + length {
                let n = stream.read(&mut buf).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buf[..n]);
            }
            let body_text = std::str::from_utf8(&request[headers_end..headers_end + length]).unwrap();
            let url = reqwest::Url::parse(&format!("http://localhost/?{body_text}")).unwrap();
            let params: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
            assert_eq!(params.get("client_id").map(String::as_str), Some(CLIENT_ID));
            match step {
                0 => assert_eq!(params.get("scope").map(String::as_str), Some("repo")),
                1 => {
                    assert_eq!(params.get("device_code").map(String::as_str), Some("device-fixture"));
                    assert_eq!(params.get("grant_type").map(String::as_str), Some("urn:ietf:params:oauth:grant-type:device_code"));
                }
                2 => {
                    assert_eq!(params.get("grant_type").map(String::as_str), Some("refresh_token"));
                    assert_eq!(params.get("refresh_token"), Some(&expected_refresh));
                }
                _ => unreachable!(),
            }
            let body = body.to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
    });
    let grant = begin_at(&base).unwrap();
    let initial = poll_at(&base, &grant.device_code).unwrap();
    save(initial, &tokio_util::sync::CancellationToken::new()).unwrap();
    let first = read_bundle().unwrap();
    let previous_access = first.access_token.clone();
    let previous_refresh = first.refresh_token.clone();
    let held = crate::credentials::prevent_secret_changes().unwrap();
    assert_eq!(token_inner(&base), Err("inProgress"));
    drop(held);
    let access = token_inner(&base).unwrap();
    let saved = read_bundle().unwrap();
    assert_eq!(access, saved.access_token);
    assert_ne!(previous_access, saved.access_token);
    assert_ne!(previous_refresh, saved.refresh_token);
    let retired = crate::redaction_wait::material_with(&crate::credentials::get_secret).unwrap();
    assert!(retired.contains(&previous_access));
    assert!(retired.contains(&previous_refresh.unwrap()));
    let temp = tempfile::tempdir().unwrap();
    crate::gh_owned::configure(temp.path());
    assert!(crate::archive_remote::s110_fake_create_and_backup(
        &access,
        temp.path()
    ));
    worker.join().unwrap();
}

#[test]
fn cancellation_never_writes_the_store() {
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();
    let reply: TokenReply =
        serde_json::from_value(serde_json::json!({"access_token":"fixture"})).unwrap();
    assert_eq!(save(reply, &cancel), Err("cancelled"));
}

#[test]
fn revoked_refresh_requires_reconnection_without_replacing_the_store() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0; 4096];
        let _ = stream.read(&mut buffer).unwrap();
        let body = r#"{"error":"bad_refresh_token"}"#;
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let value = TokenBundle {
        access_token: uuid::Uuid::new_v4().to_string(),
        refresh_token: Some(uuid::Uuid::new_v4().to_string()),
        expires_at: Some(1),
        refresh_expires_at: Some(now().unwrap() + 3600),
    };
    assert_eq!(
        resolve_token(&base, value, |_| panic!(
            "invalid refresh must not replace the store"
        )),
        Err("signed_out")
    );
    worker.join().unwrap();
}

#[test]
fn revoked_access_token_allows_sign_in_again() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let token = uuid::Uuid::new_v4().to_string();
    let expected = token.clone();
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut buffer = [0; 4096];
        let mut request = Vec::new();
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&buffer[..count]);
            assert!(request.len() <= 8192);
        }
        let request = String::from_utf8_lossy(&request);
        assert!(request.starts_with("GET /user HTTP/1.1"));
        assert!(!request.lines().next().unwrap().contains(&expected));
        assert!(request
            .lines()
            .any(|line| line.eq_ignore_ascii_case(&format!("authorization: Bearer {expected}"))));
        stream
            .write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
    });
    assert_eq!(verify_at(&base, &token), Err("signed_out"));
    worker.join().unwrap();
}

#[test]
#[ignore = "contacts GitHub; run only when explicitly requested"]
fn live_device_start_then_discard_without_approval() {
    let grant = begin().unwrap();
    assert!(!grant.device_code.is_empty());
    assert!(!grant.user_code.is_empty());
    drop(grant);
}

#[test]
fn known_tokens_are_supplied_to_archive_redaction() {
    assert!(crate::redaction_wait::protects(ACCESS_REDACTION));
    assert!(crate::redaction_wait::protects(REFRESH_REDACTION));
    let access = format!("fixture-access-{}", uuid::Uuid::new_v4());
    let refresh = format!("fixture-refresh-{}", uuid::Uuid::new_v4());
    let stored = serde_json::to_string(&TokenBundle {
        access_token: access.clone(),
        refresh_token: Some(refresh.clone()),
        expires_at: Some(100),
        refresh_expires_at: Some(200),
    })
    .unwrap();
    let known =
        known_values_with(|name| Ok((name == SECRET_NAME).then(|| stored.clone()))).unwrap();
    let references: Vec<&str> = known.iter().map(String::as_str).collect();
    let copy =
        crate::chat_archive_redact::redact_with_known(&format!("{access} {refresh}"), &references);
    assert_eq!(copy.count, 2);
    assert!(!copy.text.contains(&access));
    assert!(!copy.text.contains(&refresh));
}
