//! Host-side provider credentials and event handling; not an HTTP server.
//! Provider keys must never be exposed as AI tools or passed in command arguments.
use serde::Serialize;
use zeroize::Zeroizing;

pub mod input;
pub mod models;
pub mod tool_loop;
pub mod transport;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Claude,
    Gemini,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Gemini => "gemini",
        }
    }
}

/// Host supplies the authenticated human's secret. Never implement Debug/Serialize.
pub struct ApiKey(Zeroizing<String>);

impl ApiKey {
    pub fn new(value: String) -> Result<Self, &'static str> {
        if value.is_empty() || value.len() > 4096 || value.contains(['\r', '\n', '\0']) {
            return Err("invalid_key");
        }
        Ok(Self(Zeroizing::new(value)))
    }

    /// Host transport only; never return this value as a tool result or event.
    pub fn expose_to_transport(&self) -> &str {
        &self.0
    }
}

#[cfg(target_os = "linux")]
pub mod vault {
    use super::{ApiKey, Provider};
    use momso_saved_login_core::{backend::CredentialBackend, linux::LinuxVault};
    use std::path::Path;

    /// Separate directories are essential: S21 only supports its saved-logins key.
    /// This constructor is host-only, and uses the systemd-supplied age identity.
    pub fn open(root: &Path, provider: Provider) -> Result<LinuxVault, &'static str> {
        LinuxVault::from_systemd(&root.join(provider.name()))
    }

    pub fn save(vault: &LinuxVault, key: &ApiKey) -> Result<(), &'static str> {
        vault.put("saved-logins", key.expose_to_transport())
    }

    pub fn load(vault: &LinuxVault) -> Result<Option<ApiKey>, &'static str> {
        vault.get("saved-logins")?.map(ApiKey::new).transpose()
    }

    pub fn clear(vault: &LinuxVault) -> Result<(), &'static str> {
        vault.remove("saved-logins")
    }
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(tag = "event", content = "data")]
pub enum ChatEvent<'a> {
    #[serde(rename = "chat-delta")]
    Delta {
        #[serde(rename = "turnId")]
        turn_id: &'a str,
        #[serde(rename = "chatId")]
        chat_id: &'a str,
        delta: String,
        offset: usize,
    },
    #[serde(rename = "chat-done")]
    Done {
        #[serde(rename = "turnId")]
        turn_id: &'a str,
        #[serde(rename = "chatId")]
        chat_id: &'a str,
        state: &'static str,
        error: Option<&'static str>,
    },
}

/// Buffer until completion so a secret split across provider chunks cannot leak.
/// The host must also suppress raw CLI/API output and provider error bodies.
pub struct SafeReply {
    text: Zeroizing<String>,
    secret: ApiKey,
}

impl SafeReply {
    pub fn new(secret: ApiKey) -> Self {
        Self {
            text: Zeroizing::new(String::new()),
            secret,
        }
    }

    pub fn push(&mut self, text: &str) -> Result<(), &'static str> {
        if self.text.len().saturating_add(text.len()) > 4 * 1024 * 1024 {
            return Err("response_too_large");
        }
        self.text.push_str(text);
        Ok(())
    }

    pub fn finish(&self, turn: &str, chat: &str, cancelled: bool) -> Vec<serde_json::Value> {
        let sensitive = self.text.contains(self.secret.expose_to_transport());
        let safe = if sensitive {
            String::new()
        } else {
            self.text.to_string()
        };
        let mut events = Vec::new();
        if !safe.is_empty() {
            events.push(
                serde_json::to_value(ChatEvent::Delta {
                    turn_id: turn,
                    chat_id: chat,
                    offset: safe.len(),
                    delta: safe,
                })
                .expect("fixed event is serializable"),
            );
        }
        events.push(
            serde_json::to_value(ChatEvent::Done {
                turn_id: turn,
                chat_id: chat,
                state: if cancelled || sensitive {
                    "failed"
                } else {
                    "completed"
                },
                error: if cancelled {
                    Some("cancelled")
                } else if sensitive {
                    Some("sensitive_output")
                } else {
                    None
                },
            })
            .expect("fixed event is serializable"),
        );
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_secret_is_redacted_before_any_event_and_offset_is_bytes() {
        let mut reply = SafeReply::new(ApiKey::new("secret-value".into()).unwrap());
        reply.push("안녕 secret-").unwrap();
        reply.push("value!").unwrap();
        let events = reply.finish("turn", "chat", false);
        assert!(!serde_json::to_string(&events)
            .unwrap()
            .contains("secret-value"));
        assert_eq!(events[0]["data"]["error"], "sensitive_output");
    }

    #[test]
    fn cancelled_turn_keeps_safe_partial_text() {
        let mut reply = SafeReply::new(ApiKey::new("test-key".into()).unwrap());
        reply.push("partial").unwrap();
        let events = reply.finish("turn", "chat", true);
        assert_eq!(events[1]["data"]["state"], "failed");
        assert_eq!(events[1]["data"]["error"], "cancelled");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dedicated_vaults_keep_provider_keys_separate() {
        use age::x25519;
        use momso_saved_login_core::linux::LinuxVault;
        let dir = tempfile::tempdir().unwrap();
        let claude =
            LinuxVault::new(&dir.path().join("claude"), x25519::Identity::generate()).unwrap();
        let gemini =
            LinuxVault::new(&dir.path().join("gemini"), x25519::Identity::generate()).unwrap();
        vault::save(&claude, &ApiKey::new("fake-claude".into()).unwrap()).unwrap();
        vault::save(&gemini, &ApiKey::new("fake-gemini".into()).unwrap()).unwrap();
        assert_eq!(
            vault::load(&claude).unwrap().unwrap().expose_to_transport(),
            "fake-claude"
        );
        assert_eq!(
            vault::load(&gemini).unwrap().unwrap().expose_to_transport(),
            "fake-gemini"
        );
        let bytes = std::fs::read(dir.path().join("claude/vault.age")).unwrap();
        assert!(!bytes
            .windows(b"fake-claude".len())
            .any(|w| w == b"fake-claude"));
    }
}
