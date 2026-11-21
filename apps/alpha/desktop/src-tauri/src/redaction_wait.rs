//! The waiting queue, including its values, exists only in the OS credential store.
use crate::credentials::{self, CredentialError};
use serde::{Deserialize, Serialize};

const QUEUE: &str = "redaction-wait:queue-v1";
const MAX_AGE_SECONDS: i64 = 30 * 24 * 60 * 60;

#[derive(Deserialize, Serialize)]
struct Entry {
    kind: String,
    created_at: i64,
    value: String,
}

#[derive(Default, Deserialize, Serialize)]
struct Queue {
    entries: Vec<Entry>,
    expired_count: u64,
    #[serde(default)]
    watched_names: Vec<String>,
}

fn load_with(
    get: &impl Fn(&str) -> Result<Option<String>, CredentialError>,
) -> Result<Queue, CredentialError> {
    match get(QUEUE)? {
        None => Ok(Queue::default()),
        Some(value) => serde_json::from_str(&value).map_err(|_| CredentialError::ReadFailed),
    }
}

pub(crate) fn protects(name: &str) -> bool {
    name == crate::saved_logins::VAULT
        || (name.starts_with("ai:") && name.ends_with(":api-key"))
        || name.starts_with("object-store:")
        || (name.starts_with("backup:") && name.ends_with(":age-identity"))
}

fn kind(name: &str) -> &'static str {
    if name == crate::saved_logins::VAULT {
        "savedLogins"
    } else if name.starts_with("object-store:") {
        "objectStore"
    } else if name.starts_with("backup:") {
        "backup"
    } else {
        "ai"
    }
}

fn enqueue_with(
    active_name: &str,
    value: &str,
    now: i64,
    get: &impl Fn(&str) -> Result<Option<String>, CredentialError>,
    put: &mut impl FnMut(&str, &str) -> Result<(), CredentialError>,
) -> Result<(), CredentialError> {
    let mut queue = load_with(get)?;
    queue.entries.push(Entry {
        kind: kind(active_name).into(),
        created_at: now,
        value: value.into(),
    });
    let encoded = serde_json::to_string(&queue).map_err(|_| CredentialError::WriteFailed)?;
    // A single credential-store write preserves all prior values atomically.
    put(QUEUE, &encoded)
}

pub(crate) fn enqueue_unlocked(active_name: &str, value: &str) -> Result<(), CredentialError> {
    enqueue_with(
        active_name,
        value,
        chrono::Utc::now().timestamp(),
        &credentials::get_secret,
        &mut credentials::put_secret_unlocked,
    )
}

fn watch_new_with(
    name: &str,
    get: &impl Fn(&str) -> Result<Option<String>, CredentialError>,
    put: &mut impl FnMut(&str, &str) -> Result<(), CredentialError>,
) -> Result<(), CredentialError> {
    let mut queue = load_with(get)?;
    if !queue.watched_names.iter().any(|saved| saved == name) {
        queue.watched_names.push(name.to_owned());
        put(
            QUEUE,
            &serde_json::to_string(&queue).map_err(|_| CredentialError::WriteFailed)?,
        )?;
    }
    Ok(())
}

pub(crate) fn watch_new_unlocked(name: &str) -> Result<(), CredentialError> {
    watch_new_with(
        name,
        &credentials::get_secret,
        &mut credentials::put_secret_unlocked,
    )
}

fn watched_values_with(
    get: &impl Fn(&str) -> Result<Option<String>, CredentialError>,
) -> Result<Vec<String>, CredentialError> {
    let queue = load_with(get)?;
    let mut values = Vec::new();
    for name in queue.watched_names {
        if let Some(value) = get(&name)? {
            if name == crate::saved_logins::VAULT {
                values.extend(
                    crate::saved_logins::passwords_from_serialized(&value)
                        .map_err(|_| CredentialError::ReadFailed)?,
                );
            } else if name.starts_with("object-store:") {
                let object: serde_json::Value =
                    serde_json::from_str(&value).map_err(|_| CredentialError::ReadFailed)?;
                for field in ["accessKeyId", "secretAccessKey"] {
                    values.push(
                        object
                            .get(field)
                            .and_then(serde_json::Value::as_str)
                            .ok_or(CredentialError::ReadFailed)?
                            .to_owned(),
                    );
                }
            } else {
                values.push(value);
            }
        }
    }
    Ok(values)
}

#[cfg(not(test))]
pub(crate) fn watched_values_unlocked() -> Result<Vec<String>, CredentialError> {
    watched_values_with(&credentials::get_secret)
}

#[cfg(test)]
pub(crate) fn watched_values_unlocked() -> Result<Vec<String>, CredentialError> {
    Ok(Vec::new())
}

fn clear_with(
    get: &impl Fn(&str) -> Result<Option<String>, CredentialError>,
    put: &mut impl FnMut(&str, &str) -> Result<(), CredentialError>,
) -> Result<(), CredentialError> {
    let mut queue = load_with(get)?;
    if !queue.entries.is_empty() || !queue.watched_names.is_empty() {
        queue.entries.clear();
        queue.watched_names.clear();
        put(
            QUEUE,
            &serde_json::to_string(&queue).map_err(|_| CredentialError::WriteFailed)?,
        )?;
    }
    Ok(())
}

fn clear_after_upload_with(
    database: &std::path::Path,
    get: &impl Fn(&str) -> Result<Option<String>, CredentialError>,
    put: &mut impl FnMut(&str, &str) -> Result<(), CredentialError>,
) -> Result<(), CredentialError> {
    if load_with(get)?.entries.is_empty() {
        return clear_with(get, put);
    }
    let mut db = crate::storage::open_writable_connection(database)
        .map_err(|_| CredentialError::WriteFailed)?;
    db.busy_timeout(std::time::Duration::from_millis(100))
        .map_err(|_| CredentialError::WriteFailed)?;
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| CredentialError::WriteFailed)?;
    let dirty: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_archive_changes) OR EXISTS(SELECT 1 FROM app_runtime_meta WHERE key GLOB 'chat_archive_write:*' OR key GLOB 'chat_archive_delete:*')", [], |row| row.get(0)).map_err(|_| CredentialError::WriteFailed)?;
    if dirty {
        return Err(CredentialError::WriteFailed);
    }
    clear_with(get, put)?;
    tx.commit().map_err(|_| CredentialError::WriteFailed)
}

#[cfg(not(test))]
pub(crate) fn clear_after_upload_unlocked(
    database: &std::path::Path,
) -> Result<(), CredentialError> {
    clear_after_upload_with(
        database,
        &credentials::get_secret,
        &mut credentials::put_secret_unlocked,
    )
}

#[cfg(test)]
pub(crate) fn clear_after_upload_unlocked(
    _database: &std::path::Path,
) -> Result<(), CredentialError> {
    Ok(())
}

pub(crate) fn material_with(
    get: &impl Fn(&str) -> Result<Option<String>, CredentialError>,
) -> Result<Vec<String>, CredentialError> {
    let queue = load_with(get)?;
    let mut values = Vec::new();
    for entry in queue.entries {
        if entry.kind == "savedLogins" {
            values.extend(
                crate::saved_logins::passwords_from_serialized(&entry.value)
                    .map_err(|_| CredentialError::ReadFailed)?,
            );
        } else if entry.kind == "objectStore" {
            let object: serde_json::Value =
                serde_json::from_str(&entry.value).map_err(|_| CredentialError::ReadFailed)?;
            for field in ["accessKeyId", "secretAccessKey"] {
                values.push(
                    object
                        .get(field)
                        .and_then(serde_json::Value::as_str)
                        .ok_or(CredentialError::ReadFailed)?
                        .to_owned(),
                );
            }
        } else if entry.kind == "ai" || entry.kind == "backup" {
            values.push(entry.value);
        } else {
            return Err(CredentialError::ReadFailed);
        }
    }
    Ok(values)
}

#[cfg(not(test))]
pub(crate) fn counts() -> Result<(usize, usize), CredentialError> {
    let queue = load_with(&credentials::get_secret)?;
    Ok((
        queue.entries.len(),
        usize::try_from(queue.expired_count).unwrap_or(usize::MAX),
    ))
}
#[cfg(test)]
pub(crate) fn counts() -> Result<(usize, usize), CredentialError> {
    Ok((0, 0))
}

fn expire_with(
    now: i64,
    get: &impl Fn(&str) -> Result<Option<String>, CredentialError>,
    put: &mut impl FnMut(&str, &str) -> Result<(), CredentialError>,
) -> Result<(), CredentialError> {
    let mut queue = load_with(get)?;
    let before = queue.entries.len();
    queue
        .entries
        .retain(|entry| now.saturating_sub(entry.created_at) < MAX_AGE_SECONDS);
    let expired = before - queue.entries.len();
    if expired > 0 {
        queue.expired_count = queue.expired_count.saturating_add(expired as u64);
        put(
            QUEUE,
            &serde_json::to_string(&queue).map_err(|_| CredentialError::WriteFailed)?,
        )?;
    }
    Ok(())
}

#[cfg(not(test))]
pub(crate) fn expire_unlocked(now: i64) -> Result<(), CredentialError> {
    expire_with(
        now,
        &credentials::get_secret,
        &mut credentials::put_secret_unlocked,
    )
}
#[cfg(test)]
pub(crate) fn expire_unlocked(_now: i64) -> Result<(), CredentialError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashMap};
    #[test]
    fn retired_saved_login_values_remain_available_for_redaction() {
        let store = RefCell::new(HashMap::<String, String>::new());
        let get = |name: &str| Ok(store.borrow().get(name).cloned());
        let mut put = |name: &str, value: &str| {
            store.borrow_mut().insert(name.into(), value.into());
            Ok(())
        };
        let secret = uuid::Uuid::new_v4().to_string();
        let vault = serde_json::json!([{"id":"id","origin":"https://example.test","label":"label","username":"user","password":secret}]).to_string();
        put(crate::saved_logins::VAULT, &vault).unwrap();
        watch_new_with(crate::saved_logins::VAULT, &get, &mut put).unwrap();
        let watched = watched_values_with(&get).unwrap();
        enqueue_with(crate::saved_logins::VAULT, &vault, 100, &get, &mut put).unwrap();
        let expected = serde_json::from_str::<serde_json::Value>(&vault).unwrap()[0]["password"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(watched, vec![expected.clone()]);
        assert_eq!(material_with(&get).unwrap(), vec![expected]);
    }
    #[test]
    fn multiple_retired_keys_live_only_in_waiting_credential_and_expire() {
        let store = RefCell::new(HashMap::<String, String>::new());
        let get = |name: &str| Ok(store.borrow().get(name).cloned());
        let mut put = |name: &str, value: &str| {
            store.borrow_mut().insert(name.into(), value.into());
            Ok(())
        };
        let a = "fixture-previous-one";
        let b = "fixture-previous-two";
        enqueue_with("ai:openai:api-key", a, 100, &get, &mut put).unwrap();
        enqueue_with("ai:openai:api-key", b, 101, &get, &mut put).unwrap();
        assert_eq!(store.borrow().len(), 1);
        assert_eq!(
            material_with(&get).unwrap(),
            vec![a.to_owned(), b.to_owned()]
        );
        expire_with(100 + MAX_AGE_SECONDS, &get, &mut put).unwrap();
        let raw = store.borrow().get(QUEUE).unwrap().clone();
        assert!(!raw.contains(a));
        assert!(raw.contains(b));
        assert_eq!(load_with(&get).unwrap().expired_count, 1);
        assert_eq!(material_with(&get).unwrap(), vec![b.to_owned()]);
        clear_with(&get, &mut put).unwrap();
        assert_eq!(load_with(&get).unwrap().expired_count, 1);
    }

    #[test]
    fn successful_upload_clears_all_waiting_values() {
        let store = RefCell::new(HashMap::<String, String>::new());
        let get = |name: &str| Ok(store.borrow().get(name).cloned());
        let mut put = |name: &str, value: &str| {
            store.borrow_mut().insert(name.into(), value.into());
            Ok(())
        };
        enqueue_with(
            "ai:openai:api-key",
            "fixture-previous-one",
            100,
            &get,
            &mut put,
        )
        .unwrap();
        enqueue_with(
            "ai:openai:api-key",
            "fixture-previous-two",
            101,
            &get,
            &mut put,
        )
        .unwrap();
        clear_with(&get, &mut put).unwrap();
        assert!(material_with(&get).unwrap().is_empty());
        let raw = store.borrow().get(QUEUE).unwrap().clone();
        assert!(!raw.contains("fixture-previous-one") && !raw.contains("fixture-previous-two"));
    }

    #[test]
    fn uploaded_check_selects_recently_saved_active_keys() {
        let store = RefCell::new(HashMap::<String, String>::new());
        let get = |name: &str| Ok(store.borrow().get(name).cloned());
        let mut put = |name: &str, value: &str| {
            store.borrow_mut().insert(name.into(), value.into());
            Ok(())
        };
        put("ai:openai:api-key", "fixture-new").unwrap();
        put("ai:anthropic:api-key", "fixture-other").unwrap();
        watch_new_with("ai:openai:api-key", &get, &mut put).unwrap();
        assert_eq!(watched_values_with(&get).unwrap(), vec!["fixture-new"]);
        assert!(!store.borrow().get(QUEUE).unwrap().contains("fixture-new"));
    }

    #[test]
    fn upload_keeps_old_keys_if_chat_changed_during_push() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("fixture.db");
        crate::storage::initialize(&database, &temp.path().join("backups"), "fixture").unwrap();
        let store = RefCell::new(HashMap::<String, String>::new());
        let get = |name: &str| Ok(store.borrow().get(name).cloned());
        let mut put = |name: &str, value: &str| {
            store.borrow_mut().insert(name.into(), value.into());
            Ok(())
        };
        enqueue_with("ai:openai:api-key", "fixture-old", 100, &get, &mut put).unwrap();
        let db = rusqlite::Connection::open(&database).unwrap();
        db.execute(
            "INSERT INTO chat_archive_changes(chat_id,revision) VALUES('fixture',1)",
            [],
        )
        .unwrap();
        assert!(clear_after_upload_with(&database, &get, &mut put).is_err());
        assert_eq!(material_with(&get).unwrap(), vec!["fixture-old"]);
        db.execute("DELETE FROM chat_archive_changes", []).unwrap();
        clear_after_upload_with(&database, &get, &mut put).unwrap();
        assert!(material_with(&get).unwrap().is_empty());
    }
}
