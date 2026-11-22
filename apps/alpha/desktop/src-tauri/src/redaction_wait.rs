//! The waiting queue, including its values, exists only in the OS credential store.
use crate::credentials::{self, CredentialError};
use serde::{Deserialize, Serialize};

const QUEUE: &str = "redaction-wait:queue-v1";
const JOURNAL: &str = "redaction-wait:journal-v2";
const MAX_AGE_SECONDS: i64 = 30 * 24 * 60 * 60;

#[derive(Deserialize, Serialize)]
struct Entry {
    kind: String,
    created_at: i64,
    value: String,
}

#[derive(Clone, Deserialize, Serialize)]
struct Shards {
    generation: String,
    count: usize,
}

#[derive(Deserialize, Serialize)]
struct Journal {
    next: Option<Shards>,
    previous: Option<Shards>,
}

#[derive(Deserialize, Serialize)]
struct ShardRoot {
    format: String,
    shards: Shards,
}

#[derive(Default, Deserialize, Serialize)]
struct Queue {
    entries: Vec<Entry>,
    expired_count: u64,
    #[serde(default)]
    watched_names: Vec<String>,
}

fn root_shards(value: &str) -> Result<Option<Shards>, CredentialError> {
    let json: serde_json::Value =
        serde_json::from_str(value).map_err(|_| CredentialError::ReadFailed)?;
    if json.get("format").and_then(serde_json::Value::as_str) == Some("sharded-v2") {
        let root: ShardRoot =
            serde_json::from_value(json).map_err(|_| CredentialError::ReadFailed)?;
        Ok(Some(root.shards))
    } else {
        let _: Queue = serde_json::from_value(json).map_err(|_| CredentialError::ReadFailed)?;
        Ok(None)
    }
}

fn load_with(
    get: &impl Fn(&str) -> Result<Option<String>, CredentialError>,
) -> Result<Queue, CredentialError> {
    match get(QUEUE)? {
        None => Ok(Queue::default()),
        Some(value) => {
            let Some(shards) = root_shards(&value)? else {
                return serde_json::from_str(&value).map_err(|_| CredentialError::ReadFailed);
            };
            if shards.count == 0
                || shards.count > 4096
                || uuid::Uuid::parse_str(&shards.generation).is_err()
            {
                return Err(CredentialError::ReadFailed);
            }
            let mut value = String::new();
            for index in 0..shards.count {
                let part = get(&shard_name(&shards.generation, index))?
                    .ok_or(CredentialError::ReadFailed)?;
                value.push_str(&part);
            }
            serde_json::from_str(&value).map_err(|_| CredentialError::ReadFailed)
        }
    }
}

fn shard_name(generation: &str, index: usize) -> String {
    format!("redaction-wait:shard-v2:{generation}:{index}")
}

fn queue_put(name: &str, value: &str) -> Result<(), CredentialError> {
    if name.starts_with("redaction-wait:shard-v2:") && value.is_empty() {
        credentials::remove_secret_unlocked(name)
    } else {
        credentials::put_secret_unlocked(name, value)
    }
}

fn scrub_shards(
    shards: &Shards,
    get: &impl Fn(&str) -> Result<Option<String>, CredentialError>,
    put: &mut impl FnMut(&str, &str) -> Result<(), CredentialError>,
) -> Result<(), CredentialError> {
    if shards.count > 4096 || uuid::Uuid::parse_str(&shards.generation).is_err() {
        return Err(CredentialError::ReadFailed);
    }
    for index in 0..shards.count {
        let name = shard_name(&shards.generation, index);
        if get(&name)?.is_some() {
            put(&name, "")?;
        }
    }
    Ok(())
}

fn recover_with(
    get: &impl Fn(&str) -> Result<Option<String>, CredentialError>,
    put: &mut impl FnMut(&str, &str) -> Result<(), CredentialError>,
) -> Result<(), CredentialError> {
    let Some(value) = get(JOURNAL)? else {
        return Ok(());
    };
    if value == "null" {
        return Ok(());
    }
    let journal: Journal = serde_json::from_str(&value).map_err(|_| CredentialError::ReadFailed)?;
    let active = get(QUEUE)?
        .map(|value| root_shards(&value))
        .transpose()?
        .flatten();
    let committed = match (&journal.next, &active) {
        (Some(next), Some(current)) => next.generation == current.generation,
        (None, None) => true,
        _ => false,
    };
    let obsolete = if committed {
        journal.previous.as_ref()
    } else {
        journal.next.as_ref()
    };
    if let Some(shards) = obsolete {
        scrub_shards(shards, get, put)?;
    }
    put(JOURNAL, "null")
}

fn save_with(
    queue: &Queue,
    get: &impl Fn(&str) -> Result<Option<String>, CredentialError>,
    put: &mut impl FnMut(&str, &str) -> Result<(), CredentialError>,
) -> Result<(), CredentialError> {
    recover_with(get, put)?;
    let previous = get(QUEUE)?
        .map(|value| root_shards(&value))
        .transpose()?
        .flatten();
    let encoded = serde_json::to_string(queue).map_err(|_| CredentialError::WriteFailed)?;
    if encoded.encode_utf16().count() <= 1000 {
        if previous.is_some() {
            put(
                JOURNAL,
                &serde_json::to_string(&Journal {
                    next: None,
                    previous: previous.clone(),
                })
                .map_err(|_| CredentialError::WriteFailed)?,
            )?;
        }
        put(QUEUE, &encoded)?;
    } else {
        let generation = uuid::Uuid::new_v4().to_string();
        let mut parts = Vec::new();
        let mut part = String::new();
        let mut units = 0;
        for ch in encoded.chars() {
            if units + ch.len_utf16() > 1000 {
                parts.push(std::mem::take(&mut part));
                units = 0;
            }
            part.push(ch);
            units += ch.len_utf16();
        }
        if !part.is_empty() {
            parts.push(part);
        }
        if parts.len() > 4096 {
            return Err(CredentialError::WriteFailed);
        }
        let next = Shards {
            generation: generation.clone(),
            count: parts.len(),
        };
        put(
            JOURNAL,
            &serde_json::to_string(&Journal {
                next: Some(next),
                previous: previous.clone(),
            })
            .map_err(|_| CredentialError::WriteFailed)?,
        )?;
        for (index, part) in parts.iter().enumerate() {
            if let Err(error) = put(&shard_name(&generation, index), part) {
                for written in 0..index {
                    let _ = put(&shard_name(&generation, written), "");
                }
                return Err(error);
            }
        }
        // A legacy binary cannot parse this root and therefore fails closed.
        let manifest = ShardRoot {
            format: "sharded-v2".into(),
            shards: Shards {
                generation,
                count: parts.len(),
            },
        };
        if let Err(error) = put(
            QUEUE,
            &serde_json::to_string(&manifest).map_err(|_| CredentialError::WriteFailed)?,
        ) {
            for index in 0..manifest.shards.count {
                let _ = put(&shard_name(&manifest.shards.generation, index), "");
            }
            return Err(error);
        }
    }
    recover_with(get, put)
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
    save_with(&queue, get, put)
}

pub(crate) fn enqueue_unlocked(active_name: &str, value: &str) -> Result<(), CredentialError> {
    enqueue_with(
        active_name,
        value,
        chrono::Utc::now().timestamp(),
        &credentials::get_secret,
        &mut queue_put,
    )
}

fn watch_new_with(
    name: &str,
    get: &impl Fn(&str) -> Result<Option<String>, CredentialError>,
    put: &mut impl FnMut(&str, &str) -> Result<(), CredentialError>,
) -> Result<(), CredentialError> {
    recover_with(get, put)?;
    let mut queue = load_with(get)?;
    if !queue.watched_names.iter().any(|saved| saved == name) {
        queue.watched_names.push(name.to_owned());
        save_with(&queue, get, put)?;
    }
    Ok(())
}

pub(crate) fn watch_new_unlocked(name: &str) -> Result<(), CredentialError> {
    watch_new_with(name, &credentials::get_secret, &mut queue_put)
}

fn watched_values_with(
    get: &impl Fn(&str) -> Result<Option<String>, CredentialError>,
) -> Result<Vec<String>, CredentialError> {
    let queue = load_with(get)?;
    let mut values = Vec::new();
    for name in queue.watched_names {
        if name == crate::saved_logins::VAULT {
            values.extend(
                crate::saved_logins::passwords_with(&|key| {
                    get(key).map_err(|_| "credential_store_unavailable")
                })
                .map_err(|_| CredentialError::ReadFailed)?,
            );
        } else if let Some(value) = get(&name)? {
            if name.starts_with("object-store:") {
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
    recover_with(get, put)?;
    let mut queue = load_with(get)?;
    if !queue.entries.is_empty() || !queue.watched_names.is_empty() {
        queue.entries.clear();
        queue.watched_names.clear();
        save_with(&queue, get, put)?;
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
    clear_after_upload_with(database, &credentials::get_secret, &mut queue_put)
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
    recover_with(get, put)?;
    let mut queue = load_with(get)?;
    let before = queue.entries.len();
    queue
        .entries
        .retain(|entry| now.saturating_sub(entry.created_at) < MAX_AGE_SECONDS);
    let expired = before - queue.entries.len();
    if expired > 0 {
        queue.expired_count = queue.expired_count.saturating_add(expired as u64);
        save_with(&queue, get, put)?;
    }
    Ok(())
}

#[cfg(not(test))]
pub(crate) fn expire_unlocked(now: i64) -> Result<(), CredentialError> {
    expire_with(now, &credentials::get_secret, &mut queue_put)
}
#[cfg(test)]
pub(crate) fn expire_unlocked(_now: i64) -> Result<(), CredentialError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashMap};
    #[cfg(windows)]
    #[test]
    fn split_queue_uses_real_isolated_windows_credentials() {
        use std::collections::HashSet;
        struct Cleanup {
            service: String,
            names: RefCell<HashSet<String>>,
        }
        impl Drop for Cleanup {
            fn drop(&mut self) {
                for name in self.names.borrow().iter() {
                    if let Ok(entry) = keyring::Entry::new(&self.service, name) {
                        let _ = entry.delete_credential();
                    }
                }
            }
        }
        let cleanup = Cleanup {
            service: format!("app.momso.s47.fixture.{}", uuid::Uuid::new_v4()),
            names: RefCell::new(HashSet::new()),
        };
        let get = |name: &str| {
            let entry = keyring::Entry::new(&cleanup.service, name)
                .map_err(|_| CredentialError::ReadFailed)?;
            match entry.get_password() {
                Ok(value) => Ok(Some(value)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(_) => Err(CredentialError::ReadFailed),
            }
        };
        let mut put = |name: &str, value: &str| {
            cleanup.names.borrow_mut().insert(name.into());
            keyring::Entry::new(&cleanup.service, name)
                .and_then(|entry| entry.set_password(value))
                .map_err(|_| CredentialError::WriteFailed)
        };
        let a = "fixture-one-".repeat(100);
        let b = "fixture-two-".repeat(100);
        enqueue_with("ai:openai:api-key", &a, 100, &get, &mut put).unwrap();
        enqueue_with("ai:openai:api-key", &b, 101, &get, &mut put).unwrap();
        assert_eq!(material_with(&get).unwrap(), vec![a, b]);
        clear_with(&get, &mut put).unwrap();
        assert!(material_with(&get).unwrap().is_empty());
        for name in cleanup.names.borrow().iter() {
            let entry = keyring::Entry::new(&cleanup.service, name).unwrap();
            entry.delete_credential().unwrap();
            assert!(matches!(entry.get_password(), Err(keyring::Error::NoEntry)));
        }
        cleanup.names.borrow_mut().clear();
    }

    #[test]
    fn queue_grows_past_windows_credential_limit_and_clears_old_shards() {
        let store = RefCell::new(HashMap::<String, String>::new());
        let get = |name: &str| Ok(store.borrow().get(name).cloned());
        let mut put = |name: &str, value: &str| {
            if value.encode_utf16().count() * 2 > 2560 {
                return Err(CredentialError::WriteFailed);
            }
            store.borrow_mut().insert(name.into(), value.into());
            Ok(())
        };
        let a = "가".repeat(580);
        let b = "나".repeat(500);
        enqueue_with("ai:openai:api-key", &a, 100, &get, &mut put).unwrap();
        enqueue_with("ai:openai:api-key", &b, 101, &get, &mut put).unwrap();
        assert_eq!(material_with(&get).unwrap(), vec![a.clone(), b.clone()]);
        let root = store.borrow().get(QUEUE).unwrap().clone();
        assert!(
            serde_json::from_str::<Queue>(&root).is_err(),
            "older binaries must fail closed"
        );
        clear_with(&get, &mut put).unwrap();
        assert!(material_with(&get).unwrap().is_empty());
        assert!(store
            .borrow()
            .values()
            .all(|value| !value.contains(&a) && !value.contains(&b)));
    }

    #[test]
    fn interrupted_shard_write_keeps_previous_queue() {
        let store = RefCell::new(HashMap::<String, String>::new());
        let get = |name: &str| Ok(store.borrow().get(name).cloned());
        let mut put = |name: &str, value: &str| {
            store.borrow_mut().insert(name.into(), value.into());
            Ok(())
        };
        let prior = "previous".repeat(150);
        enqueue_with("ai:openai:api-key", &prior, 100, &get, &mut put).unwrap();
        let mut failing = |name: &str, value: &str| {
            if name == QUEUE {
                return Err(CredentialError::WriteFailed);
            }
            store.borrow_mut().insert(name.into(), value.into());
            Ok(())
        };
        assert!(enqueue_with(
            "ai:openai:api-key",
            &"new".repeat(600),
            101,
            &get,
            &mut failing
        )
        .is_err());
        assert_eq!(material_with(&get).unwrap(), vec![prior]);
        assert!(store
            .borrow()
            .values()
            .all(|value| !value.contains(&"new".repeat(100))));
    }

    #[test]
    fn interrupted_cleanup_is_retried_without_losing_the_new_queue() {
        let store = RefCell::new(HashMap::<String, String>::new());
        let get = |name: &str| Ok(store.borrow().get(name).cloned());
        let mut put = |name: &str, value: &str| {
            store.borrow_mut().insert(name.into(), value.into());
            Ok(())
        };
        let a = "previous".repeat(150);
        let b = "next".repeat(350);
        enqueue_with("ai:openai:api-key", &a, 100, &get, &mut put).unwrap();
        let previous = root_shards(store.borrow().get(QUEUE).unwrap())
            .unwrap()
            .unwrap();
        let old_name = shard_name(&previous.generation, 0);
        let mut failing = |name: &str, value: &str| {
            if name == old_name && value.is_empty() {
                return Err(CredentialError::WriteFailed);
            }
            store.borrow_mut().insert(name.into(), value.into());
            Ok(())
        };
        assert!(enqueue_with("ai:openai:api-key", &b, 101, &get, &mut failing).is_err());
        assert_eq!(material_with(&get).unwrap(), vec![a.clone(), b.clone()]);
        recover_with(&get, &mut put).unwrap();
        assert_eq!(store.borrow().get(&old_name).map(String::as_str), Some(""));
        assert_eq!(material_with(&get).unwrap(), vec![a, b]);
    }

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
