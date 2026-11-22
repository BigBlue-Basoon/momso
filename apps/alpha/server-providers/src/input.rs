//! Host-only adapter for the S32 input-request lifecycle.
use crate::{ApiKey, Provider};
use serde_json::{json, Value};

/// Implemented by the server's authenticated request broker; a model cannot
/// call this interface or answer its own request.
pub trait SecretInput {
    fn ask(&mut self, request: Value) -> Result<Option<String>, &'static str>;
}

pub fn request_key(
    broker: &mut impl SecretInput,
    request_id: &str,
    chat_id: &str,
    turn_id: &str,
    provider: Provider,
    expires_at: &str,
) -> Result<Option<ApiKey>, &'static str> {
    let request = json!({"requestId":request_id,"chatId":chat_id,"turnId":turn_id,
        "kind":"provider_api_key","origin":null,"provider":provider.name(),
        "expiresAt":expires_at,"fields":[{"id":"apiKey","secret":true}]});
    broker.ask(request)?.map(ApiKey::new).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake;
    impl SecretInput for Fake {
        fn ask(&mut self, request: Value) -> Result<Option<String>, &'static str> {
            assert_eq!(request["kind"], "provider_api_key");
            assert_eq!(request["fields"][0]["secret"], true);
            assert!(!request.to_string().contains("fake-key"));
            Ok(Some("fake-key".into()))
        }
    }
    #[test]
    fn request_uses_s32_secret_field_without_echoing_value() {
        assert_eq!(
            request_key(&mut Fake, "r", "c", "t", Provider::Gemini, "later")
                .unwrap()
                .unwrap()
                .expose_to_transport(),
            "fake-key"
        );
    }
}
