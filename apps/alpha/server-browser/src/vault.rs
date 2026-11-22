//! Trusted host bridge to the S21 vault. Never serialize its return values.
use crate::{
    tools::{CredentialError, CredentialSource},
    LoginSecret,
};
use momso_saved_login_core::linux::{LinuxVault, LoginInfo};

pub trait LoginSelector {
    /// Return an id from `choices`, or None to cancel. This runs in the host.
    fn select(&mut self, origin: &str, choices: &[LoginInfo]) -> Option<String>;
}

pub struct VaultCredentials<S> {
    pub vault: LinuxVault,
    pub selector: S,
}

impl<S: LoginSelector> CredentialSource for VaultCredentials<S> {
    fn protected_values(&mut self) -> Result<Vec<String>, CredentialError> {
        self.vault
            .protected_values()
            .map_err(|_| CredentialError)
            .map(|values| {
                values
                    .into_iter()
                    .map(|mut value| std::mem::take(&mut *value))
                    .collect()
            })
    }

    fn login_for_origin(
        &mut self,
        origin: &str,
        top_url: &str,
    ) -> Result<Option<LoginSecret>, CredentialError> {
        let url = url::Url::parse(top_url).map_err(|_| CredentialError)?;
        if url.origin().ascii_serialization() != origin {
            return Err(CredentialError);
        }
        let choices: Vec<_> =
            self.vault
                .list()
                .map_err(|_| CredentialError)?
                .into_iter()
                .filter(|entry| {
                    entry.origin == origin
                        && entry.login_path.as_ref().is_none_or(|path| {
                            url.path() == path
                                || url.path().strip_prefix(path).is_some_and(|rest| {
                                    path.ends_with('/') || rest.starts_with('/')
                                })
                        })
                })
                .collect();
        if choices.is_empty() {
            return Ok(None);
        }
        let Some(id) = self.selector.select(origin, &choices) else {
            return Ok(None);
        };
        if !choices.iter().any(|choice| choice.id == id) {
            return Err(CredentialError);
        }
        // The host obtained approval before invoking this method; top-level forms only.
        let login = self
            .vault
            .browser_login(&id, top_url, top_url, true)
            .map_err(|_| CredentialError)?;
        Ok(Some(LoginSecret::new(login.username, login.password)))
    }
}
