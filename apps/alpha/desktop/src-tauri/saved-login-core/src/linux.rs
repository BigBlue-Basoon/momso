//! Encrypted, local service vault. The service manager supplies the age identity.
use crate::backend::CredentialBackend;
use age::x25519;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::str::FromStr;

const KEY: &str = "saved-logins";

// No Debug implementation: these types can hold a password.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneSaveRequest {
    pub site_url: String,
    pub label: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub ask_before_use: bool,
    #[serde(default)]
    pub login_path: Option<String>,
    #[serde(default)]
    pub default_for_path: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginInfo {
    pub id: String,
    pub site_url: String,
    pub origin: String,
    pub label: String,
    pub username: String,
    pub created_at: String,
    pub updated_at: String,
    pub favorite: bool,
    pub ask_before_use: bool,
    pub login_path: Option<String>,
    #[serde(default)]
    pub default_for_path: bool,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    info: LoginInfo,
    password: String,
}

/// Values are intentionally not serializable or printable; only the host browser
/// should receive them. Never pass them into an AI tool result.
pub struct BrowserLogin {
    pub username: String,
    pub password: String,
}

pub struct LinuxVault {
    directory: PathBuf,
    identity: x25519::Identity,
}

impl LinuxVault {
    pub fn is_within(&self, root: &Path) -> Result<bool, &'static str> {
        let vault = self
            .directory
            .canonicalize()
            .map_err(|_| "storage_unavailable")?;
        let root = root.canonicalize().map_err(|_| "storage_unavailable")?;
        Ok(vault.starts_with(root))
    }
    /// Service credentials are kept apart from browser login records and are
    /// accessible only to trusted host code, never through login-list results.
    pub fn store_service_secret(&self, name: &str, value: &str) -> Result<(), &'static str> {
        let _lock = self.lock()?;
        self.put(&service_key(name)?, value)
    }

    pub fn read_service_secret(&self, name: &str) -> Result<Option<String>, &'static str> {
        let _lock = self.lock()?;
        self.get(&service_key(name)?)
    }

    /// Serialize a host-only read/modify/write with all other vault users.
    pub fn update_service_secret<T>(
        &self,
        name: &str,
        update: impl FnOnce(Option<&str>) -> Result<(T, Option<String>), &'static str>,
    ) -> Result<T, &'static str> {
        let _lock = self.lock()?;
        let key = service_key(name)?;
        let current = self.get(&key)?;
        let (result, replacement) = update(current.as_deref())?;
        if let Some(replacement) = replacement {
            self.put(&key, &replacement)?;
        }
        Ok(result)
    }

    pub fn delete_service_secret(&self, name: &str) -> Result<(), &'static str> {
        let _lock = self.lock()?;
        self.remove(&service_key(name)?)
    }

    /// Host-only values for validating backup copies; never expose via AI or HTTP.
    pub fn login_secrets_for_redaction(&self) -> Result<Vec<String>, &'static str> {
        let _lock = self.lock()?;
        Ok(self
            .read_entries()?
            .into_iter()
            .map(|entry| entry.password)
            .collect())
    }

    /// Keeps credential changes serialized with a host-only backup operation.
    pub fn with_backup_secrets<T>(
        &self,
        name: &str,
        use_secrets: impl FnOnce(&str, &[String]) -> Result<T, &'static str>,
    ) -> Result<T, &'static str> {
        let _lock = self.lock()?;
        let service = self.get(&service_key(name)?)?.ok_or("not_connected")?;
        let logins: Vec<String> = self
            .read_entries()?
            .into_iter()
            .map(|entry| entry.password)
            .collect();
        use_secrets(&service, &logins)
    }
    /// `credential` is a systemd-supplied file, not a command-line argument.
    pub fn from_credential_file(directory: &Path, credential: &Path) -> Result<Self, &'static str> {
        let key = fs::read_to_string(credential).map_err(|_| "credential_unavailable")?;
        let identity = x25519::Identity::from_str(key.trim()).map_err(|_| "credential_invalid")?;
        Self::new(directory, identity)
    }

    pub fn from_systemd(directory: &Path) -> Result<Self, &'static str> {
        let root = std::env::var_os("CREDENTIALS_DIRECTORY").ok_or("credential_unavailable")?;
        Self::from_credential_file(directory, &PathBuf::from(root).join("momso-vault-identity"))
    }

    pub fn new(directory: &Path, identity: x25519::Identity) -> Result<Self, &'static str> {
        fs::create_dir_all(directory).map_err(|_| "storage_unavailable")?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|_| "storage_unavailable")?;
        Ok(Self {
            directory: directory.to_path_buf(),
            identity,
        })
    }

    fn lock(&self) -> Result<File, &'static str> {
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.directory.join("vault.lock"))
            .map_err(|_| "storage_unavailable")?;
        lock.lock_exclusive().map_err(|_| "storage_unavailable")?;
        Ok(lock)
    }

    fn read_entries(&self) -> Result<Vec<Entry>, &'static str> {
        match self.get(KEY)? {
            None => Ok(Vec::new()),
            Some(value) => serde_json::from_str(&value).map_err(|_| "storage_invalid"),
        }
    }

    fn write_entries(&self, entries: &[Entry]) -> Result<(), &'static str> {
        let json = serde_json::to_string(entries).map_err(|_| "storage_unavailable")?;
        self.put(KEY, &json)
    }

    /// Caller must already have authenticated the human and protected the channel.
    pub fn save_from_phone(&self, request: PhoneSaveRequest) -> Result<LoginInfo, &'static str> {
        let origin = exact_origin(&request.site_url)?;
        let _lock = self.lock()?;
        let mut entries = self.read_entries()?;
        let now = chrono::Utc::now().to_rfc3339();
        let info = LoginInfo {
            id: uuid::Uuid::new_v4().to_string(),
            site_url: request.site_url,
            origin,
            label: request.label,
            username: request.username,
            created_at: now.clone(),
            updated_at: now,
            favorite: false,
            ask_before_use: request.ask_before_use,
            login_path: request.login_path,
            default_for_path: request.default_for_path,
        };
        entries.push(Entry {
            info: info.clone(),
            password: request.password,
        });
        self.write_entries(&entries)?;
        Ok(info)
    }

    pub fn list(&self) -> Result<Vec<LoginInfo>, &'static str> {
        let _lock = self.lock()?;
        Ok(self
            .read_entries()?
            .into_iter()
            .map(|entry| entry.info)
            .collect())
    }

    /// Host-only values used to suppress accidental credential echoes in page results.
    /// This does not authorize a login or expose values to the AI.
    pub fn protected_values(&self) -> Result<Vec<zeroize::Zeroizing<String>>, &'static str> {
        let _lock = self.lock()?;
        Ok(self
            .read_entries()?
            .into_iter()
            .flat_map(|entry| {
                [
                    zeroize::Zeroizing::new(entry.info.username),
                    zeroize::Zeroizing::new(entry.password),
                ]
            })
            .collect())
    }

    pub fn delete(&self, id: &str) -> Result<(), &'static str> {
        let _lock = self.lock()?;
        let mut entries = self.read_entries()?;
        entries.retain(|entry| entry.info.id != id);
        self.write_entries(&entries)
    }

    pub fn update_from_phone(
        &self,
        id: &str,
        request: PhoneSaveRequest,
        favorite: bool,
    ) -> Result<LoginInfo, &'static str> {
        let origin = exact_origin(&request.site_url)?;
        let _lock = self.lock()?;
        let mut entries = self.read_entries()?;
        let entry = entries
            .iter_mut()
            .find(|entry| entry.info.id == id)
            .ok_or("login_not_found")?;
        entry.info.site_url = request.site_url;
        entry.info.origin = origin;
        entry.info.label = request.label;
        entry.info.username = request.username;
        entry.info.ask_before_use = request.ask_before_use;
        entry.info.login_path = request.login_path;
        entry.info.default_for_path = request.default_for_path;
        entry.info.favorite = favorite;
        entry.info.updated_at = chrono::Utc::now().to_rfc3339();
        entry.password = request.password;
        let info = entry.info.clone();
        self.write_entries(&entries)?;
        Ok(info)
    }

    /// Check the actual top-level and frame origin immediately before filling.
    pub fn browser_login(
        &self,
        id: &str,
        top_url: &str,
        frame_url: &str,
        approved: bool,
    ) -> Result<BrowserLogin, &'static str> {
        let _lock = self.lock()?;
        let entries = self.read_entries()?;
        let entry = entries
            .into_iter()
            .find(|entry| entry.info.id == id)
            .ok_or("login_not_found")?;
        if exact_origin(top_url)? != entry.info.origin
            || exact_origin(frame_url)? != entry.info.origin
        {
            return Err("origin_mismatch");
        }
        if entry.info.ask_before_use && !approved {
            return Err("approval_required");
        }
        Ok(BrowserLogin {
            username: entry.info.username,
            password: entry.password,
        })
    }
}

fn service_key(name: &str) -> Result<String, &'static str> {
    use sha2::{Digest, Sha256};
    if name.is_empty() || name.len() > 256 {
        return Err("invalid_key");
    }
    Ok(format!("service-{:x}", Sha256::digest(name.as_bytes())))
}

fn file_name(key: &str) -> Result<String, &'static str> {
    if key == KEY {
        return Ok("vault.age".into());
    }
    if key.len() == 72
        && key.starts_with("service-")
        && key[8..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Ok(format!("{key}.age"));
    }
    Err("invalid_key")
}

fn exact_origin(value: &str) -> Result<String, &'static str> {
    let url = url::Url::parse(value).map_err(|_| "invalid_site")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("invalid_site");
    }
    Ok(url.origin().ascii_serialization())
}

impl CredentialBackend for LinuxVault {
    fn get(&self, key: &str) -> Result<Option<String>, &'static str> {
        let file = match File::open(self.directory.join(file_name(key)?)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("storage_unavailable"),
        };
        let decryptor = age::Decryptor::new(file).map_err(|_| "storage_invalid")?;
        let reader = decryptor
            .decrypt(std::iter::once(&self.identity as &dyn age::Identity))
            .map_err(|_| "credential_invalid")?;
        serde_json::from_reader(reader)
            .map(Some)
            .map_err(|_| "storage_invalid")
    }

    fn put(&self, key: &str, value: &str) -> Result<(), &'static str> {
        let destination = self.directory.join(file_name(key)?);
        let path = self
            .directory
            .join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map_err(|_| "storage_unavailable")?;
            let recipient = self.identity.to_public();
            let encryptor =
                age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
                    .map_err(|_| "storage_unavailable")?;
            let mut writer = encryptor
                .wrap_output(&mut file)
                .map_err(|_| "storage_unavailable")?;
            serde_json::to_writer(&mut writer, value).map_err(|_| "storage_unavailable")?;
            writer.finish().map_err(|_| "storage_unavailable")?;
            file.flush().map_err(|_| "storage_unavailable")?;
            file.sync_all().map_err(|_| "storage_unavailable")?;
            fs::rename(&path, destination).map_err(|_| "storage_unavailable")?;
            File::open(&self.directory)
                .and_then(|f| f.sync_all())
                .map_err(|_| "storage_unavailable")?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&path);
        }
        result
    }

    fn remove(&self, key: &str) -> Result<(), &'static str> {
        match fs::remove_file(self.directory.join(file_name(key)?)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("storage_unavailable"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use age::secrecy::ExposeSecret;

    #[test]
    fn service_secret_is_encrypted_and_not_a_browser_login() {
        let dir = tempfile::tempdir().unwrap();
        let identity = x25519::Identity::generate();
        let encoded_identity = identity.to_string();
        let token = uuid::Uuid::new_v4().to_string();
        let vault = LinuxVault::new(dir.path(), identity).unwrap();
        vault.store_service_secret("github", &token).unwrap();
        assert!(vault.list().unwrap().is_empty());
        for item in fs::read_dir(dir.path()).unwrap() {
            let bytes = fs::read(item.unwrap().path()).unwrap();
            assert!(!bytes
                .windows(token.len())
                .any(|window| window == token.as_bytes()));
        }
        drop(vault);
        let identity = x25519::Identity::from_str(encoded_identity.expose_secret()).unwrap();
        let vault = LinuxVault::new(dir.path(), identity).unwrap();
        assert!(vault.read_service_secret("github").unwrap().as_deref() == Some(token.as_str()));
        vault.delete_service_secret("github").unwrap();
        assert!(vault.read_service_secret("github").unwrap().is_none());
    }

    fn request(secret: &str) -> PhoneSaveRequest {
        PhoneSaveRequest {
            site_url: "https://example.org/login".into(),
            label: "센터".into(),
            username: "teacher".into(),
            password: secret.into(),
            ask_before_use: false,
            login_path: Some("/login".into()),
            default_for_path: false,
        }
    }

    #[test]
    fn phone_store_restart_exact_origins_and_no_password_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let identity = x25519::Identity::generate();
        let identity_text = identity.to_string();
        let secret = uuid::Uuid::new_v4().to_string();
        let vault = LinuxVault::new(dir.path(), identity).unwrap();
        let info = vault.save_from_phone(request(&secret)).unwrap();
        assert!(!serde_json::to_string(&info).unwrap().contains(&secret));
        assert!(!serde_json::to_string(&vault.list().unwrap())
            .unwrap()
            .contains(&secret));
        drop(vault);
        let identity = x25519::Identity::from_str(identity_text.expose_secret()).unwrap();
        let vault = LinuxVault::new(dir.path(), identity).unwrap();
        let login = vault
            .browser_login(
                &info.id,
                "https://example.org/a",
                "https://example.org/login",
                false,
            )
            .unwrap();
        assert!(login.password == secret);
        assert!(matches!(
            vault.browser_login(
                &info.id,
                "https://example.org.evil.test/",
                "https://example.org/",
                false
            ),
            Err("origin_mismatch")
        ));
        assert!(matches!(
            vault.browser_login(
                &info.id,
                "https://example.org/",
                "https://evil.test/",
                false
            ),
            Err("origin_mismatch")
        ));
        assert!(matches!(
            vault.browser_login(
                &info.id,
                "https://example.org:444/",
                "https://example.org/",
                false
            ),
            Err("origin_mismatch")
        ));
        vault
            .update_from_phone(&info.id, request(&secret), true)
            .unwrap();
        assert!(vault.list().unwrap()[0].favorite);
        let mut needs_approval = request(&secret);
        needs_approval.ask_before_use = true;
        vault
            .update_from_phone(&info.id, needs_approval, true)
            .unwrap();
        assert!(matches!(
            vault.browser_login(
                &info.id,
                "https://example.org/",
                "https://example.org/",
                false
            ),
            Err("approval_required")
        ));
        assert!(vault
            .browser_login(
                &info.id,
                "https://example.org/",
                "https://example.org/",
                true
            )
            .is_ok());
        for file in fs::read_dir(dir.path()).unwrap() {
            let bytes = fs::read(file.unwrap().path()).unwrap();
            assert!(!bytes
                .windows(secret.len())
                .any(|window| window == secret.as_bytes()));
        }
        vault.delete(&info.id).unwrap();
        assert!(vault.list().unwrap().is_empty());
    }

    #[test]
    fn many_accounts_wrong_key_and_integrity() {
        let dir = tempfile::tempdir().unwrap();
        let vault = LinuxVault::new(dir.path(), x25519::Identity::generate()).unwrap();
        for _ in 0..50 {
            vault
                .save_from_phone(request(&uuid::Uuid::new_v4().to_string()))
                .unwrap();
        }
        assert_eq!(vault.list().unwrap().len(), 50);
        let other = LinuxVault::new(dir.path(), x25519::Identity::generate()).unwrap();
        assert!(other.list().is_err());
        fs::write(dir.path().join("vault.age"), b"corrupt").unwrap();
        assert!(vault.list().is_err());
    }

    #[test]
    fn service_credential_file_reopens_encrypted_vault() {
        let dir = tempfile::tempdir().unwrap();
        let identity = x25519::Identity::generate();
        let identity_text = identity.to_string();
        let credential_path = dir.path().join("service-credential");
        fs::write(&credential_path, identity_text.expose_secret()).unwrap();
        let vault_dir = dir.path().join("private");
        let vault = LinuxVault::from_credential_file(&vault_dir, &credential_path).unwrap();
        let secret = uuid::Uuid::new_v4().to_string();
        let info = vault.save_from_phone(request(&secret)).unwrap();
        drop(vault);
        let reopened = LinuxVault::from_credential_file(&vault_dir, &credential_path).unwrap();
        let login = reopened
            .browser_login(
                &info.id,
                "https://example.org",
                "https://example.org",
                false,
            )
            .unwrap();
        assert!(login.password == secret);
    }

    #[test]
    fn concurrent_phone_saves_do_not_lose_entries() {
        let dir = tempfile::tempdir().unwrap();
        let vault =
            std::sync::Arc::new(LinuxVault::new(dir.path(), x25519::Identity::generate()).unwrap());
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let vault = vault.clone();
            tasks.push(std::thread::spawn(move || {
                vault
                    .save_from_phone(request(&uuid::Uuid::new_v4().to_string()))
                    .unwrap();
            }));
        }
        for task in tasks {
            task.join().unwrap();
        }
        assert_eq!(vault.list().unwrap().len(), 8);
    }
}
