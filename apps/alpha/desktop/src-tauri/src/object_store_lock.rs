//! Serializes source backup and permanent deletion across participating PCs.
use crate::{object_store, object_store_transfer, source_delete};
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
};
use uuid::Uuid;

const KEY: &str = "momso/object-store/v1/locks/source-mutation";
const OWNER_FILE: &str = ".momso-source-operation-owner";
const INSTALLATION_KEY: &str = "object-store:source-lock-installation";

pub(crate) struct SourceMutationLock {
    owner: String,
    // The OS lock is released if the process dies; the machine-bound owner
    // lets this installation recover its remote lock after a crash.
    _local_lock: File,
}

fn local_file_lock(root: &Path) -> Result<File, &'static str> {
    let root_metadata = fs::symlink_metadata(root).map_err(|_| "local_read_failed")?;
    if !root_metadata.is_dir() || crate::archive_location::is_link_or_reparse(&root_metadata) {
        return Err("unsafe_path");
    }
    let path = root.join(OWNER_FILE);
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let meta = fs::symlink_metadata(&path).map_err(|_| "local_read_failed")?;
            if !meta.is_file()
                || crate::archive_location::is_link_or_reparse(&meta)
                || meta.len() != 0
            {
                return Err("local_lock_invalid");
            }
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|_| "local_read_failed")?
        }
        Err(_) => return Err("local_write_failed"),
    };
    file.try_lock().map_err(|_| "local_operation_busy")?;
    Ok(file)
}

fn valid_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|parsed| parsed.to_string() == value)
}

fn installation_id_with(
    get: impl FnOnce() -> Result<Option<String>, &'static str>,
    set: impl FnOnce(&str) -> Result<(), &'static str>,
) -> Result<String, &'static str> {
    match get()? {
        Some(value) if valid_uuid(&value) => Ok(value),
        Some(_) => Err("local_lock_invalid"),
        None => {
            let value = Uuid::new_v4().to_string();
            set(&value)?;
            Ok(value)
        }
    }
}

fn installation_id() -> Result<String, &'static str> {
    installation_id_with(
        || crate::credentials::get_secret(INSTALLATION_KEY).map_err(|_| "local_lock_unavailable"),
        |value| {
            crate::credentials::set_secret(INSTALLATION_KEY, value)
                .map_err(|_| "local_lock_unavailable")
        },
    )
}

fn owner_for(root: &Path, credential_id: &str, installation: &str) -> Result<String, &'static str> {
    if !valid_uuid(credential_id) || !valid_uuid(installation) {
        return Err("local_lock_invalid");
    }
    let canonical = fs::canonicalize(root).map_err(|_| "local_read_failed")?;
    let path = canonical.to_str().ok_or("unsafe_path")?;
    Ok(object_store::digest(
        format!("v1\0{installation}\0{credential_id}\0{path}").as_bytes(),
    ))
}

pub(crate) fn supported(config: &object_store::Config) -> bool {
    matches!(config.provider.as_str(), "aws" | "gcp")
        && config.write_safety == Some(object_store::WriteSafety::Conditional)
}

impl SourceMutationLock {
    pub(crate) async fn acquire(
        root: &Path,
        config: &object_store::Config,
        keys: &object_store::Keys,
    ) -> Result<Self, &'static str> {
        if !supported(config) {
            return Err("remote_lock_unsupported");
        }
        let file = local_file_lock(root)?;
        let owner = owner_for(root, &config.credential_id, &installation_id()?)?;
        Self::acquire_locked(file, owner, config, keys).await
    }

    async fn acquire_locked(
        file: File,
        owner: String,
        config: &object_store::Config,
        keys: &object_store::Keys,
    ) -> Result<Self, &'static str> {
        match object_store_transfer::put_once(config, keys, KEY, owner.as_bytes().to_vec()).await {
            Ok(()) => Ok(Self {
                owner,
                _local_lock: file,
            }),
            Err("remote_conflict" | "invalid_remote_object") => Err("remote_operation_busy"),
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn release(
        self,
        config: &object_store::Config,
        keys: &object_store::Keys,
    ) -> Result<(), &'static str> {
        if source_delete::delete_remote_key(
            config,
            keys,
            KEY,
            &object_store::digest(self.owner.as_bytes()),
        )
        .await
        {
            Ok(())
        } else {
            Err("remote_lock_release_failed")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
        time::Duration,
    };

    async fn acquire_fake(
        root: &Path,
        config: &object_store::Config,
        keys: &object_store::Keys,
        installation: &str,
    ) -> Result<SourceMutationLock, &'static str> {
        if !supported(config) {
            return Err("remote_lock_unsupported");
        }
        let file = local_file_lock(root)?;
        let owner = owner_for(root, &config.credential_id, installation)?;
        SourceMutationLock::acquire_locked(file, owner, config, keys).await
    }

    #[test]
    fn installation_id_is_stable_and_store_errors_do_not_overwrite() {
        use std::cell::RefCell;
        let store = RefCell::new(None::<String>);
        let get = || Ok(store.borrow().clone());
        let set = |value: &str| {
            *store.borrow_mut() = Some(value.into());
            Ok(())
        };
        let first = installation_id_with(get, set).unwrap();
        assert!(valid_uuid(&first));
        let second = installation_id_with(
            || Ok(store.borrow().clone()),
            |_| panic!("must not replace existing ID"),
        )
        .unwrap();
        assert_eq!(first, second);
        *store.borrow_mut() = Some("invalid".into());
        assert_eq!(
            installation_id_with(
                || Ok(store.borrow().clone()),
                |_| panic!("must preserve invalid data")
            ),
            Err("local_lock_invalid")
        );
        assert_eq!(store.borrow().as_deref(), Some("invalid"));
        assert_eq!(
            installation_id_with(
                || Err("local_lock_unavailable"),
                |_| panic!("must not write after read error")
            ),
            Err("local_lock_unavailable")
        );
    }

    #[test]
    fn cloned_connection_uses_distinct_machine_and_root_owners() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let connection = Uuid::new_v4().to_string();
        let machine_a = Uuid::new_v4().to_string();
        let machine_b = Uuid::new_v4().to_string();
        let first = owner_for(&a, &connection, &machine_a).unwrap();
        assert_ne!(first, owner_for(&a, &connection, &machine_b).unwrap());
        assert_ne!(first, owner_for(&b, &connection, &machine_a).unwrap());
        assert_eq!(first, owner_for(&a, &connection, &machine_a).unwrap());
        assert!(!first.contains(&connection));
    }

    #[tokio::test]
    async fn two_pcs_serialize_and_original_owner_recovers_after_crash() {
        for provider in ["aws", "gcp"] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            let marker = Arc::new(Mutex::new(None::<Vec<u8>>));
            let remote = marker.clone();
            let done = Arc::new(AtomicBool::new(false));
            let finished = done.clone();
            let lose_response = Arc::new(AtomicBool::new(false));
            let server_lose_response = lose_response.clone();
            let server = std::thread::spawn(move || {
                let mut initial_reads = Vec::new();
                let mut initial_reads_released = false;
                while !finished.load(Ordering::Relaxed) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(10)))
                        .unwrap();
                    let mut bytes = Vec::new();
                    let mut buf = [0u8; 2048];
                    let end = loop {
                        let n = stream.read(&mut buf).unwrap();
                        assert_ne!(n, 0);
                        bytes.extend_from_slice(&buf[..n]);
                        if let Some(i) = bytes.windows(4).position(|p| p == b"\r\n\r\n") {
                            break i + 4;
                        }
                    };
                    let header = String::from_utf8(bytes[..end].to_vec())
                        .unwrap()
                        .to_ascii_lowercase();
                    let method = header.split_whitespace().next().unwrap();
                    let len = header
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length:")
                                .and_then(|s| s.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    while bytes.len() - end < len {
                        let n = stream.read(&mut buf).unwrap();
                        assert_ne!(n, 0);
                        bytes.extend_from_slice(&buf[..n]);
                    }
                    // Force both PCs to observe the same absent marker before
                    // either can issue its conditional PUT.
                    if !initial_reads_released {
                        assert_eq!(method, "get");
                        initial_reads.push(stream);
                        if initial_reads.len() == 2 {
                            for mut pending in initial_reads.drain(..) {
                                pending.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                            }
                            initial_reads_released = true;
                        }
                        continue;
                    }
                    let mut current = remote.lock().unwrap();
                    let (status, body) = match method {
                        "get" => match current.as_ref() {
                            Some(value) => ("200 OK", value.clone()),
                            None => ("404 Not Found", vec![]),
                        },
                        "put" => {
                            assert!(
                                header.contains("if-none-match: *")
                                    || header.contains("x-goog-if-generation-match: 0")
                            );
                            if current.is_some() {
                                ("412 Precondition Failed", vec![])
                            } else {
                                *current = Some(bytes[end..end + len].to_vec());
                                if server_lose_response.swap(false, Ordering::Relaxed) {
                                    // Persist the lock but lose the response as if the
                                    // connection or app ended at this exact point.
                                    continue;
                                }
                                ("200 OK", vec![])
                            }
                        }
                        "delete" => {
                            assert!(
                                header.contains("if-match: \"fixture\"")
                                    || header.contains("x-goog-if-generation-match: 7")
                            );
                            *current = None;
                            ("204 No Content", vec![])
                        }
                        "head" => (
                            if current.is_some() {
                                "200 OK"
                            } else {
                                "404 Not Found"
                            },
                            vec![],
                        ),
                        _ => panic!("unexpected method"),
                    };
                    write!(stream, "HTTP/1.1 {status}\r\nETag: \"fixture\"\r\nx-goog-generation: 7\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                    if method != "head" {
                        stream.write_all(&body).unwrap();
                    }
                }
            });
            let temp = tempfile::tempdir().unwrap();
            let a = temp.path().join("pc-a");
            let b = temp.path().join("pc-b");
            fs::create_dir(&a).unwrap();
            fs::create_dir(&b).unwrap();
            let first = object_store::Config {
                provider: provider.into(),
                endpoint: format!("http://{address}"),
                region: "auto".into(),
                bucket: "bucket".into(),
                credential_id: Uuid::new_v4().to_string(),
                last_backup_at: None,
                write_safety: Some(object_store::WriteSafety::Conditional),
            };
            // Model cloned connection settings on two different PCs.
            let second = first.clone();
            let machine_a = Uuid::new_v4().to_string();
            let machine_b = Uuid::new_v4().to_string();
            let keys = object_store::Keys {
                access_key_id: "TEST".into(),
                secret_access_key: "test".into(),
            };
            // Both PCs can observe the lock as absent before either PUT. The
            // server's conditional creation must still admit exactly one.
            let (first_race, second_race) = tokio::join!(
                acquire_fake(&a, &first, &keys, &machine_a),
                acquire_fake(&b, &second, &keys, &machine_b),
            );
            match (first_race, second_race) {
                (Ok(lock), Err("remote_operation_busy")) => {
                    lock.release(&first, &keys).await.unwrap()
                }
                (Err("remote_operation_busy"), Ok(lock)) => {
                    lock.release(&second, &keys).await.unwrap()
                }
                _ => panic!("exactly one PC must acquire the remote lock"),
            }
            let held = acquire_fake(&a, &first, &keys, &machine_a).await.unwrap();
            assert!(matches!(
                acquire_fake(&a, &first, &keys, &machine_a).await,
                Err("local_operation_busy")
            ));
            assert!(matches!(
                acquire_fake(&b, &second, &keys, &machine_b).await,
                Err("remote_operation_busy")
            ));
            drop(held); // Simulate a process crash after acquiring the remote marker.
            assert!(matches!(
                acquire_fake(&b, &second, &keys, &machine_b).await,
                Err("remote_operation_busy")
            ));
            let recovered = acquire_fake(&a, &first, &keys, &machine_a).await.unwrap();
            recovered.release(&first, &keys).await.unwrap();
            lose_response.store(true, Ordering::Relaxed);
            match acquire_fake(&a, &first, &keys, &machine_a).await {
                Ok(lock) => drop(lock), // The HTTP client may safely retry.
                Err("invalid_endpoint" | "remote_write_failed") => (),
                Err(_) => panic!("unexpected interrupted acquisition result"),
            }
            assert!(marker.lock().unwrap().is_some());
            assert!(matches!(
                acquire_fake(&b, &second, &keys, &machine_b).await,
                Err("remote_operation_busy")
            ));
            let recovered = acquire_fake(&a, &first, &keys, &machine_a).await.unwrap();
            recovered.release(&first, &keys).await.unwrap();
            let after = acquire_fake(&b, &second, &keys, &machine_b).await.unwrap();
            after.release(&second, &keys).await.unwrap();
            assert!(marker.lock().unwrap().is_none());
            done.store(true, Ordering::Relaxed);
            server.join().unwrap();
        }
    }

    #[tokio::test]
    async fn nonconditional_storage_does_not_attempt_a_lock() {
        let temp = tempfile::tempdir().unwrap();
        let config = object_store::Config {
            provider: "aws".into(),
            endpoint: "http://127.0.0.1:1".into(),
            region: "auto".into(),
            bucket: "bucket".into(),
            credential_id: Uuid::new_v4().to_string(),
            last_backup_at: None,
            write_safety: Some(object_store::WriteSafety::CheckThenWrite),
        };
        let keys = object_store::Keys {
            access_key_id: "TEST".into(),
            secret_access_key: "test".into(),
        };
        assert!(matches!(
            SourceMutationLock::acquire(temp.path(), &config, &keys).await,
            Err("remote_lock_unsupported")
        ));
        assert!(!temp.path().join(OWNER_FILE).exists());
    }
}
