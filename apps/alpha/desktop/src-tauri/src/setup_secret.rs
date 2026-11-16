//! One-time, chat-bound credential input. Secret values never enter the event.
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tauri::Emitter;
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SecretRequest {
    pub(crate) request_id: String,
    pub(crate) chat_id: String,
    pub(crate) service: String,
    pub(crate) account: String,
    pub(crate) resource_name: String,
    pub(crate) fields: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<&'static str>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SecretInput {
    pub(crate) access_key_id: String,
    pub(crate) secret_access_key: String,
    pub(crate) gcp_oauth_token: Option<String>,
}

trait SecretStore: Send + Sync {
    fn put(&self, name: &str, value: &str) -> Result<(), ()>;
    #[allow(dead_code)] // Used when the authenticated setup executor is connected.
    fn get(&self, name: &str) -> Result<Option<String>, ()>;
    fn remove(&self, name: &str) -> Result<(), ()>;
}
struct WindowsStore;
impl SecretStore for WindowsStore {
    fn put(&self, name: &str, value: &str) -> Result<(), ()> {
        crate::credentials::set_secret(name, value).map_err(|_| ())
    }
    fn get(&self, name: &str) -> Result<Option<String>, ()> {
        crate::credentials::get_secret(name).map_err(|_| ())
    }
    fn remove(&self, name: &str) -> Result<(), ()> {
        crate::credentials::delete_secret(name).map_err(|_| ())
    }
}

pub(crate) struct SecretLease {
    name: String,
    store: Arc<dyn SecretStore>,
}
impl SecretLease {
    #[allow(dead_code)] // Used when the authenticated setup executor is connected.
    pub(crate) fn take(&self) -> Result<SecretInput, &'static str> {
        let value = self
            .store
            .get(&self.name)
            .map_err(|_| "unavailable")?
            .ok_or("unavailable")?;
        serde_json::from_str(&value).map_err(|_| "unavailable")
    }
}
impl Drop for SecretLease {
    fn drop(&mut self) {
        if self.store.remove(&self.name).is_err() {
            log::warn!("setup credential cleanup failed");
        }
    }
}

struct Pending {
    chat_id: String,
    fields: Vec<&'static str>,
    sender: oneshot::Sender<SecretLease>,
}
#[derive(Clone)]
pub(crate) struct SecretBroker {
    pending: Arc<Mutex<HashMap<String, Pending>>>,
    store: Arc<dyn SecretStore>,
}
impl Default for SecretBroker {
    fn default() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            store: Arc::new(WindowsStore),
        }
    }
}
impl SecretBroker {
    pub(crate) async fn cancel(&self, request_id: &str, chat_id: &str) -> Result<(), &'static str> {
        let mut pending = self.pending.lock().await;
        match pending.get(request_id) {
            Some(request) if request.chat_id != chat_id => Err("not_found"),
            Some(_) => {
                pending.remove(request_id);
                Ok(())
            }
            None => Ok(()),
        }
    }
    async fn register(
        &self,
        request: &SecretRequest,
    ) -> Result<oneshot::Receiver<SecretLease>, &'static str> {
        let mut pending = self.pending.lock().await;
        if pending.contains_key(&request.request_id) {
            return Err("unavailable");
        }
        let (sender, receiver) = oneshot::channel();
        pending.insert(
            request.request_id.clone(),
            Pending {
                chat_id: request.chat_id.clone(),
                fields: request.fields.clone(),
                sender,
            },
        );
        Ok(receiver)
    }
    pub(crate) async fn submit(
        &self,
        request_id: &str,
        chat_id: &str,
        input: SecretInput,
    ) -> Result<(), &'static str> {
        if input.access_key_id.is_empty()
            || input.secret_access_key.is_empty()
            || input.access_key_id.len() > 4096
            || input.secret_access_key.len() > 4096
            || input
                .gcp_oauth_token
                .as_ref()
                .is_some_and(|v| v.len() > 16384)
        {
            return Err("invalid_input");
        }
        let mut pending = self.pending.lock().await;
        let Some(request) = pending.get(request_id).filter(|v| v.chat_id == chat_id) else {
            return Err("not_found");
        };
        if input.gcp_oauth_token.is_some() && !request.fields.contains(&"gcpOauthToken") {
            return Err("invalid_input");
        }
        if request.fields.contains(&"gcpOauthToken")
            && input.gcp_oauth_token.as_ref().is_none_or(|v| v.is_empty())
        {
            return Err("invalid_input");
        }
        let name = format!("setup-pending:{}", uuid::Uuid::new_v4());
        let secret = serde_json::to_string(&input).map_err(|_| "unavailable")?;
        if self.store.put(&name, &secret).is_err() {
            if self.store.remove(&name).is_err() {
                log::warn!("setup credential cleanup failed");
            }
            return Err("unavailable");
        }
        let lease = SecretLease {
            name,
            store: self.store.clone(),
        };
        let Some(request) = pending.remove(request_id) else {
            return Err("not_found");
        };
        request.sender.send(lease).map_err(|_| "cancelled")
    }
    #[allow(dead_code)]
    pub(crate) async fn request(
        &self,
        app: &tauri::AppHandle,
        request: SecretRequest,
        cancel: CancellationToken,
    ) -> Result<SecretLease, &'static str> {
        self.request_with(
            request,
            cancel,
            |value| app.emit("agent-setup-secret-request", value).is_ok(),
            |value| {
                let _ = app.emit("agent-setup-secret-closed", value);
            },
        )
        .await
    }
    pub(crate) async fn request_with(
        &self,
        request: SecretRequest,
        cancel: CancellationToken,
        emit: impl FnOnce(&SecretRequest) -> bool,
        close: impl FnOnce(serde_json::Value),
    ) -> Result<SecretLease, &'static str> {
        let receiver = self.register(&request).await?;
        if !emit(&request) {
            self.pending.lock().await.remove(&request.request_id);
            return Err("unavailable");
        }
        let outcome = tokio::select! {
            v = receiver => v.map_err(|_| "cancelled"),
            _ = cancel.cancelled() => Err("cancelled"),
            _ = tokio::time::sleep(Duration::from_secs(300)) => Err("timeout"),
        };
        self.pending.lock().await.remove(&request.request_id);
        close(serde_json::json!({"requestId":request.request_id,"chatId":request.chat_id}));
        outcome
    }
}

#[cfg(test)]
#[derive(Default)]
struct MemoryStore(std::sync::Mutex<HashMap<String, String>>);
#[cfg(test)]
impl SecretStore for MemoryStore {
    fn put(&self, name: &str, value: &str) -> Result<(), ()> {
        self.0.lock().unwrap().insert(name.into(), value.into());
        Ok(())
    }
    fn get(&self, name: &str) -> Result<Option<String>, ()> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }
    fn remove(&self, name: &str) -> Result<(), ()> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}
#[cfg(test)]
impl SecretBroker {
    pub(crate) fn in_memory() -> Self {
        Self {
            pending: Default::default(),
            store: Arc::new(MemoryStore::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct FakeStore(std::sync::Mutex<HashMap<String, String>>);
    impl SecretStore for FakeStore {
        fn put(&self, n: &str, v: &str) -> Result<(), ()> {
            self.0.lock().unwrap().insert(n.into(), v.into());
            Ok(())
        }
        fn get(&self, n: &str) -> Result<Option<String>, ()> {
            Ok(self.0.lock().unwrap().get(n).cloned())
        }
        fn remove(&self, n: &str) -> Result<(), ()> {
            self.0.lock().unwrap().remove(n);
            Ok(())
        }
    }
    #[tokio::test]
    async fn secret_is_only_in_secure_store_and_bound_to_chat() {
        let store = Arc::new(FakeStore::default());
        let broker = SecretBroker {
            pending: Default::default(),
            store: store.clone(),
        };
        let request = SecretRequest {
            request_id: "req".into(),
            chat_id: "room".into(),
            service: "cloudflare".into(),
            account: "acct".into(),
            resource_name: "momso-test".into(),
            fields: vec!["accessKeyId", "secretAccessKey"],
            reason: None,
        };
        let receiver = broker.register(&request).await.unwrap();
        let secret = "unique-test-secret-1";
        let input = || SecretInput {
            access_key_id: "test-id".into(),
            secret_access_key: secret.into(),
            gcp_oauth_token: None,
        };
        assert!(broker.submit("req", "other", input()).await.is_err());
        assert!(!serde_json::to_string(&request).unwrap().contains(secret));
        broker.submit("req", "room", input()).await.unwrap();
        let lease = receiver.await.unwrap();
        assert_eq!(lease.take().unwrap().secret_access_key, secret);
        assert!(broker.submit("req", "room", input()).await.is_err());
        drop(lease);
        assert!(store.0.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn cancelled_receiver_does_not_leave_a_secret() {
        let store = Arc::new(FakeStore::default());
        let broker = SecretBroker {
            pending: Default::default(),
            store: store.clone(),
        };
        let request = SecretRequest {
            request_id: "cancel".into(),
            chat_id: "room".into(),
            service: "aws".into(),
            account: "a".into(),
            resource_name: "momso-test".into(),
            fields: vec!["accessKeyId", "secretAccessKey"],
            reason: None,
        };
        drop(broker.register(&request).await.unwrap());
        assert_eq!(
            broker
                .submit(
                    "cancel",
                    "room",
                    SecretInput {
                        access_key_id: "id".into(),
                        secret_access_key: "secret".into(),
                        gcp_oauth_token: None
                    }
                )
                .await,
            Err("cancelled")
        );
        assert!(store.0.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn failed_secure_store_write_cleans_partial_value() {
        struct FailingStore(std::sync::Mutex<HashMap<String, String>>);
        impl SecretStore for FailingStore {
            fn put(&self, name: &str, value: &str) -> Result<(), ()> {
                self.0.lock().unwrap().insert(name.into(), value.into());
                Err(())
            }
            fn get(&self, _: &str) -> Result<Option<String>, ()> {
                Ok(None)
            }
            fn remove(&self, name: &str) -> Result<(), ()> {
                self.0.lock().unwrap().remove(name);
                Ok(())
            }
        }
        let store = Arc::new(FailingStore(std::sync::Mutex::new(HashMap::new())));
        let broker = SecretBroker {
            pending: Default::default(),
            store: store.clone(),
        };
        let request = SecretRequest {
            request_id: "fail".into(),
            chat_id: "room".into(),
            service: "aws".into(),
            account: "a".into(),
            resource_name: "momso-test".into(),
            fields: vec!["accessKeyId", "secretAccessKey"],
            reason: None,
        };
        let _receiver = broker.register(&request).await.unwrap();
        assert_eq!(
            broker
                .submit(
                    "fail",
                    "room",
                    SecretInput {
                        access_key_id: "id".into(),
                        secret_access_key: "secret".into(),
                        gcp_oauth_token: None
                    }
                )
                .await,
            Err("unavailable")
        );
        assert!(store.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancellation_closes_only_the_requested_entry() {
        let store = Arc::new(FakeStore::default());
        let broker = SecretBroker {
            pending: Default::default(),
            store: store.clone(),
        };
        let request = |id: &str| SecretRequest {
            request_id: id.into(),
            chat_id: "room".into(),
            service: "cloudflare".into(),
            account: "account".into(),
            resource_name: "momso-test".into(),
            fields: vec!["accessKeyId", "secretAccessKey"],
            reason: Some("invalid_credentials"),
        };
        let cancelled = broker.register(&request("one")).await.unwrap();
        let other = broker.register(&request("two")).await.unwrap();
        let serialized = serde_json::to_value(request("one")).unwrap();
        assert_eq!(serialized["reason"], "invalid_credentials");
        assert_eq!(broker.cancel("one", "other").await, Err("not_found"));
        assert!(broker.pending.lock().await.contains_key("one"));
        assert!(store.0.lock().unwrap().is_empty());
        broker.cancel("one", "room").await.unwrap();
        assert!(cancelled.await.is_err());
        assert_eq!(broker.cancel("one", "room").await, Ok(()));
        broker.cancel("two", "room").await.unwrap();
        assert!(other.await.is_err());
        assert!(store.0.lock().unwrap().is_empty());
    }
}
