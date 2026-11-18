use age::{secrecy::ExposeSecret, x25519};
use std::{str::FromStr, sync::Mutex};
use thiserror::Error;

const SERVICE: &str = "app.momso.desktop";
static ENTRY_CREATION_LOCK: Mutex<()> = Mutex::new(());
static SECRET_CHANGE_LOCK: Mutex<()> = Mutex::new(());

#[cfg(any(test, all(debug_assertions, feature = "g61-test-probe")))]
fn fake_store() -> Option<&'static Mutex<std::collections::HashMap<String, String>>> {
    use std::sync::OnceLock;
    static STORE: OnceLock<Mutex<std::collections::HashMap<String, String>>> = OnceLock::new();
    std::env::var_os("MOMSO_TEST_FAKE_CREDENTIAL_STORE")
        .map(|_| STORE.get_or_init(|| Mutex::new(std::collections::HashMap::new())))
}

pub(crate) fn prevent_secret_changes() -> Result<std::sync::MutexGuard<'static, ()>, CredentialError>
{
    SECRET_CHANGE_LOCK
        .lock()
        .map_err(|_| CredentialError::StoreUnavailable)
}

pub(crate) fn try_prevent_secret_changes(
) -> Result<std::sync::MutexGuard<'static, ()>, CredentialError> {
    SECRET_CHANGE_LOCK
        .try_lock()
        .map_err(|_| CredentialError::StoreUnavailable)
}

#[derive(Debug, Error)]
pub(crate) enum CredentialError {
    #[error("Windows 자격 증명 저장소를 사용할 수 없습니다")]
    StoreUnavailable,
    #[error("저장된 자격 증명을 읽을 수 없습니다")]
    ReadFailed,
    #[error("자격 증명을 안전하게 저장하지 못했습니다")]
    WriteFailed,
    #[error("자격 증명을 삭제하지 못했습니다")]
    DeleteFailed,
    #[error("복구 키 형식이 올바르지 않습니다")]
    InvalidRecoveryKey,
}

fn service_name(test_mode: bool) -> &'static str {
    if test_mode {
        "app.momso.desktop.r2-test"
    } else {
        SERVICE
    }
}

fn entry(name: &str) -> Result<keyring::Entry, CredentialError> {
    // keyring marks initialization as started before the Windows store is fully registered.
    // Settings probes run concurrently, so serialize entry creation across that short window.
    let _guard = ENTRY_CREATION_LOCK
        .lock()
        .map_err(|_| CredentialError::StoreUnavailable)?;
    #[cfg(not(test))]
    let service = service_name(crate::runtime::r2_test_enabled());
    #[cfg(test)]
    let service = {
        static TEST_SERVICE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        TEST_SERVICE
            .get_or_init(|| format!("app.momso.desktop.unit-test.{}", uuid::Uuid::new_v4()))
            .as_str()
    };
    keyring::Entry::new(service, name).map_err(|_| CredentialError::StoreUnavailable)
}

pub(crate) fn has_secret(name: &str) -> Result<bool, CredentialError> {
    #[cfg(any(test, all(debug_assertions, feature = "g61-test-probe")))]
    if let Some(store) = fake_store() {
        return Ok(store
            .lock()
            .map_err(|_| CredentialError::ReadFailed)?
            .contains_key(name));
    }
    match entry(name)?.get_password() {
        Ok(_) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(_) => Err(CredentialError::ReadFailed),
    }
}

pub(crate) fn get_secret(name: &str) -> Result<Option<String>, CredentialError> {
    #[cfg(any(test, all(debug_assertions, feature = "g61-test-probe")))]
    if let Some(store) = fake_store() {
        return Ok(store
            .lock()
            .map_err(|_| CredentialError::ReadFailed)?
            .get(name)
            .cloned());
    }
    match entry(name)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err(CredentialError::ReadFailed),
    }
}

pub(crate) fn set_secret(name: &str, secret: &str) -> Result<(), CredentialError> {
    let _guard = prevent_secret_changes()?;
    if crate::redaction_wait::protects(name) {
        if let Some(old) = get_secret(name)? {
            if old != secret {
                crate::redaction_wait::enqueue_unlocked(name, &old)?;
            }
        }
    }
    put_secret_unlocked(name, secret)?;
    if crate::redaction_wait::protects(name) {
        crate::redaction_wait::watch_new_unlocked(name)?;
    }
    Ok(())
}

pub(crate) fn put_secret_unlocked(name: &str, secret: &str) -> Result<(), CredentialError> {
    #[cfg(any(test, all(debug_assertions, feature = "g61-test-probe")))]
    if let Some(store) = fake_store() {
        store
            .lock()
            .map_err(|_| CredentialError::WriteFailed)?
            .insert(name.to_owned(), secret.to_owned());
        return Ok(());
    }
    entry(name)?
        .set_password(secret)
        .map_err(|_| CredentialError::WriteFailed)
}

pub(crate) fn delete_secret(name: &str) -> Result<(), CredentialError> {
    let _guard = prevent_secret_changes()?;
    if crate::redaction_wait::protects(name) {
        if let Some(old) = get_secret(name)? {
            crate::redaction_wait::enqueue_unlocked(name, &old)?;
        }
    }
    remove_secret_unlocked(name)
}

pub(crate) fn remove_secret_unlocked(name: &str) -> Result<(), CredentialError> {
    #[cfg(any(test, all(debug_assertions, feature = "g61-test-probe")))]
    if let Some(store) = fake_store() {
        store
            .lock()
            .map_err(|_| CredentialError::DeleteFailed)?
            .remove(name);
        return Ok(());
    }
    match entry(name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(CredentialError::DeleteFailed),
    }
}

pub(crate) fn ai_key_name(provider: &str) -> Result<&'static str, CredentialError> {
    match provider {
        "openai" => Ok("ai:openai:api-key"),
        "anthropic" => Ok("ai:anthropic:api-key"),
        "gemini" => Ok("ai:gemini:api-key"),
        _ => Err(CredentialError::StoreUnavailable),
    }
}

fn recovery_key_name(workspace_id: &str) -> String {
    format!("backup:{workspace_id}:age-identity")
}

pub(crate) fn has_backup_identity(workspace_id: &str) -> Result<bool, CredentialError> {
    has_secret(&recovery_key_name(workspace_id))
}

pub(crate) fn get_backup_identity(
    workspace_id: &str,
) -> Result<Option<x25519::Identity>, CredentialError> {
    get_secret(&recovery_key_name(workspace_id))?
        .map(|stored| {
            x25519::Identity::from_str(stored.trim())
                .map_err(|_| CredentialError::InvalidRecoveryKey)
        })
        .transpose()
}

pub(crate) fn get_or_create_backup_identity(
    workspace_id: &str,
) -> Result<(x25519::Identity, bool), CredentialError> {
    let name = recovery_key_name(workspace_id);
    if let Some(identity) = get_backup_identity(workspace_id)? {
        return Ok((identity, false));
    }

    let identity = x25519::Identity::generate();
    set_secret(&name, identity.to_string().expose_secret())?;
    Ok((identity, true))
}

pub(crate) fn export_backup_identity(workspace_id: &str) -> Result<String, CredentialError> {
    let (identity, _) = get_or_create_backup_identity(workspace_id)?;
    Ok(identity.to_string().expose_secret().to_owned())
}

pub(crate) fn import_backup_identity(
    workspace_id: &str,
    recovery_key: &str,
) -> Result<(), CredentialError> {
    let identity = x25519::Identity::from_str(recovery_key.trim())
        .map_err(|_| CredentialError::InvalidRecoveryKey)?;
    set_secret(
        &recovery_key_name(workspace_id),
        identity.to_string().expose_secret(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credentials_do_not_share_the_production_service() {
        assert_eq!(service_name(false), SERVICE);
        assert_eq!(service_name(true), "app.momso.desktop.r2-test");
        assert_ne!(service_name(false), service_name(true));
    }

    #[test]
    fn credential_names_do_not_contain_secret_material() {
        assert_eq!(ai_key_name("openai").expect("openai"), "ai:openai:api-key");
        assert_eq!(
            ai_key_name("anthropic").expect("anthropic"),
            "ai:anthropic:api-key"
        );
        assert_eq!(ai_key_name("gemini").expect("gemini"), "ai:gemini:api-key");
        assert_eq!(
            recovery_key_name("workspace-123"),
            "backup:workspace-123:age-identity"
        );
    }
}
