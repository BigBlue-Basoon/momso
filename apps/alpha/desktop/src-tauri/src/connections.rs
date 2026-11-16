use crate::credentials;
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum ConnectionError {
    #[error(transparent)]
    Credential(#[from] credentials::CredentialError),
    #[error("API 키는 공백 없이 20자 이상이어야 합니다")]
    InvalidApiKey,
}

pub(crate) fn save_api_key(provider: &str, api_key: &str) -> Result<(), ConnectionError> {
    let api_key = api_key.trim();
    if api_key.len() < 20 || api_key.chars().any(char::is_whitespace) {
        return Err(ConnectionError::InvalidApiKey);
    }
    credentials::set_secret(credentials::ai_key_name(provider)?, api_key)?;
    Ok(())
}

pub(crate) fn delete_api_key(provider: &str) -> Result<(), ConnectionError> {
    credentials::delete_secret(credentials::ai_key_name(provider)?)?;
    Ok(())
}
