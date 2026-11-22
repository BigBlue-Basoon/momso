//! Persist only keyed digests of verification codes; the code itself is never stored.
use super::*;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;
type HmacSha256 = Hmac<Sha256>;

fn key() -> Result<Zeroizing<Vec<u8>>, &'static str> {
    let root = std::env::var_os("CREDENTIALS_DIRECTORY").ok_or("credential_unavailable")?;
    let data = std::fs::read(PathBuf::from(root).join("momso-vault-identity"))
        .map_err(|_| "credential_unavailable")?;
    if data.is_empty() {
        return Err("credential_unavailable");
    }
    Ok(Zeroizing::new(data))
}
fn digest(key: &[u8], code: &str) -> Result<Vec<u8>, &'static str> {
    let mut mac = HmacSha256::new_from_slice(key).map_err(|_| "storage_unavailable")?;
    mac.update(code.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}
fn verify_key(conn: &Connection, key: &[u8]) -> Result<(), &'static str> {
    let fingerprint = Sha256::digest(key).to_vec();
    let stored: Option<Vec<u8>> = conn
        .query_row(
            "SELECT fingerprint FROM otp_redaction_meta WHERE id=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| "storage_unavailable")?;
    match stored {
        Some(value) if value != fingerprint => {
            let count: i64 = conn
                .query_row("SELECT count(*) FROM otp_redactions", [], |r| r.get(0))
                .map_err(|_| "storage_unavailable")?;
            if count > 0 {
                return Err("redaction_key_changed");
            }
            conn.execute(
                "UPDATE otp_redaction_meta SET fingerprint=?1 WHERE id=1",
                [fingerprint],
            )
            .map_err(|_| "storage_unavailable")?;
            Ok(())
        }
        Some(_) => Ok(()),
        None => {
            conn.execute(
                "INSERT INTO otp_redaction_meta(id,fingerprint) VALUES(1,?1)",
                [fingerprint],
            )
            .map_err(|_| "storage_unavailable")?;
            Ok(())
        }
    }
}
pub fn record(app: &App, code: &str) -> Result<(), &'static str> {
    let key = key()?;
    let conn = db(app).map_err(|_| "storage_unavailable")?;
    verify_key(&conn, &key)?;
    conn.execute(
        "INSERT OR IGNORE INTO otp_redactions(digest,length) VALUES (?1,?2)",
        params![digest(&key, code)?, code.len() as i64],
    )
    .map_err(|_| "storage_unavailable")?;
    Ok(())
}
pub fn redact(app: &App, value: &mut Value) -> Result<(), &'static str> {
    let key = key()?;
    let conn = db(app).map_err(|_| "storage_unavailable")?;
    verify_key(&conn, &key)?;
    let mut stmt = conn
        .prepare("SELECT digest,length FROM otp_redactions")
        .map_err(|_| "storage_unavailable")?;
    let pairs = stmt
        .query_map([], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, u32>(1)?)))
        .map_err(|_| "storage_unavailable")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "storage_unavailable")?;
    if pairs.is_empty() {
        return Ok(());
    }
    let mut groups: std::collections::BTreeMap<
        std::cmp::Reverse<u32>,
        std::collections::HashSet<Vec<u8>>,
    > = std::collections::BTreeMap::new();
    for (hash, length) in pairs {
        groups
            .entry(std::cmp::Reverse(length))
            .or_default()
            .insert(hash);
    }
    fn walk(
        value: &mut Value,
        key: &[u8],
        groups: &std::collections::BTreeMap<
            std::cmp::Reverse<u32>,
            std::collections::HashSet<Vec<u8>>,
        >,
    ) -> Result<(), &'static str> {
        match value {
            Value::String(text) => {
                let original = std::mem::take(text);
                let mut result = String::new();
                let mut cursor = 0;
                while cursor < original.len() {
                    let mut matched = 0;
                    for (std::cmp::Reverse(len), hashes) in groups {
                        if let Some(part) =
                            original.get(cursor..cursor.saturating_add(*len as usize))
                        {
                            if part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                                && hashes.contains(&digest(key, part)?)
                            {
                                matched = *len as usize;
                                break;
                            }
                        }
                    }
                    if matched > 0 {
                        result.push_str("[redacted]");
                        cursor += matched;
                    } else if let Some(ch) = original[cursor..].chars().next() {
                        result.push(ch);
                        cursor += ch.len_utf8();
                    } else {
                        break;
                    }
                }
                *text = result;
            }
            Value::Array(values) => {
                for value in values {
                    walk(value, key, groups)?;
                }
            }
            Value::Object(values) => {
                for (name, value) in values {
                    if !matches!(name.as_str(), "status" | "reason" | "data_provenance") {
                        walk(value, key, groups)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    walk(value, &key, &groups)
}
