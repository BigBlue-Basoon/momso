use super::{request, Config, Keys, WriteSafety};
use serde::Deserialize;

// AWS: https://docs.aws.amazon.com/AmazonS3/latest/userguide/bucketnamingrules.html
// GCP: https://docs.cloud.google.com/storage/docs/buckets
// R2: https://developers.cloudflare.com/r2/buckets/create-buckets/
// NCP: https://api.ncloud-docs.com/docs/en/storage-objectstorage-putbucket
pub(super) fn valid_name(provider: &str, name: &str) -> bool {
    let valid_edge = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    let bytes = name.as_bytes();
    if bytes.len() < 3 || !valid_edge(bytes[0]) || !valid_edge(bytes[bytes.len() - 1]) {
        return false;
    }
    if provider == "gcp" {
        let limit = if name.contains('.') { 222 } else { 63 };
        return bytes.len() <= limit
            && bytes
                .iter()
                .all(|b| valid_edge(*b) || matches!(b, b'-' | b'_' | b'.'))
            && name
                .split('.')
                .all(|part| !part.is_empty() && part.len() <= 63)
            && name.parse::<std::net::Ipv4Addr>().is_err()
            && !name.starts_with("goog")
            && !name.contains("google")
            && !name.contains("g00gle");
    }
    if bytes.len() > 63
        || !bytes
            .iter()
            .all(|b| valid_edge(*b) || *b == b'-' || (*b == b'.' && provider != "cloudflare"))
    {
        return false;
    }
    if name.parse::<std::net::Ipv4Addr>().is_ok() || name.contains("..") {
        return false;
    }
    if provider == "aws" {
        return !["xn--", "sthree-", "amzn-s3-demo-"]
            .iter()
            .any(|p| name.starts_with(p))
            && ![
                "-s3alias",
                "--ol-s3",
                ".mrap",
                "--x-s3",
                "--table-s3",
                "-an",
            ]
            .iter()
            .any(|s| name.ends_with(s));
    }
    true
}

pub(crate) fn automatic_name(
    provider: &str,
    username: Option<&str>,
) -> Result<String, &'static str> {
    if !matches!(provider, "aws" | "gcp" | "cloudflare" | "ncp") {
        return Err("automatic_provider_unsupported");
    }
    let username = username
        .filter(|v| !v.is_empty())
        .ok_or("username_required")?;
    let name = format!("momso-{username}");
    if !valid_name(provider, &name) {
        return Err("invalid_username_for_bucket");
    }
    Ok(name)
}

#[derive(Deserialize)]
struct BucketList {
    #[serde(rename = "Buckets")]
    buckets: BucketItems,
    #[serde(rename = "ContinuationToken", default)]
    continuation: Option<String>,
    #[serde(rename = "IsTruncated", default)]
    truncated: Option<bool>,
}
#[derive(Deserialize)]
struct BucketItems {
    #[serde(rename = "Bucket", default)]
    buckets: Vec<BucketItem>,
}
#[derive(Deserialize)]
struct BucketItem {
    #[serde(rename = "Name")]
    name: String,
}

async fn owned_s3(config: &Config, keys: &Keys) -> Result<bool, &'static str> {
    if config.provider == "cloudflare" {
        // R2 bucket names are scoped to the account in the endpoint. Inspect
        // this exact bucket without requesting the account's bucket listing.
        let response = request(config, keys, reqwest::Method::HEAD, "", &[], vec![]).await?;
        return match response.status() {
            reqwest::StatusCode::OK => Ok(true),
            reqwest::StatusCode::NOT_FOUND => Ok(false),
            reqwest::StatusCode::UNAUTHORIZED => Err("invalid_credentials"),
            reqwest::StatusCode::FORBIDDEN => Err("access_denied"),
            _ => Err("bucket_lookup_failed"),
        };
    }
    let mut service = config.clone();
    service.bucket.clear();
    let mut token: Option<String> = None;
    loop {
        let mut query = Vec::new();
        if config.provider == "aws" {
            query.push(("max-buckets", "1000"));
            if let Some(value) = token.as_deref() {
                query.push(("continuation-token", value));
            }
        }
        let response = request(&service, keys, reqwest::Method::GET, "", &query, vec![]).await?;
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return Err("bucket_list_denied");
        }
        if !response.status().is_success() {
            return Err("bucket_list_failed");
        }
        let xml = response.text().await.map_err(|_| "bucket_list_failed")?;
        let list: BucketList = quick_xml::de::from_str(&xml).map_err(|_| "bucket_list_failed")?;
        if list.buckets.buckets.iter().any(|b| b.name == config.bucket) {
            return Ok(true);
        }
        let next = list.continuation;
        match next.filter(|s| !s.is_empty()) {
            Some(next) if config.provider == "aws" && token.as_deref() != Some(&next) => {
                token = Some(next)
            }
            Some(_) => return Err("bucket_list_failed"),
            None if list.truncated == Some(true) => return Err("bucket_list_failed"),
            None => return Ok(false),
        }
    }
}

async fn ensure_s3(config: &Config, keys: &Keys) -> Result<(), &'static str> {
    ensure_s3_with_policy(config, keys, false).await
}

async fn ensure_s3_with_policy(
    config: &Config,
    keys: &Keys,
    create_only: bool,
) -> Result<(), &'static str> {
    // AWS may return 200 and reset an existing bucket's ACL in this region.
    // https://docs.aws.amazon.com/AmazonS3/latest/developerguide/ErrorResponses.html
    if create_only && config.provider == "aws" && config.region == "us-east-1" {
        return Err("unsafe_existing_bucket_semantics");
    }
    if owned_s3(config, keys).await? {
        return if create_only {
            Err("bucket_exists")
        } else {
            Ok(())
        };
    }
    let mut create = config.clone();
    create.write_safety = Some(WriteSafety::CheckThenWrite);
    let body = if config.provider == "aws" && config.region != "us-east-1" {
        format!("<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><LocationConstraint>{}</LocationConstraint></CreateBucketConfiguration>", config.region).into_bytes()
    } else {
        Vec::new()
    };
    let response = request(&create, keys, reqwest::Method::PUT, "", &[], body).await?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if body.contains("InvalidAccessKeyId")
        || body.contains("SignatureDoesNotMatch")
        || status == reqwest::StatusCode::UNAUTHORIZED
    {
        return Err("invalid_credentials");
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err("bucket_create_denied");
    }
    if status == reqwest::StatusCode::CONFLICT
        || body.contains("BucketAlreadyExists")
        || body.contains("BucketAlreadyOwnedByYou")
    {
        if create_only {
            return Err("bucket_name_taken");
        }
        return if owned_s3(config, keys).await? {
            Ok(())
        } else {
            Err("bucket_name_taken")
        };
    }
    if body.contains("InvalidBucketName") {
        return Err("invalid_username_for_bucket");
    }
    Err("bucket_create_failed")
}

// HMAC credentials work only with the XML API. Public access prevention is
// configured atomically at creation using the JSON API and an OAuth token.
// https://docs.cloud.google.com/storage/docs/authentication/hmackeys
// https://docs.cloud.google.com/storage/docs/json_api/v1/buckets/insert
// https://docs.cloud.google.com/storage/docs/using-public-access-prevention
async fn ensure_gcp_at(
    config: &Config,
    project: &str,
    token: &str,
    base: &str,
) -> Result<(), &'static str> {
    ensure_gcp_at_with_policy(config, project, token, base, false).await
}

async fn ensure_gcp_at_with_policy(
    config: &Config,
    project: &str,
    token: &str,
    base: &str,
    create_only: bool,
) -> Result<(), &'static str> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|_| "unavailable")?;
    let url =
        reqwest::Url::parse(&format!("{base}/storage/v1/b")).map_err(|_| "bucket_list_failed")?;
    let mut page: Option<String> = None;
    loop {
        let mut query = vec![("project", project), ("prefix", config.bucket.as_str())];
        if let Some(value) = page.as_deref() {
            query.push(("pageToken", value));
        }
        let mut list_url = url.clone();
        list_url.query_pairs_mut().extend_pairs(query);
        let response = client
            .get(list_url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| "bucket_list_failed")?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err("gcp_oauth_invalid");
        }
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return Err("bucket_list_denied");
        }
        if !response.status().is_success() {
            return Err("bucket_list_failed");
        }
        let data: serde_json::Value = response.json().await.map_err(|_| "bucket_list_failed")?;
        if data
            .get("items")
            .and_then(|v| v.as_array())
            .is_some_and(|items| {
                items
                    .iter()
                    .any(|v| v.get("name").and_then(|v| v.as_str()) == Some(config.bucket.as_str()))
            })
        {
            return if create_only {
                Err("bucket_exists")
            } else {
                Ok(())
            };
        }
        match data
            .get("nextPageToken")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            Some(next) if page.as_deref() != Some(next) => page = Some(next.to_owned()),
            Some(_) => return Err("bucket_list_failed"),
            None => break,
        }
    }
    let mut body = serde_json::json!({ "name": config.bucket, "iamConfiguration": { "publicAccessPrevention": "enforced", "uniformBucketLevelAccess": { "enabled": true } } });
    if config.region != "auto" {
        body["location"] = serde_json::Value::String(config.region.clone());
    }
    let mut create_url = url;
    create_url.query_pairs_mut().append_pair("project", project);
    let response = client
        .post(create_url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|_| "bucket_create_failed")?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("gcp_oauth_invalid");
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err("bucket_create_denied");
    }
    if status == reqwest::StatusCode::CONFLICT {
        return Err("bucket_name_taken");
    }
    if status == reqwest::StatusCode::BAD_REQUEST {
        return Err("bucket_create_invalid");
    }
    if !status.is_success() {
        return Err("bucket_create_failed");
    }
    let result: serde_json::Value = response
        .json()
        .await
        .map_err(|_| "bucket_privacy_unverified")?;
    if result.get("name").and_then(|v| v.as_str()) != Some(config.bucket.as_str())
        || result
            .pointer("/iamConfiguration/publicAccessPrevention")
            .and_then(|v| v.as_str())
            != Some("enforced")
    {
        return Err("bucket_privacy_unverified");
    }
    Ok(())
}

pub(super) async fn ensure(
    config: &Config,
    keys: &Keys,
    project: Option<&str>,
    token: Option<&str>,
) -> Result<(), &'static str> {
    if config.provider == "gcp" {
        let project = project
            .filter(|v| !v.is_empty())
            .ok_or("gcp_project_required")?;
        let token = token
            .filter(|v| !v.is_empty())
            .ok_or("gcp_oauth_required")?;
        ensure_gcp_at(config, project, token, "https://storage.googleapis.com").await
    } else {
        ensure_s3(config, keys).await
    }
}

// Reserved for the approved skill path; existing buckets must never be reused.
#[allow(dead_code)]
pub(super) async fn ensure_new(
    config: &Config,
    keys: &Keys,
    project: Option<&str>,
    token: Option<&str>,
) -> Result<(), &'static str> {
    match config.provider.as_str() {
        "gcp" => {
            ensure_gcp_at_with_policy(
                config,
                project
                    .filter(|v| !v.is_empty())
                    .ok_or("gcp_project_required")?,
                token
                    .filter(|v| !v.is_empty())
                    .ok_or("gcp_oauth_required")?,
                "https://storage.googleapis.com",
                true,
            )
            .await
        }
        "aws" | "cloudflare" => ensure_s3_with_policy(config, keys, true).await,
        // Existing-bucket semantics must be confirmed before an NCP write.
        _ => Err("unavailable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const EMPTY: &str = "<ListAllMyBucketsResult><Buckets/></ListAllMyBucketsResult>";
    const OWNED: &str = "<ListAllMyBucketsResult><Buckets><Bucket><Name>momso-alice</Name></Bucket></Buckets></ListAllMyBucketsResult>";

    fn server(
        replies: Vec<(&'static str, &'static str)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, response) in replies {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                    .unwrap();
                let mut data = Vec::new();
                let mut buf = [0u8; 4096];
                let head_end = loop {
                    let n = stream.read(&mut buf).unwrap();
                    assert_ne!(n, 0);
                    data.extend_from_slice(&buf[..n]);
                    if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        break pos + 4;
                    }
                };
                let head = String::from_utf8_lossy(&data[..head_end]);
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while data.len() - head_end < length {
                    let n = stream.read(&mut buf).unwrap();
                    assert_ne!(n, 0);
                    data.extend_from_slice(&buf[..n]);
                }
                requests.push(String::from_utf8(data).unwrap());
                write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
            }
            requests
        });
        (endpoint, handle)
    }

    fn config(provider: &str, endpoint: String) -> Config {
        Config {
            provider: provider.into(),
            endpoint,
            region: if provider == "aws" {
                "ap-northeast-2"
            } else {
                "auto"
            }
            .into(),
            bucket: "momso-alice".into(),
            credential_id: "test".into(),
            last_backup_at: None,
            write_safety: None,
        }
    }
    fn keys() -> Keys {
        Keys {
            access_key_id: "TEST".into(),
            secret_access_key: "test".into(),
        }
    }

    #[tokio::test]
    async fn approved_create_only_never_reuses_existing_bucket() {
        for provider in ["aws", "cloudflare"] {
            let reply = if provider == "aws" {
                ("200 OK", OWNED)
            } else {
                ("200 OK", "")
            };
            let (endpoint, thread) = server(vec![reply]);
            assert_eq!(
                ensure_s3_with_policy(&config(provider, endpoint), &keys(), true).await,
                Err("bucket_exists")
            );
            assert_eq!(thread.join().unwrap().len(), 1);
        }
        let (endpoint, thread) = server(vec![
            ("200 OK", EMPTY),
            ("409 Conflict", "BucketAlreadyOwnedByYou"),
        ]);
        assert_eq!(
            ensure_s3_with_policy(&config("aws", endpoint), &keys(), true).await,
            Err("bucket_name_taken")
        );
        assert_eq!(thread.join().unwrap().len(), 2);
        let mut aws = config("aws", "http://127.0.0.1:1".into());
        aws.region = "us-east-1".into();
        assert_eq!(
            ensure_s3_with_policy(&aws, &keys(), true).await,
            Err("unsafe_existing_bucket_semantics")
        );
        let (endpoint, thread) = server(vec![("200 OK", r#"{"items":[{"name":"momso-alice"}]}"#)]);
        assert_eq!(
            ensure_gcp_at_with_policy(
                &config("gcp", endpoint.clone()),
                "project",
                "token",
                &endpoint,
                true
            )
            .await,
            Err("bucket_exists")
        );
        assert_eq!(thread.join().unwrap().len(), 1);
    }

    #[test]
    fn provider_name_rules_are_checked_without_changing_username() {
        for provider in ["aws", "gcp", "cloudflare", "ncp"] {
            assert_eq!(
                automatic_name(provider, Some("alice")),
                Ok("momso-alice".into())
            );
            assert_eq!(
                automatic_name(provider, Some("Alice")),
                Err("invalid_username_for_bucket")
            );
            assert_eq!(
                automatic_name(provider, Some("alice-")),
                Err("invalid_username_for_bucket")
            );
            assert_eq!(automatic_name(provider, None), Err("username_required"));
        }
        assert!(automatic_name("gcp", Some("a_b")).is_ok());
        assert!(automatic_name("aws", Some("a_b")).is_err());
        assert!(automatic_name("cloudflare", Some("a.b")).is_err());
        assert!(automatic_name("ncp", Some("a.b")).is_ok());
        assert!(automatic_name("gcp", Some("google")).is_err());
        assert!(automatic_name("aws", Some(&"x".repeat(58))).is_err());
        assert_eq!(
            automatic_name("s3_compatible", Some("alice")),
            Err("automatic_provider_unsupported")
        );
    }

    #[tokio::test]
    async fn s3_providers_create_private_default_and_keep_owned_buckets() {
        for provider in ["aws", "cloudflare", "ncp"] {
            let first = if provider == "cloudflare" {
                ("404 Not Found", "")
            } else {
                ("200 OK", EMPTY)
            };
            let (endpoint, thread) = server(vec![first, ("200 OK", "")]);
            let c = config(provider, endpoint);
            assert_eq!(ensure_s3(&c, &keys()).await, Ok(()));
            let requests = thread.join().unwrap();
            assert!(if provider == "cloudflare" {
                requests[0].starts_with("HEAD /momso-alice ")
            } else {
                requests[0].starts_with("GET /")
            });
            assert!(requests[1].starts_with("PUT /momso-alice "));
            assert!(!requests[1]
                .to_ascii_lowercase()
                .contains("x-amz-acl: public"));
            if provider == "aws" {
                assert!(
                    requests[1].contains("<LocationConstraint>ap-northeast-2</LocationConstraint>")
                );
            }
            let first = if provider == "cloudflare" {
                ("200 OK", "")
            } else {
                ("200 OK", OWNED)
            };
            let (endpoint, thread) = server(vec![first]);
            assert_eq!(
                ensure_s3(&config(provider, endpoint), &keys()).await,
                Ok(())
            );
            assert_eq!(thread.join().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn s3_collision_and_denied_creation_are_distinct() {
        for provider in ["aws", "cloudflare", "ncp"] {
            let first = if provider == "cloudflare" {
                ("404 Not Found", "")
            } else {
                ("200 OK", EMPTY)
            };
            let (endpoint, thread) = server(vec![
                first,
                (
                    "409 Conflict",
                    "<Error><Code>BucketAlreadyExists</Code></Error>",
                ),
                first,
            ]);
            assert_eq!(
                ensure_s3(&config(provider, endpoint), &keys()).await,
                Err("bucket_name_taken")
            );
            thread.join().unwrap();
            let (endpoint, thread) = server(vec![
                first,
                ("403 Forbidden", "<Error><Code>AccessDenied</Code></Error>"),
            ]);
            assert_eq!(
                ensure_s3(&config(provider, endpoint), &keys()).await,
                Err("bucket_create_denied")
            );
            thread.join().unwrap();
        }
    }

    #[tokio::test]
    async fn bucket_listing_follows_provider_continuation_tokens() {
        for (provider, page) in [
            ("aws", "<ListAllMyBucketsResult><Buckets/><ContinuationToken>next</ContinuationToken></ListAllMyBucketsResult>"),
        ] {
            let (endpoint, thread) = server(vec![("200 OK", page), ("200 OK", OWNED)]);
            assert_eq!(ensure_s3(&config(provider, endpoint), &keys()).await, Ok(()));
            let requests = thread.join().unwrap();
            assert_eq!(requests.len(), 2);
            assert!(requests[1].contains("continuation-token=next"));
        }
    }

    #[tokio::test]
    async fn google_creation_enforces_public_access_prevention() {
        let created =
            r#"{"name":"momso-alice","iamConfiguration":{"publicAccessPrevention":"enforced"}}"#;
        let (endpoint, thread) = server(vec![("200 OK", "{}"), ("200 OK", created)]);
        let c = config("gcp", endpoint.clone());
        assert_eq!(
            ensure_gcp_at(&c, "my-project", "test-token", &endpoint).await,
            Ok(())
        );
        let requests = thread.join().unwrap();
        assert!(requests[0].starts_with("GET /storage/v1/b?"));
        assert!(requests[1].starts_with("POST /storage/v1/b?"));
        let body = requests[1].split_once("\r\n\r\n").unwrap().1;
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            body.pointer("/iamConfiguration/publicAccessPrevention")
                .and_then(|v| v.as_str()),
            Some("enforced")
        );
        assert_eq!(
            body.pointer("/iamConfiguration/uniformBucketLevelAccess/enabled")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[tokio::test]
    async fn google_owned_collision_denied_and_missing_credentials() {
        let (endpoint, thread) = server(vec![("200 OK", r#"{"items":[{"name":"momso-alice"}]}"#)]);
        assert_eq!(
            ensure_gcp_at(&config("gcp", endpoint.clone()), "p", "t", &endpoint).await,
            Ok(())
        );
        thread.join().unwrap();
        for (status, expected) in [
            ("409 Conflict", "bucket_name_taken"),
            ("403 Forbidden", "bucket_create_denied"),
        ] {
            let (endpoint, thread) = server(vec![("200 OK", "{}"), (status, "{}")]);
            assert_eq!(
                ensure_gcp_at(&config("gcp", endpoint.clone()), "p", "t", &endpoint).await,
                Err(expected)
            );
            thread.join().unwrap();
        }
        let c = config("gcp", "https://storage.googleapis.com".into());
        assert_eq!(
            ensure(&c, &keys(), None, None).await,
            Err("gcp_project_required")
        );
        assert_eq!(
            ensure(&c, &keys(), Some("p"), None).await,
            Err("gcp_oauth_required")
        );
    }

    #[tokio::test]
    async fn ncp_create_then_probe_ignored_condition_uses_check_then_write() {
        let (endpoint, thread) = server(vec![
            ("200 OK", EMPTY),
            ("200 OK", ""),
            ("200 OK", ""),
            ("200 OK", ""),
            ("200 OK", ""),
            ("200 OK", "second"),
        ]);
        let mut c = config("ncp", endpoint);
        c.region = "kr-standard".into();
        ensure_s3(&c, &keys()).await.unwrap();
        assert_eq!(
            super::super::probe_connection(&c, &keys()).await,
            Ok(WriteSafety::CheckThenWrite)
        );
        let requests = thread.join().unwrap();
        assert!(requests[1].starts_with("PUT /momso-alice "));
        assert!(requests[1].ends_with("\r\n\r\n"));
        assert!(requests[1].contains("/kr-standard/s3/aws4_request"));
        assert!(requests[2].starts_with("GET /momso-alice?max-keys=1 "));
        assert!(!requests[2].contains("list-type="));
        assert!(requests[4]
            .to_ascii_lowercase()
            .contains("if-none-match: *"));
    }

    #[tokio::test]
    async fn created_bucket_is_probed_before_connection() {
        for provider in ["aws", "cloudflare", "ncp", "gcp"] {
            let gcp = provider == "gcp";
            let list = if gcp { "{}" } else { EMPTY };
            let created = if gcp {
                r#"{"name":"momso-alice","iamConfiguration":{"publicAccessPrevention":"enforced"}}"#
            } else {
                ""
            };
            let (endpoint, thread) = server(vec![
                if provider == "cloudflare" {
                    ("404 Not Found", "")
                } else {
                    ("200 OK", list)
                },
                ("200 OK", created),
                ("200 OK", ""),
                ("200 OK", ""),
                ("412 Precondition Failed", ""),
                ("200 OK", "first"),
            ]);
            let c = config(provider, endpoint.clone());
            if gcp {
                ensure_gcp_at(&c, "project", "token", &endpoint)
                    .await
                    .unwrap();
            } else {
                ensure_s3(&c, &keys()).await.unwrap();
            }
            assert_eq!(
                super::super::probe_connection(&c, &keys()).await,
                Ok(WriteSafety::Conditional)
            );
            let requests = thread.join().unwrap();
            assert!(requests[2].starts_with("GET /momso-alice?"));
            assert!(requests[3].starts_with("PUT /momso-alice/"));
            assert!(requests[4].starts_with("PUT /momso-alice/"));
        }
    }

    #[tokio::test]
    async fn google_refuses_unverified_privacy_and_bucket_listing_denial() {
        let (endpoint, thread) = server(vec![
            ("200 OK", "{}"),
            ("200 OK", r#"{"name":"momso-alice"}"#),
        ]);
        assert_eq!(
            ensure_gcp_at(&config("gcp", endpoint.clone()), "p", "t", &endpoint).await,
            Err("bucket_privacy_unverified")
        );
        thread.join().unwrap();
        let (endpoint, thread) = server(vec![(
            "403 Forbidden",
            "<Error><Code>AccessDenied</Code></Error>",
        )]);
        assert_eq!(
            ensure_s3(&config("aws", endpoint), &keys()).await,
            Err("bucket_list_denied")
        );
        thread.join().unwrap();
    }

    #[tokio::test]
    async fn r2_basoon_never_lists_or_accesses_other_buckets() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let mut buckets = std::collections::HashSet::from([
                "someone-elses-private-bucket".to_string(),
                "someone-elses-archive-bucket".to_string(),
            ]);
            let mut probe: Option<Vec<u8>> = None;
            let mut requests = Vec::new();
            for _ in 0..6 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                    .unwrap();
                let mut data = Vec::new();
                let mut buf = [0; 4096];
                let end = loop {
                    let n = stream.read(&mut buf).unwrap();
                    assert_ne!(n, 0);
                    data.extend_from_slice(&buf[..n]);
                    if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                };
                let header = String::from_utf8(data[..end].to_vec()).unwrap();
                let line = header.lines().next().unwrap().to_string();
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap();
                let path = parts.next().unwrap();
                let length = header
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while data.len() - end < length {
                    let n = stream.read(&mut buf).unwrap();
                    assert_ne!(n, 0);
                    data.extend_from_slice(&buf[..n]);
                }
                let bucket = path
                    .trim_start_matches('/')
                    .split(['/', '?'])
                    .next()
                    .unwrap();
                let (status, body): (&str, Vec<u8>) = if bucket != "momso-basoon" {
                    ("403 Forbidden", Vec::new())
                } else if method == "HEAD" {
                    (
                        if buckets.contains(bucket) {
                            "200 OK"
                        } else {
                            "404 Not Found"
                        },
                        Vec::new(),
                    )
                } else if method == "PUT" && path == "/momso-basoon" {
                    buckets.insert(bucket.to_string());
                    ("200 OK", Vec::new())
                } else if method == "GET" && path.starts_with("/momso-basoon?") {
                    ("200 OK", Vec::new())
                } else if method == "PUT"
                    && path.starts_with("/momso-basoon/momso/object-store/v1/probes/")
                {
                    if probe.is_some() && header.to_ascii_lowercase().contains("if-none-match: *") {
                        ("412 Precondition Failed", Vec::new())
                    } else {
                        probe = Some(data[end..end + length].to_vec());
                        ("200 OK", Vec::new())
                    }
                } else if method == "GET"
                    && path.starts_with("/momso-basoon/momso/object-store/v1/probes/")
                {
                    ("200 OK", probe.clone().unwrap())
                } else {
                    ("403 Forbidden", Vec::new())
                };
                requests.push(line.clone());
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                if method != "HEAD" {
                    stream.write_all(&body).unwrap();
                }
            }
            (buckets, requests)
        });
        let mut c = config("cloudflare", endpoint);
        c.bucket = automatic_name("cloudflare", Some("basoon")).unwrap();
        assert_eq!(ensure_s3(&c, &keys()).await, Ok(()));
        assert_eq!(
            super::super::probe_connection(&c, &keys()).await,
            Ok(WriteSafety::Conditional)
        );
        let (buckets, requests) = thread.join().unwrap();
        assert!(buckets.contains("someone-elses-private-bucket"));
        assert!(buckets.contains("someone-elses-archive-bucket"));
        assert!(buckets.contains("momso-basoon"));
        assert_eq!(
            requests
                .iter()
                .filter(|line| line
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .trim_start_matches('/')
                    .split(['/', '?'])
                    .next()
                    != Some("momso-basoon"))
                .count(),
            0
        );
        assert!(!requests
            .iter()
            .any(|line| line.contains("someone-elses-private-bucket")
                || line.contains("someone-elses-archive-bucket")));
    }

    #[tokio::test]
    async fn r2_denied_lookup_never_attempts_bucket_creation_or_listing() {
        let (endpoint, thread) = server(vec![("403 Forbidden", "")]);
        let mut c = config("cloudflare", endpoint);
        c.bucket = automatic_name("cloudflare", Some("basoon")).unwrap();
        assert_eq!(ensure_s3(&c, &keys()).await, Err("access_denied"));
        let requests = thread.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("HEAD /momso-basoon "));
        c.bucket.clear();
        assert_eq!(
            request(&c, &keys(), reqwest::Method::GET, "", &[], vec![])
                .await
                .err(),
            Some("invalid_bucket")
        );
    }
}
