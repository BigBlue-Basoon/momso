/// Internal host-only credential storage. Values must never be returned to AI.
pub trait CredentialBackend {
    fn get(&self, key: &str) -> Result<Option<String>, &'static str>;
    fn put(&self, key: &str, value: &str) -> Result<(), &'static str>;
    fn remove(&self, key: &str) -> Result<(), &'static str>;
}
