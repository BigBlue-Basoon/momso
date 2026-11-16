use age::{secrecy::ExposeSecret, x25519};
use std::{str::FromStr, sync::Mutex};
use thiserror::Error;

const SERVICE: &str = "app.momso.desktop";
static ENTRY_CREATION_LOCK: Mutex<()> = Mutex::new(());

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

fn entry(name: &str) -> Result<keyring::Entry, CredentialError> {
    // keyring marks initialization as started before the Windows store is fully registered.
    // Settings probes run concurrently, so serialize entry creation across that short window.
    let _guard = ENTRY_CREATION_LOCK
        .lock()
        .map_err(|_| CredentialError::StoreUnavailable)?;
    keyring::Entry::new(SERVICE, name).map_err(|_| CredentialError::StoreUnavailable)
}

pub(crate) fn has_secret(name: &str) -> Result<bool, CredentialError> {
    match entry(name)?.get_password() {
        Ok(_) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(_) => Err(CredentialError::ReadFailed),
    }
}

pub(crate) fn get_secret(name: &str) -> Result<Option<String>, CredentialError> {
    match entry(name)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err(CredentialError::ReadFailed),
    }
}

pub(crate) fn set_secret(name: &str, secret: &str) -> Result<(), CredentialError> {
    entry(name)?
        .set_password(secret)
        .map_err(|_| CredentialError::WriteFailed)
}

pub(crate) fn delete_secret(name: &str) -> Result<(), CredentialError> {
    match entry(name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(CredentialError::DeleteFailed),
    }
}

pub(crate) fn ai_key_name(provider: &str) -> Result<&'static str, CredentialError> {
    match provider {
        "openai" => Ok("ai:openai:api-key"),
        "anthropic" => Ok("ai:anthropic:api-key"),
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
    fn credential_names_do_not_contain_secret_material() {
        assert_eq!(ai_key_name("openai").expect("openai"), "ai:openai:api-key");
        assert_eq!(
            recovery_key_name("workspace-123"),
            "backup:workspace-123:age-identity"
        );
    }
}
