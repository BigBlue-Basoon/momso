// A fake server exercises the proposed contract only; no product integration.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Preview {
    account: String,
    caption: String,
    media_sha256: String,
    visibility: String,
    scheduled_at: Option<String>,
    external_disclosure: bool,
}
fn digest(preview: &Preview) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(preview).unwrap()))
}
#[derive(Serialize, Deserialize)]
struct Request {
    preview: Preview,
    approved_digest: Option<String>,
}

fn fake_server(request: Request) -> bool {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut input = String::new();
        stream.read_to_string(&mut input).unwrap();
        let r: Request = serde_json::from_str(&input).unwrap();
        let accepted = r.approved_digest.as_deref() == Some(digest(&r.preview).as_str());
        stream
            .write_all(if accepted { b"accepted" } else { b"refused" })
            .unwrap();
    });
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .write_all(serde_json::to_string(&request).unwrap().as_bytes())
        .unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    server.join().unwrap();
    response == "accepted"
}

#[test]
fn fake_instagram_requires_approval_for_exact_preview() {
    let preview = Preview {
        account: "account-a".into(),
        caption: "draft".into(),
        media_sha256: "fake-file-hash".into(),
        visibility: "public".into(),
        scheduled_at: None,
        external_disclosure: false,
    };
    assert!(!fake_server(Request {
        preview: preview.clone(),
        approved_digest: None
    }));
    let approved = digest(&preview);
    assert!(fake_server(Request {
        preview: preview.clone(),
        approved_digest: Some(approved.clone())
    }));
    let mut changed = preview;
    changed.account = "account-b".into();
    assert!(!fake_server(Request {
        preview: changed,
        approved_digest: Some(approved)
    }));
}
