use std::io::{Read, Write};
use std::time::{Duration, Instant};
use tauri_plugin_updater::UpdaterExt;

#[tokio::test]
#[ignore = "run through scripts/test-updater-local.mjs with an ephemeral key"]
async fn local_signed_update_flow() {
    let public_key = std::env::var("MOMSO_TEST_UPDATER_PUBLIC_KEY").unwrap();
    let good = std::env::var("MOMSO_TEST_UPDATER_SIGNATURE").unwrap();
    let bad = std::env::var("MOMSO_TEST_UPDATER_BAD_SIGNATURE").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let start = Instant::now();
        let mut requests = 0;
        let mut flaky = 0;
        while requests < 8 && start.elapsed() < Duration::from_secs(60) {
            let mut stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(e) => panic!("local test listener: {e}"),
            };
            requests += 1;
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 4096];
            let n = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..n]);
            let endpoint = request.split_whitespace().nth(1).unwrap();
            let payload = b"momso local updater payload";
            if endpoint.ends_with(".json") {
                let signature = if endpoint == "/bad.json" { &bad } else { &good };
                let file = if endpoint == "/retry.json" {
                    "flaky"
                } else {
                    "payload"
                };
                let version = if endpoint == "/current.json" {
                    "0.0.0"
                } else {
                    "99.0.0"
                };
                let body = serde_json::json!({"version":version, "platforms": {"windows-x86_64": {
                    "url": format!("http://127.0.0.1:{port}/{file}"), "signature": signature
                }}})
                .to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            } else {
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len()).unwrap();
                if endpoint == "/flaky" && flaky == 0 {
                    flaky += 1;
                    stream.write_all(&payload[..5]).unwrap();
                } else {
                    stream.write_all(payload).unwrap();
                }
            }
        }
        assert_eq!(requests, 8);
    });
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().plugins.0.insert(
        "updater".into(),
        serde_json::json!({
            "pubkey": public_key, "endpoints": [], "dangerousInsecureTransportProtocol": true
        }),
    );
    let app = tauri::test::mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)
        .unwrap();
    for scenario in ["good", "bad", "retry"] {
        let updater = app
            .handle()
            .updater_builder()
            .endpoints(vec![format!("http://127.0.0.1:{port}/{scenario}.json")
                .parse()
                .unwrap()])
            .unwrap()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let update = updater.check().await.unwrap().expect("new version");
        let result = update.download(|_, _| {}, || {}).await;
        match scenario {
            "good" => assert_eq!(result.unwrap(), b"momso local updater payload"),
            "bad" => assert!(
                matches!(result, Err(tauri_plugin_updater::Error::Minisign(_))),
                "incorrect signature must fail verification"
            ),
            _ => {
                assert!(result.is_err(), "interrupted download must fail");
                assert_eq!(
                    update.download(|_, _| {}, || {}).await.unwrap(),
                    b"momso local updater payload"
                );
            }
        }
    }
    let current = app
        .handle()
        .updater_builder()
        .endpoints(vec![format!("http://127.0.0.1:{port}/current.json")
            .parse()
            .unwrap()])
        .unwrap()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    assert!(current.check().await.unwrap().is_none(), "no newer version");
    server.join().unwrap();
    let unused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let unused_port = unused.local_addr().unwrap().port();
    drop(unused);
    let unreachable = app
        .handle()
        .updater_builder()
        .endpoints(vec![format!("http://127.0.0.1:{unused_port}/latest.json")
            .parse()
            .unwrap()])
        .unwrap()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    assert!(
        unreachable.check().await.is_err(),
        "unreachable server must report failure"
    );
}

#[tokio::test]
#[ignore = "release workflow verifies the actual installer and configured public key"]
async fn verify_release_signature() {
    let file = std::env::var("MOMSO_RELEASE_FILE").unwrap();
    let payload = std::fs::read(&file).unwrap();
    let signature = std::fs::read_to_string(format!("{file}.sig")).unwrap();
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    let public_key = config["plugins"]["updater"]["pubkey"].as_str().unwrap();
    let version = config["version"].as_str().unwrap().to_owned();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut request = [0; 4096];
            let n = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..n]);
            let is_manifest = request.split_whitespace().nth(1) == Some("/latest.json");
            let body = if is_manifest {
                serde_json::json!({"version":version, "platforms": {"windows-x86_64": {
                    "url": format!("http://127.0.0.1:{port}/installer"), "signature": signature.trim()
                }}}).to_string().into_bytes()
            } else {
                payload.clone()
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.package_info_mut().version = semver::Version::new(0, 0, 0);
    context.config_mut().plugins.0.insert(
        "updater".into(),
        serde_json::json!({
            "pubkey": public_key, "endpoints": [], "dangerousInsecureTransportProtocol": true
        }),
    );
    let app = tauri::test::mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)
        .unwrap();
    let updater = app
        .handle()
        .updater_builder()
        .endpoints(vec![format!("http://127.0.0.1:{port}/latest.json")
            .parse()
            .unwrap()])
        .unwrap()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    updater
        .check()
        .await
        .unwrap()
        .expect("release artifact")
        .download(|_, _| {}, || {})
        .await
        .expect("signature must match configured public key");
    server.join().unwrap();
}
